//! The ADALM2000 on top of the iiod client: which IIO device and attribute does what, in the order libm2k does
//! it (scope inputs, signal outputs, supplies, monitors, digital pins).

use crate::conv::{self, AdcScale, Rail, RailCal, Range};
use anyhow::{Context as _, Result, anyhow, bail};
use iiod_client::{Attr, Client, Context, Device};
use std::time::Duration;

pub const DEFAULT_URI: &str = "ip:192.168.2.1";
/// Connect and attribute timeout; captures extend it by their own length.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

pub const ADC: &str = "m2k-adc";
pub const ADC_TRIGGER: &str = "m2k-adc-trigger";
pub const FABRIC: &str = "m2k-fabric";
/// DAC behind the scope offsets (channels 2, 3) and the signal output offsets (channels 0, 1).
pub const OFFSET_DAC: &str = "ad5625";
/// DAC that sets the user supplies.
pub const SUPPLY_DAC: &str = "ad5627";
/// Its auxiliary ADC measures the user supplies and the board temperature.
pub const MONITOR: &str = "ad9963";
pub const XADC: &str = "xadc";
pub const DACS: [&str; 2] = ["m2k-dac-a", "m2k-dac-b"];
pub const LOGIC: &str = "m2k-logic-analyzer";
pub const LOGIC_RX: &str = "m2k-logic-analyzer-rx";

/// Samples in one capture: buffers hold a multiple of 4 samples, and the device has 512 MB for everything.
pub const BUFFER_STEP: usize = 4;
pub const MAX_SAMPLES: usize = 8_000_000;

/// `ip:HOST[:PORT]` or `HOST[:PORT]` as host and port.
pub fn parse_uri(uri: &str) -> Result<(String, u16)> {
    let rest = match uri.split_once(':') {
        Some(("ip", rest)) => rest,
        Some(("usb" | "serial" | "local", _)) => {
            bail!(
                "`{uri}`: only network contexts are supported (ip:HOST[:PORT]); the M2K is at ip:192.168.2.1 over its USB network"
            )
        }
        _ => uri,
    };
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => (
            host,
            port.parse()
                .with_context(|| format!("`{uri}`: port `{port}` is not a number"))?,
        ),
        _ => (rest, iiod_client::DEFAULT_PORT),
    };
    if host.is_empty() {
        bail!("`{uri}`: no host");
    }
    Ok((host.to_string(), port))
}

/// A connection to an IIO context, with what the M2K commands need on top.
pub struct M2k {
    pub client: Client,
    pub ctx: Context,
    pub uri: String,
}

/// A channel of a device: (id, is output).
pub type Chn<'a> = (&'a str, bool);

fn chn_attr<'a>(chn: Chn<'a>, attr: &'a str) -> Attr<'a> {
    Attr::Channel {
        channel: chn.0,
        output: chn.1,
        attr,
    }
}

impl M2k {
    pub fn connect(uri: &str, timeout: Duration) -> Result<M2k> {
        let (host, port) = parse_uri(uri)?;
        let mut client = Client::connect((host.as_str(), port), timeout)
            .with_context(|| format!("cannot reach iiod at {host}:{port} ({uri}); is the M2K plugged in and its USB network up?"))?;
        let ctx = client.context().context("read the IIO context")?;
        Ok(M2k {
            client,
            ctx,
            uri: uri.to_string(),
        })
    }

    /// Fails with what was found instead when the context is not an ADALM2000.
    pub fn require_m2k(&self) -> Result<()> {
        if self.ctx.device(ADC).is_some() && self.ctx.device(FABRIC).is_some() {
            return Ok(());
        }
        bail!(
            "{} is not an ADALM2000 (hw_model: {}); `m2k info` and `m2k attr` work with any IIO device",
            self.uri,
            self.ctx.attr("hw_model").unwrap_or("unknown")
        )
    }

    /// Powers up what every measurement needs, as libm2k does when it opens an M2K: the scope front end, the
    /// clock, and the gain registers of the AD9963 output path. Harmless to repeat; nothing is powered down at
    /// the end, so the supplies stay as `m2k psu` set them.
    pub fn init(&mut self) -> Result<()> {
        let rev_a = self
            .ctx
            .attr("hw_model")
            .is_some_and(|m| m.contains("Rev.A"));
        let gain = if rev_a { 0x1b } else { 0x05 };
        if self
            .device(MONITOR)?
            .debug_attrs
            .iter()
            .any(|a| a == "direct_reg_access")
        {
            // IGAIN1 (0x68, 0x6B), IGAIN2 (0x69, 0x6C), IRSET (0x6A, 0x6D) of both DAC channels
            for (reg, value) in [
                (0x68, gain),
                (0x6b, gain),
                (0x69, 0x1c),
                (0x6c, 0x1c),
                (0x6a, 0x20),
                (0x6d, 0x20),
            ] {
                self.set(
                    MONITOR,
                    Attr::Debug("direct_reg_access"),
                    format!("0x{reg:x} 0x{value:x}"),
                )?;
            }
        }
        for chn in ["voltage0", "voltage1"] {
            self.set_bool(FABRIC, chn_attr((chn, false), "powerdown"), false)?;
        }
        self.set_bool(FABRIC, Attr::Device("clk_powerdown"), false)
    }

    pub fn device(&self, name: &str) -> Result<&Device> {
        self.ctx
            .device(name)
            .ok_or_else(|| anyhow!("no IIO device `{name}` at {}", self.uri))
    }

    fn id(&self, name: &str) -> Result<String> {
        Ok(self.device(name)?.id.clone())
    }

    pub fn get(&mut self, dev: &str, attr: Attr) -> Result<String> {
        let id = self.id(dev)?;
        Ok(self.client.read_attr(&id, attr)?)
    }

    pub fn set(&mut self, dev: &str, attr: Attr, value: impl std::fmt::Display) -> Result<()> {
        let id = self.id(dev)?;
        Ok(self.client.write_attr(&id, attr, &value.to_string())?)
    }

    pub fn get_f64(&mut self, dev: &str, attr: Attr) -> Result<f64> {
        let text = self.get(dev, attr)?;
        text.parse()
            .with_context(|| format!("{dev} {attr} is `{text}`, not a number"))
    }

    pub fn get_i64(&mut self, dev: &str, attr: Attr) -> Result<i64> {
        Ok(self.get_f64(dev, attr)?.round() as i64)
    }

    /// `1`/`0` attributes.
    pub fn get_bool(&mut self, dev: &str, attr: Attr) -> Result<bool> {
        Ok(self.get_i64(dev, attr)? != 0)
    }

    fn set_bool(&mut self, dev: &str, attr: Attr, value: bool) -> Result<()> {
        self.set(dev, attr, u8::from(value))
    }

    pub fn has_channel_attr(&self, dev: &str, chn: Chn, attr: &str) -> bool {
        self.ctx
            .device(dev)
            .and_then(|d| d.channel(chn.0, chn.1))
            .is_some_and(|c| c.attrs.iter().any(|a| a == attr))
    }

    fn has_attr(&self, dev: &str, attr: &str) -> bool {
        self.ctx
            .device(dev)
            .is_some_and(|d| d.attrs.iter().any(|a| a == attr))
    }

    /// `v0.27` as (0, 27).
    pub fn firmware(&self) -> Option<(u32, u32)> {
        let v = self.ctx.attr("fw_version")?.trim_start_matches('v');
        let (major, minor) = v.split_once('.')?;
        let digits = |s: &str| {
            s.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok()
        };
        Some((digits(major)?, digits(minor)?))
    }

    /// Sample rates a device lists in `sampling_frequency_available`.
    pub fn rates(&mut self, dev: &str) -> Result<Vec<f64>> {
        let text = self.get(dev, Attr::Device("sampling_frequency_available"))?;
        let rates: Vec<f64> = text
            .split_whitespace()
            .filter_map(|r| r.parse().ok())
            .collect();
        if rates.is_empty() {
            bail!("{dev} lists no sample rates (`{text}`)");
        }
        Ok(rates)
    }

    /// Lets a capture of `seconds` finish before the connection gives up on it.
    pub fn allow(&mut self, seconds: f64) -> Result<()> {
        let extra = Duration::from_secs_f64(seconds.clamp(0.0, 3600.0) * 1.5);
        Ok(self.client.set_timeout(DEFAULT_TIMEOUT + extra)?)
    }

    // ---- scope inputs ----

    /// The scope's settings and corrections as the device holds them.
    pub fn adc_state(&mut self) -> Result<AdcState> {
        let sample_rate = self.get_f64(ADC, Attr::Device("sampling_frequency"))?;
        let oversampling = self
            .get_i64(ADC, Attr::Device("oversampling_ratio"))?
            .max(1) as u32;
        let mut channels = [AdcChannel::default(); 2];
        for (i, ch) in channels.iter_mut().enumerate() {
            let chn = adc_chn(i);
            let gain = self.get(FABRIC, chn_attr((chn, false), "gain"))?;
            ch.range = Range::from_gain_name(&gain);
            // calibbias only exists from firmware 0.26 on
            ch.calib_offset = if self.has_channel_attr(ADC, (chn, false), "calibbias") {
                Some(self.get_i64(ADC, chn_attr((chn, false), "calibbias"))?)
            } else {
                None
            };
            ch.calib_gain = self.get_f64(ADC, chn_attr((chn, false), "calibscale"))?;
            ch.offset_raw =
                self.get_i64(OFFSET_DAC, chn_attr((offset_dac_chn(2 + i), true), "raw"))?;
        }
        Ok(AdcState {
            sample_rate,
            oversampling,
            channels,
            host_gain: self.firmware().is_some_and(|fw| fw >= (0, 32)),
        })
    }

    /// Free-running capture settings: no trigger, the given rate and ranges, no vertical offset.
    pub fn adc_configure(
        &mut self,
        sample_rate: f64,
        oversampling: u32,
        ranges: [Range; 2],
    ) -> Result<AdcState> {
        if self.has_attr(ADC, "calibrate") {
            // firmware 0.32 and later: corrections in the gateware
            self.set(ADC, Attr::Device("calibrate"), "true")?;
        }
        for i in 0..2 {
            // trigger_logic channels 4 and 5 decide what starts a capture of scope channel 1 and 2
            let logic = format!("voltage{}", 4 + i);
            self.set(ADC_TRIGGER, chn_attr((&logic, false), "mode"), "always")?;
        }
        self.set(ADC, Attr::Device("sampling_frequency"), sample_rate as u64)?;
        self.set(ADC, Attr::Device("oversampling_ratio"), oversampling)?;
        for (i, range) in ranges.into_iter().enumerate() {
            self.set(
                FABRIC,
                chn_attr((adc_chn(i), false), "gain"),
                range.gain_name(),
            )?;
        }
        let state = self.adc_state()?;
        for (i, ch) in state.channels.iter().enumerate() {
            // offset DAC = vertical offset (0 V) + calibration offset
            if let Some(offset) = ch.calib_offset {
                self.adc_set_offset_raw(i, offset)?;
            }
        }
        self.adc_state()
    }

    pub fn adc_set_offset_raw(&mut self, channel: usize, raw: i64) -> Result<()> {
        self.set(
            OFFSET_DAC,
            chn_attr((offset_dac_chn(2 + channel), true), "raw"),
            raw,
        )
    }

    /// Captures `samples` raw samples of both scope channels with the current settings.
    pub fn adc_capture(
        &mut self,
        samples: usize,
        progress: impl FnMut(usize),
    ) -> Result<[Vec<i64>; 2]> {
        let mut buf = self.adc_open(samples)?;
        let data = buf.read(self, progress);
        buf.close(self);
        data
    }

    /// Opens the scope buffer for repeated captures of `samples` samples; see [`AdcBuffer::read`].
    pub fn adc_open(&mut self, samples: usize) -> Result<AdcBuffer> {
        if samples == 0 || samples > MAX_SAMPLES {
            bail!("a capture takes 1 to {MAX_SAMPLES} samples, not {samples}");
        }
        let dev = self.device(ADC)?.clone();
        let mask = dev.mask_of(&dev.channels);
        let buffered = samples.next_multiple_of(BUFFER_STEP);
        // one kernel buffer: what is read is what was captured after the previous read, nothing older
        let _ = self.client.set_buffers_count(&dev.id, 1);
        self.client.open(&dev.id, buffered, &mask, false)?;
        Ok(AdcBuffer {
            dev,
            mask,
            samples,
            buffered,
        })
    }

    // ---- signal outputs ----

    /// Loads `volts` as a repeating buffer into the outputs in `channels` (0 = W1, 1 = W2) and starts them.
    /// The output lasts as long as this connection: iiod frees the buffers when it ends.
    pub fn dac_start(&mut self, channels: &[usize], sample_rate: f64, volts: &[f64]) -> Result<()> {
        let compensation = conv::dac_filter_compensation(sample_rate)
            .ok_or_else(|| anyhow!("no filter compensation for a DAC rate of {sample_rate} S/s"))?;
        let raw: Vec<i16> = volts
            .iter()
            .map(|v| conv::dac_volts_to_raw(*v, compensation))
            .collect();
        self.dac_start_raw(channels, sample_rate, &raw)
    }

    /// The same with samples as the DAC takes them (12 bits, left aligned, inverted).
    pub fn dac_start_raw(
        &mut self,
        channels: &[usize],
        sample_rate: f64,
        raw: &[i16],
    ) -> Result<()> {
        if !raw.len().is_multiple_of(BUFFER_STEP) || raw.len() < 16 {
            bail!(
                "an output buffer holds a multiple of 4 samples, at least 16, not {}",
                raw.len()
            );
        }
        for ch in channels {
            let dac = dac_name(*ch)?;
            self.set(dac, Attr::Device("sampling_frequency"), sample_rate as u64)?;
            self.set(dac, Attr::Device("oversampling_ratio"), 1)?;
            // hold the DMA until every channel has its data, so they start together
            self.set_bool(dac, Attr::Device("dma_sync"), true)?;
        }
        for ch in channels {
            let dev = self.device(dac_name(*ch)?)?.clone();
            let chn = dev
                .channels
                .iter()
                .find(|c| c.output && c.scan.is_some())
                .ok_or_else(|| anyhow!("{} has no buffered output channel", dev.label()))?;
            let format = chn.scan.map(|s| s.format).unwrap_or(I16_LE);
            let mut data = Vec::with_capacity(raw.len() * 2);
            for sample in raw {
                format.encode(i64::from(*sample), &mut data);
            }
            let mask = dev.mask_of([chn]);
            let _ = self.client.close(&dev.id);
            self.client.open(&dev.id, raw.len(), &mask, true)?;
            self.client.write_buf(&dev.id, &data)?;
        }
        for ch in channels {
            self.set_bool(dac_name(*ch)?, Attr::Device("dma_sync"), false)?;
        }
        for ch in channels {
            self.set_bool(
                FABRIC,
                chn_attr((dac_fabric_chn(*ch), true), "powerdown"),
                false,
            )?;
        }
        Ok(())
    }

    /// Switches an output off: amplifier powered down, buffer freed.
    pub fn dac_stop(&mut self, channel: usize) -> Result<()> {
        let dac = dac_name(channel)?;
        self.set_bool(
            FABRIC,
            chn_attr((dac_fabric_chn(channel), true), "powerdown"),
            true,
        )?;
        self.set_bool(dac, Attr::Device("dma_sync"), true)?;
        let id = self.id(dac)?;
        // EBADF when this connection has no buffer open: nothing to free
        let _ = self.client.close(&id);
        if self.has_channel_attr(dac, ("voltage0", true), "raw_enable") {
            // firmware 0.32 and later: what the output falls back to without a buffer
            self.set(dac, chn_attr(("voltage0", true), "raw_enable"), "enabled")?;
            self.set(dac, chn_attr(("voltage0", true), "raw"), 0)?;
        }
        Ok(())
    }

    /// Whether the output amplifier of a channel is on.
    pub fn dac_enabled(&mut self, channel: usize) -> Result<bool> {
        Ok(!self.get_bool(
            FABRIC,
            chn_attr((dac_fabric_chn(channel), true), "powerdown"),
        )?)
    }

    // ---- user supplies ----

    /// Factory corrections of a rail: `write` for setting it, `read` for measuring it.
    pub fn rail_cal(&self, rail: Rail) -> (RailCal, RailCal) {
        let side = match rail {
            Rail::Pos => "pos",
            Rail::Neg => "neg",
        };
        let cal = |kind: &str| {
            let get = |what: &str| {
                self.ctx
                    .attr(&format!("cal,{what}_{side}_{kind}"))
                    .and_then(|v| v.parse::<f64>().ok())
            };
            match (get("offset"), get("gain")) {
                (Some(offset), Some(gain)) => RailCal { offset, gain },
                _ => RailCal::IDENTITY,
            }
        };
        (cal("dac"), cal("adc"))
    }

    /// The two rails share one enable on early boards; later ones have one each.
    fn individual_supply_enable(&self) -> bool {
        self.has_channel_attr(FABRIC, ("voltage3", true), "user_supply_powerdown")
    }

    /// On when both the supply DAC and the rail's switch in the fabric are on. After power-up the DAC is on
    /// (at code 2048) and only the fabric switch is off, so the DAC alone says nothing.
    pub fn psu_enabled(&mut self, rail: Rail) -> Result<bool> {
        let dac_on = !self.get_bool(SUPPLY_DAC, chn_attr((rail_chn(rail), true), "powerdown"))?;
        let switch = match rail {
            Rail::Neg if self.individual_supply_enable() => "voltage3",
            _ => "voltage2",
        };
        let switch_on =
            !self.get_bool(FABRIC, chn_attr((switch, true), "user_supply_powerdown"))?;
        Ok(dac_on && switch_on)
    }

    /// Sets a rail's voltage; it comes out once the rail is enabled.
    pub fn psu_set(&mut self, rail: Rail, volts: f64) -> Result<i64> {
        let raw = conv::psu_volts_to_raw(rail, volts, self.rail_cal(rail).0)?;
        self.set(SUPPLY_DAC, chn_attr((rail_chn(rail), true), "raw"), raw)?;
        Ok(raw)
    }

    pub fn psu_enable(&mut self, rail: Rail, on: bool) -> Result<()> {
        self.set_bool(
            SUPPLY_DAC,
            chn_attr((rail_chn(rail), true), "powerdown"),
            !on,
        )?;
        if self.individual_supply_enable() {
            let chn = match rail {
                Rail::Pos => "voltage2",
                Rail::Neg => "voltage3",
            };
            self.set_bool(FABRIC, chn_attr((chn, true), "user_supply_powerdown"), !on)?;
        } else {
            let other = match rail {
                Rail::Pos => Rail::Neg,
                Rail::Neg => Rail::Pos,
            };
            if on || !self.psu_enabled(other)? {
                self.set_bool(
                    FABRIC,
                    chn_attr(("voltage2", true), "user_supply_powerdown"),
                    !on,
                )?;
            }
        }
        Ok(())
    }

    /// The voltage measured on a rail.
    pub fn psu_read(&mut self, rail: Rail) -> Result<f64> {
        let chn = match rail {
            Rail::Pos => "voltage2",
            Rail::Neg => "voltage1",
        };
        let raw = self.get_f64(MONITOR, chn_attr((chn, false), "raw"))?;
        Ok(conv::psu_raw_to_volts(rail, raw, self.rail_cal(rail).1))
    }

    // ---- monitors ----

    /// Input channels of the monitor ADCs that can be read directly: (device, channel id, label).
    pub fn dmm_channels(&self) -> Vec<(String, String, String)> {
        let mut out = vec![];
        for dev in [MONITOR, XADC] {
            let Some(device) = self.ctx.device(dev) else {
                continue;
            };
            for c in device.channels.iter().filter(|c| !c.output) {
                if c.attrs
                    .iter()
                    .any(|a| a == "raw" || a == "input" || a == "processed")
                {
                    let label = match &c.name {
                        Some(name) => format!("{dev}.{name}"),
                        None => format!("{dev}.{}", c.id),
                    };
                    out.push((dev.to_string(), c.id.clone(), label));
                }
            }
        }
        out
    }

    /// One monitor channel in volts or °C: (raw + offset) × scale / 1000, as libm2k's DMM does.
    pub fn dmm_read(&mut self, dev: &str, chn: &str) -> Result<(f64, &'static str)> {
        let has = |attr: &str| self.has_channel_attr(dev, (chn, false), attr);
        let value_attr = ["raw", "processed", "input"]
            .into_iter()
            .find(|a| has(a))
            .ok_or_else(|| anyhow!("{dev} {chn} has no value to read"))?;
        let (has_offset, has_scale) = (has("offset"), has("scale"));
        let value = self.get_f64(dev, chn_attr((chn, false), value_attr))?;
        let offset = if has_offset {
            self.get_f64(dev, chn_attr((chn, false), "offset"))?
        } else {
            0.0
        };
        let scale = if has_scale {
            self.get_f64(dev, chn_attr((chn, false), "scale"))?
        } else {
            1.0
        };
        let unit = if chn.starts_with("temp") {
            "°C"
        } else if chn.starts_with("current") {
            "A"
        } else {
            "V"
        };
        Ok(((value + offset) * scale * 0.001, unit))
    }

    /// Board temperature next to the converters, in °C.
    pub fn temperature(&mut self) -> Result<f64> {
        Ok(self.dmm_read(MONITOR, "temp0")?.0)
    }

    // ---- digital pins ----

    /// Free-running capture of the 16 digital pins: one `u16` per sample, bit N = DIO N.
    pub fn logic_capture(
        &mut self,
        samples: usize,
        sample_rate: f64,
        progress: impl FnMut(usize),
    ) -> Result<(Vec<u16>, f64)> {
        if samples == 0 || samples > MAX_SAMPLES {
            bail!("a capture takes 1 to {MAX_SAMPLES} samples, not {samples}");
        }
        let dev = self.device(LOGIC_RX)?.clone();
        for c in dev
            .channels
            .iter()
            .filter(|c| c.attrs.iter().any(|a| a == "trigger"))
        {
            self.set(LOGIC_RX, chn_attr((&c.id, false), "trigger"), "none")?;
        }
        self.set(
            LOGIC_RX,
            Attr::Device("sampling_frequency"),
            sample_rate as u64,
        )?;
        let actual = self.get_f64(LOGIC_RX, Attr::Device("sampling_frequency"))?;
        self.allow(samples as f64 / actual.max(1.0))?;
        let pins: Vec<_> = dev.channels.iter().filter(|c| c.scan.is_some()).collect();
        let mask = dev.mask_of(pins.iter().copied());
        let layout = dev.layout(&mask);
        if layout.sample_size != 2 {
            bail!(
                "{} samples are {} bytes, expected 2",
                dev.label(),
                layout.sample_size
            );
        }
        let buffered = samples.next_multiple_of(BUFFER_STEP);
        let _ = self.client.set_buffers_count(&dev.id, 1);
        self.client.open(&dev.id, buffered, &mask, false)?;
        let read = self
            .client
            .read_buf(&dev.id, buffered * 2, mask.len(), progress);
        let closed = self.client.close(&dev.id);
        let (_, data) = read?;
        closed?;
        if data.len() < samples * 2 {
            bail!(
                "{} sent {} of {} samples",
                dev.label(),
                data.len() / 2,
                samples
            );
        }
        let words = data
            .as_chunks::<2>()
            .0
            .iter()
            .take(samples)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        Ok((words, actual))
    }

    /// `in` or `out` for each of the 16 pins.
    pub fn logic_directions(&mut self) -> Result<Vec<String>> {
        (0..16)
            .map(|pin| {
                let chn = format!("voltage{pin}");
                self.get(LOGIC, chn_attr((&chn, false), "direction"))
            })
            .collect()
    }
}

const I16_LE: iiod_client::DataFormat = iiod_client::DataFormat {
    big_endian: false,
    signed: true,
    bits: 16,
    storage_bits: 16,
    shift: 0,
    repeat: 1,
};

fn adc_chn(channel: usize) -> &'static str {
    ["voltage0", "voltage1"][channel.min(1)]
}

fn offset_dac_chn(index: usize) -> &'static str {
    ["voltage0", "voltage1", "voltage2", "voltage3"][index.min(3)]
}

fn dac_fabric_chn(channel: usize) -> &'static str {
    ["voltage0", "voltage1"][channel.min(1)]
}

fn rail_chn(rail: Rail) -> &'static str {
    match rail {
        Rail::Pos => "voltage0",
        Rail::Neg => "voltage1",
    }
}

fn dac_name(channel: usize) -> Result<&'static str> {
    DACS.get(channel)
        .copied()
        .ok_or_else(|| anyhow!("no output channel {}", channel + 1))
}

/// An open scope buffer.
pub struct AdcBuffer {
    dev: Device,
    mask: Vec<u32>,
    samples: usize,
    buffered: usize,
}

impl AdcBuffer {
    /// The next `samples` raw samples of both channels. With a free-running scope that is what was captured
    /// right after the previous read (or after opening).
    pub fn read(&mut self, m2k: &mut M2k, progress: impl FnMut(usize)) -> Result<[Vec<i64>; 2]> {
        let size = self.dev.layout(&self.mask).sample_size;
        let (got, data) = m2k.client.read_buf(
            &self.dev.id,
            self.buffered * size,
            self.mask.len(),
            progress,
        )?;
        // what the server says is in the data counts, when it makes sense (iio-emu reports junk for some devices)
        let valid: Vec<u32> = got.iter().zip(&self.mask).map(|(g, m)| g & m).collect();
        let mask = if valid.iter().any(|w| *w != 0) {
            valid
        } else {
            self.mask.clone()
        };
        let layout = self.dev.layout(&mask);
        let mut out = [vec![], vec![]];
        for (i, ch) in out.iter_mut().enumerate() {
            let index = self
                .dev
                .channels
                .iter()
                .position(|c| !c.output && c.id == adc_chn(i))
                .ok_or_else(|| anyhow!("{} has no channel {}", self.dev.label(), adc_chn(i)))?;
            *ch = layout
                .extract(index, &data)
                .ok_or_else(|| anyhow!("{} did not capture {}", self.dev.label(), adc_chn(i)))?;
            if ch.len() < self.samples {
                bail!(
                    "{} sent {} of {} samples",
                    self.dev.label(),
                    ch.len(),
                    self.samples
                );
            }
            ch.truncate(self.samples);
        }
        Ok(out)
    }

    pub fn close(self, m2k: &mut M2k) {
        let _ = m2k.client.close(&self.dev.id);
    }
}

/// Scope settings and corrections read from the device.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdcState {
    /// `sampling_frequency`, before `oversampling` divides it.
    pub sample_rate: f64,
    pub oversampling: u32,
    pub channels: [AdcChannel; 2],
    /// Firmware 0.32 and later: `calibscale` is applied to the samples here, as libm2k does.
    pub host_gain: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdcChannel {
    pub range: Range,
    /// Offset correction (`calibbias`); `None` on firmware before 0.26, which does not keep it.
    pub calib_offset: Option<i64>,
    /// Gain correction (`calibscale`).
    pub calib_gain: f64,
    /// What the offset DAC is set to: calibration offset plus vertical offset.
    pub offset_raw: i64,
}

impl Default for AdcChannel {
    fn default() -> Self {
        AdcChannel {
            range: Range::V25,
            calib_offset: None,
            calib_gain: 1.0,
            offset_raw: conv::DEFAULT_OFFSET,
        }
    }
}

impl AdcState {
    /// Samples per second that come out.
    pub fn rate(&self) -> f64 {
        self.sample_rate / f64::from(self.oversampling.max(1))
    }

    /// Neither offset nor gain differ from the power-on values: the scope was not calibrated since power-up.
    pub fn uncalibrated(&self) -> bool {
        self.channels.iter().all(|c| {
            c.calib_offset.unwrap_or(conv::DEFAULT_OFFSET) == conv::DEFAULT_OFFSET
                && c.calib_gain == 1.0
        })
    }

    pub fn scale(&self, channel: usize) -> Result<AdcScale> {
        let ch = self.channels[channel.min(1)];
        let filter_compensation =
            conv::adc_filter_compensation(self.sample_rate).ok_or_else(|| {
                anyhow!(
                    "no filter compensation for an ADC rate of {} S/s",
                    self.sample_rate
                )
            })?;
        let offset_steps = ch.offset_raw - ch.calib_offset.unwrap_or(ch.offset_raw);
        Ok(AdcScale {
            range: ch.range,
            gain: if self.host_gain { ch.calib_gain } else { 1.0 },
            filter_compensation,
            vertical_offset: conv::vertical_offset_to_volts(offset_steps, ch.range),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_uris() {
        assert_eq!(
            parse_uri("ip:192.168.2.1").unwrap(),
            ("192.168.2.1".into(), 30431)
        );
        assert_eq!(
            parse_uri("ip:127.0.0.1:30555").unwrap(),
            ("127.0.0.1".into(), 30555)
        );
        assert_eq!(parse_uri("m2k.local").unwrap(), ("m2k.local".into(), 30431));
        assert_eq!(
            parse_uri("localhost:1234").unwrap(),
            ("localhost".into(), 1234)
        );
        assert!(parse_uri("usb:1.2.3").is_err());
        assert!(parse_uri("ip:host:port").is_err());
        assert!(parse_uri("ip:").is_err());
    }

    #[test]
    fn adc_state_scales() {
        let mut state = AdcState {
            sample_rate: 1e6,
            oversampling: 4,
            channels: [AdcChannel::default(); 2],
            host_gain: false,
        };
        assert_eq!(state.rate(), 250e3);
        assert!(state.uncalibrated());
        state.channels[0] = AdcChannel {
            range: Range::V2_5,
            calib_offset: Some(2040),
            calib_gain: 1.02,
            offset_raw: 2040,
        };
        assert!(!state.uncalibrated());
        let scale = state.scale(0).unwrap();
        assert_eq!(
            (scale.gain, scale.filter_compensation, scale.vertical_offset),
            (1.0, 1.10, 0.0)
        );
        state.host_gain = true;
        assert_eq!(state.scale(0).unwrap().gain, 1.02);
        state.sample_rate = 123.0;
        assert!(state.scale(0).is_err());
    }
}
