// Copyright 2023 Divy Srivastava <dj.srivastava23@gmail.com>
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

macro_rules! repr_u8 {
    ($(#[$meta:meta])* $vis:vis enum $name:ident {
      $($(#[$vmeta:meta])* $vname:ident $(= $val:expr)?,)*
    }) => {
      $(#[$meta])*
      $vis enum $name {
        $($(#[$vmeta])* $vname $(= $val)?,)*
      }

      #[derive(Debug)]
      pub enum Error {
        InvalidValue,
      }

      impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
          match self {
            Error::InvalidValue => write!(f, "invalid value"),
          }
        }
      }

      impl std::error::Error for Error {}

      impl core::convert::TryFrom<u8> for $name {
        type Error = Error;

        fn try_from(v: u8) -> Result<Self, Self::Error> {
          match v {
            $(x if x == $name::$vname as u8 => Ok($name::$vname),)*
            _ => Err(Error::InvalidValue),
          }
        }
      }
    }
}

fn mask(payload: &[u8]) -> (Vec<u8>, [u8; 4]) {
  let mask = [1, 2, 3, 4];
  let mut masked = Vec::new();
  for (i, byte) in payload.iter().enumerate() {
    masked.push(byte ^ mask[i % 4]);
  }
  (masked, mask)
}

/// Represents a WebSocket frame.
pub struct Frame {
  /// Indicates if this is the final frame in a message.
  pub fin: bool,
  /// The opcode of the frame.
  pub opcode: OpCode,
  /// The masking key of the frame, if any.
  mask: Option<[u8; 4]>,
  /// The payload of the frame.
  pub payload: Vec<u8>,
}

const MAX_HEAD_SIZE: usize = 16;

impl Frame {
  /// Creates a new WebSocket `Frame`.
  pub fn new(
    fin: bool,
    opcode: OpCode,
    mask: Option<[u8; 4]>,
    payload: Vec<u8>,
  ) -> Self {
    Self {
      fin,
      opcode,
      mask,
      payload,
    }
  }

  /// Create a new WebSocket text `Frame`.
  ///
  /// This is a convenience method for `Frame::new(true, OpCode::Text, None, payload)`.
  ///
  /// This method does not check if the payload is valid UTF-8.
  pub fn text(payload: Vec<u8>) -> Self {
    Self {
      fin: true,
      opcode: OpCode::Text,
      mask: None,
      payload,
    }
  }

  /// Create a new WebSocket binary `Frame`.
  ///
  /// This is a convenience method for `Frame::new(true, OpCode::Binary, None, payload)`.
  pub fn binary(payload: Vec<u8>) -> Self {
    Self {
      fin: true,
      opcode: OpCode::Binary,
      mask: None,
      payload,
    }
  }

  /// Create a new WebSocket close `Frame`.
  ///
  /// This is a convenience method for `Frame::new(true, OpCode::Close, None, payload)`.
  ///
  /// This method does not check if `code` is a valid close code and `reason` is valid UTF-8.
  pub fn close(code: u16, reason: &[u8], m: bool) -> Self {
    let mut payload = Vec::with_capacity(2 + reason.len());
    payload.extend_from_slice(&code.to_be_bytes());
    payload.extend_from_slice(reason);
    if m {
      let (masked, mask) = mask(&payload);
      return Self {
        fin: true,
        opcode: OpCode::Close,
        mask: Some(mask),
        payload: masked,
      };
    }
    Self {
      fin: true,
      opcode: OpCode::Close,
      mask: None,
      payload,
    }
  }

  /// Create a new WebSocket close `Frame` with a raw payload.
  ///
  /// This is a convenience method for `Frame::new(true, OpCode::Close, None, payload)`.
  ///
  /// This method does not check if `payload` is valid Close frame payload.
  pub fn close_raw(payload: Vec<u8>, m: bool) -> Self {
    if m {
      let (masked, mask) = mask(&payload);
      return Self {
        fin: true,
        opcode: OpCode::Close,
        mask: Some(mask),
        payload: masked,
      };
    }

    Self {
      fin: true,
      opcode: OpCode::Close,
      mask: None,
      payload,
    }
  }

  /// Create a new WebSocket pong `Frame`.
  ///
  /// This is a convenience method for `Frame::new(true, OpCode::Pong, None, payload)`.
  pub fn pong(payload: Vec<u8>, m: bool) -> Self {
    if m {
      let (masked, mask) = mask(&payload);
      return Self {
        fin: true,
        opcode: OpCode::Pong,
        mask: Some(mask),
        payload: masked,
      };
    }

    Self {
      fin: true,
      opcode: OpCode::Pong,
      mask: None,
      payload,
    }
  }

  /// Checks if the frame payload is valid UTF-8.
  pub fn is_utf8(&self) -> bool {
    #[cfg(feature = "simd")]
    return simdutf8::basic::from_utf8(&self.payload).is_ok();

    #[cfg(not(feature = "simd"))]
    return std::str::from_utf8(&self.payload).is_ok();
  }

  /// Unmasks the frame payload in-place. This method does nothing if the frame is not masked.
  ///
  /// Note: By default, the frame payload is unmasked by `WebSocket::read_frame`.
  pub fn unmask(&mut self) {
    if let Some(mask) = self.mask {
      crate::mask::unmask(&mut self.payload, mask);
    }
  }

  /// Formats the frame header into the head buffer. Returns the size of the length field.
  ///
  /// # Panics
  ///
  /// This method panics if the head buffer is not at least n-bytes long, where n is the size of the length field (0, 2, 4, or 10)
  pub fn fmt_head(&mut self, head: &mut [u8]) -> usize {
    head[0] = (self.fin as u8) << 7 | (self.opcode as u8);

    let len = self.payload.len();
    let size = if len < 126 {
      head[1] = len as u8;
      2
    } else if len < 65536 {
      head[1] = 126;
      head[2..4].copy_from_slice(&(len as u16).to_be_bytes());
      4
    } else {
      head[1] = 127;
      head[2..10].copy_from_slice(&(len as u64).to_be_bytes());
      10
    };

    if let Some(mask) = self.mask {
      head[1] |= 0x80;
      head[size..size + 4].copy_from_slice(&mask);
      size + 4
    } else {
      size
    }
  }

  pub async fn writev<S>(
    &mut self,
    stream: &mut S,
  ) -> Result<(), std::io::Error>
  where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
  {
    use std::io::IoSlice;

    let mut head = [0; MAX_HEAD_SIZE];
    let size = self.fmt_head(&mut head);

    let total = size + self.payload.len();

    let mut b = [IoSlice::new(&head[..size]), IoSlice::new(&self.payload)];

    let mut n = stream.write_vectored(&b).await?;
    if n == total {
      return Ok(());
    }

    // Slighly more optimized than (unstable) write_all_vectored for 2 iovecs.
    while n < size {
      b[0] = IoSlice::new(&head[n..size]);
      n += stream.write_vectored(&b).await?;
    }

    // Header out of the way.
    if n < total && n > size {
      stream.write_all(&self.payload[n - size..]).await?;
    }

    Ok(())
  }

  /// Writes the frame to the buffer and returns a slice of the buffer containing the frame.
  pub fn write<'a>(&mut self, buf: &'a mut Vec<u8>) -> &'a [u8] {
    fn reserve_enough(buf: &mut Vec<u8>, len: usize) {
      if buf.len() < len {
        buf.resize(len, 0);
      }
    }
    let len = self.payload.len();
    reserve_enough(buf, len + MAX_HEAD_SIZE);

    let size = self.fmt_head(buf);
    buf[size..size + len].copy_from_slice(&self.payload);
    &buf[..size + len]
  }
}

repr_u8! {
    #[repr(u8)]
    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    pub enum OpCode {
        Continuation = 0x0,
        Text = 0x1,
        Binary = 0x2,
        Close = 0x8,
        Ping = 0x9,
        Pong = 0xA,
    }
}

#[inline]
pub fn is_control(opcode: OpCode) -> bool {
  matches!(opcode, OpCode::Close | OpCode::Ping | OpCode::Pong)
}
