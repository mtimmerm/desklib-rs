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
use cocoa::base::{id, nil};
use cocoa::foundation::{NSInteger, NSPoint, NSSize};
use core_graphics::geometry::CGPoint;
use objc::rc::autoreleasepool;
use objc::{class, msg_send, sel, sel_impl};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::{Hash, Hasher};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    /// Changes whenever the cursor does.
    fn CGSCurrentCursorSeed() -> i32;
    fn CGEventCreate(source: *const c_void) -> *const c_void;
    fn CGEventGetLocation(event: *const c_void) -> CGPoint;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *const c_void);
}

pub struct Poller {
    seed: Option<i32>,
    current: Option<u64>,
    /// Shapes by content hash, so a cursor that comes back keeps its id.
    shapes: HashMap<u64, CursorShape>,
}

impl Poller {
    pub fn new() -> Result<Self> {
        Ok(Self {
            seed: None,
            current: None,
            shapes: HashMap::new(),
        })
    }

    pub fn current(&mut self) -> Option<u64> {
        let seed = unsafe { CGSCurrentCursorSeed() };
        if self.seed != Some(seed) {
            match autoreleasepool(|| unsafe { read_shape() }) {
                Ok(shape) => {
                    let mut h = DefaultHasher::new();
                    (shape.hotx, shape.hoty, shape.width, shape.height, &shape.rgba).hash(&mut h);
                    let id = h.finish();
                    self.shapes.entry(id).or_insert(shape);
                    self.current = Some(id);
                    self.seed = Some(seed);
                }
                Err(e) => log::debug!("failed to read cursor: {e:#}"),
            }
        }
        self.current
    }

    pub fn position(&mut self) -> Option<(i32, i32)> {
        unsafe {
            let event = CGEventCreate(std::ptr::null());
            if event.is_null() {
                return None;
            }
            let p = CGEventGetLocation(event);
            CFRelease(event);
            Some((p.x.round() as i32, p.y.round() as i32))
        }
    }

    pub fn shape(&mut self, id: u64) -> Result<CursorShape> {
        match self.shapes.get(&id) {
            Some(s) => Ok(CursorShape {
                hotx: s.hotx,
                hoty: s.hoty,
                width: s.width,
                height: s.height,
                rgba: s.rgba.clone(),
            }),
            None => bail!("unknown cursor {id}"),
        }
    }
}

// https://github.com/stweil/OSXvnc/blob/master/OSXvnc-server/mousecursor.c
unsafe fn read_shape() -> Result<CursorShape> {
    let c: id = msg_send![class!(NSCursor), currentSystemCursor];
    if c == nil {
        bail!("no current system cursor");
    }
    let hotspot: NSPoint = msg_send![c, hotSpot];
    let img: id = msg_send![c, image];
    let size: NSSize = msg_send![img, size];
    let reps: id = msg_send![img, representations];
    let count: usize = if reps == nil { 0 } else { msg_send![reps, count] };
    if count == 0 {
        bail!("cursor image has no representations");
    }
    let rep: id = msg_send![reps, objectAtIndex: 0usize];
    let (w, h) = (size.width as usize, size.height as usize);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let color: id = msg_send![rep, colorAtX: x as NSInteger y: y as NSInteger];
            if color == nil {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            let r: f64 = msg_send![color, redComponent];
            let g: f64 = msg_send![color, greenComponent];
            let b: f64 = msg_send![color, blueComponent];
            let a: f64 = msg_send![color, alphaComponent];
            rgba.extend_from_slice(&[(r * 255.) as u8, (g * 255.) as u8, (b * 255.) as u8, (a * 255.) as u8]);
        }
    }
    Ok(CursorShape {
        hotx: hotspot.x as i32,
        hoty: hotspot.y as i32,
        width: w as i32,
        height: h as i32,
        rgba,
    })
}
