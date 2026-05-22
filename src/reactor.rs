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

//! Single-thread, mio-driven server-side reactor that drives many
//! WebSocket sessions through [`ServerEngine`] with one event loop
//! and one shared receive buffer.
//!
//! This is the fast path that closes the throughput gap to uWebSockets
//! on the *many-connection, many-bytes-per-frame* workload. The
//! per-connection tokio task model (see
//! `examples/echo_server_tokio_fast.rs`) wakes one task per
//! readability per frame; at 500 concurrent connections each running
//! the bench's send-then-await-echo pattern, the per-task
//! scheduling overhead becomes the bottleneck even when every other
//! per-frame cost has been removed. The reactor in this module is
//! the structural answer: one task drives `N` fds, draining many
//! frames per `epoll_wait`.
//!
//! # Single thread, single CPU
//!
//! All work happens on the thread that calls [`Reactor::run`]. The
//! reactor never spawns a worker. This is intentional: the perf
//! comparison against uWebSockets is *single-core*, and uWS is
//! single-thread. Pull in [`tokio::task::spawn_blocking`] or a bare
//! `std::thread::spawn` from your application code if you want to
//! shard across cores.
//!
//! # HTTP upgrade
//!
//! The reactor takes already-upgraded sockets via
//! [`Reactor::add_session`]. The standalone
//! [`Reactor::run_echo_server`] helper does the WebSocket handshake
//! itself (HTTP/1.1 GET + Sec-WebSocket-Key + accept-key) so users
//! who want the canonical bench-shape echo server don't have to
//! write any HTTP code. For embedding behind hyper / axum / a
//! custom HTTP server, use [`Reactor::add_session`] after you have
//! validated the request and written the 101 response.
//!
//! # Example
//!
//! ```no_run
//! # #[cfg(all(target_os = "linux", feature = "reactor"))]
//! # fn _doc() -> std::io::Result<()> {
//! use fastwebsockets::reactor::Reactor;
//! let mut reactor = Reactor::new()?;
//! reactor.bind("127.0.0.1:8080")?;
//! reactor.run_echo()?;
//! # Ok(())
//! # }
//! ```
//!
//! Or with a custom handler:
//!
//! ```no_run
//! # #[cfg(all(target_os = "linux", feature = "reactor"))]
//! # fn _doc() -> std::io::Result<()> {
//! use fastwebsockets::reactor::Reactor;
//! use fastwebsockets::{OpCode, ServerResponse};
//! let mut reactor = Reactor::new()?;
//! reactor.bind("127.0.0.1:8080")?;
//! reactor.run(|payload, opcode| {
//!   match opcode {
//!     OpCode::Text | OpCode::Binary => {
//!       // mutate `payload` in place — the engine will send it back
//!       // with the same opcode and FIN as a response.
//!       for b in payload.iter_mut() { *b = b.to_ascii_uppercase(); }
//!       ServerResponse::Echo
//!     }
//!     _ => ServerResponse::Discard,
//!   }
//! })?;
//! # Ok(())
//! # }
//! ```

use std::collections::VecDeque;
use std::io::ErrorKind;
use std::io::IoSlice;
use std::io::Read;
use std::io::Write;
use std::net::SocketAddr;

use mio::event::Event;
use mio::net::TcpListener;
use mio::net::TcpStream;
use mio::Events;
use mio::Interest;
use mio::Poll;
use mio::Token;

use crate::frame::OpCode;
use crate::sync_server::ServerEngine;
use crate::sync_server::ServerResponse;

const LISTENER_TOKEN: Token = Token(0);

/// Default receive scratch buffer size. Sized to admit a maximum
/// 16 KiB-payload masked frame (16 KiB + 4-byte ext header + 4-byte
/// mask) in one recv with headroom for kernel coalescing of small
/// frames.
const DEFAULT_SCRATCH: usize = 64 * 1024;

const HANDSHAKE_RESPONSE_PREFIX: &[u8] =
  b"HTTP/1.1 101 Switching Protocols\r\nconnection: upgrade\r\nupgrade: websocket\r\nsec-websocket-accept: ";

#[derive(PartialEq)]
enum Phase {
  Handshake,
  Echoing,
  Closed,
}

struct Session {
  stream: TcpStream,
  engine: ServerEngine,
  // Bytes from a partial HTTP upgrade request held across recvs.
  // Only non-empty during handshake; the steady-state framing path
  // is owned by `engine.partial_len()`.
  partial_handshake: Vec<u8>,
  // Pending bytes that the kernel send buffer couldn't absorb. Drained
  // on writable events.
  wq: VecDeque<u8>,
  phase: Phase,
  interest: Interest,
}

impl Session {
  fn new(stream: TcpStream) -> Self {
    let _ = stream.set_nodelay(true);
    Self {
      stream,
      engine: ServerEngine::new(),
      partial_handshake: Vec::new(),
      wq: VecDeque::new(),
      phase: Phase::Handshake,
      interest: Interest::READABLE,
    }
  }

  /// Construct a session for a socket that has already been upgraded
  /// at the HTTP layer by the caller. The reactor will not attempt to
  /// parse a handshake on it.
  fn from_upgraded(stream: TcpStream) -> Self {
    let _ = stream.set_nodelay(true);
    Self {
      stream,
      engine: ServerEngine::new(),
      partial_handshake: Vec::new(),
      wq: VecDeque::new(),
      phase: Phase::Echoing,
      interest: Interest::READABLE,
    }
  }
}

/// Handle to a session inside the reactor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(usize);

/// Single-thread server-side WebSocket reactor.
///
/// See the module-level docs for an overview. Construct with
/// [`new`](Self::new), optionally bind a listener for built-in accept
/// with [`bind`](Self::bind), pass already-upgraded sockets with
/// [`add_session`](Self::add_session), and drive the event loop with
/// [`run`](Self::run) / [`run_echo`](Self::run_echo).
pub struct Reactor {
  poll: Poll,
  events: Events,
  sessions: slab::Slab<Session>,
  scratch: Box<[u8]>,
  listener: Option<TcpListener>,
}

impl Reactor {
  /// Create a new reactor with the default scratch capacity.
  pub fn new() -> std::io::Result<Self> {
    Self::with_capacity(DEFAULT_SCRATCH, 1024)
  }

  /// Create a new reactor with `scratch_bytes` of recv scratch and an
  /// initial events capacity of `events_capacity`. Both grow on
  /// demand if exceeded.
  pub fn with_capacity(
    scratch_bytes: usize,
    events_capacity: usize,
  ) -> std::io::Result<Self> {
    Ok(Self {
      poll: Poll::new()?,
      events: Events::with_capacity(events_capacity),
      sessions: slab::Slab::with_capacity(64),
      scratch: vec![0u8; scratch_bytes].into_boxed_slice(),
      listener: None,
    })
  }

  /// Bind a TCP listener on `addr` and register it with the reactor.
  /// Incoming connections will be accepted by [`run`](Self::run) and
  /// their HTTP upgrade negotiated inline before framing starts.
  pub fn bind(&mut self, addr: &str) -> std::io::Result<()> {
    let parsed: SocketAddr = addr.parse().map_err(|e| {
      std::io::Error::new(ErrorKind::InvalidInput, format!("{}", e))
    })?;
    let mut listener = TcpListener::bind(parsed)?;
    self
      .poll
      .registry()
      .register(&mut listener, LISTENER_TOKEN, Interest::READABLE)?;
    self.listener = Some(listener);
    Ok(())
  }

  /// Add an already-upgraded WebSocket stream to the reactor. The
  /// stream must be a mio non-blocking [`TcpStream`]; the reactor
  /// takes ownership and drives frames until close.
  ///
  /// Use this when the WebSocket handshake was negotiated outside the
  /// reactor (e.g. behind hyper / axum / a custom HTTP layer).
  pub fn add_session(
    &mut self,
    mut stream: TcpStream,
  ) -> std::io::Result<SessionId> {
    let entry = self.sessions.vacant_entry();
    let token = Token(entry.key() + 1);
    self
      .poll
      .registry()
      .register(&mut stream, token, Interest::READABLE)?;
    entry.insert(Session::from_upgraded(stream));
    Ok(SessionId(token.0))
  }

  /// Drive the event loop with an echo handler. Equivalent to
  /// calling [`run`](Self::run) with a closure that returns
  /// [`ServerResponse::Echo`] for data frames and
  /// [`ServerResponse::Discard`] for everything else.
  pub fn run_echo(&mut self) -> std::io::Result<()> {
    self.run(|_payload, opcode| match opcode {
      OpCode::Text | OpCode::Binary => ServerResponse::Echo,
      _ => ServerResponse::Discard,
    })
  }

  /// Drive the event loop until either the listener (if any) and all
  /// sessions have closed.
  ///
  /// `handler(payload, opcode)` is called inline for each data frame
  /// the engine parses. The handler runs synchronously on the
  /// reactor thread — do not block in it.
  pub fn run<H>(&mut self, mut handler: H) -> std::io::Result<()>
  where
    H: FnMut(&mut [u8], OpCode) -> ServerResponse,
  {
    loop {
      if self.listener.is_none() && self.sessions.is_empty() {
        return Ok(());
      }
      self.poll.poll(&mut self.events, None)?;
      // Take the events out so we don't hold an immutable borrow of
      // `self` across the per-event processing.
      let mut events = std::mem::replace(
        &mut self.events,
        Events::with_capacity(self.sessions.capacity().max(64)),
      );
      for event in events.iter() {
        let token = event.token();
        if token == LISTENER_TOKEN {
          self.accept_until_block()?;
        } else {
          self.process_event(event, &mut handler);
        }
      }
      events.clear();
      // Recycle the events buffer to avoid reallocation.
      let _ = std::mem::replace(&mut self.events, events);
    }
  }

  /// Drive one polling iteration. Useful for embedding the reactor
  /// inside a larger event loop (e.g. when you need to interleave it
  /// with other signal sources).
  ///
  /// `timeout = None` blocks until at least one event is ready.
  /// `timeout = Some(Duration::ZERO)` is a non-blocking poll.
  pub fn run_once<H>(
    &mut self,
    timeout: Option<std::time::Duration>,
    mut handler: H,
  ) -> std::io::Result<()>
  where
    H: FnMut(&mut [u8], OpCode) -> ServerResponse,
  {
    self.poll.poll(&mut self.events, timeout)?;
    let mut events = std::mem::replace(
      &mut self.events,
      Events::with_capacity(self.sessions.capacity().max(64)),
    );
    for event in events.iter() {
      let token = event.token();
      if token == LISTENER_TOKEN {
        self.accept_until_block()?;
      } else {
        self.process_event(event, &mut handler);
      }
    }
    events.clear();
    let _ = std::mem::replace(&mut self.events, events);
    Ok(())
  }

  fn accept_until_block(&mut self) -> std::io::Result<()> {
    let Some(listener) = self.listener.as_mut() else {
      return Ok(());
    };
    loop {
      match listener.accept() {
        Ok((stream, _)) => {
          let entry = self.sessions.vacant_entry();
          let token = Token(entry.key() + 1);
          let mut session = Session::new(stream);
          self
            .poll
            .registry()
            .register(&mut session.stream, token, Interest::READABLE)?;
          entry.insert(session);
        }
        Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(()),
        Err(_) => return Ok(()),
      }
    }
  }

  fn process_event<H>(&mut self, event: &Event, handler: &mut H)
  where
    H: FnMut(&mut [u8], OpCode) -> ServerResponse,
  {
    let idx = event.token().0.wrapping_sub(1);
    if !self.sessions.contains(idx) {
      return;
    }
    let mut close = false;
    if event.is_readable() {
      close |=
        handle_readable(&mut self.sessions[idx], &mut self.scratch, handler);
    }
    if event.is_writable() && !close {
      close |= drain_writes(&mut self.sessions[idx]).unwrap_or(true);
    }
    if !close && self.sessions[idx].phase == Phase::Closed {
      close = true;
    }
    if close {
      let mut session = self.sessions.remove(idx);
      let _ = self.poll.registry().deregister(&mut session.stream);
      return;
    }
    let _ = reregister_if_needed(
      &mut self.sessions[idx],
      &self.poll,
      Token(idx + 1),
    );
  }
}

// Returns true if the session should be closed.
fn handle_readable<H>(
  session: &mut Session,
  scratch: &mut [u8],
  handler: &mut H,
) -> bool
where
  H: FnMut(&mut [u8], OpCode) -> ServerResponse,
{
  let n = match session.stream.read(scratch) {
    Ok(0) => return true,
    Ok(n) => n,
    Err(e) if e.kind() == ErrorKind::WouldBlock => 0,
    Err(_) => return true,
  };
  if n == 0 {
    return false;
  }

  let mut read_pos: usize = 0;
  if session.phase == Phase::Handshake {
    let Some(eom) = find_double_crlf(&scratch[..n]) else {
      session.partial_handshake.extend_from_slice(&scratch[..n]);
      return false;
    };
    let header = &scratch[..eom];
    let Some(key) = find_header_value(header, b"Sec-WebSocket-Key") else {
      return true;
    };
    let accept = sec_websocket_accept(key);
    let mut resp = Vec::with_capacity(HANDSHAKE_RESPONSE_PREFIX.len() + 32);
    resp.extend_from_slice(HANDSHAKE_RESPONSE_PREFIX);
    resp.extend_from_slice(&accept);
    resp.extend_from_slice(b"\r\n\r\n");
    if write_now(
      &mut session.stream,
      &mut session.wq,
      &[IoSlice::new(&resp)],
    )
    .is_err()
    {
      return true;
    }
    read_pos = eom;
    session.phase = Phase::Echoing;
  }

  if read_pos >= n {
    return false;
  }
  let stream = &mut session.stream;
  let wq = &mut session.wq;
  let process_result = session.engine.process(
    &mut scratch[read_pos..n],
    |bytes| {
      let _ = write_contig_now(stream, wq, bytes);
    },
    handler,
  );
  if process_result.is_err() {
    return true;
  }
  session.engine.is_closed()
}

fn drain_writes(session: &mut Session) -> std::io::Result<bool> {
  while !session.wq.is_empty() {
    let (front, back) = session.wq.as_slices();
    let iovs = [IoSlice::new(front), IoSlice::new(back)];
    let n = match session.stream.write_vectored(&iovs) {
      Ok(0) => return Ok(true),
      Ok(n) => n,
      Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(false),
      Err(_) => return Ok(true),
    };
    session.wq.drain(..n);
  }
  Ok(false)
}

fn write_now(
  stream: &mut TcpStream,
  wq: &mut VecDeque<u8>,
  iovs: &[IoSlice<'_>],
) -> std::io::Result<()> {
  let total: usize = iovs.iter().map(|s| s.len()).sum();
  if !wq.is_empty() {
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

fn reregister_if_needed(
  session: &mut Session,
  poll: &Poll,
  token: Token,
) -> std::io::Result<()> {
  let want_write = !session.wq.is_empty();
  let new = if want_write {
    Interest::READABLE | Interest::WRITABLE
  } else {
    Interest::READABLE
  };
  if new != session.interest {
    poll
      .registry()
      .reregister(&mut session.stream, token, new)?;
    session.interest = new;
  }
  Ok(())
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn rfc6455_accept_key() {
    // Canonical example from RFC 6455 §1.3.
    let got = sec_websocket_accept(b"dGhlIHNhbXBsZSBub25jZQ==");
    assert_eq!(&got, b"s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
  }

  #[test]
  fn double_crlf_locator() {
    assert_eq!(find_double_crlf(b"GET / HTTP/1.1\r\n\r\n"), Some(18));
    assert_eq!(find_double_crlf(b"GET / HTTP/1.1\r\nHost: x\r\n\r\nrest"), Some(27));
    assert_eq!(find_double_crlf(b"GET / HTTP/1.1\r\n"), None);
    assert_eq!(find_double_crlf(b""), None);
  }

  #[test]
  fn header_value_lookup_case_insensitive() {
    let req =
      b"GET / HTTP/1.1\r\nHost: x\r\nSec-WebSocket-Key: AbCdEf==\r\nUpgrade: websocket\r\n\r\n";
    let v = find_header_value(req, b"sec-websocket-key").unwrap();
    assert_eq!(v, b"AbCdEf==");
    let v = find_header_value(req, b"Sec-WebSocket-Key").unwrap();
    assert_eq!(v, b"AbCdEf==");
    let v = find_header_value(req, b"upgrade").unwrap();
    assert_eq!(v, b"websocket");
    assert!(find_header_value(req, b"nope").is_none());
  }

  #[test]
  fn reactor_new_idle_returns() {
    // A reactor with no listener and no sessions returns immediately
    // from `run` (nothing to wait on). Doesn't bind anything, so it
    // works in sandboxed environments that block listen().
    let mut r = Reactor::new().unwrap();
    r.run_echo().unwrap();
  }

  /// End-to-end: feed a masked binary frame in over a UNIX socket
  /// pair, drive the reactor for one tick, observe the echoed frame
  /// on the other end. Exercises register / readable handler / engine
  /// / write path without needing `listen()`.
  #[test]
  fn reactor_echoes_a_masked_frame_via_socketpair() {
    use std::io::Read as _;
    use std::io::Write as _;
    use std::os::fd::AsRawFd;
    use std::os::fd::FromRawFd;

    // Build a masked binary frame containing b"hello".
    let mask = [1u8, 2, 3, 4];
    let mut frame = vec![0x82u8, 0x80 | 5u8];
    frame.extend_from_slice(&mask);
    for (i, b) in b"hello".iter().enumerate() {
      frame.push(b ^ mask[i & 3]);
    }

    // socketpair gives us two bidirectional fds wired together. We
    // hand the server end to the reactor and write a frame on the
    // client end. After the reactor processes the event we read the
    // echo back.
    let mut fds: [libc::c_int; 2] = [-1, -1];
    let rc = unsafe {
      libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr())
    };
    assert_eq!(rc, 0, "socketpair failed: {}", std::io::Error::last_os_error());

    // Move into std types so we can flip non-blocking + drop them
    // cleanly. Then convert the server side into a mio TcpStream by
    // way of its raw fd — mio's TcpStream is just a thin
    // non-blocking wrapper over the same fd kind.
    let server_fd = fds[0];
    let mut client = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fds[1]) };

    // Set both ends non-blocking.
    unsafe {
      let flags = libc::fcntl(server_fd, libc::F_GETFL);
      libc::fcntl(server_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
      let flags = libc::fcntl(client.as_raw_fd(), libc::F_GETFL);
      libc::fcntl(
        client.as_raw_fd(),
        libc::F_SETFL,
        flags | libc::O_NONBLOCK,
      );
    }

    let stream = unsafe { TcpStream::from_raw_fd(server_fd) };
    let mut reactor = Reactor::new().unwrap();
    let _ = reactor.add_session(stream).unwrap();

    // Write the frame on the client side first, then run the reactor.
    client.write_all(&frame).unwrap();

    // Drive a couple of ticks: one for readable on the server, one
    // for the loopback delivery of the echoed write back to the
    // client (the kernel may queue it instantly, but be generous).
    for _ in 0..4 {
      reactor
        .run_once(Some(std::time::Duration::from_millis(50)), |_, op| {
          match op {
            OpCode::Text | OpCode::Binary => ServerResponse::Echo,
            _ => ServerResponse::Discard,
          }
        })
        .unwrap();
    }

    let mut buf = [0u8; 32];
    let n = client.read(&mut buf).unwrap();
    assert_eq!(&buf[..n], &[0x82, 5, b'h', b'e', b'l', b'l', b'o']);
  }
}
