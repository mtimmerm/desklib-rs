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
use std::os::raw::{c_int, c_uint};
use x11::xfixes::XFixesGetCursorImage;
use x11::xlib::{
    Display, Window, XCloseDisplay, XDefaultRootWindow, XFree, XOpenDisplay, XQueryPointer,
};

pub struct Poller {
    display: *mut Display,
}

impl Poller {
    pub fn new() -> Result<Self> {
        let display = unsafe { XOpenDisplay(std::ptr::null()) };
        if display.is_null() {
            bail!("cannot open X display");
        }
        Ok(Self { display })
    }

    /// The current cursor's serial. XFixes gives no portable way to tell that the cursor is
    /// hidden, so this never returns `None`.
    pub fn current(&mut self) -> Option<u64> {
        unsafe {
            let img = XFixesGetCursorImage(self.display);
            if img.is_null() {
                return None;
            }
            let serial = (*img).cursor_serial as u64;
            XFree(img as _);
            Some(serial)
        }
    }

    pub fn position(&mut self) -> Option<(i32, i32)> {
        unsafe {
            let root = XDefaultRootWindow(self.display);
            let (mut root_ret, mut child): (Window, Window) = (0, 0);
            let (mut x, mut y, mut wx, mut wy): (c_int, c_int, c_int, c_int) = (0, 0, 0, 0);
            let mut mask: c_uint = 0;
            let ok = XQueryPointer(
                self.display,
                root,
                &mut root_ret,
                &mut child,
                &mut x,
                &mut y,
                &mut wx,
                &mut wy,
                &mut mask,
            );
            (ok != 0).then_some((x, y))
        }
    }

    pub fn shape(&mut self, id: u64) -> Result<CursorShape> {
        unsafe {
            let img = XFixesGetCursorImage(self.display);
            if img.is_null() {
                bail!("XFixesGetCursorImage failed");
            }
            let i = &*img;
            let result = if i.cursor_serial as u64 != id {
                Err(anyhow::anyhow!("cursor changed while reading it"))
            } else {
                let (w, h) = (i.width as usize, i.height as usize);
                // Premultiplied ARGB, one per `c_ulong`.
                let pixels = std::slice::from_raw_parts(i.pixels, w * h);
                let mut rgba = Vec::with_capacity(w * h * 4);
                for &p in pixels {
                    let p = p as u32;
                    let a = p >> 24;
                    let un = |c: u32| if a == 0 { 0 } else { (c * 255 / a).min(255) as u8 };
                    rgba.extend_from_slice(&[un((p >> 16) & 0xff), un((p >> 8) & 0xff), un(p & 0xff), a as u8]);
                }
                Ok(CursorShape {
                    hotx: i.xhot as i32,
                    hoty: i.yhot as i32,
                    width: w as i32,
                    height: h as i32,
                    rgba,
                })
            };
            XFree(img as _);
            result
        }
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        unsafe { XCloseDisplay(self.display) };
    }
}
