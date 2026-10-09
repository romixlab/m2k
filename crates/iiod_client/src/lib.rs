//! A client for iiod, the network daemon of Linux Industrial I/O devices (ADALM2000, ADALM-Pluto, ...), in pure
//! Rust: no libiio, no C. It speaks iiod's text protocol over TCP (port 30431): the one libiio 0.x uses and
//! that libiio 1.x servers still answer.
//!
//! ```no_run
//! use iiod_client::{Attr, Client};
//! use std::time::Duration;
//!
//! let mut client = Client::connect(("192.168.2.1", iiod_client::DEFAULT_PORT), Duration::from_secs(3))?;
//! let ctx = client.context()?;
//! let adc = ctx.device("m2k-adc").ok_or("no m2k-adc")?;
//! let rate = client.read_attr(&adc.id, Attr::Device("sampling_frequency"))?;
//!
//! // 1000 samples of every channel that can be captured
//! let mask = adc.mask_of(&adc.channels);
//! let layout = adc.layout(&mask);
//! client.open(&adc.id, 1000, &mask, false)?;
//! let (_, data) = client.read_buf(&adc.id, 1000 * layout.sample_size, mask.len(), |_| {})?;
//! client.close(&adc.id)?;
//! let ch0 = layout.extract(0, &data);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Not here (yet): the binary protocol of libiio 1.x, USB (the iiod interface without networking), serial,
//! zstd-compressed context (`ZPRINT`), DNS-SD discovery, and triggers (`GETTRIG`/`SETTRIG`).

mod client;
mod context;

pub use client::{Attr, Client, DEFAULT_PORT};
pub use context::{Channel, Context, DataFormat, Device, Field, Layout, ScanElement};

/// What can go wrong talking to an iiod server.
#[derive(Debug)]
pub enum Error {
    /// The connection failed or timed out.
    Io(std::io::Error),
    /// The server answered something this client does not understand.
    Protocol(String),
    /// The server refused a command with an errno (`code` is positive: 2 for `-ENOENT`).
    Errno { code: i32, what: String },
    /// The context XML could not be read.
    Xml(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Protocol(msg) => write!(f, "iiod protocol: {msg}"),
            Error::Errno { code, what } => {
                let (name, text) = errno_name(*code);
                write!(f, "{what}: {text} ({name}, -{code})")
            }
            Error::Xml(msg) => write!(f, "context XML: {msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // `Io` already prints its cause: returning it here would print it twice in error chains
        None
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// Linux errno numbers iiod answers with (the device runs Linux whatever this client runs on).
fn errno_name(code: i32) -> (&'static str, &'static str) {
    match code {
        1 => ("EPERM", "operation not permitted"),
        2 => ("ENOENT", "no such device, channel or attribute"),
        5 => ("EIO", "I/O error"),
        9 => ("EBADF", "buffer is not open"),
        11 => ("EAGAIN", "try again"),
        12 => ("ENOMEM", "out of memory"),
        13 => ("EACCES", "permission denied"),
        16 => ("EBUSY", "device is busy"),
        19 => ("ENODEV", "no such device"),
        22 => ("EINVAL", "invalid argument"),
        32 => ("EPIPE", "broken pipe"),
        34 => ("ERANGE", "value out of range"),
        38 => ("ENOSYS", "not implemented"),
        110 => ("ETIMEDOUT", "timed out"),
        _ => ("errno", "error"),
    }
}
