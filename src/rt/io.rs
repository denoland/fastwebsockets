use bytes::BufMut;
use std::future::Future;
use std::io::Result;
use std::io::{self, IoSlice};
use std::pin::Pin;

// Read bytes from a source
pub trait Read {
  fn read_buf<'a, B: BufMut + ?Sized>(
    &'a mut self,
    buf: &'a mut B,
  ) -> impl Future<Output = io::Result<usize>> + '_;
}

pub trait Write {
  async fn write_vectored(&mut self, bufs: &[IoSlice<'_>])
    -> io::Result<usize>;
}

// #[cfg(not(features = "futures"))]
// mod tokio {
//   use super::Read;
//   use bytes::BufMut;
//   use hyper_util::rt::tokio;
//   use std::future::Future;
//   use std::{io, ops::Deref};
//   use tokio::io::AsyncReadExt;
//
//   impl Read for tokio::io::AsyncReadExt {
//     async fn read_buf<B: BufMut + ?Sized>(
//       &mut self,
//       buf: &mut B,
//     ) -> io::Result<usize> {
//       AsyncReadExt
//     }
//   }
// }

#[cfg(not(feature = "futures"))]
impl<T: tokio::io::AsyncReadExt + Unpin> Read for T {
  fn read_buf<'a, B: BufMut + ?Sized>(
    &'a mut self,
    buf: &'a mut B,
  ) -> impl Future<Output = io::Result<usize>> + '_ {
    self.read_buf(buf)
  }
}

#[cfg(feature = "futures")]
impl<T: futures_lite::AsyncReadExt + Unpin> Read for T {
  fn read_buf<'a, B: BufMut + ?Sized>(
    &'a mut self,
    buf: &'a mut B,
  ) -> impl Future<Output = io::Result<usize>> + '_ {
    let dst = unsafe { &mut *(buf.chunk_mut() as *mut _ as *mut [u8]) };
    let n = self.read(dst);
  }
}
