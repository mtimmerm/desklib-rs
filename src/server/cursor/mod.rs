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

//! Polls the cursor's shape and position and forwards changes to the peer.

#[cfg(target_os = "linux")]
#[path = "x11.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod platform;
#[cfg(windows)]
#[path = "windows.rs"]
mod platform;

use super::Shared;
use crate::proto::{CursorData, CursorPosition, Message};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const POLL_INTERVAL: Duration = Duration::from_millis(33);

/// A cursor image, RGBA with straight alpha.
pub struct CursorShape {
    pub hotx: i32,
    pub hoty: i32,
    pub width: i32,
    pub height: i32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Hidden,
    Shape(u64),
}

pub fn spawn(shared: Arc<Shared>, tx: mpsc::Sender<Message>) {
    let result = std::thread::Builder::new()
        .name("desk-cursor".into())
        .spawn(move || {
            let mut poller = match platform::Poller::new() {
                Ok(p) => p,
                Err(e) => {
                    // The session still works without a remote cursor.
                    log::warn!("cursor capture unavailable: {e:#}");
                    return;
                }
            };
            run(&mut poller, &shared, &tx);
        });
    if let Err(e) = result {
        log::warn!("failed to start cursor thread: {e}");
    }
}

fn run(poller: &mut platform::Poller, shared: &Shared, tx: &mpsc::Sender<Message>) {
    let mut sent_shapes = HashSet::new();
    let mut last_state = None;
    let mut last_pos = None;

    while !shared.stopped() {
        // Messages are only marked sent once queued, so anything dropped because the queue
        // is full (behind video) is retried on the next poll.
        let state = match poller.current() {
            Some(id) => State::Shape(id),
            None => State::Hidden,
        };
        if last_state != Some(state) {
            let mut msg = Message::new();
            let new_shape = match state {
                State::Hidden => {
                    msg.set_cursor_hidden(true);
                    None
                }
                State::Shape(id) if sent_shapes.contains(&id) => {
                    msg.set_cursor_id(id);
                    None
                }
                State::Shape(id) => match poller.shape(id) {
                    Ok(shape) => {
                        msg.set_cursor_data(CursorData {
                            id,
                            hotx: shape.hotx,
                            hoty: shape.hoty,
                            width: shape.width,
                            height: shape.height,
                            colors: shape.rgba.into(),
                            ..Default::default()
                        });
                        Some(id)
                    }
                    Err(e) => {
                        log::debug!("failed to read cursor shape: {e:#}");
                        std::thread::sleep(POLL_INTERVAL);
                        continue;
                    }
                },
            };
            match tx.try_send(msg) {
                Ok(()) => {
                    last_state = Some(state);
                    sent_shapes.extend(new_shape);
                }
                Err(mpsc::error::TrySendError::Closed(_)) => return,
                Err(mpsc::error::TrySendError::Full(_)) => {}
            }
        }

        if let Some((x, y)) = poller.position() {
            let pos = shared.rect().to_frame(x, y);
            if last_pos != Some(pos) {
                let mut msg = Message::new();
                msg.set_cursor_position(CursorPosition {
                    x: pos.0,
                    y: pos.1,
                    ..Default::default()
                });
                match tx.try_send(msg) {
                    Ok(()) => last_pos = Some(pos),
                    Err(mpsc::error::TrySendError::Closed(_)) => return,
                    Err(mpsc::error::TrySendError::Full(_)) => {}
                }
            }
        }

        std::thread::sleep(POLL_INTERVAL);
    }
}
