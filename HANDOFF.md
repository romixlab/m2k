# Handoff: m2k led (DEV-4, PY-2)

Done: `crates/m2k/src/led.rs` (typed API, ssh argument building, askpass), `M2k::done_led`/`set_done_led`,
CLI `m2k led off|on|status` (`--only`, `--ssh-password`, `--ssh-known-hosts`), Python `led_status`/`led_off`/
`led_on`, unit tests, `tests/askpass.rs`, `tests/emu.rs::leds` (stand-in ssh on PATH).

Next: SKILL.md, FEATURES.md, CHANGELOG.md; check on the real M2K (ssh omarchy-m1, M2K at 192.168.2.1 there;
no scope/logic captures, another session sweeps on that bench); leave both LEDs off; `just test`, `just lint`.

Decisions: system `ssh` with the m2k binary as SSH_ASKPASS helper (password in the ssh child's environment
only); no host key check with the factory password, another password only with a pinned known hosts file of
the user's choice; `-F none` so the user's ssh config plays no part; partial result = exit code 1 for off/on.

Gotchas: `led` does not call `M2k::init` (nothing to power up); iio-emu for the emu test: /tmp/m2k-led-emu.
