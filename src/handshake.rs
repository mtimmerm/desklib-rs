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

//! The exchange that opens every session: the server introduces itself, the client replies
//! with its own version and the settings it wants, and each side checks that the other is
//! new enough.

use crate::codec::EncoderSettings;
use crate::proto::{self, ClientHello, Message, ServerInfo, Union};
use crate::transport::{MessageReader, MessageWriter};
use anyhow::{bail, Context, Result};
use std::fmt;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};

/// This version of the wire protocol. Bump it whenever a peer needs to know that the other
/// side understands something new.
pub const PROTOCOL_VERSION: u32 = 1;
/// The oldest client protocol version a server accepts.
pub const MIN_CLIENT_PROTOCOL_VERSION: u32 = 1;
/// The oldest server protocol version a client accepts.
pub const MIN_SERVER_PROTOCOL_VERSION: u32 = 1;

/// How long either side waits for the other's half of the handshake.
const TIMEOUT: Duration = Duration::from_secs(30);

fn software_version() -> String {
    format!("desklib {}", env!("CARGO_PKG_VERSION"))
}

/// Session settings the client asks the server to use.
///
/// Every field is optional: the server uses its own default for anything left as `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerConfig {
    /// Index of the display to share, in the server platform's enumeration order. Default 0.
    pub display: Option<u32>,
    /// Upper bound on frames per second; frames are only sent when the screen changes.
    /// Default 30.
    pub max_fps: Option<u32>,
    /// Encoder speed preset, 0 (slowest, best compression) to 10 (fastest). Default 10.
    pub speed: Option<u8>,
    /// Constant quantizer, 0 (best quality) to 255 (smallest). Default 120.
    pub quantizer: Option<u8>,
    /// Encoder threads; 0 uses one per CPU. Default 0.
    pub encoder_threads: Option<u32>,
}

impl ServerConfig {
    fn to_proto(&self) -> proto::ServerConfig {
        proto::ServerConfig {
            display: self.display,
            max_fps: self.max_fps,
            speed: self.speed.map(u32::from),
            quantizer: self.quantizer.map(u32::from),
            encoder_threads: self.encoder_threads,
            ..Default::default()
        }
    }

    fn from_proto(c: &proto::ServerConfig) -> Self {
        Self {
            display: c.display,
            max_fps: c.max_fps,
            speed: c.speed.map(|s| s.min(10) as u8),
            quantizer: c.quantizer.map(|q| q.min(255) as u8),
            encoder_threads: c.encoder_threads,
        }
    }
}

/// A [`ServerConfig`] with the server's defaults filled in.
#[derive(Debug, Clone)]
pub(crate) struct Settings {
    pub display: usize,
    pub max_fps: u32,
    pub encoder: EncoderSettings,
}

impl Settings {
    pub fn resolve(config: &ServerConfig) -> Self {
        let enc = EncoderSettings::default();
        Self {
            display: config.display.unwrap_or(0) as usize,
            max_fps: config.max_fps.unwrap_or(30).clamp(1, 240),
            encoder: EncoderSettings {
                speed: config.speed.unwrap_or(enc.speed).min(10),
                quantizer: config.quantizer.unwrap_or(enc.quantizer),
                threads: config.encoder_threads.map_or(enc.threads, |t| t as usize),
            },
        }
    }
}

/// The handshake failed because one side is older than the other supports.
///
/// Returned (inside the `anyhow::Error`) by [`Server::run`](crate::Server::run) and
/// [`Client::run`](crate::Client::run); retrieve it with `err.downcast_ref::<VersionMismatch>()`.
/// Both sides reach the same verdict independently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionMismatch {
    /// The server's protocol version is below what the client requires.
    ServerTooOld { server: String, server_version: u32, required: u32 },
    /// The client's protocol version is below what the server requires.
    ClientTooOld { client: String, client_version: u32, required: u32 },
}

impl fmt::Display for VersionMismatch {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::ServerTooOld { server, server_version, required } => write!(
                f,
                "server is too old: {server} speaks protocol {server_version}, the client needs at least {required}"
            ),
            Self::ClientTooOld { client, client_version, required } => write!(
                f,
                "client is too old: {client} speaks protocol {client_version}, the server needs at least {required}"
            ),
        }
    }
}

impl std::error::Error for VersionMismatch {}

/// Both sides run this on the same four numbers, so they agree on the outcome.
fn check(server: &ServerInfo, client: &ClientHello) -> Result<(), VersionMismatch> {
    if server.protocol_version < client.min_server_protocol_version {
        return Err(VersionMismatch::ServerTooOld {
            server: name(&server.software_version),
            server_version: server.protocol_version,
            required: client.min_server_protocol_version,
        });
    }
    if client.protocol_version < server.min_client_protocol_version {
        return Err(VersionMismatch::ClientTooOld {
            client: name(&client.software_version),
            client_version: client.protocol_version,
            required: server.min_client_protocol_version,
        });
    }
    Ok(())
}

/// Peers from before versioning existed send no software version.
fn name(software_version: &str) -> String {
    if software_version.is_empty() {
        "an unversioned peer".into()
    } else {
        software_version.into()
    }
}

async fn receive<R: AsyncRead + Unpin>(reader: &mut MessageReader<R>, what: &str) -> Result<Union> {
    let msg = tokio::time::timeout(TIMEOUT, reader.next())
        .await
        .with_context(|| format!("timed out waiting for {what}"))??;
    match msg {
        None => bail!("peer disconnected before sending {what}"),
        Some(Message { union: Some(u), .. }) => Ok(u),
        Some(_) => bail!("expected {what}, got an unknown message"),
    }
}

/// The server's half: introduces itself, then returns the settings the client asked for.
pub(crate) async fn server<R, W>(
    reader: &mut MessageReader<R>,
    writer: &mut MessageWriter<W>,
    source_url: &str,
) -> Result<ServerConfig>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let info = ServerInfo {
        source_url: source_url.into(),
        notice: crate::LEGAL_NOTICE.into(),
        protocol_version: PROTOCOL_VERSION,
        min_client_protocol_version: MIN_CLIENT_PROTOCOL_VERSION,
        software_version: software_version(),
        ..Default::default()
    };
    let mut msg = Message::new();
    msg.set_server_info(info.clone());
    writer.send(&msg).await?;

    let Union::ClientHello(hello) = receive(reader, "the client's hello").await? else {
        bail!("expected the client's hello");
    };
    check(&info, &hello)?;
    log::info!("client: {}", name(&hello.software_version));
    Ok(ServerConfig::from_proto(&hello.config))
}

/// What a client learns about the server it connected to, passed to
/// [`ClientConfig::on_server_info`](crate::ClientConfig::on_server_info).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerDetails {
    /// Software name and version, e.g. `"desklib 0.1.0"`; empty if the server didn't say.
    pub software_version: String,
    /// Where the source code of the server software can be obtained. The GNU AGPL (section 13)
    /// requires this offer to be shown to the people using the server through the client.
    pub source_url: String,
    /// The server software's copyright and license notice.
    pub notice: String,
}

/// The client's half: replies to the server's introduction with `config`.
pub(crate) async fn client<R, W>(
    reader: &mut MessageReader<R>,
    writer: &mut MessageWriter<W>,
    config: &ServerConfig,
) -> Result<ServerDetails>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let Union::ServerInfo(info) = receive(reader, "the server's info").await? else {
        bail!("expected the server's info");
    };
    let hello = ClientHello {
        protocol_version: PROTOCOL_VERSION,
        min_server_protocol_version: MIN_SERVER_PROTOCOL_VERSION,
        software_version: software_version(),
        config: Some(config.to_proto()).into(),
        ..Default::default()
    };
    // Sent even if the versions don't match, so the server can see why and give up too.
    let mut msg = Message::new();
    msg.set_client_hello(hello.clone());
    writer.send(&msg).await?;

    check(&info, &hello)?;
    log::info!("server: {}", name(&info.software_version));
    Ok(ServerDetails {
        software_version: info.software_version,
        source_url: info.source_url,
        notice: info.notice,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pipe() -> (
        (MessageReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>, MessageWriter<tokio::io::WriteHalf<tokio::io::DuplexStream>>),
        (MessageReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>, MessageWriter<tokio::io::WriteHalf<tokio::io::DuplexStream>>),
    ) {
        let (a, b) = tokio::io::duplex(4096);
        let (ar, aw) = tokio::io::split(a);
        let (br, bw) = tokio::io::split(b);
        (
            (MessageReader::new(ar), MessageWriter::new(aw)),
            (MessageReader::new(br), MessageWriter::new(bw)),
        )
    }

    #[tokio::test]
    async fn client_config_reaches_the_server() {
        let ((mut sr, mut sw), (mut cr, mut cw)) = pipe();
        let config = ServerConfig {
            display: Some(1),
            quantizer: Some(80),
            ..Default::default()
        };
        let (server, client) = tokio::join!(
            server(&mut sr, &mut sw, "https://example.com/src"),
            client(&mut cr, &mut cw, &config),
        );
        assert_eq!(server.unwrap(), config);
        assert_eq!(client.unwrap().source_url, "https://example.com/src");
    }

    #[test]
    fn unset_settings_take_the_server_defaults() {
        let s = Settings::resolve(&ServerConfig { max_fps: Some(1000), ..Default::default() });
        assert_eq!(s.display, 0);
        assert_eq!(s.max_fps, 240);
        assert_eq!(s.encoder.speed, EncoderSettings::default().speed);
        assert_eq!(s.encoder.quantizer, EncoderSettings::default().quantizer);
    }

    /// Plays a peer of another version against the real handshake, and returns what each
    /// side made of it.
    async fn against_server(hello: ClientHello) -> Result<ServerConfig> {
        let ((mut sr, mut sw), (mut cr, mut cw)) = pipe();
        let peer = async move {
            receive(&mut cr, "info").await.unwrap();
            let mut msg = Message::new();
            msg.set_client_hello(hello);
            cw.send(&msg).await.unwrap();
        };
        tokio::join!(server(&mut sr, &mut sw, "url"), peer).0
    }

    async fn against_client(info: ServerInfo) -> Result<ServerDetails> {
        let ((mut sr, mut sw), (mut cr, mut cw)) = pipe();
        let peer = async move {
            let mut msg = Message::new();
            msg.set_server_info(info);
            sw.send(&msg).await.unwrap();
            receive(&mut sr, "hello").await.unwrap()
        };
        let config = ServerConfig::default();
        let (result, hello) = tokio::join!(client(&mut cr, &mut cw, &config), peer);
        // The client replies even when it is about to give up.
        assert!(matches!(hello, Union::ClientHello(_)));
        result
    }

    fn mismatch<T>(r: Result<T>) -> VersionMismatch {
        match r {
            Ok(_) => panic!("expected a version mismatch"),
            Err(e) => e.downcast::<VersionMismatch>().expect("not a version mismatch"),
        }
    }

    #[tokio::test]
    async fn server_rejects_an_old_client() {
        let hello = ClientHello {
            protocol_version: MIN_CLIENT_PROTOCOL_VERSION - 1,
            software_version: "desklib 0.0.1".into(),
            ..Default::default()
        };
        assert_eq!(
            mismatch(against_server(hello).await),
            VersionMismatch::ClientTooOld {
                client: "desklib 0.0.1".into(),
                client_version: MIN_CLIENT_PROTOCOL_VERSION - 1,
                required: MIN_CLIENT_PROTOCOL_VERSION,
            }
        );
    }

    #[tokio::test]
    async fn server_rejects_a_client_that_needs_a_newer_server() {
        let hello = ClientHello {
            protocol_version: PROTOCOL_VERSION + 5,
            min_server_protocol_version: PROTOCOL_VERSION + 1,
            ..Default::default()
        };
        assert!(matches!(
            mismatch(against_server(hello).await),
            VersionMismatch::ServerTooOld { .. }
        ));
    }

    #[tokio::test]
    async fn client_rejects_an_unversioned_server() {
        // What a server from before the handshake existed sends.
        let info = ServerInfo { source_url: "url".into(), ..Default::default() };
        assert_eq!(
            mismatch(against_client(info).await),
            VersionMismatch::ServerTooOld {
                server: "an unversioned peer".into(),
                server_version: 0,
                required: MIN_SERVER_PROTOCOL_VERSION,
            }
        );
    }

    #[tokio::test]
    async fn client_rejects_a_server_that_needs_a_newer_client() {
        let info = ServerInfo {
            protocol_version: PROTOCOL_VERSION + 5,
            min_client_protocol_version: PROTOCOL_VERSION + 1,
            ..Default::default()
        };
        assert!(matches!(
            mismatch(against_client(info).await),
            VersionMismatch::ClientTooOld { .. }
        ));
    }

    #[tokio::test]
    async fn newer_peers_within_range_are_accepted() {
        let hello = ClientHello {
            protocol_version: PROTOCOL_VERSION + 3,
            min_server_protocol_version: PROTOCOL_VERSION,
            ..Default::default()
        };
        against_server(hello).await.unwrap();
        let info = ServerInfo {
            protocol_version: PROTOCOL_VERSION + 3,
            min_client_protocol_version: PROTOCOL_VERSION,
            ..Default::default()
        };
        against_client(info).await.unwrap();
    }
}
