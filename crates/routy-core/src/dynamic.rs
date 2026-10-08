//! Динамические переменные: `{{$uuid}}`, `{{$timestamp}}`, `{{$randomInt}}`,
//! `{{$randomInt 1 100}}`. Значение новое при каждой подстановке.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};

/// Имена с описанием — для подсказок в редакторе.
pub const NAMES: &[(&str, &str)] = &[
    ("$uuid", "random UUID v4"),
    ("$timestamp", "Unix time, seconds"),
    ("$randomInt", "0..1000, or `$randomInt min max`"),
];

pub fn is_dynamic(name: &str) -> bool {
    name.starts_with('$')
}

/// `name` — содержимое `{{…}}` без скобок, например `$randomInt 1 10`.
pub fn eval(name: &str) -> Result<String> {
    let mut parts = name.split_whitespace();
    let head = parts.next().unwrap_or_default();
    let args: Vec<&str> = parts.collect();
    match (head, args.as_slice()) {
        ("$uuid", []) => Ok(uuid()),
        ("$timestamp", []) => Ok(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default()
            .to_string()),
        ("$randomInt", []) => Ok(fastrand::i64(0..1000).to_string()),
        ("$randomInt", [min, max]) => {
            let parse = |s: &str| {
                s.parse::<i64>()
                    .map_err(|_| Error::expr(name, format!("`{s}` is not an integer")))
            };
            let (min, max) = (parse(min)?, parse(max)?);
            if min >= max {
                return Err(Error::expr(name, "min must be less than max"));
            }
            Ok(fastrand::i64(min..max).to_string())
        }
        _ => Err(Error::expr(
            name,
            "unknown dynamic variable; available: $uuid, $timestamp, $randomInt [min max]",
        )),
    }
}

/// Случайный UUID v4.
pub fn uuid() -> String {
    uuid_v4(fastrand::u128(..))
}

fn uuid_v4(bits: u128) -> String {
    // Версия 4 и вариант RFC 4122.
    let b = (bits & !(0xf << 76) | (0x4 << 76)) & !(0x3 << 62) | (0x2 << 62);
    let h = format!("{b:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_format() {
        let u = uuid_v4(u128::MAX);
        assert_eq!(u, "ffffffff-ffff-4fff-bfff-ffffffffffff");
        assert_eq!(uuid_v4(0), "00000000-0000-4000-8000-000000000000");
        let u = eval("$uuid").unwrap();
        assert_eq!(u.len(), 36);
        assert_ne!(u, eval("$uuid").unwrap());
    }

    #[test]
    fn values() {
        assert!(eval("$timestamp").unwrap().parse::<u64>().unwrap() > 1_700_000_000);
        let n: i64 = eval("$randomInt").unwrap().parse().unwrap();
        assert!((0..1000).contains(&n));
        let n: i64 = eval("$randomInt -5  -3").unwrap().parse().unwrap();
        assert!((-5..-3).contains(&n));
    }

    #[test]
    fn errors() {
        assert!(eval("$nope").is_err());
        assert!(eval("$uuid 1").is_err());
        assert!(eval("$randomInt 5 5").is_err());
        assert!(eval("$randomInt a 5").is_err());
    }
}
