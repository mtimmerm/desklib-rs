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

//! Replays the peer's input events on a dedicated thread.

use super::Shared;
use crate::proto::{self, Union};
use anyhow::Result;
use enigo::{Enigo, MouseButton, MouseControllable};
use std::collections::HashSet;
use std::sync::mpsc;
use std::sync::Arc;

pub enum InputEvent {
    Move { x: i32, y: i32 },
    Button { button: MouseButton, down: bool },
    Wheel { dx: i32, dy: i32 },
    Key { usb_hid: u32, down: bool },
}

impl InputEvent {
    pub fn from_message(msg: Union) -> Option<Self> {
        Some(match msg {
            Union::MouseMove(m) => Self::Move { x: m.x, y: m.y },
            Union::MouseButton(b) => {
                let button = match b.button.enum_value().ok()? {
                    proto::MouseButton::LEFT => MouseButton::Left,
                    proto::MouseButton::RIGHT => MouseButton::Right,
                    proto::MouseButton::MIDDLE => MouseButton::Middle,
                    proto::MouseButton::BACK => MouseButton::Back,
                    proto::MouseButton::FORWARD => MouseButton::Forward,
                };
                Self::Button { button, down: b.down }
            }
            Union::MouseWheel(w) => Self::Wheel { dx: w.dx, dy: w.dy },
            Union::KeyEvent(k) => Self::Key { usb_hid: k.usb_hid, down: k.down },
            _ => return None,
        })
    }
}

pub fn spawn(shared: Arc<Shared>) -> Result<mpsc::Sender<InputEvent>> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("desk-input".into())
        .spawn(move || run(rx, &shared))?;
    Ok(tx)
}

fn run(rx: mpsc::Receiver<InputEvent>, shared: &Shared) {
    let mut enigo = Enigo::default();
    let mut keys = HashSet::new();
    let mut buttons = Vec::new();

    while let Ok(event) = rx.recv() {
        match event {
            InputEvent::Move { x, y } => {
                let (x, y) = shared.rect().to_desktop(x, y);
                enigo.mouse_move_to(x, y);
            }
            InputEvent::Button { button, down } => {
                if down {
                    if let Err(e) = enigo.mouse_down(button) {
                        log::debug!("mouse_down: {e}");
                    }
                    buttons.push(button);
                } else {
                    enigo.mouse_up(button);
                    buttons.retain(|b| *b != button);
                }
            }
            InputEvent::Wheel { dx, dy } => scroll(&mut enigo, dx, dy),
            InputEvent::Key { usb_hid, down } => {
                let key = rdev::usb_hid_key_from_code(usb_hid);
                if matches!(key, rdev::Key::Unknown(_)) {
                    log::debug!("no key for USB HID usage {usb_hid:#x}");
                    continue;
                }
                if down {
                    keys.insert(key);
                } else {
                    keys.remove(&key);
                }
                simulate_key(key, down);
            }
        }
    }

    // The session is over; don't leave anything held down.
    for key in keys {
        simulate_key(key, false);
    }
    for button in buttons {
        enigo.mouse_up(button);
    }
}

fn simulate_key(key: rdev::Key, down: bool) {
    let event = if down {
        rdev::EventType::KeyPress(key)
    } else {
        rdev::EventType::KeyRelease(key)
    };
    if let Err(e) = rdev::simulate(&event) {
        log::debug!("failed to simulate {key:?}: {e:?}");
    }
}

/// `dx` / `dy` are in notches, positive right / up.
fn scroll(enigo: &mut Enigo, dx: i32, dy: i32) {
    // enigo passes Windows wheel data through raw, and elsewhere counts notches with
    // positive meaning down.
    #[cfg(windows)]
    let (dx, dy) = (dx * 120, dy * 120);
    #[cfg(not(windows))]
    let dy = -dy;
    if dy != 0 {
        MouseControllable::mouse_scroll_y(enigo, dy);
    }
    if dx != 0 {
        MouseControllable::mouse_scroll_x(enigo, dx);
    }
}
