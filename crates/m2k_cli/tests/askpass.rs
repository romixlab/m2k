//! The `m2k` binary as its own `SSH_ASKPASS` helper (`m2k led`, the Ready LED): no M2K, emulator or ssh
//! needed. `ssh` starts the helper with the prompt as its only argument and reads the answer from a pipe.

use std::process::{Command, Output};

const SECRET: &str = "s3cret-not-analog";

fn askpass(marker: Option<&str>, password: Option<&str>, prompt: &str) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_m2k"));
    cmd.arg(prompt)
        .env_remove("M2K_ASKPASS")
        .env_remove("M2K_SSH_PASSWORD")
        .env_remove("M2K_URI")
        .env("NO_COLOR", "1");
    if let Some(m) = marker {
        cmd.env("M2K_ASKPASS", m);
    }
    if let Some(p) = password {
        cmd.env("M2K_SSH_PASSWORD", p);
    }
    cmd.output().expect("run m2k")
}

#[test]
fn answers_a_password_prompt_with_the_password_from_the_environment() {
    let out = askpass(Some("1"), Some(SECRET), "root@192.168.2.1's password: ");
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{SECRET}\n"));
    assert!(out.stderr.is_empty());
}

#[test]
fn answers_nothing_else() {
    // a host key question must never be answered with the password
    let out = askpass(
        Some("1"),
        Some(SECRET),
        "Are you sure you want to continue connecting (yes/no/[fingerprint])? ",
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());

    // no password handed over
    let out = askpass(Some("1"), None, "root@192.168.2.1's password: ");
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
}

#[test]
fn without_the_marker_it_is_the_normal_cli() {
    // the password env var alone (a user who exported it) does not turn `m2k` into the helper
    let out = askpass(None, Some(SECRET), "root@192.168.2.1's password: ");
    assert!(!out.status.success());
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!all.contains(SECRET), "{all}");
    assert!(all.contains("Usage"), "{all}");
}

#[test]
fn help_and_errors_never_show_the_password() {
    for args in [
        vec!["led", "--help"],
        vec!["--uri", "ip:127.0.0.1:1", "led", "off"],
        vec!["--uri", "ip:127.0.0.1:1", "--json", "led", "status"],
        vec!["led", "--only", "nosuch"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_m2k"))
            .args(&args)
            .env_remove("M2K_ASKPASS")
            .env("M2K_SSH_PASSWORD", SECRET)
            .env("NO_COLOR", "1")
            .output()
            .expect("run m2k");
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!all.contains(SECRET), "{args:?}: {all}");
    }
}
