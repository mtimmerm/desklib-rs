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

//! The client's network side: reads server messages, decodes video on its own thread and
//! forwards local input.

use crate::codec::{DecodedFrame, Decoder};
use crate::proto::{Message, RefreshVideo, Union};
use crate::handshake::{self, ServerConfig, ServerDetails};
use crate::transport::{MessageReader, MessageWriter};
use anyhow::{bail, Context, Result};
use bytes::Bytes;
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

/// A cursor image, RGBA with straight alpha.
pub struct CursorImage {
    pub id: u64,
    pub hotx: i32,
    pub hoty: i32,
    pub width: i32,
    pub height: i32,
    pub rgba: Vec<u8>,
}

pub enum Event {
    /// The server's version, source offer and license notice.
    ServerInfo(ServerDetails),
    /// A new frame is waiting in the mailbox.
    Frame,
    CursorShape(CursorImage),
    CursorId(u64),
    CursorHidden,
    CursorPosition(i32, i32),
    /// The session ended; `Ok` if the server closed the stream.
    Closed(Result<()>),
}

/// The latest decoded frame. Frames the UI hasn't drawn yet are replaced, not queued.
pub type Mailbox = Arc<Mutex<Option<DecodedFrame>>>;

pub async fn run<R, W>(
    reader: R,
    writer: W,
    config: ServerConfig,
    mut input: mpsc::UnboundedReceiver<Message>,
    mailbox: Mailbox,
    events: impl Fn(Event) + Clone + Send + 'static,
) where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let result = serve(reader, writer, &config, &mut input, mailbox, events.clone()).await;
    events(Event::Closed(result));
}

async fn serve<R, W>(
    reader: R,
    writer: W,
    config: &ServerConfig,
    input: &mut mpsc::UnboundedReceiver<Message>,
    mailbox: Mailbox,
    events: impl Fn(Event) + Clone + Send + 'static,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut reader = MessageReader::new(reader);
    let mut writer = MessageWriter::new(writer);
    let server = handshake::client(&mut reader, &mut writer, config).await?;
    events(Event::ServerInfo(server));
    let (refresh_tx, mut refresh_rx) = mpsc::unbounded_channel();
    let video = spawn_decoder(mailbox, events.clone(), refresh_tx)?;

    loop {
        tokio::select! {
            msg = reader.next() => {
                let Some(msg) = msg? else { return Ok(()) };
                let event = match msg.union {
                    Some(Union::VideoFrame(f)) => {
                        // The decoder only stops if it failed to start.
                        video.send(f.data).context("video decoder stopped")?;
                        continue;
                    }
                    Some(Union::CursorData(c)) => Event::CursorShape(CursorImage {
                        id: c.id,
                        hotx: c.hotx,
                        hoty: c.hoty,
                        width: c.width,
                        height: c.height,
                        rgba: c.colors.to_vec(),
                    }),
                    Some(Union::SessionError(e)) => bail!("server: {}", e.message),
                    Some(Union::CursorId(id)) => Event::CursorId(id),
                    Some(Union::CursorHidden(_)) => Event::CursorHidden,
                    Some(Union::CursorPosition(p)) => Event::CursorPosition(p.x, p.y),
                    _ => continue,
                };
                events(event);
            }
            msg = input.recv() => match msg {
                Some(msg) => writer.send(&msg).await?,
                // The window was closed.
                None => return Ok(()),
            },
            Some(()) = refresh_rx.recv() => {
                let mut msg = Message::new();
                msg.set_refresh_video(RefreshVideo::new());
                writer.send(&msg).await?;
            }
        }
    }
}

fn spawn_decoder(
    mailbox: Mailbox,
    events: impl Fn(Event) + Send + 'static,
    refresh: mpsc::UnboundedSender<()>,
) -> Result<std_mpsc::Sender<Bytes>> {
    let (tx, rx) = std_mpsc::channel::<Bytes>();
    let mut decoder = Decoder::new()?;
    std::thread::Builder::new()
        .name("desk-decode".into())
        .spawn(move || {
            let mut frame = DecodedFrame { width: 0, height: 0, pixels: Vec::new() };
            // After an error, wait for a key frame rather than decoding garbage.
            let mut broken = false;
            while let Ok(data) = rx.recv() {
                match decoder.decode(&data, &mut frame) {
                    Ok(true) => {
                        broken = false;
                        let mut slot = mailbox.lock().unwrap();
                        let was_empty = slot.is_none();
                        frame = slot.replace(frame).unwrap_or(DecodedFrame {
                            width: 0,
                            height: 0,
                            pixels: Vec::new(),
                        });
                        drop(slot);
                        if was_empty {
                            events(Event::Frame);
                        }
                    }
                    Ok(false) => {}
                    Err(e) => {
                        if !broken {
                            log::warn!("failed to decode video, requesting a key frame: {e:#}");
                            broken = true;
                            let _ = refresh.send(());
                        }
                    }
                }
            }
        })?;
    Ok(tx)
}
