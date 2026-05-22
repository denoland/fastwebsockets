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

//! Non-async, callback-driven server-side WebSocket framing engine.
//!
//! This module is the entry point for event-loop-based servers
//! (mio, epoll, io_uring, callback frameworks). It exposes the same
//! frame parse / SIMD unmask / response synthesis hot path that the
//! async [`WebSocket`](crate::WebSocket) uses, without any Tokio
//! dependency and without an async state machine. The caller owns
//! the socket I/O and the buffer; the engine owns the protocol.
//!
//! See `examples/echo_server_mio.rs` for an end-to-end example. The
//! abbreviated form is:
//!
//! ```no_run
//! use fastwebsockets::{ServerEngine, ServerResponse, OpCode};
//!
//! let mut engine = ServerEngine::new();
//! let mut buf = [0u8; 65536];
//! // read bytes into buf[..filled] from your socket; then:
//! # let filled = 0;
//! # let mut write_socket = |_bytes: &[u8]| {};
//! let consumed = engine
//!   .process(
//!     &mut buf[..filled],
//!     &mut write_socket,
//!     |payload, opcode| {
//!       match opcode {
//!         OpCode::Text | OpCode::Binary => ServerResponse::Echo,
//!         _ => ServerResponse::Discard,
//!       }
//!     },
//!   )
//!   .unwrap();
//! // advance your read cursor by `consumed`.
//! ```
//!
//! The engine handles the `Ping → Pong` and `Close` reply paths
//! itself, so the caller only sees data frames. For frames small
//! enough that the response header fits in the slot freed up by
//! in-place unmasking (payload < 65 536 bytes, masked input — which
//! is every client-to-server frame in the protocol), the engine
//! writes the response header into the input buffer and emits the
//! whole response as one contiguous slice; no extra allocation, no
//! scatter/gather. For larger frames it falls back to a 10-byte
//! stack header + a second write.
//!
//! Fragmentation is not yet handled by this engine — callers that
//! need to reassemble fragmented messages should use
//! [`FragmentCollector`](crate::FragmentCollector) on the async
//! path. PRs welcome.

use crate::frame::parse_header;
use crate::frame::HeaderParse;
use crate::frame::OpCode;
use crate::mask::unmask;
use crate::WebSocketError;

/// What the user's frame handler wants the engine to send back.
pub enum ServerResponse {
  /// Send the same payload back as a same-opcode, same-FIN response.
  /// This is the hot path: the engine uses in-place response
  /// synthesis where possible (no copy, no writev).
  Echo,
  /// Don't send anything for this frame.
  Discard,
}

/// Server-side WebSocket framing engine. Stateless except for a
/// (usually empty) partial-frame buffer used when one TCP read
/// doesn't deliver a complete header — for the typical case it
/// holds nothing and never allocates.
pub struct ServerEngine {
  /// Bytes left over from a previous `process` call that didn't form
  /// a complete frame on their own. Prepended to the next input.
  partial: Vec<u8>,
  /// `true` once a Close frame has been processed; further frames
  /// are rejected.
  closed: bool,
}

impl Default for ServerEngine {
  fn default() -> Self {
    Self::new()
  }
}

impl ServerEngine {
  pub fn new() -> Self {
    Self {
      partial: Vec::new(),
      closed: false,
    }
  }

  /// Whether the peer's Close frame has been seen.
  pub fn is_closed(&self) -> bool {
    self.closed
  }

  /// How many bytes of partial-frame state the engine is currently
  /// carrying. Should be 0 in the steady state; non-zero only when a
  /// previous `process` call ran out of bytes mid-frame.
  pub fn partial_len(&self) -> usize {
    self.partial.len()
  }

  /// Drive the framing state machine over `input`. For every
  /// complete data frame found, calls `handler(payload, opcode)`
  /// where `payload` is unmasked in place. The handler returns what
  /// to send back; the engine writes the wire bytes via the `write`
  /// callback (one or two calls per response — one contiguous call
  /// for the in-place fast path, two calls (header + payload) for
  /// the fallback).
  ///
  /// Control frames (Ping, Close) are handled by the engine
  /// automatically: Ping → Pong with the same payload, Close → echo
  /// the close frame back.
  ///
  /// Returns the number of bytes from `input` consumed. The caller
  /// should advance its read cursor by this amount; whatever's left
  /// in `input[consumed..]` plus the engine's internal partial state
  /// is what's still pending.
  pub fn process<W, H>(
    &mut self,
    input: &mut [u8],
    mut write: W,
    mut handler: H,
  ) -> Result<usize, WebSocketError>
  where
    W: FnMut(&[u8]),
    H: FnMut(&mut [u8], OpCode) -> ServerResponse,
  {
    if self.closed {
      return Ok(0);
    }

    // If we're carrying a partial frame from last time, prepend its
    // bytes to the start of `input` by memmove + write — same
    // contract the user already has on the buffer.
    if !self.partial.is_empty() {
      // Move existing input bytes to make room for partial at the
      // front. This only triggers in the rare partial-recv case.
      let need = self.partial.len();
      if input.len() < need {
        // Caller didn't give us enough room; refuse and let them
        // grow.
        return Err(WebSocketError::FrameTooLarge);
      }
      input.copy_within(0..(input.len() - need), need);
      input[..need].copy_from_slice(&self.partial);
      self.partial.clear();
    }

    let mut consumed = 0usize;
    let end = input.len();
    loop {
      let remaining = &mut input[consumed..end];
      let hdr = match parse_header(remaining)? {
        HeaderParse::Complete(h) => h,
        HeaderParse::Incomplete { .. } => break,
      };
      let frame_total = hdr.total_len();
      if frame_total > remaining.len() {
        break;
      }

      let payload_start = hdr.header_len;
      let payload_end = frame_total;

      // Unmask the payload in place. After this, the mask field in
      // the buffer is dead state we can overwrite.
      if let Some(m) = hdr.mask {
        unmask(&mut remaining[payload_start..payload_end], m);
      }

      // Control-frame paths short-circuit the user callback.
      match hdr.opcode {
        OpCode::Close => {
          // Echo the close frame back, then return — the connection
          // is dead.
          emit_response(
            remaining,
            &hdr,
            ResponseKind::Echo {
              opcode: OpCode::Close,
            },
            &mut write,
          );
          self.closed = true;
          consumed += frame_total;
          return Ok(consumed);
        }
        OpCode::Ping => {
          emit_response(
            remaining,
            &hdr,
            ResponseKind::Echo {
              opcode: OpCode::Pong,
            },
            &mut write,
          );
          consumed += frame_total;
          continue;
        }
        OpCode::Pong => {
          // Server received a pong for one of its own pings (rare in
          // the echo workload). Nothing to send.
          consumed += frame_total;
          continue;
        }
        OpCode::Text | OpCode::Binary => {
          // Fragmented start frame: this engine doesn't reassemble,
          // bail with an error so the caller can fall back to the
          // async FragmentCollector path if they need it.
          if !hdr.fin {
            return Err(WebSocketError::InvalidFragment);
          }
          let response =
            handler(&mut remaining[payload_start..payload_end], hdr.opcode);
          match response {
            ServerResponse::Echo => {
              emit_response(
                remaining,
                &hdr,
                ResponseKind::Echo { opcode: hdr.opcode },
                &mut write,
              );
            }
            ServerResponse::Discard => {
              consumed += frame_total;
              continue;
            }
          }
        }
        OpCode::Continuation => {
          // Same — engine doesn't reassemble. Caller's problem.
          return Err(WebSocketError::InvalidContinuationFrame);
        }
      }

      consumed += frame_total;
    }

    // Save any unparsable tail (an incomplete frame header or a
    // header without its full payload) for the next `process` call.
    if consumed < end {
      let tail = &input[consumed..end];
      if !tail.is_empty() {
        self.partial.extend_from_slice(tail);
        consumed = end;
      }
    }

    Ok(consumed)
  }
}

enum ResponseKind {
  /// Send back the same payload that's already in the buffer.
  /// `opcode` is the response opcode (e.g. Ping → Pong).
  Echo { opcode: OpCode },
}

#[inline]
fn emit_response<W: FnMut(&[u8])>(
  frame_buf: &mut [u8],
  hdr: &crate::frame::Header,
  kind: ResponseKind,
  write: &mut W,
) {
  match kind {
    ResponseKind::Echo { opcode } => {
      // Hot path: input was masked (so we have 4 bytes to spend
      // before the payload) and the response header is ≤ 4 bytes
      // (i.e. payload_len < 65 536, so ext-127 isn't needed). Slot
      // the response header right before the payload and emit one
      // contiguous slice.
      let masked = hdr.mask.is_some();
      let payload_len = hdr.payload_len;
      let payload_start = hdr.header_len;
      let payload_end = payload_start + payload_len;
      if masked && payload_len < 65536 {
        let resp_hdr_len = if payload_len < 126 { 2 } else { 4 };
        let resp_start = payload_start - resp_hdr_len;
        frame_buf[resp_start] = 0x80 | (opcode as u8);
        if payload_len < 126 {
          frame_buf[resp_start + 1] = payload_len as u8;
        } else {
          frame_buf[resp_start + 1] = 126;
          frame_buf[resp_start + 2] = (payload_len >> 8) as u8;
          frame_buf[resp_start + 3] = (payload_len & 0xff) as u8;
        }
        write(&frame_buf[resp_start..payload_end]);
      } else {
        // Fallback: stack header, then the payload.
        let mut head = [0u8; 10];
        let head_n = fmt_server_head(&mut head, opcode, payload_len);
        write(&head[..head_n]);
        write(&frame_buf[payload_start..payload_end]);
      }
    }
  }
}

#[inline]
fn fmt_server_head(
  buf: &mut [u8],
  opcode: OpCode,
  payload_len: usize,
) -> usize {
  buf[0] = 0x80 | (opcode as u8);
  if payload_len < 126 {
    buf[1] = payload_len as u8;
    2
  } else if payload_len < 65536 {
    buf[1] = 126;
    buf[2..4].copy_from_slice(&(payload_len as u16).to_be_bytes());
    4
  } else {
    buf[1] = 127;
    buf[2..10].copy_from_slice(&(payload_len as u64).to_be_bytes());
    10
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn frame_to(bytes: &[u8]) -> Vec<u8> {
    // Build a masked Binary frame for `bytes` with mask [1,2,3,4].
    let mask = [1u8, 2, 3, 4];
    let mut out = vec![0x82u8];
    if bytes.len() < 126 {
      out.push(0x80 | bytes.len() as u8);
    } else if bytes.len() < 65536 {
      out.push(0xfe);
      out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    } else {
      out.push(0xff);
      out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    }
    out.extend_from_slice(&mask);
    for (i, b) in bytes.iter().enumerate() {
      out.push(b ^ mask[i & 3]);
    }
    out
  }

  fn echo_handler(_payload: &mut [u8], _opcode: OpCode) -> ServerResponse {
    ServerResponse::Echo
  }

  #[test]
  fn echo_short_binary() {
    let mut engine = ServerEngine::new();
    let mut frame = frame_to(b"hello");
    let mut out: Vec<u8> = Vec::new();
    let consumed = engine
      .process(&mut frame, |b| out.extend_from_slice(b), echo_handler)
      .unwrap();
    assert_eq!(consumed, frame.len());
    // Response: 0x82, 5, h, e, l, l, o
    assert_eq!(out, vec![0x82, 5, b'h', b'e', b'l', b'l', b'o']);
  }

  #[test]
  fn echo_extended_length() {
    let payload = vec![0xABu8; 16_384];
    let mut frame = frame_to(&payload);
    let mut engine = ServerEngine::new();
    let mut out = Vec::new();
    let consumed = engine
      .process(&mut frame, |b| out.extend_from_slice(b), echo_handler)
      .unwrap();
    assert_eq!(consumed, frame.len());
    // Response header: 0x82, 126, len_hi, len_lo, then 16 384 payload bytes.
    assert_eq!(out.len(), 4 + 16_384);
    assert_eq!(&out[..4], &[0x82, 126, 0x40, 0x00]);
    assert!(out[4..].iter().all(|&b| b == 0xAB));
  }

  #[test]
  fn ping_yields_pong() {
    let mut frame = vec![0x89, 0x84, 1, 2, 3, 4]; // Ping, masked, 4-byte payload "abcd"
    let payload = b"abcd";
    for (i, &b) in payload.iter().enumerate() {
      frame.push(b ^ [1u8, 2, 3, 4][i]);
    }
    let mut engine = ServerEngine::new();
    let mut out = Vec::new();
    let _ = engine
      .process(
        &mut frame,
        |b| out.extend_from_slice(b),
        |_, _| ServerResponse::Discard,
      )
      .unwrap();
    assert!(!engine.is_closed());
    // Response: pong (0x8A) + 4 bytes
    assert_eq!(out[0], 0x8A);
    assert_eq!(out[1], 4);
    assert_eq!(&out[2..6], b"abcd");
  }

  #[test]
  fn close_marks_closed() {
    let mut frame = vec![0x88, 0x80, 1, 2, 3, 4]; // Close, masked, empty
    let mut engine = ServerEngine::new();
    let mut out = Vec::new();
    let _ = engine
      .process(
        &mut frame,
        |b| out.extend_from_slice(b),
        |_, _| ServerResponse::Discard,
      )
      .unwrap();
    assert!(engine.is_closed());
    // Response: close echo with empty payload
    assert_eq!(out, vec![0x88, 0]);
  }

  #[test]
  fn batch_of_two_frames() {
    let mut buf = Vec::new();
    buf.extend_from_slice(&frame_to(b"abc"));
    buf.extend_from_slice(&frame_to(b"de"));
    let mut engine = ServerEngine::new();
    let mut out = Vec::new();
    let consumed = engine
      .process(&mut buf, |b| out.extend_from_slice(b), echo_handler)
      .unwrap();
    assert_eq!(consumed, buf.len());
    // Two responses concatenated.
    assert_eq!(out, vec![0x82, 3, b'a', b'b', b'c', 0x82, 2, b'd', b'e']);
  }

  #[test]
  fn unmasked_input_uses_fallback_writev() {
    // Server input that isn't masked is a protocol violation in
    // practice (clients must mask), but the engine should still
    // handle the case by falling back to a stack header + payload
    // write. We construct a manual unmasked Binary frame.
    let mut frame = vec![0x82u8, 0x05u8];
    frame.extend_from_slice(b"hello");
    let mut engine = ServerEngine::new();
    let mut out = Vec::new();
    let consumed = engine
      .process(&mut frame, |b| out.extend_from_slice(b), echo_handler)
      .unwrap();
    assert_eq!(consumed, frame.len());
    // Response was emitted in two writes (header + payload) which
    // concatenated equal the expected bytes.
    assert_eq!(out, vec![0x82, 5, b'h', b'e', b'l', b'l', b'o']);
  }
}
