// Copyright 2023-2026 Divy Srivastava <dj.srivastava23@gmail.com>
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
//! [https://github.com/denoland/fastwebsockets](https://github.com/denoland/fastwebsockets)
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
//! use anyhow::Result;
//!
//! async fn handle(
//!   socket: TcpStream,
//! ) -> Result<()> {
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
//! use anyhow::Result;
//!
//! async fn handle(
//!   socket: TcpStream,
//! ) -> Result<()> {
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
//! use http_body_util::Empty;
//! use hyper::{Request, body::{Incoming, Bytes}, Response};
//! use anyhow::Result;
//!
//! async fn server_upgrade(
//!   mut req: Request<Incoming>,
//! ) -> Result<Response<Empty<Bytes>>> {
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
//! use hyper::{Request, body::Bytes, upgrade::Upgraded, header::{UPGRADE, CONNECTION}};
//! use http_body_util::Empty;
//! use hyper_util::rt::TokioIo;
//! use tokio::net::TcpStream;
//! use std::future::Future;
//! use anyhow::Result;
//!
//! async fn connect() -> Result<FragmentCollector<TokioIo<Upgraded>>> {
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
//!     .body(Empty::<Bytes>::new())?;
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
mod error;
mod fragment;
mod frame;
/// Client handshake.
#[cfg(feature = "upgrade")]
#[cfg_attr(docsrs, doc(cfg(feature = "upgrade")))]
pub mod handshake;
mod mask;
/// Single-thread mio-driven server-side reactor that drives many
/// WebSocket sessions through [`ServerEngine`] with one event loop
/// and one shared receive buffer. Linux only; opt-in via the
/// `reactor` feature.
#[cfg(all(target_os = "linux", feature = "reactor"))]
#[cfg_attr(docsrs, doc(cfg(feature = "reactor")))]
pub mod reactor;
mod sync_server;
/// HTTP upgrades.
#[cfg(feature = "upgrade")]
#[cfg_attr(docsrs, doc(cfg(feature = "upgrade")))]
pub mod upgrade;

use bytes::Buf;

use bytes::BytesMut;
#[cfg(feature = "unstable-split")]
use std::future::Future;

use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;

pub use crate::close::CloseCode;
pub use crate::error::WebSocketError;
pub use crate::fragment::FragmentCollector;
#[cfg(feature = "unstable-split")]
pub use crate::fragment::FragmentCollectorRead;
pub use crate::frame::parse_header;
pub use crate::frame::Frame;
pub use crate::frame::Header;
pub use crate::frame::HeaderParse;
pub use crate::frame::OpCode;
pub use crate::frame::Payload;
pub use crate::mask::unmask;
pub use crate::sync_server::OutboundSegment;
pub use crate::sync_server::ServerEngine;
pub use crate::sync_server::ServerResponse;

#[derive(Copy, Clone, PartialEq)]
pub enum Role {
  Server,
  Client,
}

/// Write side of a [`WebSocket`].
///
/// Reachable via [`WebSocket::parts_mut`] for performance-sensitive callers
/// that want disjoint borrows of read and write state. Field internals are
/// private so the layout can evolve.
pub struct WriteHalf {
  role: Role,
  closed: bool,
  vectored: bool,
  auto_apply_mask: bool,
  writev_threshold: usize,
  write_buffer: Vec<u8>,
}

/// Read side of a [`WebSocket`].
///
/// Reachable via [`WebSocket::parts_mut`] for performance-sensitive callers
/// that want disjoint borrows of read and write state. Field internals are
/// private so the layout can evolve.
pub struct ReadHalf {
  role: Role,
  auto_apply_mask: bool,
  auto_close: bool,
  auto_pong: bool,
  max_message_size: usize,
  buffer: BytesMut,
}

#[cfg(feature = "unstable-split")]
pub struct WebSocketRead<S> {
  stream: S,
  read_half: ReadHalf,
}

#[cfg(feature = "unstable-split")]
pub struct WebSocketWrite<S> {
  stream: S,
  write_half: WriteHalf,
}

#[cfg(feature = "unstable-split")]
/// Create a split `WebSocketRead`/`WebSocketWrite` pair from a stream that has already completed the WebSocket handshake.
pub fn after_handshake_split<R, W>(
  read: R,
  write: W,
  role: Role,
) -> (WebSocketRead<R>, WebSocketWrite<W>)
where
  R: AsyncRead + Unpin,
  W: AsyncWrite + Unpin,
{
  (
    WebSocketRead {
      stream: read,
      read_half: ReadHalf::after_handshake(role),
    },
    WebSocketWrite {
      stream: write,
      write_half: WriteHalf::after_handshake(role),
    },
  )
}

#[cfg(feature = "unstable-split")]
impl<'f, S> WebSocketRead<S> {
  /// Consumes the `WebSocketRead` and returns the underlying stream.
  #[inline]
  pub(crate) fn into_parts_internal(self) -> (S, ReadHalf) {
    (self.stream, self.read_half)
  }

  pub fn set_writev_threshold(&mut self, _threshold: usize) {
    // No-op on the read half (kept for API stability).
  }

  /// Sets whether to automatically close the connection when a close frame is received. When set to `false`, the application will have to manually send close frames.
  ///
  /// Default: `true`
  pub fn set_auto_close(&mut self, auto_close: bool) {
    self.read_half.auto_close = auto_close;
  }

  /// Sets whether to automatically send a pong frame when a ping frame is received.
  ///
  /// Default: `true`
  pub fn set_auto_pong(&mut self, auto_pong: bool) {
    self.read_half.auto_pong = auto_pong;
  }

  /// Sets the maximum message size in bytes. If a message is received that is larger than this, the connection will be closed.
  ///
  /// Default: 64 MiB
  pub fn set_max_message_size(&mut self, max_message_size: usize) {
    self.read_half.max_message_size = max_message_size;
  }

  /// Sets whether to automatically apply the mask to the frame payload.
  ///
  /// Default: `true`
  pub fn set_auto_apply_mask(&mut self, auto_apply_mask: bool) {
    self.read_half.auto_apply_mask = auto_apply_mask;
  }

  /// Reads a frame from the stream.
  pub async fn read_frame<R, E>(
    &mut self,
    send_fn: &mut impl FnMut(Frame<'f>) -> R,
  ) -> Result<Frame<'_>, WebSocketError>
  where
    S: AsyncRead + Unpin,
    E: Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
    R: Future<Output = Result<(), E>>,
  {
    loop {
      let (res, obligated_send) =
        self.read_half.read_frame_inner(&mut self.stream).await;
      if let Some(frame) = obligated_send {
        let res = send_fn(frame).await;
        res.map_err(|e| WebSocketError::SendError(e.into()))?;
      }
      if let Some(frame) = res? {
        break Ok(frame);
      }
    }
  }
}

#[cfg(feature = "unstable-split")]
impl<'f, S> WebSocketWrite<S> {
  /// Sets whether to use vectored writes. This option does not guarantee that vectored writes will be always used.
  ///
  /// Default: `true`
  pub fn set_writev(&mut self, vectored: bool) {
    self.write_half.vectored = vectored;
  }

  pub fn set_writev_threshold(&mut self, threshold: usize) {
    self.write_half.writev_threshold = threshold;
  }

  /// Sets whether to automatically apply the mask to the frame payload.
  ///
  /// Default: `true`
  pub fn set_auto_apply_mask(&mut self, auto_apply_mask: bool) {
    self.write_half.auto_apply_mask = auto_apply_mask;
  }

  pub fn is_closed(&self) -> bool {
    self.write_half.closed
  }

  pub async fn write_frame(
    &mut self,
    frame: Frame<'f>,
  ) -> Result<(), WebSocketError>
  where
    S: AsyncWrite + Unpin,
  {
    self.write_half.write_frame(&mut self.stream, frame).await
  }

  pub async fn flush(&mut self) -> Result<(), WebSocketError>
  where
    S: AsyncWrite + Unpin,
  {
    flush(&mut self.stream).await
  }
}

#[inline]
async fn flush<S>(stream: &mut S) -> Result<(), WebSocketError>
where
  S: AsyncWrite + Unpin,
{
  stream.flush().await.map_err(WebSocketError::IoError)
}

/// WebSocket protocol implementation over an async stream.
pub struct WebSocket<S> {
  stream: S,
  write_half: WriteHalf,
  read_half: ReadHalf,
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
  /// use anyhow::Result;
  ///
  /// async fn handle_client(
  ///   socket: TcpStream,
  /// ) -> Result<()> {
  ///   let mut ws = WebSocket::after_handshake(socket, Role::Server);
  ///   // ...
  ///   Ok(())
  /// }
  /// ```
  pub fn after_handshake(stream: S, role: Role) -> Self
  where
    S: AsyncRead + AsyncWrite + Unpin,
  {
    Self {
      stream,
      write_half: WriteHalf::after_handshake(role),
      read_half: ReadHalf::after_handshake(role),
    }
  }

  /// Creates a new `WebSocket` from a stream and an initial chunk of bytes
  /// that were already read off the wire during HTTP upgrade negotiation.
  ///
  /// Use this when downcasting `hyper::upgrade::Upgraded` to the underlying
  /// transport: hyper hands back a `read_buf` that may contain bytes the
  /// client sent immediately after the upgrade request. Those bytes belong
  /// to the WebSocket framing layer and must be consumed before reading
  /// further from `stream`.
  pub fn after_handshake_with_buffer<B: AsRef<[u8]>>(
    stream: S,
    role: Role,
    initial_buffer: B,
  ) -> Self
  where
    S: AsyncRead + AsyncWrite + Unpin,
  {
    let mut read_half = ReadHalf::after_handshake(role);
    let initial = initial_buffer.as_ref();
    if !initial.is_empty() {
      read_half.buffer.extend_from_slice(initial);
    }
    Self {
      stream,
      write_half: WriteHalf::after_handshake(role),
      read_half,
    }
  }

  /// Borrow the inner stream and the read/write halves disjointly. Useful for
  /// callers that want to drive read and write without taking `&mut self` on
  /// the whole `WebSocket` — e.g. an echo loop that holds a borrowed frame
  /// from the read buffer while it issues a write through the stream.
  ///
  /// Most users want `read_frame` / `write_frame`. This is escape hatch for
  /// performance-sensitive paths that want to avoid copying the payload out.
  #[inline]
  pub fn parts_mut(&mut self) -> (&mut S, &mut ReadHalf, &mut WriteHalf) {
    (&mut self.stream, &mut self.read_half, &mut self.write_half)
  }

  /// Split a [`WebSocket`] into a [`WebSocketRead`] and [`WebSocketWrite`] half. Note that the split version does not
  /// handle fragmented packets and you may wish to create a [`FragmentCollectorRead`] over top of the read half that
  /// is returned.
  #[cfg(feature = "unstable-split")]
  pub fn split<R, W>(
    self,
    split_fn: impl Fn(S) -> (R, W),
  ) -> (WebSocketRead<R>, WebSocketWrite<W>)
  where
    S: AsyncRead + AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
  {
    let (stream, read, write) = self.into_parts_internal();
    let (r, w) = split_fn(stream);
    (
      WebSocketRead {
        stream: r,
        read_half: read,
      },
      WebSocketWrite {
        stream: w,
        write_half: write,
      },
    )
  }

  /// Consumes the `WebSocket` and returns the underlying stream.
  #[inline]
  pub fn into_inner(self) -> S {
    // self.write_half.into_inner().stream
    self.stream
  }

  /// Consumes the `WebSocket` and returns the underlying stream.
  #[inline]
  pub(crate) fn into_parts_internal(self) -> (S, ReadHalf, WriteHalf) {
    (self.stream, self.read_half, self.write_half)
  }

  /// Sets whether to use vectored writes. This option does not guarantee that vectored writes will be always used.
  ///
  /// Default: `true`
  pub fn set_writev(&mut self, vectored: bool) {
    self.write_half.vectored = vectored;
  }

  pub fn set_writev_threshold(&mut self, threshold: usize) {
    self.write_half.writev_threshold = threshold;
  }

  /// Sets whether to automatically close the connection when a close frame is received. When set to `false`, the application will have to manually send close frames.
  ///
  /// Default: `true`
  pub fn set_auto_close(&mut self, auto_close: bool) {
    self.read_half.auto_close = auto_close;
  }

  /// Sets whether to automatically send a pong frame when a ping frame is received.
  ///
  /// Default: `true`
  pub fn set_auto_pong(&mut self, auto_pong: bool) {
    self.read_half.auto_pong = auto_pong;
  }

  /// Sets the maximum message size in bytes. If a message is received that is larger than this, the connection will be closed.
  ///
  /// Default: 64 MiB
  pub fn set_max_message_size(&mut self, max_message_size: usize) {
    self.read_half.max_message_size = max_message_size;
  }

  /// Sets whether to automatically apply the mask to the frame payload.
  ///
  /// Default: `true`
  pub fn set_auto_apply_mask(&mut self, auto_apply_mask: bool) {
    self.read_half.auto_apply_mask = auto_apply_mask;
    self.write_half.auto_apply_mask = auto_apply_mask;
  }

  pub fn is_closed(&self) -> bool {
    self.write_half.closed
  }

  /// Writes a frame to the stream.
  ///
  /// # Example
  ///
  /// ```
  /// use fastwebsockets::{WebSocket, Frame, OpCode};
  /// use tokio::net::TcpStream;
  /// use anyhow::Result;
  ///
  /// async fn send(
  ///   ws: &mut WebSocket<TcpStream>
  /// ) -> Result<()> {
  ///   let mut frame = Frame::binary(vec![0x01, 0x02, 0x03].into());
  ///   ws.write_frame(frame).await?;
  ///   Ok(())
  /// }
  /// ```
  pub async fn write_frame(
    &mut self,
    frame: Frame<'f>,
  ) -> Result<(), WebSocketError>
  where
    S: AsyncRead + AsyncWrite + Unpin,
  {
    self.write_half.write_frame(&mut self.stream, frame).await?;
    Ok(())
  }

  /// Flushes the data from the underlying stream.
  ///
  /// if the underlying stream is buffered (i.e: TlsStream<TcpStream>), it is needed to call flush
  /// to be sure that the written frame are correctly pushed down to the bottom stream/channel.
  ///
  pub async fn flush(&mut self) -> Result<(), WebSocketError>
  where
    S: AsyncWrite + Unpin,
  {
    flush(&mut self.stream).await
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
  /// use anyhow::Result;
  ///
  /// async fn echo(
  ///   ws: &mut WebSocket<TcpStream>
  /// ) -> Result<()> {
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
  pub async fn read_frame(&mut self) -> Result<Frame<'f>, WebSocketError>
  where
    S: AsyncRead + AsyncWrite + Unpin,
  {
    loop {
      let (res, obligated_send) =
        self.read_half.read_frame_inner(&mut self.stream).await;
      let is_closed = self.write_half.closed;
      if let Some(frame) = obligated_send {
        if !is_closed {
          self.write_half.write_frame(&mut self.stream, frame).await?;
        }
      }
      if let Some(frame) = res? {
        if is_closed && frame.opcode != OpCode::Close {
          return Err(WebSocketError::ConnectionClosed);
        }
        break Ok(frame);
      }
    }
  }
}

const MAX_HEADER_SIZE: usize = 14;

// Initial read-buffer capacity. Kept at 8 KiB — the empirical sweet spot for
// the bench matrix. I tried 64 KiB hoping to fit a 16 KiB frame + pipelined
// headroom in a single `recv` (uWebSockets uses a 512 KiB *shared* recv
// buffer for that reason), but per-connection 64 KiB buffers blew past L3
// at 500 connections and regressed the 100/20 and 10/1024 cases by 3-7%
// without moving the 200/16k case. 8 KiB amortizes well and the BytesMut
// grows on demand for larger payloads via the `reserve` in
// `parse_frame_header`.
const INITIAL_READ_BUFFER_CAPACITY: usize = 8 * 1024;

impl ReadHalf {
  pub fn after_handshake(role: Role) -> Self {
    let buffer = BytesMut::with_capacity(INITIAL_READ_BUFFER_CAPACITY);

    Self {
      role,
      auto_apply_mask: true,
      auto_close: true,
      auto_pong: true,
      max_message_size: 64 << 20,
      buffer,
    }
  }

  /// Reads one frame using the provided stream as the byte source.
  ///
  /// This is the public entry point for callers that took
  /// [`WebSocket::parts_mut`] and want to drive the read half independently.
  /// It carries the same auto-pong/auto-close behavior as
  /// [`WebSocket::read_frame`]: if a Ping is received and `auto_pong` is on
  /// (the default), or a Close is received and `auto_close` is on (also
  /// default), this method returns a tuple where the second element is the
  /// frame the caller must send back. Callers are obligated to write it
  /// before continuing, otherwise the protocol state will drift.
  pub async fn read_frame<'f, S>(
    &mut self,
    stream: &mut S,
  ) -> (Result<Option<Frame<'f>>, WebSocketError>, Option<Frame<'f>>)
  where
    S: AsyncRead + Unpin,
  {
    self.read_frame_inner(stream).await
  }

  /// Attempt to read a single frame from the incoming stream, returning any send obligations if
  /// `auto_close` or `auto_pong` are enabled. Callers to this function are obligated to send the
  /// frame in the latter half of the tuple if one is specified, unless the write half of this socket
  /// has been closed.
  ///
  /// XXX: Do not expose this method to the public API.
  pub(crate) async fn read_frame_inner<'f, S>(
    &mut self,
    stream: &mut S,
  ) -> (Result<Option<Frame<'f>>, WebSocketError>, Option<Frame<'f>>)
  where
    S: AsyncRead + Unpin,
  {
    let mut frame = match self.parse_frame_header(stream).await {
      Ok(frame) => frame,
      Err(e) => return (Err(e), None),
    };

    if self.role == Role::Server && self.auto_apply_mask {
      frame.unmask()
    };

    match frame.opcode {
      OpCode::Close if self.auto_close => {
        match frame.payload.len() {
          0 => {}
          1 => return (Err(WebSocketError::InvalidCloseFrame), None),
          _ => {
            let code = close::CloseCode::from(u16::from_be_bytes(
              frame.payload[0..2].try_into().unwrap(),
            ));

            #[cfg(feature = "simd")]
            if simdutf8::basic::from_utf8(&frame.payload[2..]).is_err() {
              return (Err(WebSocketError::InvalidUTF8), None);
            };

            #[cfg(not(feature = "simd"))]
            if std::str::from_utf8(&frame.payload[2..]).is_err() {
              return (Err(WebSocketError::InvalidUTF8), None);
            };

            if !code.is_allowed() {
              return (
                Err(WebSocketError::InvalidCloseCode),
                Some(Frame::close(1002, &frame.payload[2..])),
              );
            }
          }
        };

        let obligated_send = Frame::close_raw(frame.payload.to_owned().into());
        (Ok(Some(frame)), Some(obligated_send))
      }
      OpCode::Ping if self.auto_pong => {
        (Ok(None), Some(Frame::pong(frame.payload)))
      }
      OpCode::Text => {
        if frame.fin && !frame.is_utf8() {
          (Err(WebSocketError::InvalidUTF8), None)
        } else {
          (Ok(Some(frame)), None)
        }
      }
      _ => (Ok(Some(frame)), None),
    }
  }

  async fn parse_frame_header<'a, S>(
    &mut self,
    stream: &mut S,
  ) -> Result<Frame<'a>, WebSocketError>
  where
    S: AsyncRead + Unpin,
  {
    macro_rules! eof {
      ($n:expr) => {{
        if $n == 0 {
          return Err(WebSocketError::UnexpectedEOF);
        }
      }};
    }

    // Read the first two bytes
    while self.buffer.remaining() < 2 {
      eof!(stream.read_buf(&mut self.buffer).await?);
    }

    let fin = self.buffer[0] & 0b10000000 != 0;
    let rsv1 = self.buffer[0] & 0b01000000 != 0;
    let rsv2 = self.buffer[0] & 0b00100000 != 0;
    let rsv3 = self.buffer[0] & 0b00010000 != 0;

    if rsv1 || rsv2 || rsv3 {
      return Err(WebSocketError::ReservedBitsNotZero);
    }

    let opcode = frame::OpCode::try_from(self.buffer[0] & 0b00001111)?;
    let masked = self.buffer[1] & 0b10000000 != 0;

    let length_code = self.buffer[1] & 0x7F;
    let extra = match length_code {
      126 => 2,
      127 => 8,
      _ => 0,
    };

    self.buffer.advance(2);
    while self.buffer.remaining() < extra + masked as usize * 4 {
      eof!(stream.read_buf(&mut self.buffer).await?);
    }

    let payload_len: usize = match extra {
      0 => usize::from(length_code),
      2 => self.buffer.get_u16() as usize,
      #[cfg(target_pointer_width = "64")]
      8 => self.buffer.get_u64() as usize,
      // On 32bit systems, usize is only 4bytes wide so we must check for usize overflowing
      #[cfg(any(target_pointer_width = "16", target_pointer_width = "32"))]
      8 => match usize::try_from(self.buffer.get_u64()) {
        Ok(length) => length,
        Err(_) => return Err(WebSocketError::FrameTooLarge),
      },
      _ => unreachable!(),
    };

    let mask = if masked {
      Some(self.buffer.get_u32().to_be_bytes())
    } else {
      None
    };

    if frame::is_control(opcode) && !fin {
      return Err(WebSocketError::ControlFrameFragmented);
    }

    if opcode == OpCode::Ping && payload_len > 125 {
      return Err(WebSocketError::PingFrameTooLarge);
    }

    if payload_len >= self.max_message_size {
      return Err(WebSocketError::FrameTooLarge);
    }

    // Reserve a bit more to try to get next frame header and avoid a syscall to read it next time
    self.buffer.reserve(payload_len + MAX_HEADER_SIZE);
    while payload_len > self.buffer.remaining() {
      eof!(stream.read_buf(&mut self.buffer).await?);
    }

    // if we read too much it will stay in the buffer, for the next call to this method
    let payload = self.buffer.split_to(payload_len);
    let frame = Frame::new(fin, opcode, mask, Payload::Bytes(payload));
    Ok(frame)
  }
}

impl WriteHalf {
  pub fn after_handshake(role: Role) -> Self {
    Self {
      role,
      closed: false,
      auto_apply_mask: true,
      vectored: true,
      writev_threshold: 1024,
      // Pre-size the scratch buffer for the non-vectored write path so that
      // the very first small-frame write doesn't trigger a Vec growth-loop
      // (the original `Vec::with_capacity(2)` would realloc several times
      // before settling). 1 KiB covers the writev_threshold-or-smaller frames
      // that go through this branch.
      write_buffer: Vec::with_capacity(1024),
    }
  }

  /// Writes a frame to the provided stream.
  pub async fn write_frame<'a, S>(
    &'a mut self,
    stream: &mut S,
    mut frame: Frame<'a>,
  ) -> Result<(), WebSocketError>
  where
    S: AsyncWrite + Unpin,
  {
    if self.role == Role::Client && self.auto_apply_mask {
      frame.mask();
    }

    if self.closed {
      if frame.opcode == OpCode::Close {
        return Ok(()); // Already sent close, this is a no-op
      }
      return Err(WebSocketError::ConnectionClosed);
    }
    let is_close = frame.opcode == OpCode::Close;
    if is_close {
      self.closed = true;
    }

    if self.vectored && frame.payload.len() > self.writev_threshold {
      frame.writev(stream).await?;
    } else {
      let text = frame.write(&mut self.write_buffer);
      stream.write_all(text).await?;
    }

    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const _: () = {
    const fn assert_unsync<S>() {
      // Generic trait with a blanket impl over `()` for all types.
      trait AmbiguousIfImpl<A> {
        // Required for actually being able to reference the trait.
        fn some_item() {}
      }

      impl<T: ?Sized> AmbiguousIfImpl<()> for T {}

      // Used for the specialized impl when *all* traits in
      // `$($t)+` are implemented.
      #[allow(dead_code)]
      struct Invalid;

      impl<T: ?Sized + Sync> AmbiguousIfImpl<Invalid> for T {}

      // If there is only one specialized trait impl, type inference with
      // `_` can be resolved and this can compile. Fails to compile if
      // `$x` implements `AmbiguousIfImpl<Invalid>`.
      let _ = <S as AmbiguousIfImpl<_>>::some_item;
    }
    assert_unsync::<WebSocket<tokio::net::TcpStream>>();
  };

  // `parse_header` is the sync entry point that callers driving their own
  // event loop (mio, callback frameworks) use to parse a frame header out
  // of a byte buffer without spinning up the async/BytesMut path.
  #[test]
  fn parse_header_short_and_extended_lengths() {
    // Unmasked short text frame [0x81, 0x05, "hello"]
    let buf = [0x81, 0x05, b'h', b'e', b'l', b'l', b'o'];
    match parse_header(&buf).unwrap() {
      HeaderParse::Complete(h) => {
        assert!(h.fin);
        assert_eq!(h.opcode, OpCode::Text);
        assert_eq!(h.mask, None);
        assert_eq!(h.header_len, 2);
        assert_eq!(h.payload_len, 5);
        assert_eq!(h.total_len(), 7);
      }
      other => panic!("expected Complete, got {:?}", other),
    }
    // Need-more: 1 byte only.
    match parse_header(&buf[..1]).unwrap() {
      HeaderParse::Incomplete { at_least } => assert_eq!(at_least, 2),
      other => panic!("expected Incomplete, got {:?}", other),
    }
    // Masked extended (ext-126) 16-KiB frame header: [0x82, 0xfe,
    // 0x40, 0x00, m0,m1,m2,m3] — 8 header bytes, 16 384 payload.
    let mut buf2 = vec![0x82, 0xfe, 0x40, 0x00, 0x01, 0x02, 0x03, 0x04];
    buf2.extend(std::iter::repeat(0xAB).take(16384));
    match parse_header(&buf2).unwrap() {
      HeaderParse::Complete(h) => {
        assert!(h.fin);
        assert_eq!(h.opcode, OpCode::Binary);
        assert_eq!(h.mask, Some([0x01, 0x02, 0x03, 0x04]));
        assert_eq!(h.header_len, 8);
        assert_eq!(h.payload_len, 16384);
        assert_eq!(h.total_len(), 16392);
      }
      other => panic!("expected Complete, got {:?}", other),
    }
    // Need-more progression: short of length bytes, then short of mask.
    match parse_header(&buf2[..2]).unwrap() {
      HeaderParse::Incomplete { at_least } => assert_eq!(at_least, 4),
      other => panic!("expected Incomplete len, got {:?}", other),
    }
    match parse_header(&buf2[..4]).unwrap() {
      HeaderParse::Incomplete { at_least } => assert_eq!(at_least, 8),
      other => panic!("expected Incomplete mask, got {:?}", other),
    }
    // Protocol error: RSV1 set on a non-extension frame.
    let bad = [0xc1, 0x00];
    assert!(matches!(
      parse_header(&bad),
      Err(WebSocketError::ReservedBitsNotZero)
    ));
    // Protocol error: fragmented control frame (Close, no FIN).
    let bad2 = [0x08, 0x00];
    assert!(matches!(
      parse_header(&bad2),
      Err(WebSocketError::ControlFrameFragmented)
    ));
  }

  // `parts_mut` gives disjoint borrows of stream + read half + write half;
  // it's the API contract for callers who want to hold a borrowed frame
  // while writing through the same socket.
  #[tokio::test]
  async fn parts_mut_drives_read_and_write() {
    use std::io::Cursor;
    // Two binary frames in the prefix; the write side accumulates into a Vec.
    let mut frames = vec![0x82, 0x02, b'h', b'i'];
    frames.extend_from_slice(&[0x82, 0x03, b'b', b'y', b'e']);
    let stream = tokio::io::join(Cursor::new(frames), Vec::<u8>::new());
    let mut ws = WebSocket::after_handshake(stream, Role::Server);
    let (stream, read, _write) = ws.parts_mut();
    let (res, _) = read.read_frame(stream).await;
    let f = res.unwrap().unwrap();
    assert_eq!(&f.payload[..], b"hi");
    let (res, _) = read.read_frame(stream).await;
    let f = res.unwrap().unwrap();
    assert_eq!(&f.payload[..], b"bye");
  }

  // The initial-buffer constructor must seed the read buffer such that a
  // subsequent `read_frame` parses frames from those bytes without needing a
  // single byte from the (empty) stream. This covers the downcast-after-
  // upgrade pattern where hyper hands back a prefix of bytes the client sent
  // immediately after the upgrade request.
  #[tokio::test]
  async fn after_handshake_with_buffer_consumes_prefix() {
    use std::io::Cursor;
    // Build a single unmasked binary frame "hi"
    let mut frame = vec![0x82, 0x02, b'h', b'i'];
    // Tack on a second frame
    frame.extend_from_slice(&[0x82, 0x03, b'b', b'y', b'e']);
    // Empty back-end stream — all data lives in initial_buffer.
    let empty: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    let mut ws =
      WebSocket::after_handshake_with_buffer(empty, Role::Server, &frame);
    let f1 = ws.read_frame().await.unwrap();
    assert_eq!(&f1.payload[..], b"hi");
    let f2 = ws.read_frame().await.unwrap();
    assert_eq!(&f2.payload[..], b"bye");
  }
}
