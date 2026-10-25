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

//! Safe wrapper over rav1d's dav1d-compatible API.

use super::yuv::{i420_to_xrgb, Plane};
use anyhow::{bail, Result};
use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::DAV1D_PIXEL_LAYOUT_I420;
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_get_picture,
    dav1d_open, dav1d_picture_unref, dav1d_send_data,
};
use rav1d::Dav1dResult;
use std::mem::MaybeUninit;
use std::ptr::NonNull;

const EAGAIN: i32 = -(libc_eagain());

const fn libc_eagain() -> i32 {
    if cfg!(any(target_os = "macos", target_os = "ios", target_os = "freebsd")) {
        35
    } else {
        11
    }
}

fn check(res: Dav1dResult, what: &str) -> Result<()> {
    if res.0 < 0 {
        bail!("{what} failed: {}", res.0);
    }
    Ok(())
}

/// A decoded frame as `0x00RRGGBB` pixels.
pub struct DecodedFrame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}

pub struct Decoder {
    ctx: Dav1dContext,
}

// SAFETY: the context is only used through `&mut self`, and dav1d contexts may move between
// threads.
unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new() -> Result<Self> {
        let mut settings = MaybeUninit::<Dav1dSettings>::uninit();
        // SAFETY: `dav1d_default_settings` initializes the whole struct.
        let mut settings = unsafe {
            dav1d_default_settings(NonNull::new_unchecked(settings.as_mut_ptr()));
            settings.assume_init()
        };
        // Output each frame as soon as it is decoded.
        settings.max_frame_delay = 1;
        let mut ctx: Option<Dav1dContext> = None;
        // SAFETY: both pointers are valid for the duration of the call.
        let res = unsafe { dav1d_open(Some(NonNull::from(&mut ctx)), Some(NonNull::from(&mut settings))) };
        check(res, "dav1d_open")?;
        match ctx {
            Some(ctx) => Ok(Self { ctx }),
            None => bail!("dav1d_open returned no context"),
        }
    }

    /// Decodes one temporal unit; returns the frame it completes, if any.
    pub fn decode(&mut self, data: &[u8], out: &mut DecodedFrame) -> Result<bool> {
        if data.is_empty() {
            return Ok(false);
        }
        let mut input = Dav1dData::default();
        // SAFETY: `dav1d_data_create` returns a buffer of `data.len()` bytes owned by `input`.
        unsafe {
            let buf = dav1d_data_create(Some(NonNull::from(&mut input)), data.len());
            if buf.is_null() {
                bail!("dav1d_data_create failed");
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), buf, data.len());
        }
        let mut got = false;
        let result = (|| {
            while input.sz > 0 {
                // SAFETY: `ctx` is open and `input` is a valid data reference.
                let res = unsafe { dav1d_send_data(Some(self.ctx), Some(NonNull::from(&mut input))) };
                if res.0 == EAGAIN {
                    // A picture must be taken out before more data fits.
                    if !self.get_picture(out)? {
                        bail!("decoder neither accepts data nor outputs pictures");
                    }
                    got = true;
                } else {
                    check(res, "dav1d_send_data")?;
                }
            }
            while self.get_picture(out)? {
                got = true;
            }
            Ok(())
        })();
        // SAFETY: `input` is either consumed (a no-op) or still holds our reference.
        unsafe { dav1d_data_unref(Some(NonNull::from(&mut input))) };
        result.map(|()| got)
    }

    fn get_picture(&mut self, out: &mut DecodedFrame) -> Result<bool> {
        let mut pic = Dav1dPicture::default();
        // SAFETY: `ctx` is open and `pic` is valid to write to.
        let res = unsafe { dav1d_get_picture(Some(self.ctx), Some(NonNull::from(&mut pic))) };
        if res.0 == EAGAIN {
            return Ok(false);
        }
        check(res, "dav1d_get_picture")?;
        let converted = convert(&pic, out);
        // SAFETY: `pic` holds a picture reference from `dav1d_get_picture`.
        unsafe { dav1d_picture_unref(Some(NonNull::from(&mut pic))) };
        converted.map(|()| true)
    }
}

fn convert(pic: &Dav1dPicture, out: &mut DecodedFrame) -> Result<()> {
    if pic.p.layout != DAV1D_PIXEL_LAYOUT_I420 || pic.p.bpc != 8 {
        bail!("unsupported picture format (layout {}, {} bpc)", pic.p.layout, pic.p.bpc);
    }
    let (w, h) = (pic.p.w as usize, pic.p.h as usize);
    let (ys, cs) = (pic.stride[0] as usize, pic.stride[1] as usize);
    let (Some(y), Some(u), Some(v)) = (pic.data[0], pic.data[1], pic.data[2]) else {
        bail!("picture has no data");
    };
    let ch = (h + 1) / 2;
    // SAFETY: dav1d guarantees each plane spans `stride * rows` bytes.
    let (y, u, v) = unsafe {
        (
            std::slice::from_raw_parts(y.as_ptr() as *const u8, ys * h),
            std::slice::from_raw_parts(u.as_ptr() as *const u8, cs * ch),
            std::slice::from_raw_parts(v.as_ptr() as *const u8, cs * ch),
        )
    };
    out.width = w;
    out.height = h;
    i420_to_xrgb(
        Plane { data: y, stride: ys },
        Plane { data: u, stride: cs },
        Plane { data: v, stride: cs },
        w,
        h,
        &mut out.pixels,
    );
    Ok(())
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let mut ctx = Some(self.ctx);
        // SAFETY: `ctx` came from `dav1d_open` and is not used afterwards.
        unsafe { dav1d_close(Some(NonNull::from(&mut ctx))) };
    }
}
