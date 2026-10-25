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

use super::yuv::{bgra_to_i420, I420};
use anyhow::{anyhow, bail, Result};
use rav1e::config::SpeedSettings;
use rav1e::prelude::*;

/// Encoder tuning; see [`crate::ServerConfig`].
#[derive(Debug, Clone)]
pub struct EncoderSettings {
    /// rav1e speed preset, 0 (slowest) to 10 (fastest).
    pub speed: u8,
    /// Constant quantizer, 0 (best) to 255 (worst).
    pub quantizer: u8,
    /// Encoder threads; 0 picks one per CPU.
    pub threads: usize,
}

impl Default for EncoderSettings {
    fn default() -> Self {
        Self {
            speed: 10,
            quantizer: 120,
            threads: 0,
        }
    }
}

pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub key: bool,
}

/// Real-time AV1 encoder: every input frame produces its packet immediately.
pub struct Encoder {
    ctx: Context<u8>,
    width: usize,
    height: usize,
    yuv: I420,
}

impl Encoder {
    pub fn new(width: usize, height: usize, settings: &EncoderSettings) -> Result<Self> {
        if width == 0 || height == 0 {
            bail!("invalid frame size {width}x{height}");
        }
        let threads = match settings.threads {
            0 => std::thread::available_parallelism().map_or(1, |n| n.get()),
            n => n,
        };
        let mut speed_settings = SpeedSettings::from_preset(settings.speed);
        // Both look ahead at future frames, which would hold back output.
        speed_settings.rdo_lookahead_frames = 1;
        speed_settings.scene_detection_mode = SceneDetectionSpeed::None;
        let enc = EncoderConfig {
            width,
            height,
            low_latency: true,
            quantizer: settings.quantizer as usize,
            // The stream is reliable, so key frames are only needed on request.
            min_key_frame_interval: 0,
            max_key_frame_interval: u16::MAX as u64,
            // Tiles are what rav1e parallelizes a single frame over.
            tiles: threads.min(64),
            speed_settings,
            ..EncoderConfig::with_speed_preset(settings.speed)
        };
        let mut ctx = Config::new()
            .with_encoder_config(enc)
            .with_threads(threads)
            .new_context()
            .map_err(|e| anyhow!("invalid encoder config: {e}"))?;
        ctx.set_zero_latency(true);
        Ok(Self {
            ctx,
            width,
            height,
            yuv: I420::new(),
        })
    }

    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// Encodes one BGRA frame of the size this encoder was created with.
    pub fn encode(&mut self, bgra: &[u8], stride: usize, force_key: bool) -> Result<Vec<EncodedFrame>> {
        bgra_to_i420(bgra, stride, self.width, self.height, &mut self.yuv);
        let mut frame = self.ctx.new_frame();
        let cw = self.yuv.chroma_width();
        frame.planes[0].copy_from_raw_u8(&self.yuv.y, self.width, 1);
        frame.planes[1].copy_from_raw_u8(&self.yuv.u, cw, 1);
        frame.planes[2].copy_from_raw_u8(&self.yuv.v, cw, 1);
        let params = force_key.then(|| FrameParameters {
            frame_type_override: FrameTypeOverride::Key,
            ..Default::default()
        });
        self.ctx
            .send_frame((frame, params))
            .map_err(|e| anyhow!("send_frame: {e}"))?;

        let mut out = Vec::new();
        loop {
            match self.ctx.receive_packet() {
                Ok(packet) => out.push(EncodedFrame {
                    key: packet.frame_type == FrameType::KEY,
                    data: packet.data,
                }),
                Err(EncoderStatus::Encoded) => continue,
                Err(EncoderStatus::NeedMoreData) => break,
                Err(e) => bail!("receive_packet: {e}"),
            }
        }
        Ok(out)
    }
}
