//! Echo server example using io_uring
//!
//! This example demonstrates using fastwebsockets with io_uring for potentially improved performance.
//! 
//! Run with: cargo run --example echo_server_uring --features io-uring

use fastwebsockets::{upgrade::upgrade, Frame, OpCode, Role, WebSocket, WebSocketError};
use http_body_util::Empty;
use hyper::{body::Bytes, server::conn::http1, service::service_fn, Request, Response};
use std::future::Future;
use std::pin::Pin;

async fn server_upgrade(
    mut req: Request<hyper::body::Incoming>,
) -> Result<Response<Empty<Bytes>>, WebSocketError> {
    let (response, fut) = upgrade(&mut req)?;

    tokio::spawn(async move {
        if let Err(e) = handle_client(fut).await {
            eprintln!("Error in websocket connection: {}", e);
        }
    });

    Ok(response)
}

async fn handle_client(fut: Pin<Box<dyn Future<Output = Result<WebSocket<hyper_util::rt::TokioIo<fastwebsockets::uring::TcpStream>>, WebSocketError>> + Send>>) -> Result<(), WebSocketError> {
    let ws = fut.await?;
    let mut ws = ws;
    ws.set_writev(false);
    ws.set_auto_close(true);
    ws.set_auto_pong(true);

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

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    fastwebsockets::uring::start(async {
        let listener = fastwebsockets::uring::TcpListener::bind("127.0.0.1:9001".parse().unwrap()).await?;
        println!("Server listening on http://127.0.0.1:9001");

        loop {
            let (stream, _) = listener.accept().await?;
            let io = hyper_util::rt::TokioIo::new(stream);

            tokio::spawn(async move {
                let conn = http1::Builder::new()
                    .serve_connection(io, service_fn(server_upgrade))
                    .with_upgrades();

                if let Err(e) = conn.await {
                    eprintln!("HTTP connection error: {}", e);
                }
            });
        }

        #[allow(unreachable_code)]
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    })
}