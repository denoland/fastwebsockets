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
//! # When to use this vs the tokio adapter
//!
//! `fastwebsockets` exposes two server-side fast paths and they have
//! different shapes:
//!
//! - **`crate::sync_server::ServerEngine` + a tokio task per
//!   connection** (the pattern in
//!   `examples/echo_server_tokio_fast.rs`). The engine handles
//!   parse / unmask / response framing synchronously, the task
//!   handles I/O via tokio's `read().await` + `try_write`. Picks up
//!   tokio integration (timers, channels, hyper upgrades, multi-
//!   threaded runtime) for free; the cost is one task plus one
//!   `read()`-future per connection. This is the universal
//!   fallback and what the existing `WebSocket<WebSocketStream>`
//!   public API plugs into.
//! - **`reactor::Reactor`** (this module, Linux only). One thread,
//!   one mio event loop, one shared 64 KiB recv buffer, many
//!   sessions. No per-connection task, no per-frame `Future`, no
//!   per-task scheduling. Framing runs in the same `ServerEngine`
//!   as the tokio path, just invoked from inside the mio dispatch
//!   loop instead of inside a tokio task.
//!
//! Pick the tokio adapter when you want the WS connection to look
//! and behave like any other tokio future in a larger async app.
//! Pick the reactor when many WebSocket sessions need to be
//! multiplexed cheaply on one core — proxies, broadcast/PubSub
//! brokers, push notifications, telemetry fan-in, the high-fd
//! arms of WebSocket gateways. The reactor is also the right tool
//! when a manager (HTTP server / runtime extension / etc.) wants
//! to own many fds on its own thread and route frames in and out
//! via queues; the [`Sender`] gives that manager a cross-thread
//! command/wake path.
//!
//! # Single thread, single CPU
//!
//! All work happens on the thread that calls [`Reactor::run`]. The
//! reactor never spawns a worker — this is what keeps the single-
//! core perf comparison vs uWebSockets honest. Compose it with the
//! rest of your app via your own thread strategy: one reactor per
//! CPU core via `std::thread::spawn`, or one reactor on a
//! dedicated thread alongside a tokio runtime, with the runtime
//! pushing outbound work through the reactor's [`Sender`].
//!
//! # HTTP upgrade
//!
//! Two integration shapes:
//!
//! - **Built-in.** [`Reactor::bind`] registers a TCP listener with
//!   the reactor; [`Reactor::run`] / [`Reactor::run_echo`] then
//!   accepts connections, parses the HTTP/1.1 upgrade (GET +
//!   `Sec-WebSocket-Key` + 101 response with the RFC 6455 accept
//!   key), and starts framing. Use this for self-contained binaries.
//! - **Embedded.** Most real integrations look like this: an
//!   existing HTTP server (hyper, axum, Deno's `ext/http`, custom)
//!   negotiates the upgrade, hands the raw upgraded TCP socket to
//!   [`Reactor::add_session`] as a `mio::net::TcpStream`, and the
//!   reactor takes it from there. The reactor never touches HTTP
//!   for that session — it goes straight to framing.
//!
//! # API at a glance
//!
//! - [`Reactor::new`] / [`Reactor::bind`] / [`Reactor::add_session`]
//!   — set up the reactor and its sessions.
//! - [`Reactor::sender`] — cross-thread handle for posting
//!   outbound work. Clone freely; safe to call from any thread.
//! - [`Handler`] trait + [`Connection`] handle — what user code
//!   implements. `on_open` / `on_frame` / `on_close` callbacks run
//!   inline on the reactor thread; the per-call [`Connection`]
//!   handle exposes `echo()`, `send(opcode, bytes)`, `close()`,
//!   and `id()`. The handler may not borrow other sessions
//!   directly — use [`Sender`] for cross-session writes.
//! - [`Reactor::run`] — drive the event loop with your handler.
//! - [`Reactor::run_once`] — single tick, for embedding the
//!   reactor inside a larger event loop.
//! - [`Reactor::run_echo`] — convenience for the bench-shape pure-
//!   echo server. Real applications use [`Reactor::run`].
//!
//! # Examples
//!
//! Minimal echo server (benchmark shape):
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
//! Custom per-frame handler with in-place payload mutation:
//!
//! ```no_run
//! # #[cfg(all(target_os = "linux", feature = "reactor"))]
//! # fn _doc() -> std::io::Result<()> {
//! use fastwebsockets::reactor::{Reactor, handler_fn};
//! use fastwebsockets::OpCode;
//! let mut reactor = Reactor::new()?;
//! reactor.bind("127.0.0.1:8080")?;
//! reactor.run(&mut handler_fn(|conn, payload, opcode| match opcode {
//!   OpCode::Text | OpCode::Binary => {
//!     for b in payload.iter_mut() { *b = b.to_ascii_uppercase(); }
//!     conn.echo();
//!   }
//!   _ => {}
//! }))?;
//! # Ok(())
//! # }
//! ```
//!
//! Full general-purpose server (broadcast broker) — see
//! `examples/reactor_chat_broker.rs` for a runnable version that
//! exercises [`Sender`] for cross-session fan-out.
//!
//! # Embedding from an HTTP server or runtime extension (e.g. Deno)
//!
//! The reactor is a *manager* primitive. The expected shape when
//! plugging it into a larger stack (Deno's `ext/websocket`, an axum
//! app, a custom HTTP gateway) is **not** "spawn the reactor as
//! your whole server" — it is "keep the existing async HTTP /
//! websocket path as the universal one, and hand only the eligible
//! hot sessions to a dedicated reactor thread."
//!
//! For Deno specifically, today's path is
//! `op_http_upgrade_websocket` → `extract_network_stream()` →
//! `WebSocket::after_handshake(WebSocketStream::new(...))` → split
//! into `FragmentCollectorRead` + `WebSocketWrite` behind
//! `AsyncRefCell`, with JS pulling events via `op_ws_next_event` and
//! pushing sends via separate ops. The reactor does not replace
//! that path one-for-one — Deno's JS API is per-socket events over
//! resource ids, while the reactor's whole point is "one event loop
//! owns many fds." The integration is a side-by-side fast path, not
//! a swap-in:
//!
//! 1. **Keep the existing Tokio `WebSocket<WebSocketStream>` path
//!    as the default and universal path.** It handles TCP, TLS,
//!    Unix, vsock, tunnel, HTTP/2, buffered upgrade bytes, and the
//!    existing resource/op model. Do not break any of those by
//!    routing them through the reactor.
//! 2. **Add a Linux-only fast path for the common HTTP/1.1
//!    upgraded plain TCP case**, behind a feature flag or runtime
//!    experiment first. Only `NetworkStream::Tcp(stream)` is
//!    eligible; TLS / H2 / Unix / vsock / tunnel and non-Linux
//!    builds fall back to the existing path immediately.
//! 3. **Move the upgraded socket into a reactor-backed manager.**
//!    In `op_http_upgrade_websocket_next`, after
//!    `extract_network_stream()` returns `(NetworkStream::Tcp(s),
//!    Bytes)`, convert `s` to a `mio::net::TcpStream` and pass it
//!    plus the buffered upgrade bytes to
//!    [`Reactor::add_session_with_prefix`]. The prefix bytes
//!    (whatever Hyper already drained from the kernel) are
//!    processed through [`ServerEngine`] before the next socket
//!    read, so no frame is lost on the seam.
//! 4. **Run the reactor on a dedicated thread.** The
//!    [`Reactor::run`] call does not return until all sessions and
//!    senders are gone, so park it on its own
//!    `std::thread::spawn`. Multiple manager threads (one reactor
//!    each) is the right scaling strategy if one core saturates;
//!    do not try to share a [`Reactor`] across threads.
//! 5. **JS-facing ops route through channels, not direct calls.**
//!    Keep `op_ws_next_event` / `op_ws_send_*` / `op_ws_close`
//!    looking the same to JS. Under the hood:
//!    - Each Deno resource holds an inbound `tokio::sync::mpsc`
//!      receiver + a [`SessionId`] + a clone of the reactor's
//!      [`Sender`].
//!    - `next_event` awaits the inbound receiver.
//!    - `send_*` calls [`Sender::send`] (which is sync and wakes
//!      the reactor via `mio::Waker`).
//!    - `close` calls [`Sender::close`].
//!    The reactor-side [`Handler`] forwards each
//!    [`Handler::on_frame`] / [`Handler::on_open`] /
//!    [`Handler::on_close`] into the right resource's inbound
//!    channel and never touches JS state directly.
//! 6. **Fall back, never crash.** Anything the reactor cannot
//!    handle (TLS, H2, Unix sockets, vsock, tunnel, non-Linux
//!    builds, an upgrade buffer larger than your seam can carry,
//!    a Deno permission that the reactor path can't observe yet)
//!    should fall back to the existing `WebSocket<WebSocketStream>`
//!    path. The reactor is an optimization, not a contract change.
//!
//! ## Perf caveat for runtime integrations
//!
//! If every received frame still crosses into JS one-by-one, a
//! runtime-integrated benchmark will *not* reproduce the pure-Rust
//! echo numbers in this PR's benchmark section. That is fine and
//! expected: the value of the reactor in that setting is removing
//! Tokio per-connection scheduling and per-frame `Future` overhead
//! from the Rust side, not eliminating the cost of crossing the JS
//! boundary. Bench the two layers separately — one Rust-only
//! benchmark against the resource/queue manager shape, one full
//! Deno benchmark against `Deno.serve()` — so the JS/op overhead
//! is attributed to JS/ops and the Rust-side win is attributed to
//! the reactor.
//!
//! ## Required surface, and where it lives
//!
//! Every piece a Deno-style embedder needs is already on the
//! [`Reactor`] / [`Handler`] / [`Sender`] surface:
//!
//! | Need | API |
//! |---|---|
//! | Adopt an already-upgraded TCP socket | [`Reactor::add_session`] |
//! | Preserve buffered upgrade bytes across the seam | [`Reactor::add_session_with_prefix`] |
//! | Stable per-socket id for JS resources | [`SessionId`] (returned from both `add_session*`) |
//! | Inbound event delivery | [`Handler::on_open`] / [`Handler::on_frame`] / [`Handler::on_close`] |
//! | Outbound command path from another thread | [`Sender::send`] |
//! | Close from another thread (also fires `on_close`) | [`Sender::close`] |
//! | Wake the reactor from another thread | [`Sender`] is `mio::Waker`-backed; both `send` and `close` wake automatically |
//! | Embed inside an existing event loop | [`Reactor::run_once`] |
//!
//! There is no extra API the embedder has to add. [`Reactor::run_echo`]
//! is **not** the embedding entry point; it is the bench-shape demo
//! that the headline single-core throughput numbers were taken
//! against.

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
const WAKER_TOKEN: Token = Token(usize::MAX);

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
  // Bytes leftover from an HTTP upgrade negotiated outside the
  // reactor (e.g. by hyper, axum, or a custom HTTP layer) that
  // were already pulled from the kernel buffer before the socket
  // changed hands. Prepended to the first recv so the engine sees
  // a continuous WebSocket stream. Only ever non-empty when the
  // session was added via
  // [`Reactor::add_session_with_prefix`](Reactor::add_session_with_prefix).
  pending_prefix: Vec<u8>,
  // True until [`Handler::on_open`] has fired for this session.
  // Set on every newly created session and cleared on the first
  // open-eligible event: handshake-just-completed (reactor-built-in
  // upgrade), the first prefix-processing tick (`add_session_with_prefix`),
  // or the first handle_readable for a pre-upgraded session
  // (`add_session`).
  needs_open: bool,
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
      pending_prefix: Vec::new(),
      needs_open: true,
      wq: VecDeque::new(),
      phase: Phase::Handshake,
      interest: Interest::READABLE,
    }
  }

  /// Construct a session for a socket that has already been upgraded
  /// at the HTTP layer by the caller. The reactor will not attempt to
  /// parse a handshake on it. `prefix` is any bytes pulled from the
  /// kernel buffer before the handoff (e.g. hyper's
  /// `Parts::read_buf`); they are prepended to the next recv and
  /// processed before any new socket data.
  fn from_upgraded(stream: TcpStream, prefix: Vec<u8>) -> Self {
    let _ = stream.set_nodelay(true);
    Self {
      stream,
      engine: ServerEngine::new(),
      partial_handshake: Vec::new(),
      pending_prefix: prefix,
      needs_open: true,
      wq: VecDeque::new(),
      phase: Phase::Echoing,
      interest: Interest::READABLE,
    }
  }
}

/// Handle to a session inside the reactor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(usize);

/// Per-frame outbound actions queued by the user handler.
///
/// Kept private; mutated only through [`Connection`]'s methods.
#[derive(Default)]
struct Outbound {
  /// Set by [`Connection::echo`]. Maps to
  /// [`ServerResponse::Echo`] when the engine asks what to do with
  /// this frame: the engine then writes the response header into
  /// the freed-up mask slot and emits the payload zero-copy.
  echo: bool,
  /// Set by [`Connection::close`]. After the current frame is
  /// processed, the reactor transitions the session to [`Phase::Closed`]
  /// and drops it from the slab once the write queue drains.
  close: bool,
  /// Bytes pushed by [`Connection::send`]. Includes the frame
  /// header. Drained into the per-session write queue after the
  /// frame handler returns.
  sends: Vec<u8>,
}

/// Per-frame handle the reactor passes to a [`Handler`]. Identifies
/// the session and offers three outbound actions:
///
/// - [`echo`](Self::echo): send this frame's (possibly mutated)
///   payload back as a same-opcode, same-FIN response. Zero-copy on
///   the hot path (masked input + payload < 65 536 bytes): the
///   engine writes the response header into the slot the mask
///   freed up in the recv buffer and ships the contiguous slice
///   in one `send()`.
/// - [`send`](Self::send): queue an arbitrary outbound frame
///   (opcode + payload). The bytes are copied into the session's
///   outbound queue and sent in FIFO order with respect to other
///   `send` calls and any subsequent `echo`.
/// - [`close`](Self::close): start a graceful close after the
///   current write queue drains.
///
/// `Connection` is short-lived — valid only for the duration of one
/// [`Handler::on_frame`] / [`Handler::on_open`] call. To remember a
/// connection across calls, save its [`id`](Self::id) and look it
/// up later via your own data structure (e.g. a `HashMap`); the
/// reactor's `SessionId`s are stable for the lifetime of a session.
pub struct Connection<'a> {
  id: SessionId,
  out: &'a mut Outbound,
}

impl Connection<'_> {
  /// Stable identifier for this session. Same value across all
  /// [`Handler`] callbacks until the session closes.
  pub fn id(&self) -> SessionId {
    self.id
  }

  /// Echo this frame's payload back, with the same opcode and FIN
  /// bit. Zero-copy in the common case (masked client input, payload
  /// < 65 536 bytes). If the handler mutated `payload` before
  /// calling this, the modified bytes are what go on the wire — the
  /// engine writes the response header into the buffer in place.
  ///
  /// Calling `echo` more than once per frame has no extra effect.
  pub fn echo(&mut self) {
    self.out.echo = true;
  }

  /// Queue an arbitrary outbound frame. Builds a server-side
  /// (unmasked) WebSocket header for `opcode` + `payload` and
  /// appends it to the session's outbound queue. The bytes are
  /// copied; ownership of `payload` stays with the caller.
  ///
  /// Multiple `send` calls within one [`Handler::on_frame`] queue in
  /// FIFO order; `send` bytes precede any [`echo`](Self::echo)
  /// response for the same frame.
  pub fn send(&mut self, opcode: OpCode, payload: &[u8]) {
    let mut hdr = [0u8; 10];
    let n = fmt_server_head(&mut hdr, opcode, payload.len());
    self.out.sends.extend_from_slice(&hdr[..n]);
    self.out.sends.extend_from_slice(payload);
  }

  /// Start a graceful close. The reactor sends the queued outbound
  /// bytes (including any [`send`](Self::send) / [`echo`](Self::echo)
  /// queued in the current frame), then closes the socket and
  /// removes the session.
  pub fn close(&mut self) {
    self.out.close = true;
  }
}

/// User code that implements WebSocket server logic on top of the
/// reactor.
///
/// The trait is split into three callbacks. All three are called
/// inline on the reactor thread: do not block, do not call into
/// async runtimes. For long-running work, offload to a worker
/// thread / channel / queue and respond from the next call.
pub trait Handler {
  /// Called once per session, after the WebSocket handshake
  /// succeeds (whether negotiated by the reactor in [`Reactor::bind`]
  /// flow or supplied pre-upgraded via [`Reactor::add_session`]).
  /// Use this to allocate per-session state or send a greeting
  /// frame.
  fn on_open(&mut self, conn: &mut Connection<'_>) {
    let _ = conn;
  }

  /// Called for each WebSocket data frame (Text or Binary) the
  /// engine parses. `payload` is the unmasked frame body inside
  /// the engine's recv buffer; mutating it before
  /// [`Connection::echo`] sends the modified bytes back with no
  /// extra allocation. Control frames (Ping → Pong, Close echo)
  /// are handled internally and do not reach this callback.
  fn on_frame(
    &mut self,
    conn: &mut Connection<'_>,
    payload: &mut [u8],
    opcode: OpCode,
  );

  /// Called once per session, after the socket has closed or the
  /// reactor has finished draining a [`Connection::close`]. The
  /// `SessionId` is no longer valid after this call.
  fn on_close(&mut self, id: SessionId) {
    let _ = id;
  }
}

/// Adapt a closure into a [`Handler`] for the common "only handle
/// data frames" case. The wrapped closure becomes
/// [`Handler::on_frame`]; `on_open` and `on_close` keep their
/// default no-op implementations.
///
/// ```no_run
/// # #[cfg(all(target_os = "linux", feature = "reactor"))]
/// # fn _doc() -> std::io::Result<()> {
/// use fastwebsockets::reactor::{Reactor, handler_fn};
/// let mut reactor = Reactor::new()?;
/// reactor.bind("127.0.0.1:8080")?;
/// reactor.run(&mut handler_fn(|conn, payload, opcode| {
///   conn.echo();
///   let _ = (payload, opcode);
/// }))?;
/// # Ok(())
/// # }
/// ```
pub fn handler_fn<F>(f: F) -> impl Handler
where
  F: FnMut(&mut Connection<'_>, &mut [u8], OpCode),
{
  struct FnHandler<F>(F);
  impl<F> Handler for FnHandler<F>
  where
    F: FnMut(&mut Connection<'_>, &mut [u8], OpCode),
  {
    fn on_frame(
      &mut self,
      conn: &mut Connection<'_>,
      payload: &mut [u8],
      opcode: OpCode,
    ) {
      (self.0)(conn, payload, opcode)
    }
  }
  FnHandler(f)
}

/// A cross-thread command to a [`Reactor`]. Posted via [`Sender`];
/// consumed by the reactor before each `poll`.
enum Command {
  /// Build a server-side frame and append it to the session's
  /// outbound queue, then re-arm writability so the reactor drains
  /// it on the next tick.
  Send {
    id: SessionId,
    opcode: OpCode,
    payload: Vec<u8>,
  },
  /// Mark the session for graceful close after pending writes
  /// drain.
  Close { id: SessionId },
}

/// Cross-thread handle for posting outbound work to a running
/// [`Reactor`]. Construct with [`Reactor::sender`]; clone freely.
/// Calls return immediately; the reactor processes the queue in
/// FIFO order from inside its own poll loop.
///
/// This is the integration point Deno (or any other manager that
/// owns a tokio runtime + a reactor thread) uses to push frames
/// out to a session whose [`SessionId`] is known but whose
/// per-session state lives on the reactor thread. Sending a
/// command to a closed session is a no-op.
#[derive(Clone)]
pub struct Sender {
  inner: std::sync::Arc<SenderInner>,
}

struct SenderInner {
  queue: std::sync::Mutex<std::collections::VecDeque<Command>>,
  waker: std::sync::Arc<mio::Waker>,
}

impl Sender {
  /// Queue a frame to be sent on the given session.
  ///
  /// `payload` is copied. Returns `Ok` once the command is queued;
  /// actual delivery is asynchronous (the reactor wakes, drains
  /// the queue, appends header + payload to the session's outbound
  /// buffer, then writes when the socket is writable).
  pub fn send(
    &self,
    id: SessionId,
    opcode: OpCode,
    payload: Vec<u8>,
  ) -> std::io::Result<()> {
    {
      let mut q = self
        .inner
        .queue
        .lock()
        .expect("reactor command queue poisoned");
      q.push_back(Command::Send {
        id,
        opcode,
        payload,
      });
    }
    self.inner.waker.wake()
  }

  /// Queue a graceful close on the given session. The reactor
  /// stops reading immediately, drains pending writes, then drops
  /// the session and fires [`Handler::on_close`].
  pub fn close(&self, id: SessionId) -> std::io::Result<()> {
    {
      let mut q = self
        .inner
        .queue
        .lock()
        .expect("reactor command queue poisoned");
      q.push_back(Command::Close { id });
    }
    self.inner.waker.wake()
  }
}

/// Single-thread server-side WebSocket reactor.
///
/// See the module-level docs for an overview. Construct with
/// [`new`](Self::new), optionally bind a listener for built-in accept
/// with [`bind`](Self::bind), pass already-upgraded sockets with
/// [`add_session`](Self::add_session), grab a [`Sender`] via
/// [`sender`](Self::sender) if you need cross-thread outbound
/// posting, and drive the event loop with [`run`](Self::run) /
/// [`run_echo`](Self::run_echo).
pub struct Reactor {
  poll: Poll,
  events: Events,
  sessions: slab::Slab<Session>,
  scratch: Box<[u8]>,
  listener: Option<TcpListener>,
  sender_inner: std::sync::Arc<SenderInner>,
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
    let poll = Poll::new()?;
    let waker =
      std::sync::Arc::new(mio::Waker::new(poll.registry(), WAKER_TOKEN)?);
    let sender_inner = std::sync::Arc::new(SenderInner {
      queue: std::sync::Mutex::new(std::collections::VecDeque::new()),
      waker,
    });
    Ok(Self {
      poll,
      events: Events::with_capacity(events_capacity),
      sessions: slab::Slab::with_capacity(64),
      scratch: vec![0u8; scratch_bytes].into_boxed_slice(),
      listener: None,
      sender_inner,
    })
  }

  /// Clone a cross-thread [`Sender`] handle. Send / close commands
  /// posted through it wake the reactor and are applied before the
  /// next poll. Clone the sender as many times as you need.
  ///
  /// This is the integration point for embedding the reactor
  /// behind a manager that lives on a different thread: hand the
  /// manager a [`Sender`] when you create the reactor and use it
  /// to push outbound frames / close commands from anywhere.
  pub fn sender(&self) -> Sender {
    Sender {
      inner: std::sync::Arc::clone(&self.sender_inner),
    }
  }

  /// Bind a TCP listener on `addr` and register it with the reactor.
  /// Incoming connections will be accepted by [`run`](Self::run) and
  /// their HTTP upgrade negotiated inline before framing starts.
  pub fn bind(&mut self, addr: &str) -> std::io::Result<()> {
    let parsed: SocketAddr = addr.parse().map_err(|e| {
      std::io::Error::new(ErrorKind::InvalidInput, format!("{}", e))
    })?;
    let mut listener = TcpListener::bind(parsed)?;
    self.poll.registry().register(
      &mut listener,
      LISTENER_TOKEN,
      Interest::READABLE,
    )?;
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
    stream: TcpStream,
  ) -> std::io::Result<SessionId> {
    self.add_session_with_prefix(stream, Vec::new())
  }

  /// Add an already-upgraded WebSocket stream plus any bytes that
  /// were already pulled from its kernel buffer before the handoff.
  ///
  /// HTTP upgrade libraries (hyper, axum, …) typically deliver an
  /// upgraded socket plus a leftover buffer of bytes that were
  /// read past the HTTP request boundary. The first WebSocket
  /// frame the client sent may be entirely inside that buffer (a
  /// pipelined client), or straddle it; in either case those bytes
  /// must be processed before any new socket read or the engine
  /// will start reading mid-frame from the kernel.
  ///
  /// Pass `prefix` empty if you don't have any (equivalent to
  /// [`add_session`](Self::add_session)).
  ///
  /// The prefix is processed on the next call to
  /// [`run`](Self::run) / [`run_once`](Self::run_once) — the
  /// reactor wakes itself via the cross-thread [`Sender`]'s
  /// waker so the new session is picked up promptly even if no
  /// other event source has fired.
  pub fn add_session_with_prefix(
    &mut self,
    mut stream: TcpStream,
    prefix: Vec<u8>,
  ) -> std::io::Result<SessionId> {
    let entry = self.sessions.vacant_entry();
    let token = Token(entry.key() + 1);
    self
      .poll
      .registry()
      .register(&mut stream, token, Interest::READABLE)?;
    let has_prefix = !prefix.is_empty();
    entry.insert(Session::from_upgraded(stream, prefix));
    if has_prefix {
      // Make sure the run loop ticks soon, even if no other event
      // source has data. We piggy-back on the cross-thread waker
      // (which is also what `Sender` uses); failing to wake here
      // would leave the prefix unprocessed until the next event
      // arrives on its own.
      let _ = self.sender_inner.waker.wake();
    }
    Ok(SessionId(token.0))
  }

  /// Drive the event loop with a built-in echo handler.
  /// Equivalent to calling [`run`](Self::run) with a handler that
  /// always calls [`Connection::echo`] on every data frame.
  ///
  /// **This is a demo / benchmark entry point, not the embedding
  /// API.** The headline single-core throughput numbers in this
  /// crate's perf report are taken against this path because it
  /// is the minimum work a reactor-driven WebSocket server can do.
  /// Real applications — including HTTP-server / runtime-extension
  /// embedders such as Deno — should use [`run`](Self::run) with
  /// their own [`Handler`] implementation, route already-upgraded
  /// sockets through [`add_session`](Self::add_session) /
  /// [`add_session_with_prefix`](Self::add_session_with_prefix),
  /// and post cross-thread sends through [`Sender`]. See the
  /// "Embedding from an HTTP server or runtime extension" section
  /// in the module-level docs.
  pub fn run_echo(&mut self) -> std::io::Result<()> {
    struct EchoHandler;
    impl Handler for EchoHandler {
      fn on_frame(
        &mut self,
        conn: &mut Connection<'_>,
        _payload: &mut [u8],
        _opcode: OpCode,
      ) {
        conn.echo();
      }
    }
    self.run(&mut EchoHandler)
  }

  /// Drive the event loop. Runs until the listener (if any) is
  /// dropped and all sessions have closed.
  ///
  /// `handler` is invoked synchronously on the reactor thread: do
  /// not block, do not enter an async runtime. To do non-trivial
  /// work, offload to a worker via a channel and reply from the
  /// next callback. See [`Handler`] / [`Connection`] for the per-
  /// frame API.
  pub fn run<H: Handler>(&mut self, handler: &mut H) -> std::io::Result<()> {
    loop {
      // The reactor keeps running while it has a listener OR active
      // sessions OR a cross-thread sender that may still post work.
      // Otherwise the call returns Ok(()) so callers using
      // bind+run get a finite lifetime.
      if self.listener.is_none()
        && self.sessions.is_empty()
        && std::sync::Arc::strong_count(&self.sender_inner) == 1
      {
        return Ok(());
      }
      self.drain_commands(handler);
      self.process_pending_prefixes(handler);
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
          self.accept_until_block(handler)?;
        } else if token == WAKER_TOKEN {
          self.drain_commands(handler);
          self.process_pending_prefixes(handler);
        } else {
          self.process_event(event, handler);
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
  pub fn run_once<H: Handler>(
    &mut self,
    timeout: Option<std::time::Duration>,
    handler: &mut H,
  ) -> std::io::Result<()> {
    self.drain_commands(handler);
    self.process_pending_prefixes(handler);
    self.poll.poll(&mut self.events, timeout)?;
    let mut events = std::mem::replace(
      &mut self.events,
      Events::with_capacity(self.sessions.capacity().max(64)),
    );
    for event in events.iter() {
      let token = event.token();
      if token == LISTENER_TOKEN {
        self.accept_until_block(handler)?;
      } else if token == WAKER_TOKEN {
        self.drain_commands(handler);
        self.process_pending_prefixes(handler);
      } else {
        self.process_event(event, handler);
      }
    }
    events.clear();
    let _ = std::mem::replace(&mut self.events, events);
    Ok(())
  }

  /// Walk active sessions looking for ones that arrived with a
  /// non-empty `pending_prefix` and drive the engine over those
  /// bytes inline (no socket read). Called once at the top of each
  /// run iteration and whenever the cross-thread waker fires, so a
  /// freshly-added session's leftover bytes are visible to the
  /// user handler before the reactor parks in `poll`. Iterates the
  /// slab linearly because pending sessions are normally a small
  /// minority of total sessions in steady state.
  fn process_pending_prefixes<H: Handler>(&mut self, handler: &mut H) {
    // Snapshot keys so we don't iterate while we may remove from
    // the slab.
    let keys: Vec<usize> = self
      .sessions
      .iter()
      .filter_map(|(i, s)| (!s.pending_prefix.is_empty()).then_some(i))
      .collect();
    for idx in keys {
      if !self.sessions.contains(idx) {
        continue;
      }
      let session_id = SessionId(idx + 1);
      let close = process_pending_prefix(
        &mut self.sessions[idx],
        session_id,
        &mut self.scratch,
        handler,
      );
      if close {
        let mut s = self.sessions.remove(idx);
        let _ = self.poll.registry().deregister(&mut s.stream);
        handler.on_close(session_id);
      } else {
        let _ = reregister_if_needed(
          &mut self.sessions[idx],
          &self.poll,
          Token(idx + 1),
        );
      }
    }
  }

  /// Drain any commands posted via [`Sender`] and apply them to
  /// the session slab. Sends queue bytes; close marks the session
  /// for graceful close (drained on the next event tick).
  fn drain_commands<H: Handler>(&mut self, handler: &mut H) {
    let drained: Vec<Command> = {
      let mut q = self
        .sender_inner
        .queue
        .lock()
        .expect("reactor command queue poisoned");
      q.drain(..).collect()
    };
    for cmd in drained {
      match cmd {
        Command::Send {
          id,
          opcode,
          payload,
        } => {
          let idx = id.0.wrapping_sub(1);
          if !self.sessions.contains(idx) {
            continue;
          }
          let session = &mut self.sessions[idx];
          if session.phase == Phase::Handshake || session.phase == Phase::Closed
          {
            continue;
          }
          let mut hdr = [0u8; 10];
          let n = fmt_server_head(&mut hdr, opcode, payload.len());
          // Append directly to the wq; we don't try the "write
          // immediately" fast path here because we're outside of an
          // event tick, the socket may not be writable, and the
          // reregister call below will arm WRITABLE so the next
          // tick drains.
          session.wq.extend(&hdr[..n]);
          session.wq.extend(&payload);
          let _ = reregister_if_needed(session, &self.poll, Token(idx + 1));
        }
        Command::Close { id } => {
          let idx = id.0.wrapping_sub(1);
          if !self.sessions.contains(idx) {
            continue;
          }
          let session = &mut self.sessions[idx];
          session.phase = Phase::Closed;
          if session.wq.is_empty() {
            // Nothing to drain; remove the session right away and
            // notify.
            let mut s = self.sessions.remove(idx);
            let _ = self.poll.registry().deregister(&mut s.stream);
            handler.on_close(id);
          } else {
            // Make sure we get woken to drain the wq.
            let _ = reregister_if_needed(session, &self.poll, Token(idx + 1));
          }
        }
      }
    }
  }

  fn accept_until_block<H: Handler>(
    &mut self,
    _handler: &mut H,
  ) -> std::io::Result<()> {
    let Some(listener) = self.listener.as_mut() else {
      return Ok(());
    };
    loop {
      match listener.accept() {
        Ok((stream, _)) => {
          let entry = self.sessions.vacant_entry();
          let token = Token(entry.key() + 1);
          let mut session = Session::new(stream);
          self.poll.registry().register(
            &mut session.stream,
            token,
            Interest::READABLE,
          )?;
          entry.insert(session);
          // Handshake hasn't completed yet; `on_open` will fire from
          // `handle_readable` once the upgrade succeeds. For
          // pre-upgraded sessions added via `add_session` the same
          // hook fires on the first readable event.
        }
        Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(()),
        Err(_) => return Ok(()),
      }
    }
  }

  fn process_event<H: Handler>(&mut self, event: &Event, handler: &mut H) {
    let idx = event.token().0.wrapping_sub(1);
    if !self.sessions.contains(idx) {
      return;
    }
    let session_id = SessionId(idx + 1);
    let mut close = false;
    if event.is_readable() {
      close |= handle_readable(
        &mut self.sessions[idx],
        session_id,
        &mut self.scratch,
        handler,
      );
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
      handler.on_close(session_id);
      return;
    }
    let _ =
      reregister_if_needed(&mut self.sessions[idx], &self.poll, Token(idx + 1));
  }
}

// Returns true if the session should be closed.
fn handle_readable<H: Handler>(
  session: &mut Session,
  session_id: SessionId,
  scratch: &mut [u8],
  handler: &mut H,
) -> bool {
  // Drain any pending_prefix into the front of the recv scratch.
  // For embedders that add an already-upgraded socket via
  // `add_session_with_prefix`, those bytes were pulled from the
  // kernel by the upstream HTTP layer; the engine has to see
  // them before any bytes the socket still has buffered.
  let prefix_len = if !session.pending_prefix.is_empty() {
    let p = std::mem::take(&mut session.pending_prefix);
    if p.len() > scratch.len() {
      // Caller handed us more leftover bytes than scratch can
      // hold in one go. The engine's own partial-frame buffer
      // can absorb anything that doesn't fit in one call to
      // `process`, so loop and feed slices of `scratch.len()`
      // until exhausted. Rare; only relevant if the embedder
      // passes a prefix larger than 64 KiB.
      let mut left = p.as_slice();
      while left.len() > scratch.len() {
        scratch.copy_from_slice(&left[..scratch.len()]);
        if process_buffered(session, session_id, scratch, handler).is_err()
          || session.engine.is_closed()
        {
          return true;
        }
        left = &left[scratch.len()..];
      }
      let n = left.len();
      scratch[..n].copy_from_slice(left);
      n
    } else {
      scratch[..p.len()].copy_from_slice(&p);
      p.len()
    }
  } else {
    0
  };

  // Read what the kernel has on top of (after) the prefix.
  let n = match session.stream.read(&mut scratch[prefix_len..]) {
    Ok(0) if prefix_len == 0 => return true,
    Ok(n) => n,
    Err(e) if e.kind() == ErrorKind::WouldBlock => 0,
    Err(_) => return true,
  };
  let n = prefix_len + n;
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
    if write_now(&mut session.stream, &mut session.wq, &[IoSlice::new(&resp)])
      .is_err()
    {
      return true;
    }
    read_pos = eom;
    session.phase = Phase::Echoing;
  }

  // Fire `on_open` once per session, regardless of whether the
  // session arrived via the reactor's built-in handshake or via
  // `add_session` / `add_session_with_prefix` from an external
  // HTTP layer.
  if session.needs_open {
    session.needs_open = false;
    let mut out = Outbound::default();
    {
      let mut conn = Connection {
        id: session_id,
        out: &mut out,
      };
      handler.on_open(&mut conn);
    }
    apply_outbound(session, &mut out);
    if out.close {
      session.phase = Phase::Closed;
    }
  }

  if read_pos >= n {
    return false;
  }

  // Process whatever WebSocket frames are in scratch[read_pos..n].
  // The engine calls the handler closure once per data frame and
  // the write closure once per engine-emitted response chunk; both
  // need shared access to `session.stream` + `session.wq`, so we
  // wrap them in RefCells. The two closures don't run concurrently
  // (the engine drives them serially), so the RefCell borrows
  // never overlap in practice.
  let mut process_close = false;
  let process_result = {
    let stream_cell = std::cell::RefCell::new(&mut session.stream);
    let wq_cell = std::cell::RefCell::new(&mut session.wq);
    session.engine.process(
      &mut scratch[read_pos..n],
      |bytes| {
        let mut stream = stream_cell.borrow_mut();
        let mut wq = wq_cell.borrow_mut();
        let _ = write_contig_now(*stream, *wq, bytes);
      },
      |payload, opcode| {
        let mut out = Outbound::default();
        {
          let mut conn = Connection {
            id: session_id,
            out: &mut out,
          };
          handler.on_frame(&mut conn, payload, opcode);
        }
        // Drain user-queued sends before the engine emits the
        // echo response for this frame, so the wire order is
        // [user sends..., echo].
        if !out.sends.is_empty() {
          let mut stream = stream_cell.borrow_mut();
          let mut wq = wq_cell.borrow_mut();
          let _ = write_contig_now(*stream, *wq, &out.sends);
        }
        if out.close {
          process_close = true;
        }
        if out.echo {
          ServerResponse::Echo
        } else {
          ServerResponse::Discard
        }
      },
    )
  };
  if process_result.is_err() {
    return true;
  }
  if process_close {
    session.phase = Phase::Closed;
  }
  session.engine.is_closed()
}

/// Apply user-queued sends + close from `on_open` (which runs before
/// any framing). Echo is meaningless during `on_open` (no inbound
/// frame to echo), but `send` and `close` are.
fn apply_outbound(session: &mut Session, out: &mut Outbound) {
  if !out.sends.is_empty() {
    let _ = write_contig_now(&mut session.stream, &mut session.wq, &out.sends);
  }
  out.sends.clear();
}

/// Build a server-side (unmasked) WebSocket frame header for an
/// `opcode` + payload-length combination. Returns the number of
/// header bytes written to `buf`. Used by [`Connection::send`].
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

/// Process `scratch[..scratch.len()]` as a chunk of pre-buffered
/// bytes (no kernel read). Used by [`handle_readable`] when the
/// caller-supplied prefix is larger than the scratch buffer can
/// hold in one engine call. Returns Err if the engine signaled a
/// protocol failure on the chunk.
fn process_buffered<H: Handler>(
  session: &mut Session,
  session_id: SessionId,
  scratch: &mut [u8],
  handler: &mut H,
) -> Result<(), ()> {
  // Same dispatch shape as `handle_readable`'s engine call, minus
  // the handshake leg (sessions that get a pending_prefix are
  // always already in Phase::Echoing).
  let stream_cell = std::cell::RefCell::new(&mut session.stream);
  let wq_cell = std::cell::RefCell::new(&mut session.wq);
  let mut process_close = false;
  let result = session.engine.process(
    scratch,
    |bytes| {
      let mut stream = stream_cell.borrow_mut();
      let mut wq = wq_cell.borrow_mut();
      let _ = write_contig_now(*stream, *wq, bytes);
    },
    |payload, opcode| {
      let mut out = Outbound::default();
      {
        let mut conn = Connection {
          id: session_id,
          out: &mut out,
        };
        handler.on_frame(&mut conn, payload, opcode);
      }
      if !out.sends.is_empty() {
        let mut stream = stream_cell.borrow_mut();
        let mut wq = wq_cell.borrow_mut();
        let _ = write_contig_now(*stream, *wq, &out.sends);
      }
      if out.close {
        process_close = true;
      }
      if out.echo {
        ServerResponse::Echo
      } else {
        ServerResponse::Discard
      }
    },
  );
  if process_close {
    session.phase = Phase::Closed;
  }
  if result.is_err() {
    Err(())
  } else {
    Ok(())
  }
}

/// Walk a single session's pending_prefix through the engine. No
/// kernel read; this is for sessions added via
/// [`Reactor::add_session_with_prefix`] before the reactor has
/// seen any event for them. Returns true if the session should be
/// closed (engine error / Close frame seen).
fn process_pending_prefix<H: Handler>(
  session: &mut Session,
  session_id: SessionId,
  scratch: &mut [u8],
  handler: &mut H,
) -> bool {
  let prefix = std::mem::take(&mut session.pending_prefix);
  // Fire on_open on the first time we see the session, before the
  // user sees any frames.
  if session.needs_open {
    session.needs_open = false;
    let mut out = Outbound::default();
    {
      let mut conn = Connection {
        id: session_id,
        out: &mut out,
      };
      handler.on_open(&mut conn);
    }
    apply_outbound(session, &mut out);
    if out.close {
      session.phase = Phase::Closed;
      return true;
    }
  }
  // Run the prefix through the engine. Loop if it doesn't fit in
  // one scratch.
  let mut left = prefix.as_slice();
  while !left.is_empty() {
    let n = left.len().min(scratch.len());
    scratch[..n].copy_from_slice(&left[..n]);
    let chunk = &mut scratch[..n];
    if process_buffered(session, session_id, chunk, handler).is_err() {
      return true;
    }
    if session.engine.is_closed() || session.phase == Phase::Closed {
      return true;
    }
    left = &left[n..];
  }
  false
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
    assert_eq!(
      find_double_crlf(b"GET / HTTP/1.1\r\nHost: x\r\n\r\nrest"),
      Some(27)
    );
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

  /// Set up a socket-pair and register the server end with the
  /// reactor as an already-upgraded session. Returns
  /// `(reactor, client_side)`.
  fn paired() -> (Reactor, std::os::unix::net::UnixStream) {
    use std::os::fd::AsRawFd;
    use std::os::fd::FromRawFd;
    let mut fds: [libc::c_int; 2] = [-1, -1];
    let rc = unsafe {
      libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr())
    };
    assert_eq!(
      rc,
      0,
      "socketpair failed: {}",
      std::io::Error::last_os_error()
    );
    let server_fd = fds[0];
    let client = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fds[1]) };
    unsafe {
      let flags = libc::fcntl(server_fd, libc::F_GETFL);
      libc::fcntl(server_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
      let flags = libc::fcntl(client.as_raw_fd(), libc::F_GETFL);
      libc::fcntl(client.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    let stream = unsafe { TcpStream::from_raw_fd(server_fd) };
    let mut reactor = Reactor::new().unwrap();
    let _ = reactor.add_session(stream).unwrap();
    (reactor, client)
  }

  /// Build a client→server masked frame for `bytes` with opcode
  /// 0x82 (Binary, FIN).
  fn mk_masked_binary(bytes: &[u8]) -> Vec<u8> {
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

  /// Drive the reactor for up to a few ticks so any pending
  /// readable/writable events fire and the kernel hands the
  /// outbound bytes back to the client side of the socket pair.
  fn tick<H: Handler>(reactor: &mut Reactor, handler: &mut H) {
    for _ in 0..4 {
      reactor
        .run_once(Some(std::time::Duration::from_millis(50)), handler)
        .unwrap();
    }
  }

  /// `Handler::on_frame` -> `conn.echo()` reflects a masked binary
  /// frame back unmasked, with the in-place response synthesis.
  #[test]
  fn reactor_echoes_via_handler_trait() {
    use std::io::Read as _;
    use std::io::Write as _;

    let (mut reactor, mut client) = paired();
    client.write_all(&mk_masked_binary(b"hello")).unwrap();

    struct EchoOnly;
    impl Handler for EchoOnly {
      fn on_frame(
        &mut self,
        conn: &mut Connection<'_>,
        _payload: &mut [u8],
        _opcode: OpCode,
      ) {
        conn.echo();
      }
    }
    tick(&mut reactor, &mut EchoOnly);

    let mut buf = [0u8; 32];
    let n = client.read(&mut buf).unwrap();
    assert_eq!(&buf[..n], &[0x82, 5, b'h', b'e', b'l', b'l', b'o']);
  }

  /// `Connection::send` queues a server-side (unmasked) frame
  /// independent of any echo. The reactor sends `send` bytes before
  /// the echo for the same frame, so we can observe both.
  #[test]
  fn reactor_send_then_echo_in_order() {
    use std::io::Read as _;
    use std::io::Write as _;

    let (mut reactor, mut client) = paired();
    client.write_all(&mk_masked_binary(b"PING")).unwrap();

    struct SendThenEcho;
    impl Handler for SendThenEcho {
      fn on_frame(
        &mut self,
        conn: &mut Connection<'_>,
        _payload: &mut [u8],
        _opcode: OpCode,
      ) {
        conn.send(OpCode::Binary, b"hi");
        conn.echo();
      }
    }
    tick(&mut reactor, &mut SendThenEcho);

    let mut buf = [0u8; 64];
    let n = client.read(&mut buf).unwrap();
    // First: "hi" (server-sent, 2-byte unmasked Binary frame), then
    // "PING" (echo, 4-byte unmasked Binary frame).
    assert_eq!(
      &buf[..n],
      &[0x82, 2, b'h', b'i', 0x82, 4, b'P', b'I', b'N', b'G']
    );
  }

  /// Handler can mutate the payload before calling `echo`; the
  /// modified bytes go on the wire in place (no extra copy).
  #[test]
  fn reactor_mutate_then_echo() {
    use std::io::Read as _;
    use std::io::Write as _;

    let (mut reactor, mut client) = paired();
    client.write_all(&mk_masked_binary(b"abcd")).unwrap();

    let mut h = handler_fn(|conn, payload, _op| {
      for b in payload.iter_mut() {
        *b = b.to_ascii_uppercase();
      }
      conn.echo();
    });
    tick(&mut reactor, &mut h);

    let mut buf = [0u8; 32];
    let n = client.read(&mut buf).unwrap();
    assert_eq!(&buf[..n], &[0x82, 4, b'A', b'B', b'C', b'D']);
  }

  /// Cross-thread Sender: post a `send` command from inside the
  /// handler (proxy for posting from another thread; same code
  /// path, easier to test deterministically) and verify the bytes
  /// land on the wire even though the handler itself didn't call
  /// `conn.send`.
  #[test]
  fn sender_send_command_delivers() {
    use std::io::Read as _;
    use std::io::Write as _;

    let (mut reactor, mut client) = paired();
    let sender = reactor.sender();
    client.write_all(&mk_masked_binary(b"ping")).unwrap();

    // The handler captures `sender` and the SessionId from the
    // first frame it sees, then posts a Send command through the
    // Sender. The reactor processes commands at the top of each
    // poll, so the queued bytes go out on the very next tick.
    let sent_id: std::cell::Cell<Option<SessionId>> =
      std::cell::Cell::new(None);
    {
      let mut h = handler_fn(|conn, _payload, _op| {
        sent_id.set(Some(conn.id()));
        sender
          .send(conn.id(), OpCode::Binary, b"pong".to_vec())
          .unwrap();
      });
      tick(&mut reactor, &mut h);
    }

    assert!(sent_id.get().is_some());
    let mut buf = [0u8; 64];
    let n = client.read(&mut buf).unwrap();
    assert_eq!(&buf[..n], &[0x82, 4, b'p', b'o', b'n', b'g']);
  }

  /// `add_session_with_prefix` feeds caller-supplied leftover bytes
  /// (e.g. hyper's `Parts::read_buf` after an HTTP upgrade) to the
  /// engine before reading anything from the socket. The prefix
  /// here contains a complete masked Binary frame, so the handler
  /// fires once and the echo lands on the client side without any
  /// new bytes ever crossing the socket.
  #[test]
  fn add_session_with_prefix_processes_leftover_bytes() {
    use std::io::Read as _;
    use std::os::fd::AsRawFd;
    use std::os::fd::FromRawFd;

    let mut fds: [libc::c_int; 2] = [-1, -1];
    let rc = unsafe {
      libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr())
    };
    assert_eq!(rc, 0);
    let server_fd = fds[0];
    let mut client =
      unsafe { std::os::unix::net::UnixStream::from_raw_fd(fds[1]) };
    unsafe {
      let f = libc::fcntl(server_fd, libc::F_GETFL);
      libc::fcntl(server_fd, libc::F_SETFL, f | libc::O_NONBLOCK);
      let f = libc::fcntl(client.as_raw_fd(), libc::F_GETFL);
      libc::fcntl(client.as_raw_fd(), libc::F_SETFL, f | libc::O_NONBLOCK);
    }
    let stream = unsafe { TcpStream::from_raw_fd(server_fd) };

    let prefix = mk_masked_binary(b"prefixed!");
    let mut reactor = Reactor::new().unwrap();
    let _id = reactor.add_session_with_prefix(stream, prefix).unwrap();

    let mut h = handler_fn(|conn, _payload, _opcode| conn.echo());
    tick(&mut reactor, &mut h);

    let mut buf = [0u8; 64];
    let n = client.read(&mut buf).unwrap();
    assert_eq!(
      &buf[..n],
      &[0x82, 9, b'p', b'r', b'e', b'f', b'i', b'x', b'e', b'd', b'!']
    );
  }

  /// `Handler::on_open` fires exactly once per session, before any
  /// frames, for every session — including pre-upgraded sessions
  /// supplied via `add_session` (no prefix, no handshake leg).
  #[test]
  fn on_open_fires_for_pre_upgraded_sessions() {
    use std::io::Write as _;

    let (mut reactor, mut client) = paired();
    client.write_all(&mk_masked_binary(b"hi")).unwrap();

    struct CountingHandler {
      opens: usize,
      frames: usize,
    }
    impl Handler for CountingHandler {
      fn on_open(&mut self, _conn: &mut Connection<'_>) {
        self.opens += 1;
      }
      fn on_frame(
        &mut self,
        _conn: &mut Connection<'_>,
        _payload: &mut [u8],
        _opcode: OpCode,
      ) {
        self.frames += 1;
      }
    }
    let mut h = CountingHandler {
      opens: 0,
      frames: 0,
    };
    tick(&mut reactor, &mut h);
    assert_eq!(h.opens, 1, "on_open should fire exactly once");
    assert_eq!(h.frames, 1, "on_frame should see the one frame");
  }

  /// Cross-thread Sender close: posting `close` from outside the
  /// handler drops the session and fires `on_close`.
  #[test]
  fn sender_close_command_drops_session() {
    use std::io::Write as _;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    let (mut reactor, mut client) = paired();
    let sender = reactor.sender();
    client.write_all(&mk_masked_binary(b"hello")).unwrap();

    let closed = Arc::new(AtomicBool::new(false));
    let closed_in_handler = Arc::clone(&closed);
    let mut sent_id: Option<SessionId> = None;
    struct H<'a> {
      sender: Sender,
      closed: &'a AtomicBool,
      seen: &'a mut Option<SessionId>,
    }
    impl Handler for H<'_> {
      fn on_frame(
        &mut self,
        conn: &mut Connection<'_>,
        _payload: &mut [u8],
        _opcode: OpCode,
      ) {
        *self.seen = Some(conn.id());
        self.sender.close(conn.id()).unwrap();
      }
      fn on_close(&mut self, _id: SessionId) {
        self.closed.store(true, Ordering::SeqCst);
      }
    }
    let mut h = H {
      sender,
      closed: &closed_in_handler,
      seen: &mut sent_id,
    };
    tick(&mut reactor, &mut h);

    assert!(sent_id.is_some());
    assert!(closed.load(Ordering::SeqCst), "on_close was not fired");
  }
}
