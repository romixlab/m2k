//! The two user supply rails, V+ (0 to 5 V) and V- (-5 to 0 V), as typed data.

use crate::conv::{self, Rail};
use crate::device::M2k;
use crate::error::{Result, bail};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RailStatus {
    pub enabled: bool,
    pub measured: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Status {
    pub pos: RailStatus,
    pub neg: RailStatus,
}

fn read(m: &mut M2k, rail: Rail) -> Result<RailStatus> {
    Ok(RailStatus {
        enabled: m.psu_enabled(rail)?,
        measured: m.psu_read(rail)?,
    })
}

/// Measured voltage and enabled state of both rails.
pub fn status(m: &mut M2k) -> Result<Status> {
    Ok(Status {
        pos: read(m, Rail::Pos)?,
        neg: read(m, Rail::Neg)?,
    })
}

/// Sets and switches on the given rails (`None` leaves a rail as it is), then waits for the measurement to
/// settle and reads it back.
pub fn set(m: &mut M2k, pos: Option<f64>, neg: Option<f64>) -> Result<Status> {
    if pos.is_none() && neg.is_none() {
        bail!("give a V+ and/or V- value");
    }
    for (rail, volts) in [(Rail::Pos, pos), (Rail::Neg, neg)] {
        if let Some(v) = volts {
            // check both before switching anything on
            conv::psu_volts_to_raw(rail, v, conv::RailCal::IDENTITY)?;
        }
    }
    for (rail, volts) in [(Rail::Pos, pos), (Rail::Neg, neg)] {
        if let Some(v) = volts {
            m.psu_set(rail, v)?;
            m.psu_enable(rail, true)?;
        }
    }
    // the rails settle within a few ms; the monitor ADC needs a moment too
    std::thread::sleep(Duration::from_millis(200));
    status(m)
}

/// Switches rails off (both when neither `pos` nor `neg` is set) and zeroes them, then reads them back.
pub fn off(m: &mut M2k, pos: bool, neg: bool) -> Result<Status> {
    let both = !pos && !neg;
    for (rail, chosen) in [(Rail::Pos, pos), (Rail::Neg, neg)] {
        if both || chosen {
            m.psu_enable(rail, false)?;
            m.psu_set(rail, 0.0)?;
        }
    }
    status(m)
}
