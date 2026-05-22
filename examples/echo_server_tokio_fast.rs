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
//! framing. The "Deno-friendly" fast path: I/O stays async (so it can
//! be embedded in a larger tokio app), but the per-frame parse / unmask
//! / response synthesis runs synchronously inside
//! `ServerEngine::process_into`. There is no `Future` state machine per
//! frame, no `BytesMut::split_to`, no per-frame Arc atomic, and no
//! memcpy of the response payload thanks to the zero-copy outbound-
//! segment API.
//!
//! Per-frame loop:
//!
//! ```text
//!   loop {
//!     n = stream.read(scratch).await?;                  // 1 async await
//!     engine.process_into(&mut scratch[..n], handler)?; // sync
//!     write_outbound(&stream, ...);                     // mostly syscalls
//!     engine.clear_outbound();
//!   }
//! ```
//!
//! The write side uses `try_write` / `try_write_vectored` and only
//! awaits `writable()` if the kernel send buffer is full. On loopback
//! / small frames this means zero per-frame write futures: one
//! `read().await` plus a direct `send()` syscall. The single-segment
//! short-circuit avoids `writev` (which is ~15% more expensive than
//! `send` per syscall under loopback strace) for the common case where
//! the engine produced one in-place response.

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
use tokio::net::TcpListener;
use tokio::net::TcpStream;

use fastwebsockets::upgrade;

const SCRATCH_LEN: usize = 64 * 1024;

async fn echo_loop(mut stream: TcpStream) -> std::io::Result<()> {
  let _ = stream.set_nodelay(true);
  let mut engine = ServerEngine::new();
  let mut scratch = vec![0u8; SCRATCH_LEN];
  loop {
    // 1 async await per round trip: drive the I/O driver here, then do
    // the rest with raw try_* syscalls that don't construct a per-call
    // Future. Using `read().await` (not `readable().await; try_read`)
    // because read() correctly clears tokio's internal readiness flag
    // on WouldBlock, whereas mixing readable() + try_read in a tight
    // loop relies on try_read's internal flag bookkeeping and was the
    // root cause of the v3 regression — the WouldBlock branch was
    // allocating one readable() future per miss, ~1k times per second
    // at 200 connections.
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
    write_outbound(&stream, &engine, &scratch).await?;
    engine.clear_outbound();
    if engine.is_closed() {
      break;
    }
  }
  Ok(())
}

/// Build IoSlices from the engine's outbound segments and ship them
/// to the wire. The hot path — one in-place echo segment — short-
/// circuits to `try_write` (a direct `send()` syscall, no future
/// state machine, no `writev` setup). The multi-segment fallback
/// uses `try_write_vectored`. `writable().await` is only entered when
/// the kernel send buffer is actually full.
async fn write_outbound(
  stream: &TcpStream,
  engine: &ServerEngine,
  scratch: &[u8],
) -> std::io::Result<()> {
  let segs = engine.outbound_segments();
  if segs.is_empty() {
    return Ok(());
  }
  let local = engine.outbound_local();

  // Hot path: a single in-place Input segment. Drive it with `send()`
  // — under strace this is 13 µs/call vs writev's 15 µs/call, and
  // unlike `AsyncWriteExt::write_all` it does not allocate / poll a
  // per-call Future when the kernel accepts the bytes immediately,
  // which is the steady-state case on loopback.
  if segs.len() == 1 {
    let slice = match segs[0] {
      OutboundSegment::Input { start, len } => {
        &scratch[start as usize..start as usize + len as usize]
      }
      OutboundSegment::Local { start, len } => {
        &local[start as usize..start as usize + len as usize]
      }
    };
    let mut bytes = slice;
    while !bytes.is_empty() {
      match stream.try_write(bytes) {
        Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
        Ok(n) => bytes = &bytes[n..],
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
          stream.writable().await?;
        }
        Err(e) => return Err(e),
      }
    }
    return Ok(());
  }

  // Multi-segment path: build iovecs on the stack (segs.len() is
  // bounded by frames-per-recv, which is 1–2 on the bench).
  const STACK_IOVS: usize = 8;
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

  // Drain via try_write_vectored, fall back to try_write for any
  // residual partial iovec.
  let mut head = 0usize;
  let mut consumed_in_head = 0usize;
  let mut total: usize = iovs.iter().map(|s| s.len()).sum();
  while total > 0 {
    let n = if consumed_in_head == 0 {
      match stream.try_write_vectored(&iovs[head..]) {
        Ok(n) => n,
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
          stream.writable().await?;
          continue;
        }
        Err(e) => return Err(e),
      }
    } else {
      match stream.try_write(&iovs[head][consumed_in_head..]) {
        Ok(n) => n,
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
          stream.writable().await?;
          continue;
        }
        Err(e) => return Err(e),
      }
    };
    if n == 0 {
      return Err(std::io::ErrorKind::WriteZero.into());
    }
    total -= n;
    if consumed_in_head > 0 {
      let remaining_in_head = iovs[head].len() - consumed_in_head;
      if n >= remaining_in_head {
        head += 1;
        consumed_in_head = 0;
        let mut left = n - remaining_in_head;
        while head < iovs.len() && left >= iovs[head].len() {
          left -= iovs[head].len();
          head += 1;
        }
        if head < iovs.len() {
          consumed_in_head = left;
        }
      } else {
        consumed_in_head += n;
      }
    } else {
      let mut left = n;
      while head < iovs.len() && left >= iovs[head].len() {
        left -= iovs[head].len();
        head += 1;
      }
      if head < iovs.len() {
        consumed_in_head = left;
      }
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
      if !parts.read_buf.is_empty() {
        // Tiny request-pipeline tail from hyper. Feed it through the
        // engine before entering the steady-state loop.
        let mut engine = ServerEngine::new();
        let mut prefix = parts.read_buf.to_vec();
        let _ = engine.process_into(&mut prefix, |_, op| match op {
          OpCode::Text | OpCode::Binary => ServerResponse::Echo,
          _ => ServerResponse::Discard,
        });
        write_outbound(&stream, &engine, &prefix).await?;
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
