//! Shape ответов: `shape Order { … }` проекта против структур Go из ответов обработчиков.
//!
//! Shape пользователя может быть строже кода — литералы вместо `string`, `integer` вместо
//! `number`, поле без `null` — это не расхождение. Расхождение — поле, которого в коде нет,
//! или тип, который коду противоречит. Правка переписывает shape по коду, оставляя совместимые
//! поля пользователя как есть.

use std::path::Path;

use super::ShapeDef;
use super::diff::{ChangeKind, Edit, GoRef, Sink};
use crate::lang::ast::{Item, Shape, ShapeDecl, ShapeField};

/// Shape из кода как дерево: `{ id: integer }` → [`Shape::Object`].
pub(super) fn parse_def(def: &ShapeDef) -> Option<Shape> {
    let file =
        crate::lang::parse::parse(&format!("shape {} {}", def.name, def.shape), None).ok()?;
    file.items.into_iter().find_map(|i| match i {
        Item::Shape(d) => Some(d.shape),
        _ => None,
    })
}

/// Расхождения объявления `decl` (в файле `file`, текст `src`) с типом Go.
pub(super) fn compare(
    def: &ShapeDef,
    decl: &ShapeDecl,
    src: &str,
    file: &Path,
) -> Vec<super::Change> {
    let mut sink = Sink {
        file,
        src,
        go: GoRef {
            file: def.source.clone(),
            line: def.line,
        },
        out: Vec::new(),
    };
    let (Some(Shape::Object(code)), Shape::Object(mine)) = (parse_def(def), &decl.shape) else {
        return sink.out;
    };
    let mut kinds = Vec::new();
    for g in &code {
        match mine.iter().find(|u| u.name == g.name) {
            None => kinds.push(ChangeKind::ShapeMissingField {
                shape: def.name.clone(),
                field: g.name.clone(),
                ty: text(&g.shape),
            }),
            Some(u) if !compatible(&u.shape, &g.shape) => kinds.push(ChangeKind::ShapeFieldType {
                shape: def.name.clone(),
                field: g.name.clone(),
                was: text(&u.shape),
                now: text(&g.shape),
            }),
            Some(_) => {}
        }
    }
    for u in mine
        .iter()
        .filter(|u| !code.iter().any(|g| g.name == u.name))
    {
        kinds.push(ChangeKind::ShapeUnknownField {
            shape: def.name.clone(),
            field: u.name.clone(),
        });
    }
    if kinds.is_empty() {
        return sink.out;
    }
    // Одна правка на всё объявление: поля по коду, совместимые — как у пользователя.
    let merged: Vec<ShapeField> = code
        .iter()
        .map(|g| match mine.iter().find(|u| u.name == g.name) {
            Some(u) if compatible(&u.shape, &g.shape) => u.clone(),
            _ => g.clone(),
        })
        .collect();
    let fix = Edit {
        start: decl.span.start,
        end: decl.span.end,
        text: format!("shape {} {}", decl.name, text(&Shape::Object(merged))),
        block: false,
    };
    for kind in kinds {
        sink.push(decl.span.start, kind, Some(fix.clone()));
    }
    sink.out
}

/// Значения, подходящие к `mine`, подходят и к `code` — с поправкой на неизвестное:
/// `any`, `schema()`, именованные shape против объектов не сравниваются.
fn compatible(mine: &Shape, code: &Shape) -> bool {
    use Shape::*;
    match (mine, code) {
        (Any | Schema(_), _) | (_, Any | Schema(_)) => true,
        (Union(alts), _) => alts.iter().all(|a| compatible(a, code)),
        (_, Union(alts)) => alts.iter().any(|a| compatible(mine, a)),
        (String, String) | (Boolean, Boolean) | (Null, Null) => true,
        (Number | Integer, Number | Integer) => true,
        (Literal(v), _) => match code {
            String => v.is_string(),
            Number => v.is_number(),
            Integer => v.as_f64().is_some_and(|n| n.fract() == 0.0),
            Boolean => v.is_boolean(),
            Null => v.is_null(),
            Literal(c) => v == c,
            Named(..) | Object(_) => false,
            _ => true,
        },
        (Array(a), Array(b)) => compatible(a, b),
        (Object(a), Object(b)) => a.iter().all(|u| {
            b.iter()
                .find(|g| g.name == u.name)
                .is_some_and(|g| compatible(&u.shape, &g.shape))
        }),
        (Named(a, _), Named(b, _)) => a == b,
        (Named(..), Object(_)) | (Object(_), Named(..)) => true,
        _ => false,
    }
}

/// Shape одной строкой, как в `.outry`.
pub(super) fn text(s: &Shape) -> String {
    match s {
        Shape::Any => "any".into(),
        Shape::String => "string".into(),
        Shape::Number => "number".into(),
        Shape::Integer => "integer".into(),
        Shape::Boolean => "boolean".into(),
        Shape::Null => "null".into(),
        Shape::Literal(v) => v.to_string(),
        Shape::Union(alts) => alts.iter().map(text).collect::<Vec<_>>().join(" | "),
        Shape::Array(inner) => format!("[{}]", text(inner)),
        Shape::Object(fields) if fields.is_empty() => "{}".into(),
        Shape::Object(fields) => format!(
            "{{ {} }}",
            fields
                .iter()
                .map(|f| {
                    let name = if f
                        .name
                        .starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                        && f.name
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
                        f.name.clone()
                    } else {
                        serde_json::to_string(&f.name).unwrap_or_default()
                    };
                    format!(
                        "{name}{}: {}",
                        if f.optional { "?" } else { "" },
                        text(&f.shape)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Shape::Named(n, _) => n.clone(),
        Shape::Schema(p) => format!("schema({})", serde_json::to_string(p).unwrap_or_default()),
    }
}

/// Текст новых объявлений для `shapes.outry`: комментарий с типом Go и `shape`.
pub(super) fn declarations(defs: &[ShapeDef]) -> String {
    let mut out = String::new();
    for d in defs {
        let source = d.source.to_string_lossy().replace('\\', "/");
        out.push_str(&format!(
            "\n// {} — {source}:{}\nshape {} {}\n",
            d.go_type, d.line, d.name, d.shape
        ));
    }
    out
}
