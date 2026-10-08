//! Мини-язык для `> save` и `> assert`.
//!
//! Путь:      `status`, `duration`, `body`, `body.user.id`, `body.items[0].name`,
//!            `headers.content-type` (имя заголовка без учёта регистра).
//! Проверка:  `<путь> <op> <значение>` или `<путь> exists`.
//!            op: `==` `!=` `<` `<=` `>` `>=` `contains`.
//!            Значение — JSON-литерал (`200`, `"ok"`, `true`, `null`) или голое слово.

use serde_json::Value;

use crate::error::{Error, Result};

/// То, к чему применяются выражения: ответ сервера, приведённый к JSON-значениям.
pub trait Subject {
    fn status(&self) -> u16;
    fn duration_ms(&self) -> u64;
    fn header(&self, name: &str) -> Option<&str>;
    fn body_json(&self) -> Option<&Value>;
    fn body_text(&self) -> &str;
}

#[derive(Debug, Clone, PartialEq)]
pub enum Root {
    Status,
    Duration,
    Body,
    Header(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    Key(String),
    Index(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    pub source: String,
    pub root: Root,
    pub segments: Vec<Segment>,
}

impl Path {
    pub fn parse(src: &str) -> Result<Path> {
        let src = src.trim();
        let err = |msg: &str| Error::expr(src, msg);
        if src.is_empty() {
            return Err(err("empty path"));
        }

        if let Some(name) = src.strip_prefix("headers.") {
            if name.is_empty() {
                return Err(err("header name is empty"));
            }
            return Ok(Path {
                source: src.into(),
                root: Root::Header(name.to_ascii_lowercase()),
                segments: vec![],
            });
        }

        let head_end = src.find(['.', '[']).unwrap_or(src.len());
        let root = match &src[..head_end] {
            "status" => Root::Status,
            "duration" => Root::Duration,
            "body" => Root::Body,
            other => {
                return Err(err(&format!(
                    "unknown root `{other}` (expected status, duration, body or headers.<name>)"
                )));
            }
        };

        let mut segments = Vec::new();
        let mut rest = &src[head_end..];
        while !rest.is_empty() {
            if let Some(r) = rest.strip_prefix('.') {
                let end = r.find(['.', '[']).unwrap_or(r.len());
                if end == 0 {
                    return Err(err("empty key after `.`"));
                }
                segments.push(Segment::Key(r[..end].to_string()));
                rest = &r[end..];
            } else if let Some(r) = rest.strip_prefix('[') {
                let end = r.find(']').ok_or_else(|| err("unclosed `[`"))?;
                let inner = r[..end].trim();
                let seg =
                    if let Some(key) = inner.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                        Segment::Key(key.to_string())
                    } else {
                        Segment::Index(
                            inner
                                .parse()
                                .map_err(|_| err("index must be a number or \"key\""))?,
                        )
                    };
                segments.push(seg);
                rest = &r[end + 1..];
            } else {
                return Err(err("expected `.` or `[`"));
            }
        }

        if !segments.is_empty() && root != Root::Body {
            return Err(err("only `body` can be indexed"));
        }
        Ok(Path {
            source: src.into(),
            root,
            segments,
        })
    }

    /// `None` — значения по такому пути нет.
    pub fn eval(&self, s: &dyn Subject) -> Option<Value> {
        match &self.root {
            Root::Status => Some(Value::from(s.status())),
            Root::Duration => Some(Value::from(s.duration_ms())),
            Root::Header(name) => s.header(name).map(|v| Value::String(v.to_string())),
            Root::Body => {
                let Some(mut cur) = s.body_json() else {
                    // Не-JSON тело доступно только целиком.
                    return self
                        .segments
                        .is_empty()
                        .then(|| Value::String(s.body_text().to_string()));
                };
                for seg in &self.segments {
                    cur = match seg {
                        Segment::Key(k) => cur.get(k)?,
                        Segment::Index(i) => cur.get(i)?,
                    };
                }
                Some(cur.clone())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Contains,
    Exists,
}

impl Op {
    fn parse(s: &str) -> Option<Op> {
        Some(match s {
            "==" => Op::Eq,
            "!=" => Op::Ne,
            "<" => Op::Lt,
            "<=" => Op::Le,
            ">" => Op::Gt,
            ">=" => Op::Ge,
            "contains" => Op::Contains,
            "exists" => Op::Exists,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Assertion {
    pub source: String,
    pub path: Path,
    pub op: Op,
    pub expected: Value,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AssertOutcome {
    pub source: String,
    pub passed: bool,
    pub actual: Option<Value>,
    /// Почему проверка не прошла, для `*.routy`: `body.total is 0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Assertion {
    pub fn parse(src: &str) -> Result<Assertion> {
        let src = src.trim();
        let err = |msg: &str| Error::expr(src, msg);

        let (lhs, rest) = split_path(src);
        let path = Path::parse(lhs)?;
        let rest = rest.trim_start();
        let (op_str, rhs) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let op = Op::parse(op_str)
            .ok_or_else(|| err("expected an operator: == != < <= > >= contains exists"))?;
        let rhs = rhs.trim();

        let expected = match op {
            Op::Exists if !rhs.is_empty() => return Err(err("`exists` takes no value")),
            Op::Exists => Value::Null,
            _ if rhs.is_empty() => return Err(err("missing value after operator")),
            _ => literal(rhs),
        };
        Ok(Assertion {
            source: src.into(),
            path,
            op,
            expected,
        })
    }

    pub fn check(&self, s: &dyn Subject) -> AssertOutcome {
        let actual = self.path.eval(s);
        let passed = match (&actual, self.op) {
            (a, Op::Exists) => a.is_some(),
            (None, Op::Ne) => true,
            (None, _) => false,
            (Some(a), op) => compare(a, op, &self.expected),
        };
        AssertOutcome {
            source: self.source.clone(),
            passed,
            actual,
            detail: None,
        }
    }
}

/// Отделяет путь от остального выражения; пробелы внутри `[...]` не считаются разделителем.
fn split_path(src: &str) -> (&str, &str) {
    let mut depth = 0usize;
    for (i, c) in src.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            c if c.is_whitespace() && depth == 0 => return (&src[..i], &src[i..]),
            _ => {}
        }
    }
    (src, "")
}

/// JSON-литерал, а если не парсится — строка как есть (`== ok` то же, что `== "ok"`).
fn literal(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_string()))
}

fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn compare(actual: &Value, op: Op, expected: &Value) -> bool {
    use std::cmp::Ordering;

    let ordering = || -> Option<Ordering> {
        match (as_number(actual), as_number(expected)) {
            (Some(a), Some(b)) => a.partial_cmp(&b),
            _ => match (actual, expected) {
                (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
                _ => None,
            },
        }
    };
    let equal = || {
        actual == expected
            || matches!((as_number(actual), as_number(expected)), (Some(a), Some(b)) if a == b)
    };

    match op {
        Op::Eq => equal(),
        Op::Ne => !equal(),
        Op::Lt => ordering() == Some(Ordering::Less),
        Op::Le => matches!(ordering(), Some(Ordering::Less | Ordering::Equal)),
        Op::Gt => ordering() == Some(Ordering::Greater),
        Op::Ge => matches!(ordering(), Some(Ordering::Greater | Ordering::Equal)),
        Op::Contains => match actual {
            Value::String(a) => match expected {
                Value::String(e) => a.contains(e.as_str()),
                other => a.contains(&other.to_string()),
            },
            Value::Array(items) => items.contains(expected),
            Value::Object(map) => expected.as_str().is_some_and(|k| map.contains_key(k)),
            _ => false,
        },
        Op::Exists => true,
    }
}

/// Значение для `> save`: строки без кавычек, остальное — компактный JSON.
pub fn value_to_var(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Fake {
        body: Option<Value>,
    }

    impl Subject for Fake {
        fn status(&self) -> u16 {
            201
        }
        fn duration_ms(&self) -> u64 {
            42
        }
        fn header(&self, name: &str) -> Option<&str> {
            (name == "content-type").then_some("application/json; charset=utf-8")
        }
        fn body_json(&self) -> Option<&Value> {
            self.body.as_ref()
        }
        fn body_text(&self) -> &str {
            "raw"
        }
    }

    fn subject() -> Fake {
        Fake {
            body: Some(json!({
                "access_token": "abc",
                "user": {"id": 7, "tags": ["a", "b"]},
                "items": [{"name": "first"}],
                "weird key": 1
            })),
        }
    }

    fn check(src: &str) -> bool {
        Assertion::parse(src).unwrap().check(&subject()).passed
    }

    #[test]
    fn paths() {
        let s = subject();
        let eval = |p: &str| Path::parse(p).unwrap().eval(&s);
        assert_eq!(eval("status"), Some(json!(201)));
        assert_eq!(eval("body.access_token"), Some(json!("abc")));
        assert_eq!(eval("body.items[0].name"), Some(json!("first")));
        assert_eq!(eval("body[\"weird key\"]"), Some(json!(1)));
        assert_eq!(eval("body.nope"), None);
        assert_eq!(
            eval("headers.Content-Type"),
            Some(json!("application/json; charset=utf-8"))
        );
    }

    #[test]
    fn path_errors() {
        assert!(Path::parse("foo").is_err());
        assert!(Path::parse("body.").is_err());
        assert!(Path::parse("body[x]").is_err());
        assert!(Path::parse("status.code").is_err());
    }

    #[test]
    fn assertions() {
        assert!(check("status == 201"));
        assert!(!check("status == 200"));
        assert!(check("status >= 200"));
        assert!(check("status < 300"));
        assert!(check("duration < 1000"));
        assert!(check("body.access_token == \"abc\""));
        assert!(check("body.access_token == abc"));
        assert!(check("body.user.tags contains \"b\""));
        assert!(check("body.user contains \"id\""));
        assert!(check("headers.content-type contains json"));
        assert!(check("body.user.id exists"));
        assert!(!check("body.missing exists"));
        assert!(check("body.missing != 1"));
        assert!(check("body[\"weird key\"] == 1"));
    }

    #[test]
    fn non_json_body_is_text() {
        let s = Fake { body: None };
        assert_eq!(Path::parse("body").unwrap().eval(&s), Some(json!("raw")));
        assert_eq!(Path::parse("body.x").unwrap().eval(&s), None);
    }

    #[test]
    fn assertion_errors() {
        assert!(Assertion::parse("status").is_err());
        assert!(Assertion::parse("status ~= 1").is_err());
        assert!(Assertion::parse("status ==").is_err());
        assert!(Assertion::parse("body.x exists 1").is_err());
    }
}
