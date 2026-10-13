# Changelog

All notable changes to m2k_cli are recorded here, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). [FEATURES.md](FEATURES.md) holds the current status of
every feature; IDs in parentheses refer to it.

## [Unreleased]

## [0.7.0] - 2026-10-13

### Added

- `m2k led off|on|status`: the two blue LEDs of the M2K, `--only done|ready` for one of them; RAM only, both are
  back on after a power cycle. The steady DONE LED goes through iiod; the blinking Ready LED (the kernel's
  heartbeat LED) through the `ssh` on the PC as root with the factory password, or `--ssh-password` /
  `M2K_SSH_PASSWORD` together with `--ssh-known-hosts FILE` / `M2K_SSH_KNOWN_HOSTS` (pins the M2K's host key
  in a file of your choice). When ssh is missing or the login is refused, the DONE LED is still switched and
  the output says that the Ready LED was not changed and why (exit code 1 for `off`/`on`) (DEV-4).
- Library: `m2k::led` (`status`, `set`, `Ssh`, `askpass_main`) and `M2k::done_led`/`set_done_led` (DEV-4).
- Python: `M2k.led_status`/`led_off`/`led_on` and `LedStatus` (PY-2).
- New environment variables: `M2K_SSH_PASSWORD`, `M2K_SSH_KNOWN_HOSTS`; `M2K_ASKPASS` is internal (set by
  `m2k` for the ssh it starts) (DEV-4).

## [0.6.0] - 2026-10-10

### Changed

- The repo is github.com/romixlab/m2k (was m2k_cli): the plain name for the repo and the library, `m2k_cli` for
  the binary crate.

## [0.5.0] - 2026-10-10

### Added

- `m2k` library crate (`crates/m2k`): the typed API behind every CLI command (device, conversions,
  calibration, capture summaries, and `scope`/`gen`/`logic`/`psu`/`dmm` as typed calls returning
  structs/arrays), with its own `m2k::Error` and no `anyhow`/`serde_json`/`clap` dependency (CLI-5).
- `m2k_py` Python module (`crates/m2k_py`): PyO3 (abi3, Python >= 3.9), the `M2k` class and data classes on
  top of the `m2k` library, built by maturin; `just test-py` runs its offline pytest smoke test (PY-1).

### Changed

- `m2k_cli` is now the thin CLI on top of `crates/m2k` (library-first split, tpm CLAUDE.md "Code repos"):
  same commands, flags, JSON output and shell completion as before, no functional change (CLI-5).
  `cargo install --path .` becomes `cargo install --path crates/m2k_cli`.

## [0.4.1] - 2026-10-10

### Fixed

- `cal run` failed right after the M2K's power-up (`write iio:device0 oversampling_ratio = 0: EINVAL`): the saved
  oversampling ratio reads 0 then and is now put back as at least 1 (CAL-1).
- `scope -n` takes at most 4194304 samples per channel, the device's real limit, with a clear error above it;
  `-n 8M` (the old stated maximum) ended in `read buffer ... errno -27` (EFBIG) (SCOPE-1).

## [0.4.0] - 2026-10-09

### Added

- `justfile` with the recipes every work repo has: `just test`, `just lint` (fmt check, clippy), `just install`,
  `just deploy` (this PC up to main, then install).

### Fixed

- `logic --trigger-delay` below -8192 fails before touching the device with a message naming the limit (the M2K
  keeps at most 8192 samples from before the trigger, at every rate) instead of the driver's bare
  `invalid argument (EINVAL)` (LOGIC-3).

## [0.3.1] - 2026-10-09

### Fixed

- `--vcd` writes the timescale as one sample period (`1 us` at 1 MS/s, `10 ns` at 100 MS/s) and a last
  timestamp at the end of the capture: sigrok took the old `1 ps` as a 1 THz sample rate (LOGIC-5).
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
