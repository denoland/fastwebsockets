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
//! inside `ServerEngine::process_into` — no `Future` state machine per
//! frame, no `BytesMut::split_to`, no per-frame Arc atomic, and no
//! adapter-side memcpy of the response payload thanks to the
//! zero-copy outbound-segment API: the engine writes the response
//! header into the same buffer the recv landed in, and reports the
//! result as a list of byte ranges within that buffer. The adapter
//! then drives `write_vectored` directly from the recv buffer.
//!
//! Per-frame loop:
//!
//! ```text
//!   loop {
//!     n = stream.read(scratch).await?;                  // 1 async await
//!     engine.process_into(&mut scratch[..n], handler)?; // sync
//!     stream.write_all_vectored(&iovs).await?;          // 1 async await
//!     engine.clear_outbound();
//!   }
//! ```

use std::io::IoSlice;

use fastwebsockets::OpCode;
use fastwebsockets::OutboundSegment;
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
  loop {
    let n = stream.read(&mut scratch).await?;
    if n == 0 {
      break;
    }
    let res =
      engine.process_into(&mut scratch[..n], |_payload, opcode| match opcode {
        OpCode::Text | OpCode::Binary => ServerResponse::Echo,
        _ => ServerResponse::Discard,
      });
    if res.is_err() {
      break;
    }
    write_outbound(&mut stream, &engine, &scratch).await?;
    engine.clear_outbound();
    if engine.is_closed() {
      break;
    }
  }
  Ok(())
}

/// Build IoSlices from the engine's outbound segments and ship them
/// through `write_vectored`. `Input` segments slice `scratch` directly
/// (zero-copy); `Local` segments slice the engine's small header
/// scratch.
async fn write_outbound(
  stream: &mut TcpStream,
  engine: &ServerEngine,
  scratch: &[u8],
) -> std::io::Result<()> {
  let segs = engine.outbound_segments();
  if segs.is_empty() {
    return Ok(());
  }
  let local = engine.outbound_local();

  // We don't know how many iovecs we'll need; the bench's load_test
  // delivers one frame per recv so usually just 1, occasionally 2.
  // Build them on the stack with a small array; spill to a Vec only
  // if there are more than `STACK_IOVS` segments in this batch.
  const STACK_IOVS: usize = 16;
  let mut stack: [std::mem::MaybeUninit<IoSlice<'_>>; STACK_IOVS] =
    [const { std::mem::MaybeUninit::uninit() }; STACK_IOVS];
  let mut spill: Vec<IoSlice<'_>>;
  let iovs: &[IoSlice<'_>] = if segs.len() <= STACK_IOVS {
    for (i, seg) in segs.iter().enumerate() {
      let slice = match seg {
        OutboundSegment::Input { start, len } => {
          &scratch[*start as usize..*start as usize + *len as usize]
        }
        OutboundSegment::Local { start, len } => {
          &local[*start as usize..*start as usize + *len as usize]
        }
      };
      stack[i].write(IoSlice::new(slice));
    }
    // SAFETY: we just initialized stack[0..segs.len()].
    unsafe {
      std::slice::from_raw_parts(
        stack.as_ptr() as *const IoSlice<'_>,
        segs.len(),
      )
    }
  } else {
    spill = Vec::with_capacity(segs.len());
    for seg in segs {
      let slice = match seg {
        OutboundSegment::Input { start, len } => {
          &scratch[*start as usize..*start as usize + *len as usize]
        }
        OutboundSegment::Local { start, len } => {
          &local[*start as usize..*start as usize + *len as usize]
        }
      };
      spill.push(IoSlice::new(slice));
    }
    &spill
  };

  // Drain the iovs via repeated write_vectored. Each call may write
  // fewer bytes than total; we re-slice and try again.
  let mut total: usize = iovs.iter().map(|s| s.len()).sum();
  let mut head = 0usize;
  while total > 0 {
    let n = stream.write_vectored(&iovs[head..]).await?;
    if n == 0 {
      return Err(std::io::ErrorKind::WriteZero.into());
    }
    total = total.saturating_sub(n);
    if total == 0 {
      break;
    }
    // Advance past fully-consumed iovecs.
    let mut consumed = n;
    while head < iovs.len() && consumed >= iovs[head].len() {
      consumed -= iovs[head].len();
      head += 1;
    }
    if head < iovs.len() && consumed > 0 {
      // Partial iovec: fall back to write_all for the remainder.
      stream.write_all(&iovs[head][consumed..]).await?;
      total = total.saturating_sub(iovs[head].len() - consumed);
      head += 1;
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
      let mut stream = parts.io.into_inner();
      // hyper occasionally has a tiny tail of bytes (post-handshake
      // request bytes the client pipelined). Feed them to the engine
      // before entering the steady-state loop.
      if !parts.read_buf.is_empty() {
        let mut engine = ServerEngine::new();
        let mut prefix = parts.read_buf.to_vec();
        let _ = engine.process_into(&mut prefix, |_, op| match op {
          OpCode::Text | OpCode::Binary => ServerResponse::Echo,
          _ => ServerResponse::Discard,
        });
        write_outbound(&mut stream, &engine, &prefix).await?;
        engine.clear_outbound();
      }
      echo_loop(stream).await?;
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
