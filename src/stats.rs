//! What a capture looks like in a few numbers: the lines `scope`, `voltmeter` and `logic` print.

use serde_json::{Value, json};

/// Summary of an analog capture of one channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Analog {
    pub samples: usize,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    /// RMS including the DC part.
    pub rms: f64,
    /// RMS of what is left without the mean.
    pub ac_rms: f64,
    /// From rising crossings of the middle between min and max; `None` without at least two of them.
    pub frequency: Option<f64>,
    /// Whole periods seen.
    pub periods: usize,
    /// Share of the time above the middle, over whole periods.
    pub duty: Option<f64>,
    /// Samples at the end of the ADC range: the signal is larger than the input range.
    pub clipped: usize,
}

impl Analog {
    pub fn vpp(&self) -> f64 {
        self.max - self.min
    }

    pub fn json(&self) -> Value {
        json!({
            "samples": self.samples,
            "min": self.min,
            "max": self.max,
            "mean": self.mean,
            "rms": self.rms,
            "ac_rms": self.ac_rms,
            "vpp": self.vpp(),
            "frequency": self.frequency,
            "periods": self.periods,
            "duty": self.duty,
            "clipped": self.clipped,
        })
    }
}

/// Summarizes `volts` sampled at `sample_rate`; `clipped` is counted by the caller from the raw samples. A
/// swing below `min_swing` (a few ADC steps) is noise: no frequency is looked for in it.
pub fn analog(volts: &[f64], sample_rate: f64, clipped: usize, min_swing: f64) -> Analog {
    let n = volts.len();
    if n == 0 {
        return Analog {
            samples: 0,
            min: 0.0,
            max: 0.0,
            mean: 0.0,
            rms: 0.0,
            ac_rms: 0.0,
            frequency: None,
            periods: 0,
            duty: None,
            clipped,
        };
    }
    let (mut min, mut max, mut sum, mut squares) = (f64::INFINITY, f64::NEG_INFINITY, 0.0, 0.0);
    for v in volts {
        min = min.min(*v);
        max = max.max(*v);
        sum += v;
        squares += v * v;
    }
    let mean = sum / n as f64;
    let mean_square = squares / n as f64;
    let (frequency, periods, duty) = if max - min >= min_swing {
        periodicity(volts, sample_rate, min, max)
    } else {
        (None, 0, None)
    };
    Analog {
        samples: n,
        min,
        max,
        mean,
        rms: mean_square.sqrt(),
        ac_rms: (mean_square - mean * mean).max(0.0).sqrt(),
        frequency,
        periods,
        duty,
        clipped,
    }
}

/// Frequency, whole periods and duty cycle from rising crossings of the middle level, with a hysteresis of a
/// tenth of the swing so that noise on a flat or slow signal is not counted.
fn periodicity(
    volts: &[f64],
    sample_rate: f64,
    min: f64,
    max: f64,
) -> (Option<f64>, usize, Option<f64>) {
    let swing = max - min;
    if swing <= 0.0 || sample_rate <= 0.0 {
        return (None, 0, None);
    }
    let mid = (max + min) / 2.0;
    let (low, high) = (mid - swing * 0.1, mid + swing * 0.1);
    let mut armed = false;
    // rising crossings as fractional sample positions
    let mut crossings: Vec<f64> = vec![];
    let mut last_below = 0usize;
    for (i, v) in volts.iter().enumerate() {
        if *v < low {
            armed = true;
        }
        if *v < mid {
            last_below = i;
        } else if armed && *v > high {
            armed = false;
            let (a, b) = (
                volts[last_below],
                volts[(last_below + 1).min(volts.len() - 1)],
            );
            let frac = if b > a {
                ((mid - a) / (b - a)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            crossings.push(last_below as f64 + frac);
        }
    }
    if crossings.len() < 2 {
        return (None, 0, None);
    }
    let periods = crossings.len() - 1;
    let span = crossings[periods] - crossings[0];
    if span <= 0.0 {
        return (None, 0, None);
    }
    let (first, last) = (
        crossings[0].ceil() as usize,
        crossings[periods].ceil() as usize,
    );
    let above = volts[first..last.min(volts.len())]
        .iter()
        .filter(|v| **v >= mid)
        .count();
    let duty = (last > first).then(|| above as f64 / (last - first) as f64);
    (Some(periods as f64 * sample_rate / span), periods, duty)
}

/// Summary of one digital pin over a capture.
#[derive(Debug, Clone, PartialEq)]
pub struct Digital {
    pub channel: u32,
    pub samples: usize,
    pub rising: usize,
    pub falling: usize,
    /// Share of the samples that are high.
    pub high: f64,
    /// From the first to the last rising edge; `None` without at least two.
    pub frequency: Option<f64>,
    /// Shortest time between two edges, in seconds.
    pub min_pulse: Option<f64>,
}

impl Digital {
    pub fn edges(&self) -> usize {
        self.rising + self.falling
    }

    /// `low`, `high` or `toggling`.
    pub fn state(&self) -> &'static str {
        match (self.edges(), self.high > 0.5) {
            (0, false) => "low",
            (0, true) => "high",
            _ => "toggling",
        }
    }

    pub fn json(&self) -> Value {
        json!({
            "channel": self.channel,
            "state": self.state(),
            "edges": self.edges(),
            "rising": self.rising,
            "falling": self.falling,
            "high": self.high,
            "frequency": self.frequency,
            "min_pulse": self.min_pulse,
        })
    }
}

/// Summarizes bit `channel` of the 16-bit samples in `words`.
pub fn digital(words: &[u16], channel: u32, sample_rate: f64) -> Digital {
    let bit = |w: &u16| w >> channel & 1 == 1;
    let mut d = Digital {
        channel,
        samples: words.len(),
        rising: 0,
        falling: 0,
        high: 0.0,
        frequency: None,
        min_pulse: None,
    };
    let Some(mut prev) = words.first().map(bit) else {
        return d;
    };
    let (mut high, mut first_rise, mut last_rise) = (usize::from(prev), None, 0);
    let (mut last_edge, mut min_gap) = (None::<usize>, None::<usize>);
    for (i, w) in words.iter().enumerate().skip(1) {
        let now = bit(w);
        high += usize::from(now);
        if now != prev {
            if now {
                d.rising += 1;
                first_rise.get_or_insert(i);
                last_rise = i;
            } else {
                d.falling += 1;
            }
            if let Some(gap) = last_edge.map(|e| i - e) {
                min_gap = Some(min_gap.map_or(gap, |m| m.min(gap)));
            }
            last_edge = Some(i);
            prev = now;
        }
    }
    d.high = high as f64 / words.len() as f64;
    if let Some(first) = first_rise.filter(|f| d.rising >= 2 && last_rise > *f) {
        d.frequency = Some((d.rising - 1) as f64 * sample_rate / (last_rise - first) as f64);
    }
    d.min_pulse = min_gap.map(|g| g as f64 / sample_rate);
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_stats() {
        let rate = 100e3;
        let volts: Vec<f64> = (0..10_000)
            .map(|i| 1.0 + 2.0 * (std::f64::consts::TAU * 1000.0 * i as f64 / rate).sin())
            .collect();
        let s = analog(&volts, rate, 0, 0.0);
        assert!((s.mean - 1.0).abs() < 1e-3, "{s:?}");
        assert!((s.vpp() - 4.0).abs() < 1e-2);
        assert!((s.ac_rms - 2.0 / 2f64.sqrt()).abs() < 1e-3);
        assert!((s.rms - (1.0f64 + 2.0).sqrt()).abs() < 1e-3);
        assert!((s.frequency.unwrap() - 1000.0).abs() < 0.5, "{s:?}");
        assert_eq!(s.periods, 98);
        assert!((s.duty.unwrap() - 0.5).abs() < 0.02);
    }

    #[test]
    fn flat_and_noisy_signals_have_no_frequency() {
        let s = analog(&[3.3; 100], 1e3, 0, 0.0);
        assert_eq!((s.frequency, s.vpp(), s.ac_rms), (None, 0.0, 0.0));
        // one step is not a period
        let step: Vec<f64> = (0..100).map(|i| if i < 50 { 0.0 } else { 1.0 }).collect();
        assert_eq!(analog(&step, 1e3, 0, 0.0).frequency, None);
        assert_eq!(analog(&[], 1e3, 2, 0.0).clipped, 2);
        // a small square is noise when the floor is above it
        let noise: Vec<f64> = (0..400)
            .map(|i| if i % 4 < 2 { 0.01 } else { 0.0 })
            .collect();
        assert!(analog(&noise, 1e3, 0, 0.0).frequency.is_some());
        assert_eq!(analog(&noise, 1e3, 0, 0.1).frequency, None);
    }

    #[test]
    fn square_wave_duty() {
        // 25 % high, period 20 samples
        let volts: Vec<f64> = (0..400)
            .map(|i| if i % 20 < 5 { 3.3 } else { 0.0 })
            .collect();
        let s = analog(&volts, 20e3, 0, 0.0);
        assert!((s.frequency.unwrap() - 1000.0).abs() < 1.0, "{s:?}");
        assert!((s.duty.unwrap() - 0.25).abs() < 0.01, "{s:?}");
    }

    #[test]
    fn digital_stats() {
        // bit 0: period 4 samples; bit 1: stuck high; bit 2: stuck low
        let words: Vec<u16> = (0..40).map(|i| u16::from(i % 4 < 2) | 2).collect();
        let d0 = digital(&words, 0, 4000.0);
        assert_eq!(d0.state(), "toggling");
        assert_eq!((d0.rising, d0.falling), (9, 10));
        assert_eq!(d0.frequency, Some(1000.0));
        assert_eq!(d0.min_pulse, Some(0.0005));
        assert_eq!(d0.high, 0.5);
        let d1 = digital(&words, 1, 4000.0);
        assert_eq!((d1.state(), d1.edges(), d1.frequency), ("high", 0, None));
        assert_eq!(digital(&words, 2, 4000.0).state(), "low");
        assert_eq!(digital(&[], 0, 1.0).samples, 0);
    }
}
