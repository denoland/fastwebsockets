mod io;
pub use io::{Read, Write};

#[cfg(feature = "futures")]
mod read_buf;

#[cfg(all(feature = "upgrade", feature = "futures"))]
mod hyper_compat;

#[cfg(all(feature = "upgrade", feature = "futures"))]
pub use hyper_compat::FuturesIo;
