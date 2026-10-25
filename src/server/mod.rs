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

//! Shares the local desktop with one peer per [`Server::run`] call.

mod cursor;
mod input;
mod video;

use crate::codec::EncoderSettings;
use crate::proto::{Message, ServerInfo, Union};
use crate::transport::{MessageReader, MessageWriter};
use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

/// Server settings.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Index of the display to share, in the platform's enumeration order.
    pub display: usize,
    /// Upper bound on frames per second. Frames are only sent when the screen changes.
    pub max_fps: u32,
    /// rav1e speed preset, 0 (slowest) to 10 (fastest).
    pub speed: u8,
    /// Constant quantizer, 0 (best quality) to 255 (smallest).
    pub quantizer: u8,
    /// Encoder threads; 0 uses one per CPU.
    pub encoder_threads: usize,
    /// Where users can get the source code of this server, sent to every client that connects.
    ///
    /// The GNU AGPL (section 13) requires a modified version that users interact with over a
    /// network to offer them its source code. If you modify desklib, set this to where the
    /// source of your modified version can be obtained.
    pub source_url: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        let enc = EncoderSettings::default();
        Self {
            display: 0,
            max_fps: 30,
            speed: enc.speed,
            quantizer: enc.quantizer,
            encoder_threads: enc.threads,
            source_url: crate::SOURCE_URL.into(),
        }
    }
}

impl ServerConfig {
    fn encoder_settings(&self) -> EncoderSettings {
        EncoderSettings {
            speed: self.speed,
            quantizer: self.quantizer,
            threads: self.encoder_threads,
        }
    }
}

/// Sends a view of the local desktop over a stream and replays the input events received
/// from it.
///
/// ```no_run
/// # async fn f() -> anyhow::Result<()> {
/// let listener = tokio::net::TcpListener::bind("0.0.0.0:21118").await?;
/// let (stream, _) = listener.accept().await?;
/// let (reader, writer) = stream.into_split();
/// desklib::Server::new(Default::default()).run(reader, writer).await
/// # }
/// ```
pub struct Server {
    config: ServerConfig,
}

/// Where the shared display sits on the desktop, for mapping frame pixels to input
/// coordinates and back.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DisplayRect {
    /// Origin in the coordinate space that input injection and cursor positions use.
    pub x: i32,
    pub y: i32,
    /// Frame pixels per input coordinate unit (above 1 on HiDPI macOS).
    pub scale: f64,
}

impl DisplayRect {
    pub fn to_desktop(&self, x: i32, y: i32) -> (i32, i32) {
        (
            self.x + (x as f64 / self.scale).round() as i32,
            self.y + (y as f64 / self.scale).round() as i32,
        )
    }

    pub fn to_frame(&self, x: i32, y: i32) -> (i32, i32) {
        (
            ((x - self.x) as f64 * self.scale).round() as i32,
            ((y - self.y) as f64 * self.scale).round() as i32,
        )
    }
}

/// State shared between a session's threads.
pub(crate) struct Shared {
    stopped: AtomicBool,
    key_frame_requested: AtomicBool,
    rect: Mutex<DisplayRect>,
    error: Mutex<Option<anyhow::Error>>,
}

impl Shared {
    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    pub fn take_key_frame_request(&self) -> bool {
        self.key_frame_requested.swap(false, Ordering::Relaxed)
    }

    pub fn rect(&self) -> DisplayRect {
        *self.rect.lock().unwrap()
    }

    pub fn set_rect(&self, rect: DisplayRect) {
        *self.rect.lock().unwrap() = rect;
    }

    /// Records why a worker thread gave up; the session then ends with this error.
    pub fn fail(&self, err: anyhow::Error) {
        self.error.lock().unwrap().get_or_insert(err);
    }
}

/// Stops the session's threads however `run` exits, including by being cancelled.
struct StopOnDrop(Arc<Shared>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.stopped.store(true, Ordering::Relaxed);
    }
}

impl Server {
    pub fn new(config: ServerConfig) -> Self {
        Self { config }
    }

    /// Serves one peer until it disconnects or an error occurs. Returns `Ok` when the peer
    /// closes the stream.
    pub async fn run<R, W>(&self, reader: R, writer: W) -> Result<()>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        #[cfg(windows)]
        set_dpi_aware();
        log::info!("{} Source: {}", crate::LEGAL_NOTICE, self.config.source_url);

        let shared = Arc::new(Shared {
            stopped: AtomicBool::new(false),
            key_frame_requested: AtomicBool::new(false),
            rect: Mutex::new(DisplayRect { x: 0, y: 0, scale: 1.0 }),
            error: Mutex::new(None),
        });
        let _stop = StopOnDrop(shared.clone());

        // Small, so a slow peer throttles capture rather than queueing stale frames.
        let (out_tx, mut out_rx) = mpsc::channel::<Message>(4);
        video::spawn(self.config.clone(), shared.clone(), out_tx.clone())?;
        cursor::spawn(shared.clone(), out_tx);
        let input = input::spawn(shared.clone())?;

        let mut reader = MessageReader::new(reader);
        let mut writer = MessageWriter::new(writer);

        let mut info = Message::new();
        info.set_server_info(ServerInfo {
            source_url: self.config.source_url.clone(),
            notice: crate::LEGAL_NOTICE.into(),
            ..Default::default()
        });
        writer.send(&info).await?;

        loop {
            tokio::select! {
                msg = reader.next() => {
                    let Some(msg) = msg? else { return Ok(()) };
                    match msg.union {
                        Some(Union::RefreshVideo(_)) => {
                            shared.key_frame_requested.store(true, Ordering::Relaxed);
                        }
                        Some(other) => {
                            if let Some(event) = input::InputEvent::from_message(other) {
                                // The input thread only stops once the session is over.
                                let _ = input.send(event);
                            }
                        }
                        None => {}
                    }
                }
                msg = out_rx.recv() => {
                    match msg {
                        Some(msg) => writer.send(&msg).await?,
                        // Every producer has exited, which they only do on error.
                        None => {
                            return Err(shared.error.lock().unwrap().take()
                                .unwrap_or_else(|| anyhow!("capture stopped")));
                        }
                    }
                }
            }
        }
    }

    /// Like [`run`](Self::run), for a stream that is both readable and writable.
    pub async fn run_stream<S>(&self, stream: S) -> Result<()>
    where
        S: AsyncRead + AsyncWrite,
    {
        let (reader, writer) = tokio::io::split(stream);
        self.run(reader, writer).await
    }
}

/// Capture, cursor positions and injected input all need to agree on physical pixels.
#[cfg(windows)]
fn set_dpi_aware() {
    use winapi::um::shellscalingapi::{SetProcessDpiAwareness, PROCESS_PER_MONITOR_DPI_AWARE};
    // Fails harmlessly if the process has already chosen its awareness.
    unsafe { SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE) };
}
