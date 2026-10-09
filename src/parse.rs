//! Numbers with SI suffixes (`1k`, `2.5M`, `100m`), durations (`500ms`, `10s`), and printing them back.

use anyhow::{Result, anyhow, bail};
use std::time::Duration;

/// `1k` → 1000, `2.5M` → 2.5e6, `100m` → 0.1, `1e6` → 1e6, `4u` → 4e-6. A trailing unit (`Hz`, `V`, `S/s`) is
/// ignored, so `1kHz` and `1.5V` work too.
pub fn si(text: &str) -> Result<f64> {
    let t = text.trim();
    let t = t
        .strip_suffix("S/s")
        .or_else(|| t.strip_suffix("Hz"))
        .or_else(|| t.strip_suffix("sps"))
        .or_else(|| t.strip_suffix('V'))
        .unwrap_or(t);
    let (num, mult) = match t.char_indices().last() {
        Some((i, c)) => match c {
            'k' | 'K' => (&t[..i], 1e3),
            'M' => (&t[..i], 1e6),
            'G' => (&t[..i], 1e9),
            'm' => (&t[..i], 1e-3),
            'u' | 'µ' => (&t[..i], 1e-6),
            _ => (t, 1.0),
        },
        None => (t, 1.0),
    };
    let value: f64 = num
        .parse()
        .map_err(|_| anyhow!("`{text}` is not a number (examples: 1000, 1k, 2.5M, 100m)"))?;
    if !value.is_finite() {
        bail!("`{text}` is not a finite number");
    }
    Ok(value * mult)
}

/// A count like `10k` or `1M`, rounded.
pub fn count(text: &str) -> Result<usize> {
    let v = si(text)?;
    if v < 1.0 || v > 1e12 {
        bail!("`{text}`: expected a count of at least 1");
    }
    Ok(v.round() as usize)
}

/// `500ms`, `10s`, `2m`, `1h`, `250us`; a bare number is seconds.
pub fn duration(text: &str) -> Result<Duration> {
    let t = text.trim();
    let (num, scale) = if let Some(n) = t.strip_suffix("ms") {
        (n, 1e-3)
    } else if let Some(n) = t.strip_suffix("us") {
        (n, 1e-6)
    } else if let Some(n) = t.strip_suffix('s') {
        (n, 1.0)
    } else if let Some(n) = t.strip_suffix('m') {
        (n, 60.0)
    } else if let Some(n) = t.strip_suffix('h') {
        (n, 3600.0)
    } else {
        (t, 1.0)
    };
    let v: f64 = num
        .trim()
        .parse()
        .map_err(|_| anyhow!("`{text}` is not a duration (examples: 500ms, 10s, 2m)"))?;
    if !v.is_finite() || v < 0.0 {
        bail!("`{text}` is not a duration");
    }
    Ok(Duration::from_secs_f64(v * scale))
}

/// 1500 → `1.5k`, 0.0025 → `2.5m`: three significant digits.
pub fn fmt_si(value: f64) -> String {
    let a = value.abs();
    let (scaled, suffix) = if a == 0.0 || !a.is_finite() {
        (value, "")
    } else if a >= 1e9 {
        (value / 1e9, "G")
    } else if a >= 1e6 {
        (value / 1e6, "M")
    } else if a >= 1e3 {
        (value / 1e3, "k")
    } else if a >= 1.0 {
        (value, "")
    } else if a >= 1e-3 {
        (value * 1e3, "m")
    } else if a >= 1e-6 {
        (value * 1e6, "µ")
    } else {
        (value * 1e9, "n")
    };
    let digits = if scaled.abs() >= 100.0 {
        0
    } else if scaled.abs() >= 10.0 {
        1
    } else {
        2
    };
    let text = format!("{scaled:.digits$}");
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    };
    format!("{text}{suffix}")
}

/// A value with a unit: `1 MS/s`, `2.5 kHz`.
pub fn fmt_unit(value: f64, unit: &str) -> String {
    let text = fmt_si(value);
    match text.char_indices().last() {
        Some((i, c)) if c.is_alphabetic() => format!("{} {c}{unit}", &text[..i]),
        _ => format!("{text} {unit}"),
    }
}

/// Seconds as `10 ms`, `1.5 s`.
pub fn fmt_secs(secs: f64) -> String {
    fmt_unit(secs, "s")
}

/// A list of pins or channels: `0-3,7`.
pub fn list(text: &str, max: u32) -> Result<Vec<u32>> {
    let mut out = vec![];
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let num = |s: &str| -> Result<u32> {
            let n: u32 = s
                .trim()
                .parse()
                .map_err(|_| anyhow!("`{s}` in `{text}` is not a number"))?;
            if n > max {
                bail!("{n} in `{text}` is above {max}");
            }
            Ok(n)
        };
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b) = (num(a)?, num(b)?);
                if a > b {
                    bail!("`{part}`: range goes backwards");
                }
                out.extend(a..=b);
            }
            None => out.push(num(part)?),
        }
    }
    if out.is_empty() {
        bail!("`{text}`: empty list");
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_si() {
        assert_eq!(si("1k").unwrap(), 1e3);
        assert_eq!(si("2.5M").unwrap(), 2.5e6);
        assert_eq!(si("100m").unwrap(), 0.1);
        assert_eq!(si("1e6").unwrap(), 1e6);
        assert_eq!(si("-1.5V").unwrap(), -1.5);
        assert_eq!(si("1kHz").unwrap(), 1e3);
        assert!((si("4u").unwrap() - 4e-6).abs() < 1e-18);
        assert!((si("4µ").unwrap() - 4e-6).abs() < 1e-18);
        assert!(si("abc").is_err());
        assert!(si("").is_err());
        assert_eq!(count("10k").unwrap(), 10_000);
        assert!(count("0").is_err());
    }

    #[test]
    fn parses_durations() {
        assert_eq!(duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(duration("10s").unwrap(), Duration::from_secs(10));
        assert_eq!(duration("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(duration("3").unwrap(), Duration::from_secs(3));
        assert!(duration("-1s").is_err());
        assert!(duration("x").is_err());
    }

    #[test]
    fn formats() {
        assert_eq!(fmt_si(1500.0), "1.5k");
        assert_eq!(fmt_si(1e8), "100M");
        assert_eq!(fmt_si(0.0025), "2.5m");
        assert_eq!(fmt_si(0.0), "0");
        assert_eq!(fmt_si(999.4), "999");
        assert_eq!(fmt_secs(0.01), "10 ms");
        assert_eq!(fmt_secs(1.5), "1.5 s");
        assert_eq!(fmt_secs(2e-6), "2 µs");
        assert_eq!(fmt_unit(1e6, "S/s"), "1 MS/s");
        assert_eq!(fmt_unit(50.0, "Hz"), "50 Hz");
    }

    #[test]
    fn parses_lists() {
        assert_eq!(list("0-3,7", 15).unwrap(), [0, 1, 2, 3, 7]);
        assert_eq!(list("2,1,2", 15).unwrap(), [1, 2]);
        assert!(list("16", 15).is_err());
        assert!(list("3-1", 15).is_err());
        assert!(list("", 15).is_err());
    }
}
