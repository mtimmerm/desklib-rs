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

//! Capture and encode loop, on its own thread since capturers are not `Send` on every
//! platform and encoding is CPU-bound.

use super::{DisplayRect, ServerConfig, Shared};
use crate::codec::{EncodedFrame, Encoder};
use crate::proto::{Message, VideoFrame};
use anyhow::{anyhow, bail, Context, Result};
use scrap::{Capturer, Display, Frame, Pixfmt, TraitCapturer, TraitPixelBuffer};
use std::io::ErrorKind;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// How long one capture call may block, which bounds how quickly the thread notices the
/// session ending.
const CAPTURE_TIMEOUT: Duration = Duration::from_millis(100);

pub fn spawn(config: ServerConfig, shared: Arc<Shared>, tx: mpsc::Sender<Message>) -> Result<()> {
    std::thread::Builder::new()
        .name("desk-video".into())
        .spawn(move || {
            if let Err(e) = run(&config, &shared, &tx) {
                log::error!("video: {e:#}");
                shared.fail(e);
            }
        })?;
    Ok(())
}

fn open_display(index: usize) -> Result<Display> {
    let mut displays = Display::all().context("failed to enumerate displays")?;
    if index >= displays.len() {
        bail!("display {index} does not exist ({} found)", displays.len());
    }
    Ok(displays.swap_remove(index))
}

fn display_rect(display: &Display) -> DisplayRect {
    let (x, y) = display.origin();
    #[cfg(target_os = "macos")]
    let scale = display.scale();
    #[cfg(not(target_os = "macos"))]
    let scale = 1.0;
    DisplayRect { x, y, scale }
}

/// The last frame encoded, kept to answer key frame requests while the screen is static.
struct LastFrame {
    pixels: Vec<u8>,
    stride: usize,
}

fn run(config: &ServerConfig, shared: &Shared, tx: &mpsc::Sender<Message>) -> Result<()> {
    let interval = Duration::from_secs(1) / config.max_fps.max(1);
    let settings = config.encoder_settings();
    let mut encoder: Option<Encoder> = None;
    let mut last = LastFrame { pixels: Vec::new(), stride: 0 };
    let mut failures = 0;

    while !shared.stopped() {
        let display = open_display(config.display)?;
        shared.set_rect(display_rect(&display));
        let mut capturer = match Capturer::new(display) {
            Ok(c) => c,
            Err(e) => {
                // E.g. the secure desktop is showing; keep retrying for a while.
                failures += 1;
                if failures > 50 {
                    return Err(e).context("failed to start capture");
                }
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
        };
        let mut next_frame_at = Instant::now();

        loop {
            if shared.stopped() {
                return Ok(());
            }
            let now = Instant::now();
            if now < next_frame_at {
                std::thread::sleep(next_frame_at - now);
            }
            let packets = match capturer.frame(CAPTURE_TIMEOUT) {
                Ok(Frame::PixelBuffer(frame)) => {
                    failures = 0;
                    next_frame_at = Instant::now() + interval;
                    let (w, h) = (frame.width(), frame.height());
                    if w == 0 || h == 0 {
                        continue;
                    }
                    let enc = match &mut encoder {
                        Some(enc) if enc.size() == (w, h) => enc,
                        // A new encoder starts with a key frame, carrying the new size.
                        _ => encoder.insert(Encoder::new(w, h, &settings)?),
                    };
                    let stride = frame.stride()[0];
                    last.stride = stride;
                    last.pixels.clear();
                    match frame.pixfmt() {
                        Pixfmt::BGRA => last.pixels.extend_from_slice(&frame.data()[..stride * h]),
                        Pixfmt::RGBA => {
                            last.pixels.extend(
                                frame.data()[..stride * h]
                                    .chunks_exact(4)
                                    .flat_map(|p| [p[2], p[1], p[0], p[3]]),
                            );
                        }
                        other => bail!("unsupported capture format {other:?}"),
                    }
                    enc.encode(&last.pixels, stride, shared.take_key_frame_request())?
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    // Unchanged; only re-encode if the client asked for a key frame.
                    match &mut encoder {
                        Some(enc) if !last.pixels.is_empty() && shared.take_key_frame_request() => {
                            enc.encode(&last.pixels, last.stride, true)?
                        }
                        _ => continue,
                    }
                }
                Err(e) => {
                    // Typically a mode change or desktop switch; start over with a fresh
                    // capturer.
                    log::debug!("capture failed, restarting: {e}");
                    failures += 1;
                    if failures > 50 {
                        return Err(anyhow!(e).context("capture keeps failing"));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                    break;
                }
            };
            for EncodedFrame { data, key } in packets {
                let mut msg = Message::new();
                msg.set_video_frame(VideoFrame {
                    data: data.into(),
                    key,
                    ..Default::default()
                });
                if tx.blocking_send(msg).is_err() {
                    // The session is over.
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}
