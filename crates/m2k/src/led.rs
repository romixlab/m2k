//! The two blue LEDs of the ADALM2000, as typed data: switch them off on a dark bench, and back on.
//!
//! - **DONE** (steady): the FPGA's DONE LED, overridden through the IIO attribute
//!   `done_led_overwrite_powerdown` of `m2k-fabric` (what libm2k's `setLed` writes). Needs only the iiod
//!   connection.
//! - **Ready** (blinking): the kernel LED `led0:green` with the `heartbeat` trigger. iiod cannot reach it;
//!   only a shell on the M2K can, so this one goes through the system `ssh` ([`Ssh`]): `root@HOST` with the
//!   password ADI documents (`analog`) unless another is given.
//!
//! Both settings live in RAM: after a power cycle the LEDs are back on.
//!
//! # How the password reaches ssh
//!
//! `ssh` asks for a password through the program in `SSH_ASKPASS`. That program is a binary that calls
//! [`askpass_main`] first thing in its `main` (the `m2k` CLI does); the password is handed over in the
//! environment of the `ssh` child only ([`ENV_PASSWORD`], next to the [`ENV_ASKPASS`] marker): no temporary
//! file, never on a command line, never logged or returned in an error.
//!
//! # Host key
//!
//! The M2K is a point-to-point USB network device, every unit answers at the same 192.168.2.1 and the factory
//! password is public, so by default the host key is **not checked** and nothing is written to any
//! `known_hosts` file: with the factory password there is no secret a fake host could learn, and the commands
//! sent are fixed strings. A password other than the factory one is a real secret, so it is only sent with
//! [`Ssh::known_hosts`] set: that file (never the user's `~/.ssh/known_hosts`) pins the key on first use and
//! ssh refuses the login when it changes.

use crate::device::{M2k, parse_uri};
use crate::error::{Error, Result, bail, err};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The password ADI documents for `root` on the M2K.
pub const FACTORY_PASSWORD: &str = "analog";
/// Carries the password to the askpass helper (and is where the CLI and Python module read it from).
pub const ENV_PASSWORD: &str = "M2K_SSH_PASSWORD";
/// Set to `1` in the `ssh` child's environment: tells the helper it runs as `SSH_ASKPASS`.
pub const ENV_ASKPASS: &str = "M2K_ASKPASS";
const USER: &str = "root";
const SYSFS: &str = "/sys/class/leds/led0:green";

/// Which LED.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Led {
    /// The steady DONE LED (IIO).
    Done,
    /// The blinking "Ready" LED (ssh).
    Ready,
}

/// The kernel LED behind "Ready".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ready {
    /// Lit or blinking: a trigger other than `none`, or a brightness above 0.
    pub on: bool,
    /// The active kernel trigger: `heartbeat` after power-up, `none` when switched off.
    pub trigger: String,
    pub brightness: u32,
}

/// What both LEDs are after a call. A field is `None` for an LED that was not asked for.
#[derive(Debug)]
pub struct Status {
    /// The DONE LED is on.
    pub done: Option<bool>,
    /// The Ready LED, or why ssh could not reach it (it was not changed then).
    pub ready: Option<Result<Ready>>,
}

impl Status {
    /// An LED that was asked for could not be reached.
    pub fn partial(&self) -> bool {
        matches!(self.ready, Some(Err(_)))
    }
}

/// A password that does not show up in `Debug` output.
#[derive(Clone, PartialEq, Eq)]
pub struct Password(String);

impl Password {
    pub fn new(password: impl Into<String>) -> Password {
        Password(password.into())
    }

    /// [`ENV_PASSWORD`] when set and not empty, else the factory password.
    pub fn from_env() -> Password {
        match std::env::var(ENV_PASSWORD) {
            Ok(p) if !p.is_empty() => Password(p),
            _ => Password::default(),
        }
    }

    pub fn is_factory(&self) -> bool {
        self.0 == FACTORY_PASSWORD
    }
}

impl Default for Password {
    fn default() -> Password {
        Password(FACTORY_PASSWORD.into())
    }
}

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Password(***)")
    }
}

impl std::str::FromStr for Password {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> std::result::Result<Password, Self::Err> {
        Ok(Password(s.to_string()))
    }
}

/// How to reach the shell on the M2K.
#[derive(Debug, Clone)]
pub struct Ssh {
    pub password: Password,
    /// The binary `ssh` runs as `SSH_ASKPASS`: one that calls [`askpass_main`] (the `m2k` CLI). `None`: the
    /// Ready LED is reported as unreachable.
    pub askpass: Option<PathBuf>,
    /// Pin the host key in this file (created on first use) and refuse a changed one. `None`: no host key
    /// check, allowed with the factory password only (see the module docs).
    pub known_hosts: Option<PathBuf>,
    /// Connect timeout; the whole call gets a few seconds more.
    pub timeout: Duration,
    /// The ssh client, looked up on `PATH`: OpenSSH 8.4 or later (`SSH_ASKPASS_REQUIRE`).
    pub program: OsString,
}

impl Ssh {
    /// The factory password, the `m2k` CLI found on `PATH` as askpass helper, no host key pinning.
    pub fn new(timeout: Duration) -> Ssh {
        Ssh {
            password: Password::default(),
            askpass: find_on_path("m2k"),
            known_hosts: None,
            timeout,
            program: "ssh".into(),
        }
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let name = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(&name))
        .find(|p| p.is_file())
}

// ---- the askpass helper ----

/// What the askpass helper answers: the password, only when `ssh` marked the call ([`ENV_ASKPASS`] is `1`) and
/// asks for a password (not for a host key confirmation or anything else).
fn askpass_reply<'a>(
    marker: Option<&str>,
    password: Option<&'a str>,
    prompt: Option<&str>,
) -> Option<&'a str> {
    if marker != Some("1") {
        return None;
    }
    let prompt = prompt?.to_ascii_lowercase();
    if !prompt.contains("password") {
        return None;
    }
    password.filter(|p| !p.is_empty())
}

/// Call first thing in `main` of a binary that serves as [`Ssh::askpass`]. Does nothing in a normal run. When
/// `ssh` started the binary as its `SSH_ASKPASS` program, prints the password it was handed in the environment
/// to `ssh`'s pipe and exits (with 1 and no output for any prompt that is not a password prompt, or when stdout
/// is a terminal).
pub fn askpass_main() {
    if std::env::var(ENV_ASKPASS).as_deref() != Ok("1") {
        return;
    }
    let password = std::env::var(ENV_PASSWORD).ok();
    let prompt = std::env::args().nth(1);
    let reply = askpass_reply(Some("1"), password.as_deref(), prompt.as_deref());
    let code = match reply {
        // ssh reads the answer from a pipe; a terminal here means someone set the marker by hand
        Some(p) if !std::io::IsTerminal::is_terminal(&std::io::stdout()) => {
            println!("{p}");
            0
        }
        _ => 1,
    };
    std::process::exit(code);
}

// ---- ssh ----

/// What to do with the Ready LED; every variant ends with reading it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Status,
    Off,
    On,
}

/// The shell command for the M2K: fixed text, nothing from the user in it.
fn remote_command(action: Action) -> String {
    let read = format!("cat {SYSFS}/trigger {SYSFS}/brightness");
    match action {
        Action::Status => read,
        Action::Off => {
            format!("echo none > {SYSFS}/trigger && echo 0 > {SYSFS}/brightness && {read}")
        }
        Action::On => format!("echo heartbeat > {SYSFS}/trigger && {read}"),
    }
}

/// A host name or IPv4 address that cannot be taken for an ssh option or carry anything else.
fn check_host(host: &str) -> Result<()> {
    let ok = host.starts_with(|c: char| c.is_ascii_alphanumeric())
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if !ok {
        bail!("`{host}` is not a host name ssh can be given");
    }
    Ok(())
}

/// The arguments of `ssh` (without the program). The password is not among them.
fn ssh_args(ssh: &Ssh, host: &str, action: Action) -> Result<Vec<OsString>> {
    check_host(host)?;
    let mut args: Vec<OsString> = vec![];
    let mut opt = |o: &str| {
        args.push("-o".into());
        args.push(o.into());
    };
    match &ssh.known_hosts {
        Some(file) => {
            let file = file
                .to_str()
                .filter(|f| !f.contains(char::is_whitespace) && !f.contains(['"', '%']))
                .ok_or_else(|| {
                    err!(
                        "known hosts file {}: a path without spaces, quotes or `%`, please",
                        file.display()
                    )
                })?;
            opt("StrictHostKeyChecking=accept-new");
            opt(&format!("UserKnownHostsFile={file}"));
        }
        None => {
            if !ssh.password.is_factory() {
                bail!(
                    "a password other than the factory one is only sent to a host whose key is pinned: \
                     give a known hosts file (--ssh-known-hosts FILE; created on first use)"
                );
            }
            opt("StrictHostKeyChecking=no");
            opt("UserKnownHostsFile=/dev/null");
        }
    }
    opt("GlobalKnownHostsFile=/dev/null");
    opt("UpdateHostKeys=no");
    opt("CheckHostIP=no");
    // password only: no key of the user is offered to the M2K, no agent is asked
    opt("PreferredAuthentications=password");
    opt("PubkeyAuthentication=no");
    opt("IdentityAgent=none");
    opt("NumberOfPasswordPrompts=1");
    opt("BatchMode=no");
    opt(&format!(
        "ConnectTimeout={}",
        ssh.timeout.as_secs_f64().ceil().max(1.0) as u64
    ));
    opt("LogLevel=ERROR");
    args.extend(
        ["-F", "none", "-T", "-a", "-x", "-l", USER, "--", host]
            .into_iter()
            .map(OsString::from),
    );
    args.push(remote_command(action).into());
    Ok(args)
}

/// `[heartbeat]` out of `none timer [heartbeat] mmc0`, then the brightness on the next line.
fn parse_ready(output: &str) -> Result<Ready> {
    let mut lines = output.lines().map(str::trim).filter(|l| !l.is_empty());
    let trigger = lines
        .next()
        .and_then(|l| l.split_whitespace().find_map(|w| w.strip_prefix('[')))
        .and_then(|w| w.strip_suffix(']'))
        .filter(|t| {
            !t.is_empty()
                && t.len() <= 64
                && t.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
        });
    let brightness = lines.next().and_then(|l| l.parse::<u32>().ok());
    match (trigger, brightness) {
        (Some(trigger), Some(brightness)) => Ok(Ready {
            on: trigger != "none" || brightness > 0,
            trigger: trigger.to_string(),
            brightness,
        }),
        _ => bail!("the M2K's shell answered something else than the LED's trigger and brightness"),
    }
}

/// The last line of what ssh printed, safe to show: no control characters, not too long.
fn last_line(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let line = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    line.trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect()
}

/// Why ssh failed, from its exit code and what it printed.
fn failure(code: Option<i32>, stderr: &[u8], host: &str) -> Error {
    let line = last_line(stderr);
    let lower = line.to_ascii_lowercase();
    if code != Some(255) {
        // ssh passes the remote command's exit code on
        return err!(
            "the command on the M2K failed (exit code {}): {line}",
            code.map_or("none".into(), |c| c.to_string())
        );
    }
    if lower.contains("permission denied") {
        err!(
            "ssh login as {USER}@{host} was refused: wrong password? (--ssh-password or {ENV_PASSWORD}; \
             the factory one is `{FACTORY_PASSWORD}`)"
        )
    } else if lower.contains("host key") || lower.contains("identification has changed") {
        err!(
            "the host key of {host} is not the pinned one: another M2K, new firmware, or not the M2K at all; \
             remove its line from the known hosts file if that is expected ({line})"
        )
    } else {
        err!("ssh to {USER}@{host} failed: {line}")
    }
}

fn run(ssh: &Ssh, host: &str, action: Action) -> Result<Ready> {
    let Some(askpass) = &ssh.askpass else {
        bail!(
            "no askpass helper to hand ssh the password: install the `m2k` CLI (cargo install m2k_cli) \
             or set the path to it"
        );
    };
    let args = ssh_args(ssh, host, action)?;
    if let Some(file) = &ssh.known_hosts {
        // ssh logs in also when it cannot write the file, and the key would not be pinned then
        if let Some(dir) = file.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| err!("create {}: {e}", dir.display()))?;
        }
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(file)
            .map_err(|e| err!("known hosts file {} cannot be written: {e}", file.display()))?;
    }
    let mut child = match Command::new(&ssh.program)
        .args(args)
        .env("SSH_ASKPASS", askpass)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env(ENV_ASKPASS, "1")
        .env(ENV_PASSWORD, &ssh.password.0)
        .env_remove("SSH_AUTH_SOCK")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(
            "`{}` is not installed (OpenSSH client, 8.4 or later)",
            Path::new(&ssh.program).display()
        ),
        Err(e) => bail!("start `{}`: {e}", Path::new(&ssh.program).display()),
    };
    // the answer is two short lines, far below a pipe's buffer: waiting before reading cannot block ssh
    let deadline = Instant::now() + ssh.timeout + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("ssh to {USER}@{host} did not finish in time");
            }
            Err(e) => bail!("wait for ssh: {e}"),
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| err!("read what ssh printed: {e}"))?;
    if !out.status.success() {
        return Err(failure(out.status.code(), &out.stderr, host));
    }
    parse_ready(&String::from_utf8_lossy(&out.stdout))
}

// ---- the typed calls ----

fn apply(m: &mut M2k, only: Option<Led>, ssh: &Ssh, action: Action) -> Result<Status> {
    m.require_m2k()?;
    let done = match only {
        Some(Led::Ready) => None,
        _ => {
            match action {
                Action::Status => {}
                Action::Off => m.set_done_led(false)?,
                Action::On => m.set_done_led(true)?,
            }
            Some(m.done_led()?)
        }
    };
    let ready = match only {
        Some(Led::Done) => None,
        _ => Some(parse_uri(&m.uri).and_then(|(host, _)| run(ssh, &host, action))),
    };
    Ok(Status { done, ready })
}

/// Both LEDs (or `only` one) as they are now. The Ready LED is an `Err` inside when ssh cannot reach it.
pub fn status(m: &mut M2k, only: Option<Led>, ssh: &Ssh) -> Result<Status> {
    apply(m, only, ssh, Action::Status)
}

/// Switches both LEDs (or `only` one) on or off and reads them back. The DONE LED is switched first; when ssh
/// then cannot reach the Ready LED, that one is an `Err` inside (and unchanged) and [`Status::partial`] is true.
/// RAM only: both are back on after a power cycle.
pub fn set(m: &mut M2k, on: bool, only: Option<Led>, ssh: &Ssh) -> Result<Status> {
    apply(m, only, ssh, if on { Action::On } else { Action::Off })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "s3cret-not-analog";

    fn ssh() -> Ssh {
        Ssh {
            password: Password::default(),
            askpass: Some("/usr/bin/m2k".into()),
            known_hosts: None,
            timeout: Duration::from_millis(2500),
            program: "ssh".into(),
        }
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn remote_commands_are_the_documented_ones() {
        assert_eq!(
            remote_command(Action::Off),
            "echo none > /sys/class/leds/led0:green/trigger && echo 0 > /sys/class/leds/led0:green/brightness \
             && cat /sys/class/leds/led0:green/trigger /sys/class/leds/led0:green/brightness"
        );
        assert!(
            remote_command(Action::On)
                .starts_with("echo heartbeat > /sys/class/leds/led0:green/trigger && cat ")
        );
        assert!(remote_command(Action::Status).starts_with("cat "));
    }

    #[test]
    fn args_without_host_key_check_for_the_factory_password() {
        let args = strings(&ssh_args(&ssh(), "192.168.2.1", Action::Status).unwrap());
        for want in [
            "StrictHostKeyChecking=no",
            "UserKnownHostsFile=/dev/null",
            "GlobalKnownHostsFile=/dev/null",
            "PreferredAuthentications=password",
            "PubkeyAuthentication=no",
            "IdentityAgent=none",
            "NumberOfPasswordPrompts=1",
            "ConnectTimeout=3",
        ] {
            assert!(args.contains(&want.to_string()), "{want} missing: {args:?}");
        }
        // the user's ssh config is not read, the destination cannot be an option, the command comes last
        let n = args.len();
        assert_eq!(
            args[n - 10..n - 2],
            ["-F", "none", "-T", "-a", "-x", "-l", "root", "--"]
        );
        assert_eq!(args[n - 2], "192.168.2.1");
        assert_eq!(args[n - 1], remote_command(Action::Status));
        assert!(!args.iter().any(|a| a.contains(FACTORY_PASSWORD)));
    }

    #[test]
    fn another_password_needs_a_pinned_host_key() {
        let mut s = ssh();
        s.password = Password::new(SECRET);
        let e = ssh_args(&s, "192.168.2.1", Action::Off)
            .unwrap_err()
            .to_string();
        assert!(e.contains("known hosts file"), "{e}");
        assert!(!e.contains(SECRET));

        s.known_hosts = Some("/home/me/.config/m2k/known_hosts".into());
        let args = strings(&ssh_args(&s, "192.168.2.1", Action::Off).unwrap());
        assert!(args.contains(&"StrictHostKeyChecking=accept-new".to_string()));
        assert!(args.contains(&"UserKnownHostsFile=/home/me/.config/m2k/known_hosts".to_string()));
        assert!(!args.iter().any(|a| a.contains("StrictHostKeyChecking=no")));
        assert!(!args.iter().any(|a| a.contains(SECRET)), "{args:?}");

        s.known_hosts = Some("/tmp/with space/known_hosts".into());
        assert!(ssh_args(&s, "192.168.2.1", Action::Off).is_err());
    }

    #[test]
    fn hosts_that_could_be_options_are_refused() {
        for bad in [
            "",
            "-oProxyCommand=id",
            "a b",
            "host;id",
            "root@host",
            "$(id)",
            "::1",
        ] {
            assert!(ssh_args(&ssh(), bad, Action::Status).is_err(), "{bad}");
        }
        for good in ["192.168.2.1", "m2k.local", "omarchy-m1"] {
            assert!(ssh_args(&ssh(), good, Action::Status).is_ok(), "{good}");
        }
    }

    #[test]
    fn parses_trigger_and_brightness() {
        let on = parse_ready("none usb-gadget usb-host timer [heartbeat] mmc0\n255\n").unwrap();
        assert_eq!(
            on,
            Ready {
                on: true,
                trigger: "heartbeat".into(),
                brightness: 255
            }
        );
        let off = parse_ready("[none] usb-gadget timer heartbeat\n0\n").unwrap();
        assert!(!off.on);
        assert_eq!(off.trigger, "none");
        // lit without a trigger is on too
        assert!(parse_ready("[none] timer\n1\n").unwrap().on);
        for bad in [
            "",
            "none timer\n0\n",
            "[none] timer\n",
            "[\x1b[31m] x\n0\n",
            "[none]\nmany\n",
        ] {
            assert!(parse_ready(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn failures_say_why() {
        let denied = failure(
            Some(255),
            b"root@192.168.2.1: Permission denied (publickey,password).\n",
            "192.168.2.1",
        )
        .to_string();
        assert!(denied.contains("wrong password"), "{denied}");
        let key = failure(
            Some(255),
            b"@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @\nHost key verification failed.\n",
            "192.168.2.1",
        )
        .to_string();
        assert!(key.contains("not the pinned one"), "{key}");
        let down = failure(
            Some(255),
            b"ssh: connect to host 192.168.2.1 port 22: Connection refused\r\n",
            "192.168.2.1",
        )
        .to_string();
        assert!(down.ends_with("port 22: Connection refused"), "{down}");
        let remote = failure(Some(1), b"sh: can't create x\x1b[2J\n", "192.168.2.1").to_string();
        assert!(remote.contains("exit code 1"), "{remote}");
        assert!(!remote.contains('\x1b'));
    }

    #[test]
    fn askpass_answers_password_prompts_only() {
        let p = Some(SECRET);
        assert_eq!(
            askpass_reply(Some("1"), p, Some("root@192.168.2.1's password: ")),
            p
        );
        assert_eq!(askpass_reply(Some("1"), p, Some("Password:")), p);
        // not started by ssh for us, or asked something else
        assert_eq!(askpass_reply(None, p, Some("root@h's password: ")), None);
        assert_eq!(
            askpass_reply(Some("0"), p, Some("root@h's password: ")),
            None
        );
        assert_eq!(
            askpass_reply(
                Some("1"),
                p,
                Some("Are you sure you want to continue connecting (yes/no/[fingerprint])? ")
            ),
            None
        );
        assert_eq!(askpass_reply(Some("1"), p, None), None);
        assert_eq!(askpass_reply(Some("1"), None, Some("password: ")), None);
        assert_eq!(askpass_reply(Some("1"), Some(""), Some("password: ")), None);
    }

    #[test]
    fn password_is_not_in_debug_output() {
        let mut s = ssh();
        s.password = Password::new(SECRET);
        let text = format!("{s:?}");
        assert!(!text.contains(SECRET), "{text}");
        assert!(text.contains("***"));
        assert!(!Password::new(SECRET).is_factory());
        assert!(Password::default().is_factory());
    }

    #[test]
    fn missing_ssh_or_askpass_is_a_plain_error() {
        let mut s = ssh();
        s.program = "m2k-test-no-such-ssh".into();
        let e = run(&s, "192.168.2.1", Action::Status)
            .unwrap_err()
            .to_string();
        assert!(e.contains("is not installed"), "{e}");
        s.askpass = None;
        let e = run(&s, "192.168.2.1", Action::Status)
            .unwrap_err()
            .to_string();
        assert!(e.contains("no askpass helper"), "{e}");
    }

    /// A stand-in for `ssh` that does what ssh does with `SSH_ASKPASS`, and a stand-in askpass that answers
    /// from the environment like [`askpass_main`]: the password arrives through the environment only.
    #[cfg(unix)]
    #[test]
    fn password_travels_in_the_environment_not_in_argv() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("m2k_led_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = |name: &str, body: &str| {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            path
        };
        let askpass = script(
            "askpass",
            r#"[ "$M2K_ASKPASS" = 1 ] && printf '%s\n' "$M2K_SSH_PASSWORD""#,
        );
        // logs in when the askpass answer is the expected password and no argument holds it
        let fake_ssh = script(
            "ssh",
            r#"case "$*" in *s3cret*) echo "password in argv" >&2; exit 255;; esac
[ "$SSH_ASKPASS_REQUIRE" = force ] || { echo "no SSH_ASKPASS_REQUIRE" >&2; exit 255; }
[ "$("$SSH_ASKPASS" "root@192.168.2.1's password: ")" = s3cret-not-analog ] \
    || { echo "root@192.168.2.1: Permission denied (publickey,password)." >&2; exit 255; }
printf '[none] timer heartbeat\n0\n'"#,
        );
        let mut s = ssh();
        s.program = fake_ssh.into();
        s.askpass = Some(askpass);
        s.password = Password::new(SECRET);
        s.known_hosts = Some(dir.join("known_hosts"));
        let ready = run(&s, "192.168.2.1", Action::Off).unwrap();
        assert!(!ready.on);

        s.password = Password::new("wrong");
        let e = run(&s, "192.168.2.1", Action::Off).unwrap_err().to_string();
        assert!(e.contains("wrong password"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
