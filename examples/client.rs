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

//! Connects to the `server` example and shows its desktop.
//!
//! `cargo run --release --example client -- 192.0.2.1:21118`

use anyhow::Context;
use desklib::{Client, ClientConfig};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

/// Resolves `addr`, treating the unspecified address (`0.0.0.0` / `[::]`, as passed to the
/// server to listen everywhere) as this machine. Unix connects there anyway; Windows refuses.
async fn connect(addr: &str) -> anyhow::Result<tokio::net::TcpStream> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(addr)
        .await
        .with_context(|| format!("invalid address {addr}"))?
        .map(|a| match a {
            SocketAddr::V4(v4) if v4.ip().is_unspecified() => (Ipv4Addr::LOCALHOST, a.port()).into(),
            SocketAddr::V6(v6) if v6.ip().is_unspecified() => (Ipv6Addr::LOCALHOST, a.port()).into(),
            a => a,
        })
        .collect();
    tokio::net::TcpStream::connect(&addrs[..])
        .await
        .with_context(|| format!("failed to connect to {addr}"))
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:21118".into());
    let runtime = tokio::runtime::Runtime::new()?;
    let stream = runtime.block_on(connect(&addr))?;
    stream.set_nodelay(true)?;
    let (reader, writer) = stream.into_split();
    // Run the window on this (main) thread; network I/O goes to `runtime`.
    let _guard = runtime.enter();
    Client::new(ClientConfig { title: format!("Remote Desktop - {addr}") }).run(reader, writer)
}
