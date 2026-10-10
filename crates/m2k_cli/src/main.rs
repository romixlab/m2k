//! `m2k`: drive an Analog Devices ADALM2000 (M2K) through its iiod daemon, on top of the `m2k` library.
//!
//! The command tree and handlers live in `cli.rs`; number parsing and formatting in `parse.rs`; shell
//! completion in `complete.rs`. The M2K's devices, conversions, calibration and typed captures are in the `m2k`
//! crate (`crates/m2k`).

mod cli;
mod complete;
mod parse;

fn main() {
    cli::main()
}
