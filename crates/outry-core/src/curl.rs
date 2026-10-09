//! Готовый запрос → команда `curl` (POSIX shell) для «Copy as curl».

use crate::runner::ResolvedRequest;

/// Одна команда: метод и адрес в первой строке, дальше по аргументу на строку. Бинарное тело
/// не вставляется: вместо него `--data-binary @body.bin` и комментарий над командой.
pub fn command(req: &ResolvedRequest, body: Option<&[u8]>) -> String {
    let mut head = String::new();
    let method = match req.method.as_str() {
        "GET" => String::new(),
        "HEAD" => "--head ".into(),
        m => format!("-X {} ", quote(m)),
    };
    let mut args = vec![format!("curl {method}{}", quote(&req.url))];
    for h in &req.headers {
        args.push(format!("-H {}", quote(&format!("{}: {}", h.name, h.value))));
    }
    match body.map(std::str::from_utf8) {
        None => {}
        Some(Ok(text)) => args.push(format!("--data-raw {}", quote(text))),
        Some(Err(_)) => {
            let len = body.map_or(0, <[u8]>::len);
            head = format!("# the body is binary ({len} bytes): save it as body.bin\n");
            args.push("--data-binary @body.bin".into());
        }
    }
    head + &args.join(" \\\n  ")
}

/// Аргумент в одинарных кавычках; простые слова — как есть.
fn quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '@' | ',' | '=')
        });
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Header;

    fn req(method: &str, headers: &[(&str, &str)]) -> ResolvedRequest {
        ResolvedRequest {
            method: method.into(),
            url: "https://api.example.com/users?q=a b&x=1".into(),
            headers: headers
                .iter()
                .map(|(n, v)| Header {
                    name: n.to_string(),
                    value: v.to_string(),
                })
                .collect(),
            body: None,
        }
    }

    #[test]
    fn get_post_and_quoting() {
        assert_eq!(
            command(&req("GET", &[]), None),
            "curl 'https://api.example.com/users?q=a b&x=1'"
        );
        let r = req("POST", &[("Content-Type", "application/json")]);
        assert_eq!(
            command(&r, Some(br#"{"name":"O'Brien"}"#)),
            "curl -X POST 'https://api.example.com/users?q=a b&x=1' \\\n  \
             -H 'Content-Type: application/json' \\\n  \
             --data-raw '{\"name\":\"O'\\''Brien\"}'"
        );
        assert!(command(&req("HEAD", &[]), None).starts_with("curl --head "));
    }

    #[test]
    fn binary_body_is_not_inlined() {
        let out = command(&req("PUT", &[]), Some(&[0xff, 0xfe, 0x00]));
        assert!(out.starts_with("# the body is binary (3 bytes)"), "{out}");
        assert!(out.ends_with("--data-binary @body.bin"), "{out}");
    }
}
