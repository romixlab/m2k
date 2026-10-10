//! Typed Rust API for the Analog Devices ADALM2000 (M2K) on top of the pure-Rust `iiod_client`: scope, signal
//! generator, power supplies, voltmeter, logic analyzer, calibration. `m2k_cli` is the CLI on top of this,
//! `m2k_py` the Python module.
//!
//! ```no_run
//! # fn demo() -> m2k::Result<()> {
//! use std::time::Duration;
//!
//! let mut dev = m2k::M2k::connect(m2k::DEFAULT_URI, Duration::from_secs(5))?;
//! dev.require_m2k()?;
//! dev.init()?; // powers up the scope front end, as libm2k does when it opens an M2K
//! let cap = m2k::scope::capture(&mut dev, 10_000, 1e6, [m2k::conv::Range::V25; 2], |_| {})?;
//! println!("{:+.4} V", cap.stats[0].mean);
//! # Ok(()) }
//! ```
//!
//! The M2K's devices and attributes are in [`device`], raw-to-volts conversions with libm2k's constants in
//! [`conv`], self-calibration in [`cal`]; [`scope`], [`wave`] (behind `m2k gen`), [`logic`], [`psu`] and
//! [`dmm`] are the typed calls behind the CLI commands of the same name, [`stats`] the capture summaries they
//! return.

// two channels with parallel arrays everywhere: index loops read better than zips
#![allow(clippy::needless_range_loop)]

pub mod cal;
pub mod conv;
pub mod device;
pub mod dmm;
mod error;
pub mod logic;
pub mod psu;
pub mod scope;
pub mod stats;
pub mod wave;

pub use device::{
    DEFAULT_TIMEOUT, DEFAULT_URI, LogicTrigger, M2k, TriggerCondition, check_logic_trigger_delay,
    parse_uri,
};
pub use error::{Context, Error, Result};
