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

//! BT.601 limited-range conversion between packed 32-bit pixels and planar I420.

/// Planar 4:2:0 image; chroma planes are `ceil(w/2) x ceil(h/2)`.
pub struct I420 {
    pub width: usize,
    pub height: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl I420 {
    pub fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            y: Vec::new(),
            u: Vec::new(),
            v: Vec::new(),
        }
    }

    pub fn chroma_width(&self) -> usize {
        (self.width + 1) / 2
    }

    pub fn chroma_height(&self) -> usize {
        (self.height + 1) / 2
    }

    fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.y.resize(width * height, 0);
        let chroma = self.chroma_width() * self.chroma_height();
        self.u.resize(chroma, 0);
        self.v.resize(chroma, 0);
    }
}

#[inline]
fn luma(r: i32, g: i32, b: i32) -> u8 {
    ((66 * r + 129 * g + 25 * b + 128) >> 8) as u8 + 16
}

/// Fills `out` from BGRA (byte order B, G, R, A) pixels with the given row stride in bytes.
pub fn bgra_to_i420(src: &[u8], stride: usize, width: usize, height: usize, out: &mut I420) {
    out.resize(width, height);
    for row in 0..height {
        let src_row = &src[row * stride..][..width * 4];
        let y_row = &mut out.y[row * width..][..width];
        for (px, y) in src_row.chunks_exact(4).zip(y_row) {
            *y = luma(px[2] as i32, px[1] as i32, px[0] as i32);
        }
    }

    // Chroma from the average of each 2x2 block, clamped at the right and bottom edges.
    let cw = out.chroma_width();
    for cy in 0..out.chroma_height() {
        let r0 = &src[2 * cy * stride..];
        let r1 = &src[(2 * cy + 1).min(height - 1) * stride..];
        for cx in 0..cw {
            let x0 = 2 * cx * 4;
            let x1 = (2 * cx + 1).min(width - 1) * 4;
            let (mut r, mut g, mut b) = (0, 0, 0);
            for p in [&r0[x0..x0 + 4], &r0[x1..x1 + 4], &r1[x0..x0 + 4], &r1[x1..x1 + 4]] {
                b += p[0] as i32;
                g += p[1] as i32;
                r += p[2] as i32;
            }
            let (r, g, b) = ((r + 2) >> 2, (g + 2) >> 2, (b + 2) >> 2);
            out.u[cy * cw + cx] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
            out.v[cy * cw + cx] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
        }
    }
}

/// A plane borrowed from a decoder picture.
pub struct Plane<'a> {
    pub data: &'a [u8],
    pub stride: usize,
}

/// Writes `0x00RRGGBB` pixels, `width` per row, into `out`.
pub fn i420_to_xrgb(
    y: Plane,
    u: Plane,
    v: Plane,
    width: usize,
    height: usize,
    out: &mut Vec<u32>,
) {
    out.resize(width * height, 0);
    for row in 0..height {
        let y_row = &y.data[row * y.stride..][..width];
        let u_row = &u.data[(row / 2) * u.stride..];
        let v_row = &v.data[(row / 2) * v.stride..];
        let out_row = &mut out[row * width..][..width];
        for (x, (o, &yy)) in out_row.iter_mut().zip(y_row).enumerate() {
            let c = 298 * (yy as i32 - 16);
            let d = u_row[x / 2] as i32 - 128;
            let e = v_row[x / 2] as i32 - 128;
            let r = ((c + 409 * e + 128) >> 8).clamp(0, 255) as u32;
            let g = ((c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u32;
            let b = ((c + 516 * d + 128) >> 8).clamp(0, 255) as u32;
            *o = (r << 16) | (g << 8) | b;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_is_close() {
        // 3x3 exercises the odd-size edge handling.
        let colors: [[u8; 3]; 9] = [
            [255, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
            [255, 255, 255],
            [0, 0, 0],
            [128, 128, 128],
            [10, 200, 30],
            [250, 128, 5],
            [60, 60, 200],
        ];
        // Solid 2x2 blocks keep chroma subsampling out of the comparison.
        for [r, g, b] in colors {
            let src: Vec<u8> = (0..9).flat_map(|_| [b, g, r, 255]).collect();
            let mut i420 = I420::new();
            bgra_to_i420(&src, 12, 3, 3, &mut i420);
            let mut out = Vec::new();
            i420_to_xrgb(
                Plane { data: &i420.y, stride: 3 },
                Plane { data: &i420.u, stride: 2 },
                Plane { data: &i420.v, stride: 2 },
                3,
                3,
                &mut out,
            );
            for px in out {
                let got = [(px >> 16) as u8, (px >> 8) as u8, px as u8];
                for (a, b) in got.iter().zip([r, g, b]) {
                    assert!((*a as i32 - b as i32).abs() <= 3, "{got:?} vs {:?}", [r, g, b]);
                }
            }
        }
    }
}
