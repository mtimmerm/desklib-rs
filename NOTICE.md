# Notices

## desklib

desklib — remote desktop over any async stream
Copyright (C) 2026 Matt Timmermans <matt@timmermans.org>

This program is free software: you can redistribute it and/or modify it under the terms of
the GNU Affero General Public License, version 3, as published by the Free Software
Foundation. It is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY;
without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See
the GNU Affero General Public License, in [LICENSE](LICENSE), for more details.

The source code of this version is available at <https://github.com/mtimmerm/desklib-rs>.

## Derived from RustDesk

desklib is a modified version of **RustDesk**:

- Project: <https://github.com/rustdesk/rustdesk>
- Copyright (C) Purslane Tech Pte. Ltd. and the RustDesk contributors
- License: GNU Affero General Public License, version 3
- Based on commit `1297da6d186e58d9096be24072ae3998cdbbf8fe` (2026-10-24)

### Statement of modification

Modified by Matt Timmermans, October 2026 (first published 2026-10-25). RustDesk is an
application. desklib turns it into a library, keeping RustDesk's screen capture and input
injection code. The changes:

- Removed the user interfaces (Flutter, Sciter), rendezvous/relay networking, NAT traversal,
  encryption, configuration, IPC, service installation, updater, audio, clipboard, file
  transfer, terminal, port forwarding, printing, virtual displays, privacy mode, translations,
  packaging, and all C/C++ code and bindings, including the libvpx/libaom/libyuv codecs.
- Replaced RustDesk's session layer with a new server and client (`src/`) that run over any
  `AsyncRead`/`AsyncWrite` pair, with a new, much smaller wire protocol
  (`protos/message.proto`) and AV1 video via rav1e and rav1d.
- Portions of `src/` are adapted from RustDesk: cursor capture (`src/server/cursor/`) from
  RustDesk's `src/platform/windows.rs`, `windows.cc`, `linux.rs` and `macos.rs`; the capture
  loop and input replay (`src/server/`) from RustDesk's `src/server/`; and the protocol from
  RustDesk's `message.proto`.
- Modified the bundled libraries in `libs/` as listed below.

## Bundled components

Every file in `libs/` that desklib changed or added carries a notice at its top saying so.

### libs/scrap: screen capture

- From RustDesk's `libs/scrap`, itself derived from scrap by Ram (quadrupleslap),
  <https://github.com/quadrupleslap/scrap>, published under the MIT license (see
  [libs/scrap/LICENSE-MIT](libs/scrap/LICENSE-MIT)).
- The Wayland capture code (`src/wayland/`) is derived from Weylus,
  <https://github.com/H-M-H/Weylus>, Copyright (C) 2020-Present H-M-H, used with its author's
  consent. It is licensed under the GNU AGPL, version 3 or later.
- RustDesk's and desklib's changes are distributed under the GNU AGPL, version 3.
- desklib changes: removed the codec bindings (libvpx, libaom, libyuv, hardware and Android
  codecs), recording, camera, Android, DRM and magnifier capture, and the GPU-texture output
  path. Replaced libyuv rotation with Rust code (`src/dxgi/rotate.rs`), made GDI capture
  top-down, and removed the dependency on RustDesk's `hbb_common`.
- Files changed or added: `Cargo.toml`, `build.rs`, `src/lib.rs`, `src/common/mod.rs`,
  `src/common/dxgi.rs`, `src/dxgi/mod.rs`, `src/dxgi/gdi.rs`, `src/dxgi/hdr.rs`,
  `src/dxgi/rotate.rs` (new), `src/quartz/capturer.rs`, `src/quartz/config.rs`,
  `src/quartz/ffi.rs`, `src/wayland/display.rs`, `src/wayland/pipewire.rs`,
  `src/x11/capturer.rs`, `src/x11/ffi.rs`, `src/x11/iter.rs`, `src/x11/server.rs`.

### libs/enigo: input injection

- From RustDesk's `libs/enigo`, itself derived from enigo, <https://github.com/enigo-rs/enigo>,
  Copyright (c) 2017 pythoneer, MIT license (see [libs/enigo/LICENSE](libs/enigo/LICENSE)).
- `src/log_throttle.rs` is from RustDesk's `hbb_common` (<https://github.com/rustdesk/hbb_common>),
  part of RustDesk.
- RustDesk's and desklib's changes are distributed under the GNU AGPL, version 3.
- desklib changes: removed the dependency on `hbb_common` (vendoring `log_throttle.rs`), the
  build script and CI files.
- Files changed or added: `Cargo.toml`, `src/lib.rs`, `src/linux/xdo.rs`,
  `src/log_throttle.rs` (new here), `src/macos/macos_impl.rs`, `src/win/win_impl.rs`.

### libs/base: platform probes

- From RustDesk's `libs/base`, GNU AGPL, version 3.
- desklib changes: reduced it to the Linux display-server probes. Removed the protocol,
  configuration, file transfer and the Windows and macOS modules, and inlined shell helpers
  from `hbb_common`.
- Files changed: `Cargo.toml`, `src/lib.rs`, `src/platform/mod.rs`, `src/platform/linux.rs`.

### libs/libxdo-sys-stub: runtime libxdo loader

- From RustDesk's `libs/libxdo-sys-stub`, GNU AGPL, version 3.
- desklib changes: replaced its `hbb_common` dependency with direct dependencies.
- Files changed: `Cargo.toml`, `src/lib.rs`.

### libs/rav1e: AV1 encoder

- rav1e 0.8.1, <https://github.com/xiph/rav1e>, Copyright (c) 2017-2022 the rav1e
  contributors, BSD 2-Clause license and Alliance for Open Media Patent License 1.0 (see
  [libs/rav1e/LICENSE](libs/rav1e/LICENSE) and [libs/rav1e/PATENTS](libs/rav1e/PATENTS)).
- desklib changes: added `Context::set_zero_latency`, which outputs each frame as soon as it
  is encoded (`src/api/context.rs`, `src/api/internal.rs`). Fixed two compiler warnings
  (`src/predict.rs`, `src/tiling/tile_restoration_state.rs`).

## Dependencies

desklib also depends on other crates, which Cargo fetches and which keep their own licenses.
Among them, rav1d (<https://github.com/memorysafety/rav1d>, BSD 2-Clause) is the AV1
decoder, and rdev (<https://github.com/rustdesk-org/rdev>, MIT) handles key injection.
`cargo tree` lists them all.

## Your obligations under the GNU AGPL

If you convey desklib, or a work based on it, you must do so under the GNU AGPL, version 3,
and provide its Corresponding Source. If you modify desklib and run it so that users interact
with it over a network, section 13 requires you to offer those users its source code. The
server sends its offer, `ServerOptions::source_url`, to every client; set it to where the
source of your modified version can be obtained.

desklib's client shows the remote desktop in a window whose title the application chooses.
Applications built on it must let their users see, somewhere convenient such as an About box
(sections 5(d) and 13):

- desklib's copyright and license notice and source location: `desklib::LEGAL_NOTICE` and
  `desklib::SOURCE_URL`;
- the connected server's notice and source offer, which the client passes to the application
  through `ClientConfig::on_server_info`.

Keep the copyright, license and modification notices, including this file.
