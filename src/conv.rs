//! Raw values to volts and back, with the constants and formulas of ADI's libm2k (`m2kanalogin_impl.cpp`,
//! `m2kanalogout_impl.cpp`, `m2kpowersupply_impl.cpp`, `m2kcalibration_impl.cpp`). Pure functions, no device.

use anyhow::{Result, bail};

/// Scope input range, set by the gain of the input stage (`m2k-fabric` `gain`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Range {
    /// ±25 V, gain `low`.
    V25,
    /// ±2.5 V, gain `high`.
    V2_5,
}

impl Range {
    /// The value of the `gain` attribute.
    pub fn gain_name(self) -> &'static str {
        match self {
            Range::V25 => "low",
            Range::V2_5 => "high",
        }
    }

    pub fn from_gain_name(name: &str) -> Range {
        if name == "high" {
            Range::V2_5
        } else {
            Range::V25
        }
    }

    /// Gain of the input stage.
    pub fn hw_gain(self) -> f64 {
        match self {
            Range::V25 => 0.02017,
            Range::V2_5 => 0.21229,
        }
    }

    /// The largest voltage, of either sign, the range shows.
    pub fn limit(self) -> f64 {
        match self {
            Range::V25 => 25.0,
            Range::V2_5 => 2.5,
        }
    }
}

/// ADC full scale in volts at the ADC pins, and the gain of the stage in front of it.
const ADC_VREF: f64 = 0.78;
const ADC_GAIN: f64 = 1.3;
/// Raw ADC samples are 12 bit signed.
pub const ADC_RAW_MAX: i64 = 2047;
pub const ADC_RAW_MIN: i64 = -2048;
/// Offset DAC value without any correction.
pub const DEFAULT_OFFSET: i64 = 2048;

/// The decimation filters change the amplitude; libm2k's table per ADC `sampling_frequency`.
pub fn adc_filter_compensation(sample_rate: f64) -> Option<f64> {
    [
        (1e8, 1.00),
        (1e7, 1.05),
        (1e6, 1.10),
        (1e5, 1.15),
        (1e4, 1.20),
        (1e3, 1.26),
    ]
    .into_iter()
    .find(|(rate, _)| *rate == sample_rate)
    .map(|(_, comp)| comp)
}

/// The same for the DACs, per `sampling_frequency` of `m2k-dac-a`/`-b`.
pub fn dac_filter_compensation(sample_rate: f64) -> Option<f64> {
    [
        (75e6, 1.00),
        (75e5, 1.525879),
        (75e4, 1.164153),
        (75e3, 1.776357),
        (75e2, 1.355253),
        (75e1, 1.033976),
    ]
    .into_iter()
    .find(|(rate, _)| *rate == sample_rate)
    .map(|(_, comp)| comp)
}

/// How one scope channel turns raw samples into volts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdcScale {
    pub range: Range,
    /// Gain correction applied here (`calibscale` on firmware 0.32 and later, else 1: older gateware applies it).
    pub gain: f64,
    pub filter_compensation: f64,
    /// Volts the offset DAC shifts the input by.
    pub vertical_offset: f64,
}

impl AdcScale {
    pub fn volts(&self, raw: i64) -> f64 {
        raw as f64 * self.volts_per_lsb() - self.vertical_offset
    }

    pub fn volts_per_lsb(&self) -> f64 {
        ADC_VREF / (2048.0 * ADC_GAIN * self.range.hw_gain()) * self.gain * self.filter_compensation
    }
}

/// Volts at the ADC itself, as the calibration measures them: input stage and corrections left out.
pub fn adc_raw_to_volts_uncorrected(raw: f64, gain: f64) -> f64 {
    raw * ADC_VREF / (2048.0 * ADC_GAIN) * gain
}

/// Offset DAC steps for a vertical offset in volts.
pub fn vertical_offset_to_raw(volts: f64, range: Range) -> i64 {
    (volts * 4096.0 * range.hw_gain() * ADC_GAIN / 2.693 / 1.2) as i64
}

pub fn vertical_offset_to_volts(raw: i64, range: Range) -> f64 {
    raw as f64 * 2.693 * 1.2 / (4096.0 * range.hw_gain() * ADC_GAIN)
}

/// Volts per DAC step before calibration; the gateware scales by `calibscale`, so this stays the nominal value.
pub const DAC_VLSB: f64 = 10.0 / 4095.0;

/// A voltage as the 16-bit sample the DAC buffer takes (12 bits, left aligned, inverted).
pub fn dac_volts_to_raw(volts: f64, filter_compensation: f64) -> i16 {
    let steps = ((volts * (-1.0 / DAC_VLSB) - 0.5) / filter_compensation) as i32;
    (steps.clamp(-2048, 2047) << 4) as i16
}

pub fn dac_raw_to_volts(raw: i16, filter_compensation: f64) -> f64 {
    -((f64::from(raw >> 4) + 0.5) * filter_compensation * DAC_VLSB)
}

/// The DAC swings about ±5 V.
pub const DAC_LIMIT: f64 = 5.0;

/// One of the two user supplies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rail {
    /// V+, 0 to 5 V.
    Pos,
    /// V-, 0 to -5 V.
    Neg,
}

impl Rail {
    pub fn name(self) -> &'static str {
        match self {
            Rail::Pos => "V+",
            Rail::Neg => "V-",
        }
    }

    fn write_coefficient(self) -> f64 {
        match self {
            Rail::Pos => 4095.0 / (5.02 * 1.2),
            Rail::Neg => 4095.0 / (-5.1 * 1.2),
        }
    }

    fn read_coefficient(self) -> f64 {
        match self {
            Rail::Pos => 6.4 / 4095.0,
            Rail::Neg => -6.4 / 4095.0,
        }
    }
}

/// Factory offset and gain of a supply rail from the `cal,*` context attributes; `IDENTITY` when missing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RailCal {
    pub offset: f64,
    pub gain: f64,
}

impl RailCal {
    pub const IDENTITY: RailCal = RailCal {
        offset: 0.0,
        gain: 1.0,
    };
}

/// Raw value for the supply DAC (`ad5627`).
pub fn psu_volts_to_raw(rail: Rail, volts: f64, cal: RailCal) -> Result<i64> {
    let ok = match rail {
        Rail::Pos => (0.0..=5.0).contains(&volts),
        Rail::Neg => (-5.0..=0.0).contains(&volts),
    };
    if !ok {
        bail!(
            "{} takes {} V, not {volts}",
            rail.name(),
            if rail == Rail::Pos { "0 to 5" } else { "-5 to 0" }
        );
    }
    let raw = (volts * cal.gain + cal.offset) * rail.write_coefficient();
    Ok((raw.max(0.0) as i64).min(4095))
}

/// Volts from the supply monitor ADC (`ad9963`).
pub fn psu_raw_to_volts(rail: Rail, raw: f64, cal: RailCal) -> f64 {
    (raw * rail.read_coefficient() + cal.offset) * cal.gain
}

/// One row of the factory calibration table (`cal,temp_lut`): the corrections found at one temperature.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LutRow {
    pub temperature: f64,
    pub adc_offset: [i64; 2],
    pub adc_gain: [f64; 2],
    pub dac_offset: [i64; 2],
    pub dac_gain: [f64; 2],
}

/// Parses `cal,temp_lut`: nine comma separated numbers per temperature.
pub fn parse_temp_lut(text: &str) -> Result<Vec<LutRow>> {
    let values: Vec<&str> = text.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    if values.is_empty() || values.len() % 9 != 0 {
        bail!(
            "calibration table has {} values, expected a multiple of 9",
            values.len()
        );
    }
    let float = |s: &str| -> Result<f64> {
        s.parse()
            .map_err(|_| anyhow::anyhow!("calibration table: `{s}` is not a number"))
    };
    let int = |s: &str| -> Result<i64> {
        s.parse()
            .map_err(|_| anyhow::anyhow!("calibration table: `{s}` is not an integer"))
    };
    let mut rows = vec![];
    for row in values.chunks_exact(9) {
        rows.push(LutRow {
            temperature: float(row[0])?,
            adc_offset: [int(row[1])?, int(row[2])?],
            adc_gain: [float(row[3])?, float(row[4])?],
            dac_offset: [int(row[5])?, int(row[6])?],
            dac_gain: [float(row[7])?, float(row[8])?],
        });
    }
    Ok(rows)
}

/// The row measured closest to `temperature`.
pub fn nearest_lut_row(rows: &[LutRow], temperature: f64) -> Option<LutRow> {
    rows.iter()
        .copied()
        .min_by(|a, b| (a.temperature - temperature).abs().total_cmp(&(b.temperature - temperature).abs()))
}

/// A cyclic DAC buffer that holds whole periods of a signal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WavePlan {
    pub sample_rate: f64,
    pub samples: usize,
    pub periods: usize,
}

impl WavePlan {
    /// The frequency that really comes out.
    pub fn frequency(&self) -> f64 {
        self.periods as f64 * self.sample_rate / self.samples as f64
    }
}

/// Buffers must hold a multiple of 4 samples, and at least 16.
const DAC_BUFFER_STEP: usize = 4;
const DAC_BUFFER_MIN: usize = 16;
/// Enough for one period of 300 Hz at 75 MS/s; lower frequencies use a lower sample rate.
const DAC_BUFFER_MAX: usize = 1 << 18;
const MAX_PERIODS: usize = 256;

/// Picks the DAC sample rate and buffer length for a periodic signal of `frequency` Hz: the highest rate whose
/// buffer still fits, then the number of whole periods that brings the output frequency closest.
pub fn plan_wave(frequency: f64, rates: &[f64]) -> Result<WavePlan> {
    if !frequency.is_finite() || frequency <= 0.0 {
        bail!("frequency must be above 0 Hz");
    }
    let mut rates = rates.to_vec();
    rates.sort_by(|a, b| b.total_cmp(a));
    let Some(&top) = rates.first() else {
        bail!("the DAC lists no sample rates");
    };
    if frequency > top / 4.0 {
        bail!(
            "{frequency} Hz is above what the DAC can make ({} Hz at {top} S/s)",
            top / 4.0
        );
    }
    for rate in rates {
        let per_period = rate / frequency;
        if per_period > DAC_BUFFER_MAX as f64 {
            continue;
        }
        let mut best: Option<(f64, WavePlan)> = None;
        for periods in 1..=MAX_PERIODS {
            let exact = per_period * periods as f64;
            if exact > DAC_BUFFER_MAX as f64 {
                break;
            }
            let samples = ((exact / DAC_BUFFER_STEP as f64).round() as usize * DAC_BUFFER_STEP).max(DAC_BUFFER_MIN);
            if samples > DAC_BUFFER_MAX || (samples as f64) < periods as f64 * 4.0 {
                continue;
            }
            let plan = WavePlan {
                sample_rate: rate,
                samples,
                periods,
            };
            let error = (plan.frequency() - frequency).abs() / frequency;
            if best.is_none_or(|(e, _)| error < e - 1e-12) {
                best = Some((error, plan));
            }
            if error < 1e-12 {
                break;
            }
        }
        if let Some((_, plan)) = best {
            return Ok(plan);
        }
    }
    bail!("{frequency} Hz is too low for the DAC's lowest sample rate")
}

/// Picks the ADC `sampling_frequency` and `oversampling_ratio` for a wanted sample rate: the lowest listed rate
/// that is at least `wanted`, divided down to the nearest rate. Returns (sampling_frequency, ratio, actual).
pub fn plan_adc_rate(wanted: f64, rates: &[f64]) -> Result<(f64, u32, f64)> {
    if !wanted.is_finite() || wanted <= 0.0 {
        bail!("sample rate must be above 0");
    }
    let mut rates = rates.to_vec();
    rates.sort_by(|a, b| a.total_cmp(b));
    let Some(&top) = rates.last() else {
        bail!("the ADC lists no sample rates");
    };
    if wanted > top {
        bail!("{wanted} S/s is above the ADC's {top} S/s");
    }
    let base = rates.into_iter().find(|r| *r >= wanted).unwrap_or(top);
    let ratio = (base / wanted).round().clamp(1.0, f64::from(u32::MAX)) as u32;
    Ok((base, ratio, base / f64::from(ratio)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAC_RATES: [f64; 6] = [750.0, 7500.0, 75e3, 75e4, 75e5, 75e6];
    const ADC_RATES: [f64; 6] = [1e3, 1e4, 1e5, 1e6, 1e7, 1e8];

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn adc_volts_match_libm2k() {
        // libm2k: sample * 0.78 / (2048 * 1.3 * hw_gain) * correction * compensation + offset
        let scale = AdcScale {
            range: Range::V25,
            gain: 1.0,
            filter_compensation: 1.0,
            vertical_offset: 0.0,
        };
        assert!(close(scale.volts(2047), 29.04, 0.01), "{}", scale.volts(2047));
        assert!(close(scale.volts(-100), -1.4525, 0.001));
        let high = AdcScale {
            range: Range::V2_5,
            gain: 1.02,
            filter_compensation: 1.1,
            vertical_offset: 0.5,
        };
        let expect = 1000.0 * 0.78 / (2048.0 * 1.3 * 0.21229) * 1.02 * 1.1 - 0.5;
        assert!(close(high.volts(1000), expect, 1e-12));
        assert_eq!(adc_filter_compensation(1e6), Some(1.10));
        assert_eq!(adc_filter_compensation(2e6), None);
    }

    #[test]
    fn vertical_offset_round_trips() {
        for range in [Range::V25, Range::V2_5] {
            let raw = vertical_offset_to_raw(1.0, range);
            assert!(close(vertical_offset_to_volts(raw, range), 1.0, range.limit() / 500.0));
        }
        assert_eq!(vertical_offset_to_raw(0.0, Range::V25), 0);
        assert_eq!(Range::from_gain_name("high"), Range::V2_5);
        assert_eq!(Range::from_gain_name("low").gain_name(), "low");
    }

    #[test]
    fn dac_raw_matches_libm2k() {
        // (short)((V * (-1 / vlsb) - 0.5) / compensation) << 4
        assert_eq!(dac_volts_to_raw(0.0, 1.0), 0);
        assert_eq!(dac_volts_to_raw(1.0, 1.0), -409 << 4);
        assert_eq!(dac_volts_to_raw(-1.0, 1.0), 409 << 4);
        assert_eq!(dac_volts_to_raw(2.5, 1.525879), -671 << 4);
        // clipped rather than wrapped
        assert_eq!(dac_volts_to_raw(-9.0, 1.0), 2047 << 4);
        assert_eq!(dac_volts_to_raw(9.0, 1.0), -2048 << 4);
        assert!(close(dac_raw_to_volts(dac_volts_to_raw(3.3, 1.0), 1.0), 3.3, DAC_VLSB));
        assert_eq!(dac_filter_compensation(75e4), Some(1.164153));
    }

    #[test]
    fn psu_matches_libm2k() {
        let id = RailCal::IDENTITY;
        assert_eq!(psu_volts_to_raw(Rail::Pos, 5.0, id).unwrap(), 3398);
        assert_eq!(psu_volts_to_raw(Rail::Neg, -5.0, id).unwrap(), 3345);
        assert_eq!(psu_volts_to_raw(Rail::Pos, 0.0, id).unwrap(), 0);
        let cal = RailCal {
            offset: 0.018,
            gain: 0.998048,
        };
        assert_eq!(psu_volts_to_raw(Rail::Pos, 3.3, cal).unwrap(), 2251);
        assert!(psu_volts_to_raw(Rail::Pos, 5.5, id).is_err());
        assert!(psu_volts_to_raw(Rail::Pos, -1.0, id).is_err());
        assert!(psu_volts_to_raw(Rail::Neg, 3.0, id).is_err());
        assert!(close(psu_raw_to_volts(Rail::Pos, 2112.0, id), 3.3008, 1e-3));
        assert!(close(psu_raw_to_volts(Rail::Neg, 2112.0, id), -3.3008, 1e-3));
    }

    #[test]
    fn parses_temp_lut() {
        let rows = parse_temp_lut("32.5,2040,2050,1.01,0.99,2060,2070,1.02,0.98, 45,2041,2051,1.0,1.0,2061,2071,1.0,1.0").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].adc_offset, [2040, 2050]);
        assert_eq!(rows[0].dac_gain, [1.02, 0.98]);
        assert_eq!(nearest_lut_row(&rows, 40.0).unwrap().temperature, 45.0);
        assert_eq!(nearest_lut_row(&rows, -10.0).unwrap().temperature, 32.5);
        assert!(parse_temp_lut("1,2,3").is_err());
        assert!(parse_temp_lut("").is_err());
        assert!(parse_temp_lut("a,1,2,3,4,5,6,7,8").is_err());
    }

    #[test]
    fn plans_waves() {
        let p = plan_wave(1000.0, &DAC_RATES).unwrap();
        assert_eq!((p.sample_rate, p.samples, p.periods), (75e6, 75000, 1));
        // 7 kHz does not divide 75 MS/s: seven periods do
        let p = plan_wave(7000.0, &DAC_RATES).unwrap();
        assert_eq!((p.samples, p.periods), (75000, 7));
        assert!(close(p.frequency(), 7000.0, 1e-9));
        // too long for 75 MS/s
        let p = plan_wave(50.0, &DAC_RATES).unwrap();
        assert_eq!((p.sample_rate, p.samples, p.periods), (75e5, 150_000, 1));
        let p = plan_wave(10e6, &DAC_RATES).unwrap();
        assert_eq!(p.samples % 4, 0);
        assert!(p.samples >= 16);
        assert!(close(p.frequency(), 10e6, 1.0), "{p:?}");
        let p = plan_wave(0.01, &DAC_RATES).unwrap();
        assert_eq!(p.sample_rate, 750.0);
        assert!(plan_wave(30e6, &DAC_RATES).is_err());
        assert!(plan_wave(0.0, &DAC_RATES).is_err());
        assert!(plan_wave(0.0001, &DAC_RATES).is_err());
    }

    #[test]
    fn plans_adc_rates() {
        assert_eq!(plan_adc_rate(1e6, &ADC_RATES).unwrap(), (1e6, 1, 1e6));
        assert_eq!(plan_adc_rate(250e3, &ADC_RATES).unwrap(), (1e6, 4, 250e3));
        assert_eq!(plan_adc_rate(1e8, &ADC_RATES).unwrap(), (1e8, 1, 1e8));
        assert_eq!(plan_adc_rate(100.0, &ADC_RATES).unwrap(), (1e3, 10, 100.0));
        assert!(plan_adc_rate(2e8, &ADC_RATES).is_err());
        assert!(plan_adc_rate(0.0, &ADC_RATES).is_err());
    }
}
