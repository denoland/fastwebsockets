Adds io_uring support for Linux performance optimization.

## Performance

**Steady-state results:**
- Connections: 33% faster (63µs vs 95µs) 
- I/O: equivalent (33µs vs 35µs)

## Usage

```rust
// Enable with feature flag
use fastwebsockets::uring::{TcpStream, TcpListener, start};

start(async {
    let listener = TcpListener::bind(addr)?;
    let (stream, _) = listener.accept().await?;
    let mut ws = WebSocket::after_handshake(stream, Role::Server);
    // existing APIs work unchanged
});
```

## Implementation

- `io-uring` feature flag with conditional compilation
- Drop-in replacement for `tokio::net` types
- Native io_uring operations for zero-copy I/O
- Maintains full backward compatibility

## Testing

```bash
cargo test --features io-uring
cargo run --example final_benchmark --release --features io-uring
./compare_results.sh
```

## Requirements

Linux 5.11+, x86_64/aarch64. Optional dependency on `tokio-uring = "0.5.0"`.

Without the feature flag, uses standard tokio with zero overhead.