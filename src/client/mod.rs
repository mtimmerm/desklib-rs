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

use crate::{ServerConfig, ServerDetails};
use anyhow::{anyhow, Result};
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use winit::event_loop::EventLoop;
use winit::platform::run_on_demand::EventLoopExtRunOnDemand;
use window::{App, UserEvent};

/// Called with the server's details once the handshake succeeds.
pub type ServerInfoCallback = Arc<dyn Fn(&ServerDetails) + Send + Sync>;

/// Client settings.
#[derive(Clone)]
pub struct ClientConfig {
    /// Window title.
    pub title: String,
    /// Session settings to ask the server for: which display to share, video quality, and
    /// so on. Anything left unset gets the server's default.
    pub server: ServerConfig,
    /// Receives the server's software version, license notice and source code offer when the
    /// session starts. It runs on the thread that called [`Client::run`].
    ///
    /// The GNU AGPL requires software built on desklib to let its users see the license
    /// notices and get the source code: both desklib's own ([`LEGAL_NOTICE`](crate::LEGAL_NOTICE),
    /// [`SOURCE_URL`](crate::SOURCE_URL)) and, since the user interacts with the server over
    /// the network, the server's (section 13). Show them somewhere convenient, such as an
    /// About box. They are also logged.
    pub on_server_info: Option<ServerInfoCallback>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            title: "Remote Desktop".into(),
            server: ServerConfig::default(),
            on_server_info: None,
        }
    }
}

impl std::fmt::Debug for ClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("ClientConfig")
            .field("title", &self.title)
            .field("server", &self.server)
            .field("on_server_info", &self.on_server_info.as_ref().map(|_| "<callback>"))
            .finish()
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
    /// The session starts with a handshake that sends [`ClientConfig::server`] to the server.
    /// If either side's version is too old for the other, this returns a
    /// [`VersionMismatch`](crate::VersionMismatch) error.
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
            let session = session::run(
                reader,
                writer,
                self.config.server.clone(),
                input_rx,
                mailbox.clone(),
                events,
            );
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
