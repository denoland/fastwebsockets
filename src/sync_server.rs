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

/// One segment of an outbound write produced by
/// [`ServerEngine::process_into`].
///
/// Two flavors:
/// - `Input`: a byte range *within the input buffer that was passed
///   to the last `process_into` call*. The engine wrote the response
///   header into that buffer (in the freed-up mask slot) and the
///   payload was already there, so the caller can write the slice
///   directly without copying.
/// - `Local`: a byte range within the engine's small internal
///   header-scratch buffer. Only used when the in-place trick doesn't
///   apply (ext-127 payloads, unmasked input frames). Use
///   [`ServerEngine::outbound_local`] to get the underlying bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundSegment {
  /// `start..start+len` within the most recent `process_into` input.
  Input { start: u32, len: u32 },
  /// `start..start+len` within `engine.outbound_local()`.
  Local { start: u32, len: u32 },
}

/// Server-side WebSocket framing engine. Stateless except for a
/// (usually empty) partial-frame buffer used when one TCP read
/// doesn't deliver a complete header — for the typical case it
/// holds nothing and never allocates.
pub struct ServerEngine {
  /// Bytes left over from a previous `process` call that didn't form
  /// a complete frame on their own. Prepended to the next input.
  partial: Vec<u8>,
  /// Small buffer for response-header bytes that don't fit in the
  /// input frame's mask slot (only used by the writev-fallback path
  /// for ext-127 / unmasked inputs).
  outbound_local: Vec<u8>,
  /// Outbound segments produced by the most recent `process_into`
  /// call. The caller iterates these and writes them to the socket
  /// before calling `process_into` again (the `Input` variants refer
  /// to that previous input buffer).
  outbound: Vec<OutboundSegment>,
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
      outbound_local: Vec::new(),
      outbound: Vec::new(),
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

  /// Outbound segments produced by the most recent
  /// [`process_into`](Self::process_into) call. The caller iterates
  /// these — `Input` segments slice the input buffer they passed to
  /// `process_into`; `Local` segments slice
  /// [`outbound_local`](Self::outbound_local) — and writes them to
  /// the socket.
  pub fn outbound_segments(&self) -> &[OutboundSegment] {
    &self.outbound
  }

  /// The engine-owned scratch buffer that `OutboundSegment::Local`
  /// segments index into.
  pub fn outbound_local(&self) -> &[u8] {
    &self.outbound_local
  }

  /// Drop the outbound state after the caller has written it to the
  /// socket. Call this once per `process_into` cycle, after writing.
  pub fn clear_outbound(&mut self) {
    self.outbound_local.clear();
    self.outbound.clear();
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

  /// Zero-copy variant of [`process`](Self::process). Does the same
  /// frame parse / unmask / response synthesis, but instead of
  /// calling a write callback for each output slice, accumulates
  /// outbound segments internally. The caller reads them back via
  /// [`outbound_segments`](Self::outbound_segments) /
  /// [`outbound_local`](Self::outbound_local), writes them to the
  /// socket (e.g. via `writev`), and calls
  /// [`clear_outbound`](Self::clear_outbound).
  ///
  /// The key difference: `Input` segments reference the input buffer
  /// directly. The caller can write straight from that buffer with no
  /// extra memcpy. This is the path the tokio adapter
  /// (`echo_server_tokio_fast.rs`) uses to match the bare-mio
  /// throughput.
  ///
  /// Returns the number of input bytes consumed. Outbound segments
  /// produced by this call are only valid until the next
  /// `process_into` (which conceptually reuses the input buffer).
  pub fn process_into<H>(
    &mut self,
    input: &mut [u8],
    mut handler: H,
  ) -> Result<usize, WebSocketError>
  where
    H: FnMut(&mut [u8], OpCode) -> ServerResponse,
  {
    if self.closed {
      return Ok(0);
    }

    // Same partial-frame prepend as the callback path. Rare in
    // practice; the `extend_from_slice` allocates only if a real
    // straddle happens.
    if !self.partial.is_empty() {
      let need = self.partial.len();
      if input.len() < need {
        return Err(WebSocketError::FrameTooLarge);
      }
      input.copy_within(0..(input.len() - need), need);
      input[..need].copy_from_slice(&self.partial);
      self.partial.clear();
    }

    let mut consumed = 0usize;
    let end = input.len();
    loop {
      let remaining_start = consumed;
      let remaining = &mut input[remaining_start..end];
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

      if let Some(m) = hdr.mask {
        unmask(&mut remaining[payload_start..payload_end], m);
      }

      let (resp_opcode, close_after, skip) = match hdr.opcode {
        OpCode::Close => (OpCode::Close, true, false),
        OpCode::Ping => (OpCode::Pong, false, false),
        OpCode::Pong => (OpCode::Pong, false, true),
        OpCode::Text | OpCode::Binary => {
          if !hdr.fin {
            return Err(WebSocketError::InvalidFragment);
          }
          let response =
            handler(&mut remaining[payload_start..payload_end], hdr.opcode);
          match response {
            ServerResponse::Echo => (hdr.opcode, false, false),
            ServerResponse::Discard => (hdr.opcode, false, true),
          }
        }
        OpCode::Continuation => {
          return Err(WebSocketError::InvalidContinuationFrame);
        }
      };

      if !skip {
        emit_response_into(
          &mut input[remaining_start..],
          remaining_start,
          &hdr,
          resp_opcode,
          &mut self.outbound_local,
          &mut self.outbound,
        );
      }

      consumed += frame_total;
      if close_after {
        self.closed = true;
        return Ok(consumed);
      }
    }

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

/// Zero-copy variant of `emit_response`: rather than calling a write
/// callback, push descriptors into the engine's outbound-segment
/// list. `frame_buf` is `&mut input[frame_origin..]` so we can record
/// offsets relative to the original `input`.
#[inline]
fn emit_response_into(
  frame_buf: &mut [u8],
  frame_origin: usize,
  hdr: &crate::frame::Header,
  opcode: OpCode,
  local: &mut Vec<u8>,
  segments: &mut Vec<OutboundSegment>,
) {
  let masked = hdr.mask.is_some();
  let payload_len = hdr.payload_len;
  let payload_start = hdr.header_len;
  let payload_end = payload_start + payload_len;
  if masked && payload_len < 65536 {
    // In-place: rewrite the response header into the mask slot, then
    // record a single Input range spanning the response header +
    // payload contiguously.
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
    let total = resp_hdr_len + payload_len;
    segments.push(OutboundSegment::Input {
      start: (frame_origin + resp_start) as u32,
      len: total as u32,
    });
  } else {
    // Fallback: emit the header into the engine's local scratch and
    // record two segments (header + payload).
    let head_start = local.len();
    let mut head = [0u8; 10];
    let n = fmt_server_head(&mut head, opcode, payload_len);
    local.extend_from_slice(&head[..n]);
    segments.push(OutboundSegment::Local {
      start: head_start as u32,
      len: n as u32,
    });
    segments.push(OutboundSegment::Input {
      start: (frame_origin + payload_start) as u32,
      len: payload_len as u32,
    });
  }
  // Suppress unused-variable warning from `payload_end` in the
  // fallback branch (we already used it via slice math above).
  let _ = payload_end;
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

  /// Helper: drain the engine's outbound segments into a flat Vec the
  /// way an adapter would (concatenating Input/Local segments).
  fn drain_outbound(engine: &mut ServerEngine, input: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let local = engine.outbound_local().to_vec();
    for seg in engine.outbound_segments() {
      match seg {
        OutboundSegment::Input { start, len } => {
          out.extend_from_slice(
            &input[*start as usize..*start as usize + *len as usize],
          );
        }
        OutboundSegment::Local { start, len } => {
          out.extend_from_slice(
            &local[*start as usize..*start as usize + *len as usize],
          );
        }
      }
    }
    engine.clear_outbound();
    out
  }

  #[test]
  fn process_into_zero_copy_short() {
    let mut engine = ServerEngine::new();
    let mut frame = frame_to(b"hello");
    let frame_copy = frame.clone(); // for the index lookup after process
    let _ = engine.process_into(&mut frame, echo_handler).unwrap();
    // The engine should produce one Input segment that, when sliced
    // from the post-process frame, equals the expected response. We
    // use `frame` itself (post-mutation) because process_into writes
    // the response header into the mask slot.
    let _ = frame_copy; // silence unused
    let out = drain_outbound(&mut engine, &frame);
    assert_eq!(out, vec![0x82, 5, b'h', b'e', b'l', b'l', b'o']);
    // Outbound should be a single Input segment — zero-copy.
    assert!(engine.outbound_local().is_empty());
  }

  #[test]
  fn process_into_zero_copy_extended() {
    let mut engine = ServerEngine::new();
    let payload = vec![0xCDu8; 16_384];
    let mut frame = frame_to(&payload);
    let _ = engine.process_into(&mut frame, echo_handler).unwrap();
    let out = drain_outbound(&mut engine, &frame);
    assert_eq!(out.len(), 4 + 16_384);
    assert_eq!(&out[..4], &[0x82, 126, 0x40, 0x00]);
    assert!(out[4..].iter().all(|&b| b == 0xCD));
  }

  #[test]
  fn process_into_fallback_writev_uses_local() {
    // Unmasked input (protocol-violating from a client, but exercises
    // the writev fallback path that uses engine.outbound_local).
    let mut frame = vec![0x82u8, 0x05u8];
    frame.extend_from_slice(b"hello");
    let mut engine = ServerEngine::new();
    let _ = engine.process_into(&mut frame, echo_handler).unwrap();
    // Two segments: Local (header) then Input (payload).
    let segs = engine.outbound_segments();
    assert_eq!(segs.len(), 2);
    assert!(matches!(segs[0], OutboundSegment::Local { .. }));
    assert!(matches!(segs[1], OutboundSegment::Input { .. }));
    let out = drain_outbound(&mut engine, &frame);
    assert_eq!(out, vec![0x82, 5, b'h', b'e', b'l', b'l', b'o']);
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
