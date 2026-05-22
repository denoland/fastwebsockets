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

//! End-to-end demo of `fastwebsockets::reactor::Reactor` as a
//! general WebSocket server. Implements a small broadcast chat
//! broker that exercises the full public API:
//!
//! - `Handler::on_open` records each new session id
//! - `Handler::on_frame` forwards every received frame to every
//!   *other* session via the cross-thread `Sender`
//! - `Handler::on_close` removes the session id from the roster
//! - The cross-thread `Sender` is what makes broadcast possible —
//!   you can't borrow another session from inside a `Handler`
//!   callback because the reactor holds it; posting commands
//!   through `Sender` defers the writes to the next poll tick.
//!
//! This is the shape a manager-style integration (e.g. Deno's
//! ext/websocket bridging eligible plain-TCP HTTP/1.1 sessions
//! into a reactor-backed worker) would use: many fds owned by
//! one reactor, command queue from the outside world, the reactor
//! drains commands at the top of each poll.

#[cfg(not(all(target_os = "linux", feature = "reactor")))]
fn main() {
  eprintln!("reactor_chat_broker: requires --features reactor on Linux");
}

#[cfg(all(target_os = "linux", feature = "reactor"))]
fn main() -> std::io::Result<()> {
  use fastwebsockets::reactor::{
    Connection, Handler, Reactor, Sender, SessionId,
  };
  use fastwebsockets::OpCode;
  use std::collections::HashSet;

  struct Broker {
    sender: Sender,
    members: HashSet<SessionId>,
  }
  impl Handler for Broker {
    fn on_open(&mut self, conn: &mut Connection<'_>) {
      self.members.insert(conn.id());
      conn.send(OpCode::Text, b"welcome");
    }
    fn on_frame(
      &mut self,
      conn: &mut Connection<'_>,
      payload: &mut [u8],
      opcode: OpCode,
    ) {
      // Fan out to every peer. We use the cross-thread Sender even
      // though we're on the reactor thread — it queues the bytes
      // and lets the reactor drain them at the top of the next
      // poll. The handler can't directly borrow another session
      // because the reactor holds it; Sender solves that.
      for &peer in &self.members {
        if peer == conn.id() {
          continue;
        }
        let _ = self.sender.send(peer, opcode, payload.to_vec());
      }
    }
    fn on_close(&mut self, id: SessionId) {
      self.members.remove(&id);
    }
  }

  let addr =
    std::env::var("FWS_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
  let mut reactor = Reactor::new()?;
  reactor.bind(&addr)?;
  let sender = reactor.sender();
  let mut broker = Broker {
    sender,
    members: HashSet::new(),
  };
  eprintln!("reactor chat broker listening on {}", addr);
  reactor.run(&mut broker)
}
