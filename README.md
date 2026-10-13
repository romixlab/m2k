# m2k

`m2k`: a Rust library, CLI and agent skill, and a Python module, for the Analog Devices ADALM2000 (M2K):
scope, signal generator, power supplies, voltmeter, logic analyzer and calibration, over the network with a
pure-Rust iiod client (no libiio, no libm2k, no C).

```sh
cargo install --path crates/m2k_cli         # (crates.io: not published yet)
m2k info                                    # model, serial, firmware, calibration, outputs, supplies
m2k cal run                                 # self-calibration, ~1.5 s; kept until power-off
m2k scope -n 10k -r 1M --csv cap.csv        # both channels: stats per channel, all samples as CSV
m2k gen -c 1 --for 10s sine -f 1k -a 1      # W1: 1 kHz, ±1 V; off again after 10 s (or Ctrl+C)
m2k voltmeter --range 2.5                   # DC and AC rms on 1+/1- and 2+/2-
m2k psu set --pos 3.3 --neg -3.3; m2k psu off
m2k logic -n 100k -r 10M -p 0-7
m2k led off                                 # both blue LEDs off until the next power cycle (`led on`)
m2k --json scope                            # every command: machine-readable output
source <(COMPLETE=bash m2k)                 # shell completion (zsh, fish too)
```

The M2K shows up as a USB network adapter; `m2k` talks to iiod at `192.168.2.1:30431` (`--uri ip:HOST[:PORT]` or
`M2K_URI` for another address, e.g. ADI's `iio-emu adalm2000` emulator).

- Agent skill: [skills/m2k/SKILL.md](skills/m2k/SKILL.md)
- Features, status and what was verified on a real M2K: [FEATURES.md](FEATURES.md); changes:
  [CHANGELOG.md](CHANGELOG.md); dev rules: [AGENTS.md](AGENTS.md)
- [`crates/m2k`](crates/m2k): the typed Rust library on its own (no CLI, no JSON); `crates/m2k_cli` above is
  the thin CLI built on it.
- [`crates/m2k_py`](crates/m2k_py): a Python module (PyO3 + maturin) for notebooks and test benches,
  `pip install` from a checkout with `maturin develop` or `uv run` inside it.
- [`crates/iiod_client`](crates/iiod_client): the iiod client on its own, for any IIO device (ADALM-Pluto, ...).

The conversions and the calibration procedure follow ADI's [libm2k](https://github.com/analogdevicesinc/libm2k);
the protocol follows libiio's iiod. m2k is licensed under MIT or Apache-2.0, at your option
([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)).
