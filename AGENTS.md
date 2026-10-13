# Working on m2k

Guidance for AI agents and contributors. Read this before changing code.

The m2k repo is the Cargo workspace for the Analog Devices ADALM2000 (M2K): scope, signal generator, power
supplies, voltmeter, logic analyzer and calibration, library first (tpm CLAUDE.md "Code repos": tools are a
library first) — the `m2k` Rust library (`crates/m2k`), its thin CLI and agent skill on top
(`crates/m2k_cli`, binary `m2k`), and a Python module for notebooks and test benches (`crates/m2k_py`). It
talks to the iiod daemon on the M2K over TCP (port 30431; the M2K is a USB network adapter at 192.168.2.1)
with its own pure-Rust iiod client (`crates/iiod_client`): no libiio, no libm2k, no C. The M2K specifics
(which IIO device and attribute does what, raw-to-volts formulas, calibration) are ported from ADI's libm2k:
https://github.com/analogdevicesinc/libm2k (`src/analog/*_impl.cpp`, `src/m2kcalibration_impl.cpp`,
`src/m2k_impl.cpp`). The protocol: libiio's `iiod` and `libtinyiiod`
(https://github.com/analogdevicesinc/libtinyiiod, `parser.c`, `tinyiiod.c`).

## FEATURES.md is the source of truth

[FEATURES.md](FEATURES.md) lists every feature with its status, every known bug, and what is planned, with stable
IDs per area (`IIO`, `DEV`, `PSU`, `DMM`, `SCOPE`, `GEN`, `LOGIC`, `CAL`, `CLI`, `PY`, `SKILL`, `DAEMON`).

- **Read the relevant area before starting.** A "new" bug or idea is often already recorded.
- **Name IDs with a short slug when talking to the user**: `SCOPE-2 trigger`, never a bare `SCOPE-2`. Commit
  messages, CHANGELOG and code `TODO`s keep the bare ID.
- **Update it in the same commit** as the code. New ids: `tpm work claim AREA` prints the next free one. Never
  renumber or reuse IDs.
- Status ✅ means verified on a real M2K; 🧪 means emulator or unit tests only. Keep that honest, and say in the
  item what was checked how.

## CHANGELOG.md records every change

[CHANGELOG.md](CHANGELOG.md) is the history, FEATURES.md the current state; keep both.

- Every change a user would notice gets an entry under `## [Unreleased]` in the same commit: `### Added`,
  `### Changed`, `### Fixed`, `### Removed`. Short and user-facing, with the feature ID in parentheses.
- Also record new env vars. Pure refactors and typo fixes don't need an entry.

## Layout

Cargo workspace, edition 2024, one version for all crates (`[workspace.package]`), the same layout as the
other instrument tools (tpm CLAUDE.md "Code repos": tools are a library first, gw121_cli/saleae_rs pattern):

- `crates/iiod_client` — the iiod client, with nothing M2K-specific: it is meant to be shared with the
  ADALM-Pluto skill (IIO-6). `client.rs` the protocol over one TCP connection, `context.rs` the XML context,
  sample layout and formats. Std only plus `roxmltree`; keep it that way.
- `crates/m2k` — the library, the typed API; everything that talks to the device or knows the M2K's formulas
  lives here. Errors are `m2k::Error` (thiserror), never `anyhow`.
  - `device.rs` — the M2K on top of `iiod_client::Client`: device names, power-up (`init`), scope buffer
    open/capture, signal outputs, supplies, monitors, digital pins. Every attribute write the M2K needs lives
    here or in `cal.rs`.
  - `conv.rs` — pure conversions with libm2k's constants (ADC/DAC volts, supplies, filter compensation, wave
    and sample-rate planning, factory table parsing). Unit tests check them against libm2k's formulas.
  - `cal.rs` — self-calibration as libm2k's `M2kCalibrationImpl`, factory table, reset, and `run` (the typed
    call behind `m2k cal run`: calibrate the scope, then the outputs unless `adc_only`).
  - `stats.rs` — capture summaries (`Analog`, `Digital`): what `scope`, `voltmeter` and `logic` summarize.
  - `led.rs` — the two blue LEDs (DEV-4): DONE through iiod, Ready through the system `ssh` (`Ssh`, the
    argument building, `askpass_main` for the binary that serves as `SSH_ASKPASS`). The only place that starts
    another program and handles a password: the password travels in the ssh child's environment only, never
    in argv, a file, an error or `Debug` output; keep the unit tests that check this.
  - `scope.rs`, `gen.rs`, `logic.rs`, `psu.rs`, `dmm.rs` — the typed calls behind the CLI commands of the same
    name (CLI-5): `scope::capture`/`scope::voltmeter`, `gen::build_wave`, `logic::capture`,
    `psu::status`/`set`/`off`, `dmm::readings`. Captures come back as structs/arrays, not text.
  - `error.rs` — `Error` (thiserror; wraps `iiod_client::Error` — the server's errno reaches callers with its
    name — or a validation message) with a `Context`/`bail!`/`err!` trio giving `anyhow`-like ergonomics,
    used only inside this crate.
- `crates/m2k_cli` — the thin `m2k` binary: `src/cli.rs` the command tree (clap derive) and handlers, calling
  into the `m2k` crate and formatting the result (JSON glue for its plain structs — `m2k` has no serde_json
  dependency — lives here, e.g. `analog_json`/`digital_json`/`corrections_json`); `--json` output through
  `Out::print`, warnings through `Out::warn`, progress through `Out::progress`/`Out::spinner`. `src/parse.rs`
  SI numbers (`1k`, `2.5M`), durations, pin lists, and printing numbers back — CLI-only: the `m2k` crate takes
  plain numbers, never strings. `src/complete.rs` dynamic shell completion, its Bash adapter copied from
  wire_weaver_cli / saleae_cli / gw121_cli (keep in sync). `build.rs` `GIT_SHA` and `BUILD_TIME` for
  `--version`. `anyhow` only here. `tests/emu.rs` end to end against iio-emu; `tests/device.rs` against a
  real M2K when asked to; `tests/askpass.rs` the binary as its own `SSH_ASKPASS` helper (DEV-4), offline.
- `crates/m2k_py` — the `m2k` Python module (PY-1; PyO3, abi3 for Python >= 3.9, built by maturin from
  `pyproject.toml`; `publish = false` on crates.io, it ships as a wheel): the `M2k` class
  (`scope`/`voltmeter`/`logic`/`gen_start`/`gen_stop`/`psu_status`/`psu_set`/`psu_off`/`dmm`/`cal_run`/
  `cal_status`/`cal_reset`/`temperature`/`led_status`/`led_off`/`led_on`), data classes for the typed calls'
  results (`ScopeCapture`/`Analog`/`LogicCapture`/`Digital`/`PsuStatus`/`DmmReading`/`Corrections`/
  `LedStatus`), exceptions as `M2kError` mapped from `m2k::Error`. `gen_start` takes a plain sample array (build it with numpy) rather
  than the CLI's shape flags — there is no Python equivalent of `gen::build_wave` yet. `m2k.pyi` (type stubs
  and docstrings, shipped by maturin) must match `src/lib.rs`; `tests/test_smoke.py` is the offline pytest
  smoke test (no M2K or emulator: exercises the module boundary and error mapping only; `tests/device.rs`-style
  real-device Python tests are not written yet).
- `skills/m2k/SKILL.md` — the agent skill. Update it when commands or their output change.

## Commands

```sh
cargo build --workspace
cargo run -p m2k_cli -- info                   # the M2K at ip:192.168.2.1 (M2K_URI / --uri to change)
cargo run -p m2k_cli -- scope -n 10k -r 1M
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo test --workspace                         # unit tests; tests/emu.rs needs iio-emu (else it skips)
IIO_EMU=/path/to/iio-emu cargo test -p m2k_cli --test emu
M2K_TEST_URI=ip:192.168.2.1 cargo test -p m2k_cli --test device                      # real M2K, reads only
M2K_TEST_URI=ip:192.168.2.1 M2K_TEST_LOOPBACK=1 cargo test -p m2k_cli --test device  # + outputs, needs the loopback
cargo install --path crates/m2k_cli            # puts `m2k` in ~/.cargo/bin
just test-py                                   # crates/m2k_py's pytest smoke test, via uv; no device needed
```

### Building iio-emu

ADI's emulator (https://github.com/analogdevicesinc/iio-emu, `adalm2000` profile) needs cmake, libxml2 and
libtinyiiod. Build it in `/tmp` (C++, never part of this repo): clone `libtinyiiod` and `iio-emu`, then
`cmake -S libtinyiiod -B libtinyiiod/build -DBUILD_EXAMPLES=OFF -DCMAKE_INSTALL_PREFIX=$PWD/prefix
-DCMAKE_POLICY_VERSION_MINIMUM=3.5 && cmake --build libtinyiiod/build && cmake --install libtinyiiod/build`, then
`cmake -S iio-emu -B iio-emu/build -DCMAKE_PREFIX_PATH=$PWD/prefix -DCMAKE_POLICY_VERSION_MINIMUM=3.5 && cmake
--build iio-emu/build`. Run `iio-emu adalm2000 -p 30555` (`-p` after the profile) and use `--uri
ip:127.0.0.1:30555`. No cmake installed: `uv pip install cmake` into a venv.

What the emulator does not do: calibration modes (`cal run` fails there), triggers, digital input signals. Its
scope reads back what W1/W2 play but treats the 75 MS/s output samples as 100 MS/s, so frequencies come back
4/3 too high. Calibration, real signal levels and anything timing-related need a real M2K.

## Hardware safety

- Reading (info, attr reads, dmm, voltmeter, scope, logic) is harmless anytime; so is `m2k led` (the two
  LEDs on the board, nothing on the connectors). It logs in to the M2K's shell as root over ssh with fixed
  commands: never extend that to anything that writes outside `/sys/class/leds` without asking.
- Outputs and supplies can damage whatever is wired to the M2K. Ask the user what is connected before driving
  them, keep levels to what they allow, and switch everything off after a test (`m2k gen -c 1,2 off`,
  `m2k psu off`, or the Python module's `gen_stop`/`psu_off`). `m2k cal run` (and the Python `cal_run`) drives
  W1/W2 to about 2.5 V for a moment.
- Never write firmware, the M2K's config (`config.txt` on its mass storage) or udev rules without asking.

## Dependencies

Pure Rust only (tpm "Code repos" rule): no libiio, libm2k, libxml2 or any `-sys` crate. Split by crate:
`iiod_client` only `roxmltree`; `m2k` only `iiod_client` and `thiserror`; `m2k_cli` adds `clap`,
`clap_complete`, `indicatif`, `ctrlc`, `serde_json`, `anyhow`; `m2k_py` adds `pyo3` (abi3: needs a Python at
build time for `python3-config`, not at runtime). Check new dependencies with `cargo tree` for `-sys` crates
and `cc`/`bindgen` build deps.

## Publishing

Crates `m2k` (library), `m2k_cli` (binary `m2k`) and `iiod_client` on crates.io (not published yet; publish
`iiod_client` first, then `m2k`, then `m2k_cli`), repo `github.com/romixlab/m2k_cli`, MIT or Apache-2.0.
`m2k_py` ships as a Python wheel built by maturin (`publish = false` in its `Cargo.toml`), not to crates.io.
`tpm land` bumps the version for all of them, the user runs `cargo publish` per crate (in that order) and
`maturin publish` (or `uv build && uv publish`) for the wheel. Public repo: nothing private in commits, docs
or the skill (no device serials, internal hosts, paths, ids). libm2k is LGPL-2.1: port formulas and constants
(facts), never copy its code.

## Code conventions

- Errors: `m2k::Error` (thiserror; wraps `iiod_client::Error` or holds a validation message) in the `m2k`
  crate, `anyhow` with `.context(...)` in `m2k_cli`, `M2kError` (mapped from `m2k::Error`) in `m2k_py`; the
  server's errno reaches the user with its name (`EINVAL`) through `iiod_client::Error`'s `Display`. No
  `unwrap`/`expect` on data from the device or the user.
- Every CLI command supports `--json`; keep text output short and line-oriented (agents read it), units
  spelled out (`1 MS/s`, `+0.500 V`). The `m2k` crate itself has no JSON or text formatting — that is CLI (and
  Python docstring) concern.
- Every command that changes an output leaves it in a known state on error and on Ctrl+C.

## Tests

- Pure logic (conversions, stats, parsing, protocol framing against a scripted server): unit tests next to the
  code, in whichever crate the code lives in (`m2k` for conversions/stats/calibration, `m2k_cli` for CLI
  parsing/formatting).
- Device behaviour: `crates/m2k_cli/tests/emu.rs` (own emulator per test on a free port) and
  `crates/m2k_cli/tests/device.rs` (opt-in); both run the `m2k` binary end to end. A conversion gets a test
  with the value libm2k's formula gives. A bug fix starts with a failing test when the bug can be reproduced
  in one.
- `crates/m2k_py/tests/test_smoke.py`: offline (no M2K or emulator), checks the module boundary and error
  mapping; run with `just test-py`.

## Commits

Conventional Commits with a scope: `feat(scope): ...`, `fix(iiod): ...`, scopes `iiod`, `info`, `psu`, `dmm`,
`scope`, `gen`, `logic`, `cal`, `cli`, `py`, `skill`, `build`. Short imperative summary, blank line, body with
what and why; reference feature IDs.

Commit on your own initiative on a session branch (`tpm work new SLUG`), each finished step one commit with
FEATURES.md and CHANGELOG.md updated in it. When the work is done and the user agrees, `tpm land` puts it on main.

## Versions

Landing bumps the version, not each work commit (tpm CLAUDE.md "Landing bumps the version"): session commits add
CHANGELOG entries under `[Unreleased]` without bumping; `tpm land` makes the `release: x.y.z` commit.
`m2k --version` prints version, git SHA and build time (`build.rs`, no extra crates).
