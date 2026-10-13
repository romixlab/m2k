//! Python module for the Analog Devices ADALM2000 (M2K): scope, signal generator, power supplies, voltmeter,
//! logic analyzer, calibration, on top of the `m2k` Rust library. See `m2k.pyi` for the full signatures.
//!
//! Waveforms for the generator are plain float lists (build them with numpy and pass the samples directly),
//! rather than the shape enum the CLI parses from `m2k gen sine -f ...` flags.

use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use std::time::Duration;

pyo3::create_exception!(m2k, M2kError, PyException);

fn err(e: m2k_lib::Error) -> PyErr {
    M2kError::new_err(e.to_string())
}

/// Summary of an analog capture of one channel.
#[pyclass(get_all, skip_from_py_object)]
#[derive(Clone)]
struct Analog {
    samples: usize,
    min: f64,
    max: f64,
    mean: f64,
    rms: f64,
    ac_rms: f64,
    vpp: f64,
    frequency: Option<f64>,
    periods: usize,
    duty: Option<f64>,
    clipped: usize,
}

impl From<&m2k_lib::stats::Analog> for Analog {
    fn from(a: &m2k_lib::stats::Analog) -> Self {
        Analog {
            samples: a.samples,
            min: a.min,
            max: a.max,
            mean: a.mean,
            rms: a.rms,
            ac_rms: a.ac_rms,
            vpp: a.vpp(),
            frequency: a.frequency,
            periods: a.periods,
            duty: a.duty,
            clipped: a.clipped,
        }
    }
}

/// A capture of both scope channels, in volts.
#[pyclass(get_all, skip_from_py_object)]
struct ScopeCapture {
    rate: f64,
    ch1: Vec<f64>,
    ch2: Vec<f64>,
    ch1_stats: Analog,
    ch2_stats: Analog,
    range1: f64,
    range2: f64,
    calibrated: bool,
}

impl From<m2k_lib::scope::Capture> for ScopeCapture {
    fn from(c: m2k_lib::scope::Capture) -> Self {
        ScopeCapture {
            rate: c.rate,
            ch1_stats: Analog::from(&c.stats[0]),
            ch2_stats: Analog::from(&c.stats[1]),
            range1: c.ranges[0].limit(),
            range2: c.ranges[1].limit(),
            calibrated: c.calibrated,
            ch1: c.volts[0].clone(),
            ch2: c.volts[1].clone(),
        }
    }
}

/// Summary of one digital pin over a [`LogicCapture`].
#[pyclass(get_all, skip_from_py_object)]
#[derive(Clone)]
struct Digital {
    channel: u32,
    state: String,
    rising: usize,
    falling: usize,
    high: f64,
    frequency: Option<f64>,
    min_pulse: Option<f64>,
}

impl From<&m2k_lib::stats::Digital> for Digital {
    fn from(d: &m2k_lib::stats::Digital) -> Self {
        Digital {
            channel: d.channel,
            state: d.state().to_string(),
            rising: d.rising,
            falling: d.falling,
            high: d.high,
            frequency: d.frequency,
            min_pulse: d.min_pulse,
        }
    }
}

/// A capture of the 16 digital pins, summarized per pin asked for.
#[pyclass(get_all, skip_from_py_object)]
struct LogicCapture {
    rate: f64,
    /// One integer per sample, bit N = DIO N.
    words: Vec<u16>,
    pins: Vec<Digital>,
}

/// Measured voltage and enabled state of both user supply rails.
#[pyclass(get_all, skip_from_py_object)]
#[derive(Clone, Copy)]
struct PsuStatus {
    pos_enabled: bool,
    pos_measured: f64,
    neg_enabled: bool,
    neg_measured: f64,
}

impl From<m2k_lib::psu::Status> for PsuStatus {
    fn from(s: m2k_lib::psu::Status) -> Self {
        PsuStatus {
            pos_enabled: s.pos.enabled,
            pos_measured: s.pos.measured,
            neg_enabled: s.neg.enabled,
            neg_measured: s.neg.measured,
        }
    }
}

/// One board monitor reading (`M2k.dmm`): a supply rail, temperature or FPGA voltage.
#[pyclass(get_all, skip_from_py_object)]
#[derive(Clone)]
struct DmmReading {
    channel: String,
    value: f64,
    unit: String,
}

impl From<m2k_lib::dmm::Reading> for DmmReading {
    fn from(r: m2k_lib::dmm::Reading) -> Self {
        DmmReading {
            channel: r.channel,
            value: r.value,
            unit: r.unit.to_string(),
        }
    }
}

/// The corrections the M2K holds after `cal_run`/`cal_status`/`cal_reset`.
#[pyclass(get_all, skip_from_py_object)]
#[derive(Clone)]
struct Corrections {
    calibrated: bool,
    /// Scope offset per channel ([ch1, ch2]); `None` on firmware before 0.26.
    adc_offset: Vec<Option<i64>>,
    adc_gain: Vec<f64>,
    dac_offset: Vec<i64>,
    dac_gain: Vec<f64>,
}

impl From<m2k_lib::cal::Corrections> for Corrections {
    fn from(c: m2k_lib::cal::Corrections) -> Self {
        Corrections {
            calibrated: c.calibrated(),
            adc_offset: c.adc_offset.to_vec(),
            adc_gain: c.adc_gain.to_vec(),
            dac_offset: c.dac_offset.to_vec(),
            dac_gain: c.dac_gain.to_vec(),
        }
    }
}

/// Both LEDs after `M2k.led_status`/`led_off`/`led_on`; `None` for an LED that was not asked for.
#[pyclass(get_all, skip_from_py_object)]
#[derive(Clone)]
struct LedStatus {
    /// The steady DONE LED is on.
    done: Option<bool>,
    /// The blinking Ready LED is on; `None` also when ssh could not reach it (see `ready_error`).
    ready: Option<bool>,
    /// Its kernel trigger: `heartbeat` (blinking) or `none`.
    ready_trigger: Option<String>,
    /// Why ssh could not reach the Ready LED; it was not changed then.
    ready_error: Option<String>,
}

impl From<m2k_lib::led::Status> for LedStatus {
    fn from(s: m2k_lib::led::Status) -> Self {
        let (ready, ready_trigger, ready_error) = match s.ready {
            Some(Ok(r)) => (Some(r.on), Some(r.trigger), None),
            Some(Err(e)) => (None, None, Some(e.to_string())),
            None => (None, None, None),
        };
        LedStatus {
            done: s.done,
            ready,
            ready_trigger,
            ready_error,
        }
    }
}

fn led_of(only: Option<&str>) -> PyResult<Option<m2k_lib::led::Led>> {
    match only {
        None => Ok(None),
        Some("done") => Ok(Some(m2k_lib::led::Led::Done)),
        Some("ready") => Ok(Some(m2k_lib::led::Led::Ready)),
        Some(other) => Err(M2kError::new_err(format!(
            "only=`{other}`: the LEDs are `done` and `ready`"
        ))),
    }
}

fn range_of(volts: f64) -> m2k_lib::conv::Range {
    if volts <= 2.5 {
        m2k_lib::conv::Range::V2_5
    } else {
        m2k_lib::conv::Range::V25
    }
}

/// A connection to an ADALM2000.
#[pyclass]
struct M2k(m2k_lib::M2k, Duration);

impl M2k {
    /// How `led_*` reach the shell on the M2K: the `m2k` CLI on PATH is the askpass helper.
    fn ssh(
        &self,
        password: Option<String>,
        known_hosts: Option<std::path::PathBuf>,
    ) -> m2k_lib::led::Ssh {
        let mut ssh = m2k_lib::led::Ssh::new(self.1);
        ssh.password = match password {
            Some(p) => m2k_lib::led::Password::new(p),
            None => m2k_lib::led::Password::from_env(),
        };
        ssh.known_hosts = known_hosts;
        ssh
    }

    fn led_set(
        &mut self,
        on: bool,
        only: Option<&str>,
        ssh_password: Option<String>,
        ssh_known_hosts: Option<std::path::PathBuf>,
    ) -> PyResult<LedStatus> {
        let ssh = self.ssh(ssh_password, ssh_known_hosts);
        m2k_lib::led::set(&mut self.0, on, led_of(only)?, &ssh)
            .map(LedStatus::from)
            .map_err(err)
    }
}

#[pymethods]
impl M2k {
    /// Connects to the M2K at `uri` (`ip:192.168.2.1` when omitted) and powers up the scope front end, as
    /// libm2k does when it opens an M2K.
    #[new]
    #[pyo3(signature = (uri=None, timeout=5.0))]
    fn new(uri: Option<&str>, timeout: f64) -> PyResult<Self> {
        let uri = uri.unwrap_or(m2k_lib::DEFAULT_URI);
        let timeout = Duration::try_from_secs_f64(timeout)
            .map_err(|e| M2kError::new_err(format!("timeout: {e}")))?;
        let mut dev = m2k_lib::M2k::connect(uri, timeout).map_err(err)?;
        dev.require_m2k().map_err(err)?;
        dev.init().map_err(err)?;
        Ok(M2k(dev, timeout))
    }

    /// Board temperature next to the converters, in °C.
    fn temperature(&mut self) -> PyResult<f64> {
        self.0.temperature().map_err(err)
    }

    /// Captures `samples` samples per channel at (close to) `rate` S/s, at `range1`/`range2` volts (25 or 2.5;
    /// `range2` defaults to `range1`).
    #[pyo3(signature = (samples, rate, range1=25.0, range2=None))]
    fn scope(
        &mut self,
        samples: usize,
        rate: f64,
        range1: f64,
        range2: Option<f64>,
    ) -> PyResult<ScopeCapture> {
        let ranges = [range_of(range1), range_of(range2.unwrap_or(range1))];
        let cap =
            m2k_lib::scope::capture(&mut self.0, samples, rate, ranges, |_| {}).map_err(err)?;
        Ok(ScopeCapture::from(cap))
    }

    /// DC and AC rms on both channels, at `range` volts (25 or 2.5).
    #[pyo3(signature = (range=25.0))]
    fn voltmeter(&mut self, range: f64) -> PyResult<ScopeCapture> {
        let cap = m2k_lib::scope::voltmeter(&mut self.0, range_of(range), |_| {}).map_err(err)?;
        Ok(ScopeCapture::from(cap))
    }

    /// Captures `samples` samples of the 16 digital pins at (close to) `rate` S/s, summarized for `pins`
    /// (all 16 when omitted).
    #[pyo3(signature = (samples, rate, pins=None))]
    fn logic(
        &mut self,
        samples: usize,
        rate: f64,
        pins: Option<Vec<u32>>,
    ) -> PyResult<LogicCapture> {
        let pins = pins.unwrap_or_else(|| (0..16).collect());
        let trigger = m2k_lib::LogicTrigger::default();
        let cap = m2k_lib::logic::capture(&mut self.0, samples, rate, &pins, &trigger, |_| {})
            .map_err(err)?;
        Ok(LogicCapture {
            rate: cap.rate,
            words: cap.words,
            pins: cap.pins.iter().map(Digital::from).collect(),
        })
    }

    /// Loads `volts` as a repeating buffer into `channels` (0 = W1, 1 = W2) and starts them; the output lasts
    /// until `gen_stop` or the connection closes.
    fn gen_start(&mut self, channels: Vec<usize>, rate: f64, volts: Vec<f64>) -> PyResult<()> {
        self.0.dac_start(&channels, rate, &volts).map_err(err)
    }

    /// Switches an output off: amplifier powered down, buffer freed.
    fn gen_stop(&mut self, channel: usize) -> PyResult<()> {
        self.0.dac_stop(channel).map_err(err)
    }

    /// Measured voltage and enabled state of both rails.
    fn psu_status(&mut self) -> PyResult<PsuStatus> {
        m2k_lib::psu::status(&mut self.0)
            .map(PsuStatus::from)
            .map_err(err)
    }

    /// Sets and switches on the given rails (`None` leaves a rail as it is).
    #[pyo3(signature = (pos=None, neg=None))]
    fn psu_set(&mut self, pos: Option<f64>, neg: Option<f64>) -> PyResult<PsuStatus> {
        m2k_lib::psu::set(&mut self.0, pos, neg)
            .map(PsuStatus::from)
            .map_err(err)
    }

    /// Switches rails off (both when neither `pos` nor `neg` is set).
    #[pyo3(signature = (pos=false, neg=false))]
    fn psu_off(&mut self, pos: bool, neg: bool) -> PyResult<PsuStatus> {
        m2k_lib::psu::off(&mut self.0, pos, neg)
            .map(PsuStatus::from)
            .map_err(err)
    }

    /// Every monitor channel (supply rails, temperature, FPGA voltages) whose label contains one of
    /// `filter` (all of them when omitted).
    #[pyo3(signature = (filter=None))]
    fn dmm(&mut self, filter: Option<Vec<String>>) -> PyResult<Vec<DmmReading>> {
        let filter = filter.unwrap_or_default();
        m2k_lib::dmm::readings(&mut self.0, &filter)
            .map(|rs| rs.into_iter().map(DmmReading::from).collect())
            .map_err(err)
    }

    /// Self-calibrates (about 5 s): scope inputs, then the outputs too unless `adc_only` (W1/W2 are driven to
    /// ~2.5 V for a moment).
    #[pyo3(signature = (adc_only=false))]
    fn cal_run(&mut self, adc_only: bool) -> PyResult<Corrections> {
        m2k_lib::cal::run(&mut self.0, adc_only, &mut |_| {})
            .map(Corrections::from)
            .map_err(err)
    }

    /// The corrections the M2K currently holds.
    fn cal_status(&mut self) -> PyResult<Corrections> {
        m2k_lib::cal::Corrections::read(&mut self.0)
            .map(Corrections::from)
            .map_err(err)
    }

    /// Back to the power-on values: no correction at all.
    fn cal_reset(&mut self) -> PyResult<Corrections> {
        m2k_lib::cal::reset(&mut self.0).map_err(err)?;
        m2k_lib::cal::Corrections::read(&mut self.0)
            .map(Corrections::from)
            .map_err(err)
    }

    /// The two blue LEDs as they are now (`only`: `"done"` or `"ready"`).
    #[pyo3(signature = (only=None, ssh_password=None, ssh_known_hosts=None))]
    fn led_status(
        &mut self,
        only: Option<&str>,
        ssh_password: Option<String>,
        ssh_known_hosts: Option<std::path::PathBuf>,
    ) -> PyResult<LedStatus> {
        let ssh = self.ssh(ssh_password, ssh_known_hosts);
        m2k_lib::led::status(&mut self.0, led_of(only)?, &ssh)
            .map(LedStatus::from)
            .map_err(err)
    }

    /// Switches the LEDs off until the M2K is power cycled.
    #[pyo3(signature = (only=None, ssh_password=None, ssh_known_hosts=None))]
    fn led_off(
        &mut self,
        only: Option<&str>,
        ssh_password: Option<String>,
        ssh_known_hosts: Option<std::path::PathBuf>,
    ) -> PyResult<LedStatus> {
        self.led_set(false, only, ssh_password, ssh_known_hosts)
    }

    /// Switches the LEDs back on.
    #[pyo3(signature = (only=None, ssh_password=None, ssh_known_hosts=None))]
    fn led_on(
        &mut self,
        only: Option<&str>,
        ssh_password: Option<String>,
        ssh_known_hosts: Option<std::path::PathBuf>,
    ) -> PyResult<LedStatus> {
        self.led_set(true, only, ssh_password, ssh_known_hosts)
    }
}

#[pymodule]
fn m2k(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("M2kError", m.py().get_type::<M2kError>())?;
    m.add_class::<M2k>()?;
    m.add_class::<ScopeCapture>()?;
    m.add_class::<Analog>()?;
    m.add_class::<LogicCapture>()?;
    m.add_class::<Digital>()?;
    m.add_class::<PsuStatus>()?;
    m.add_class::<DmmReading>()?;
    m.add_class::<Corrections>()?;
    m.add_class::<LedStatus>()?;
    Ok(())
}
