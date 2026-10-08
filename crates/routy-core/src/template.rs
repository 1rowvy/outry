//! Подстановка `{{name}}`. Пробелы внутри скобок допустимы: `{{ base }}`.

use crate::error::{Error, Result};

/// Имена всех переменных в строке, в порядке появления.
pub fn placeholders(src: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        out.push(after[..end].trim());
        rest = &after[end + 2..];
    }
    out
}

/// Подставляет значения; если чего-то не хватает — ошибка со списком всех недостающих имён.
pub fn render(src: &str, mut lookup: impl FnMut(&str) -> Result<Option<String>>) -> Result<String> {
    let mut out = String::with_capacity(src.len());
    let mut missing = Vec::new();
    let mut rest = src;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        out.push_str(&rest[..start]);
        let name = after[..end].trim();
        match lookup(name)? {
            Some(v) => out.push_str(&v),
            None => {
                if !missing.iter().any(|m| m == name) {
                    missing.push(name.to_string());
                }
            }
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    if missing.is_empty() {
        Ok(out)
    } else {
        Err(Error::MissingVars(missing))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(name: &str) -> Result<Option<String>> {
        Ok(match name {
            "base" => Some("http://x".into()),
            "id" => Some("7".into()),
            _ => None,
        })
    }

    #[test]
    fn renders() {
        assert_eq!(
            render("{{base}}/users/{{ id }}", lookup).unwrap(),
            "http://x/users/7"
        );
        assert_eq!(render("no vars", lookup).unwrap(), "no vars");
        assert_eq!(
            render("{{base}} {{unclosed", lookup).unwrap(),
            "http://x {{unclosed"
        );
    }

    #[test]
    fn reports_all_missing() {
        match render("{{a}} {{base}} {{b}} {{a}}", lookup) {
            Err(Error::MissingVars(v)) => assert_eq!(v, ["a", "b"]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn lists_placeholders() {
        assert_eq!(placeholders("{{a}}/{{ b }}/{{"), ["a", "b"]);
    }
}
