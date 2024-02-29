use bytes::BufMut;
use futures_lite::AsyncRead;
use pin_project::pin_project;
use std::future::Future;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::{io, iop, mem};

#[pin_project]
#[derive(Debug)]
#[must_use = "Futures do nothing unless polled"]
struct ReadBuf<'a, R: ?Sized, B: ?Sized> {
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
    let me = self.project();
    if !me.buf.has_remaining_mut() {
      return Poll::Ready(Ok(0));
    }
    let n = unsafe {
      let dst =
        &mut *(me.buf.chunk_mut() as *mut _ as *mut [mem::MaybeUninit<u8>]);
      let mut buf = tokio::io::ReadBuf::uninit(dst);
      let ptr = buf.filled().as_ptr();
      if AsyncRead::poll_read(Pin::new(me.reader), cx, &mut buf)?.is_pending() {
        return Poll::Pending;
      }

      assert_eq!(ptr, buf.filled().as_ptr());
      buf.filled().len()
    };
    unsafe {
      me.buf.advance_mut(n);
    }

    Poll::Ready(Ok(n))
  }
}
