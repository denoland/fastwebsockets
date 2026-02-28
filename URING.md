# io_uring Integration for fastwebsockets

This document describes the io_uring integration added to fastwebsockets for improved performance on Linux systems.

## Overview

The io_uring integration provides:
- **Feature flag**: `io-uring` for conditional compilation
- **Type compatibility**: Drop-in replacement for `tokio::net` types
- **Runtime integration**: Uses `tokio_uring::start()` when enabled
- **Fallback support**: Automatically falls back to standard tokio when disabled

## Usage

### Enable io_uring support

Add the feature to your `Cargo.toml`:

```toml
[dependencies]
fastwebsockets = { version = "0.10", features = ["io-uring"] }
```

### Basic usage

```rust
// This code works with both tokio and io_uring backends
use fastwebsockets::{WebSocket, Role, Frame, OpCode, uring};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    uring::start(async {
        let listener = uring::TcpListener::bind("127.0.0.1:8080".parse().unwrap())?;
        
        loop {
            let (stream, _) = listener.accept().await?;
            let mut ws = WebSocket::after_handshake(stream, Role::Server);
            
            // All fastwebsockets APIs work unchanged
            let frame = ws.read_frame().await?;
            if frame.opcode == OpCode::Text {
                ws.write_frame(frame).await?; // Echo
            }
        }
    })
}
```

### Conditional compilation

The integration uses conditional compilation to provide optimal performance:

- **With `io-uring` feature**: Uses `tokio_uring::net::TcpStream/TcpListener` + `tokio_uring::start()`
- **Without feature**: Uses `tokio::net::TcpStream/TcpListener` + standard tokio runtime

## Implementation Details

### Module structure

- `fastwebsockets::uring`: Main module for io_uring integration
- `fastwebsockets::uring::net`: Networking types (TcpStream, TcpListener)
- `fastwebsockets::uring::start()`: Runtime startup function

### Type compatibility

The wrapper types implement `AsyncRead` and `AsyncWrite` to maintain compatibility with existing fastwebsockets APIs:

```rust
pub struct TcpStream {
    inner: tokio_uring::net::TcpStream,
}

impl AsyncRead for TcpStream { /* ... */ }
impl AsyncWrite for TcpStream { /* ... */ }
```

### Performance characteristics

- **Benefits**: Zero-copy I/O, reduced syscalls, better CPU efficiency
- **Requirements**: Linux kernel 5.11+, x86_64 or aarch64
- **Limitations**: AsyncRead/AsyncWrite adapter has some overhead

## Current Status

✅ **Working**:
- Feature flag compilation
- Type system integration  
- Runtime selection
- API compatibility
- Basic functionality

⚠️ **Known limitations**:
- AsyncRead/AsyncWrite adapter has polling limitations
- Some advanced tokio-uring features not exposed
- Testing coverage incomplete

🔄 **Future improvements**:
- Native io_uring buffer integration
- Performance benchmarks
- Advanced io_uring features
- Comprehensive testing

## Performance

Initial performance baseline (standard tokio):
```
tokio 100 messages: 6.4ms
tokio 1000 messages: 70.7ms
```

The io_uring integration is designed to improve these numbers, especially for:
- High connection counts
- Large message throughput
- CPU-bound scenarios

## Examples

See the `examples/` directory:
- `working_demo.rs`: Basic integration demo
- `bench_comparison.rs`: Performance comparison framework
- `simple_uring_test.rs`: WebSocket functionality test

## Building and Testing

```bash
# Test with io_uring
cargo test --features io-uring

# Test fallback
cargo test

# Run examples
cargo run --example working_demo --features io-uring
cargo run --example working_demo  # fallback

# Benchmark (tokio baseline)
cargo run --example bench_comparison
```

## Requirements

- **OS**: Linux with io_uring support
- **Kernel**: 5.11+ recommended (5.4 not supported)
- **Architecture**: x86_64 or aarch64
- **Runtime**: `tokio_uring::start()` instead of standard tokio runtime

## License

Same as fastwebsockets - Apache 2.0