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

//! AV1 video: rav1e to encode, rav1d to decode.

mod decoder;
mod encoder;
mod yuv;

pub use decoder::{DecodedFrame, Decoder};
pub use encoder::{EncodedFrame, Encoder, EncoderSettings};

#[cfg(test)]
mod tests {
    use super::*;

    /// A moving gradient, so consecutive frames differ.
    fn frame(w: usize, h: usize, t: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                v.extend_from_slice(&[(x + t) as u8, (y * 2) as u8, ((x + y) / 2) as u8, 255]);
            }
        }
        v
    }

    #[test]
    fn every_frame_decodes_without_delay() {
        let (w, h) = (128, 72);
        let mut enc = Encoder::new(w, h, &EncoderSettings::default()).unwrap();
        let mut dec = Decoder::new().unwrap();
        let mut out = DecodedFrame { width: 0, height: 0, pixels: Vec::new() };
        for t in 0..10 {
            let src = frame(w, h, t * 3);
            let packets = enc.encode(&src, w * 4, t == 5).unwrap();
            assert_eq!(packets.len(), 1, "frame {t} was not output immediately");
            assert_eq!(packets[0].key, t == 0 || t == 5, "frame {t}");
            assert!(dec.decode(&packets[0].data, &mut out).unwrap(), "frame {t} not decoded");
            assert_eq!((out.width, out.height), (w, h));

            // Lossy, but each channel should be in the right neighbourhood on average.
            let mut err = 0u64;
            for (px, s) in out.pixels.iter().zip(src.chunks_exact(4)) {
                let got = [(*px >> 16) as u8, (*px >> 8) as u8, *px as u8];
                for (a, b) in got.iter().zip([s[2], s[1], s[0]]) {
                    err += (*a as i64 - b as i64).unsigned_abs();
                }
            }
            let mean = err as f64 / (w * h * 3) as f64;
            assert!(mean < 8.0, "frame {t}: mean error {mean}");
        }
    }

    #[test]
    fn odd_sizes() {
        let (w, h) = (33, 17);
        let mut enc = Encoder::new(w, h, &EncoderSettings::default()).unwrap();
        let mut dec = Decoder::new().unwrap();
        let mut out = DecodedFrame { width: 0, height: 0, pixels: Vec::new() };
        let packets = enc.encode(&frame(w, h, 0), w * 4, false).unwrap();
        assert!(dec.decode(&packets[0].data, &mut out).unwrap());
        assert_eq!((out.width, out.height), (w, h));
    }
}

/// Times the codec on a screenshot of the primary display. Opt-in, and only meaningful in
/// release builds: `cargo test --release [--no-default-features] -- --ignored --nocapture bench`.
#[cfg(test)]
mod bench {
    use super::*;
    use scrap::{Capturer, Display, Frame, TraitCapturer, TraitPixelBuffer};
    use std::time::{Duration, Instant};

    fn screenshot() -> (Vec<u8>, usize, usize) {
        let display = Display::all().unwrap().swap_remove(0);
        let (w, h) = (display.width(), display.height());
        let mut capturer = Capturer::new(display).unwrap();
        loop {
            if let Ok(Frame::PixelBuffer(f)) = capturer.frame(Duration::from_millis(200)) {
                let stride = f.stride()[0];
                let mut packed = Vec::with_capacity(w * h * 4);
                for row in f.data().chunks(stride).take(h) {
                    packed.extend_from_slice(&row[..w * 4]);
                }
                return (packed, w, h);
            }
        }
    }

    #[test]
    #[ignore]
    fn bench() {
        let (mut img, w, h) = screenshot();
        let mut enc = Encoder::new(w, h, &EncoderSettings::default()).unwrap();
        let mut dec = Decoder::new().unwrap();
        let mut out = DecodedFrame { width: 0, height: 0, pixels: Vec::new() };
        let mut run = |img: &[u8]| {
            let t = Instant::now();
            let packets = enc.encode(img, w * 4, false).unwrap();
            let encode = t.elapsed();
            let t = Instant::now();
            for p in &packets {
                dec.decode(&p.data, &mut out).unwrap();
            }
            let bytes: usize = packets.iter().map(|p| p.data.len()).sum();
            (encode, t.elapsed(), bytes)
        };
        let report = |name: &str, samples: &[(Duration, Duration, usize)]| {
            let n = samples.len() as u32;
            let enc: Duration = samples.iter().map(|s| s.0).sum::<Duration>() / n;
            let dec: Duration = samples.iter().map(|s| s.1).sum::<Duration>() / n;
            let bytes = samples.iter().map(|s| s.2).sum::<usize>() / n as usize;
            println!("{name:10} encode {:6.1} ms   decode {:5.1} ms   {bytes:7} bytes", enc.as_secs_f64() * 1e3, dec.as_secs_f64() * 1e3);
        };

        println!("{w}x{h}, asm: {}", cfg!(feature = "asm"));
        report("key frame", &[run(&img)]);
        let typing: Vec<_> = (0..6)
            .map(|i| {
                // A character-sized dark box appearing per frame, like typing.
                for y in 300..320 {
                    for x in (300 + i * 12)..(310 + i * 12) {
                        img[(y * w + x) * 4..][..3].copy_from_slice(&[20, 20, 20]);
                    }
                }
                run(&img)
            })
            .collect();
        report("typing", &typing);
        let scrolling: Vec<_> = (0..6)
            .map(|_| {
                img.copy_within(40 * w * 4.., 0);
                run(&img)
            })
            .collect();
        report("scrolling", &scrolling);
        let unchanged: Vec<_> = (0..3).map(|_| run(&img)).collect();
        report("unchanged", &unchanged);
    }
}
