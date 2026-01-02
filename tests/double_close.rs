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

// Test for double close bug
// https://github.com/denoland/fastwebsockets/pull/72
// https://github.com/denoland/deno/issues/21642
//
// The bug: Sending a Close frame twice results in two Close frames being
// sent over the wire, violating WebSocket protocol. The second close should
// be a no-op.

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use fastwebsockets::{Frame, Role, WebSocket};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// A mock stream that counts how many times write is called and how many bytes
/// are written, allowing us to verify that only one Close frame is sent.
struct MockStream {
  write_call_count: Arc<AtomicUsize>,
  bytes_written: Arc<AtomicUsize>,
}

impl MockStream {
  fn new() -> (Self, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let write_call_count = Arc::new(AtomicUsize::new(0));
    let bytes_written = Arc::new(AtomicUsize::new(0));
    (
      Self {
        write_call_count: write_call_count.clone(),
        bytes_written: bytes_written.clone(),
      },
      write_call_count,
      bytes_written,
    )
  }
}

impl AsyncRead for MockStream {
  fn poll_read(
    self: Pin<&mut Self>,
    _cx: &mut Context<'_>,
    _buf: &mut ReadBuf<'_>,
  ) -> Poll<std::io::Result<()>> {
    // Return pending - we don't need to read for this test
    Poll::Pending
  }
}

impl AsyncWrite for MockStream {
  fn poll_write(
    self: Pin<&mut Self>,
    _cx: &mut Context<'_>,
    buf: &[u8],
  ) -> Poll<std::io::Result<usize>> {
    self.write_call_count.fetch_add(1, Ordering::SeqCst);
    self.bytes_written.fetch_add(buf.len(), Ordering::SeqCst);
    Poll::Ready(Ok(buf.len()))
  }

  fn poll_flush(
    self: Pin<&mut Self>,
    _cx: &mut Context<'_>,
  ) -> Poll<std::io::Result<()>> {
    Poll::Ready(Ok(()))
  }

  fn poll_shutdown(
    self: Pin<&mut Self>,
    _cx: &mut Context<'_>,
  ) -> Poll<std::io::Result<()>> {
    Poll::Ready(Ok(()))
  }
}

#[tokio::test]
async fn test_double_close_sends_only_one_frame() {
  let (stream, write_count, bytes_written) = MockStream::new();

  let mut ws = WebSocket::after_handshake(stream, Role::Server);
  ws.set_writev(false);
  ws.set_auto_close(true);

  // Send first close frame - this should succeed and write data
  let result1 = ws.write_frame(Frame::close(1000, b"goodbye")).await;
  assert!(result1.is_ok(), "First close should succeed");

  let writes_after_first = write_count.load(Ordering::SeqCst);
  let bytes_after_first = bytes_written.load(Ordering::SeqCst);

  assert!(writes_after_first > 0, "First close should write data");
  assert!(bytes_after_first > 0, "First close should write bytes");

  // Send second close frame - this should be a no-op (not write any data)
  let result2 = ws.write_frame(Frame::close(1000, b"goodbye again")).await;
  assert!(result2.is_ok(), "Second close should succeed (as no-op)");

  let writes_after_second = write_count.load(Ordering::SeqCst);
  let bytes_after_second = bytes_written.load(Ordering::SeqCst);

  // THE BUG: Currently this fails because the second close DOES write data
  assert_eq!(
    writes_after_first,
    writes_after_second,
    "Second close should NOT write any data (bug: it wrote {} more bytes)",
    bytes_after_second - bytes_after_first
  );
}

#[tokio::test]
async fn test_is_closed_after_close_frame() {
  let (stream, _, _) = MockStream::new();

  let mut ws = WebSocket::after_handshake(stream, Role::Server);
  ws.set_writev(false);

  assert!(!ws.is_closed(), "Should not be closed initially");

  // Send close frame
  ws.write_frame(Frame::close(1000, b"")).await.unwrap();

  assert!(ws.is_closed(), "Should be closed after sending close frame");
}

#[tokio::test]
async fn test_non_close_frame_after_close_fails() {
  let (stream, _, _) = MockStream::new();

  let mut ws = WebSocket::after_handshake(stream, Role::Server);
  ws.set_writev(false);

  // Send close frame
  ws.write_frame(Frame::close(1000, b"")).await.unwrap();

  // Try to send a text frame after close - should fail
  let result = ws.write_frame(Frame::text(b"hello".to_vec().into())).await;

  assert!(
    result.is_err(),
    "Sending non-close frame after close should fail"
  );
  assert!(
    matches!(
      result,
      Err(fastwebsockets::WebSocketError::ConnectionClosed)
    ),
    "Should get ConnectionClosed error"
  );
}
