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

//! _fastwebsockets_ is a minimal, fast WebSocket server implementation.
//!
//! [https://github.com/littledivy/fastwebsockets](https://github.com/littledivy/fastwebsockets)
//!
//! Passes the _Autobahn|TestSuite_ and fuzzed with LLVM's _libfuzzer_.
//!
//! You can use it as a raw websocket frame parser and deal with spec compliance yourself, or you can use it as a full-fledged websocket server.
//!
//! # Example
//!
//! ```
//! use tokio::net::TcpStream;
//! use fastwebsockets::{WebSocket, OpCode, Role};
//!
//! async fn handle(
//!   socket: TcpStream,
//! ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//!   let mut ws = WebSocket::after_handshake(socket, Role::Server);
//!   ws.set_writev(false);
//!   ws.set_auto_close(true);
//!   ws.set_auto_pong(true);
//!
//!   loop {
//!     let frame = ws.read_frame().await?;
//!     match frame.opcode {
//!       OpCode::Close => break,
//!       OpCode::Text | OpCode::Binary => {
//!         ws.write_frame(frame).await?;
//!       }
//!       _ => {}
//!     }
//!   }
//!   Ok(())
//! }
//! ```
//!
//! ## Fragmentation
//!
//! By default, fastwebsockets will give the application raw frames with FIN set. Other
//! crates like tungstenite which will give you a single message with all the frames
//! concatenated.
//!
//! For concanated frames, use `FragmentCollector`:
//! ```
//! use fastwebsockets::{FragmentCollector, WebSocket, Role};
//! use tokio::net::TcpStream;
//!
//! async fn handle(
//!   socket: TcpStream,
//! ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//!   let mut ws = WebSocket::after_handshake(socket, Role::Server);
//!   let mut ws = FragmentCollector::new(ws);
//!   let incoming = ws.read_frame().await?;
//!   // Always returns full messages
//!   assert!(incoming.fin);
//!   Ok(())
//! }
//! ```
//!
//! _permessage-deflate is not supported yet._
//!
//! ## HTTP Upgrades
//!
//! Enable the `upgrade` feature to do server-side upgrades and client-side
//! handshakes.
//!
//! This feature is powered by [hyper](https://docs.rs/hyper).
//!
//! ```
//! use fastwebsockets::upgrade::upgrade;
//! use hyper::{Request, Body, Response};
//!
//! async fn server_upgrade(
//!   mut req: Request<Body>,
//! ) -> Result<Response<Body>, Box<dyn std::error::Error + Send + Sync>> {
//!   let (response, fut) = upgrade(&mut req)?;
//!
//!   tokio::spawn(async move {
//!     let ws = fut.await;
//!     // Do something with the websocket
//!   });
//!
//!   Ok(response)
//! }
//! ```
//!
//! Use the `handshake` module for client-side handshakes.
//!
//! ```
//! use fastwebsockets::handshake;
//! use fastwebsockets::FragmentCollector;
//! use hyper::{Request, Body, upgrade::Upgraded, header::{UPGRADE, CONNECTION}};
//! use tokio::net::TcpStream;
//! use std::future::Future;
//!
//! // Define a type alias for convenience
//! type Result<T> =
//!   std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
//!
//! async fn connect() -> Result<FragmentCollector<Upgraded>> {
//!   let stream = TcpStream::connect("localhost:9001").await?;
//!
//!   let req = Request::builder()
//!     .method("GET")
//!     .uri("http://localhost:9001/")
//!     .header("Host", "localhost:9001")
//!     .header(UPGRADE, "websocket")
//!     .header(CONNECTION, "upgrade")
//!     .header(
//!       "Sec-WebSocket-Key",
//!       fastwebsockets::handshake::generate_key(),
//!     )
//!     .header("Sec-WebSocket-Version", "13")
//!     .body(Body::empty())?;
//!
//!   let (ws, _) = handshake::client(&SpawnExecutor, req, stream).await?;
//!   Ok(FragmentCollector::new(ws))
//! }
//!
//! // Tie hyper's executor to tokio runtime
//! struct SpawnExecutor;
//!
//! impl<Fut> hyper::rt::Executor<Fut> for SpawnExecutor
//! where
//!   Fut: Future + Send + 'static,
//!   Fut::Output: Send + 'static,
//! {
//!   fn execute(&self, fut: Fut) {
//!     tokio::task::spawn(fut);
//!   }
//! }
//! ```

#![cfg_attr(docsrs, feature(doc_cfg))]

mod close;
mod fragment;
mod frame;
/// Client handshake.
#[cfg(feature = "upgrade")]
#[cfg_attr(docsrs, doc(cfg(feature = "upgrade")))]
pub mod handshake;
mod mask;
/// HTTP upgrades.
#[cfg(feature = "upgrade")]
#[cfg_attr(docsrs, doc(cfg(feature = "upgrade")))]
pub mod upgrade;

use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

pub use crate::close::CloseCode;
pub use crate::fragment::FragmentCollector;
pub use crate::frame::Frame;
pub use crate::frame::OpCode;
pub use crate::frame::CowMut;
pub use crate::mask::unmask;

#[derive(PartialEq)]
pub enum Role {
  Server,
  Client,
}

// 512 KiB
const RECV_SIZE: usize = 524288;

static mut RECV_BUF: Option<Vec<u8>> = None;

/// WebSocket protocol implementation over an async stream.
pub struct WebSocket<S> {
  stream: S,
  write_buffer: Vec<u8>,
  vectored: bool,
  auto_close: bool,
  auto_pong: bool,
  max_message_size: usize,
  auto_apply_mask: bool,
  closed: bool,
  role: Role,
  nread: Option<usize>,
  read_offset: usize,
}

impl<'f, S> WebSocket<S> {
  /// Creates a new `WebSocket` from a stream that has already completed the WebSocket handshake.
  ///
  /// Use the `upgrade` feature to handle server upgrades and client handshakes.
  ///
  /// # Example
  ///
  /// ```
  /// use tokio::net::TcpStream;
  /// use fastwebsockets::{WebSocket, OpCode, Role};
  ///
  /// async fn handle_client(
  ///   socket: TcpStream,
  /// ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  ///   let mut ws = WebSocket::after_handshake(socket, Role::Server);
  ///   // ...
  ///   Ok(())
  /// }
  /// ```
  pub fn after_handshake(stream: S, role: Role) -> Self
  where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
  {
    unsafe {
      if RECV_BUF.is_none() {
        RECV_BUF = Some(vec![0; RECV_SIZE]);
      }
    }

    Self {
      stream,
      write_buffer: Vec::with_capacity(2),
      vectored: true,
      auto_close: true,
      auto_pong: true,
      auto_apply_mask: true,
      max_message_size: 64 << 20,
      nread: None,
      closed: false,
      role,
      read_offset: 0,
    }
  }

  /// Consumes the `WebSocket` and returns the underlying stream.
  #[inline]
  pub fn into_inner(self) -> S {
    self.stream
  }

  /// Sets whether to use vectored writes. This option does not guarantee that vectored writes will be always used.
  ///
  /// Default: `true`
  pub fn set_writev(&mut self, vectored: bool) {
    self.vectored = vectored;
  }

  /// Sets whether to automatically close the connection when a close frame is received. When set to `false`, the application will have to manually send close frames.
  ///
  /// Default: `true`
  pub fn set_auto_close(&mut self, auto_close: bool) {
    self.auto_close = auto_close;
  }

  /// Sets whether to automatically send a pong frame when a ping frame is received.
  ///
  /// Default: `true`
  pub fn set_auto_pong(&mut self, auto_pong: bool) {
    self.auto_pong = auto_pong;
  }

  /// Sets the maximum message size in bytes. If a message is received that is larger than this, the connection will be closed.
  ///
  /// Default: 64 MiB
  pub fn set_max_message_size(&mut self, max_message_size: usize) {
    self.max_message_size = max_message_size;
  }

  /// Sets whether to automatically apply the mask to the frame payload.
  ///
  /// Default: `true`
  pub fn set_auto_apply_mask(&mut self, auto_apply_mask: bool) {
    self.auto_apply_mask = auto_apply_mask;
  }

  /// Writes a frame to the stream.
  ///
  /// This method will not mask the frame payload.
  ///
  /// # Example
  ///
  /// ```
  /// use fastwebsockets::{WebSocket, Frame, OpCode};
  /// use tokio::net::TcpStream;
  ///
  /// async fn send(
  ///   ws: &mut WebSocket<TcpStream>
  /// ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  ///   let mut frame = Frame::binary(vec![0x01, 0x02, 0x03].into());
  ///   ws.write_frame(frame).await?;
  ///   Ok(())
  /// }
  /// ```
  pub async fn write_frame(
    &mut self,
    mut frame: Frame<'f>,
  ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
  where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
  {
    if self.role == Role::Client && self.auto_apply_mask {
      frame.mask();
    }

    if frame.opcode == OpCode::Close {
      self.closed = true;
    }

    if self.vectored {
      frame.writev(&mut self.stream).await?;
    } else {
      let text = frame.write(&mut self.write_buffer);
      self.stream.write_all(text).await?;
    }

    Ok(())
  }

  /// Reads a frame from the stream.
  ///
  /// This method will unmask the frame payload. For fragmented frames, use `FragmentCollector::read_frame`.
  ///
  /// Text frames payload is guaranteed to be valid UTF-8.
  ///
  /// # Example
  ///
  /// ```
  /// use fastwebsockets::{OpCode, WebSocket, Frame};
  /// use tokio::net::TcpStream;
  ///
  /// async fn echo(
  ///   ws: &mut WebSocket<TcpStream>
  /// ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  ///   let frame = ws.read_frame().await?;
  ///   match frame.opcode {
  ///     OpCode::Text | OpCode::Binary => {
  ///       ws.write_frame(frame).await?;
  ///     }
  ///     _ => {}
  ///   }
  ///   Ok(())
  /// }
  /// ```
  pub async fn read_frame(
    &mut self,
  ) -> Result<Frame<'f>, Box<dyn std::error::Error + Send + Sync>>
  where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
  {
    loop {
      let mut frame = self.parse_frame_header().await?;
      if self.role == Role::Server && self.auto_apply_mask {
        frame.unmask()
      };

      if self.closed && frame.opcode != OpCode::Close {
        return Err("connection is closed".into());
      }

      match frame.opcode {
        OpCode::Close if self.auto_close && !self.closed => {
          match frame.payload.len() {
            0 => {}
            1 => return Err("invalid close frame".into()),
            _ => {
              let code = close::CloseCode::from(u16::from_be_bytes(
                frame.payload[0..2].try_into().unwrap(),
              ));

              #[cfg(feature = "simd")]
              simdutf8::basic::from_utf8(&frame.payload[2..])?;

              #[cfg(not(feature = "simd"))]
              std::str::from_utf8(&frame.payload[2..])?;

              if !code.is_allowed() {
                let _ = self
                  .write_frame(Frame::close(1002, &frame.payload[2..]))
                  .await;

                return Err("invalid close code".into());
              }
            }
          };

          // let _ = self
          //  .write_frame(Frame::close_raw(frame.payload.clone()))
           // .await;
          break Ok(frame);
        }
        OpCode::Ping if self.auto_pong => {
          self.write_frame(Frame::pong(frame.payload)).await?;
        }
        OpCode::Text => {
          if frame.fin && !frame.is_utf8() {
            break Err("invalid utf-8".into());
          }

          break Ok(frame);
        }
        _ => break Ok(frame),
      }
    }
  }

  async fn parse_frame_header(
    &mut self,
  ) -> Result<Frame<'f>, Box<dyn std::error::Error + Send + Sync>>
  where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
  {
    macro_rules! eof {
      ($n:expr) => {{
        let n = $n;
        if n == 0 {
          return Err("unexpected eof".into());
        }
        n
      }};
    }

    let mut nread = self.nread.unwrap_or(0);
    // let head = &mut self.read_buffer[self.read_offset..];
    let head = unsafe {
      let buf = RECV_BUF.as_mut().unwrap();
      &mut buf[self.read_offset..]
    };

    while nread < 2 {
      nread += eof!(self.stream.read(&mut head[nread..]).await?);
    }

    let fin = head[0] & 0b10000000 != 0;

    let rsv1 = head[0] & 0b01000000 != 0;
    let rsv2 = head[0] & 0b00100000 != 0;
    let rsv3 = head[0] & 0b00010000 != 0;

    if rsv1 || rsv2 || rsv3 {
      return Err("reserved bits are not zero".into());
    }

    let opcode = frame::OpCode::try_from(head[0] & 0b00001111)?;
    let masked = head[1] & 0b10000000 != 0;

    let length_code = head[1] & 0x7F;
    let extra = match length_code {
      126 => 2,
      127 => 8,
      _ => 0,
    };

    let length: usize = if extra > 0 {
      while nread < 2 + extra {
        nread += eof!(self.stream.read(&mut head[nread..]).await?);
      }

      match extra {
        2 => u16::from_be_bytes(head[2..4].try_into().unwrap()) as usize,
        8 => usize::from_be_bytes(head[2..10].try_into().unwrap()),
        _ => unreachable!(),
      }
    } else {
      usize::from(length_code)
    };

    let mask = match masked {
      true => {
        while nread < 2 + extra + 4 {
          nread += eof!(self.stream.read(&mut head[nread..]).await?);
        }

        Some(head[2 + extra..2 + extra + 4].try_into().unwrap())
      }
      false => None,
    };

    if frame::is_control(opcode) && !fin {
      return Err("control frame must not be fragmented".into());
    }

    if opcode == OpCode::Ping && length > 125 {
      return Err("Ping frame too large".into());
    }

    if length >= self.max_message_size {
      return Err("Frame too large".into());
    }

    let required = 2 + extra + mask.map(|_| 4).unwrap_or(0) + length;

    self.nread = None;
    if required > nread {
      // Allocate more space
      let mut new_head = head.to_vec();
      new_head.resize(required, 0);

      self.stream.read_exact(&mut new_head[nread..]).await?;
      return Ok(Frame::new(
        fin,
        opcode,
        mask,
        CowMut::Owned(new_head[required - length..].to_vec()),
      ));
    }

    let payload = &mut head[required - length..required];
    let frame = Frame::new(
      fin,
      opcode,
      mask,
      unsafe {
        CowMut::Borrowed(std::mem::transmute(payload))
      }
    );

    if nread > required {
      // We read too much
      self.read_offset += required;
      self.nread = Some(nread - required);
    }

    Ok(frame)
  }
}
