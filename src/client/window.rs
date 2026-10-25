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

//! The viewer window: draws frames, mirrors the remote cursor and captures input.

use super::keymap;
use super::session::{CursorImage, Event, Mailbox};
use super::ClientConfig;
use crate::codec::DecodedFrame;
use crate::proto::{self, KeyEvent, Message, MouseButtonEvent, MouseMove, MouseWheel};
use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet};
use std::num::NonZeroU32;
use std::sync::Arc;
use tokio::sync::mpsc;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::PhysicalKey;
use winit::window::{CustomCursor, Window, WindowId};

/// A session event tagged with the session it belongs to, since the event loop outlives
/// sessions.
pub struct UserEvent {
    pub session: u64,
    pub event: Event,
}

/// Pixels scrolled with a touchpad that count as one wheel notch.
const PIXELS_PER_NOTCH: f64 = 50.0;

/// Where the frame is drawn within the window.
#[derive(Clone, Copy)]
struct View {
    x: f64,
    y: f64,
    scale: f64,
}

struct Cursor {
    image: CursorImage,
    native: Option<CustomCursor>,
}

pub struct App {
    session: u64,
    config: ClientConfig,
    mailbox: Mailbox,
    input: Option<mpsc::UnboundedSender<Message>>,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    frame: Option<DecodedFrame>,
    sized_for: Option<(usize, usize)>,
    view: Option<View>,
    cursors: HashMap<u64, Cursor>,
    cursor: Option<u64>,
    cursor_hidden: bool,
    remote_pos: Option<(i32, i32)>,
    pointer_inside: bool,
    keys: HashSet<u32>,
    buttons: HashSet<proto::MouseButton>,
    scroll: (f64, f64),
    pub result: Option<Result<()>>,
}

impl App {
    pub fn new(
        session: u64,
        config: ClientConfig,
        mailbox: Mailbox,
        input: mpsc::UnboundedSender<Message>,
    ) -> Self {
        Self {
            session,
            config,
            mailbox,
            input: Some(input),
            window: None,
            surface: None,
            frame: None,
            sized_for: None,
            view: None,
            cursors: HashMap::new(),
            cursor: None,
            cursor_hidden: false,
            remote_pos: None,
            pointer_inside: false,
            keys: HashSet::new(),
            buttons: HashSet::new(),
            scroll: (0.0, 0.0),
            result: None,
        }
    }

    fn send(&self, msg: Message) {
        if let Some(input) = &self.input {
            // Fails only once the session is over, which `Event::Closed` reports.
            let _ = input.send(msg);
        }
    }

    fn finish(&mut self, event_loop: &ActiveEventLoop, result: Result<()>) {
        self.release_all();
        self.result.get_or_insert(result);
        // Dropping the sender ends the network task.
        self.input = None;
        self.surface = None;
        self.window = None;
        event_loop.exit();
    }

    /// Lifts everything held down, so nothing stays stuck on the server after focus moves
    /// away or the window closes.
    fn release_all(&mut self) {
        for usb_hid in std::mem::take(&mut self.keys) {
            let mut msg = Message::new();
            msg.set_key_event(KeyEvent { usb_hid, down: false, ..Default::default() });
            self.send(msg);
        }
        for button in std::mem::take(&mut self.buttons) {
            let mut msg = Message::new();
            msg.set_mouse_button(MouseButtonEvent {
                button: button.into(),
                down: false,
                ..Default::default()
            });
            self.send(msg);
        }
    }

    fn redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn on_frame(&mut self) {
        let Some(frame) = self.mailbox.lock().unwrap().take() else { return };
        let size = (frame.width, frame.height);
        self.frame = Some(frame);
        if self.sized_for != Some(size) {
            self.sized_for = Some(size);
            self.fit_window(size);
        }
        self.redraw();
    }

    /// Sizes the window to the remote display, within the monitor it is on.
    fn fit_window(&self, (w, h): (usize, usize)) {
        let Some(window) = &self.window else { return };
        let (mut w, mut h) = (w as f64, h as f64);
        let monitor = window.current_monitor();
        if let Some(monitor) = &monitor {
            let m = monitor.size();
            let fit = (m.width as f64 * 0.9 / w).min(m.height as f64 * 0.85 / h).min(1.0);
            w *= fit;
            h *= fit;
        }
        let _ = window.request_inner_size(PhysicalSize::new(w.round() as u32, h.round() as u32));
        if let Some(monitor) = monitor {
            let (m, p) = (monitor.size(), monitor.position());
            window.set_outer_position(PhysicalPosition::new(
                p.x + ((m.width as f64 - w) / 2.0) as i32,
                p.y + ((m.height as f64 - h) / 2.0) as i32,
            ));
        }
    }

    fn set_cursor(&mut self, event_loop: &ActiveEventLoop, id: u64) {
        let (Some(window), Some(cursor)) = (&self.window, self.cursors.get_mut(&id)) else {
            return;
        };
        self.cursor = Some(id);
        self.cursor_hidden = false;
        if cursor.native.is_none() {
            let img = &cursor.image;
            match CustomCursor::from_rgba(
                img.rgba.clone(),
                img.width as u16,
                img.height as u16,
                img.hotx.clamp(0, img.width - 1) as u16,
                img.hoty.clamp(0, img.height - 1) as u16,
            ) {
                Ok(source) => cursor.native = Some(event_loop.create_custom_cursor(source)),
                Err(e) => log::debug!("unusable cursor image: {e}"),
            }
        }
        if let Some(native) = &cursor.native {
            window.set_cursor(native.clone());
        }
        window.set_cursor_visible(true);
        self.redraw();
    }

    /// Maps a window position to frame pixels, if it is over the frame.
    fn to_remote(&self, pos: PhysicalPosition<f64>) -> Option<(i32, i32)> {
        let (view, frame) = (self.view?, self.frame.as_ref()?);
        let x = ((pos.x - view.x) / view.scale).floor();
        let y = ((pos.y - view.y) / view.scale).floor();
        (x >= 0.0 && y >= 0.0 && x < frame.width as f64 && y < frame.height as f64)
            .then_some((x as i32, y as i32))
    }

    fn draw(&mut self) -> Result<()> {
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return Ok(());
        };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return Ok(());
        };
        surface.resize(w, h).map_err(|e| anyhow!("{e}"))?;
        let mut buffer = surface.buffer_mut().map_err(|e| anyhow!("{e}"))?;
        let (bw, bh) = (size.width as usize, size.height as usize);
        buffer.fill(0);

        if let Some(frame) = &self.frame {
            let (fw, fh) = (frame.width, frame.height);
            let scale = (bw as f64 / fw as f64).min(bh as f64 / fh as f64);
            let (dw, dh) = (((fw as f64 * scale) as usize).min(bw), ((fh as f64 * scale) as usize).min(bh));
            let (ox, oy) = ((bw - dw) / 2, (bh - dh) / 2);
            self.view = Some(View { x: ox as f64, y: oy as f64, scale });

            if dw == fw && dh == fh {
                for y in 0..fh {
                    buffer[(oy + y) * bw + ox..][..fw].copy_from_slice(&frame.pixels[y * fw..][..fw]);
                }
            } else {
                // Nearest neighbour.
                let xs: Vec<usize> = (0..dw).map(|x| ((x as f64 / scale) as usize).min(fw - 1)).collect();
                for y in 0..dh {
                    let src = &frame.pixels[((y as f64 / scale) as usize).min(fh - 1) * fw..][..fw];
                    let dst = &mut buffer[(oy + y) * bw + ox..][..dw];
                    for (d, &sx) in dst.iter_mut().zip(&xs) {
                        *d = src[sx];
                    }
                }
            }

            // While the local pointer is over the window it shows the remote shape itself;
            // otherwise draw the remote cursor where the server says it is.
            if !self.pointer_inside && !self.cursor_hidden {
                if let (Some(c), Some((rx, ry))) = (self.cursor.and_then(|id| self.cursors.get(&id)), self.remote_pos) {
                    let img = &c.image;
                    let cx = ox as f64 + rx as f64 * scale - img.hotx as f64;
                    let cy = oy as f64 + ry as f64 * scale - img.hoty as f64;
                    for y in 0..img.height {
                        for x in 0..img.width {
                            let (px, py) = ((cx + x as f64) as isize, (cy + y as f64) as isize);
                            if px < 0 || py < 0 || px >= bw as isize || py >= bh as isize {
                                continue;
                            }
                            let s = &img.rgba[((y * img.width + x) * 4) as usize..][..4];
                            let a = s[3] as u32;
                            if a == 0 {
                                continue;
                            }
                            let d = &mut buffer[py as usize * bw + px as usize];
                            let blend = |sc: u8, shift: u32| {
                                let dc = (*d >> shift) & 0xff;
                                ((sc as u32 * a + dc * (255 - a)) / 255) << shift
                            };
                            *d = blend(s[0], 16) | blend(s[1], 8) | blend(s[2], 0);
                        }
                    }
                }
            }
        }

        buffer.present().map_err(|e| anyhow!("{e}"))?;
        Ok(())
    }

    fn mouse_button(&mut self, button: MouseButton, state: ElementState) {
        let button = match button {
            MouseButton::Left => proto::MouseButton::LEFT,
            MouseButton::Right => proto::MouseButton::RIGHT,
            MouseButton::Middle => proto::MouseButton::MIDDLE,
            MouseButton::Back => proto::MouseButton::BACK,
            MouseButton::Forward => proto::MouseButton::FORWARD,
            MouseButton::Other(_) => return,
        };
        let down = state == ElementState::Pressed;
        if down {
            self.buttons.insert(button);
        } else if !self.buttons.remove(&button) {
            // Pressed outside the window; the server never saw it go down.
            return;
        }
        let mut msg = Message::new();
        msg.set_mouse_button(MouseButtonEvent { button: button.into(), down, ..Default::default() });
        self.send(msg);
    }

    fn mouse_wheel(&mut self, delta: MouseScrollDelta) {
        // winit's positive values move the content right and down, i.e. scroll left and up.
        let (dx, dy) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (-x as f64, y as f64),
            MouseScrollDelta::PixelDelta(p) => (-p.x / PIXELS_PER_NOTCH, p.y / PIXELS_PER_NOTCH),
        };
        self.scroll.0 += dx;
        self.scroll.1 += dy;
        let (nx, ny) = (self.scroll.0.trunc(), self.scroll.1.trunc());
        if nx == 0.0 && ny == 0.0 {
            return;
        }
        self.scroll.0 -= nx;
        self.scroll.1 -= ny;
        let mut msg = Message::new();
        msg.set_mouse_wheel(MouseWheel { dx: nx as i32, dy: ny as i32, ..Default::default() });
        self.send(msg);
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(self.config.title.clone())
            .with_inner_size(PhysicalSize::new(1280, 720));
        let created = event_loop.create_window(attrs).map_err(anyhow::Error::from).and_then(|window| {
            let window = Arc::new(window);
            let context = softbuffer::Context::new(window.clone()).map_err(|e| anyhow!("{e}"))?;
            let surface = softbuffer::Surface::new(&context, window.clone()).map_err(|e| anyhow!("{e}"))?;
            Ok((window, surface))
        });
        match created {
            Ok((window, surface)) => {
                self.window = Some(window);
                self.surface = Some(surface);
            }
            Err(e) => self.finish(event_loop, Err(e.context("failed to create window"))),
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        if event.session != self.session {
            return;
        }
        match event.event {
            Event::ServerInfo(details) => {
                log::info!("server: {} Source: {}", details.notice, details.source_url);
                if let Some(callback) = &self.config.on_server_info {
                    callback(&details);
                }
            }
            Event::Frame => self.on_frame(),
            Event::CursorShape(image) => {
                let id = image.id;
                self.cursors.insert(id, Cursor { image, native: None });
                self.set_cursor(event_loop, id);
            }
            Event::CursorId(id) => self.set_cursor(event_loop, id),
            Event::CursorHidden => {
                self.cursor_hidden = true;
                if let Some(w) = &self.window {
                    w.set_cursor_visible(false);
                }
                self.redraw();
            }
            Event::CursorPosition(x, y) => {
                self.remote_pos = Some((x, y));
                if !self.pointer_inside {
                    self.redraw();
                }
            }
            Event::Closed(result) => self.finish(event_loop, result),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.finish(event_loop, Ok(())),
            WindowEvent::Resized(_) => self.redraw(),
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.draw() {
                    log::warn!("failed to draw: {e:#}");
                }
            }
            WindowEvent::CursorEntered { .. } => {
                self.pointer_inside = true;
                self.redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                self.pointer_inside = false;
                self.redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer_inside = true;
                if let Some((x, y)) = self.to_remote(position) {
                    let mut msg = Message::new();
                    msg.set_mouse_move(MouseMove { x, y, ..Default::default() });
                    self.send(msg);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => self.mouse_button(button, state),
            WindowEvent::MouseWheel { delta, .. } => self.mouse_wheel(delta),
            WindowEvent::KeyboardInput { event, .. } => {
                // The server's own key repeat takes care of held keys.
                if event.repeat {
                    return;
                }
                let PhysicalKey::Code(code) = event.physical_key else { return };
                let Some(usb_hid) = keymap::usb_hid(code) else { return };
                let down = event.state == ElementState::Pressed;
                if down {
                    self.keys.insert(usb_hid);
                } else if !self.keys.remove(&usb_hid) {
                    return;
                }
                let mut msg = Message::new();
                msg.set_key_event(KeyEvent { usb_hid, down, ..Default::default() });
                self.send(msg);
            }
            WindowEvent::Focused(false) => self.release_all(),
            _ => {}
        }
    }
}
