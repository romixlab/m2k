//! Board monitor readings (supply rails, temperature, FPGA voltages; not the scope inputs) as typed data.

use crate::device::M2k;
use crate::error::{Result, bail};

#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    /// `device.name` or `device.id`, e.g. `ad9963.temp0`.
    pub channel: String,
    pub device: String,
    pub id: String,
    pub value: f64,
    pub unit: &'static str,
}

/// Every monitor channel whose label contains one of `filter` (all of them when `filter` is empty).
pub fn readings(m: &mut M2k, filter: &[String]) -> Result<Vec<Reading>> {
    let channels = m.dmm_channels();
    if channels.is_empty() {
        bail!("{} has no monitor ADC channels (ad9963, xadc)", m.uri);
    }
    let mut out = vec![];
    for (dev, chn, label) in channels {
        if !filter.is_empty() && !filter.iter().any(|f| label.contains(f.as_str())) {
            continue;
        }
        let (value, unit) = m.dmm_read(&dev, &chn)?;
        out.push(Reading {
            channel: label,
            device: dev,
            id: chn,
            value,
            unit,
        });
    }
    if out.is_empty() {
        bail!("no monitor channel matches {}", filter.join(", "));
    }
    Ok(out)
}
