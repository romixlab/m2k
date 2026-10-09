//! Self-calibration of the scope inputs and signal outputs, step by step as libm2k's `M2kCalibrationImpl` does
//! it. The M2K keeps the corrections in registers until it loses power, so one run serves every later command.
//!
//! - Scope offset: inputs switched to ground (`calibration_mode` `adc_gnd`), the offset DAC value that brings
//!   the average closest to zero, a coarse estimate and then 21 values around it.
//! - Scope gain: inputs on the internal 0.46172 V reference (`adc_ref1`), gain = reference / measured.
//! - Output offset and gain: outputs looped to the inputs (`dac`), measured at code 0 and at code 1024.

use crate::conv::{self, DEFAULT_OFFSET};
use crate::m2k::{ADC, ADC_TRIGGER, DACS, FABRIC, M2k, OFFSET_DAC};
use anyhow::{Result, bail};
use iiod_client::Attr;
use serde_json::{Value, json};
use std::time::Duration;

const OFFSET_SAMPLES: usize = 100_000;
const GAIN_SAMPLES: usize = 150_000;
/// Voltage of the internal reference `adc_ref1`.
const VREF1: f64 = 0.46172;
/// Offset DAC values tried around the coarse estimate.
const FINE_SPAN: i64 = 20;
/// DAC code of the gain measurement.
const DAC_GAIN_CODE: i16 = 1024;
/// Gain of the path from the DAC to the ADC in `dac` calibration mode, and the volts per offset DAC step there.
const DAC_PATH_GAIN: f64 = 9.06;
const DAC_OFFSET_STEP: f64 = 0.002658;
/// Volts per DAC step the gain correction is relative to.
const DAC_CAL_VLSB: f64 = 10.0 / 4096.0;
/// A correction outside of this is a measurement gone wrong, not a property of the board.
const PLAUSIBLE_GAIN: std::ops::RangeInclusive<f64> = 0.5..=1.5;
const PLAUSIBLE_OFFSET: std::ops::RangeInclusive<i64> = 1024..=3072;

fn in_chn<'a>(channel: usize, attr: &'a str) -> Attr<'a> {
    Attr::Channel {
        channel: ["voltage0", "voltage1"][channel.min(1)],
        output: false,
        attr,
    }
}

fn offset_dac(index: usize) -> Attr<'static> {
    Attr::Channel {
        channel: ["voltage0", "voltage1", "voltage2", "voltage3"][index.min(3)],
        output: true,
        attr: "raw",
    }
}

/// The corrections the device holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Corrections {
    /// Scope offset per channel; `None` on firmware before 0.26, which does not keep it.
    pub adc_offset: [Option<i64>; 2],
    pub adc_gain: [f64; 2],
    pub dac_offset: [i64; 2],
    pub dac_gain: [f64; 2],
}

impl Corrections {
    pub fn read(m: &mut M2k) -> Result<Corrections> {
        let mut c = Corrections {
            adc_offset: [None; 2],
            adc_gain: [1.0; 2],
            dac_offset: [DEFAULT_OFFSET; 2],
            dac_gain: [1.0; 2],
        };
        for i in 0..2 {
            if m.has_channel_attr(ADC, (["voltage0", "voltage1"][i], false), "calibbias") {
                c.adc_offset[i] = Some(m.get_i64(ADC, in_chn(i, "calibbias"))?);
            }
            c.adc_gain[i] = m.get_f64(ADC, in_chn(i, "calibscale"))?;
            c.dac_offset[i] = m.get_i64(OFFSET_DAC, offset_dac(i))?;
            c.dac_gain[i] = m.get_f64(DACS[i], Attr::Device("calibscale"))?;
        }
        Ok(c)
    }

    /// Something differs from the power-on values: a calibration ran since power-up (libm2k's test).
    pub fn calibrated(&self) -> bool {
        (0..2).any(|i| {
            self.adc_offset[i].unwrap_or(DEFAULT_OFFSET) != DEFAULT_OFFSET
                || self.dac_offset[i] != DEFAULT_OFFSET
                || self.adc_gain[i] != 1.0
                || self.dac_gain[i] != 1.0
        })
    }

    pub fn json(&self) -> Value {
        json!({
            "calibrated": self.calibrated(),
            "adc_offset": self.adc_offset,
            "adc_gain": self.adc_gain,
            "dac_offset": self.dac_offset,
            "dac_gain": self.dac_gain,
        })
    }

    pub fn text(&self) -> String {
        let offset = |o: Option<i64>| o.map_or("unknown".to_string(), |o| o.to_string());
        format!(
            "scope: offset {} / {}, gain {:.6} / {:.6}\noutputs: offset {} / {}, gain {:.6} / {:.6}",
            offset(self.adc_offset[0]),
            offset(self.adc_offset[1]),
            self.adc_gain[0],
            self.adc_gain[1],
            self.dac_offset[0],
            self.dac_offset[1],
            self.dac_gain[0],
            self.dac_gain[1],
        )
    }
}

/// Writes a scope offset: remembered in `calibbias` and put on the offset DAC (vertical offset 0).
fn set_adc_offset(m: &mut M2k, channel: usize, offset: i64) -> Result<()> {
    if m.has_channel_attr(ADC, (["voltage0", "voltage1"][channel], false), "calibbias") {
        m.set(ADC, in_chn(channel, "calibbias"), offset)?;
    }
    m.adc_set_offset_raw(channel, offset)
}

fn set_mode(m: &mut M2k, mode: &str) -> Result<()> {
    m.set(FABRIC, Attr::Device("calibration_mode"), mode)
}

fn average(samples: &[i64]) -> f64 {
    samples.iter().sum::<i64>() as f64 / samples.len().max(1) as f64
}

/// Scope settings a calibration changes, to put back afterwards.
struct AdcSetup {
    trigger_modes: [String; 2],
    sample_rate: f64,
    oversampling: i64,
}

impl AdcSetup {
    /// Saves the settings and puts the scope in the state the measurements need: free running, 100 MS/s, no
    /// gain correction.
    fn enter(m: &mut M2k) -> Result<AdcSetup> {
        let mode = |i: usize| Attr::Channel {
            channel: ["voltage4", "voltage5"][i],
            output: false,
            attr: "mode",
        };
        let saved = AdcSetup {
            trigger_modes: [m.get(ADC_TRIGGER, mode(0))?, m.get(ADC_TRIGGER, mode(1))?],
            sample_rate: m.get_f64(ADC, Attr::Device("sampling_frequency"))?,
            oversampling: m.get_i64(ADC, Attr::Device("oversampling_ratio"))?,
        };
        for i in 0..2 {
            m.set(ADC_TRIGGER, mode(i), "always")?;
            m.set(ADC, in_chn(i, "calibscale"), 1.0)?;
        }
        m.set(ADC, Attr::Device("sampling_frequency"), 100_000_000)?;
        m.set(ADC, Attr::Device("oversampling_ratio"), 1)?;
        Ok(saved)
    }

    fn restore(&self, m: &mut M2k) -> Result<()> {
        for i in 0..2 {
            let mode = Attr::Channel {
                channel: ["voltage4", "voltage5"][i],
                output: false,
                attr: "mode",
            };
            m.set(ADC_TRIGGER, mode, &self.trigger_modes[i])?;
        }
        m.set(
            ADC,
            Attr::Device("sampling_frequency"),
            self.sample_rate as u64,
        )?;
        m.set(ADC, Attr::Device("oversampling_ratio"), self.oversampling)
    }
}

/// Calibrates the scope inputs; `step` is told what is being measured. On failure the corrections the device
/// had before are put back.
pub fn calibrate_adc(m: &mut M2k, step: &mut dyn FnMut(&str)) -> Result<([i64; 2], [f64; 2])> {
    let before = Corrections::read(m)?;
    let setup = AdcSetup::enter(m)?;
    let measured = measure_adc(m, step);
    let _ = set_mode(m, "none");
    let result = measured.and_then(|(offset, gain)| {
        for i in 0..2 {
            if !PLAUSIBLE_GAIN.contains(&gain[i]) || !PLAUSIBLE_OFFSET.contains(&offset[i]) {
                bail!(
                    "scope channel {} measured an implausible correction (offset {}, gain {:.4}); nothing changed",
                    i + 1,
                    offset[i],
                    gain[i]
                );
            }
        }
        Ok((offset, gain))
    });
    let (offset, gain) = match &result {
        Ok((offset, gain)) => (*offset, *gain),
        Err(_) => (
            before.adc_offset.map(|o| o.unwrap_or(DEFAULT_OFFSET)),
            before.adc_gain,
        ),
    };
    for i in 0..2 {
        set_adc_offset(m, i, offset[i])?;
        m.set(ADC, in_chn(i, "calibscale"), gain[i])?;
    }
    setup.restore(m)?;
    result
}

fn measure_adc(m: &mut M2k, step: &mut dyn FnMut(&str)) -> Result<([i64; 2], [f64; 2])> {
    step("scope offset: inputs to ground");
    set_mode(m, "adc_gnd")?;
    for i in 0..2 {
        set_adc_offset(m, i, DEFAULT_OFFSET)?;
    }
    std::thread::sleep(Duration::from_millis(50));
    let data = m.adc_capture(OFFSET_SAMPLES, |_| {})?;
    let coarse: Vec<i64> = data
        .iter()
        .map(|ch| {
            let volts = conv::adc_raw_to_volts_uncorrected(average(ch).trunc(), 1.0);
            (2048.0 - volts * 4096.0 * 1.3 / 3.192) as i64
        })
        .collect();

    step("scope offset: fine search");
    let first = [coarse[0] - FINE_SPAN / 2, coarse[1] - FINE_SPAN / 2];
    for i in 0..2 {
        set_adc_offset(m, i, first[i])?;
    }
    std::thread::sleep(Duration::from_millis(5));
    let mut buf = m.adc_open(OFFSET_SAMPLES)?;
    let mut best = [(f64::INFINITY, DEFAULT_OFFSET); 2];
    let mut search = || -> Result<()> {
        for k in 0..=FINE_SPAN {
            // the buffer read below was filled before this write: it belongs to candidate k, not k + 1
            for i in 0..2 {
                set_adc_offset(m, i, first[i] + k + 1)?;
            }
            std::thread::sleep(Duration::from_millis(5));
            let data = buf.read(m, |_| {})?;
            for i in 0..2 {
                let avg = average(&data[i]).abs();
                if avg < best[i].0 {
                    best[i] = (avg, first[i] + k);
                }
            }
        }
        Ok(())
    };
    let searched = search();
    buf.close(m);
    searched?;
    let offset = [best[0].1, best[1].1];
    set_mode(m, "none")?;
    for i in 0..2 {
        set_adc_offset(m, i, offset[i])?;
    }

    step("scope gain: inputs to the reference");
    set_mode(m, "adc_ref1")?;
    std::thread::sleep(Duration::from_millis(50));
    let data = m.adc_capture(GAIN_SAMPLES, |_| {})?;
    let mut gain = [1.0; 2];
    for i in 0..2 {
        let volts = conv::adc_raw_to_volts_uncorrected(average(&data[i]).trunc(), 1.0);
        gain[i] = VREF1 / volts;
    }
    Ok((offset, gain))
}

/// Calibrates the signal outputs against the scope inputs, which must be calibrated (`adc_gain` are their gain
/// corrections). Drives W1 and W2 to about 2.5 V for a moment. On failure the previous corrections are put back.
pub fn calibrate_dac(
    m: &mut M2k,
    adc_gain: [f64; 2],
    step: &mut dyn FnMut(&str),
) -> Result<([i64; 2], [f64; 2])> {
    let before = Corrections::read(m)?;
    let setup = AdcSetup::enter(m)?;
    let mut dac_rates = [0.0; 2];
    let mut dac_oversampling = [1; 2];
    for i in 0..2 {
        dac_rates[i] = m.get_f64(DACS[i], Attr::Device("sampling_frequency"))?;
        dac_oversampling[i] = m.get_i64(DACS[i], Attr::Device("oversampling_ratio"))?;
        m.set(DACS[i], Attr::Device("calibscale"), 1.0)?;
    }
    let measured = measure_dac(m, adc_gain, step);
    for i in 0..2 {
        let _ = m.dac_stop(i);
    }
    let _ = set_mode(m, "none");
    let result = measured.and_then(|(offset, gain)| {
        for i in 0..2 {
            if !PLAUSIBLE_GAIN.contains(&gain[i]) || !PLAUSIBLE_OFFSET.contains(&offset[i]) {
                bail!(
                    "output {} measured an implausible correction (offset {}, gain {:.4}); nothing changed",
                    i + 1,
                    offset[i],
                    gain[i]
                );
            }
        }
        Ok((offset, gain))
    });
    let (offset, gain) = match &result {
        Ok((offset, gain)) => (*offset, *gain),
        Err(_) => (before.dac_offset, before.dac_gain),
    };
    for i in 0..2 {
        m.set(OFFSET_DAC, offset_dac(i), offset[i])?;
        m.set(DACS[i], Attr::Device("calibscale"), gain[i])?;
        m.set(ADC, in_chn(i, "calibscale"), adc_gain[i])?;
        m.set(
            DACS[i],
            Attr::Device("sampling_frequency"),
            dac_rates[i] as u64,
        )?;
        m.set(
            DACS[i],
            Attr::Device("oversampling_ratio"),
            dac_oversampling[i],
        )?;
    }
    setup.restore(m)?;
    result
}

fn measure_dac(
    m: &mut M2k,
    adc_gain: [f64; 2],
    step: &mut dyn FnMut(&str),
) -> Result<([i64; 2], [f64; 2])> {
    step("output offset: outputs at code 0");
    set_mode(m, "dac")?;
    for i in 0..2 {
        m.set(OFFSET_DAC, offset_dac(i), DEFAULT_OFFSET)?;
    }
    // 12-bit codes are left aligned and inverted in the 16-bit sample
    m.dac_start_raw(&[0, 1], 75e6, &[0; 256])?;
    std::thread::sleep(Duration::from_millis(50));
    let data = m.adc_capture(OFFSET_SAMPLES, |_| {})?;
    let mut offset = [DEFAULT_OFFSET; 2];
    for i in 0..2 {
        let volts = conv::adc_raw_to_volts_uncorrected(average(&data[i]).trunc(), adc_gain[i]);
        offset[i] = (2048.0 - volts * DAC_PATH_GAIN / DAC_OFFSET_STEP) as i64;
        m.set(OFFSET_DAC, offset_dac(i), offset[i].clamp(0, 4095))?;
    }

    step("output gain: outputs at code 1024");
    m.dac_start_raw(&[0, 1], 75e6, &[-DAC_GAIN_CODE << 4; 256])?;
    std::thread::sleep(Duration::from_millis(50));
    let data = m.adc_capture(GAIN_SAMPLES, |_| {})?;
    let mut gain = [1.0; 2];
    for i in 0..2 {
        let volts = conv::adc_raw_to_volts_uncorrected(average(&data[i]).trunc(), adc_gain[i]);
        let vlsb = volts * DAC_PATH_GAIN / f64::from(DAC_GAIN_CODE);
        gain[i] = DAC_CAL_VLSB / vlsb;
    }
    Ok((offset, gain))
}

/// Back to the power-on values: no correction at all.
pub fn reset(m: &mut M2k) -> Result<()> {
    set_mode(m, "none")?;
    for i in 0..2 {
        set_adc_offset(m, i, DEFAULT_OFFSET)?;
        m.set(ADC, in_chn(i, "calibscale"), 1.0)?;
        m.set(OFFSET_DAC, offset_dac(i), DEFAULT_OFFSET)?;
        m.set(DACS[i], Attr::Device("calibscale"), 1.0)?;
    }
    Ok(())
}

/// Applies the factory calibration table (`cal,temp_lut`) row closest to the board temperature; returns the
/// board temperature and the row. Boards calibrated at the factory with firmware 0.26 or later have one.
pub fn apply_lut(m: &mut M2k) -> Result<(f64, conv::LutRow)> {
    let Some(text) = m.ctx.attr("cal,temp_lut").map(str::to_string) else {
        bail!(
            "this M2K has no factory calibration table (no cal,temp_lut context attribute); use `m2k cal run`"
        );
    };
    let rows = conv::parse_temp_lut(&text)?;
    let temperature = m.temperature()?;
    let Some(row) = conv::nearest_lut_row(&rows, temperature) else {
        bail!("the factory calibration table is empty");
    };
    for i in 0..2 {
        set_adc_offset(m, i, row.adc_offset[i])?;
        m.set(ADC, in_chn(i, "calibscale"), row.adc_gain[i])?;
        m.set(OFFSET_DAC, offset_dac(i), row.dac_offset[i])?;
        m.set(DACS[i], Attr::Device("calibscale"), row.dac_gain[i])?;
    }
    Ok((temperature, row))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibrated_means_not_power_on_values() {
        let mut c = Corrections {
            adc_offset: [Some(2048), None],
            adc_gain: [1.0; 2],
            dac_offset: [2048; 2],
            dac_gain: [1.0; 2],
        };
        assert!(!c.calibrated());
        assert_eq!(c.json()["adc_offset"], json!([2048, null]));
        c.dac_gain[1] = 0.98;
        assert!(c.calibrated());
        assert!(c.text().contains("offset 2048 / unknown"), "{}", c.text());
    }

    #[test]
    fn averages() {
        assert_eq!(average(&[1, 2, 3, -2]), 1.0);
        assert_eq!(average(&[]), 0.0);
    }
}
