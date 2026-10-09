//! `matches schema("./order.schema.json")`: проверка по JSON Schema. Поддержано то, что встречается
//! в схемах API (draft 7 / 2020-12, OpenAPI 3.0 `nullable`): `type`, `enum`, `const`, `properties`,
//! `required`, `additionalProperties`, `items`, `prefixItems`, длины, границы чисел, `pattern`,
//! `allOf` / `anyOf` / `oneOf` / `not` и `$ref` внутри файла. Остальные ключи не проверяются.

use serde_json::{Map, Value};

use super::eval::{deep_eq, show, type_name};

/// Первое несовпадение: `None` — значение подходит. `Err` — сама схема некорректна.
pub fn mismatch(schema: &Value, v: &Value, path: &str) -> Result<Option<String>, String> {
    Check { root: schema }.check(schema, v, path, 0)
}

struct Check<'a> {
    root: &'a Value,
}

fn at(p: &str) -> String {
    if p.is_empty() {
        String::new()
    } else {
        format!("{p}: ")
    }
}

fn is_type(v: &Value, t: &str) -> bool {
    match t {
        "integer" => v.as_f64().is_some_and(|n| n.fract() == 0.0),
        "number" => v.is_number(),
        t => type_name(v) == t,
    }
}

impl<'a> Check<'a> {
    fn resolve(&self, r: &str) -> Result<&'a Value, String> {
        let Some(pointer) = r.strip_prefix('#') else {
            return Err(format!(
                "$ref `{r}`: only references inside the file (`#/…`) are supported"
            ));
        };
        self.root
            .pointer(pointer)
            .ok_or_else(|| format!("$ref `{r}` not found"))
    }

    fn check(
        &self,
        s: &'a Value,
        v: &Value,
        path: &str,
        depth: usize,
    ) -> Result<Option<String>, String> {
        if depth > 64 {
            return Err(format!("{}schema nesting is too deep", at(path)));
        }
        let s = match s {
            Value::Bool(true) => return Ok(None),
            Value::Bool(false) => return Ok(Some(format!("{}no value is allowed", at(path)))),
            Value::Object(s) => s,
            _ => {
                return Err(format!(
                    "{}a schema must be an object or a boolean",
                    at(path)
                ));
            }
        };
        if let Some(Value::String(r)) = s.get("$ref") {
            let target = self.resolve(r)?;
            if let Some(m) = self.check(target, v, path, depth + 1)? {
                return Ok(Some(m));
            }
        }

        if v.is_null() && s.get("nullable") == Some(&Value::Bool(true)) {
            return Ok(None);
        }
        if let Some(t) = s.get("type") {
            let types: Vec<&str> = match t {
                Value::String(t) => vec![t.as_str()],
                Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
                _ => vec![],
            };
            if !types.is_empty() && !types.iter().any(|t| is_type(v, t)) {
                return Ok(Some(format!(
                    "{}expected {}, got {}",
                    at(path),
                    types.join(" | "),
                    show(v)
                )));
            }
        }
        if let Some(c) = s.get("const") {
            if !deep_eq(v, c) {
                return Ok(Some(format!(
                    "{}expected {}, got {}",
                    at(path),
                    show(c),
                    show(v)
                )));
            }
        }
        if let Some(Value::Array(options)) = s.get("enum") {
            if !options.iter().any(|o| deep_eq(v, o)) {
                let list: Vec<String> = options.iter().map(show).collect();
                return Ok(Some(format!(
                    "{}expected one of {}, got {}",
                    at(path),
                    list.join(", "),
                    show(v)
                )));
            }
        }

        match v {
            Value::Object(map) => {
                if let Some(m) = self.object(s, map, path, depth)? {
                    return Ok(Some(m));
                }
            }
            Value::Array(items) => {
                if let Some(m) = self.array(s, items, path, depth)? {
                    return Ok(Some(m));
                }
            }
            Value::String(text) => {
                let len = text.chars().count() as u64;
                if let Some(min) = s.get("minLength").and_then(Value::as_u64) {
                    if len < min {
                        return Ok(Some(format!(
                            "{}expected at least {min} characters, got {}",
                            at(path),
                            show(v)
                        )));
                    }
                }
                if let Some(max) = s.get("maxLength").and_then(Value::as_u64) {
                    if len > max {
                        return Ok(Some(format!(
                            "{}expected at most {max} characters, got {len}",
                            at(path)
                        )));
                    }
                }
                if let Some(Value::String(p)) = s.get("pattern") {
                    let re = regex::Regex::new(p)
                        .map_err(|e| format!("{}pattern /{p}/: {e}", at(path)))?;
                    if !re.is_match(text) {
                        return Ok(Some(format!(
                            "{}{} does not match /{p}/",
                            at(path),
                            show(v)
                        )));
                    }
                }
            }
            Value::Number(n) => {
                let n = n.as_f64().unwrap_or_default();
                let bound = |key: &str| s.get(key).and_then(Value::as_f64);
                let fails = [
                    ("minimum", bound("minimum").filter(|b| n < *b), ">="),
                    ("maximum", bound("maximum").filter(|b| n > *b), "<="),
                    (
                        "exclusiveMinimum",
                        bound("exclusiveMinimum").filter(|b| n <= *b),
                        ">",
                    ),
                    (
                        "exclusiveMaximum",
                        bound("exclusiveMaximum").filter(|b| n >= *b),
                        "<",
                    ),
                ];
                for (_, b, op) in fails {
                    if let Some(b) = b {
                        return Ok(Some(format!(
                            "{}expected a number {op} {}, got {}",
                            at(path),
                            show(&super::parse::num_value(b)),
                            show(v)
                        )));
                    }
                }
            }
            _ => {}
        }

        if let Some(Value::Array(all)) = s.get("allOf") {
            for sub in all {
                if let Some(m) = self.check(sub, v, path, depth + 1)? {
                    return Ok(Some(m));
                }
            }
        }
        if let Some(Value::Array(any)) = s.get("anyOf") {
            let mut first = None;
            for sub in any {
                match self.check(sub, v, path, depth + 1)? {
                    None => {
                        first = None;
                        break;
                    }
                    Some(m) => {
                        first.get_or_insert(m);
                    }
                }
            }
            if let Some(m) = first {
                return Ok(Some(format!("{}matches none of anyOf ({m})", at(path))));
            }
        }
        if let Some(Value::Array(one)) = s.get("oneOf") {
            let mut matched = 0;
            let mut first = None;
            for sub in one {
                match self.check(sub, v, path, depth + 1)? {
                    None => matched += 1,
                    Some(m) => {
                        first.get_or_insert(m);
                    }
                }
            }
            match matched {
                1 => {}
                0 => {
                    let m = first.unwrap_or_default();
                    return Ok(Some(format!("{}matches none of oneOf ({m})", at(path))));
                }
                n => {
                    return Ok(Some(format!(
                        "{}matches {n} schemas of oneOf, expected one",
                        at(path)
                    )));
                }
            }
        }
        if let Some(not) = s.get("not") {
            if self.check(not, v, path, depth + 1)?.is_none() {
                return Ok(Some(format!("{}matches a schema in `not`", at(path))));
            }
        }
        Ok(None)
    }

    fn object(
        &self,
        s: &'a Map<String, Value>,
        map: &Map<String, Value>,
        path: &str,
        depth: usize,
    ) -> Result<Option<String>, String> {
        let field = |k: &str| {
            if path.is_empty() {
                k.to_string()
            } else {
                format!("{path}.{k}")
            }
        };
        if let Some(Value::Array(required)) = s.get("required") {
            for k in required.iter().filter_map(Value::as_str) {
                if !map.contains_key(k) {
                    return Ok(Some(format!("{}: missing", field(k))));
                }
            }
        }
        let props = s.get("properties").and_then(Value::as_object);
        for (k, fv) in map {
            match props.and_then(|p| p.get(k)) {
                Some(sub) => {
                    if let Some(m) = self.check(sub, fv, &field(k), depth + 1)? {
                        return Ok(Some(m));
                    }
                }
                None => match s.get("additionalProperties") {
                    Some(Value::Bool(false)) => {
                        return Ok(Some(format!("{}: unexpected field", field(k))));
                    }
                    Some(extra @ Value::Object(_)) => {
                        if let Some(m) = self.check(extra, fv, &field(k), depth + 1)? {
                            return Ok(Some(m));
                        }
                    }
                    _ => {}
                },
            }
        }
        Ok(None)
    }

    fn array(
        &self,
        s: &'a Map<String, Value>,
        items: &[Value],
        path: &str,
        depth: usize,
    ) -> Result<Option<String>, String> {
        let len = items.len() as u64;
        if let Some(min) = s.get("minItems").and_then(Value::as_u64) {
            if len < min {
                return Ok(Some(format!(
                    "{}expected at least {min} items, got {len}",
                    at(path)
                )));
            }
        }
        if let Some(max) = s.get("maxItems").and_then(Value::as_u64) {
            if len > max {
                return Ok(Some(format!(
                    "{}expected at most {max} items, got {len}",
                    at(path)
                )));
            }
        }
        // `prefixItems` (2020-12) или `items: [...]` (draft 7) — по позициям, дальше `items`.
        let (prefix, rest): (&[Value], Option<&Value>) =
            match (s.get("prefixItems"), s.get("items")) {
                (Some(Value::Array(p)), rest) => (p, rest),
                (None, Some(Value::Array(p))) => (p, s.get("additionalItems")),
                (None, rest) => (&[], rest),
                (Some(_), rest) => (&[], rest),
            };
        for (i, item) in items.iter().enumerate() {
            let sub = prefix.get(i).or(rest);
            if let Some(sub) = sub {
                if let Some(m) = self.check(sub, item, &format!("{path}[{i}]"), depth + 1)? {
                    return Ok(Some(m));
                }
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn check(schema: Value, v: Value) -> Option<String> {
        mismatch(&schema, &v, "body").unwrap()
    }

    #[test]
    fn keywords() {
        let order = json!({
            "type": "object",
            "required": ["id", "items"],
            "properties": {
                "id": { "type": "string", "pattern": "^o-" },
                "total": { "type": "number", "minimum": 0 },
                "status": { "enum": ["new", "paid"] },
                "note": { "type": "string", "nullable": true },
                "items": { "type": "array", "minItems": 1, "items": { "$ref": "#/$defs/item" } }
            },
            "$defs": {
                "item": {
                    "type": "object",
                    "required": ["sku"],
                    "additionalProperties": false,
                    "properties": { "sku": { "type": "string" }, "qty": { "type": "integer" } }
                }
            }
        });
        let ok = json!({ "id": "o-1", "total": 5, "status": "new", "note": null, "items": [{ "sku": "A", "qty": 2 }] });
        assert_eq!(check(order.clone(), ok), None);

        let cases = [
            (json!({ "items": [] }), "body.id: missing"),
            (
                json!({ "id": "x", "items": [] }),
                "body.id: \"x\" does not match /^o-/",
            ),
            (
                json!({ "id": "o-", "items": [] }),
                "body.items: expected at least 1 items, got 0",
            ),
            (
                json!({ "id": "o-", "items": [{ "sku": "A", "qty": 1.5 }] }),
                "body.items[0].qty: expected integer, got 1.5",
            ),
            (
                json!({ "id": "o-", "items": [{ "sku": "A", "x": 1 }] }),
                "body.items[0].x: unexpected field",
            ),
            (
                json!({ "id": "o-", "status": "old", "items": [{ "sku": "A" }] }),
                "body.status: expected one of \"new\", \"paid\", got \"old\"",
            ),
            (
                json!({ "id": "o-", "total": -1, "items": [{ "sku": "A" }] }),
                "body.total: expected a number >= 0, got -1",
            ),
        ];
        for (v, want) in cases {
            assert_eq!(check(order.clone(), v).as_deref(), Some(want));
        }

        let one = json!({ "oneOf": [{ "type": "string" }, { "type": "integer" }] });
        assert_eq!(check(one.clone(), json!(1)), None);
        assert!(check(one, json!(true)).unwrap().contains("none of oneOf"));
        assert!(mismatch(&json!({ "$ref": "other.json#/x" }), &json!(1), "").is_err());
    }
}
