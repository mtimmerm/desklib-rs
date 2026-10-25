# desklib

A Rust remote desktop library, derived from [RustDesk](https://github.com/rustdesk/rustdesk).

- `Server` captures the local desktop, sends it as AV1 video, and replays the peer's mouse
  and keyboard input.
- `Client` shows the remote desktop in a window, mirrors the remote cursor, and sends local
  mouse and keyboard input.

Both run over any `AsyncRead` + `AsyncWrite` pair. Transport, discovery, authentication and
encryption are up to you: wrap the stream in TLS, tunnel it over SSH, and so on.

Building needs a Rust toolchain plus [nasm](https://www.nasm.us) on the `PATH` (x86/x86_64)
or a C compiler (ARM), for the codecs' assembly. To build with Rust alone, disable the default
`asm` feature (see Performance).

```rust
// Server
let (reader, writer) = tcp_stream.into_split();
desklib::Server::new(Default::default()).run(reader, writer).await?;

// Client: blocks running the window (on macOS it must be the main thread)
let (reader, writer) = tcp_stream.into_split();
desklib::Client::new(Default::default()).run(reader, writer)?;
```

Try it with the examples (plain TCP, so only use them on a trusted network):

```
cargo run --release --example server -- 0.0.0.0:21118
cargo run --release --example client -- <server-ip>:21118
```

## Protocol

Length-delimited protobuf messages (`protos/message.proto`). A session opens with a handshake:

1. The server sends `ServerInfo`: its protocol version, the oldest client version it supports,
   its software version, and the AGPL source offer and license notice.
2. The client replies with `ClientHello`: its protocol version, the oldest server version it
   supports, its software version, and the session settings (`ServerConfig`: display, frame
   rate, encoder speed and quality, threads).

Each side checks the versions independently and closes the connection if the other side is
too old, returning a `VersionMismatch` error. The settings are all optional, so the server
uses its own defaults for anything the client leaves out, including settings added in newer
versions. The `.proto` file lists the rules for changing the protocol compatibly.

After the handshake:

- server → client: AV1 video frames, cursor shapes, position and visibility
- client → server: mouse moves, buttons, wheel, keys (as USB HID usages), key-frame requests

## Performance

The default `asm` feature builds rav1e and rav1d with their hand-written assembly, which
needs nasm (x86/x86_64) or a C compiler (ARM). With `--no-default-features` the build needs
only a Rust toolchain, but encoding is much slower. Measured on a 12-thread desktop at
2560×1440:

| | `asm` (default) | `--no-default-features` |
|-|-|-|
| key frame | 178 ms | 524 ms |
| typing | 49 ms | 83 ms |
| scrolling | 63 ms | 124 ms |
| unchanged screen | 48 ms | 76 ms |
| decode | 14–21 ms | 17–32 ms |

To measure on your machine: `cargo test --release [--no-default-features] -- --ignored --nocapture bench`.

Frames are only encoded when the screen changes. Build in release mode; debug builds already
optimize the codecs.

## Platform support

| | capture | cursor | input |
|-|-|-|-|
| Windows | DXGI (GDI fallback) | yes | yes |
| macOS | Quartz | yes | yes |
| Linux X11 | XShm | XFixes | XTest |
| Linux Wayland | PipeWire portal (`wayland` feature) | no | no |

Windows is tested. macOS and Linux type-check but haven't been run.

## Layout

- `src/server/`: capture/encode loop, input replay, cursor polling
- `src/client/`: network session and decoder, viewer window, key mapping
- `src/codec/`: rav1e encoder, rav1d decoder, YUV conversion
- `libs/scrap/`: screen capture
- `libs/enigo/`: mouse injection (keys go through `rdev`)
- `libs/base/`: Linux display-server probes
- `libs/rav1e/`: rav1e 0.8.1 with one addition, `Context::set_zero_latency`, so each frame
  is output immediately rather than after four frames of lookahead
- `libs/libxdo-sys-stub/`: loads libxdo at runtime, so it isn't needed at build time

## License and attribution

desklib is Copyright (C) 2026 Matt Timmermans and is licensed under the
[GNU Affero General Public License, version 3](LICENSE). It is a modified version of
[RustDesk](https://github.com/rustdesk/rustdesk), Copyright (C) Purslane Tech Pte. Ltd. and
the RustDesk contributors, which is also AGPL-3.0. Most of its screen capture and input code
comes from RustDesk.

[NOTICE.md](NOTICE.md) says what was changed from RustDesk and when, and credits the bundled
components and their licenses: scrap (MIT), enigo (MIT), Weylus (AGPL-3.0-or-later) and
rav1e (BSD-2-Clause).

If you distribute desklib or software built on it, the AGPL requires you to do so under the
AGPL and to provide the complete source code. If you modify it and let people use it over a
network, you must also offer them the source. The server sends its source offer
(`ServerOptions::source_url`; point it at your modified source) to every client. Applications
must let their users see the license notices and source offers, for example in an About box:
desklib's own (`desklib::LEGAL_NOTICE` and `desklib::SOURCE_URL`) and the server's, which the
client passes to `ClientConfig::on_server_info`. See NOTICE.md for details.
