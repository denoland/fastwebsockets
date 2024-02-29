use bytes::BufMut;
use futures_lite::ready;
use futures_lite::AsyncRead;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::{io, mem};

#[pin_project]
#[derive(Debug)]
#[must_use = "Futures do nothing unless polled"]
struct ReadBuf<'a, R: ?Sized, B: ?Sized> {
  #[pin]
  reader: &'a mut R,
  buf: &'a mut B,
}

pub(crate) fn read_buf<'a, R, B>(
  reader: &'a mut R,
  buf: &'a mut B,
) -> ReadBuf<'a, R, B>
where
  R: AsyncRead + Unpin + ?Sized,
  B: BufMut + ?Sized,
{
  ReadBuf { reader, buf }
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
      ready!(this.reader.poll_read(cx, buf.initialize_unfilled()))?
    };
    unsafe {
      this.buf.advance_mut(n);
    }
    Poll::Ready(Ok(n))
  }
}
