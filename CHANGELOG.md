# Changelog

All notable changes to m2k_cli are recorded here, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). [FEATURES.md](FEATURES.md) holds the current status of
every feature; IDs in parentheses refer to it.

## [Unreleased]

### Fixed

- Builds again after the 0.3.0 release: both crates take their version from `[workspace.package]`
  (`version.workspace = true`), so a release bumps them together.

## [0.3.0] - 2026-10-09

### Added

- `m2k digital out FILE.csv -r RATE`: a digital pattern generator from a CSV sample buffer (the format
  `logic --csv` writes), per-pin push-pull or open-drain (`--open-drain`), cyclic or one-shot (`--cyclic`,
  `--for`) (LOGIC-2).
- `m2k logic --trigger PIN:COND` (repeatable): edge/level trigger per pin, `--trigger-and`,
  `--trigger-delay` (pretrigger/delay in samples), `--trigger-wait` (LOGIC-3).
- `m2k logic --vcd FILE.vcd`: a Value Change Dump of the capture alongside `--csv`, for sigrok and other
  waveform viewers (LOGIC-5).

## [0.2.1] - 2026-10-09

### Fixed

- Builds again: `iiod_client` carries the workspace version (the 0.2.0 release bumped only the requirement on
  it), and LICENSE-APACHE names the right copyright holder.

## [0.2.0] - 2026-10-09

### Added

- `iiod_client` crate: a pure-Rust client for the iiod text protocol over TCP (attributes, buffers, context XML,
  sample layout and formats), no libiio; meant to be shared with the ADALM-Pluto skill (IIO-1, IIO-2, IIO-3).
- `m2k` CLI for the ADALM2000 at `ip:192.168.2.1` (`--uri`, `M2K_URI`, `--timeout`):
  - `m2k info [--devices]` and `m2k attr` for any IIO attribute, also of other IIO devices (DEV-1, DEV-2);
    every M2K command powers up the scope front end and clock like libm2k (DEV-3).
  - `m2k psu [status|set --pos V --neg V|off]` with the factory supply corrections (PSU-1).
  - `m2k dmm` (board monitors) and `m2k voltmeter` (DC and AC rms of the scope inputs) (DMM-1, DMM-2).
  - `m2k scope`: both channels, sample rate, range, statistics with frequency and duty, clipping and
    calibration warnings, `--csv` (SCOPE-1).
  - `m2k gen`: constant, sine, square, triangle, sawtooth on W1/W2 with exact frequencies, until Ctrl+C or
    `--for`, `gen off` (GEN-1).
  - `m2k logic`: the 16 digital pins with per-pin state, edges and frequency, `--csv` (LOGIC-1).
  - `m2k cal [status|run|lut|reset]`: libm2k's self-calibration, the factory table, reset (CAL-1, CAL-2, CAL-3).
- `--json` on every command, `--version` with git SHA and build time, colour, progress bars, dynamic shell
  completion with live device, channel and attribute names (`source <(COMPLETE=bash m2k)`) (CLI-1..CLI-4).
- Agent skill `skills/m2k/SKILL.md` (SKILL-1).
- Tests against ADI's iio-emu (`IIO_EMU`) and, opt-in, a real M2K (`M2K_TEST_URI`, `M2K_TEST_LOOPBACK`).
