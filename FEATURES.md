# m2k_cli features and roadmap

This file is the single source of truth for what m2k_cli does, what is broken and what is planned.
[CHANGELOG.md](CHANGELOG.md) records what changed and when; this file records the current state.

## How to use this file

- **Status**: ✅ done (verified on a real M2K) · 🧪 done, verified on iio-emu only or not with a real signal ·
  🚧 partial (the note says what is missing) · 🐛 known bug · 📋 planned · 💡 idea · ⛔ blocked (on what) ·
  🔍 needs a check.
- **IDs** (`SCOPE-3`) are stable: never renumber or reuse one. New ids come from `tpm work claim AREA`. Use the ID
  in commit messages, CHANGELOG entries and code `TODO`s (`// TODO(SCOPE-3): ...`); name it with a short slug
  when talking to people (`SCOPE-3 vertical-offset`).
- When finishing work, update the item in the same commit. Obsolete items go to *Dropped and superseded*.

Real-device checks so far: one ADALM2000 Rev.D, firmware v0.27 (iiod 0.21), over its USB network, with W1 wired
to 1+, W2 to 2+, 1-/2- to GND, nothing on the digital pins or supplies (9 Oct 2026). Emulator: iio-emu 0.2.0
`adalm2000` profile (Rev.C, firmware v0.26).

## iiod client (`IIO`, crate `crates/iiod_client`)

- ✅ **IIO-1 Text protocol over TCP**: `VERSION`, `PRINT`, `TIMEOUT`, `READ`/`WRITE` of device, channel, debug
  and buffer attributes, `SET BUFFERS_COUNT`, `OPEN` (cyclic too), `READBUF`, `WRITEBUF`, `CLOSE`, `EXIT`;
  errno answers become typed errors with the Linux errno name (`client.rs`). Pure Rust, std only (plus
  roxmltree). Verified against iio-emu (tinyiiod) and the M2K's iiod 0.21.
- ✅ **IIO-2 Context model**: the XML from `PRINT` as devices, channels, attributes, scan elements and sample
  formats (`le:s12/16>>0`, repeat, big endian); lookup by id or name (`context.rs`).
- ✅ **IIO-3 Sample layout**: channel mask from channels, sample size and per-channel offsets the way the kernel
  lays them out (channels sharing a scan index share storage, like the M2K's 16 digital pins), decode and
  encode of samples.
- 📋 **IIO-4 Binary protocol of libiio 1.x**: iiod 1.x still answers the text protocol (libiio 1.x clients
  switch to binary with `BINARY`); needed only for features the text protocol lacks (events, several buffers
  per device).
- 📋 **IIO-5 Other transports**: USB without the network (iiod's USB function interface), serial, `ZPRINT`
  (zstd context), DNS-SD discovery of `_iio._tcp`.
- 📋 **IIO-6 Reuse for the ADALM-Pluto skill** (P2618#7oct-0032): the crate has no M2K specifics; publish it or
  share it as a path/git dependency, then add the Pluto's buffers (complex 16-bit I/Q, `cf-ad9361-lpc`).
- 💡 **IIO-7 Triggers**: `GETTRIG`/`SETTRIG` are not wrapped (the M2K needs none).

## Device (`DEV`)

- ✅ **DEV-1 info**: model, serial, firmware, kernel, iiod version, calibration state, temperature, outputs and
  supplies on/off, device count; `--devices` lists devices and channels; `--json` has all context attributes.
  Works with any IIO context (no M2K parts then).
- ✅ **DEV-2 attr**: list devices, read every attribute of a device or channel, read or write one (`-c`, `-o`,
  `--debug`, `--buffer`); generic for any IIO device.
- ✅ **DEV-3 Power-up**: every M2K command first powers the scope front end and clock and sets the AD9963 output
  gain registers, as libm2k does when it opens an M2K (`M2k::init`). Without it a freshly booted M2K has its
  scope inputs powered down. Nothing is powered down at the end (libm2k does that on close); see CAL-4.

## Power supplies (`PSU`)

- ✅ **PSU-1 psu**: `psu` (status with measured voltage), `psu set --pos V --neg V` (sets and switches on, factory
  `cal,*` corrections applied like libm2k), `psu off [--pos|--neg]`; on/off reads both the DAC and the fabric
  switch. Supplies stay on after the command. Verified on the real M2K from 0 to ±5 V (measured within 10 mV on
  the M2K's own monitor ADC; not yet checked with an external meter).

## Monitors and voltmeter (`DMM`)

- ✅ **DMM-1 dmm**: every monitor channel of `ad9963` and `xadc` (supplies, temperatures, FPGA rails) as
  (raw + offset) × scale / 1000, filter by label.
- ✅ **DMM-2 voltmeter**: DC (mean) and AC rms of both scope inputs over 100 ms at 100 kS/s (whole mains periods),
  `--range 25|2.5`. Verified with the loopback: within 8 mV in the ±2.5 V range after `cal run`.

## Scope (`SCOPE`)

- ✅ **SCOPE-1 scope**: free-running capture of both channels, `-n` (up to 8M), `-r` (native 1k..100M, other rates
  by the ADC's oversampling), `--range`/`--range1`/`--range2`, stats per channel (mean, min, max, vpp, rms, AC
  rms, frequency and duty from mid-level crossings with hysteresis and a 10-LSB noise floor, clipping count),
  warnings when uncalibrated or clipped, `--csv`. Volts with libm2k's formula (range gain, filter compensation,
  vertical offset, `calibscale` on firmware 0.32+). Verified on the real M2K: 1 kHz sine and 10 kHz 25 % square
  from W1/W2 measured right; same DC reading at every native rate.
- 📋 **SCOPE-2 Trigger**: analog level/edge trigger and delay (`m2k-adc-trigger`); captures are free-running.
- 📋 **SCOPE-3 Vertical offset**: `--offset` per channel (offset DAC), conversion already in `conv.rs`.
- 💡 **SCOPE-4 Long captures**: stream several buffers to a file for captures beyond one buffer.

## Signal generator (`GEN`)

- ✅ **GEN-1 gen**: W1/W2 (`-c 1,2`), `const`, `sine`, `square` (`--duty`), `triangle`, `sawtooth` with
  `-f`/`-a` (peak)/`-o`/`--phase`; the DAC rate and whole-period buffer length are picked so the frequency is
  exact or closest (`conv::plan_wave`); runs until Ctrl+C/SIGTERM or `--for`, then switches the outputs off;
  `gen off`. Verified on the real M2K through the loopback after `cal run`: constant -4..+2.5 V within 1.5 %
  in the ±25 V range, 0, ±0.5, 1 V within 8 mV in the ±2.5 V range; sine frequency exact.
- 📋 **GEN-2 Output without a running process**: the cyclic buffer belongs to the iiod connection, so the output
  ends with the `m2k gen` process (killed with SIGKILL, the amplifier stays on at ~0 V until `gen off`).
  Firmware 0.32+ has `raw`/`raw_enable` for a constant without a buffer; a daemon on the M2K (DAEMON-1) could
  hold buffers.
- 💡 **GEN-3 Arbitrary waveform** from a CSV file, and a capture-while-generating mode in one command.

## Logic analyzer (`LOGIC`)

- 🧪 **LOGIC-1 logic**: free-running capture of DIO0-15 (`-n`, `-r`, `-p`), per pin low/high/toggling, edges,
  frequency, share high, shortest pulse, pin direction; `--csv`. Runs on the real M2K (open pins read high) and
  the emulator (pins read low); not yet checked with a real signal on a pin.
- 📋 **LOGIC-2 Digital outputs**: set pins, play patterns (`m2k-logic-analyzer-tx`, `direction`).
- 📋 **LOGIC-3 Trigger**: edge/level per pin (`trigger` attributes of `m2k-logic-analyzer-rx`).
- 💡 **LOGIC-4 Protocol decode**: UART/SPI/I2C summaries like saleae_cli's, from a capture.

## Calibration (`CAL`)

- ✅ **CAL-1 cal run**: libm2k's self-calibration step by step: scope offset (inputs grounded, coarse estimate
  and a 21-value fine search), scope gain (internal 0.46172 V reference), output offset and gain (outputs
  looped inside, codes 0 and 1024); `--adc-only`; implausible results are refused and the previous corrections
  put back. Takes 1.3 s on the real M2K; afterwards the loopback agrees (GEN-1). The emulator does not model
  the calibration modes: there `cal run` fails cleanly, which the tests check.
- 🧪 **CAL-2 cal lut**: applies the factory table (`cal,temp_lut`) row nearest the board temperature, as
  libm2k's `calibrateFromContext`. Parser unit-tested; neither the test M2K nor the emulator has a table.
- ✅ **CAL-3 cal status / cal reset**: the corrections the M2K holds (`calibbias`, `calibscale`, offset DACs)
  and whether they differ from power-on (libm2k's `isCalibrated` test); reset to power-on values.
- 🔍 **CAL-4 ±25 V range offset**: after `cal run` the ±25 V range reads about -60 mV at 0 V on the test board
  (±2.5 V range: within 2 mV); the offset DAC step is 30 mV there. Check what Scopy/libm2k read on the same
  board before treating it as a bug.

## CLI (`CLI`)

- ✅ **CLI-1 Version**: `--version` prints version, git SHA (`-dirty`) and build time (`build.rs`).
- ✅ **CLI-2 Shell completion**: dynamic, `source <(COMPLETE=bash m2k)` (zsh, fish, elvish, powershell); device,
  channel and attribute names live from the device at `M2K_URI` (300 ms), else the M2K's known names; the Bash
  adapter from wire_weaver_cli/saleae_cli quotes values with spaces.
- ✅ **CLI-3 JSON output**: `--json` on every command (one object per line), errors as `{"error": ...}` with exit
  code 1; text output is short and line-oriented.
- ✅ **CLI-4 Colour and progress**: colour on a terminal without `NO_COLOR`; progress bars for captures over 1 s or
  500k samples, a spinner with the current step for `cal run`; nothing of either with `--json` or when not a
  terminal.

## Agent skill (`SKILL`)

- 🧪 **SKILL-1 skills/m2k/SKILL.md**: what to ask about wiring, recipes per task, reading the output, safety with
  outputs and supplies, what is verified where. Not yet used on a real debugging session.
- 📋 **SKILL-2 Skill tree**: usage signals and skilltree entries for the `m2k` commands (P2618 task).

## On-device daemon (`DAEMON`)

- 💡 **DAEMON-1 Rust daemon on the M2K** (Zynq-7010, Buildroot, ADI bitstream kept): on-device triggers and
  measurements, outputs that outlive the client; custom HDL only with a concrete need (task P2618#9oct-1207).

## Dropped and superseded

(none yet)
