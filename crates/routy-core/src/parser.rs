//! Формат файла запроса (`*.http`):
//!
//! ```text
//! # комментарии в начале файла и среди заголовков: `#` или `//`
//! POST {{base}}/users
//! Authorization: Bearer {{token}}
//!
//! {"name": "Viktor"}
//!
//! > save token = body.access_token
//! > assert status == 200
//! ```
//!
//! Строка запроса: `МЕТОД URL [HTTP/версия]`, метод можно опустить (тогда GET).
//! После заголовков — пустая строка и тело. Строки `> …` в самом конце файла — директивы.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::expr::{Assertion, Path};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Directive {
    Save { var: String, path: Path },
    Assert(Assertion),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RequestFile {
    pub method: String,
    pub url: String,
    pub headers: Vec<Header>,
    pub body: Option<String>,
    pub directives: Vec<Directive>,
}

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('#') || t.starts_with("//")
}

fn is_method(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_uppercase())
}

pub fn parse(src: &str) -> Result<RequestFile> {
    let lines: Vec<&str> = src.lines().collect();
    let n = lines.len();

    // Директивы: хвост файла из строк `>` и пустых строк.
    let mut body_end = n;
    while body_end > 0 {
        let t = lines[body_end - 1].trim();
        if t.is_empty() || t.starts_with('>') {
            body_end -= 1;
        } else {
            break;
        }
    }
    let mut directives = Vec::new();
    for (i, line) in lines.iter().enumerate().skip(body_end) {
        if let Some(d) = line.trim().strip_prefix('>') {
            directives.push(parse_directive(d.trim()).map_err(|e| at_line(e, i + 1))?);
        }
    }

    // Строка запроса.
    let mut i = 0;
    while i < body_end && (lines[i].trim().is_empty() || is_comment(lines[i])) {
        i += 1;
    }
    if i >= body_end {
        return Err(Error::parse(
            i.max(1),
            "no request line (expected `METHOD URL`)",
        ));
    }
    let (method, url) = parse_request_line(lines[i]).map_err(|m| Error::parse(i + 1, m))?;
    i += 1;

    // Заголовки до пустой строки.
    let mut headers = Vec::new();
    while i < body_end && !lines[i].trim().is_empty() {
        let line = lines[i];
        if !is_comment(line) {
            let (name, value) = line.split_once(':').ok_or_else(|| {
                Error::parse(
                    i + 1,
                    format!("expected `Name: value`, got `{}`", line.trim()),
                )
            })?;
            let name = name.trim();
            if name.is_empty() || name.contains(char::is_whitespace) {
                return Err(Error::parse(i + 1, format!("invalid header name `{name}`")));
            }
            headers.push(Header {
                name: name.to_string(),
                value: value.trim().to_string(),
            });
        }
        i += 1;
    }

    // Тело: всё, что между заголовками и директивами.
    while i < body_end && lines[i].trim().is_empty() {
        i += 1;
    }
    let body = (i < body_end).then(|| lines[i..body_end].join("\n"));

    Ok(RequestFile {
        method,
        url,
        headers,
        body,
        directives,
    })
}

/// Слова через пробел; внутри `{{ … }}` пробелы не разделяют (`{{$randomInt 1 10}}`).
fn words(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = None;
    let mut depth = false;
    let mut chars = line.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let next = chars.peek().map(|&(_, n)| n);
        if !depth && c == '{' && next == Some('{') {
            depth = true;
        } else if depth && c == '}' && next == Some('}') {
            depth = false;
        }
        if c.is_whitespace() && !depth {
            if let Some(s) = start.take() {
                out.push(&line[s..i]);
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        out.push(&line[s..]);
    }
    out
}

fn parse_request_line(line: &str) -> Result<(String, String), String> {
    let mut parts = words(line).into_iter();
    let first = parts.next().ok_or("empty request line")?;
    let (method, url) = if is_method(first) {
        let url = parts
            .next()
            .ok_or_else(|| format!("missing URL after `{first}`"))?;
        (first.to_string(), url.to_string())
    } else {
        ("GET".to_string(), first.to_string())
    };
    match parts.next() {
        None => {}
        Some(v) if v.starts_with("HTTP/") => {}
        Some(extra) => {
            return Err(format!(
                "unexpected `{extra}` after URL (spaces in URL must be encoded)"
            ));
        }
    }
    if let Some(extra) = parts.next() {
        return Err(format!("unexpected `{extra}` at end of request line"));
    }
    Ok((method, url))
}

fn parse_directive(src: &str) -> Result<Directive> {
    let (kw, rest) = src.split_once(char::is_whitespace).unwrap_or((src, ""));
    match kw {
        "save" => {
            let (var, path) = rest
                .split_once('=')
                .ok_or_else(|| Error::parse(0, "expected `> save <name> = <path>`"))?;
            let var = var.trim();
            if var.is_empty()
                || !var
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
                return Err(Error::parse(0, format!("invalid variable name `{var}`")));
            }
            Ok(Directive::Save {
                var: var.to_string(),
                path: Path::parse(path)?,
            })
        }
        "assert" => Ok(Directive::Assert(Assertion::parse(rest)?)),
        other => Err(Error::parse(
            0,
            format!("unknown directive `{other}` (expected save or assert)"),
        )),
    }
}

fn at_line(e: Error, line: usize) -> Error {
    match e {
        Error::Parse { msg, .. } => Error::Parse { line, msg },
        other => Error::Parse {
            line,
            msg: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Op;

    #[test]
    fn full_example() {
        let src = "POST {{base}}/users\nAuthorization: Bearer {{token}}\n\n{\"name\": \"Viktor\"}\n\n> save token = body.access_token\n> assert status == 200\n";
        let r = parse(src).unwrap();
        assert_eq!(r.method, "POST");
        assert_eq!(r.url, "{{base}}/users");
        assert_eq!(
            r.headers,
            vec![Header {
                name: "Authorization".into(),
                value: "Bearer {{token}}".into()
            }]
        );
        assert_eq!(r.body.as_deref(), Some("{\"name\": \"Viktor\"}"));
        assert_eq!(r.directives.len(), 2);
        assert!(matches!(&r.directives[0], Directive::Save { var, .. } if var == "token"));
        assert!(matches!(&r.directives[1], Directive::Assert(a) if a.op == Op::Eq));
    }

    #[test]
    fn minimal_and_comments() {
        let r = parse("# list users\n// another comment\n\n{{base}}/users HTTP/1.1\n# hidden: header\nAccept: */*\n").unwrap();
        assert_eq!(r.method, "GET");
        assert_eq!(r.headers.len(), 1);
        assert_eq!(r.body, None);
        assert!(r.directives.is_empty());
    }

    #[test]
    fn spaces_inside_braces_stay_in_url() {
        let r = parse("POST {{base}}/x/{{ $randomInt 1 10 }}?a={{$uuid}} HTTP/1.1\n").unwrap();
        assert_eq!(r.method, "POST");
        assert_eq!(r.url, "{{base}}/x/{{ $randomInt 1 10 }}?a={{$uuid}}");
        assert!(parse("GET /x {{a}}\n").is_err());
    }

    #[test]
    fn multiline_body_keeps_inner_blank_lines() {
        let r = parse("PUT /x\n\nline1\n\nline3\n\n\n").unwrap();
        assert_eq!(r.body.as_deref(), Some("line1\n\nline3"));
    }

    #[test]
    fn crlf() {
        let r = parse("GET /x\r\nA: b\r\n\r\nbody\r\n> assert status == 200\r\n").unwrap();
        assert_eq!(r.headers[0].value, "b");
        assert_eq!(r.body.as_deref(), Some("body"));
        assert_eq!(r.directives.len(), 1);
    }

    #[test]
    fn errors_have_lines() {
        let err = |s: &str| match parse(s).unwrap_err() {
            Error::Parse { line, .. } => line,
            e => panic!("{e}"),
        };
        assert_eq!(err(""), 1);
        assert_eq!(err("# only comment\n"), 1);
        assert_eq!(err("GET /x\nbroken header\n"), 2);
        assert_eq!(err("GET /a b\n"), 1);
        assert_eq!(err("GET /x\n\n> save = body\n"), 3);
        assert_eq!(err("GET /x\n\n> assert status\n"), 3);
        assert_eq!(err("GET /x\n\n> frobnicate\n"), 3);
    }
}
