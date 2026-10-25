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

use super::CursorShape;
use anyhow::{bail, Result};
use std::mem;
use winapi::shared::windef::{HBITMAP, HCURSOR, HDC, POINT};
use winapi::um::wingdi::{
    DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS,
};
use winapi::um::winuser::{
    GetCursorInfo, GetCursorPos, GetDC, GetIconInfo, ReleaseDC, CURSORINFO, CURSOR_SHOWING,
    ICONINFO,
};

pub struct Poller;

impl Poller {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }

    /// The current cursor handle, or `None` while it is hidden.
    pub fn current(&mut self) -> Option<u64> {
        unsafe {
            let mut ci: CURSORINFO = mem::zeroed();
            ci.cbSize = mem::size_of::<CURSORINFO>() as _;
            if GetCursorInfo(&mut ci) == 0 || ci.flags & CURSOR_SHOWING == 0 || ci.hCursor.is_null()
            {
                return None;
            }
            Some(ci.hCursor as u64)
        }
    }

    pub fn position(&mut self) -> Option<(i32, i32)> {
        unsafe {
            let mut p: POINT = mem::zeroed();
            (GetCursorPos(&mut p) != 0).then_some((p.x, p.y))
        }
    }

    pub fn shape(&mut self, id: u64) -> Result<CursorShape> {
        unsafe { shape(id as HCURSOR) }
    }
}

/// Owns the bitmaps `GetIconInfo` creates.
struct IconInfo(ICONINFO);

impl Drop for IconInfo {
    fn drop(&mut self) {
        unsafe {
            if !self.0.hbmColor.is_null() {
                DeleteObject(self.0.hbmColor as _);
            }
            if !self.0.hbmMask.is_null() {
                DeleteObject(self.0.hbmMask as _);
            }
        }
    }
}

struct ScreenDC(HDC);

impl Drop for ScreenDC {
    fn drop(&mut self) {
        unsafe { ReleaseDC(std::ptr::null_mut(), self.0) };
    }
}

unsafe fn bitmap_size(bitmap: HBITMAP) -> Result<(i32, i32)> {
    let mut bm: BITMAP = mem::zeroed();
    if GetObjectW(bitmap as _, mem::size_of::<BITMAP>() as _, &mut bm as *mut _ as _) == 0 {
        bail!("GetObject failed");
    }
    Ok((bm.bmWidth, bm.bmHeight))
}

/// Reads a bitmap as top-down 32-bit BGRA, whatever its native format.
unsafe fn bitmap_bits(dc: HDC, bitmap: HBITMAP, width: i32, height: i32) -> Result<Vec<u8>> {
    let mut bi: BITMAPINFO = mem::zeroed();
    bi.bmiHeader = BITMAPINFOHEADER {
        biSize: mem::size_of::<BITMAPINFOHEADER>() as _,
        biWidth: width,
        biHeight: -height,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..mem::zeroed()
    };
    let mut bits = vec![0u8; (width * height * 4) as usize];
    if GetDIBits(dc, bitmap, 0, height as _, bits.as_mut_ptr() as _, &mut bi, DIB_RGB_COLORS) == 0
    {
        bail!("GetDIBits failed");
    }
    Ok(bits)
}

unsafe fn shape(cursor: HCURSOR) -> Result<CursorShape> {
    let mut ii: ICONINFO = mem::zeroed();
    if GetIconInfo(cursor, &mut ii) == 0 {
        bail!("GetIconInfo failed");
    }
    let ii = IconInfo(ii);
    if ii.0.hbmMask.is_null() {
        bail!("cursor has no mask");
    }
    let dc = ScreenDC(GetDC(std::ptr::null_mut()));
    if dc.0.is_null() {
        bail!("GetDC failed");
    }

    let (width, mask_height) = bitmap_size(ii.0.hbmMask)?;
    // Monochrome cursors stack the AND mask on top of the XOR mask.
    let height = if ii.0.hbmColor.is_null() { mask_height / 2 } else { mask_height };
    if width <= 0 || height <= 0 {
        bail!("empty cursor");
    }
    let mask = bitmap_bits(dc.0, ii.0.hbmMask, width, mask_height)?;
    // In 32-bit form each mask bit became a black or white pixel.
    let mask_bit = |x: i32, y: i32| mask[((y * width + x) * 4) as usize] != 0;

    let n = (width * height) as usize;
    let mut rgba = vec![0u8; n * 4];
    let mut inverted = vec![false; n];
    if !ii.0.hbmColor.is_null() {
        let color = bitmap_bits(dc.0, ii.0.hbmColor, width, height)?;
        let has_alpha = color.chunks_exact(4).any(|p| p[3] != 0);
        for i in 0..n {
            let (x, y) = ((i as i32) % width, (i as i32) / width);
            let p = &color[i * 4..i * 4 + 4];
            let alpha = if has_alpha {
                p[3]
            } else if mask_bit(x, y) {
                // AND bit set: transparent, unless the color inverts the screen.
                if p[..3] != [0, 0, 0] {
                    inverted[i] = true;
                }
                0
            } else {
                255
            };
            rgba[i * 4..i * 4 + 4].copy_from_slice(&[p[2], p[1], p[0], alpha]);
        }
    } else {
        for i in 0..n {
            let (x, y) = ((i as i32) % width, (i as i32) / width);
            let (and, xor) = (mask_bit(x, y), mask_bit(x, y + height));
            let px = match (and, xor) {
                (false, false) => [0, 0, 0, 255],
                (false, true) => [255, 255, 255, 255],
                (true, false) => [0, 0, 0, 0],
                (true, true) => {
                    inverted[i] = true;
                    [0, 0, 0, 0]
                }
            };
            rgba[i * 4..i * 4 + 4].copy_from_slice(&px);
        }
    }

    // Screen-inverting pixels (the I-beam) can't be shown as an image: draw them black
    // with a white outline so they stay visible on any background.
    if inverted.contains(&true) {
        for i in (0..n).filter(|&i| inverted[i]) {
            rgba[i * 4..i * 4 + 4].copy_from_slice(&[0, 0, 0, 255]);
        }
        let opaque: Vec<bool> = (0..n).map(|i| rgba[i * 4 + 3] != 0).collect();
        for y in 0..height {
            for x in 0..width {
                let i = (y * width + x) as usize;
                if opaque[i] {
                    continue;
                }
                let near_inverted = (-1..=1).any(|dy| {
                    (-1..=1).any(|dx| {
                        let (nx, ny) = (x + dx, y + dy);
                        nx >= 0
                            && ny >= 0
                            && nx < width
                            && ny < height
                            && inverted[(ny * width + nx) as usize]
                    })
                });
                if near_inverted {
                    rgba[i * 4..i * 4 + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
    }

    Ok(CursorShape {
        hotx: ii.0.xHotspot as i32,
        hoty: ii.0.yHotspot as i32,
        width,
        height,
        rgba,
    })
}
