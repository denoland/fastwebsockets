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

#[cfg(feature = "unstable-split")]
use std::future::Future;
use std::ops::Deref;

use flate2::{Compress, Compression, Decompress};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::error::WebSocketError;
use crate::fragment_compressor::FragmentCompressor;
use crate::frame::Frame;
use crate::permessage_deflate::PermessageDeflateWebSocketExtension;
use crate::OpCode;
use crate::ReadHalf;
use crate::Role;
use crate::WebSocket;
#[cfg(feature = "unstable-split")]
use crate::WebSocketRead;
use crate::WriteHalf;

pub enum Fragment {
  Text(Option<utf8::Incomplete>, Vec<u8>, usize),
  Binary(Vec<u8>),
}

impl Fragment {
  /// Returns the payload of the fragment.
  fn take_buffer(self) -> Vec<u8> {
    match self {
      Fragment::Text(_, buffer, _) => buffer,
      Fragment::Binary(buffer) => buffer,
    }
  }
}

/// Collects fragmented messages over a WebSocket connection and returns the completed message once all fragments have been received.
///
/// This is useful for applications that do not want to deal with fragmented messages and the default behavior of tungstenite.
/// The payload is buffered in memory until the final fragment is received
/// so use this when streaming messages is not an option.
///
/// # Example
///
/// ```
/// use tokio::net::TcpStream;
/// use fastwebsockets::{WebSocket, FragmentCollector, OpCode, Role};
/// use anyhow::Result;
///
/// async fn handle_client(
///   socket: TcpStream,
/// ) -> Result<()> {
///   let ws = WebSocket::after_handshake(socket, Role::Server);
///   let mut ws = FragmentCollector::new(ws);
///
///   loop {
///     let frame = ws.read_frame().await?;
///     match frame.opcode {
///       OpCode::Close => break,
///       OpCode::Text | OpCode::Binary => {
///         ws.write_frame(frame).await?;
///       }
///       _ => {}
///     }
///   }
///   Ok(())
/// }
/// ```
///
pub struct FragmentCollector<S> {
  stream: S,
  read_half: ReadHalf,
  write_half: WriteHalf,

  fragments: Fragments,
  compressor: Option<Compress>,
  permessage_deflate: Option<PermessageDeflateWebSocketExtension>,
}

impl<'f, S> FragmentCollector<S> {
  /// Creates a new `FragmentCollector` with the provided `WebSocket`.
  pub fn new(ws: WebSocket<S>) -> FragmentCollector<S>
  where
    S: AsyncRead + AsyncWrite + Unpin,
  {
    let (stream, read_half, write_half, permessage_deflate) =
      ws.into_parts_internal();

    let (compressor, decompressor_window_bits) = match permessage_deflate {
      Some(ref permessage_deflate) => {
        let (compressor_window_bits, decompressor_window_bits) =
          match read_half.role {
            Role::Client => (
              permessage_deflate
                .client_max_window_bits
                .unwrap_or(Some(15)),
              permessage_deflate.server_max_window_bits,
            ),

            Role::Server => (
              permessage_deflate.server_max_window_bits,
              permessage_deflate
                .client_max_window_bits
                .unwrap_or(Some(15)),
            ),
          };

        let compressor_window_bits = compressor_window_bits.unwrap_or(15);
        let decompressor_window_bits = decompressor_window_bits.unwrap_or(15);

        (
          Some(Compress::new_with_window_bits(
            Compression::default(),
            false,
            compressor_window_bits,
          )),
          Some(decompressor_window_bits),
        )
      }
      None => (None, None),
    };

    FragmentCollector {
      stream,
      read_half,
      write_half,
      fragments: Fragments::new(decompressor_window_bits),
      compressor,
      permessage_deflate,
    }
  }

  /// Reads a WebSocket frame, collecting fragmented messages until the final frame is received and returns the completed message.
  ///
  /// Text frames payload is guaranteed to be valid UTF-8.
  pub async fn read_frame(&mut self) -> Result<Frame<'f>, WebSocketError>
  where
    S: AsyncRead + AsyncWrite + Unpin,
  {
    let use_context_takeover =
      self
        .permessage_deflate
        .as_ref()
        .map_or(true, |permessage_deflate| match self.write_half.role {
          Role::Client => permessage_deflate.server_context_takeover,
          Role::Server => permessage_deflate.client_context_takeover,
        });

    loop {
      let (res, obligated_send) =
        self.read_half.read_frame_inner(&mut self.stream).await;
      let is_closed = self.write_half.closed;
      if let Some(obligated_send) = obligated_send {
        if !is_closed {
          self.write_frame(obligated_send).await?;
        }
      }
      let Some(frame) = res? else {
        continue;
      };
      if is_closed && frame.opcode != OpCode::Close {
        return Err(WebSocketError::ConnectionClosed);
      }
      if let Some(frame) = self
        .fragments
        .accumulate(frame, self.read_half.max_message_size)?
      {
        if !use_context_takeover {
          self.fragments.reset();
        }

        return Ok(frame);
      }
    }
  }

  /// See `WebSocket::write_frame`.
  pub async fn write_frame(
    &mut self,
    frame: Frame<'f>,
  ) -> Result<(), WebSocketError>
  where
    S: AsyncRead + AsyncWrite + Unpin,
  {
    let can_compress = match frame.opcode {
      OpCode::Continuation | OpCode::Text | OpCode::Binary => true,
      OpCode::Close | OpCode::Ping | OpCode::Pong => false,
    };

    let frame = frame;

    if can_compress && !frame.compressed {
      if let Some(compressor) = self.compressor.as_mut() {
        let use_context_takeover =
          self
            .permessage_deflate
            .as_ref()
            .map_or(true, |permessage_deflate| match self.write_half.role {
              Role::Client => permessage_deflate.client_context_takeover,
              Role::Server => permessage_deflate.server_context_takeover,
            });

        let mut fragment_compressor =
          FragmentCompressor::new(frame.payload.deref(), compressor);

        let mut first = true;

        while let Some(fragment) = fragment_compressor.next() {
          let (done, payload) = fragment.map_err(|err| {
            eprintln!("{:?}", err);
            WebSocketError::InvalidEncoding
          })?;

          if payload.is_empty() {
            continue;
          }

          let opcode = if first {
            first = false;
            frame.opcode
          } else {
            OpCode::Continuation
          };

          let frame = Frame::new(done, opcode, None, payload.into(), true);
          self.write_half.write_frame(&mut self.stream, frame).await?;
        }

        if !use_context_takeover {
          compressor.reset();
        }

        return Ok(());
      }
    }

    self.write_half.write_frame(&mut self.stream, frame).await?;

    Ok(())
  }

  /// Consumes the `FragmentCollector` and returns the underlying stream.
  #[inline]
  pub fn into_inner(self) -> S {
    self.stream
  }
}

#[cfg(feature = "unstable-split")]
pub struct FragmentCollectorRead<S> {
  stream: S,
  read_half: ReadHalf,
  fragments: Fragments,
}

#[cfg(feature = "unstable-split")]
impl<'f, S> FragmentCollectorRead<S> {
  /// Creates a new `FragmentCollector` with the provided `WebSocket`.
  pub fn new(ws: WebSocketRead<S>) -> FragmentCollectorRead<S>
  where
    S: AsyncRead + Unpin,
  {
    let (stream, read_half) = ws.into_parts_internal();
    FragmentCollectorRead {
      stream,
      read_half,
      fragments: Fragments::new(None),
    }
  }

  /// Reads a WebSocket frame, collecting fragmented messages until the final frame is received and returns the completed message.
  ///
  /// Text frames payload is guaranteed to be valid UTF-8.
  ///
  /// # Arguments
  ///
  /// * `send_fn`: Closure must ensure frames are sent by write side of split WebSocket to correctly implement auto-close and auto-pong.
  pub async fn read_frame<R, E>(
    &mut self,
    send_fn: &mut impl FnMut(Frame<'f>) -> R,
  ) -> Result<Frame<'f>, WebSocketError>
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
      let Some(frame) = res? else {
        continue;
      };
      if let Some(frame) = self
        .fragments
        .accumulate(frame, self.read_half.max_message_size)?
      {
        return Ok(frame);
      }
    }
  }
}

/// Accumulates potentially fragmented [`Frame`]s to defragment the incoming WebSocket stream.
struct Fragments {
  fragments: Option<Fragment>,

  opcode: OpCode,
  compressed: bool,

  decompressor: Option<Decompress>,
}

impl Fragments {
  pub fn new(window_bits: Option<u8>) -> Self {
    let decompressor = window_bits.map(|window_bits: _| {
      Decompress::new_with_window_bits(false, window_bits)
    });

    Self {
      fragments: None,

      opcode: OpCode::Close,
      compressed: false,

      decompressor,
    }
  }

  pub fn reset(&mut self) {
    if let Some(decompressor) = self.decompressor.as_mut() {
      decompressor.reset(false);
    }
  }

  pub fn accumulate<'f>(
    &mut self,
    frame: Frame<'f>,
    max_message_size: usize,
  ) -> Result<Option<Frame<'f>>, WebSocketError> {
    match frame.opcode {
      OpCode::Text | OpCode::Binary => {
        if frame.fin {
          if self.fragments.is_some() {
            return Err(WebSocketError::InvalidFragment);
          }

          let frame = if frame.compressed {
            frame
              .inflate(&mut self.decompressor.as_mut().unwrap())
              .unwrap()
          } else {
            frame
          };

          // Validate UTF-8 for unfragmented text messages
          if frame.opcode == OpCode::Text {
            match utf8::decode(&frame.payload) {
              Ok(_) => {}
              Err(utf8::DecodeError::Incomplete { .. }) => {
                return Err(WebSocketError::InvalidUTF8);
              }
              Err(utf8::DecodeError::Invalid { .. }) => {
                return Err(WebSocketError::InvalidUTF8);
              }
            }
          }

          return Ok(Some(Frame::new(
            true,
            frame.opcode,
            None,
            frame.payload,
            false,
          )));
        } else {
          if frame.payload.len() >= max_message_size {
            return Err(WebSocketError::FrameTooLarge);
          }

          self.fragments = match frame.opcode {
            OpCode::Text => {
              if frame.compressed {
                Some(Fragment::Text(
                  None,
                  frame.payload.to_vec(),
                  frame.payload.len(),
                ))
              } else {
                match utf8::decode(&frame.payload) {
                  Ok(text) => Some(Fragment::Text(
                    None,
                    text.as_bytes().to_vec(),
                    frame.payload.len(),
                  )),
                  Err(utf8::DecodeError::Incomplete {
                    valid_prefix,
                    incomplete_suffix,
                  }) => Some(Fragment::Text(
                    Some(incomplete_suffix),
                    valid_prefix.as_bytes().to_vec(),
                    frame.payload.len(),
                  )),
                  Err(utf8::DecodeError::Invalid { .. }) => {
                    return Err(WebSocketError::InvalidUTF8);
                  }
                }
              }
            }
            OpCode::Binary => Some(Fragment::Binary(frame.payload.into())),
            _ => unreachable!(),
          };

          self.opcode = frame.opcode;
          self.compressed = frame.compressed;
        }
      }
      OpCode::Continuation => match self.fragments.as_mut() {
        None => {
          return Err(WebSocketError::InvalidContinuationFrame);
        }
        Some(Fragment::Text(data, input, message_len)) => {
          let new_message_len = message_len
            .checked_add(frame.payload.len())
            .ok_or(WebSocketError::FrameTooLarge)?;

          if new_message_len >= max_message_size {
            return Err(WebSocketError::FrameTooLarge);
          }
          *message_len = new_message_len;

          if self.compressed {
            input.extend_from_slice(&frame.payload[..]);
          } else {
            let mut tail = &frame.payload[..];
            if let Some(mut incomplete) = data.take() {
              if let Some((result, rest)) =
                incomplete.try_complete(&frame.payload)
              {
                tail = rest;
                match result {
                  Ok(text) => {
                    input.extend_from_slice(text.as_bytes());
                  }
                  Err(_) => {
                    return Err(WebSocketError::InvalidUTF8);
                  }
                }
              } else {
                tail = &[];
                data.replace(incomplete);
              }
            }

            match utf8::decode(tail) {
              Ok(text) => {
                input.extend_from_slice(text.as_bytes());
              }
              Err(utf8::DecodeError::Incomplete {
                valid_prefix,
                incomplete_suffix,
              }) => {
                input.extend_from_slice(valid_prefix.as_bytes());
                *data = Some(incomplete_suffix);
              }
              Err(utf8::DecodeError::Invalid { valid_prefix, .. }) => {
                input.extend_from_slice(valid_prefix.as_bytes());
                return Err(WebSocketError::InvalidUTF8);
              }
            }
          }

          if frame.fin {
            let final_frame = Frame::new(
              true,
              self.opcode,
              None,
              self.fragments.take().unwrap().take_buffer().into(),
              self.compressed,
            );

            let final_frame = if final_frame.compressed {
              let final_frame = final_frame
                .inflate(self.decompressor.as_mut().unwrap())
                .unwrap();

              if utf8::decode(&final_frame.payload[..]).is_err() {
                return Err(WebSocketError::InvalidUTF8);
              }
              final_frame
            } else {
              final_frame
            };

            return Ok(Some(final_frame));
          }
        }

        Some(Fragment::Binary(data)) => {
          let message_len = data
            .len()
            .checked_add(frame.payload.len())
            .ok_or(WebSocketError::FrameTooLarge)?;

          if message_len >= max_message_size {
            return Err(WebSocketError::FrameTooLarge);
          }

          data.extend_from_slice(&frame.payload);

          if frame.fin {
            let frame = Frame::new(
              true,
              self.opcode,
              None,
              self.fragments.take().unwrap().take_buffer().into(),
              self.compressed,
            );

            let frame = if frame.compressed {
              frame.inflate(self.decompressor.as_mut().unwrap()).unwrap()
            } else {
              frame
            };

            return Ok(Some(frame));
          }
        }
      },
      _ => return Ok(Some(frame)),
    }

    Ok(None)
  }
}
