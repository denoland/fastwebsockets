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

//! Minimal demo of the public [`fastwebsockets::reactor::Reactor`]
//! API. Single-thread, single-CPU, no tokio: one event loop drives
//! all accepted WebSocket sessions through `ServerEngine`.
//!
//! Equivalent to `examples/echo_server_mio.rs`, but implemented as a
//! library consumer rather than as a hand-written mio loop — the
//! ~400 lines of mio + handshake + framing dispatch in that example
//! now collapse to the body of this one. The framing and event loop
//! live in `crate::reactor`.

// Stub for non-Linux / non-reactor builds so `cargo build --examples`
// still works on macOS / Windows.
#[cfg(not(all(target_os = "linux", feature = "reactor")))]
fn main() {
  eprintln!("echo_server_reactor: requires --features reactor on Linux");
}

#[cfg(all(target_os = "linux", feature = "reactor"))]
fn main() -> std::io::Result<()> {
  let addr =
    std::env::var("FWS_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
  let mut reactor = fastwebsockets::reactor::Reactor::new()?;
  reactor.bind(&addr)?;
  eprintln!("reactor echo listening on {}", addr);
  reactor.run_echo()
}
