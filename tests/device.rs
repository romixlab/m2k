//! Against a real ADALM2000, only when `M2K_TEST_URI` is set (e.g. `ip:192.168.2.1`); skipped otherwise.
//!
//! Without more it only reads (info, monitors, scope, logic). With `M2K_TEST_LOOPBACK=1` it also drives the
//! outputs and calibrates, and needs W1 wired to 1+, W2 to 2+, and 1-, 2- to GND. It switches every output
//! and supply off again.

mod common;
use common::{f, gen_bg, json, wait};
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
