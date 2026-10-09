//! `m2k`: drive an Analog Devices ADALM2000 (M2K) through its iiod daemon, with the pure-Rust `iiod_client`.
//!
//! The command tree and handlers live in `cli.rs`, the M2K's devices and attributes in `m2k.rs`, raw-to-volts
//! conversions in `conv.rs` and calibration in `calib.rs`.

// two channels with parallel arrays everywhere: index loops read better than zips
#![allow(clippy::needless_range_loop)]

mod calib;
mod cli;
mod complete;
mod conv;
mod m2k;
mod parse;
mod stats;

fn main() {
    cli::main()
}
