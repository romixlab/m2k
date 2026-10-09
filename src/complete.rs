//! Shell completions, produced by the `m2k` binary itself when the shell calls it with `COMPLETE=<shell>` set
//! (see [CompleteEnv]): `source <(COMPLETE=bash m2k)`, `COMPLETE=fish m2k | source`, ...
//!
//! Device, channel and attribute names come live from the device at `M2K_URI` (default the M2K at
//! 192.168.2.1, 300 ms to answer), else from the ADALM2000's known devices. The Bash adapter ([Bash]) is the one
//! from wire_weaver_cli and saleae_cli (keep in sync), so values with spaces complete as one word.

use crate::m2k;
use clap::Command;
use clap_complete::CompleteEnv;
use clap_complete::CompletionCandidate;
use clap_complete::env::{Elvish, EnvCompleter, Fish, Powershell, Shells, Zsh};
use iiod_client::Context;
use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

/// Answers a shell completion request and exits if there is one, otherwise does nothing.
pub(crate) fn complete(factory: fn() -> Command) {
    CompleteEnv::with_factory(factory)
        .shells(Shells(&[&Bash, &Elvish, &Fish, &Powershell, &Zsh]))
        .complete();
}

/// The context of the device at `M2K_URI`, if it answers quickly.
fn live() -> Option<Context> {
    let uri = std::env::var("M2K_URI").unwrap_or_else(|_| m2k::DEFAULT_URI.into());
    let (host, port) = m2k::parse_uri(&uri).ok()?;
    let addr = std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), port))
        .ok()?
        .next()?;
    let mut client = iiod_client::Client::connect(addr, Duration::from_millis(300)).ok()?;
    client.context().ok()
}

/// Devices of the ADALM2000, when no device answers.
const M2K_DEVICES: [&str; 14] = [
    "m2k-adc",
    "xadc",
    "m2k-logic-analyzer-rx",
    "m2k-fabric",
    "m2k-adc-trigger",
    "ad9963",
    "ad5625",
    "ad5627",
    "pll",
    "m2k-dds",
    "m2k-dac-a",
    "m2k-dac-b",
    "m2k-logic-analyzer",
    "m2k-logic-analyzer-tx",
];

pub(crate) fn device_names() -> Vec<CompletionCandidate> {
    match live() {
        Some(ctx) => ctx
            .devices
            .iter()
            .map(|d| {
                CompletionCandidate::new(d.name.clone().unwrap_or_else(|| d.id.clone()))
                    .help(Some(d.id.clone().into()))
            })
            .collect(),
        None => M2K_DEVICES
            .iter()
            .map(|d| CompletionCandidate::new(*d))
            .collect(),
    }
}

fn unique(mut names: Vec<String>) -> Vec<CompletionCandidate> {
    names.sort();
    names.dedup();
    names.into_iter().map(CompletionCandidate::new).collect()
}

/// Every channel id of the live context.
pub(crate) fn channel_names() -> Vec<CompletionCandidate> {
    let Some(ctx) = live() else {
        return unique(
            (0..16)
                .map(|i| format!("voltage{i}"))
                .chain(["temp0".into()])
                .collect(),
        );
    };
    unique(
        ctx.devices
            .iter()
            .flat_map(|d| d.channels.iter().map(|c| c.id.clone()))
            .collect(),
    )
}

/// Every attribute name of the live context (device, channel, buffer and debug).
pub(crate) fn attr_names() -> Vec<CompletionCandidate> {
    let Some(ctx) = live() else {
        return unique(
            [
                "sampling_frequency",
                "sampling_frequency_available",
                "oversampling_ratio",
                "raw",
                "scale",
                "offset",
                "powerdown",
                "calibscale",
                "calibbias",
                "gain",
                "calibration_mode",
            ]
            .map(String::from)
            .to_vec(),
        );
    };
    unique(
        ctx.devices
            .iter()
            .flat_map(|d| {
                d.attrs
                    .iter()
                    .chain(&d.buffer_attrs)
                    .chain(&d.debug_attrs)
                    .chain(d.channels.iter().flat_map(|c| c.attrs.iter()))
                    .cloned()
            })
            .collect(),
    )
}

/// Bash adapter that quotes completions.
///
/// Bash inserts completions verbatim and splits the word being completed on `COMP_WORDBREAKS` (quotes, `@`, `:`,
/// `=`, ...), while clap_complete's adapter passes neither quoting nor these splits on, so values with spaces
/// broke into several words and values like `--api name@^0.1` could not be completed. Here the raw command line is
/// split by `m2k` instead, and each candidate is turned into the text bash should put in place of its current word:
/// only the part after the last word break, escaped for an already open quote or single-quoted when needed.
struct Bash;

impl EnvCompleter for Bash {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn is(&self, name: &str) -> bool {
        name == "bash"
    }

    fn write_registration(
        &self,
        var: &str,
        name: &str,
        bin: &str,
        completer: &str,
        buf: &mut dyn Write,
    ) -> Result<(), std::io::Error> {
        let script = r#"
_m2k_complete_NAME() {
    local IFS=$'\013'
    if compopt +o nospace 2> /dev/null; then
        local space=false
    else
        local space=true
    fi
    COMPREPLY=( $( \
        _M2K_COMP_LINE="${COMP_LINE:0:COMP_POINT}" \
        _M2K_COMP_WORDBREAKS="$COMP_WORDBREAKS" \
        _CLAP_IFS="$IFS" \
        VAR="bash" \
        'COMPLETER' -- "$1" \
    ) )
    if [[ $? != 0 ]]; then
        unset COMPREPLY
    elif [[ $space == false ]] && [[ "${COMPREPLY-}" =~ [=/:]\'?$ ]]; then
        compopt -o nospace
    fi
}
if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -o nospace -o bashdefault -o nosort -F _m2k_complete_NAME BIN
else
    complete -o nospace -o bashdefault -F _m2k_complete_NAME BIN
fi
"#
        .replace("NAME", &name.replace('-', "_"))
        .replace("BIN", bin)
        .replace("COMPLETER", &completer.replace('\'', r"'\''"))
        .replace("VAR", var);
        writeln!(buf, "{script}")
    }

    fn write_complete(
        &self,
        cmd: &mut Command,
        _args: Vec<OsString>,
        current_dir: Option<&Path>,
        buf: &mut dyn Write,
    ) -> Result<(), std::io::Error> {
        let line = std::env::var("_M2K_COMP_LINE").unwrap_or_default();
        let wordbreaks = std::env::var("_M2K_COMP_WORDBREAKS").unwrap_or_default();
        let ifs = std::env::var("_CLAP_IFS").unwrap_or_else(|_| "\n".into());
        let (words, current) = split_line(&line, &wordbreaks);
        let index = words.len() - 1;
        let args = words.into_iter().map(OsString::from).collect();
        let completions = clap_complete::engine::complete(cmd, args, index, current_dir)?;
        let replies: Vec<_> = completions
            .iter()
            .filter_map(|c| current.reply(&c.get_value().to_string_lossy()))
            .collect();
        write!(buf, "{}", replies.join(&ifs))
    }
}

/// The word being completed, as seen by bash.
#[derive(Debug, PartialEq)]
struct CurrentWord {
    /// The word without quotes and escapes.
    value: String,
    /// Quote left open, if any.
    open_quote: Option<char>,
    /// Length of the start of `value` that bash keeps: before the open quote or the last word break.
    kept: usize,
}

/// Splits a command line (up to the cursor) into shell words, the last one being the word being completed.
fn split_line(line: &str, wordbreaks: &str) -> (Vec<String>, CurrentWord) {
    let mut words = vec![];
    let mut word = String::new();
    let mut in_word = false;
    let mut quote = None;
    let mut kept = 0;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => {
                quote = None;
                // bash treats the closing quote as a word break
                kept = word.len();
            }
            (Some('"'), '\\') => match chars.next() {
                Some(e @ ('"' | '\\' | '$' | '`')) => word.push(e),
                Some(e) => {
                    word.push('\\');
                    word.push(e);
                }
                None => word.push('\\'),
            },
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
                kept = word.len();
            }
            (None, '\\') => {
                in_word = true;
                if let Some(e) = chars.next() {
                    word.push(e);
                }
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
                kept = 0;
            }
            (None, c) => {
                in_word = true;
                if wordbreaks.contains(c) {
                    // bash keeps `@` and `$` (its rl_special_prefixes) as the start of the next word
                    kept = word.len()
                        + if matches!(c, '@' | '$') {
                            0
                        } else {
                            c.len_utf8()
                        };
                }
                word.push(c);
            }
        }
    }
    words.push(word.clone());
    let current = CurrentWord {
        value: word,
        open_quote: quote,
        kept,
    };
    (words, current)
}

impl CurrentWord {
    /// Text to put in place of bash's current word to complete `candidate`, `None` if it doesn't continue the word.
    fn reply(&self, candidate: &str) -> Option<String> {
        let rest = candidate.strip_prefix(&self.value[..self.kept])?;
        Some(match self.open_quote {
            Some('"') => rest
                .chars()
                .flat_map(|c| match c {
                    '"' | '\\' | '$' | '`' => vec!['\\', c],
                    c => vec![c],
                })
                .collect(),
            Some(_) => rest.replace('\'', r"'\''"),
            None => quote(rest),
        })
    }
}

/// Single-quotes a value containing characters the shell would interpret (spaces, quotes, `$`, ...).
fn quote(v: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "-_.,:/@%+=^~".contains(c);
    if v.chars().all(plain) {
        v.to_string()
    } else {
        format!("'{}'", v.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORDBREAKS: &str = "\"'><=;|&(:@";

    fn reply(line: &str, candidate: &str) -> Option<String> {
        split_line(line, WORDBREAKS).1.reply(candidate)
    }

    #[test]
    fn splits_words() {
        let (words, current) = split_line(r#"m2k  --label 'a b' "c\"d" e\ f g"#, WORDBREAKS);
        assert_eq!(words, ["m2k", "--label", "a b", "c\"d", "e f", "g"]);
        assert_eq!(current.value, "g");
        let (words, current) = split_line("m2k --label ", WORDBREAKS);
        assert_eq!(words, ["m2k", "--label", ""]);
        assert_eq!(current.value, "");
    }

    #[test]
    fn quotes_values_needing_it() {
        assert_eq!(
            reply("m2k --label N", "Nucleo on the desk").unwrap(),
            "'Nucleo on the desk'"
        );
        assert_eq!(reply("m2k --label ", "it's").unwrap(), r"'it'\''s'");
        assert_eq!(reply("m2k --serial 21", "2100AB").unwrap(), "2100AB");
    }

    #[test]
    fn escapes_for_open_quote() {
        assert_eq!(
            reply("m2k --label 'Nu", "Nucleo on the desk").unwrap(),
            "Nucleo on the desk"
        );
        assert_eq!(
            reply(r#"m2k --label "Nu"#, r#"Nu "x" $y"#).unwrap(),
            r#"Nu \"x\" \$y"#
        );
        assert_eq!(
            reply("m2k --label 'a b' --label 'it", "it's").unwrap(),
            r"it'\''s"
        );
    }

    #[test]
    fn replaces_after_word_break() {
        assert_eq!(
            reply("m2k --api blinky_api@", "blinky_api@^0.1.0").unwrap(),
            "@^0.1.0"
        );
        assert_eq!(reply("m2k --vid-pid c0de:", "c0de:cafe").unwrap(), "cafe");
        assert_eq!(
            reply("m2k --label=Nu", "--label=Nucleo on the desk").unwrap(),
            "'Nucleo on the desk'"
        );
        assert_eq!(reply("m2k --api blinky_api@", "other"), None);
    }
}
