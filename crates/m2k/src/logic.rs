//! A logic analyzer capture as typed data: raw per-sample words and a summary per requested pin.

use crate::device::{LogicTrigger, M2k};
use crate::error::Result;
use crate::stats::{self, Digital};

/// A capture of the 16 digital pins, summarized per pin asked for in `capture`'s `pins`.
pub struct Capture {
    /// Samples per second actually used (the nearest the divider can hit to what was asked for).
    pub rate: f64,
    /// One `u16` per sample, bit N = DIO N.
    pub words: Vec<u16>,
    /// Per-pin summary, in the order of the `pins` passed to [`capture`].
    pub pins: Vec<Digital>,
    /// `in`/`out` for each of the 16 pins.
    pub directions: Vec<String>,
}

/// Captures `samples` samples of all 16 pins at (close to) `rate`, summarized for `pins`; `progress` is told
/// bytes captured so far. The rate really used is [`Capture::rate`].
pub fn capture(
    m: &mut M2k,
    samples: usize,
    rate: f64,
    pins: &[u32],
    trigger: &LogicTrigger,
    progress: impl FnMut(usize),
) -> Result<Capture> {
    let (words, actual) = m.logic_capture(samples, rate, trigger, progress)?;
    let directions = m.logic_directions().unwrap_or_default();
    let pin_stats = pins
        .iter()
        .map(|&p| stats::digital(&words, p, actual))
        .collect();
    Ok(Capture {
        rate: actual,
        words,
        pins: pin_stats,
        directions,
    })
}
