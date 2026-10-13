//! The command tree (clap derive) and the command handlers; `--json` output goes through [`Out::print`]. The
//! M2K's devices and the typed calls behind each command (capture, calibration, ...) live in the `m2k` crate;
//! this module parses arguments, calls into it, and formats text/JSON/CSV/VCD.

use crate::{complete, parse};
use anyhow::{Context as _, Result, anyhow, bail};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::ArgValueCandidates;
use iiod_client::Attr;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use m2k::cal::{self, Corrections};
use m2k::conv::{Rail, Range};
use m2k::{LogicTrigger, M2k, TriggerCondition, device, dmm, led, logic, psu, scope, stats, wave};
use serde_json::{Value, json};
use std::io::{IsTerminal, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("GIT_SHA"),
    ", built ",
    env!("BUILD_TIME"),
    ")"
);

#[derive(Parser, Debug)]
#[command(name = "m2k", version = VERSION, about, long_about = "\
Drive an Analog Devices ADALM2000 (M2K) over the network: scope, signal generator, power supplies, voltmeter,
logic analyzer. Talks to the iiod daemon on the M2K directly (pure Rust, no libiio).

The M2K shows up as a USB network adapter and answers at 192.168.2.1. Typical use: `m2k info`, then
`m2k scope -n 10k -r 1M`. Shell completion: `source <(COMPLETE=bash m2k)` (or zsh, fish).")]
struct Cli {
    /// Device: ip:HOST[:PORT] or HOST[:PORT].
    #[arg(long, short = 'u', global = true, env = "M2K_URI", default_value = m2k::DEFAULT_URI)]
    uri: String,
    /// Machine-readable JSON on stdout.
    #[arg(long, global = true)]
    json: bool,
    /// Connect and attribute timeout (500ms, 5s, ...); captures add their own length.
    #[arg(long, global = true, default_value = "5s", value_parser = parse::duration)]
    timeout: Duration,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Model, serial, firmware, calibration, temperature, outputs and supplies; any IIO device works.
    Info {
        /// Also list the IIO devices and their channels.
        #[arg(long)]
        devices: bool,
    },
    /// Read or write any IIO attribute (also for other IIO devices than the M2K, e.g. a Pluto).
    Attr(AttrArgs),
    /// User power supplies V+ (0 to 5 V) and V- (-5 to 0 V).
    Psu {
        #[command(subcommand)]
        cmd: Option<PsuCmd>,
    },
    /// Board monitors: supply rails, temperature, FPGA voltages (slow ADCs, no scope inputs).
    Dmm {
        /// Only channels whose label contains one of these (e.g. temp, vccint).
        filter: Vec<String>,
    },
    /// DC and AC (rms) voltage on the scope inputs 1+/1- and 2+/2-.
    Voltmeter {
        /// Input range: 25 (±25 V) or 2.5 (±2.5 V, finer steps).
        #[arg(long, default_value = "25")]
        range: RangeArg,
    },
    /// Capture the two scope channels: statistics per channel, optionally all samples as CSV.
    Scope(ScopeArgs),
    /// Signal generator outputs W1/W2: constant or periodic signal; runs until Ctrl+C or --for.
    Gen(GenArgs),
    /// Capture the 16 digital pins DIO0-15: state, edges and frequency per pin, optionally CSV/VCD.
    Logic(LogicArgs),
    /// Digital pin outputs: pattern generator from a sample buffer.
    Digital {
        #[command(subcommand)]
        cmd: DigitalCmd,
    },
    /// Calibration of scope inputs and signal outputs (kept by the M2K until power-off).
    Cal {
        #[command(subcommand)]
        cmd: Option<CalCmd>,
    },
    /// The two blue LEDs: off for a dark bench, on again, or what they are now.
    #[command(long_about = "\
The two blue LEDs of the M2K: switch them off (a dark bench), on again, or show what they are now.

  done   the steady DONE LED, through iiod like everything else.
  ready  the blinking \"Ready\" LED (the kernel's heartbeat). Only a shell on the M2K reaches it, so this one
         goes through the `ssh` on this PC: root@HOST with the factory password `analog`, HOST from --uri.

RAM only: both LEDs are back on after a power cycle of the M2K.

When ssh is missing or the login is refused, the DONE LED is still switched; the output says that the Ready LED
was not changed and why, and `off`/`on` end with exit code 1.

The password goes to ssh through this program acting as its SSH_ASKPASS helper (handed over in the environment
of the ssh child: no file, not on a command line) and is never printed. The M2K's host key is not checked and
nothing is written to a known_hosts file, as long as the password is the public factory one; another password is
only sent with --ssh-known-hosts FILE, which pins the key on first use and refuses a changed one.")]
    Led(LedArgs),
}

#[derive(Args, Debug)]
struct LedArgs {
    #[command(subcommand)]
    cmd: Option<LedCmd>,
    /// Only this LED (both without it).
    #[arg(long, global = true, value_name = "LED")]
    only: Option<LedArg>,
    /// Password of root on the M2K for the Ready LED [default: the factory one, `analog`]. Better through the
    /// environment than here: other users of this PC can see command lines.
    #[arg(long, global = true, env = led::ENV_PASSWORD, hide_env_values = true, value_name = "PASSWORD")]
    ssh_password: Option<led::Password>,
    /// Pin the M2K's ssh host key in this file (created on first use) and refuse a changed one; needed for a
    /// password other than the factory one. Never your ~/.ssh/known_hosts.
    #[arg(long, global = true, env = "M2K_SSH_KNOWN_HOSTS", value_name = "FILE", value_hint = clap::ValueHint::FilePath)]
    ssh_known_hosts: Option<PathBuf>,
}

#[derive(Subcommand, Debug, Clone, Copy)]
enum LedCmd {
    /// What both LEDs are now (the default); the Ready LED only when ssh gets in.
    Status,
    /// Switch the LEDs off, until the M2K is power cycled.
    Off,
    /// Switch the LEDs back on (Ready blinks again).
    On,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LedArg {
    /// The steady DONE LED (through iiod).
    Done,
    /// The blinking Ready LED (through ssh).
    Ready,
}

impl From<LedArg> for led::Led {
    fn from(l: LedArg) -> led::Led {
        match l {
            LedArg::Done => led::Led::Done,
            LedArg::Ready => led::Led::Ready,
        }
    }
}

#[derive(Args, Debug)]
struct AttrArgs {
    /// Device id or name (iio:device0, m2k-adc); without it, the devices are listed.
    #[arg(add = ArgValueCandidates::new(complete::device_names))]
    device: Option<String>,
    /// Attribute; without it, every attribute of the device (or channel) is read.
    #[arg(add = ArgValueCandidates::new(complete::attr_names))]
    attr: Option<String>,
    /// Value to write.
    value: Option<String>,
    /// Channel id or name (voltage0) for a channel attribute.
    #[arg(long, short = 'c', add = ArgValueCandidates::new(complete::channel_names))]
    channel: Option<String>,
    /// The channel is an output (input by default; needed when both exist, like m2k-fabric voltage0).
    #[arg(long, short = 'o', requires = "channel")]
    output: bool,
    /// A debug attribute.
    #[arg(long, conflicts_with_all = ["channel", "buffer"])]
    debug: bool,
    /// A buffer attribute.
    #[arg(long, conflicts_with = "channel")]
    buffer: bool,
}

#[derive(Subcommand, Debug)]
enum PsuCmd {
    /// Measured voltage and state of both rails (the default).
    Status,
    /// Set and switch on rails; the others stay as they are.
    Set {
        /// V+ in volts, 0 to 5.
        #[arg(long, allow_hyphen_values = true, value_parser = parse::si)]
        pos: Option<f64>,
        /// V- in volts, -5 to 0.
        #[arg(long, allow_hyphen_values = true, value_parser = parse::si)]
        neg: Option<f64>,
    },
    /// Switch rails off (both unless --pos or --neg).
    Off {
        #[arg(long)]
        pos: bool,
        #[arg(long)]
        neg: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RangeArg {
    #[value(name = "25")]
    V25,
    #[value(name = "2.5")]
    V2_5,
}

impl From<RangeArg> for Range {
    fn from(r: RangeArg) -> Range {
        match r {
            RangeArg::V25 => Range::V25,
            RangeArg::V2_5 => Range::V2_5,
        }
    }
}

#[derive(Args, Debug)]
struct ScopeArgs {
    /// Samples per channel (10k, 1M; at most 4M = 4194304).
    #[arg(short = 'n', long, default_value = "10k", value_parser = parse::count)]
    samples: usize,
    /// Sample rate (1k to 100M); rates between the native 1k/10k/.../100M are made by averaging.
    #[arg(short = 'r', long, default_value = "1M", value_parser = parse::si)]
    rate: f64,
    /// Input range of both channels: 25 (±25 V) or 2.5 (±2.5 V).
    #[arg(long, default_value = "25")]
    range: RangeArg,
    /// Range of channel 1 only.
    #[arg(long)]
    range1: Option<RangeArg>,
    /// Range of channel 2 only.
    #[arg(long)]
    range2: Option<RangeArg>,
    /// Write all samples as CSV: time_s,ch1_v,ch2_v.
    #[arg(long)]
    csv: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct GenArgs {
    /// Outputs: 1 (W1), 2 (W2) or 1,2.
    #[arg(short = 'c', long, default_value = "1", value_parser = gen_channels)]
    channel: Outputs,
    /// Stop after this long (10s, 1m) instead of at Ctrl+C.
    #[arg(long = "for", value_parser = parse::duration)]
    duration: Option<Duration>,
    #[command(subcommand)]
    shape: Shape,
}

/// Output indices, 0 = W1.
#[derive(Debug, Clone, PartialEq)]
struct Outputs(Vec<usize>);

fn gen_channels(text: &str) -> Result<Outputs> {
    let list = parse::list(text, 2)?;
    if list.contains(&0) {
        bail!("outputs are 1 (W1) and 2 (W2)");
    }
    Ok(Outputs(list.into_iter().map(|c| c as usize - 1).collect()))
}

/// Digital pin numbers.
#[derive(Debug, Clone, PartialEq)]
struct Pins(Vec<u32>);

#[derive(Subcommand, Debug, Clone)]
enum Shape {
    /// A constant voltage, -5 to 5 V.
    Const {
        #[arg(allow_hyphen_values = true, value_parser = parse::si)]
        volts: f64,
    },
    Sine(Periodic),
    Square(Periodic),
    Triangle(Periodic),
    /// Rising ramp.
    Sawtooth(Periodic),
    /// Switch the outputs off (the amplifier; a buffer of another `m2k gen` ends with that process).
    Off,
}

#[derive(Args, Debug, Clone)]
struct Periodic {
    /// Frequency (1k, 2.5M).
    #[arg(short, long, value_parser = parse::si)]
    freq: f64,
    /// Amplitude in volts, peak (a 1 V sine swings from -1 to +1 V around the offset).
    #[arg(short, long, default_value = "1", value_parser = parse::si)]
    amp: f64,
    /// DC offset in volts.
    #[arg(short, long, default_value = "0", allow_hyphen_values = true, value_parser = parse::si)]
    offset: f64,
    /// Phase in degrees.
    #[arg(long, default_value = "0", allow_hyphen_values = true)]
    phase: f64,
    /// Share of the period high, in percent (square only).
    #[arg(long, default_value = "50")]
    duty: f64,
}

impl Periodic {
    fn to_lib(&self) -> wave::Periodic {
        wave::Periodic {
            freq: self.freq,
            amp: self.amp,
            offset: self.offset,
            phase: self.phase,
            duty: self.duty,
        }
    }
}

/// `None` for `Off`, which is not a wave (the caller switches the output off instead).
fn to_lib_shape(shape: &Shape) -> Option<wave::Shape> {
    Some(match shape {
        Shape::Const { volts } => wave::Shape::Const(*volts),
        Shape::Off => return None,
        Shape::Sine(p) => wave::Shape::Sine(p.to_lib()),
        Shape::Square(p) => wave::Shape::Square(p.to_lib()),
        Shape::Triangle(p) => wave::Shape::Triangle(p.to_lib()),
        Shape::Sawtooth(p) => wave::Shape::Sawtooth(p.to_lib()),
    })
}

#[derive(Args, Debug)]
struct LogicArgs {
    /// Samples (10k, 1M; at most 8M).
    #[arg(short = 'n', long, default_value = "10k", value_parser = parse::count)]
    samples: usize,
    /// Sample rate (up to 100M).
    #[arg(short = 'r', long, default_value = "1M", value_parser = parse::si)]
    rate: f64,
    /// Pins to report: 0-15, 0-3,7.
    #[arg(short = 'p', long, default_value = "0-15", value_parser = |s: &str| parse::list(s, 15).map(Pins))]
    pins: Pins,
    /// Trigger condition per pin: `PIN:COND` (e.g. `7:rising`), repeatable; COND is none, any, rising,
    /// falling, low or high. Without one the capture is free-running (starts right away).
    #[arg(long = "trigger", value_parser = parse_trigger)]
    trigger: Vec<(u32, TriggerCondition)>,
    /// Several --trigger pins must all match (AND) instead of any one of them (OR, the default).
    #[arg(long, requires = "trigger")]
    trigger_and: bool,
    /// Samples kept from before the trigger (negative, at most 8192: the M2K's limit) or a delay in samples
    /// after it before the capture starts (positive).
    #[arg(
        long,
        default_value = "0",
        allow_hyphen_values = true,
        requires = "trigger"
    )]
    trigger_delay: i32,
    /// Extra time to wait for the trigger condition, on top of the capture length.
    #[arg(long, default_value = "30s", value_parser = parse::duration)]
    trigger_wait: Duration,
    /// Write all samples as CSV: time_s and one column per reported pin.
    #[arg(long)]
    csv: Option<PathBuf>,
    /// Write all samples as a VCD (Value Change Dump): one wire per reported pin, for sigrok and other
    /// waveform viewers.
    #[arg(long)]
    vcd: Option<PathBuf>,
}

fn parse_trigger(text: &str) -> Result<(u32, TriggerCondition)> {
    let (pin, cond) = text
        .split_once(':')
        .ok_or_else(|| anyhow!("`{text}`: expected PIN:CONDITION (e.g. 7:rising)"))?;
    let pin: u32 = pin
        .trim()
        .parse()
        .map_err(|_| anyhow!("`{pin}` in `{text}` is not a pin number"))?;
    if pin > 15 {
        bail!("{pin} in `{text}` is above 15");
    }
    Ok((pin, TriggerCondition::parse(cond.trim())?))
}

#[derive(Subcommand, Debug)]
enum DigitalCmd {
    /// Play a pattern on digital pins: push-pull or open-drain, cyclic or one-shot.
    Out(DigitalOutArgs),
}

#[derive(Args, Debug)]
struct DigitalOutArgs {
    /// CSV file with a `dioN` column per pin to drive (the format `m2k logic --csv` writes); other
    /// columns (e.g. time_s) are ignored.
    csv: PathBuf,
    /// Sample rate (up to 100M); the pins' divider rounds it, a warning says what was really used.
    #[arg(short = 'r', long, value_parser = parse::si)]
    rate: f64,
    /// Pins driven open-drain (tristated for `1`, like I2C/1-Wire need); the rest are push-pull. Must be
    /// a subset of the CSV's pins: 0-3,7.
    #[arg(long, value_parser = |s: &str| parse::list(s, 15).map(Pins))]
    open_drain: Option<Pins>,
    /// Repeat the buffer until Ctrl+C or --for, instead of playing it once.
    #[arg(long)]
    cyclic: bool,
    /// Stop after this long (only with --cyclic; a one-shot buffer stops on its own).
    #[arg(long = "for", value_parser = parse::duration, requires = "cyclic")]
    duration: Option<Duration>,
}

#[derive(Subcommand, Debug)]
enum CalCmd {
    /// Corrections the M2K holds, and whether it was calibrated since power-up (the default).
    Status,
    /// Self-calibrate (about 5 s): scope inputs, then the outputs (W1/W2 are driven to ~2.5 V for a moment).
    Run {
        /// Only the scope inputs; the outputs are left alone.
        #[arg(long)]
        adc_only: bool,
    },
    /// Load the factory calibration table entry closest to the board temperature (cal,temp_lut).
    Lut,
    /// Back to the power-on values: no corrections.
    Reset,
}

/// Output of a command: text for people, or JSON with `--json`.
struct Out {
    json: bool,
}

impl Out {
    fn print(&self, text: impl AsRef<str>, value: Value) {
        if self.json {
            println!("{value}");
        } else {
            let text = text.as_ref();
            if !text.is_empty() {
                println!("{}", text.trim_end());
            }
        }
    }

    /// A warning on stderr (text mode only; in JSON it is part of the value).
    fn warn(&self, text: &str) {
        if !self.json {
            eprintln!("{} {text}", style::yellow("warning:"));
        }
    }

    /// A progress bar on stderr when a person watches; hidden otherwise.
    fn progress(&self, len: u64, msg: &str) -> ProgressBar {
        let bar = ProgressBar::with_draw_target(Some(len), ProgressDrawTarget::stderr());
        if self.json || !std::io::stderr().is_terminal() {
            bar.set_draw_target(ProgressDrawTarget::hidden());
        }
        bar.set_style(
            ProgressStyle::with_template(
                "{msg} [{bar:30.cyan/blue}] {bytes}/{total_bytes} {bytes_per_sec} {eta}",
            )
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=> "),
        );
        bar.set_message(msg.to_string());
        bar
    }

    fn spinner(&self, msg: &str) -> ProgressBar {
        let bar = ProgressBar::new_spinner();
        if self.json || !std::io::stderr().is_terminal() {
            bar.set_draw_target(ProgressDrawTarget::hidden());
        }
        bar.enable_steady_tick(Duration::from_millis(100));
        bar.set_message(msg.to_string());
        bar
    }
}

/// Colour on stdout/stderr when it is a terminal and `NO_COLOR` is not set.
mod style {
    use std::io::IsTerminal;
    use std::sync::OnceLock;

    fn on() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| {
            std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
                && std::io::stderr().is_terminal()
                && std::io::stdout().is_terminal()
        })
    }

    fn paint(code: &str, text: &str) -> String {
        if on() {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    pub fn bold(t: &str) -> String {
        paint("1", t)
    }
    pub fn green(t: &str) -> String {
        paint("32", t)
    }
    pub fn yellow(t: &str) -> String {
        paint("33", t)
    }
    pub fn red(t: &str) -> String {
        paint("31", t)
    }
    pub fn dim(t: &str) -> String {
        paint("2", t)
    }
}

pub fn main() {
    // ssh started us as its SSH_ASKPASS helper (`m2k led`): answer and exit, before anything else
    led::askpass_main();
    complete::complete(Cli::command);
    let cli = Cli::parse();
    let json = cli.json;
    if let Err(e) = run(cli) {
        if json {
            println!("{}", json!({ "error": format!("{e:#}") }));
        } else {
            eprintln!("{} {e:#}", style::red("error:"));
        }
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let out = Out { json: cli.json };
    let connect = || -> Result<M2k> { Ok(M2k::connect(&cli.uri, cli.timeout)?) };
    match cli.cmd {
        Cmd::Info { devices } => info(&out, &mut connect()?, devices),
        Cmd::Attr(args) => attr(&out, &mut connect()?, args),
        Cmd::Psu { cmd } => psu(
            &out,
            &mut m2k_connect(connect)?,
            cmd.unwrap_or(PsuCmd::Status),
        ),
        Cmd::Dmm { filter } => dmm(&out, &mut connect()?, &filter),
        Cmd::Voltmeter { range } => voltmeter(&out, &mut m2k_connect(connect)?, range.into()),
        Cmd::Scope(args) => scope_cmd(&out, &mut m2k_connect(connect)?, args),
        Cmd::Gen(args) => gen_cmd(&out, &mut m2k_connect(connect)?, args),
        Cmd::Logic(args) => logic_cmd(&out, &mut m2k_connect(connect)?, args),
        Cmd::Digital { cmd } => digital(&out, &mut m2k_connect(connect)?, cmd),
        Cmd::Cal { cmd } => cal_cmd(
            &out,
            &mut m2k_connect(connect)?,
            cmd.unwrap_or(CalCmd::Status),
        ),
        Cmd::Led(args) => {
            let mut m = connect()?;
            m.require_m2k()?;
            led_cmd(&out, &mut m, args, cli.timeout)
        }
    }
}

fn m2k_connect(connect: impl FnOnce() -> Result<M2k>) -> Result<M2k> {
    let mut m = connect()?;
    m.require_m2k()?;
    m.init().context("power up the M2K")?;
    Ok(m)
}

fn on_off(on: bool) -> String {
    if on {
        style::green("on")
    } else {
        style::dim("off")
    }
}

// ---- JSON for types the m2k crate returns (no serde_json dependency there) ----

fn analog_json(s: &stats::Analog) -> Value {
    json!({
        "samples": s.samples,
        "min": s.min,
        "max": s.max,
        "mean": s.mean,
        "rms": s.rms,
        "ac_rms": s.ac_rms,
        "vpp": s.vpp(),
        "frequency": s.frequency,
        "periods": s.periods,
        "duty": s.duty,
        "clipped": s.clipped,
    })
}

fn digital_json(d: &stats::Digital) -> Value {
    json!({
        "channel": d.channel,
        "state": d.state(),
        "edges": d.edges(),
        "rising": d.rising,
        "falling": d.falling,
        "high": d.high,
        "frequency": d.frequency,
        "min_pulse": d.min_pulse,
    })
}

fn corrections_json(c: &Corrections) -> Value {
    json!({
        "calibrated": c.calibrated(),
        "adc_offset": c.adc_offset,
        "adc_gain": c.adc_gain,
        "dac_offset": c.dac_offset,
        "dac_gain": c.dac_gain,
    })
}

// ---- info ----

fn info(out: &Out, m: &mut M2k, devices: bool) -> Result<()> {
    let version = m.client.version().unwrap_or_default();
    let ctx_attrs: serde_json::Map<String, Value> = m
        .ctx
        .attrs
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    let model = m
        .ctx
        .attr("hw_model")
        .unwrap_or("unknown model")
        .to_string();
    let serial = m.ctx.attr("hw_serial").unwrap_or("unknown").to_string();
    let firmware = m.ctx.attr("fw_version").unwrap_or("unknown").to_string();
    let is_m2k = m.require_m2k().is_ok();
    let mut text = format!(
        "{} at {} (iiod {version})\nmodel: {model}\nserial: {serial}\nfirmware: {firmware}{}\n",
        style::bold(if is_m2k { "ADALM2000" } else { "IIO context" }),
        m.uri,
        m.ctx
            .attr("local,kernel")
            .map(|k| format!(", kernel {k}"))
            .unwrap_or_default(),
    );
    let mut value = json!({
        "uri": m.uri,
        "iiod_version": version,
        "model": model,
        "serial": serial,
        "firmware": firmware,
        "m2k": is_m2k,
        "context": ctx_attrs,
    });
    if is_m2k {
        let corr = Corrections::read(m)?;
        let temperature = m.temperature().ok();
        let outputs = [m.dac_enabled(0)?, m.dac_enabled(1)?];
        let supplies = [m.psu_enabled(Rail::Pos)?, m.psu_enabled(Rail::Neg)?];
        let lut = m.ctx.attr("cal,temp_lut").is_some();
        text += &format!(
            "calibration: {}{}\n",
            if corr.calibrated() {
                style::green("calibrated since power-up")
            } else {
                style::yellow("not calibrated since power-up (run `m2k cal run`)")
            },
            if lut {
                ", factory table available (`m2k cal lut`)"
            } else {
                ""
            }
        );
        if let Some(t) = temperature {
            text += &format!("temperature: {t:.1} °C\n");
        }
        text += &format!(
            "outputs: W1 {}, W2 {}\nsupplies: V+ {}, V- {}\n",
            on_off(outputs[0]),
            on_off(outputs[1]),
            on_off(supplies[0]),
            on_off(supplies[1])
        );
        value["calibration"] = corrections_json(&corr);
        value["calibration"]["factory_table"] = json!(lut);
        value["temperature"] = json!(temperature);
        value["outputs"] = json!({ "w1": outputs[0], "w2": outputs[1] });
        value["supplies"] = json!({ "pos": supplies[0], "neg": supplies[1] });
    }
    text += &format!("{} IIO devices", m.ctx.devices.len());
    if devices {
        text += ":\n";
        for d in &m.ctx.devices {
            let chans: Vec<String> = d
                .channels
                .iter()
                .map(|c| {
                    format!(
                        "{}{}{}",
                        if c.output { "out:" } else { "" },
                        c.id,
                        if c.scan.is_some() { "*" } else { "" }
                    )
                })
                .collect();
            text += &format!("  {}: {}\n", d.label(), chans.join(" "));
        }
        text += &style::dim("  (* = can be captured or played through a buffer)");
    } else {
        text += &style::dim(" (`m2k info --devices`)");
    }
    value["devices"] = m
        .ctx
        .devices
        .iter()
        .map(|d| {
            json!({
                "id": d.id,
                "name": d.name,
                "attrs": d.attrs,
                "channels": d.channels.iter().map(|c| json!({
                    "id": c.id, "name": c.name, "output": c.output, "buffered": c.scan.is_some(), "attrs": c.attrs,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    out.print(text, value);
    Ok(())
}

// ---- attr ----

fn attr(out: &Out, m: &mut M2k, args: AttrArgs) -> Result<()> {
    let Some(dev_name) = args.device else {
        let text: Vec<String> = m.ctx.devices.iter().map(|d| d.label()).collect();
        let value: Vec<Value> = m
            .ctx
            .devices
            .iter()
            .map(|d| json!({ "id": d.id, "name": d.name }))
            .collect();
        out.print(text.join("\n"), json!({ "devices": value }));
        return Ok(());
    };
    let dev = m.device(&dev_name)?.clone();
    let channel = match &args.channel {
        Some(c) => {
            let chn = dev
                .channel(c, args.output)
                .or_else(|| (!args.output).then(|| dev.channel(c, true)).flatten())
                .with_context(|| {
                    format!(
                        "{} has no {} channel `{c}` ({})",
                        dev.label(),
                        if args.output {
                            "output"
                        } else {
                            "input or output"
                        },
                        dev.channels
                            .iter()
                            .map(|c| c.id.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                })?;
            Some(chn.clone())
        }
        None => None,
    };
    let (debug, buffer) = (args.debug, args.buffer);
    let names: Vec<String> = match (&channel, args.debug, args.buffer) {
        (Some(c), _, _) => c.attrs.clone(),
        (None, true, _) => dev.debug_attrs.clone(),
        (None, _, true) => dev.buffer_attrs.clone(),
        (None, false, false) => dev.attrs.clone(),
    };
    let Some(name) = args.attr else {
        // everything, with values
        let mut text = vec![];
        let mut values = serde_json::Map::new();
        for n in &names {
            let v = m
                .client
                .read_attr(&dev.id, attr_of(channel.as_ref(), n, debug, buffer));
            let shown = match &v {
                Ok(v) => v.clone(),
                Err(e) => format!("({e})"),
            };
            text.push(format!("{n} = {shown}"));
            values.insert(n.clone(), v.map(Value::String).unwrap_or(Value::Null));
        }
        if channel.is_none() && !args.debug && !args.buffer && !dev.channels.is_empty() {
            let chans: Vec<String> = dev
                .channels
                .iter()
                .map(|c| format!("{}{}", if c.output { "out:" } else { "" }, c.id))
                .collect();
            text.push(style::dim(&format!(
                "channels (-c ID): {}",
                chans.join(" ")
            )));
        }
        out.print(text.join("\n"), json!({ "device": dev.id, "channel": channel.as_ref().map(|c| &c.id), "attrs": values }));
        return Ok(());
    };
    if !names.contains(&name) {
        out.warn(&format!(
            "`{name}` is not listed in the context ({}); trying anyway",
            names.join(" ")
        ));
    }
    let what = attr_of(channel.as_ref(), &name, debug, buffer);
    if let Some(value) = args.value {
        m.client.write_attr(&dev.id, what, &value)?;
    }
    let value = m.client.read_attr(&dev.id, what)?;
    out.print(
        &value,
        json!({ "device": dev.id, "attr": what.to_string(), "value": value }),
    );
    Ok(())
}

fn attr_of<'a>(
    channel: Option<&'a iiod_client::Channel>,
    name: &'a str,
    debug: bool,
    buffer: bool,
) -> Attr<'a> {
    match channel {
        Some(c) => Attr::Channel {
            channel: &c.id,
            output: c.output,
            attr: name,
        },
        None if debug => Attr::Debug(name),
        None if buffer => Attr::Buffer(name),
        None => Attr::Device(name),
    }
}

// ---- psu ----

fn psu_status_text(s: &psu::Status) -> (String, Value) {
    let mut text = vec![];
    let mut value = serde_json::Map::new();
    for (name, key, r) in [("V+", "pos", s.pos), ("V-", "neg", s.neg)] {
        text.push(format!(
            "{name}: {} {:+.3} V measured",
            on_off(r.enabled),
            r.measured
        ));
        value.insert(
            key.into(),
            json!({ "enabled": r.enabled, "measured": r.measured }),
        );
    }
    (text.join("\n"), Value::Object(value))
}

fn psu(out: &Out, m: &mut M2k, cmd: PsuCmd) -> Result<()> {
    let status = match cmd {
        PsuCmd::Status => psu::status(m)?,
        PsuCmd::Set { pos, neg } => {
            if pos.is_none() && neg.is_none() {
                bail!("give --pos VOLTS and/or --neg VOLTS");
            }
            psu::set(m, pos, neg)?
        }
        PsuCmd::Off { pos, neg } => psu::off(m, pos, neg)?,
    };
    let (text, value) = psu_status_text(&status);
    out.print(text, value);
    Ok(())
}

// ---- dmm ----

fn dmm(out: &Out, m: &mut M2k, filter: &[String]) -> Result<()> {
    let readings = dmm::readings(m, filter)?;
    let text: Vec<String> = readings
        .iter()
        .map(|r| format!("{}: {:.4} {}", r.channel, r.value, r.unit))
        .collect();
    let values: Vec<Value> = readings
        .iter()
        .map(|r| {
            json!({ "channel": r.channel, "device": r.device, "id": r.id, "value": r.value, "unit": r.unit })
        })
        .collect();
    out.print(text.join("\n"), json!({ "readings": values }));
    Ok(())
}

// ---- scope ----

/// Captures `samples` samples at `rate` and `ranges`, with a progress bar for long captures and a warning
/// when the rate asked for is not one the ADC can hit.
fn do_capture(
    out: &Out,
    m: &mut M2k,
    samples: usize,
    rate: f64,
    ranges: [Range; 2],
) -> Result<scope::Capture> {
    let secs = samples as f64 / rate;
    let bytes = (samples.next_multiple_of(device::BUFFER_STEP) * 4) as u64;
    let bar = (secs > 1.0 || samples > 500_000).then(|| out.progress(bytes, "capture"));
    let cap = scope::capture(m, samples, rate, ranges, |n| {
        if let Some(b) = &bar {
            b.set_position(n as u64);
        }
    })?;
    if let Some(b) = bar {
        b.finish_and_clear();
    }
    if (cap.rate - rate).abs() > rate * 1e-6 {
        out.warn(&format!(
            "{} is not available, capturing at {}",
            parse::fmt_unit(rate, "S/s"),
            parse::fmt_unit(cap.rate, "S/s")
        ));
    }
    Ok(cap)
}

fn analog_line(name: &str, s: &stats::Analog) -> String {
    let mut parts = vec![];
    if let Some(f) = s.frequency {
        parts.push(format!(
            "{} ({} periods)",
            parse::fmt_unit(f, "Hz"),
            s.periods
        ));
        if let Some(d) = s.duty {
            parts.push(format!("duty {:.0} %", d * 100.0));
        }
    }
    parts.push(format!("mean {:+.4} V", s.mean));
    parts.push(format!("vpp {:.4} V", s.vpp()));
    parts.push(format!("min {:+.4} V", s.min));
    parts.push(format!("max {:+.4} V", s.max));
    parts.push(format!("rms {:.4} V", s.rms));
    parts.push(format!("ac rms {:.4} V", s.ac_rms));
    format!("{}: {}", style::bold(name), parts.join(", "))
}

fn capture_warnings(out: &Out, cap: &scope::Capture) -> Vec<String> {
    let mut warnings = vec![];
    if !cap.calibrated {
        warnings.push("the M2K was not calibrated since power-up: offsets can be tens of mV off; `m2k cal run` (5 s) fixes that".to_string());
    }
    for i in 0..2 {
        if cap.stats[i].clipped > 0 {
            warnings.push(format!(
                "ch{} hit the end of the ±{} V range in {} samples: the signal is larger than shown{}",
                i + 1,
                cap.ranges[i].limit(),
                cap.stats[i].clipped,
                if cap.ranges[i] == Range::V2_5 { "; use --range 25" } else { "" }
            ));
        }
    }
    for w in &warnings {
        out.warn(w);
    }
    warnings
}

fn scope_cmd(out: &Out, m: &mut M2k, args: ScopeArgs) -> Result<()> {
    let ranges = [
        args.range1.unwrap_or(args.range).into(),
        args.range2.unwrap_or(args.range).into(),
    ];
    let cap = do_capture(out, m, args.samples, args.rate, ranges)?;
    let warnings = capture_warnings(out, &cap);
    let mut text = format!(
        "scope: {} samples at {} ({}), ranges ±{} V / ±{} V, {}\n",
        args.samples,
        parse::fmt_unit(cap.rate, "S/s"),
        parse::fmt_secs(args.samples as f64 / cap.rate),
        cap.ranges[0].limit(),
        cap.ranges[1].limit(),
        if cap.calibrated {
            "calibrated"
        } else {
            "uncalibrated"
        }
    );
    for i in 0..2 {
        text += &analog_line(&format!("ch{}", i + 1), &cap.stats[i]);
        text.push('\n');
    }
    let mut value = json!({
        "samples": args.samples,
        "rate": cap.rate,
        "ranges": [cap.ranges[0].limit(), cap.ranges[1].limit()],
        "calibrated": cap.calibrated,
        "ch1": analog_json(&cap.stats[0]),
        "ch2": analog_json(&cap.stats[1]),
        "warnings": warnings,
    });
    if let Some(path) = args.csv {
        let mut rows = String::with_capacity(args.samples * 32);
        rows.push_str("time_s,ch1_v,ch2_v\n");
        for k in 0..cap.volts[0].len() {
            rows += &format!(
                "{:.9},{:.5},{:.5}\n",
                k as f64 / cap.rate,
                cap.volts[0][k],
                cap.volts[1][k]
            );
        }
        write_file(&path, rows.as_bytes())?;
        text += &format!(
            "csv: {} ({} rows: time_s,ch1_v,ch2_v)",
            path.display(),
            args.samples
        );
        value["csv"] = json!(path.display().to_string());
    }
    out.print(text, value);
    Ok(())
}

/// Writes to a temporary name next to `path`, then moves it in place.
fn write_file(path: &std::path::Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("part");
    let mut f = std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    f.write_all(data)
        .with_context(|| format!("write {}", tmp.display()))?;
    drop(f);
    std::fs::rename(&tmp, path)
        .with_context(|| format!("move {} to {}", tmp.display(), path.display()))
}

// ---- voltmeter ----

fn voltmeter(out: &Out, m: &mut M2k, range: Range) -> Result<()> {
    // 100 ms at 100 kS/s: whole periods of 50 and 60 Hz mains, so hum averages out of the DC value
    let cap = do_capture(out, m, 10_000, 100e3, [range, range])?;
    let warnings = capture_warnings(out, &cap);
    let mut text = vec![];
    let mut value =
        json!({ "range": range.limit(), "calibrated": cap.calibrated, "warnings": warnings });
    for i in 0..2 {
        let s = &cap.stats[i];
        text.push(format!(
            "{}: {:+.4} V DC, {:.4} V AC rms",
            style::bold(&format!("ch{}", i + 1)),
            s.mean,
            s.ac_rms
        ));
        value[format!("ch{}", i + 1)] =
            json!({ "dc": s.mean, "ac_rms": s.ac_rms, "min": s.min, "max": s.max });
    }
    out.print(text.join("\n"), value);
    Ok(())
}

// ---- gen ----

fn gen_cmd(out: &Out, m: &mut M2k, args: GenArgs) -> Result<()> {
    let channels = args.channel.0;
    let names: Vec<String> = channels.iter().map(|c| format!("W{}", c + 1)).collect();
    let Some(shape) = to_lib_shape(&args.shape) else {
        for c in &channels {
            m.dac_stop(*c)?;
        }
        out.print(
            format!("{}: off", names.join(", ")),
            json!({ "outputs": names, "running": false }),
        );
        return Ok(());
    };
    let rates = m.rates(device::DACS[0])?;
    let wave = wave::build_wave(&shape, &rates)?;
    let wave_text = match &args.shape {
        Shape::Const { volts } => format!("constant {volts:+.3} V"),
        Shape::Sine(p) | Shape::Square(p) | Shape::Triangle(p) | Shape::Sawtooth(p) => {
            let name = match &args.shape {
                Shape::Sine(_) => "sine",
                Shape::Square(_) => "square",
                Shape::Triangle(_) => "triangle",
                _ => "sawtooth",
            };
            format!(
                "{name} {}, {:.3} V peak, offset {:+.3} V ({} samples at {})",
                parse::fmt_unit(wave.frequency.unwrap_or(0.0), "Hz"),
                p.amp,
                p.offset,
                wave.volts.len(),
                parse::fmt_unit(wave.rate, "S/s")
            )
        }
        Shape::Off => unreachable!("handled above"),
    };
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst))
            .context("install the Ctrl+C handler")?;
    }
    let started = m.dac_start(&channels, wave.rate, &wave.volts);
    if let Err(e) = started {
        for c in &channels {
            let _ = m.dac_stop(*c);
        }
        return Err(e.into());
    }
    let until = match args.duration {
        Some(d) => format!("for {}", parse::fmt_secs(d.as_secs_f64())),
        None => "until Ctrl+C".to_string(),
    };
    out.print(
        format!("{}: {}; running {until}", names.join(", "), wave_text),
        json!({
            "outputs": names,
            "running": true,
            "shape": wave_text,
            "frequency": wave.frequency,
            "sample_rate": wave.rate,
            "samples": wave.volts.len(),
            "duration": args.duration.map(|d| d.as_secs_f64()),
        }),
    );
    let _ = std::io::stdout().flush();
    let start = Instant::now();
    while !stop.load(Ordering::SeqCst) && args.duration.is_none_or(|d| start.elapsed() < d) {
        std::thread::sleep(Duration::from_millis(50));
    }
    for c in &channels {
        m.dac_stop(*c)?;
    }
    if !out.json {
        eprintln!(
            "{}: off after {}",
            names.join(", "),
            parse::fmt_secs(start.elapsed().as_secs_f64())
        );
    }
    Ok(())
}

// ---- logic ----

fn logic_cmd(out: &Out, m: &mut M2k, args: LogicArgs) -> Result<()> {
    let pins = args.pins.0;
    let trigger = LogicTrigger {
        pins: args.trigger,
        and: args.trigger_and,
        delay: args.trigger_delay,
        wait: args.trigger_wait,
    };
    let secs = args.samples as f64 / args.rate;
    let bar = (secs > 1.0 || args.samples > 1_000_000).then(|| {
        out.progress(
            (args.samples.next_multiple_of(device::BUFFER_STEP) * 2) as u64,
            "capture",
        )
    });
    let cap = logic::capture(m, args.samples, args.rate, &pins, &trigger, |n| {
        if let Some(b) = &bar {
            b.set_position(n as u64);
        }
    })?;
    if let Some(b) = bar {
        b.finish_and_clear();
    }
    if (cap.rate - args.rate).abs() > args.rate * 1e-6 {
        out.warn(&format!(
            "logic runs at {}, not {}",
            parse::fmt_unit(cap.rate, "S/s"),
            parse::fmt_unit(args.rate, "S/s")
        ));
    }
    let mut text = format!(
        "logic: {} samples at {} ({})\n",
        args.samples,
        parse::fmt_unit(cap.rate, "S/s"),
        parse::fmt_secs(args.samples as f64 / cap.rate)
    );
    let mut low = vec![];
    let mut high = vec![];
    let mut values = vec![];
    for (pin, d) in pins.iter().zip(&cap.pins) {
        let dir = cap
            .directions
            .get(*pin as usize)
            .cloned()
            .unwrap_or_default();
        let mut v = digital_json(d);
        v["direction"] = json!(dir);
        values.push(v);
        let tag = if dir == "out" { " (output)" } else { "" };
        match d.state() {
            "low" => low.push(format!("{pin}{tag}")),
            "high" => high.push(format!("{pin}{tag}")),
            _ => {
                let mut parts = vec![
                    format!("{} edges", d.edges()),
                    format!("{:.0} % high", d.high * 100.0),
                ];
                if let Some(f) = d.frequency {
                    parts.insert(0, parse::fmt_unit(f, "Hz"));
                }
                if let Some(p) = d.min_pulse {
                    parts.push(format!("shortest pulse {}", parse::fmt_secs(p)));
                }
                text += &format!(
                    "{}: toggling, {}{tag}\n",
                    style::bold(&format!("DIO{pin}")),
                    parts.join(", ")
                );
            }
        }
    }
    if !high.is_empty() {
        text += &format!("high: DIO {}\n", high.join(", "));
    }
    if !low.is_empty() {
        text += &format!("low: DIO {}\n", low.join(", "));
    }
    let sample_index = if trigger.pins.is_empty() {
        None
    } else {
        Some(-trigger.delay)
    };
    if let Some(i) = sample_index {
        let joiner = if trigger.and { " and " } else { " or " };
        text += &format!(
            "trigger: {} (delay {} samples, sample {i} in the capture)\n",
            trigger
                .pins
                .iter()
                .map(|(p, c)| format!("DIO{p} {}", c.as_str()))
                .collect::<Vec<_>>()
                .join(joiner),
            trigger.delay
        );
    }
    let mut value = json!({
        "samples": args.samples,
        "rate": cap.rate,
        "pins": values,
        "trigger": {
            "pins": trigger.pins.iter().map(|(p, c)| json!({ "pin": p, "condition": c.as_str() })).collect::<Vec<_>>(),
            "logic": if trigger.and { "and" } else { "or" },
            "delay": trigger.delay,
            "sample_index": sample_index,
        },
    });
    if let Some(path) = args.csv {
        let mut rows = String::from("time_s");
        for p in &pins {
            rows += &format!(",dio{p}");
        }
        rows.push('\n');
        for (k, w) in cap.words.iter().enumerate() {
            rows += &format!("{:.9}", k as f64 / cap.rate);
            for p in &pins {
                rows += if w >> p & 1 == 1 { ",1" } else { ",0" };
            }
            rows.push('\n');
        }
        write_file(&path, rows.as_bytes())?;
        text += &format!("csv: {} ({} rows)\n", path.display(), cap.words.len());
        value["csv"] = json!(path.display().to_string());
    }
    if let Some(path) = args.vcd {
        write_vcd(&path, &pins, &cap.words, cap.rate)?;
        text += &format!("vcd: {} ({} pins)", path.display(), pins.len());
        value["vcd"] = json!(path.display().to_string());
    }
    out.print(text, value);
    Ok(())
}

/// The VCD timescale for a sample period in ps, and how many timescale ticks one sample takes. sigrok reads
/// the timescale as its sample period, so it is one sample (`1 us` at 1 MS/s, `10 ns` at 100 MS/s) when the
/// period is 1, 10 or 100 of a unit; otherwise 1 ps with the period in ticks.
fn vcd_timescale(ps_per_sample: u64) -> (String, u64) {
    const UNITS: [(u64, &str); 5] = [
        (1_000_000_000_000, "s"),
        (1_000_000_000, "ms"),
        (1_000_000, "us"),
        (1_000, "ns"),
        (1, "ps"),
    ];
    for (unit_ps, unit) in UNITS {
        for mult in [100, 10, 1] {
            if ps_per_sample == mult * unit_ps {
                return (format!("{mult} {unit}"), 1);
            }
        }
    }
    ("1 ps".into(), ps_per_sample)
}

/// A Value Change Dump sigrok and other waveform viewers can import: one wire per pin, timescale from
/// [`vcd_timescale`], only the samples where a reported pin changed, and a last timestamp at the end of the
/// capture so its full length survives.
fn write_vcd(path: &Path, pins: &[u32], words: &[u16], rate: f64) -> Result<()> {
    let (timescale, ticks_per_sample) = vcd_timescale((1e12 / rate).round().max(1.0) as u64);
    let symbols: Vec<char> = (b'!'..=b'~')
        .map(|b| b as char)
        .filter(|c| *c != '$' && !c.is_ascii_digit())
        .take(pins.len())
        .collect();
    let mut s = String::new();
    s += &format!("$timescale {timescale} $end\n$scope module m2k $end\n");
    for (sym, p) in symbols.iter().zip(pins) {
        s += &format!("$var wire 1 {sym} dio{p} $end\n");
    }
    s += "$upscope $end\n$enddefinitions $end\n";
    let mut last: Option<u16> = None;
    for (k, w) in words.iter().enumerate() {
        if last == Some(*w) {
            continue;
        }
        s += &format!("#{}\n", k as u64 * ticks_per_sample);
        if k == 0 {
            s += "$dumpvars\n";
        }
        for (sym, p) in symbols.iter().zip(pins) {
            let bit = (w >> p) & 1;
            if last.is_none_or(|l| (l >> p) & 1 != bit) {
                s += &format!("{bit}{sym}\n");
            }
        }
        if k == 0 {
            s += "$end\n";
        }
        last = Some(*w);
    }
    s += &format!("#{}\n", words.len() as u64 * ticks_per_sample);
    write_file(path, s.as_bytes())
}

// ---- digital ----

fn digital(out: &Out, m: &mut M2k, cmd: DigitalCmd) -> Result<()> {
    match cmd {
        DigitalCmd::Out(args) => digital_out(out, m, args),
    }
}

/// A CSV with a `dioN` column per driven pin (the format `m2k logic --csv` writes): pins in column
/// order, and one `u16` word per row (bit N = DIO N of the row's pins; other bits 0).
fn read_digital_csv(path: &Path) -> Result<(Vec<u32>, Vec<u16>)> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut lines = text.lines();
    let header = lines
        .next()
        .ok_or_else(|| anyhow!("{}: empty file", path.display()))?;
    let columns: Vec<(usize, u32)> = header
        .split(',')
        .enumerate()
        .filter_map(|(i, c)| {
            c.trim()
                .strip_prefix("dio")
                .and_then(|n| n.parse().ok())
                .map(|p| (i, p))
        })
        .collect();
    if columns.is_empty() {
        bail!(
            "{}: no `dioN` column in the header (`{header}`)",
            path.display()
        );
    }
    let mut words = vec![];
    for (n, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        let mut w: u16 = 0;
        for &(i, pin) in &columns {
            let v = fields
                .get(i)
                .ok_or_else(|| anyhow!("{}:{}: missing column {i}", path.display(), n + 2))?
                .trim();
            match v {
                "1" => w |= 1 << pin,
                "0" => {}
                _ => bail!("{}:{}: `{v}` is not 0 or 1", path.display(), n + 2),
            }
        }
        words.push(w);
    }
    let pins = columns.into_iter().map(|(_, p)| p).collect();
    Ok((pins, words))
}

fn digital_out(out: &Out, m: &mut M2k, args: DigitalOutArgs) -> Result<()> {
    let (pins, words) = read_digital_csv(&args.csv)?;
    let open_drain = args.open_drain.map(|p| p.0).unwrap_or_default();
    for p in &open_drain {
        if !pins.contains(p) {
            bail!(
                "--open-drain {p}: not one of the CSV's pins ({})",
                pins.iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
    }
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst))
            .context("install the Ctrl+C handler")?;
    }
    let started = m.digital_start(&pins, &open_drain, args.rate, &words, args.cyclic);
    let actual = match started {
        Ok(r) => r,
        Err(e) => {
            let _ = m.digital_stop(&pins);
            return Err(e.into());
        }
    };
    if (actual - args.rate).abs() > args.rate * 1e-6 {
        out.warn(&format!(
            "{} is not available, driving at {}",
            parse::fmt_unit(args.rate, "S/s"),
            parse::fmt_unit(actual, "S/s")
        ));
    }
    let names: Vec<String> = pins.iter().map(|p| format!("DIO{p}")).collect();
    let mode = if open_drain.is_empty() {
        "push-pull".to_string()
    } else {
        format!(
            "open-drain: DIO {}",
            open_drain
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    let until = if args.cyclic {
        match args.duration {
            Some(d) => format!("for {}", parse::fmt_secs(d.as_secs_f64())),
            None => "until Ctrl+C".to_string(),
        }
    } else {
        format!("once ({})", parse::fmt_secs(words.len() as f64 / actual))
    };
    out.print(
        format!(
            "{}: {} samples at {} ({}); running {until}",
            names.join(", "),
            words.len(),
            parse::fmt_unit(actual, "S/s"),
            mode
        ),
        json!({
            "pins": pins,
            "open_drain": open_drain,
            "rate": actual,
            "cyclic": args.cyclic,
            "running": true,
            "samples": words.len(),
            "csv": args.csv.display().to_string(),
        }),
    );
    let _ = std::io::stdout().flush();
    let start = Instant::now();
    if args.cyclic {
        while !stop.load(Ordering::SeqCst) && args.duration.is_none_or(|d| start.elapsed() < d) {
            std::thread::sleep(Duration::from_millis(50));
        }
    } else {
        let secs = words.len() as f64 / actual;
        while !stop.load(Ordering::SeqCst) && start.elapsed().as_secs_f64() < secs {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    m.digital_stop(&pins)?;
    if !out.json {
        eprintln!(
            "{}: off after {}",
            names.join(", "),
            parse::fmt_secs(start.elapsed().as_secs_f64())
        );
    }
    Ok(())
}

// ---- cal ----

fn cal_cmd(out: &Out, m: &mut M2k, cmd: CalCmd) -> Result<()> {
    match cmd {
        CalCmd::Status => {
            let c = Corrections::read(m)?;
            let head = if c.calibrated() {
                style::green("calibrated since power-up")
            } else {
                style::yellow("not calibrated since power-up (`m2k cal run`)")
            };
            out.print(format!("{head}\n{}", c.text()), corrections_json(&c));
        }
        CalCmd::Run { adc_only } => {
            let start = Instant::now();
            let bar = out.spinner("calibrating");
            let mut step = |s: &str| bar.set_message(s.to_string());
            let res = cal::run(m, adc_only, &mut step);
            bar.finish_and_clear();
            let c = res?;
            let mut v = corrections_json(&c);
            v["seconds"] = json!(start.elapsed().as_secs_f64());
            v["adc_only"] = json!(adc_only);
            out.print(
                format!(
                    "{} in {}\n{}",
                    style::green(if adc_only {
                        "scope calibrated"
                    } else {
                        "scope and outputs calibrated"
                    }),
                    parse::fmt_secs(start.elapsed().as_secs_f64()),
                    c.text()
                ),
                v,
            );
        }
        CalCmd::Lut => {
            let (temperature, row) = cal::apply_lut(m)?;
            let c = Corrections::read(m)?;
            let mut v = corrections_json(&c);
            v["temperature"] = json!(temperature);
            v["table_temperature"] = json!(row.temperature);
            out.print(
                format!(
                    "factory calibration for {:.1} °C loaded (board at {temperature:.1} °C)\n{}",
                    row.temperature,
                    c.text()
                ),
                v,
            );
        }
        CalCmd::Reset => {
            cal::reset(m)?;
            let c = Corrections::read(m)?;
            out.print(
                format!("corrections reset to power-on values\n{}", c.text()),
                corrections_json(&c),
            );
        }
    }
    Ok(())
}

// ---- led ----

/// One line and the JSON of an LED: `ready` is `None` for the DONE LED, which has no trigger to show.
fn led_line(name: &str, on: bool, ready: Option<&led::Ready>) -> (String, Value) {
    let mut value = json!({ "on": on });
    let detail = match ready {
        Some(r) => {
            value["trigger"] = json!(r.trigger);
            value["brightness"] = json!(r.brightness);
            let blinking = if r.trigger == "heartbeat" {
                "blinking, "
            } else {
                ""
            };
            style::dim(&format!(" ({blinking}trigger {})", r.trigger))
        }
        None => String::new(),
    };
    (format!("{name}: {}{detail}", on_off(on)), value)
}

/// Text and JSON of `m2k led`: a line per LED asked for; an unreachable Ready LED says why.
fn led_text(status: &led::Status, changing: bool) -> (String, Value) {
    let mut text = vec![];
    let mut value = json!({ "done": null, "ready": null, "partial": status.partial() });
    if let Some(on) = status.done {
        let (line, v) = led_line("done", on, None);
        text.push(line);
        value["done"] = v;
    }
    match &status.ready {
        Some(Ok(r)) => {
            let (line, v) = led_line("ready", r.on, Some(r));
            text.push(line);
            value["ready"] = v;
        }
        Some(Err(e)) => {
            let what = if changing { "not changed" } else { "unknown" };
            text.push(format!("ready: {}: {e}", style::yellow(what)));
            value["ready"] = json!({ "on": null, "changed": false, "error": e.to_string() });
        }
        None => {}
    }
    (text.join("\n"), value)
}

fn led_cmd(out: &Out, m: &mut M2k, args: LedArgs, timeout: Duration) -> Result<()> {
    let ssh = led::Ssh {
        password: args.ssh_password.unwrap_or_default(),
        askpass: std::env::current_exe().ok(),
        known_hosts: args.ssh_known_hosts,
        timeout,
        program: "ssh".into(),
    };
    let only = args.only.map(led::Led::from);
    let cmd = args.cmd.unwrap_or(LedCmd::Status);
    let status = match cmd {
        LedCmd::Status => led::status(m, only, &ssh)?,
        LedCmd::Off => led::set(m, false, only, &ssh)?,
        LedCmd::On => led::set(m, true, only, &ssh)?,
    };
    let changing = !matches!(cmd, LedCmd::Status);
    let (mut text, value) = led_text(&status, changing);
    if matches!(cmd, LedCmd::Off) {
        text += &format!(
            "\n{}",
            style::dim("until the M2K is power cycled; `m2k led on` brings them back")
        );
    }
    out.print(text, value);
    if changing && status.partial() {
        // the result is printed in full; the exit code says that not everything asked for was done
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses() {
        Cli::command().debug_assert();
        let cli = Cli::try_parse_from([
            "m2k", "gen", "-c", "1,2", "--for", "2s", "sine", "-f", "1k", "-a", "0.5", "-o", "-0.2",
        ])
        .unwrap();
        let Cmd::Gen(g) = cli.cmd else { panic!() };
        assert_eq!(g.channel, Outputs(vec![0, 1]));
        let Shape::Sine(p) = g.shape else { panic!() };
        assert_eq!((p.freq, p.amp, p.offset), (1000.0, 0.5, -0.2));
        let cli = Cli::try_parse_from(["m2k", "gen", "const", "-1.5"]).unwrap();
        let Cmd::Gen(g) = cli.cmd else { panic!() };
        assert!(matches!(g.shape, Shape::Const { volts } if volts == -1.5));
        assert!(Cli::try_parse_from(["m2k", "gen", "-c", "0", "off"]).is_err());
        let cli = Cli::try_parse_from(["m2k", "psu", "set", "--neg", "-3.3"]).unwrap();
        assert!(
            matches!(cli.cmd, Cmd::Psu { cmd: Some(PsuCmd::Set { pos: None, neg: Some(v) }) } if v == -3.3)
        );
        let cli =
            Cli::try_parse_from(["m2k", "scope", "-n", "100k", "-r", "10M", "--range2", "2.5"])
                .unwrap();
        let Cmd::Scope(s) = cli.cmd else { panic!() };
        assert_eq!((s.samples, s.rate), (100_000, 10e6));

        let cli = Cli::try_parse_from([
            "m2k",
            "logic",
            "--trigger",
            "3:rising",
            "--trigger",
            "7:low",
            "--trigger-and",
            "--trigger-delay",
            "-100",
            "--vcd",
            "/tmp/x.vcd",
        ])
        .unwrap();
        let Cmd::Logic(l) = cli.cmd else { panic!() };
        assert_eq!(
            l.trigger,
            [
                (3, TriggerCondition::EdgeRising),
                (7, TriggerCondition::LevelLow)
            ]
        );
        assert!(l.trigger_and);
        assert_eq!(l.trigger_delay, -100);
        assert!(Cli::try_parse_from(["m2k", "logic", "--trigger-and"]).is_err());

        let cli = Cli::try_parse_from([
            "m2k",
            "digital",
            "out",
            "pattern.csv",
            "-r",
            "1M",
            "--open-drain",
            "0,1",
            "--cyclic",
            "--for",
            "5s",
        ])
        .unwrap();
        let Cmd::Digital {
            cmd: DigitalCmd::Out(d),
        } = cli.cmd
        else {
            panic!()
        };
        assert_eq!(d.rate, 1e6);
        assert_eq!(d.open_drain.unwrap().0, [0, 1]);
        assert!(d.cyclic);
        assert!(
            Cli::try_parse_from([
                "m2k",
                "digital",
                "out",
                "pattern.csv",
                "-r",
                "1M",
                "--for",
                "5s"
            ])
            .is_err()
        );
    }

    #[test]
    fn led_parses_and_keeps_the_password_out_of_debug() {
        let cli = Cli::try_parse_from(["m2k", "led"]).unwrap();
        let Cmd::Led(l) = cli.cmd else { panic!() };
        assert!(l.cmd.is_none() && l.only.is_none());
        let cli = Cli::try_parse_from([
            "m2k",
            "led",
            "off",
            "--only",
            "ready",
            "--ssh-password",
            "s3cret-not-analog",
            "--ssh-known-hosts",
            "/tmp/kh",
        ])
        .unwrap();
        assert!(!format!("{cli:?}").contains("s3cret"));
        let Cmd::Led(l) = cli.cmd else { panic!() };
        assert!(matches!(l.cmd, Some(LedCmd::Off)));
        assert!(matches!(l.only, Some(LedArg::Ready)));
        assert_eq!(
            l.ssh_password,
            Some(led::Password::new("s3cret-not-analog"))
        );
        assert!(Cli::try_parse_from(["m2k", "led", "--only", "both"]).is_err());
        // the help names the env var, never its value
        let help = Cli::command()
            .find_subcommand_mut("led")
            .unwrap()
            .render_long_help()
            .to_string();
        assert!(help.contains("M2K_SSH_PASSWORD"), "{help}");
        assert!(help.contains("power cycle"), "{help}");
    }

    #[test]
    fn led_output_names_the_led_that_was_not_changed() {
        let ok = led::Status {
            done: Some(false),
            ready: Some(Ok(led::Ready {
                on: true,
                trigger: "heartbeat".into(),
                brightness: 255,
            })),
        };
        let (text, value) = led_text(&ok, true);
        assert_eq!(text, "done: off\nready: on (blinking, trigger heartbeat)");
        assert_eq!(
            value,
            json!({
                "done": { "on": false },
                "ready": { "on": true, "trigger": "heartbeat", "brightness": 255 },
                "partial": false,
            })
        );

        let why = led::Status {
            done: Some(false),
            ready: Some(Err(m2k::Error::Invalid("no ssh".into()))),
        };
        let (text, value) = led_text(&why, true);
        assert_eq!(text, "done: off\nready: not changed: no ssh");
        assert_eq!(value["partial"], true);
        assert_eq!(value["ready"]["changed"], false);
        assert_eq!(value["ready"]["error"], "no ssh");
        assert!(led_text(&why, false).0.contains("ready: unknown: no ssh"));

        let only_done = led::Status {
            done: Some(true),
            ready: None,
        };
        let (text, value) = led_text(&only_done, false);
        assert_eq!(text, "done: on");
        assert!(value["ready"].is_null());
    }

    fn write_temp(name: &str, content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("m2k_cli_test_{}_{name}", std::process::id()));
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn digital_csv_roundtrip() {
        let path = write_temp(
            "pattern.csv",
            "time_s,dio0,dio3\n0.0,0,0\n0.1,1,0\n0.2,1,1\n",
        );
        let (pins, words) = read_digital_csv(&path).unwrap();
        assert_eq!(pins, [0, 3]);
        assert_eq!(words, [0b0000, 0b0001, 0b1001]);
        std::fs::remove_file(&path).unwrap();

        let bad = write_temp("bad.csv", "a,b\n1,2\n");
        assert!(read_digital_csv(&bad).is_err());
        std::fs::remove_file(&bad).unwrap();
    }

    #[test]
    fn vcd_format() {
        let path = std::env::temp_dir().join(format!("m2k_cli_test_{}.vcd", std::process::id()));
        write_vcd(&path, &[0, 1], &[0b00, 0b01, 0b01, 0b11], 1e6).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        // one tick per sample at 1 MS/s, so sigrok reads 1 MHz as the sample rate
        assert!(text.contains("$timescale 1 us $end"));
        assert!(text.contains("$var wire 1 ! dio0 $end"));
        assert!(text.contains("$var wire 1 \" dio1 $end"));
        assert!(text.contains("#0\n$dumpvars\n0!\n0\"\n$end\n"));
        // sample 1 changes dio0 to 1; sample 2 repeats, so no entry; sample 3 changes dio1 too
        assert!(text.contains("#1\n1!\n"));
        assert!(!text.contains("#2\n"));
        assert!(text.contains("#3\n1\"\n"));
        // the capture ends after 4 samples
        assert!(text.ends_with("#4\n"));
    }

    #[test]
    fn vcd_timescales() {
        assert_eq!(vcd_timescale(1_000_000), ("1 us".into(), 1));
        assert_eq!(vcd_timescale(10_000), ("10 ns".into(), 1));
        assert_eq!(vcd_timescale(100_000_000), ("100 us".into(), 1));
        assert_eq!(vcd_timescale(1_000_000_000), ("1 ms".into(), 1));
        // 3 MS/s: 333333 ps, no round unit
        assert_eq!(vcd_timescale(333_333), ("1 ps".into(), 333_333));
    }
}
