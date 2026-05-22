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

//! Tokio-based echo server that uses `fastwebsockets::ServerEngine` for
//! framing. This is the "Deno-friendly" fast path: the I/O stays async
//! (so it integrates with the surrounding tokio app), but the per-frame
//! parse / unmask / response synthesis hot path runs synchronously
//! inside `ServerEngine::process` — no `Future` state machine per frame,
//! no `BytesMut::split_to`, no per-frame Arc atomic.
//!
//! Runs the bench's standard upgrade dance via hyper, then hands the
//! upgraded `TcpStream` to a tight async loop:
//!
//! ```text
//!   loop {
//!     n = stream.read(scratch).await?;          // 1 async await
//!     engine.process(&mut scratch[..n], ...)?;  // sync — the hot path
//!     stream.write_all(&wq).await?;             // 1 async await
//!   }
//! ```
//!
//! The Engine writes outbound bytes into a per-connection `Vec<u8>`
//! that we drain on every cycle. For the 16 KiB echo case this is one
//! extra memcpy (engine→wq, ~3 µs at our measured 7 GB/s scalar path)
//! vs the pure-mio path's "write straight from scratch"; in exchange
//! the rest of the tokio app's existing async machinery composes
//! cleanly.

use fastwebsockets::OpCode;
use fastwebsockets::ServerEngine;
use fastwebsockets::ServerResponse;
use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::Request;
use hyper::Response;
use hyper_util::rt::TokioIo;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::TcpStream;

use fastwebsockets::upgrade;

const SCRATCH_LEN: usize = 64 * 1024;

async fn echo_loop(mut stream: TcpStream) -> std::io::Result<()> {
  let _ = stream.set_nodelay(true);
  let mut engine = ServerEngine::new();
  let mut scratch = vec![0u8; SCRATCH_LEN];
  let mut wq: Vec<u8> = Vec::with_capacity(SCRATCH_LEN);
  loop {
    let n = stream.read(&mut scratch).await?;
    if n == 0 {
      break;
    }
    // engine.process is sync — the only async points in the per-frame
    // loop are the read and write above/below.
    let res = engine.process(
      &mut scratch[..n],
      |bytes| wq.extend_from_slice(bytes),
      |_payload, opcode| match opcode {
        OpCode::Text | OpCode::Binary => ServerResponse::Echo,
        _ => ServerResponse::Discard,
      },
    );
    if res.is_err() {
      break;
    }
    if !wq.is_empty() {
      stream.write_all(&wq).await?;
      wq.clear();
    }
    if engine.is_closed() {
      break;
    }
  }
  Ok(())
}

async fn handle_client(
  fut: upgrade::UpgradeFut,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let upgraded = fut.upgraded().await?;
  match upgraded.downcast::<TokioIo<TcpStream>>() {
    Ok(parts) => {
      let stream = parts.io.into_inner();
      // hyper may have already buffered a few bytes from the client; in
      // the bench's ping-pong flow the first WebSocket frame doesn't
      // arrive until after the upgrade response, so this is normally
      // empty.
      if !parts.read_buf.is_empty() {
        // For the rare prefix case, feed those bytes to a one-shot
        // engine call. Simpler than threading a prefix buffer through
        // the loop.
        let mut engine = ServerEngine::new();
        let mut scratch = parts.read_buf.to_vec();
        let mut wq = Vec::new();
        let _ = engine.process(
          &mut scratch,
          |b| wq.extend_from_slice(b),
          |_, op| match op {
            OpCode::Text | OpCode::Binary => ServerResponse::Echo,
            _ => ServerResponse::Discard,
          },
        );
        if !wq.is_empty() {
          let mut stream = stream;
          stream.write_all(&wq).await?;
          echo_loop(stream).await?;
        } else {
          echo_loop(stream).await?;
        }
      } else {
        echo_loop(stream).await?;
      }
    }
    Err(_) => return Err("TLS / non-TCP upgrade not supported here".into()),
  }
  Ok(())
}

async fn server_upgrade(
  mut req: Request<Incoming>,
) -> Result<Response<Empty<Bytes>>, Box<dyn std::error::Error + Send + Sync>> {
  let (response, fut) = upgrade::upgrade(&mut req)?;
  tokio::task::spawn(async move {
    if let Err(e) = tokio::task::unconstrained(handle_client(fut)).await {
      eprintln!("ws connection error: {}", e);
    }
  });
  Ok(response)
}

fn main() -> std::io::Result<()> {
  let rt = tokio::runtime::Builder::new_current_thread()
    .enable_io()
    .build()?;
  let addr =
    std::env::var("FWS_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
  rt.block_on(async move {
    let listener = TcpListener::bind(&addr).await?;
    eprintln!("tokio-fast echo listening on {}", addr);
    loop {
      let (stream, _) = listener.accept().await?;
      let _ = stream.set_nodelay(true);
      tokio::spawn(async move {
        let io = TokioIo::new(stream);
        let conn = http1::Builder::new()
          .serve_connection(io, service_fn(server_upgrade))
          .with_upgrades();
        if let Err(e) = conn.await {
          eprintln!("hyper conn error: {:?}", e);
        }
      });
    }
  })
}
