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

//! Remote desktop over any `AsyncRead + AsyncWrite` stream.
//!
//! - [`Server`] sends a view of the local desktop over the stream and replays the input
//!   events it receives.
//! - [`Client`] displays the remote desktop in a window and captures and sends local input
//!   events.
//!
//! Transport, authentication and encryption are the caller's: wrap the stream in TLS, tunnel
//! it over SSH, and so on. Video is AV1, encoded with rav1e and decoded with rav1d.

// objc 0.2 macros test cfg(cargo-clippy).
#![cfg_attr(target_os = "macos", allow(unexpected_cfgs))]

mod client;
mod codec;
mod handshake;
mod proto;
mod server;
mod transport;

#[cfg(test)]
mod tests;

pub use client::{Client, ClientConfig, ServerInfoCallback};
pub use handshake::{
    ServerConfig, ServerDetails, VersionMismatch, MIN_CLIENT_PROTOCOL_VERSION, MIN_SERVER_PROTOCOL_VERSION,
    PROTOCOL_VERSION,
};
pub use server::{Server, ServerOptions};

/// Copyright and license notice for desklib.
///
/// The GNU AGPL (section 5(d)) requires interactive programs built on desklib to display it,
/// for example in an About box, along with where to get the source ([`SOURCE_URL`]).
pub const LEGAL_NOTICE: &str = "desklib \u{a9} 2026 Matt Timmermans, derived from RustDesk \u{a9} Purslane Tech Pte. Ltd. and the RustDesk contributors. Free software under the GNU AGPL v3, with ABSOLUTELY NO WARRANTY.";

/// Where the source code of this version of desklib is published.
///
/// If you distribute a modified version, or run one that users interact with over a network,
/// the GNU AGPL requires you to offer its source code: change this, or set
/// [`ServerOptions::source_url`], to where your modified source can be obtained.
pub const SOURCE_URL: &str = env!("CARGO_PKG_REPOSITORY");
