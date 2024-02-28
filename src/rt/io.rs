use bytes::BufMut;
use std::future::Future;
use std::io::Result;
use std::io::{self, IoSlice};
use std::pin::Pin;

// Read bytes from a source
pub trait Read {
  fn read_buf<B: BufMut + ?Sized>(
    &mut self,
    buf: &mut B,
  ) -> impl Future<Output = io::Result<usize>>;
}

pub trait Write {
  async fn write_vectored(&mut self, bufs: &[IoSlice<'_>])
    -> io::Result<usize>;
}

mod tokio {
  use super::Read;
  use bytes::BufMut;
  use std::future::Future;
  use std::{io, ops::Deref};
  struct TokioReader<T>(T);

  impl<T> Read for TokioReader<T>
  where
    T: tokio::io::AsyncReadExt + Unpin,
  {
    fn read_buf<B: BufMut + ?Sized>(
      &mut self,
      buf: &mut B,
    ) -> impl Future<Output = io::Result<usize>> {
    }
  }
}
