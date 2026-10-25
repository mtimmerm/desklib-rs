// desklib: remote desktop over any async stream.
// Copyright (C) 2026 Matt Timmermans
//
// Derived from RustDesk (https://github.com/rustdesk/rustdesk),
// Copyright (C) Purslane Tech Pte. Ltd. and the RustDesk contributors. See NOTICE.md.
//
// This program is free software: you can redistribute it and/or modify it under the terms of
// the GNU Affero General Public License, version 3, as published by the Free Software
// Foundation. It is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY;
// without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
// See the GNU Affero General Public License for more details. You should have received a copy
// of it with this program (LICENSE); if not, see <https://www.gnu.org/licenses/>.

//! End-to-end tests against the real desktop.

use crate::client::session::{self, Event};
use crate::proto::{Message, MouseMove};
use crate::{Server, ServerConfig};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

/// Runs a server and the client's network side against each other and checks that video
/// decodes and the cursor arrives. Needs an interactive desktop, so it is opt-in:
/// `cargo test --release -- --ignored`.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn server_to_client_session() {
    let (server_end, client_end) = tokio::io::duplex(1 << 20);
    let server = tokio::spawn(async move {
        Server::new(ServerConfig::default()).run_stream(server_end).await
    });

    let (reader, writer) = tokio::io::split(client_end);
    let mailbox: session::Mailbox = Arc::new(Mutex::new(None));
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    tokio::spawn(session::run(reader, writer, input_rx, mailbox.clone(), move |e| {
        let _ = event_tx.send(e);
    }));

    let (mut frames, mut cursor_shape, mut cursor_pos, mut info) = (0, false, None, false);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while frames < 1 || !cursor_shape || cursor_pos.is_none() || !info {
        let event = tokio::time::timeout_at(deadline, event_rx.recv())
            .await
            .expect("timed out waiting for the session")
            .expect("session ended");
        match event {
            Event::Frame => {
                let frame = mailbox.lock().unwrap().take().unwrap();
                assert!(frame.width > 0 && frame.height > 0);
                assert_eq!(frame.pixels.len(), frame.width * frame.height);
                frames += 1;
            }
            Event::CursorShape(c) => {
                assert_eq!(c.rgba.len(), (c.width * c.height * 4) as usize);
                cursor_shape = true;
            }
            Event::CursorPosition(x, y) => cursor_pos = Some((x, y)),
            // The AGPL source offer comes first.
            Event::ServerInfo { source_url, notice } => {
                assert_eq!(frames, 0, "server info must precede video");
                assert_eq!(source_url, crate::SOURCE_URL);
                assert_eq!(notice, crate::LEGAL_NOTICE);
                info = true;
            }
            Event::Closed(r) => panic!("session closed: {r:?}"),
            _ => {}
        }
    }

    // Input flows the other way without upsetting the session. Moving the pointer to where
    // it already is keeps the test from disturbing whoever is at the desk.
    let (x, y) = cursor_pos.unwrap();
    let mut msg = Message::new();
    msg.set_mouse_move(MouseMove { x, y, ..Default::default() });
    input_tx.send(msg).unwrap();

    // Closing the client side ends the server cleanly.
    drop(input_tx);
    let result = tokio::time::timeout(Duration::from_secs(10), server).await;
    result.expect("server did not stop").unwrap().unwrap();
}
