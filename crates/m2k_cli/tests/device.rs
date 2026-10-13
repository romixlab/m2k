//! Against a real ADALM2000, only when `M2K_TEST_URI` is set (e.g. `ip:192.168.2.1`); skipped otherwise.
//!
//! Without more it only reads (info, monitors, scope, logic), switches the two LEDs and back (`leds`, needs
//! `ssh` on this PC) and drives DIO0 (`digital_self_loopback`,
//! which needs nothing wired: `digital out` and `logic` read the same physical pin). With
//! `M2K_TEST_LOOPBACK=1` it also drives the analog outputs and calibrates, and needs W1 wired to 1+, W2 to
//! 2+, and 1-, 2- to GND. It switches every output and supply off again.

mod common;
use common::{digital_bg, f, gen_bg, json, wait};
use std::time::Duration;

fn uri() -> Option<String> {
    let uri = std::env::var("M2K_TEST_URI").ok();
    if uri.is_none() {
        eprintln!("skipped: set M2K_TEST_URI=ip:192.168.2.1 to test a real M2K");
    }
    uri
}

fn loopback() -> bool {
    std::env::var("M2K_TEST_LOOPBACK").is_ok_and(|v| v == "1")
}

#[test]
fn reads() {
    let Some(uri) = uri() else { return };
    let v = json(&uri, &["info"]);
    assert_eq!(v["m2k"], true, "{v}");
    assert!(
        v["temperature"]
            .as_f64()
            .is_some_and(|t| (0.0..100.0).contains(&t)),
        "{v}"
    );
    let v = json(&uri, &["dmm", "vccint"]);
    assert!((f(&v["readings"][0]["value"]) - 1.0).abs() < 0.1, "{v}");
    let v = json(&uri, &["scope", "-n", "10k", "-r", "1M"]);
    assert_eq!(v["samples"], 10000);
    let v = json(&uri, &["logic", "-n", "10k"]);
    assert_eq!(v["pins"].as_array().unwrap().len(), 16);
}

#[test]
fn loopback_and_supplies() {
    let Some(uri) = uri() else { return };
    if !loopback() {
        eprintln!(
            "skipped: set M2K_TEST_LOOPBACK=1 with W1->1+, W2->2+, 1-/2- to GND to drive outputs"
        );
        return;
    }
    json(&uri, &["cal", "run"]);
    for volts in ["-1", "0.5"] {
        let g = gen_bg(&uri, &["-c", "1,2", "--for", "1500ms", "const", volts]);
        std::thread::sleep(Duration::from_millis(700));
        let v = json(&uri, &["voltmeter", "--range", "2.5"]);
        wait(g);
        let want: f64 = volts.parse().unwrap();
        for ch in ["ch1", "ch2"] {
            assert!((f(&v[ch]["dc"]) - want).abs() < 0.03, "{volts} V: {v}");
        }
    }
    let g = gen_bg(
        &uri,
        &["-c", "1", "--for", "2s", "sine", "-f", "1k", "-a", "1"],
    );
    std::thread::sleep(Duration::from_millis(700));
    let v = json(&uri, &["scope", "-n", "100k", "-r", "10M"]);
    wait(g);
    assert!((f(&v["ch1"]["frequency"]) - 1000.0).abs() < 2.0, "{v}");
    assert!((f(&v["ch1"]["vpp"]) - 2.0).abs() < 0.15, "{v}");
    let v = json(&uri, &["info"]);
    assert_eq!(
        v["outputs"],
        serde_json::json!({ "w1": false, "w2": false })
    );

    let v = json(&uri, &["psu", "set", "--pos", "1", "--neg", "-1"]);
    json(&uri, &["psu", "off"]);
    assert!((f(&v["pos"]["measured"]) - 1.0).abs() < 0.05, "{v}");
    assert!((f(&v["neg"]["measured"]) + 1.0).abs() < 0.05, "{v}");
}

/// Digital out and logic trigger (LOGIC-2, LOGIC-3), self-contained: `digital out` drives DIO0 and
/// `logic` reads the same physical pin back, no external wiring needed (DIO0 is otherwise free on this
/// bench). Independent of `M2K_TEST_LOOPBACK` (that one is about the analog W1/W2 wiring).
#[test]
fn digital_self_loopback() {
    let Some(uri) = uri() else { return };
    let dir = std::env::temp_dir().join(format!("m2k_device_digital_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("pattern.csv");
    // cyclic 0,1,0,1 at 10 kS/s: DIO0 toggles every sample, a 5 kHz square wave
    std::fs::write(&csv, "time_s,dio0\n0,0\n0.1,1\n0.2,0\n0.3,1\n").unwrap();

    let g = digital_bg(
        &uri,
        &[
            csv.to_str().unwrap(),
            "-r",
            "10k",
            "--cyclic",
            "--for",
            "1s",
        ],
    );
    std::thread::sleep(Duration::from_millis(200));
    let v = json(&uri, &["logic", "-n", "20k", "-r", "100k", "-p", "0"]);
    let pin = &v["pins"][0];
    assert_eq!(
        pin["direction"], "out",
        "DIO0 should read as an output while `digital out` drives it: {v}"
    );
    assert!(
        pin["edges"].as_u64().unwrap_or(0) > 100,
        "expected DIO0 to toggle: `digital out` and `logic` read the same physical pin: {v}"
    );
    assert!((f(&pin["frequency"]) - 5000.0).abs() < 100.0, "{v}");
    wait(g);

    let v = json(
        &uri,
        &["attr", "m2k-logic-analyzer", "-c", "voltage0", "direction"],
    );
    assert_eq!(
        v["value"], "in",
        "`digital out` should tri-state DIO0 again once it stops: {v}"
    );

    // with DIO0 still toggling, a rising-edge trigger should fire almost immediately
    let g = digital_bg(
        &uri,
        &[
            csv.to_str().unwrap(),
            "-r",
            "10k",
            "--cyclic",
            "--for",
            "2s",
        ],
    );
    std::thread::sleep(Duration::from_millis(200));
    let v = json(
        &uri,
        &[
            "logic",
            "-n",
            "1000",
            "-r",
            "100k",
            "--trigger",
            "0:rising",
            "--trigger-wait",
            "3s",
        ],
    );
    wait(g);
    assert_eq!(v["trigger"]["sample_index"], 0, "{v}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Both blue LEDs, switched to the other state and back: the DONE LED through iiod, the Ready LED through
/// the system `ssh` with the factory password (the M2K's ssh server must be reachable from this PC).
#[test]
fn leds() {
    let Some(uri) = uri() else { return };
    let before = json(&uri, &["led"]);
    let (done, ready) = (&before["done"]["on"], &before["ready"]["on"]);
    assert!(done.is_boolean() && ready.is_boolean(), "{before}");
    let name = |on: &serde_json::Value| if on == true { "on" } else { "off" };
    let other = |on: &serde_json::Value| if on == true { "off" } else { "on" };

    let v = json(&uri, &["led", other(done), "--only", "done"]);
    assert_eq!(v["done"]["on"] == true, done == false, "{v}");
    assert!(v["ready"].is_null(), "{v}");
    let v = json(&uri, &["led", other(ready), "--only", "ready"]);
    assert_eq!(v["ready"]["on"] == true, ready == false, "{v}");
    let trigger = if ready == false { "heartbeat" } else { "none" };
    assert_eq!(v["ready"]["trigger"], trigger, "{v}");

    json(&uri, &["led", name(done), "--only", "done"]);
    json(&uri, &["led", name(ready), "--only", "ready"]);
    let after = json(&uri, &["led"]);
    assert_eq!(after["done"]["on"], *done, "{after}");
    assert_eq!(after["ready"]["on"], *ready, "{after}");
    assert_eq!(after["partial"], false);
}
