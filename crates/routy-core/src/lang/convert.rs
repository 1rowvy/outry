//! `routy convert`: `*.http` → `*.routy`. Запрос переписывается текстом (JSON-тело остаётся
//! JSON, плейсхолдеры `{{x}}` становятся выражениями), а результат проходит через `fmt` —
//! это и канонический вид, и проверка, что получился разбираемый файл.

use serde_json::Value;

use super::parse::{RESERVED, is_ident_char, is_ident_start};
use crate::error::{Error, Result};
use crate::expr::{Assertion, Op, Path, Root, Segment};
use crate::parser::{Directive, parse};

/// `.http` → текст `.routy`. `stem` — имя файла без расширения: из него имя запроса, если
/// первый комментарий не похож на заголовок.
pub fn convert(src: &str, stem: &str) -> Result<String> {
    let req = parse(src)?;
    let (top, header_comments) = comments(src);

    let mut out = String::new();
    let title_like = top.first().is_some_and(|t| {
        let words = t.split_whitespace().count();
        (1..=4).contains(&words) && !t.ends_with('.') && !t.contains(':')
    });
    if !top.is_empty() && !title_like {
        out.push_str(&format!("// {}\n", stem_title(stem)));
    }
    for line in &top {
        out.push_str(&format!("// {line}\n").replace("//  ", "// "));
    }

    out.push_str(&format!("{} {} {{\n", req.method, target(&req.url)?));
    if !req.headers.is_empty() || !header_comments.is_empty() {
        out.push_str("headers {\n");
        for c in &header_comments {
            out.push_str(&format!("// {c}\n"));
        }
        for h in &req.headers {
            let bare = h
                .name
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'));
            let key = if bare { h.name.clone() } else { quote(&h.name) };
            out.push_str(&format!("{key}: {}\n", template(&h.value)?));
        }
        out.push_str("}\n");
    }
    if let Some(body) = &req.body {
        out.push_str(&format!("body {}\n", body_expr(body)?));
    }

    let mut checks = Vec::new();
    let mut saves = Vec::new();
    for d in &req.directives {
        match d {
            Directive::Assert(a) => checks.push(assertion(a)),
            Directive::Save { var, path } => {
                if !is_name(var) {
                    return Err(Error::Run(format!(
                        "`> save {var}`: variable names in .routy are identifiers, rename it first"
                    )));
                }
                saves.push(format!("save {var} = {}", path_expr(path)));
            }
        }
    }
    if !checks.is_empty() {
        out.push_str("expect {\n");
        for c in checks {
            out.push_str(&c);
            out.push('\n');
        }
        out.push_str("}\n");
    }
    for s in saves {
        out.push_str(&s);
        out.push('\n');
    }
    out.push_str("}\n");

    super::fmt::format(&out)
        .map_err(|e| Error::Run(format!("generated .routy does not parse ({e}):\n{out}")))
}

/// Комментарии над строкой запроса и среди заголовков, без `#` / `//`.
fn comments(src: &str) -> (Vec<String>, Vec<String>) {
    let strip = |l: &str| {
        let t = l.trim();
        t.strip_prefix("//")
            .or_else(|| t.strip_prefix('#'))
            .map(|c| c.trim().to_string())
    };
    let mut lines = src.lines();
    let mut top = Vec::new();
    for l in lines.by_ref() {
        if l.trim().is_empty() {
            continue;
        }
        match strip(l) {
            Some(c) => top.push(c),
            None => break, // строка запроса
        }
    }
    let mut headers = Vec::new();
    for l in lines {
        if l.trim().is_empty() {
            break;
        }
        if let Some(c) = strip(l) {
            headers.push(c);
        }
    }
    (top, headers)
}

/// `1-login` → `Login`, `get-user` → `Get user`.
fn stem_title(stem: &str) -> String {
    let words: Vec<&str> = stem
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !w.chars().all(|c| c.is_ascii_digit()))
        .collect();
    let text = if words.is_empty() {
        "Request".to_string()
    } else {
        words.join(" ")
    };
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn is_name(s: &str) -> bool {
    s.starts_with(is_ident_start)
        && s.chars().all(is_ident_char)
        && !RESERVED.contains(&s)
        && !matches!(s, "true" | "false" | "null")
}

fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '$' => out.push_str("\\$"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `{{x}}` → выражение: переменная, `vars["x-y"]` или встроенная функция для `{{$uuid}}` и т.п.
fn var_expr(name: &str) -> Result<String> {
    let mut parts = name.split_whitespace();
    let head = parts.next().unwrap_or_default();
    let args: Vec<&str> = parts.collect();
    Ok(match (head, args.as_slice()) {
        ("$uuid", []) => "uuid()".into(),
        ("$timestamp", []) => "now()".into(),
        // В .http верхняя граница не включалась, в .routy — включается.
        ("$randomInt", []) => "randomInt(0, 999)".into(),
        ("$randomInt", [min, max]) => {
            let max: i64 = max
                .parse()
                .map_err(|_| Error::expr(name, "`$randomInt` bounds must be integers"))?;
            format!("randomInt({min}, {})", max - 1)
        }
        _ if head.starts_with('$') => {
            return Err(Error::expr(name, "unknown dynamic variable"));
        }
        _ if is_name(name) => name.to_string(),
        _ => format!("vars[{}]", quote(name)),
    })
}

/// Куски текста и плейсхолдеры `{{…}}` по порядку.
enum Piece<'a> {
    Text(&'a str),
    Var(&'a str),
}

fn pieces(s: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find("{{") {
        let Some(end) = rest[start + 2..].find("}}") else {
            break;
        };
        if start > 0 {
            out.push(Piece::Text(&rest[..start]));
        }
        out.push(Piece::Var(rest[start + 2..start + 2 + end].trim()));
        rest = &rest[start + 2 + end + 2..];
    }
    if !rest.is_empty() {
        out.push(Piece::Text(rest));
    }
    out
}

/// Строка с плейсхолдерами → `"Bearer ${token}"`; один плейсхолдер целиком → само выражение.
fn template(s: &str) -> Result<String> {
    let ps = pieces(s);
    if let [Piece::Var(v)] = ps.as_slice() {
        return var_expr(v);
    }
    let mut out = String::from("\"");
    for p in ps {
        match p {
            Piece::Text(t) => {
                let q = quote(t);
                out.push_str(&q[1..q.len() - 1]);
            }
            Piece::Var(v) => out.push_str(&format!("${{{}}}", var_expr(v)?)),
        }
    }
    out.push('"');
    Ok(out)
}

/// `{{base}}/users/{{id}}` → `/users/{id}`; полный URL — как есть; остальное — строкой.
fn target(url: &str) -> Result<String> {
    let ps = pieces(url);
    let rest = match ps.split_first() {
        Some((Piece::Var("base"), rest)) if matches!(rest.first(), Some(Piece::Text(t)) if t.starts_with('/')) => {
            rest
        }
        _ if url.starts_with("http://") || url.starts_with("https://") => &ps[..],
        _ => return template(url),
    };
    let mut out = String::new();
    for p in rest {
        match p {
            Piece::Text(t) => {
                if t.contains(['{', '}', '"', ' ']) {
                    return template(url);
                }
                out.push_str(t);
            }
            Piece::Var(v) if is_name(v) => out.push_str(&format!("{{{v}}}")),
            Piece::Var(v) => out.push_str(&format!("${{{}}}", var_expr(v)?)),
        }
    }
    Ok(out)
}

/// JSON-тело — значением (`body { … }`), остальное — строкой.
fn body_expr(body: &str) -> Result<String> {
    if let Some(json) = json_body(body)? {
        return Ok(json);
    }
    let ps = pieces(body);
    let multiline = body.contains('\n');
    if !multiline {
        return template(body);
    }
    let mut out = String::from("\"\"\"\n");
    for p in ps {
        match p {
            Piece::Text(t) => out.push_str(&t.replace('\\', "\\\\").replace("${", "\\${")),
            Piece::Var(v) => out.push_str(&format!("${{{}}}", var_expr(v)?)),
        }
    }
    out.push_str("\n\"\"\"");
    Ok(out)
}

/// Если тело — JSON (плейсхолдеры в строках или вместо значений), то оно же с выражениями.
fn json_body(body: &str) -> Result<Option<String>> {
    // Проверка: плейсхолдеры вне строк заменяем на 0, внутри — оставляем.
    let mut probe = String::new();
    let mut out = String::new();
    let mut in_str = false;
    let mut escaped = false;
    let mut str_start = 0;
    let b = body;
    let mut i = 0;
    while i < b.len() {
        let r = &b[i..];
        if let Some(inner) = r.strip_prefix("{{") {
            if let Some(end) = inner.find("}}") {
                let name = inner[..end].trim();
                let expr = var_expr(name)?;
                let len = 2 + end + 2;
                if in_str {
                    probe.push('x');
                    // `"{{x}}"` целиком — значение переменной без кавычек.
                    let whole = out.len() == str_start + 1 && b[i + len..].starts_with('"');
                    if whole {
                        out.truncate(str_start);
                        out.push_str(&expr);
                        probe.push('"');
                        in_str = false;
                        i += len + 1;
                        continue;
                    }
                    out.push_str(&format!("${{{expr}}}"));
                } else {
                    probe.push('0');
                    if name.starts_with('$') {
                        out.push_str(&expr);
                    } else {
                        out.push_str(&format!("number({expr})"));
                    }
                }
                i += len;
                continue;
            }
        }
        let c = r.chars().next().unwrap_or(' ');
        probe.push(c);
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            } else if c == '$' && r[1..].starts_with('{') {
                out.push('\\');
            }
        } else if c == '"' {
            in_str = true;
            str_start = out.len();
        }
        out.push(c);
        i += c.len_utf8();
    }
    let parsed: std::result::Result<Value, _> = serde_json::from_str(&probe);
    Ok(match parsed {
        Ok(Value::Object(_) | Value::Array(_)) => Some(out.trim().to_string()),
        _ => None,
    })
}

fn path_expr(p: &Path) -> String {
    let mut out = match &p.root {
        Root::Status => "status".to_string(),
        Root::Duration => "duration".to_string(),
        Root::Body => "body".to_string(),
        Root::Header(h) => {
            let member = h.starts_with(is_ident_start)
                && h.chars().all(|c| is_ident_char(c) || c == '-')
                && !h.ends_with('-');
            if member {
                format!("headers.{h}")
            } else {
                format!("headers[{}]", quote(h))
            }
        }
    };
    for s in &p.segments {
        match s {
            Segment::Key(k) if k.starts_with(is_ident_start) && k.chars().all(is_ident_char) => {
                out.push('.');
                out.push_str(k);
            }
            Segment::Key(k) => out.push_str(&format!("[{}]", quote(k))),
            Segment::Index(i) => out.push_str(&format!("[{i}]")),
        }
    }
    out
}

fn literal(v: &Value) -> String {
    match v {
        Value::String(s) => quote(s),
        other => other.to_string(),
    }
}

fn assertion(a: &Assertion) -> String {
    let path = path_expr(&a.path);
    let header = matches!(a.path.root, Root::Header(_));
    let op = match a.op {
        Op::Exists => return format!("{path} != null"),
        Op::Contains => return format!("{path}.contains({})", literal(&a.expected)),
        Op::Eq => "==",
        Op::Ne => "!=",
        Op::Lt => "<",
        Op::Le => "<=",
        Op::Gt => ">",
        Op::Ge => ">=",
    };
    // Заголовки — строки; в .http их сравнивали с числами как числа.
    let lhs = if header && a.expected.is_number() {
        format!("number({path})")
    } else {
        path
    };
    format!("{lhs} {op} {}", literal(&a.expected))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_requests() {
        let src = r#"# Login: httpbin echoes the request body back.
POST {{base}}/anything/login/{{id}}?x={{ q }}
Authorization: Bearer {{token}}
# a header comment
X-Id: {{$uuid}}
X-Trace: {{trace-id}}

{"user": "{{user}}", "n": {{count}}, "s": "a ${b} {{$timestamp}}", "nested": [1, {"k": null}]}

> save token = body.json.access_token
> assert status == 200
> assert body.token == demo-token
> assert headers.content-type contains json
> assert headers.content-length < 1000
> assert body.items[0]["a b"] exists
"#;
        let want = r#"// Login: httpbin echoes the request body back.
Login: POST /anything/login/{id}?x={q} {
  headers {
    // a header comment
    Authorization: "Bearer ${token}"
    X-Id: uuid()
    X-Trace: vars["trace-id"]
  }

  body {
    user,
    n: number(count),
    s: "a \${b} ${now()}",
    nested: [1, { k: null }],
  }

  expect {
    status == 200
    body.token == "demo-token"
    headers.content-type.contains("json")
    number(headers.content-length) < 1000
    body.items[0]["a b"] != null
  }

  save token = body.json.access_token
}
"#;
        assert_eq!(convert(src, "1-login").unwrap(), want);
    }

    #[test]
    fn short_comment_is_the_name_and_text_bodies() {
        let src = "# Get user\nGET https://example.com/users/{{id}}\n\nline one\n{{name}} here\n";
        let want = "GetUser: GET https://example.com/users/{id} {\n  body \"\"\"\n    line one\n    ${name} here\n  \"\"\"\n}\n";
        assert_eq!(convert(src, "get").unwrap(), want);

        let src = "GET {{auth_url}}/token\n\n> assert status == 200\n";
        let want = "GET \"${auth_url}/token\" {\n  expect { status == 200 }\n}\n";
        assert_eq!(convert(src, "token").unwrap(), want);
    }
}
