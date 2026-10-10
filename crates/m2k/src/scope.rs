//! A scope capture as typed data: raw samples turned into volts and a per-channel statistical summary.

use crate::conv::{self, Range};
use crate::device::{self, M2k};
use crate::error::{Result, err};
use crate::stats::{self, Analog};

/// A capture of both scope channels, in volts.
pub struct Capture {
    /// Samples per second actually used (the nearest the ADC's divider can hit to what was asked for).
    pub rate: f64,
    pub volts: [Vec<f64>; 2],
    pub stats: [Analog; 2],
    pub ranges: [Range; 2],
    /// The scope was calibrated since power-up.
    pub calibrated: bool,
}

/// Captures `samples` samples per channel at (close to) `rate`, in `ranges`; `progress` is told bytes captured
/// so far. The rate really used is [`Capture::rate`].
pub fn capture(
    m: &mut M2k,
    samples: usize,
    rate: f64,
    ranges: [Range; 2],
    progress: impl FnMut(usize),
) -> Result<Capture> {
    let available = m.rates(device::ADC)?;
    let (base, ratio, _) = conv::plan_adc_rate(rate, &available)?;
    let state = m.adc_configure(base, ratio, ranges)?;
    let secs = samples as f64 / state.rate();
    m.allow(secs)?;
    let raw = m.adc_capture(samples, progress)?;
    let mut volts = [vec![], vec![]];
    let mut st = vec![];
    for i in 0..2 {
        let scale = state.scale(i)?;
        volts[i] = raw[i].iter().map(|r| scale.volts(*r)).collect();
        let clipped = raw[i]
            .iter()
            .filter(|r| **r <= conv::ADC_RAW_MIN || **r >= conv::ADC_RAW_MAX)
            .count();
        // ten ADC steps: below that a "frequency" is noise
        st.push(stats::analog(
            &volts[i],
            state.rate(),
            clipped,
            10.0 * scale.volts_per_lsb(),
        ));
    }
    let [s1, s2]: [Analog; 2] = st.try_into().map_err(|_| err!("two channels"))?;
    Ok(Capture {
        rate: state.rate(),
        volts,
        stats: [s1, s2],
        ranges: [state.channels[0].range, state.channels[1].range],
        calibrated: !state.uncalibrated(),
    })
}

/// DC and AC rms on both channels: 100 ms at 100 kS/s (whole mains periods, so hum averages out of the DC
/// value), at `range`.
pub fn voltmeter(m: &mut M2k, range: Range, progress: impl FnMut(usize)) -> Result<Capture> {
    capture(m, 10_000, 100e3, [range, range], progress)
}
