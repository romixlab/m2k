---
name: m2k
description: Measure and drive circuits with an Analog Devices ADALM2000 (M2K) through the `m2k` CLI: 2-channel scope, 2-channel signal generator, ±5 V supplies, voltmeter, 16-pin logic analyzer, self-calibration. Use when the user wants to measure a voltage or signal, look at a waveform, check a frequency, power a small circuit, generate a test signal, or check digital pins with an M2K / ADALM2000 / "the Analog Devices board".
---

# ADALM2000 (M2K)

The `m2k` CLI (source: https://github.com/romixlab/m2k_cli; `cargo install --path .` in a checkout) talks to the
iiod daemon on the M2K over its USB network (`192.168.2.1`), no libiio or Scopy needed. Every command takes
`--json` (errors become `{"error": "..."}` with exit code 1); text output is one fact per line.

## 0. Check the device (once per session)

```sh
m2k info
```

Shows model, firmware, `calibration: ...`, temperature, and whether outputs W1/W2 and supplies V+/V- are on.
Unreachable: the M2K is not plugged in, still booting (~20 s), or its USB network has no address on this PC
(the host should get 192.168.2.10 by DHCP). Another address: `--uri ip:HOST` or `M2K_URI`.

If it says `not calibrated since power-up`, run `m2k cal run` (~1.5 s; drives W1/W2 to about 2.5 V for a
moment, so ask first if something sensitive is on W1/W2). Uncalibrated readings can be 100+ mV off, and the
generator's amplitude can be 50 % off. The M2K keeps the calibration until it loses power.

## 1. Ask before driving anything

You cannot see the bench. Reading is harmless; outputs and supplies are not. Ask, in one message, only what you
don't know:

- **What is wired where.** Scope: 1+/1- and 2+/2- are differential inputs; for a normal measurement 1- (2-) goes
  to the circuit's ground. Generator: W1, W2 against GND. Supplies: V+ (0..5 V), V- (-5..0 V), up to ~50 mA.
  Digital: DIO0-15, 3.3 V logic (5 V tolerant); `digital out` switches a pin to an output for as long as it
  runs and back to an input (tri-state) when it stops — check nothing else is driving that pin at the same
  time, and use `--open-drain` for buses that need it (I2C, 1-Wire) so two drivers never fight.
- **Allowed levels** for anything you drive, and the expected range of what you measure (±2.5 V range for small
  signals, ±25 V otherwise).
- **For signals**: expected frequency, so you can pick the sample rate (10x the highest frequency of interest,
  and enough samples for several periods).

After a test, switch outputs off again: `m2k gen -c 1,2 off`, `m2k psu off`. Don't leave supplies on unless the
user asks to power their circuit.

## 2. Recipes

```sh
m2k voltmeter                         # DC + AC rms on both inputs, ±25 V range
m2k voltmeter --range 2.5             # finer: below ±2.5 V
m2k scope -n 10k -r 1M                # 10 ms at 1 MS/s: stats per channel
m2k scope -n 100k -r 10M --range1 2.5 --csv /tmp/cap.csv   # all samples to CSV (time_s,ch1_v,ch2_v)
m2k psu set --pos 3.3                 # V+ on at 3.3 V (prints the measured value); --neg -5 for V-
m2k psu off
m2k gen -c 1 --for 30s sine -f 1k -a 0.5 -o 1     # W1: 1 kHz, 0.5 V peak around 1 V, for 30 s
m2k gen -c 1,2 --for 10s const 1.2                # both outputs at 1.2 V
m2k gen -c 2 --for 10s square -f 10k -a 1.65 -o 1.65 --duty 25   # 0..3.3 V logic-level clock
m2k logic -n 100k -r 10M -p 0-7       # DIO0-7: state, edges, frequency per pin
m2k logic -n 1M -r 10M --trigger 3:rising --trigger-delay -1000 --vcd /tmp/cap.vcd   # wait for DIO3 to rise, keep 1000 samples before it (at most 8192)
m2k digital out pattern.csv -r 1M --open-drain 0,1 --cyclic --for 5s   # play a dioN-column CSV on its pins, 0/1 open-drain
m2k dmm                               # board monitors: supply rails, temperatures
m2k attr m2k-adc                      # any IIO attribute (read; add a value to write)
```

`gen` holds its output only while it runs: start it in the background (or with `--for`), measure, then let it
end. While generating, measure from a second shell:

```sh
m2k gen -c 1 --for 5s sine -f 1k -a 1 &   # background
sleep 1; m2k scope -n 100k -r 10M          # W1 wired to 1+: see it on ch1
wait
```

Sample rates: native 1k, 10k, 100k, 1M, 10M, 100M S/s; others are made by averaging (the warning says what you
got). A capture holds up to 8M samples per channel. Sine frequencies are exact when the output rate divides
into whole periods; the `gen` line prints the real frequency.

`logic` and `digital out` share one clock divided from 100 MS/s across all 16 pins: asking for fewer pins
(`-p`, or fewer `dioN` columns) does not raise the limit (unlike an 8-channel Saleae, which gets faster with
fewer channels). The divider rounds to the nearest rate it can hit; a warning says the real one used.

## 3. Read the output

```
scope: 100000 samples at 10 MS/s (10 ms), ranges ±25 V / ±25 V, calibrated
ch1: 1 kHz (9 periods), duty 49 %, mean -0.0532 V, vpp 2.0894 V, min -1.0828 V, max +1.0066 V, rms 0.7154 V, ac rms 0.7134 V
ch2: mean +0.1603 V, vpp 0.0320 V, min +0.1438 V, max +0.1758 V, rms 0.1603 V, ac rms 0.0021 V
```

- A frequency appears only for signals swinging more than ~10 ADC steps (≈ 0.14 V in the ±25 V range,
  ≈ 15 mV in ±2.5 V); a line without one is DC or noise. `periods` is how many whole periods were seen: fewer
  than ~3 is a rough estimate, capture longer.
- `vpp` of ~30 mV with nothing connected is the noise of the ±25 V range; use `--range 2.5` for small signals.
- `warning: chN hit the end of the ±2.5 V range`: the signal is bigger than shown, use `--range 25`.
- `warning: ... not calibrated`: run `m2k cal run` and measure again.
- Logic: `high: DIO ...` / `low: DIO ...` list static pins; toggling pins get their own line. Unconnected pins
  read high. `(output)` marks pins configured as outputs. A triggered capture prints a `trigger: ...` line with
  which pin(s) fired, the delay and which sample in the capture it landed on (`sample_index` in `--json`);
  without `--trigger` the capture starts right away as before.
- Accuracy after `cal run` (measured through a W1→1+ loopback): ±2.5 V range within ~10 mV; ±25 V range within
  ~1.5 % plus up to ~70 mV offset.

## 4. Report to the user

Say what you measured in plain words (levels, frequency, ripple, what changed when), the setup (range, rate,
what was driven), whether the M2K was calibrated, and the likely meaning. Quote the key lines, not whole dumps;
offer the CSV path for a closer look.

## Limits (prototype 0.1)

- Scope captures still have no trigger (SCOPE-2): they start right away. To catch an event, capture longer and
  look in the CSV. Digital captures do have a trigger now (LOGIC-3, see `m2k logic --trigger`).
- No arbitrary analog waveforms, no scope vertical offset yet.
- A killed `m2k gen` (SIGKILL) leaves the output amplifier on at about 0 V: `m2k gen -c 1,2 off`. A killed
  `m2k digital out` leaves its pins driving the last sample: `m2k attr m2k-logic-analyzer -c voltageN direction in`
  per pin tri-states it again.
- Verified on a real M2K (Rev.D, firmware 0.27): info, attr, psu (on its own monitor), dmm, voltmeter, scope
  and gen through a loopback, cal run/status/reset, logic with open pins, `digital out` and `logic --trigger`
  self-contained on DIO0 (push-pull and open-drain, trigger firing where predicted). Not yet verified: `cal lut`
  (no board at hand had a factory table), supplies against an external meter, VCD against sigrok-cli.
