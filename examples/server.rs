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

//! Shares this desktop with whoever connects, one at a time, over plain TCP.
//!
//! `cargo run --release --example server -- 0.0.0.0:21118`
//!
//! Plain TCP has no authentication or encryption; anyone who can reach the port controls
//! the machine. Wrap the stream in TLS or tunnel it for anything beyond a trusted network.

use desklib::{Server, ServerConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:21118".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    log::info!("listening on {addr}");
    let server = Server::new(ServerConfig::default());
    loop {
        let (stream, peer) = listener.accept().await?;
        stream.set_nodelay(true)?;
        log::info!("{peer} connected");
        let (reader, writer) = stream.into_split();
        match server.run(reader, writer).await {
            Ok(()) => log::info!("{peer} disconnected"),
            Err(e) => log::warn!("{peer}: {e:#}"),
        }
    }
}
