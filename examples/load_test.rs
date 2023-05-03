use std::cell::UnsafeCell;
use std::future::Future;

use hyper::header::CONNECTION;
use hyper::header::UPGRADE;
use hyper::upgrade::Upgraded;
use hyper::Body;
use hyper::Request;
use std::cell::RefCell;
use std::rc::Rc;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

type Result<T> =
  std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct SpawnExecutor;

impl<Fut> hyper::rt::Executor<Fut> for SpawnExecutor
where
  Fut: Future + Send + 'static,
  Fut::Output: Send + 'static,
{
  fn execute(&self, fut: Fut) {
    tokio::task::spawn(fut);
  }
}

async fn connect(port: u16) -> Result<Upgraded> {
  let addr = format!("localhost:{}", port);
  let stream = TcpStream::connect(&addr).await?;

  let req = Request::builder()
    .method("GET")
    .uri(format!("http://{}/", addr))
    .header("Host", &addr)
    .header(UPGRADE, "websocket")
    .header(CONNECTION, "upgrade")
    .header(
      "Sec-WebSocket-Key",
      fastwebsockets::handshake::generate_key(),
    )
    .header("Sec-WebSocket-Version", "13")
    .body(Body::empty())?;

  let (ws, _) =
    fastwebsockets::handshake::client(&SpawnExecutor, req, stream).await?;
  Ok(ws.into_inner())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
  // ./load_test <connections> <port> [optioan: duration(s)] [optional: output file]
  let args: Vec<String> = std::env::args().collect();
  if args.len() < 3 {
    println!(
      "Usage: ./load_test <connections> <port> [duration(s)] [output file]"
    );
    std::process::exit(1);
  }

  let connections = args[1].parse::<usize>().unwrap();
  let port = args[2].parse::<u16>().unwrap();
  let duration = args.get(3).map(|s| s.parse::<u64>().unwrap());
  let mut output_file = args.get(4).map(|s| std::fs::File::create(s).unwrap());

  let wses = Rc::new(RefCell::new(
    futures::future::join_all((0..connections).map(|_| connect(port)))
      .await
      .into_iter()
      .collect::<Result<Vec<_>>>()?,
  ));

  println!("Running benchmark now...");

  let count = UnsafeCell::new(0);
  let count_ptr = count.get();

  let payload: [u8; 20] = rand::random();

  let wses = Rc::clone(&wses);

  let mut bench_fut = Box::pin(async move {
    let mut wses = wses.borrow_mut();
    futures::future::join_all(wses.iter_mut().map(|ws| async move {
      let mut frame = vec![130, 128 | 20, 1, 2, 3, 4];
      frame.extend_from_slice(&payload);

      let mut payload = vec![0; frame.len() - 4];
      loop {
        ws.write_all(&frame).await.unwrap();
        let _ = ws.read_exact(&mut payload).await.unwrap();
        unsafe {
          *count_ptr += 1;
        }
      }
    }))
    .await;
  });

  let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
  interval.tick().await;

  let mut ticks = 0;
  'l: loop {
    tokio::select! {
      _ = interval.tick() => {
        ticks += 1;
        if let Some(duration) = duration {
          if ticks > duration {
            break 'l;
          }
        }
        println!("{} msg/second", unsafe { *count_ptr });
        if let Some(ref mut output_file) = output_file {
            use std::io::Write;
          writeln!(output_file, "{}", unsafe { *count_ptr }).unwrap();
        }
        unsafe {
          *count_ptr = 0;
        }
      }
      _ = &mut bench_fut => {}
    }
  }

  Ok(())
}
