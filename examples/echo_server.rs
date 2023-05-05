// Copyright 2023 Divy Srivastava <dj.srivastava23@gmail.com>
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

use fastwebsockets::upgrade;
use fastwebsockets::OpCode;
use hyper::server::conn::Http;
use hyper::service::service_fn;
use hyper::Body;
use hyper::Request;
use hyper::Response;
use tokio::net::TcpListener;

use futures::ready;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::io::unix::AsyncFd;

pub struct AsyncTcpStream {
    inner: AsyncFd<TcpStream>,
}

impl AsyncTcpStream {
    pub fn new(tcp: TcpStream) -> io::Result<Self> {
        tcp.set_nonblocking(true)?;
        Ok(Self {
            inner: AsyncFd::with_interest(tcp, tokio::io::Interest::READABLE)?,
        })
    }
}

impl AsyncRead for AsyncTcpStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>
    ) -> Poll<io::Result<()>> {
        loop {
            let mut guard = ready!(self.inner.poll_read_ready(cx))?;

            let unfilled = buf.initialize_unfilled();
            match guard.try_io(|inner| inner.get_ref().read(unfilled)) {
                Ok(Ok(len)) => {
                    buf.advance(len);
                    return Poll::Ready(Ok(()));
                },
                Ok(Err(err)) => return Poll::Ready(Err(err)),
                Err(_would_block) => continue,
            }
        }
    }
}

impl AsyncWrite for AsyncTcpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8]
    ) -> Poll<io::Result<usize>> {
        match self.inner.get_ref().write(buf) {
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => panic!("unexpected WouldBlock"),
            other => return Poll::Ready(other),
        };

        loop {
            let mut guard = ready!(self.inner.poll_write_ready(cx))?;

            match guard.try_io(|inner| inner.get_ref().write(buf)) {
                Ok(result) => return Poll::Ready(result),
                Err(_would_block) => continue,
            }
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        // tcp flush is a no-op
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        self.inner.get_ref().shutdown(std::net::Shutdown::Write)?;
        Poll::Ready(Ok(()))
    }
}

async fn handle_client(
  fut: upgrade::UpgradeFut,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let mut ws = fastwebsockets::FragmentCollector::new(fut.await?);

  loop {
    let frame = ws.read_frame().await?;
    match frame.opcode {
      OpCode::Close => break,
      OpCode::Text | OpCode::Binary => {
        ws.write_frame(frame).await?;
      }
      _ => {}
    }
  }

  Ok(())
}
async fn server_upgrade(
  mut req: Request<Body>,
) -> Result<Response<Body>, Box<dyn std::error::Error + Send + Sync>> {
  let (response, fut) = upgrade::upgrade(&mut req)?;

  tokio::spawn(async move {
    if let Err(e) = handle_client(fut).await {
      eprintln!("Error in websocket connection: {}", e);
    }
  });

  Ok(response)
}

async fn serve() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  let listener = TcpListener::bind("127.0.0.1:8080").await?;
  println!("Server started, listening on {}", "127.0.0.1:8080");
  loop {
    let (stream, _) = listener.accept().await?;
    let std_stream = stream.into_std()?;
    let stream = AsyncTcpStream::new(std_stream)?;
    println!("Client connected");
    tokio::spawn(async move {
      let conn_fut = Http::new()
        .serve_connection(stream, service_fn(server_upgrade))
        .with_upgrades();
      if let Err(e) = conn_fut.await {
        println!("An error occurred: {:?}", e);
      }
    });
  }
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
  tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .build()
    .unwrap()
    .block_on(serve())
}
