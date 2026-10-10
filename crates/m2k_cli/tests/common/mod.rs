//! Running the `m2k` binary in tests.

use serde_json::Value;
use std::process::{Command, Output};

pub fn m2k(uri: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_m2k"))
        .args(["--uri", uri])
        .args(args)
        .env_remove("M2K_URI")
        .env("NO_COLOR", "1")
        .output()
        .expect("run m2k")
}

/// Runs with `--json`, expects success, returns the value.
pub fn json(uri: &str, args: &[&str]) -> Value {
    let mut all = vec!["--json"];
    all.extend(args);
    let out = m2k(uri, &all);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "m2k {args:?} failed: {stdout} {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(stdout.lines().next().unwrap_or_default())
        .unwrap_or_else(|e| panic!("m2k {args:?}: not JSON ({e}): {stdout}"))
}

pub fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"))
}

/// Starts `m2k gen ARGS` in the background (it runs until killed or `--for` ends).
pub fn gen_bg(uri: &str, args: &[&str]) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_m2k"))
        .args(["--uri", uri, "gen"])
        .args(args)
        .env("NO_COLOR", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start m2k gen")
}

/// Waits for a `--for` generator or digital output to finish and checks it ended cleanly.
pub fn wait(mut child: std::process::Child) {
    let status = child.wait().expect("wait for m2k gen/digital out");
    assert!(status.success(), "m2k gen/digital out failed: {status}");
}

/// Starts `m2k digital out ARGS` in the background (it runs until killed, the buffer ends, or `--for`).
/// Only `tests/device.rs` uses this today; `common` is compiled once per test binary.
#[allow(dead_code)]
pub fn digital_bg(uri: &str, args: &[&str]) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_m2k"))
        .args(["--uri", uri, "digital", "out"])
        .args(args)
        .env("NO_COLOR", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start m2k digital out")
}
