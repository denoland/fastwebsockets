#![no_main]

use libfuzzer_sys::fuzz_target;

use std::cell::RefCell;
use std::rc::Rc;

#[derive(Debug)]
struct ArbitraryByteStream {
  data: Vec<u8>,
  read_all: Rc<RefCell<Option<tokio::sync::oneshot::Sender<()>>>>,
}

impl ArbitraryByteStream {
  fn new(data: Vec<u8>, read_all: tokio::sync::oneshot::Sender<()>) -> Self {
    Self {
      data,
      read_all: Rc::new(RefCell::new(Some(read_all))),
    }
  }
}

impl tokio::io::AsyncRead for ArbitraryByteStream {
  fn poll_read(
    self: std::pin::Pin<&mut Self>,
    _cx: &mut std::task::Context<'_>,
    buf: &mut tokio::io::ReadBuf<'_>,
  ) -> std::task::Poll<std::io::Result<()>> {
    let this = self.get_mut();
    let len = std::cmp::min(buf.remaining(), this.data.len());
    let data = this.data.drain(..len).collect::<Vec<_>>();
    buf.put_slice(&data);

    if this.data.is_empty() {
      if let Some(tx) = this.read_all.borrow_mut().take() {
        let _ = tx.send(()).unwrap();
      }
      return std::task::Poll::Pending;
    }

    std::task::Poll::Ready(Ok(()))
  }
}

impl tokio::io::AsyncWrite for ArbitraryByteStream {
  fn poll_write(
    self: std::pin::Pin<&mut Self>,
    _cx: &mut std::task::Context<'_>,
    buf: &[u8],
  ) -> std::task::Poll<std::io::Result<usize>> {
    std::task::Poll::Ready(Ok(buf.len()))
  }

  fn poll_flush(
    self: std::pin::Pin<&mut Self>,
    _cx: &mut std::task::Context<'_>,
  ) -> std::task::Poll<std::io::Result<()>> {
    std::task::Poll::Ready(Ok(()))
  }

  fn poll_shutdown(
    self: std::pin::Pin<&mut Self>,
    _cx: &mut std::task::Context<'_>,
  ) -> std::task::Poll<std::io::Result<()>> {
    std::task::Poll::Ready(Ok(()))
  }
}

fuzz_target!(|data: &[u8]| {
  let (tx, rx) = tokio::sync::oneshot::channel();
  let mut stream = ArbitraryByteStream::new(data.to_vec(), tx);

  let mut ws = sockdeez::WebSocket::after_handshake(stream);
  ws.set_writev(false);
  ws.set_auto_close(true);
  ws.set_auto_pong(true);

  futures::executor::block_on(async move {
    tokio::select! {
        // We've read all the data, so we're done
        _ = rx => {}
        // We've read a frame, so we're done
        _ = ws.read_frame() => {}
    }
  });
});
