#![no_main]

use libfuzzer_sys::fuzz_target;

use tokio::sync::oneshot::Sender;
use tokio::sync::oneshot;
use std::pin::Pin;
use std::task::{Context, Poll};

#[derive(Debug)]
struct ArbitraryByteStream {
  data: Vec<u8>,
  tx: Option<Sender<()>>,
}

impl ArbitraryByteStream {
  fn new(data: Vec<u8>, tx: Sender<()>) -> Self {
    Self {
      data,
      tx: Some(tx),
    }
  }
}

impl tokio::io::AsyncRead for ArbitraryByteStream {
  fn poll_read(
    self: Pin<&mut Self>,
    _cx: &mut Context<'_>,
    buf: &mut tokio::io::ReadBuf<'_>,
  ) -> Poll<std::io::Result<()>> {
    let this = self.get_mut();
    let len = std::cmp::min(buf.remaining(), this.data.len());
    let data = this.data.drain(..len).collect::<Vec<_>>();
    buf.put_slice(&data);

    if this.data.is_empty() {
      if let Some(tx) = this.tx.take() {
        let _ = tx.send(()).unwrap();
      }
    
      return Poll::Pending;
    }

    Poll::Ready(Ok(()))
  }
}

impl tokio::io::AsyncWrite for ArbitraryByteStream {
  fn poll_write(
    self: Pin<&mut Self>,
    _cx: &mut Context<'_>,
    buf: &[u8],
  ) -> Poll<std::io::Result<usize>> {
    Poll::Ready(Ok(buf.len()))
  }

  fn poll_flush(
    self: std::pin::Pin<&mut Self>,
    _cx: &mut Context<'_>,
  ) -> Poll<std::io::Result<()>> {
    Poll::Ready(Ok(()))
  }

  fn poll_shutdown(
    self: std::pin::Pin<&mut Self>,
    _cx: &mut Context<'_>,
  ) -> Poll<std::io::Result<()>> {
    Poll::Ready(Ok(()))
  }
}

fuzz_target!(|data: &[u8]| {
  let (tx, rx) = oneshot::channel();
  let stream = ArbitraryByteStream::new(data.to_vec(), tx);

  let mut ws = fastwebsockets::WebSocket::after_handshake(stream, fastwebsockets::Role::Server);
  ws.set_writev(false);
  ws.set_auto_close(true);
  ws.set_auto_pong(true);
  ws.set_max_message_size(u16::MAX as usize);

  futures::executor::block_on(async move {
    tokio::select! {
        // We've read all the data, so we're done
        _ = rx => {}
        // We've read a frame, so we're done
        _ = ws.read_frame() => {}
    }
  });
});
