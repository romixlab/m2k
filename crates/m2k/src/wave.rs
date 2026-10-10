//! A signal generator waveform as typed data: one period-filled buffer computed from the available DAC rates.

use crate::conv;
use crate::error::{Result, bail};

/// A periodic signal: frequency, peak amplitude, DC offset, phase (degrees) and duty cycle (percent, square
/// only).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Periodic {
    pub freq: f64,
    pub amp: f64,
    pub offset: f64,
    pub phase: f64,
    pub duty: f64,
}

/// A signal shape for [`build_wave`]. Switching an output off is not a wave; callers do that with
/// `M2k::dac_stop` instead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// A constant voltage, -5 to 5 V.
    Const(f64),
    Sine(Periodic),
    Square(Periodic),
    Triangle(Periodic),
    /// Rising ramp.
    Sawtooth(Periodic),
}

/// One period-filled buffer of a shape, and what it really is.
pub struct Wave {
    pub rate: f64,
    pub volts: Vec<f64>,
    /// The frequency really output (`None` for a constant); close to what was asked for, not always exact.
    pub frequency: Option<f64>,
}

/// Builds the sample buffer for `shape` from the DAC's listed `rates`, picking the rate and period count that
/// bring the output frequency closest to what was asked for (see [`conv::plan_wave`]).
pub fn build_wave(shape: &Shape, rates: &[f64]) -> Result<Wave> {
    let check = |lo: f64, hi: f64| -> Result<()> {
        if lo < -conv::DAC_LIMIT - 1e-9 || hi > conv::DAC_LIMIT + 1e-9 {
            bail!("the signal goes from {lo:.3} to {hi:.3} V; the outputs swing -5 to 5 V");
        }
        Ok(())
    };
    match shape {
        Shape::Const(volts) => {
            check(*volts, *volts)?;
            let rate = rates.iter().copied().fold(f64::NAN, f64::max);
            Ok(Wave {
                rate,
                volts: vec![*volts; 256],
                frequency: None,
            })
        }
        Shape::Sine(p) | Shape::Square(p) | Shape::Triangle(p) | Shape::Sawtooth(p) => {
            if p.amp < 0.0 {
                bail!("amplitude must not be negative");
            }
            check(p.offset - p.amp, p.offset + p.amp)?;
            if !(0.0..=100.0).contains(&p.duty) {
                bail!("duty is 0 to 100 %");
            }
            let plan = conv::plan_wave(p.freq, rates)?;
            let phase0 = p.phase / 360.0;
            let volts = (0..plan.samples)
                .map(|k| {
                    let x = (k as f64 * plan.periods as f64 / plan.samples as f64 + phase0)
                        .rem_euclid(1.0);
                    let unit = match shape {
                        Shape::Sine(_) => (std::f64::consts::TAU * x).sin(),
                        Shape::Square(_) => {
                            if x < p.duty / 100.0 {
                                1.0
                            } else {
                                -1.0
                            }
                        }
                        Shape::Triangle(_) => 1.0 - 4.0 * (x - 0.5).abs(),
                        _ => 2.0 * x - 1.0,
                    };
                    p.offset + p.amp * unit
                })
                .collect();
            Ok(Wave {
                rate: plan.sample_rate,
                volts,
                frequency: Some(plan.frequency()),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATES: [f64; 6] = [750.0, 7500.0, 75e3, 75e4, 75e5, 75e6];

    fn periodic(freq: f64, amp: f64, offset: f64) -> Periodic {
        Periodic {
            freq,
            amp,
            offset,
            phase: 0.0,
            duty: 50.0,
        }
    }

    #[test]
    fn waves() {
        let w = build_wave(&Shape::Sine(periodic(1000.0, 1.0, 0.5)), &RATES).unwrap();
        assert_eq!(w.volts.len(), 75000);
        let max = w.volts.iter().copied().fold(f64::MIN, f64::max);
        let min = w.volts.iter().copied().fold(f64::MAX, f64::min);
        assert!(
            (max - 1.5).abs() < 1e-6 && (min + 0.5).abs() < 1e-6,
            "{min} {max}"
        );
        let t = build_wave(&Shape::Triangle(periodic(1e6, 2.0, 0.0)), &RATES).unwrap();
        let max = t.volts.iter().copied().fold(f64::MIN, f64::max);
        let min = t.volts.iter().copied().fold(f64::MAX, f64::min);
        assert!(
            max <= 2.0 + 1e-9 && min >= -2.0 - 1e-9 && max > 1.5 && min < -1.5,
            "{min} {max}"
        );
        let sq = build_wave(
            &Shape::Square(Periodic {
                duty: 25.0,
                ..periodic(1e6, 1.0, 0.0)
            }),
            &RATES,
        )
        .unwrap();
        let high = sq.volts.iter().filter(|v| **v > 0.0).count() as f64 / sq.volts.len() as f64;
        assert!((high - 0.25).abs() < 0.02, "{high}");
        let c = build_wave(&Shape::Const(-1.25), &RATES).unwrap();
        assert_eq!(c.rate, 75e6);
        assert!(c.volts.iter().all(|v| *v == -1.25));
        assert!(build_wave(&Shape::Sine(periodic(1000.0, 3.0, 3.0)), &RATES).is_err());
        assert!(build_wave(&Shape::Const(6.0), &RATES).is_err());
    }
}
