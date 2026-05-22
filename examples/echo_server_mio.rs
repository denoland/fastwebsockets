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

//! mio-driven WebSocket echo server using fastwebsockets's core.
//!
//! This example is the experimental answer to the question "is the
//! single-thread gap between fastwebsockets and uWebSockets in our
//! WebSocket framing/parsing/masking, or is it Tokio/futures overhead?"
//! It does the upgrade by hand, drives the event loop with `mio::Poll`
//! directly (no async runtime, no futures state machines), uses
//! `fastwebsockets::unmask` for masking, and inlines the frame
//! parser/writer.
//!
//! The structure is:
//!   - one `mio::Poll`
//!   - one `TcpListener` registered against it
//!   - per-connection `Conn` state in a `Slab` (token-indexed)
//!   - each iteration of the event loop reads as much as the socket
//!     gives us, parses any complete frames from the read buffer in
//!     place, builds the response by writev directly through
//!     `os::unix::io::AsRawFd` so we go through one syscall per frame
//!
//! This is the same dispatch shape as uWebSockets / uSockets: one
//! event-loop thread, callbacks called inline, no per-connection
//! tasks. If the single-core gap with uWS is in Tokio/futures, this
//! example closes it; if not, it shows the remaining gap is in the
//! framing/syscall path and that's the next thing to optimize.
//!
//! Run as `target/release/examples/echo_server_mio` on Linux. Same
//! `FWS_ADDR` env var as the main example; no `FWS_WORKERS` here —
//! pure single-thread.

// Non-Linux gets a stub binary so `cargo build --all-targets` works on
// macOS/Windows CI; the body of this example uses mio's Linux backend
// (epoll) directly. Future work could lift the same shape to kqueue.
#[cfg(not(target_os = "linux"))]
fn main() {
  eprintln!("echo_server_mio: linux-only example (uses epoll via mio)");
}

#[cfg(target_os = "linux")]
mod linux {

  use std::collections::VecDeque;
  use std::io::ErrorKind;
  use std::io::IoSlice;
  use std::io::Read;
  use std::io::Write;
  use std::os::unix::io::AsRawFd;

  use mio::event::Event;
  use mio::net::TcpListener;
  use mio::net::TcpStream;
  use mio::Events;
  use mio::Interest;
  use mio::Poll;
  use mio::Token;

  use fastwebsockets::unmask;

  const LISTENER: Token = Token(0);

  // Buffer just over a 16 KiB-frame's worth of bytes, fitting a full client
  // frame (header + mask + 16 KiB payload = 16392 B) plus a little headroom.
  const BUF_LEN: usize = 64 * 1024;

  const RESPONSE_PREFIX: &[u8] =
  b"HTTP/1.1 101 Switching Protocols\r\nconnection: upgrade\r\nupgrade: websocket\r\nsec-websocket-accept: ";

  #[derive(PartialEq)]
  enum Phase {
    Handshake,
    Echoing,
    Closed,
  }

  // Per-connection state. The big 64 KiB recv buffer that v1..v8 kept here
  // is gone — it now lives once in the event loop and is reused across
  // every connection. The only per-conn read state is a small `partial`
  // Vec that holds the tail of an incomplete frame when one TCP recv
  // didn't deliver a whole frame; for the bench's ping-pong workload it's
  // empty almost all the time and the Vec never allocates.
  //
  // 500 conns × 64 KiB was 32 MiB, past L3 on a 16 MiB Cascadelake. With
  // a shared scratch, the working set during one event is one 64 KiB
  // buffer (stays hot in L2) plus the Conn struct itself (~64 bytes).
  struct Conn {
    stream: TcpStream,
    partial: Vec<u8>,
    wq: VecDeque<u8>,
    phase: Phase,
    interest: Interest,
  }

  impl Conn {
    fn new(stream: TcpStream) -> Self {
      let _ = stream.set_nodelay(true);
      Self {
        stream,
        partial: Vec::new(),
        wq: VecDeque::new(),
        phase: Phase::Handshake,
        interest: Interest::READABLE,
      }
    }
  }

  fn sec_websocket_accept(key: &[u8]) -> [u8; 28] {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use sha1::Digest;
    let mut sha1 = sha1::Sha1::new();
    sha1.update(key);
    sha1.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let digest = sha1.finalize();
    let mut out = [0u8; 28];
    let n = STANDARD.encode_slice(digest.as_slice(), &mut out).unwrap();
    debug_assert_eq!(n, 28);
    out
  }

  fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    if buf.len() < 4 {
      return None;
    }
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
  }

  fn find_header_value<'a>(buf: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let mut start = 0usize;
    while start < buf.len() {
      let line_end = buf[start..]
        .windows(2)
        .position(|w| w == b"\r\n")
        .map(|p| start + p)
        .unwrap_or(buf.len());
      let line = &buf[start..line_end];
      if let Some(colon) = line.iter().position(|&b| b == b':') {
        let lhs = &line[..colon];
        if lhs.eq_ignore_ascii_case(name) {
          let mut v = &line[colon + 1..];
          while !v.is_empty() && (v[0] == b' ' || v[0] == b'\t') {
            v = &v[1..];
          }
          return Some(v);
        }
      }
      start = line_end + 2;
    }
    None
  }

  #[inline]
  fn fmt_server_head(buf: &mut [u8], opcode: u8, payload_len: usize) -> usize {
    buf[0] = 0x80 | opcode;
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

  // Returns true if the connection should be closed.
  fn drain_writes(conn: &mut Conn) -> std::io::Result<bool> {
    while !conn.wq.is_empty() {
      let (front, back) = conn.wq.as_slices();
      let iovs = [IoSlice::new(front), IoSlice::new(back)];
      let n = match conn.stream.write_vectored(&iovs) {
        Ok(0) => return Ok(true),
        Ok(n) => n,
        Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(false),
        Err(_) => return Ok(true),
      };
      conn.wq.drain(..n);
    }
    Ok(false)
  }

  // Try to write directly to the socket; if would-block, push what's left
  // onto the write queue and let the next writable event drain it.
  //
  // Takes `stream` and `wq` separately rather than a `&mut Conn` so the
  // caller can build `iovs` from a borrow into `conn.rbuf` and still
  // hand us a mutable write-queue.
  fn write_now(
    stream: &mut TcpStream,
    wq: &mut VecDeque<u8>,
    iovs: &[IoSlice<'_>],
  ) -> std::io::Result<()> {
    let total: usize = iovs.iter().map(|s| s.len()).sum();
    if !wq.is_empty() {
      // Write queue has pending data; we have to enqueue to preserve order.
      for iov in iovs {
        wq.extend(iov.iter());
      }
      return Ok(());
    }
    let n = match stream.write_vectored(iovs) {
      Ok(0) => return Err(ErrorKind::WriteZero.into()),
      Ok(n) => n,
      Err(e) if e.kind() == ErrorKind::WouldBlock => 0,
      Err(e) => return Err(e),
    };
    if n == total {
      return Ok(());
    }
    // Partial write: enqueue the tail.
    let mut skip = n;
    for iov in iovs {
      if skip >= iov.len() {
        skip -= iov.len();
      } else {
        wq.extend(iov[skip..].iter());
        skip = 0;
      }
    }
    Ok(())
  }

  // Drive the WebSocket framing on a connection that just had a readable
  // event. `scratch` is a shared buffer owned by the event loop and
  // reused across every connection — we drain conn.partial into it,
  // recv the rest, parse frames in place, write echoes, and save any
  // unparsable tail back to conn.partial. This keeps the working set at
  // one buffer in cache regardless of connection count.
  fn handle_readable(conn: &mut Conn, scratch: &mut [u8]) -> bool {
    // Lay any saved tail at the front of the scratch buffer.
    let mut filled = conn.partial.len();
    if filled > 0 {
      scratch[..filled].copy_from_slice(&conn.partial);
      conn.partial.clear();
    }

    // One recv per event (see the v5 commit message for why).
    match conn.stream.read(&mut scratch[filled..]) {
      Ok(0) => return true,
      Ok(n) => filled += n,
      Err(e) if e.kind() == ErrorKind::WouldBlock => {}
      Err(_) => return true,
    }

    let mut read_pos: usize = 0;

    if conn.phase == Phase::Handshake {
      let Some(eom) = find_double_crlf(&scratch[..filled]) else {
        // Incomplete handshake — save what we have and try again later.
        save_tail(conn, scratch, 0, filled);
        return false;
      };
      let header = &scratch[..eom];
      let Some(key) = find_header_value(header, b"Sec-WebSocket-Key") else {
        return true;
      };
      let accept = sec_websocket_accept(key);
      let mut resp = Vec::with_capacity(RESPONSE_PREFIX.len() + 28 + 4);
      resp.extend_from_slice(RESPONSE_PREFIX);
      resp.extend_from_slice(&accept);
      resp.extend_from_slice(b"\r\n\r\n");
      if write_now(&mut conn.stream, &mut conn.wq, &[IoSlice::new(&resp)])
        .is_err()
      {
        return true;
      }
      read_pos = eom;
      conn.phase = Phase::Echoing;
    }

    let mut head = [0u8; 10];
    loop {
      let avail = filled - read_pos;
      if avail < 2 {
        break;
      }
      let off = read_pos;
      let b0 = scratch[off];
      let b1 = scratch[off + 1];
      let fin = (b0 & 0x80) != 0;
      let opcode = b0 & 0x0f;
      let masked = (b1 & 0x80) != 0;
      let len_code = b1 & 0x7f;

      let (header_size, payload_len): (usize, usize) = match len_code {
        0..=125 => (2, len_code as usize),
        126 => {
          if avail < 4 {
            break;
          }
          (
            4,
            u16::from_be_bytes([scratch[off + 2], scratch[off + 3]]) as usize,
          )
        }
        127 => {
          if avail < 10 {
            break;
          }
          (
            10,
            u64::from_be_bytes(scratch[off + 2..off + 10].try_into().unwrap())
              as usize,
          )
        }
        _ => unreachable!(),
      };
      let mask_size = if masked { 4 } else { 0 };
      let total_header = header_size + mask_size;
      if avail < total_header {
        break;
      }
      let frame_total = total_header + payload_len;
      if frame_total > scratch.len() {
        // Pathologically large frame — clean shutdown.
        return true;
      }
      if avail < frame_total {
        break;
      }

      let mask_bytes = if masked {
        let mut m = [0u8; 4];
        m.copy_from_slice(&scratch[off + header_size..off + header_size + 4]);
        Some(m)
      } else {
        None
      };

      if let Some(m) = mask_bytes {
        unmask(&mut scratch[off + total_header..off + frame_total], m);
      }

      if !fin && opcode != 0 {
        return true;
      }

      let resp_opcode = match opcode {
        0x1 | 0x2 => 0x80 | opcode,
        0x9 => 0x8A,
        0x8 => 0x88,
        _ => {
          read_pos += frame_total;
          continue;
        }
      };
      let close_after = opcode == 0x8;
      let inplace_ok = masked && payload_len < 65536;
      if inplace_ok {
        let resp_hdr_len = if payload_len < 126 { 2 } else { 4 };
        let resp_start = off + total_header - resp_hdr_len;
        scratch[resp_start] = resp_opcode;
        if payload_len < 126 {
          scratch[resp_start + 1] = payload_len as u8;
        } else {
          scratch[resp_start + 1] = 126;
          scratch[resp_start + 2] = (payload_len >> 8) as u8;
          scratch[resp_start + 3] = (payload_len & 0xff) as u8;
        }
        let payload_total = resp_hdr_len + payload_len;
        let bytes = &scratch[resp_start..resp_start + payload_total];
        let _ = write_contig_now(&mut conn.stream, &mut conn.wq, bytes);
      } else {
        let n = fmt_server_head(&mut head, resp_opcode & 0x7f, payload_len);
        let payload = &scratch[off + total_header..off + frame_total];
        let iovs = [IoSlice::new(&head[..n]), IoSlice::new(payload)];
        let _ = write_now(&mut conn.stream, &mut conn.wq, &iovs);
      }
      if close_after {
        return true;
      }

      read_pos += frame_total;
    }

    save_tail(conn, scratch, read_pos, filled);
    false
  }

  // Save the still-unparsed tail of the scratch buffer back to the
  // connection. Empty on the common load_test case (one full frame per
  // recv) — the Vec never grows.
  #[inline]
  fn save_tail(conn: &mut Conn, scratch: &[u8], start: usize, end: usize) {
    if start == end {
      return;
    }
    conn.partial.extend_from_slice(&scratch[start..end]);
  }

  // Single contiguous write — same partial-write handling as write_now
  // but without the iovec dance.
  fn write_contig_now(
    stream: &mut TcpStream,
    wq: &mut VecDeque<u8>,
    bytes: &[u8],
  ) -> std::io::Result<()> {
    if !wq.is_empty() {
      wq.extend(bytes.iter());
      return Ok(());
    }
    let n = match stream.write(bytes) {
      Ok(0) => return Err(ErrorKind::WriteZero.into()),
      Ok(n) => n,
      Err(e) if e.kind() == ErrorKind::WouldBlock => 0,
      Err(e) => return Err(e),
    };
    if n < bytes.len() {
      wq.extend(bytes[n..].iter());
    }
    Ok(())
  }

  fn handle_writable(conn: &mut Conn) -> bool {
    drain_writes(conn).unwrap_or(true)
  }

  fn reregister_if_needed(
    conn: &mut Conn,
    poll: &Poll,
    token: Token,
  ) -> std::io::Result<()> {
    let want_write = !conn.wq.is_empty();
    let new = if want_write {
      Interest::READABLE | Interest::WRITABLE
    } else {
      Interest::READABLE
    };
    if new != conn.interest {
      poll.registry().reregister(&mut conn.stream, token, new)?;
      conn.interest = new;
    }
    Ok(())
  }

  fn process_event(
    conns: &mut slab::Slab<Conn>,
    poll: &Poll,
    event: &Event,
    scratch: &mut [u8],
  ) {
    let token = event.token();
    let idx = token.0 - 1;
    if !conns.contains(idx) {
      return;
    }
    let mut close = false;
    {
      let conn = &mut conns[idx];
      if event.is_readable() {
        close |= handle_readable(conn, scratch);
      }
      if event.is_writable() && !close {
        close |= handle_writable(conn);
      }
      if !close && conn.phase == Phase::Closed {
        close = true;
      }
    }
    if close {
      let mut conn = conns.remove(idx);
      let _ = poll.registry().deregister(&mut conn.stream);
      return;
    }
    let _ = reregister_if_needed(&mut conns[idx], poll, token);
  }

  fn run(addr: &str) -> std::io::Result<()> {
    let mut poll = Poll::new()?;
    let mut events = Events::with_capacity(1024);
    let parsed: std::net::SocketAddr = addr.parse().map_err(|e| {
      std::io::Error::new(ErrorKind::InvalidInput, format!("{}", e))
    })?;
    let mut listener = TcpListener::bind(parsed)?;
    poll
      .registry()
      .register(&mut listener, LISTENER, Interest::READABLE)?;
    eprintln!(
      "mio echo listening on {} (fd={})",
      addr,
      listener.as_raw_fd()
    );
    let mut conns: slab::Slab<Conn> = slab::Slab::with_capacity(1024);
    // One shared scratch buffer for *all* connections. Allocated once,
    // reused for every readable event. Stays in cache because it's
    // touched on every cycle.
    let mut scratch: Box<[u8; BUF_LEN]> = Box::new([0u8; BUF_LEN]);
    loop {
      poll.poll(&mut events, None)?;
      for event in events.iter() {
        if event.token() == LISTENER {
          loop {
            match listener.accept() {
              Ok((stream, _)) => {
                let entry = conns.vacant_entry();
                let token = Token(entry.key() + 1);
                let mut conn = Conn::new(stream);
                if let Err(e) = poll.registry().register(
                  &mut conn.stream,
                  token,
                  Interest::READABLE,
                ) {
                  eprintln!("register failed: {}", e);
                  continue;
                }
                entry.insert(conn);
              }
              Err(e) if e.kind() == ErrorKind::WouldBlock => break,
              Err(e) => {
                eprintln!("accept error: {}", e);
                break;
              }
            }
          }
        } else {
          process_event(&mut conns, &poll, event, scratch.as_mut_slice());
        }
      }
    }
  }

  pub fn entry() -> std::io::Result<()> {
    let addr = std::env::var("FWS_ADDR")
      .unwrap_or_else(|_| "127.0.0.1:8080".to_string());
    run(&addr)
  }
} // mod linux

#[cfg(target_os = "linux")]
fn main() -> std::io::Result<()> {
  linux::entry()
}
