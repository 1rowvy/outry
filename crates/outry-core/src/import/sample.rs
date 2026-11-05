//! Пример JSON-значения для тела запроса: общий для Go и Rust.

use super::JsonType;

/// Пример JSON с сохранением порядка полей.
pub(crate) enum Json {
    Null,
    Bool,
    Num,
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub(crate) fn json_type(&self) -> Option<JsonType> {
        Some(match self {
            Json::Null => return None,
            Json::Bool => JsonType::Boolean,
            Json::Num => JsonType::Number,
            Json::Str(_) => JsonType::String,
            Json::Arr(_) => JsonType::Array,
            Json::Obj(_) => JsonType::Object,
        })
    }

    pub(crate) fn pretty(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out
    }

    fn write(&self, out: &mut String, indent: usize) {
        let pad = |n: usize| "  ".repeat(n);
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool => out.push_str("false"),
            Json::Num => out.push('0'),
            Json::Str(s) => out.push_str(&serde_json::to_string(s).unwrap_or_default()),
            Json::Arr(items) if items.is_empty() => out.push_str("[]"),
            Json::Obj(items) if items.is_empty() => out.push_str("{}"),
            Json::Arr(items)
                if items
                    .iter()
                    .all(|v| !matches!(v, Json::Arr(_) | Json::Obj(_))) =>
            {
                out.push('[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    v.write(out, indent);
                }
                out.push(']');
            }
            Json::Arr(items) => {
                out.push_str("[\n");
                for (i, v) in items.iter().enumerate() {
                    out.push_str(&pad(indent + 1));
                    v.write(out, indent + 1);
                    out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
                }
                out.push_str(&pad(indent));
                out.push(']');
            }
            Json::Obj(items) => {
                out.push_str("{\n");
                for (i, (k, v)) in items.iter().enumerate() {
                    out.push_str(&pad(indent + 1));
                    out.push_str(&serde_json::to_string(k).unwrap_or_default());
                    out.push_str(": ");
                    v.write(out, indent + 1);
                    out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
                }
                out.push_str(&pad(indent));
                out.push('}');
            }
        }
    }
}
