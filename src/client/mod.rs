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

//! Views and controls a remote desktop in a local window.

mod keymap;
pub(crate) mod session;
mod window;

use anyhow::{anyhow, Result};
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use winit::event_loop::EventLoop;
use winit::platform::run_on_demand::EventLoopExtRunOnDemand;
use window::{App, UserEvent};

/// Client settings.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Window title. The license notice and source links, which the GNU AGPL requires the
    /// window to show, are appended to it.
    pub title: String,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self { title: "Remote Desktop".into() }
    }
}

/// Displays a remote desktop received over a stream and sends local keyboard and mouse
/// input back over it.
///
/// ```no_run
/// # async fn f() -> anyhow::Result<()> {
/// let stream = tokio::net::TcpStream::connect("192.0.2.1:21118").await?;
/// let (reader, writer) = stream.into_split();
/// desklib::Client::new(Default::default()).run(reader, writer)
/// # }
/// ```
pub struct Client {
    config: ClientConfig,
}

thread_local! {
    // winit allows one event loop per process, so it is kept for later sessions on the
    // same thread.
    static EVENT_LOOP: RefCell<Option<EventLoop<UserEvent>>> = const { RefCell::new(None) };
}

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

fn new_event_loop() -> Result<EventLoop<UserEvent>> {
    #[allow(unused_mut)]
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(windows)]
    winit::platform::windows::EventLoopBuilderExtWindows::with_any_thread(&mut builder, true);
    #[cfg(target_os = "linux")]
    {
        winit::platform::x11::EventLoopBuilderExtX11::with_any_thread(&mut builder, true);
        winit::platform::wayland::EventLoopBuilderExtWayland::with_any_thread(&mut builder, true);
    }
    Ok(builder.build()?)
}

impl Client {
    pub fn new(config: ClientConfig) -> Self {
        Self { config }
    }

    /// Opens a window showing the remote desktop and runs until the window is closed (`Ok`)
    /// or the session fails.
    ///
    /// This blocks the calling thread running the window's event loop. On macOS it must be
    /// called from the main thread; elsewhere any thread works, but sessions must keep to
    /// the thread that ran the first one. Network I/O runs on the current Tokio runtime if
    /// there is one (it must be multi-threaded, since this thread is blocked), or else on a
    /// runtime of its own.
    pub fn run<R, W>(&self, reader: R, writer: W) -> Result<()>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        log::info!("{} Source: {}", crate::LEGAL_NOTICE, crate::SOURCE_URL);
        EVENT_LOOP.with(|cell| {
            let mut slot = cell.borrow_mut();
            if slot.is_none() {
                *slot = Some(new_event_loop()?);
            }
            let event_loop = slot.as_mut().unwrap();

            let session_id = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
            let proxy = event_loop.create_proxy();
            let events = move |event| {
                // Fails only once the event loop is gone.
                let _ = proxy.send_event(UserEvent { session: session_id, event });
            };
            let mailbox = Arc::new(Mutex::new(None));
            let (input_tx, input_rx) = mpsc::unbounded_channel();
            let session = session::run(reader, writer, input_rx, mailbox.clone(), events);
            match tokio::runtime::Handle::try_current() {
                Ok(handle) => {
                    handle.spawn(session);
                }
                Err(_) => {
                    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
                    std::thread::Builder::new()
                        .name("desk-client-net".into())
                        .spawn(move || runtime.block_on(session))?;
                }
            }

            let mut app = App::new(session_id, self.config.clone(), mailbox, input_tx);
            event_loop.run_app_on_demand(&mut app)?;
            app.result.unwrap_or_else(|| Err(anyhow!("event loop exited unexpectedly")))
        })
    }

    /// Like [`run`](Self::run), for a stream that is both readable and writable.
    pub fn run_stream<S>(&self, stream: S) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (reader, writer) = tokio::io::split(stream);
        self.run(reader, writer)
    }
}
