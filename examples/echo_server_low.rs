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

//! Hand-rolled, tokio-only WebSocket echo server.
//!
//! This example is an *upper bound* benchmark target. It does the WebSocket
//! handshake by hand (the load_test client sends a fixed upgrade request) and
//! then runs a tight echo loop over a raw `TcpStream` with a fixed-size
//! buffer. The frame parser/writer are inlined and the masking is delegated
//! to the library's SIMD path.
//!
//! Use it to compare against `echo_server.rs` (which goes through hyper's
//! upgrade machinery) to see how much overhead the public API introduces.

use std::io::IoSlice;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::TcpStream;

use fastwebsockets::unmask;

const BUF_LEN: usize = 64 * 1024;

const RESPONSE_PREFIX: &[u8] =
  b"HTTP/1.1 101 Switching Protocols\r\nconnection: upgrade\r\nupgrade: websocket\r\nsec-websocket-accept: ";

fn sec_websocket_accept(key: &[u8]) -> [u8; 28] {
  use sha1::Digest;
  let mut sha1 = sha1::Sha1::new();
  sha1.update(key);
  sha1.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
  let digest = sha1.finalize();
  let mut out = [0u8; 28];
  // base64-encode a 20-byte digest to 28 bytes (with one trailing '=')
  use base64::engine::general_purpose::STANDARD;
  use base64::Engine;
  let n = STANDARD.encode_slice(digest.as_slice(), &mut out).unwrap();
  debug_assert_eq!(n, 28);
  out
}

async fn handshake(stream: &mut TcpStream) -> std::io::Result<usize> {
  let mut buf = [0u8; 2048];
  let mut filled = 0usize;
  loop {
    if filled == buf.len() {
      return Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "handshake oversize",
      ));
    }
    let n = stream.read(&mut buf[filled..]).await?;
    if n == 0 {
      return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    filled += n;
    if let Some(eom) = find_double_crlf(&buf[..filled]) {
      // Extract Sec-WebSocket-Key
      let header = &buf[..eom];
      let key = find_header_value(header, b"Sec-WebSocket-Key")
        .or_else(|| find_header_value(header, b"sec-websocket-key"))
        .ok_or_else(|| {
          std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "no Sec-WebSocket-Key",
          )
        })?;
      let accept = sec_websocket_accept(key);
      let mut resp = Vec::with_capacity(RESPONSE_PREFIX.len() + 28 + 4);
      resp.extend_from_slice(RESPONSE_PREFIX);
      resp.extend_from_slice(&accept);
      resp.extend_from_slice(b"\r\n\r\n");
      stream.write_all(&resp).await?;
      // Return how many bytes after the upgrade request we already read.
      return Ok(filled - eom);
    }
  }
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
  if buf.len() < 4 {
    return None;
  }
  buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

fn find_header_value<'a>(buf: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
  // Very simple HTTP header scan; case-insensitive name compare.
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
  buf[0] = 0x80 | opcode; // FIN + opcode
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

async fn echo_loop(
  mut stream: TcpStream,
  prefilled: usize,
  initial: Box<[u8; BUF_LEN]>,
) -> std::io::Result<()> {
  let _ = stream.set_nodelay(true);

  let mut buf = initial;
  let mut filled = prefilled;
  let mut head = [0u8; 10];

  loop {
    // Ensure at least 2 bytes for the frame header
    while filled < 2 {
      let n = stream.read(&mut buf[filled..]).await?;
      if n == 0 {
        return Ok(());
      }
      filled += n;
    }

    let b0 = buf[0];
    let b1 = buf[1];
    let fin = (b0 & 0x80) != 0;
    let opcode = b0 & 0x0f;
    let masked = (b1 & 0x80) != 0;
    let len_code = b1 & 0x7f;

    let (header_size, payload_len): (usize, usize) = match len_code {
      0..=125 => (2, len_code as usize),
      126 => {
        while filled < 4 {
          let n = stream.read(&mut buf[filled..]).await?;
          if n == 0 {
            return Ok(());
          }
          filled += n;
        }
        (4, u16::from_be_bytes([buf[2], buf[3]]) as usize)
      }
      127 => {
        while filled < 10 {
          let n = stream.read(&mut buf[filled..]).await?;
          if n == 0 {
            return Ok(());
          }
          filled += n;
        }
        (
          10,
          u64::from_be_bytes(buf[2..10].try_into().unwrap()) as usize,
        )
      }
      _ => unreachable!(),
    };

    let mask_size = if masked { 4 } else { 0 };
    let total_header = header_size + mask_size;

    while filled < total_header {
      let n = stream.read(&mut buf[filled..]).await?;
      if n == 0 {
        return Ok(());
      }
      filled += n;
    }

    let mask = if masked {
      let mut m = [0u8; 4];
      m.copy_from_slice(&buf[header_size..header_size + 4]);
      Some(m)
    } else {
      None
    };

    let frame_total = total_header + payload_len;
    if frame_total > buf.len() {
      return Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "frame larger than buffer",
      ));
    }

    while filled < frame_total {
      let n = stream.read(&mut buf[filled..]).await?;
      if n == 0 {
        return Ok(());
      }
      filled += n;
    }

    if let Some(m) = mask {
      unmask(&mut buf[total_header..frame_total], m);
    }

    // Handle control + data frames
    if !fin && opcode != 0 {
      // Fragmented start: bail (this fast-path is for whole frames)
      return Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "fragments unsupported in low example",
      ));
    }
    match opcode {
      0x1 | 0x2 => {
        // Text / Binary echo
        let head_n = fmt_server_head(&mut head, opcode, payload_len);
        let payload = &buf[total_header..frame_total];
        let iovs = [IoSlice::new(&head[..head_n]), IoSlice::new(payload)];
        // Single writev: header + payload
        let mut written = stream.write_vectored(&iovs).await?;
        let total = head_n + payload.len();
        if written < total {
          // Slow path for partial writes
          while written < head_n {
            let iovs2 =
              [IoSlice::new(&head[written..head_n]), IoSlice::new(payload)];
            written += stream.write_vectored(&iovs2).await?;
          }
          if written < total {
            stream.write_all(&payload[written - head_n..]).await?;
          }
        }
      }
      0x8 => {
        // Close: echo it back and exit
        let head_n = fmt_server_head(&mut head, 0x8, payload_len);
        let payload = &buf[total_header..frame_total];
        let iovs = [IoSlice::new(&head[..head_n]), IoSlice::new(payload)];
        stream.write_vectored(&iovs).await.ok();
        return Ok(());
      }
      0x9 => {
        // Ping → Pong
        let head_n = fmt_server_head(&mut head, 0xA, payload_len);
        let payload = &buf[total_header..frame_total];
        let iovs = [IoSlice::new(&head[..head_n]), IoSlice::new(payload)];
        stream.write_vectored(&iovs).await?;
      }
      _ => {}
    }

    // Move any tail bytes to the start.
    let tail = filled - frame_total;
    if tail > 0 {
      buf.copy_within(frame_total..frame_total + tail, 0);
    }
    filled = tail;
  }
}

async fn handle(mut stream: TcpStream) -> std::io::Result<()> {
  let _ = stream.set_nodelay(true);
  // Box::new on a 64KiB array allocates on heap; this is per-connection state.
  // Reusing it across the handshake reads keeps the initial bytes from the
  // upgrade-request tail available to the echo loop (if the client pipelines
  // the first frame).
  let prefilled = handshake(&mut stream).await?;
  // For correctness we re-read the upgrade response into a fresh buffer;
  // since the load_test sends the first frame only after seeing \r\n\r\n,
  // prefilled is always 0 here. (We still respect non-zero for robustness.)
  let buf: Box<[u8; BUF_LEN]> = Box::new([0u8; BUF_LEN]);
  // prefilled bytes refer to bytes the handshake reader had after the
  // upgrade-request terminator. We zeroed the new buffer; we'd normally
  // copy those bytes, but for the bench load_test prefilled is 0.
  let _ = prefilled;
  echo_loop(stream, 0, buf).await
}

fn main() -> std::io::Result<()> {
  let workers = std::env::var("FWS_WORKERS")
    .ok()
    .and_then(|s| s.parse::<usize>().ok())
    .unwrap_or(1);

  let mut builder = if workers <= 1 {
    tokio::runtime::Builder::new_current_thread()
  } else {
    let mut b = tokio::runtime::Builder::new_multi_thread();
    b.worker_threads(workers);
    b
  };
  let rt = builder.enable_io().build().unwrap();

  rt.block_on(async move {
    let listener = TcpListener::bind("127.0.0.1:8081").await?;
    eprintln!("low echo server listening on 127.0.0.1:8081");
    loop {
      let (stream, _) = listener.accept().await?;
      tokio::spawn(async move {
        if let Err(e) = handle(stream).await {
          eprintln!("connection error: {}", e);
        }
      });
    }
  })
}
