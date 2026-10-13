//! End to end against ADI's iio-emu in its `adalm2000` profile (github.com/analogdevicesinc/iio-emu): the
//! binary from `IIO_EMU`, else `iio-emu` on PATH. Skipped (passes with a note) when there is none. Each test
//! starts its own emulator on a free port.
//!
//! What the emulator models: the context and attributes of a Rev.C M2K with firmware 0.26, the supplies
//! (set value = measured value), and the scope reading back what W1/W2 play. It does not model the calibration
//! modes, the trigger, or digital inputs (they read 0). Its scope treats the 75 MS/s output samples as 100 MS/s,
//! so frequencies read back 4/3 too high.

mod common;
use common::{f, gen_bg, json, m2k, wait};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Emu {
    child: Child,
    uri: String,
}

impl Drop for Emu {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn emu() -> Option<Emu> {
    let bin = std::env::var("IIO_EMU").unwrap_or_else(|_| "iio-emu".into());
    let port = TcpListener::bind("127.0.0.1:0")
        .ok()?
        .local_addr()
        .ok()?
        .port();
    let child = match Command::new(&bin)
        .args(["adalm2000", "-p", &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => {
            eprintln!("skipped: no iio-emu (set IIO_EMU or put iio-emu on PATH)");
            return None;
        }
    };
    let emu = Emu {
        child,
        uri: format!("ip:127.0.0.1:{port}"),
    };
    let start = Instant::now();
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "iio-emu did not start"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    Some(emu)
}

#[test]
fn info_attr_and_errors() {
    let Some(emu) = emu() else { return };
    let v = json(&emu.uri, &["info"]);
    assert_eq!(v["m2k"], true);
    assert_eq!(v["firmware"], "v0.26");
    assert_eq!(v["serial"], "ADALM2000 Demo");
    assert_eq!(v["devices"].as_array().unwrap().len(), 14);
    assert_eq!(v["outputs"]["w1"], false);

    let v = json(
        &emu.uri,
        &["attr", "m2k-adc", "sampling_frequency", "1000000"],
    );
    assert_eq!(v["value"], "1000000");
    let v = json(
        &emu.uri,
        &["attr", "m2k-adc", "-c", "voltage0", "calibbias"],
    );
    assert_eq!(v["value"], "1050");
    let v = json(&emu.uri, &["attr", "m2k-fabric", "-c", "voltage0", "-o"]);
    assert!(v["attrs"]["powerdown"].is_string(), "{v}");

    let out = m2k(&emu.uri, &["--json", "attr", "nosuch"]);
    assert!(!out.status.success());
    let err: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        err["error"]
            .as_str()
            .unwrap()
            .contains("no IIO device `nosuch`"),
        "{err}"
    );
}

#[test]
fn supplies_and_monitors() {
    let Some(emu) = emu() else { return };
    let v = json(&emu.uri, &["psu", "set", "--pos", "3.3", "--neg", "-2"]);
    assert_eq!(v["pos"]["enabled"], true);
    assert!((f(&v["pos"]["measured"]) - 3.3).abs() < 0.05, "{v}");
    assert!((f(&v["neg"]["measured"]) + 2.0).abs() < 0.05, "{v}");
    let v = json(&emu.uri, &["psu", "off"]);
    assert_eq!(v["pos"]["enabled"], false);
    assert_eq!(v["neg"]["enabled"], false);
    assert!(
        !m2k(&emu.uri, &["psu", "set", "--pos", "5.5"])
            .status
            .success()
    );

    let v = json(&emu.uri, &["dmm", "temp"]);
    let readings = v["readings"].as_array().unwrap();
    assert_eq!(readings.len(), 2, "{v}");
    assert!(
        readings
            .iter()
            .all(|r| r["unit"] == "°C" && (0.0..100.0).contains(&f(&r["value"])))
    );
}

#[test]
fn generator_into_scope() {
    let Some(emu) = emu() else { return };
    let g = gen_bg(
        &emu.uri,
        &[
            "-c", "1,2", "--for", "2s", "sine", "-f", "1k", "-a", "2", "-o", "0.5",
        ],
    );
    std::thread::sleep(Duration::from_millis(500));
    let v = json(&emu.uri, &["scope", "-n", "10k", "-r", "1M"]);
    wait(g);
    for ch in ["ch1", "ch2"] {
        let s = &v[ch];
        assert!((f(&s["vpp"]) - 4.0).abs() < 0.1, "{v}");
        assert!((f(&s["mean"]) - 0.5).abs() < 0.1, "{v}");
        // the emulator's resampling: 4/3 of the real frequency
        assert!((f(&s["frequency"]) - 1333.3).abs() < 20.0, "{v}");
    }
    let v = json(&emu.uri, &["info"]);
    assert_eq!(v["outputs"]["w1"], false, "generator left W1 on");

    let g = gen_bg(&emu.uri, &["-c", "2", "--for", "1500ms", "const", "-1.5"]);
    std::thread::sleep(Duration::from_millis(500));
    let v = json(&emu.uri, &["voltmeter"]);
    wait(g);
    assert!((f(&v["ch2"]["dc"]) + 1.5).abs() < 0.05, "{v}");

    // CSV of every sample
    let dir = std::env::temp_dir().join(format!("m2k_emu_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("scope.csv");
    let v = json(
        &emu.uri,
        &["scope", "-n", "1000", "--csv", csv.to_str().unwrap()],
    );
    assert_eq!(v["samples"], 1000);
    let text = std::fs::read_to_string(&csv).unwrap();
    assert_eq!(text.lines().count(), 1001);
    assert!(text.starts_with("time_s,ch1_v,ch2_v\n"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn digital_out_and_logic_trigger() {
    let Some(emu) = emu() else { return };
    let dir = std::env::temp_dir().join(format!("m2k_emu_digital_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // direction/outputmode attributes round-trip, and the pins go back to `in` after a one-shot buffer
    let csv = dir.join("pattern.csv");
    std::fs::write(
        &csv,
        "time_s,dio0,dio1\n0.0,0,0\n0.1,1,0\n0.2,1,1\n0.3,0,1\n",
    )
    .unwrap();
    let v = json(
        &emu.uri,
        &[
            "digital",
            "out",
            csv.to_str().unwrap(),
            "-r",
            "1k",
            "--open-drain",
            "0",
        ],
    );
    assert_eq!(v["pins"], serde_json::json!([0, 1]));
    assert_eq!(v["open_drain"], serde_json::json!([0]));
    assert_eq!(v["samples"], 4);
    let v = json(&emu.uri, &["attr", "m2k-logic-analyzer", "-c", "voltage0"]);
    assert_eq!(v["attrs"]["direction"], "in", "{v}");
    assert_eq!(v["attrs"]["outputmode"], "open-drain", "{v}");
    let v = json(&emu.uri, &["attr", "m2k-logic-analyzer", "-c", "voltage1"]);
    assert_eq!(v["attrs"]["direction"], "in", "{v}");
    assert_eq!(v["attrs"]["outputmode"], "push-pull", "{v}");

    // a bad CSV and a pin outside the CSV's own are rejected before anything is touched
    let bad = dir.join("bad.csv");
    std::fs::write(&bad, "a,b\n1,2\n").unwrap();
    assert!(
        !m2k(
            &emu.uri,
            &["digital", "out", bad.to_str().unwrap(), "-r", "1k"]
        )
        .status
        .success()
    );
    assert!(
        !m2k(
            &emu.uri,
            &[
                "digital",
                "out",
                csv.to_str().unwrap(),
                "-r",
                "1k",
                "--open-drain",
                "5"
            ],
        )
        .status
        .success()
    );

    // a trigger on the emulator (which models no digital inputs or triggers) still round-trips the
    // attributes and returns right away; the shape has a stable `trigger` object either way
    let v = json(
        &emu.uri,
        &[
            "logic",
            "-n",
            "1k",
            "-r",
            "1M",
            "--trigger",
            "3:rising",
            "--trigger-delay",
            "-10",
        ],
    );
    assert_eq!(
        v["trigger"],
        serde_json::json!({
            "pins": [{"pin": 3, "condition": "edge-rising"}],
            "logic": "or",
            "delay": -10,
            "sample_index": 10,
        })
    );
    let v = json(&emu.uri, &["logic", "-n", "1k", "-p", "0"]);
    assert_eq!(
        v["trigger"],
        serde_json::json!({ "pins": [], "logic": "or", "delay": 0, "sample_index": null })
    );

    // CSV/VCD export
    let csv_out = dir.join("cap.csv");
    let vcd_out = dir.join("cap.vcd");
    let v = json(
        &emu.uri,
        &[
            "logic",
            "-n",
            "1000",
            "-p",
            "0-3",
            "--csv",
            csv_out.to_str().unwrap(),
            "--vcd",
            vcd_out.to_str().unwrap(),
        ],
    );
    assert_eq!(v["samples"], 1000);
    assert_eq!(
        std::fs::read_to_string(&csv_out).unwrap().lines().count(),
        1001
    );
    let vcd = std::fs::read_to_string(&vcd_out).unwrap();
    assert!(vcd.starts_with("$timescale 1 ps $end\n"));
    assert!(vcd.contains("$var wire 1 ! dio0 $end"));
    assert!(vcd.contains("$dumpvars\n"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn logic_and_calibration() {
    let Some(emu) = emu() else { return };
    let v = json(&emu.uri, &["logic", "-n", "5k", "-p", "0-3"]);
    let pins = v["pins"].as_array().unwrap();
    assert_eq!(pins.len(), 4);
    assert!(pins.iter().all(|p| p["state"] == "low"), "{v}");

    let v = json(&emu.uri, &["cal"]);
    assert_eq!(v["adc_offset"], serde_json::json!([1050, 1060]));
    // the emulator has no calibration modes: the measurement fails and nothing may change
    let out = m2k(&emu.uri, &["--json", "cal", "run"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("implausible"));
    assert_eq!(json(&emu.uri, &["cal"]), v);
    let v = json(&emu.uri, &["cal", "reset"]);
    assert_eq!(v["calibrated"], false);
}

/// A stand-in for `ssh` on PATH: asks its `SSH_ASKPASS` program (the `m2k` binary) for the password like ssh
/// does, lets `analog` in and answers like the M2K's shell; the LED state lives in a file next to it.
#[cfg(unix)]
fn fake_ssh_dir() -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("m2k_cli_test_ssh_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ssh = dir.join("ssh");
    std::fs::write(
        &ssh,
        r#"#!/bin/sh
dir=$(dirname "$0")
[ "$("$SSH_ASKPASS" "root@127.0.0.1's password: ")" = analog ] \
    || { echo "root@127.0.0.1: Permission denied (publickey,password)." >&2; exit 255; }
for last; do :; done
case "$last" in
"echo none"*) echo off > "$dir/state" ;;
"echo heartbeat"*) echo on > "$dir/state" ;;
esac
if [ "$(cat "$dir/state" 2>/dev/null)" = off ]; then
    printf '[none] timer heartbeat\n0\n'
else
    printf 'none timer [heartbeat]\n255\n'
fi
"#,
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

#[cfg(unix)]
#[test]
fn leds() {
    let Some(emu) = emu() else { return };
    let dir = fake_ssh_dir();
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let run = |args: &[&str], password: Option<&str>| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_m2k"));
        cmd.args(["--uri", &emu.uri, "--json"])
            .args(args)
            .env("PATH", &path)
            .env_remove("M2K_SSH_PASSWORD")
            .env_remove("M2K_SSH_KNOWN_HOSTS");
        if let Some(p) = password {
            cmd.env("M2K_SSH_PASSWORD", p);
        }
        let out = cmd.output().expect("run m2k");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let v: serde_json::Value = serde_json::from_str(stdout.lines().next().unwrap_or_default())
            .unwrap_or_else(|e| panic!("m2k {args:?}: not JSON ({e}): {stdout}"));
        (out.status.code(), v)
    };

    let (code, v) = run(&["led", "off"], None);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["done"]["on"], false, "{v}");
    assert_eq!(v["ready"]["on"], false, "{v}");
    assert_eq!(v["ready"]["trigger"], "none");
    assert_eq!(v["partial"], false);
    let v = json(
        &emu.uri,
        &[
            "attr",
            "m2k-fabric",
            "-c",
            "voltage4",
            "-o",
            "done_led_overwrite_powerdown",
        ],
    );
    assert_eq!(v["value"], "1");

    let (code, v) = run(&["led", "on", "--only", "done"], None);
    assert_eq!(code, Some(0));
    assert_eq!(v["done"]["on"], true, "{v}");
    assert!(v["ready"].is_null(), "{v}");
    let (_, v) = run(&["led"], None);
    assert_eq!(v["done"]["on"], true, "{v}");
    assert_eq!(v["ready"]["on"], false, "{v}");
    let (_, v) = run(&["led", "on", "--only", "ready"], None);
    assert!(v["done"].is_null(), "{v}");
    assert_eq!(v["ready"]["trigger"], "heartbeat", "{v}");

    // a refused login: the DONE LED still switches, the Ready LED is named as not changed, exit code 1;
    // the password is nowhere in the output
    let kh = dir.join("known_hosts");
    let (code, v) = run(
        &["led", "off", "--ssh-known-hosts", kh.to_str().unwrap()],
        Some("s3cret-not-analog"),
    );
    assert_eq!(code, Some(1), "{v}");
    assert_eq!(v["done"]["on"], false, "{v}");
    assert_eq!(v["ready"]["changed"], false, "{v}");
    assert_eq!(v["partial"], true);
    assert!(
        v["ready"]["error"]
            .as_str()
            .unwrap()
            .contains("wrong password"),
        "{v}"
    );
    assert!(!v.to_string().contains("s3cret"), "{v}");
    // status with the same trouble is not a failure: the Ready LED is just unknown
    let (code, v) = run(
        &["led", "--ssh-known-hosts", kh.to_str().unwrap()],
        Some("s3cret-not-analog"),
    );
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["ready"]["on"], serde_json::Value::Null, "{v}");
    // another password without a pinned host key is not sent at all
    let (code, v) = run(&["led", "off"], Some("s3cret-not-analog"));
    assert_eq!(code, Some(1));
    assert!(
        v["ready"]["error"]
            .as_str()
            .unwrap()
            .contains("known hosts file"),
        "{v}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
