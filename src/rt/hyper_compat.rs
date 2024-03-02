use core::slice;
use pin_project::pin_project;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

#[pin_project]
pub struct FuturesIo<T> {
  #[pin]
  inner: T,
}

impl<T> FuturesIo<T> {
  pub fn new(inner: T) -> Self {
    Self { inner }
  }

  pub fn inner(&self) -> &T {
    &self.inner
  }

  pub fn get_pin_mut(self: Pin<&mut Self>) -> Pin<&mut T> {
    self.project().inner
  }
}

impl<T> hyper::rt::Write for FuturesIo<T>
where
  T: futures_lite::AsyncWrite,
{
  fn poll_write(
    self: Pin<&mut Self>,
    cx: &mut Context<'_>,
    buf: &[u8],
  ) -> Poll<Result<usize, io::Error>> {
    self.get_pin_mut().poll_write(cx, buf)
  }

  fn poll_flush(
    self: Pin<&mut Self>,
    cx: &mut Context<'_>,
  ) -> Poll<Result<(), io::Error>> {
    self.get_pin_mut().poll_flush(cx)
  }

  fn poll_shutdown(
    self: Pin<&mut Self>,
    cx: &mut Context<'_>,
  ) -> Poll<Result<(), io::Error>> {
    self.get_pin_mut().poll_close(cx)
  }
}

impl<T> hyper::rt::Read for FuturesIo<T>
where
  T: futures_lite::AsyncRead,
{
  fn poll_read(
    self: Pin<&mut Self>,
    cx: &mut Context<'_>,
    mut buf: hyper::rt::ReadBufCursor<'_>,
  ) -> Poll<Result<(), io::Error>> {
    let read_slice = unsafe {
      let buffer = buf.as_mut();
      buffer.as_mut_ptr().write_bytes(0, buffer.len());
      slice::from_raw_parts_mut(buffer.as_mut_ptr() as *mut u8, buffer.len())
    };
    self.get_pin_mut().poll_read(cx, read_slice).map(|n| {
      if let Ok(n) = n {
        unsafe { buf.advance(n) };
      }
      Ok(())
    })
  }
}

impl<T> futures_lite::AsyncRead for FuturesIo<T>
where
  T: hyper::rt::Read,
{
  fn poll_read(
    self: Pin<&mut Self>,
    cx: &mut Context<'_>,
    buf: &mut [u8],
  ) -> Poll<io::Result<usize>> {
    let mut buffer = hyper::rt::ReadBuf::new(buf);
    let filled = match hyper::rt::Read::poll_read(
      self.get_pin_mut(),
      cx,
      buffer.unfilled(),
    ) {
      Poll::Ready(Ok(())) => buffer.filled().len(),
      Poll::Pending => return Poll::Pending,
      Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
    };
    Poll::Ready(Ok(filled))
  }
}

impl<T> futures_lite::AsyncWrite for FuturesIo<T>
where
  T: hyper::rt::Write,
{
  fn poll_write(
    self: Pin<&mut Self>,
    cx: &mut Context<'_>,
    buf: &[u8],
  ) -> Poll<io::Result<usize>> {
    hyper::rt::Write::poll_write(self.get_pin_mut(), cx, buf)
  }

  fn poll_flush(
    self: Pin<&mut Self>,
    cx: &mut Context<'_>,
  ) -> Poll<io::Result<()>> {
    hyper::rt::Write::poll_flush(self.get_pin_mut(), cx)
  }

  fn poll_close(
    self: Pin<&mut Self>,
    cx: &mut Context<'_>,
  ) -> Poll<io::Result<()>> {
    hyper::rt::Write::poll_shutdown(self.get_pin_mut(), cx)
  }
}
