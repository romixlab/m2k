# m2k features and roadmap

This file is the single source of truth for what m2k does, what is broken and what is planned.
[CHANGELOG.md](CHANGELOG.md) records what changed and when; this file records the current state.

## How to use this file

- **Status**: ✅ done (verified on a real M2K) · 🧪 done, verified on iio-emu only or not with a real signal ·
  🚧 partial (the note says what is missing) · 🐛 known bug · 📋 planned · 💡 idea · ⛔ blocked (on what) ·
  🔍 needs a check.
- **IDs** (`SCOPE-3`) are stable: never renumber or reuse one. New ids come from `tpm work claim AREA`. Use the ID
  in commit messages, CHANGELOG entries and code `TODO`s (`// TODO(SCOPE-3): ...`); name it with a short slug
  when talking to people (`SCOPE-3 vertical-offset`). Areas: `IIO`, `DEV`, `PSU`, `DMM`, `SCOPE`, `GEN`,
  `LOGIC`, `CAL`, `CLI`, `PY`, `SKILL`, `DAEMON`.
- When finishing work, update the item in the same commit. Obsolete items go to *Dropped and superseded*.

Real-device checks so far: one ADALM2000 Rev.D, firmware v0.27 (iiod 0.21), over its USB network, with W1 wired
to 1+, W2 to 2+, 1-/2- to GND, nothing external on the digital pins or supplies (9 Oct 2026). Emulator: iio-emu
0.2.0 `adalm2000` profile (Rev.C, firmware v0.26).

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
- ✅ **DEV-4 LEDs** (P2618#13oct-1031): `m2k led off|on|status` for the two blue LEDs, `--only done|ready` for
  one of them; RAM only, both are back on after a power cycle. The steady **DONE** LED through iiod
  (`m2k-fabric` out:voltage4 `done_led_overwrite_powerdown`, 1 = off, what libm2k's `setLed` writes:
  `M2k::done_led`/`set_done_led`). The blinking **Ready** LED is the kernel's `led0:green` with the `heartbeat`
  trigger, which iiod cannot reach: `m2k::led` runs the system `ssh` (OpenSSH 8.4 or later, no new dependency)
  as `root@HOST` (HOST from `--uri`) with fixed commands (`echo none > trigger; echo 0 > brightness`, or `echo
  heartbeat > trigger`, then reading both back). The password (`--ssh-password` / `M2K_SSH_PASSWORD`, default
  the factory `analog`) reaches ssh through the `m2k` binary acting as its own `SSH_ASKPASS` helper
  (`led::askpass_main`), handed over in the environment of the ssh child only: no file, not in argv, not in
  any output; the helper answers password prompts only. ssh runs with `-F none`, password authentication only
  (no key or agent of the user is offered). Host key: not checked and nothing written to a `known_hosts` file
  with the factory password (public password, every M2K at the same 192.168.2.1, fixed commands: nothing for a
  fake host to learn); another password is only sent with `--ssh-known-hosts FILE` / `M2K_SSH_KNOWN_HOSTS`
  (a file of the user's choice, never `~/.ssh/known_hosts`), which pins the key on first use and refuses a
  changed one. No ssh, no login or a changed key: the DONE LED is still switched, the output names the Ready
  LED as `not changed` with the reason (`"partial": true` in JSON) and `off`/`on` exit with 1; `status` shows
  it as `unknown` and exits with 0. `led` does not run the power-up (DEV-3). Verified on the Rev.D, firmware
  v0.27 (dropbear 2019.78), from a PC with OpenSSH 10.6, 13 Oct 2026: off/status/on/status per LED and both
  (read back through the attribute and sysfs; nobody looked at the LEDs during this run, the mapping to the
  two LEDs was checked by hand before), missing ssh, wrong password, pinned and changed host key,
  `tests/device.rs::leds`; `tests/emu.rs::leds` with a stand-in ssh, `tests/askpass.rs` and unit tests for the
  argument building. Not checked: OpenSSH on Windows.

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

- ✅ **SCOPE-1 scope**: free-running capture of both channels, `-n` (up to 4 Mi = 4194304 per channel: the two channels share a 16 MiB
  buffer, one more sample and the driver answers EFBIG; was given as 8M until 10 Oct 2026), `-r` (native 1k..100M, other rates
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
- ✅ **LOGIC-2 Digital outputs**: `m2k digital out FILE.csv -r RATE`: a pattern from a CSV sample buffer
  (`dioN` columns, the format `logic --csv` writes), per-pin push-pull or open-drain (`--open-drain`), cyclic or
  one-shot (`--cyclic`/`--for`, else plays once); pins switch to `out` while running and back to `in` (tri-state)
  on stop, Ctrl+C or error. Verified on the real M2K: DIO0 driven push-pull and open-drain, `direction`/
  `outputmode` round-trip, pin tri-states again after a one-shot buffer (`tests/device.rs::digital_self_loopback`,
  9 Oct 2026); on the emulator: attribute round-trip, buffer accept/reject.
- ✅ **LOGIC-3 Trigger**: `logic --trigger PIN:COND` (repeatable; none/any/rising/falling/low/high),
  `--trigger-and` (AND instead of OR across pins), `--trigger-delay` (negative = pretrigger samples kept,
  positive = delay after the event), `--trigger-wait` (extra time to wait for it). The pretrigger is at most
  8192 samples at every rate (found on the real M2K, 9 Oct 2026: the driver answers EINVAL from -8193 on); more
  is refused with that limit in the message. Attribute names and the
  divider-based `trigger_delay` sign convention are from the kernel driver
  (`drivers/iio/logic/m2k-logic-analyzer.c`), not libm2k. The emulator does not model digital inputs or
  triggers (AGENTS.md), so there it only exercises the attributes; verified on the real M2K: a `digital out`
  pattern on DIO0 (push-pull and open-drain) read back by `logic` on the same physical pin, including a
  rising-edge trigger firing at `sample_index` 0 as predicted, with no external wiring needed
  (`tests/device.rs::digital_self_loopback`, 9 Oct 2026).
- 💡 **LOGIC-4 Protocol decode**: UART/SPI/I2C summaries like saleae_cli's, from a capture.
- 🧪 **LOGIC-5 VCD export**: `logic --vcd FILE.vcd` alongside `--csv`: one wire per reported pin, `dioN` names,
  picosecond timescale, only the samples where a reported pin changed (`$dumpvars` for the initial state).
  For sigrok and other waveform viewers to import captures (cross-check plan P2623). Not yet confirmed against
  `sigrok-cli` (not installed on this machine yet).
- 🐛 **LOGIC-6 emu test vs. current iio-emu HEAD**: `tests/emu.rs::digital_out_and_logic_trigger` expects the VCD
  timescale `1 ps` for `logic -n 1000 -p 0-3` (no `-r`, so the default `1M`); against a freshly built iio-emu
  (git HEAD, 10 Oct 2026, not the pinned 0.2.0 `adalm2000` profile AGENTS.md records) it fails the same way on
  main before this split, so it is a pre-existing emulator-version drift, not a regression (found while
  verifying CLI-5). The other four `tests/emu.rs` tests and all `tests/device.rs` pass. Needs a pinned
  iio-emu version or a look at what rate the logic analyzer now negotiates there.
- Max sample rate: both `logic` and `digital out` share one clock divided down from 100 MS/s for however many
  of the 16 pins are involved; asking for fewer pins (`-p`) does not raise the limit, unlike e.g. a Saleae
  Logic 8 (saleae_cli). The rate is rounded to the nearest the divider can hit; a warning says the real rate
  used.

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
- 🧪 **CLI-5 Library-first split** (P2618#10oct-1107c): `crates/m2k` is the typed API (device, conversions,
  calibration, capture summaries, and the typed calls `scope`/`gen`/`logic`/`psu`/`dmm` behind each CLI
  command) with no `anyhow`/`serde_json`/`clap` dependency and its own `m2k::Error` (thiserror); `crates/m2k_cli`
  is the same `m2k` binary as before (unchanged commands, flags, JSON output, shell completion) now calling
  into it; `crates/m2k_py` is new (PY-1). No functional change to any CLI command. Verified: `cargo test
  --workspace` (all unit tests), `crates/m2k_cli/tests/emu.rs` against a freshly built iio-emu (4 of 5 pass;
  the fifth is LOGIC-6, pre-existing on main too) and `tests/device.rs` (3 of 3, skipped without
  `M2K_TEST_URI`); not yet rerun against a real M2K since the split (the underlying device/calibration code
  is unchanged from what was checked there before, 9-10 Oct 2026).

## Python module (`PY`)

- 🧪 **PY-1 m2k_py** (P2618#10oct-1107c): `crates/m2k_py`, a PyO3 module (abi3, Python >= 3.9) wrapping
  `crates/m2k`'s typed API: `M2k` class with `scope`, `voltmeter`, `logic`, `gen_start`/`gen_stop`,
  `psu_status`/`psu_set`/`psu_off`, `dmm`, `cal_run`/`cal_status`/`cal_reset`, `temperature`; data classes for
  each typed call's result; errors as `M2kError`. `gen_start` takes a plain sample array (build it with numpy)
  rather than the CLI's shape flags — no Python equivalent of `gen::build_wave` yet. Verified:
  `tests/test_smoke.py` (offline: module shape and error mapping, no M2K or emulator) via `just test-py`; not
  yet exercised against the emulator or a real M2K.
- 🧪 **PY-2 LEDs**: `M2k.led_status`/`led_off`/`led_on` (`only`, `ssh_password`, `ssh_known_hosts`) returning
  `LedStatus` (`done`, `ready`, `ready_trigger`, `ready_error`), on top of DEV-4. The askpass helper is the
  `m2k` CLI found on PATH; without it the Ready LED is reported in `ready_error` and the DONE LED still
  switches. Verified: `tests/test_smoke.py` (the methods exist); not yet run against a real M2K.

## Agent skill (`SKILL`)

- 🧪 **SKILL-1 skills/m2k/SKILL.md**: what to ask about wiring, recipes per task, reading the output, safety with
  outputs and supplies, what is verified where. Not yet used on a real debugging session.
- 📋 **SKILL-2 Skill tree**: usage signals and skilltree entries for the `m2k` commands, `m2k led` (DEV-4)
  included (P2618 task).

## On-device daemon (`DAEMON`)

- 💡 **DAEMON-1 Rust daemon on the M2K** (Zynq-7010, Buildroot, ADI bitstream kept): on-device triggers and
  measurements, outputs that outlive the client; custom HDL only with a concrete need (task P2618#9oct-1207).

## Dropped and superseded

(none yet)
