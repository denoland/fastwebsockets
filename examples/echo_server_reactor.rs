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

//! Bench-shape demo of [`fastwebsockets::reactor::Reactor`] —
//! pure echo, the canonical perf comparison against uWebSockets.
//! Calls the built-in [`Reactor::run_echo`] convenience; for a
//! real-world handler with mutated frames / arbitrary sends /
//! cross-thread `Sender`, see `examples/reactor_chat_broker.rs`.
//!
//! Run with:
//!
//! ```text
//!   FWS_ADDR=127.0.0.1:8080 cargo run --release \
//!     --features reactor --example echo_server_reactor
//! ```

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
