use bytes::BufMut;
use futures_lite::ready;
use futures_lite::AsyncRead;
use pin_project::pin_project;
use std::future::Future;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::{io, mem};

#[pin_project]
#[derive(Debug)]
#[must_use = "Futures do nothing unless polled"]
pub(crate) struct ReadBuf<'a, R: ?Sized, B: ?Sized> {
  reader: &'a mut R,
  buf: &'a mut B,
  #[pin]
  _pin: PhantomPinned,
}

pub(crate) fn read_buf<'a, R, B>(
  reader: &'a mut R,
  buf: &'a mut B,
) -> ReadBuf<'a, R, B>
where
  R: AsyncRead + Unpin + ?Sized,
  B: BufMut + ?Sized,
{
  ReadBuf {
    reader,
    buf,
    _pin: PhantomPinned,
  }
}

impl<R, B> Future for ReadBuf<'_, R, B>
where
  R: AsyncRead + Unpin + ?Sized,
  B: BufMut + ?Sized,
{
  type Output = io::Result<usize>;
  fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.project();
    if !this.buf.has_remaining_mut() {
      return Poll::Ready(Ok(0));
    }
    let n = {
      let spare = unsafe {
        &mut *(this.buf.chunk_mut() as *mut _ as *mut [mem::MaybeUninit<u8>])
      };
      let mut buf = tokio::io::ReadBuf::uninit(spare);
      ready!(Pin::new(this.reader).poll_read(cx, buf.initialize_unfilled()))?
    };
    unsafe {
      this.buf.advance_mut(n);
    }
    Poll::Ready(Ok(n))
  }
}
