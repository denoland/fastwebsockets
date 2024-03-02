#[cfg(feature = "futures")]
use super::read_buf;
use bytes::BufMut;
use std::future::Future;
use std::io::{self, IoSlice};

// Read bytes from a source
pub trait Read {
  fn read_buf<'a, B: BufMut + ?Sized>(
    &'a mut self,
    buf: &'a mut B,
  ) -> impl Future<Output = io::Result<usize>> + '_;
}

pub trait Write {
  fn write_vectored<'a>(
    &'a mut self,
    bufs: &'a [IoSlice<'_>],
  ) -> impl Future<Output = io::Result<usize>> + '_;

  fn write_all<'a>(
    &'a mut self,
    src: &'a [u8],
  ) -> impl Future<Output = io::Result<()>> + '_;
}

#[cfg(not(feature = "futures"))]
impl<T: tokio::io::AsyncReadExt + Unpin> Read for T {
  fn read_buf<'a, B: BufMut + ?Sized>(
    &'a mut self,
    buf: &'a mut B,
  ) -> impl Future<Output = io::Result<usize>> + '_ {
    self.read_buf(buf)
  }
}

#[cfg(not(feature = "futures"))]
impl<T: tokio::io::AsyncWriteExt + Unpin> Write for T {
  fn write_all<'a>(
    &'a mut self,
    src: &'a [u8],
  ) -> impl Future<Output = io::Result<()>> + '_ {
    self.write_all(src)
  }

  fn write_vectored<'a>(
    &'a mut self,
    bufs: &'a [IoSlice<'_>],
  ) -> impl Future<Output = io::Result<usize>> + '_ {
    self.write_vectored(bufs)
  }
}

#[cfg(feature = "futures")]
impl<T: futures_lite::AsyncReadExt + Unpin> Read for T {
  fn read_buf<'a, B: BufMut + ?Sized>(
    &'a mut self,
    buf: &'a mut B,
  ) -> impl Future<Output = io::Result<usize>> + '_ {
    read_buf::read_buf(self, buf)
  }
}

#[cfg(feature = "futures")]
impl<T: futures_lite::AsyncWriteExt + Unpin> Write for T {
  fn write_all<'a>(
    &'a mut self,
    src: &'a [u8],
  ) -> impl Future<Output = io::Result<()>> + '_ {
    self.write_all(src)
  }

  fn write_vectored<'a>(
    &'a mut self,
    bufs: &'a [IoSlice<'_>],
  ) -> impl Future<Output = io::Result<usize>> + '_ {
    self.write_vectored(bufs)
  }
}
