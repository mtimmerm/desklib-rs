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

//! Length-delimited protobuf messages over an `AsyncRead` / `AsyncWrite` pair.

use crate::proto::Message;
use anyhow::{Context, Result};
use futures::{SinkExt, StreamExt};
use protobuf::Message as _;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

/// Large enough for a key frame of a big display.
const MAX_FRAME_LEN: usize = 64 * 1024 * 1024;

fn codec() -> LengthDelimitedCodec {
    LengthDelimitedCodec::builder()
        .max_frame_length(MAX_FRAME_LEN)
        .new_codec()
}

pub(crate) struct MessageReader<R> {
    inner: FramedRead<R, LengthDelimitedCodec>,
}

impl<R: AsyncRead + Unpin> MessageReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            inner: FramedRead::new(reader, codec()),
        }
    }

    /// Returns `None` once the peer has closed the stream.
    pub async fn next(&mut self) -> Result<Option<Message>> {
        match self.inner.next().await {
            None => Ok(None),
            Some(frame) => {
                let frame = frame.context("failed to read message")?;
                let msg = Message::parse_from_tokio_bytes(&frame.freeze())
                    .context("failed to parse message")?;
                Ok(Some(msg))
            }
        }
    }
}

pub(crate) struct MessageWriter<W> {
    inner: FramedWrite<W, LengthDelimitedCodec>,
}

impl<W: AsyncWrite + Unpin> MessageWriter<W> {
    pub fn new(writer: W) -> Self {
        Self {
            inner: FramedWrite::new(writer, codec()),
        }
    }

    pub async fn send(&mut self, msg: &Message) -> Result<()> {
        let bytes = msg.write_to_bytes()?;
        self.inner
            .send(bytes::Bytes::from(bytes))
            .await
            .context("failed to write message")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::KeyEvent;

    #[tokio::test]
    async fn round_trip() {
        let (a, b) = tokio::io::duplex(1024);
        let mut writer = MessageWriter::new(a);
        let mut reader = MessageReader::new(b);

        let mut msg = Message::new();
        msg.set_key_event(KeyEvent {
            usb_hid: 0x04,
            down: true,
            ..Default::default()
        });
        writer.send(&msg).await.unwrap();
        assert_eq!(reader.next().await.unwrap(), Some(msg));

        drop(writer);
        assert_eq!(reader.next().await.unwrap(), None);
    }
}
