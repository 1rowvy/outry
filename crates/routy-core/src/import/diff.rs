//! Сравнение роута из кода с запросом проекта: что поменялось в Go и что поправить в запросе.
//!
//! Путь и метод сравниваются, когда запрос найден по `handler:` (иначе они совпали по определению).
//! Тело — только литерал-объект `body { … }` (в `.http` — валидный JSON): поля сверяются со
//! структурой по именам из тегов `json` (вложенные — `user.email`, `items[].id`), типы — только у
//! литералов. Запрос, который ждёт `status` 4xx, — негативный тест: тело, query и заголовки в нём
//! неправильные нарочно.
//!
//! У расхождения в `.routy`, которое правится без потери смысла, есть [`Edit`] — правка текста
//! файла (`--fix`, «Apply» в приложении). Параметры пути не переименовываются: на них ссылаются
//! вызовы и переменные.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{ANY, AuthHeader, Field, JsonType, Route, header_entry, key};
use crate::lang::ast::{BinOp, Body, Expr, ExprKind, Request, Span, StrPart, TargetPart};
use crate::lang::parse::line_col;

/// Расхождение запроса с кодом. Место — в файле запроса (относительно корня проекта).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    /// Ключ для выбора правки: файл, место и текст
    pub id: String,
    pub file: PathBuf,
    /// 1-based
    pub line: usize,
    pub col: usize,
    pub severity: Severity,
    #[serde(flatten)]
    pub kind: ChangeKind,
    /// `kind` текстом: `path changed in code: /a → /b`
    pub message: String,
    /// Место в Go-коде (относительно каталога сканирования)
    pub go: GoRef,
    /// Правка, если расхождение исправляется автоматически
    #[serde(skip)]
    pub fix: Option<Edit>,
    pub fixable: bool,
    /// Diff этой правки (только для исправимых)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoRef {
    pub file: PathBuf,
    pub line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Запрос не сходится с кодом: `--check` падает
    Error,
    /// Стоит посмотреть, но запрос рабочий
    Warning,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        })
    }
}

/// Замена `start..end` (байты) на `text`. `block` — `text` это поле запроса, а блока `{ … }`
/// у запроса нет: соседние такие вставки оборачиваются в один блок.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub block: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChangeKind {
    /// Метод в коде другой (запрос найден по `handler:`).
    Method { was: String, now: String },
    /// Путь в коде другой: `/users/{id}` → `/v2/users/{id}`.
    Path { was: String, now: String },
    /// Тот же путь, но параметр называется иначе: `{id}` → `{user_id}`.
    PathParam { was: String, now: String },
    /// Обработчик читает тело, а запрос его не шлёт.
    NoBody { type_name: String },
    /// Обязательного поля нет в `body`.
    MissingField { field: String, ty: String },
    /// Поля из `body` нет в структуре.
    UnknownField { field: String, type_name: String },
    /// Литерал в `body` не того типа: `"25"` для `int`.
    FieldType {
        field: String,
        ty: String,
        sent: JsonType,
    },
    /// Обработчик читает query-параметр, которого запрос не шлёт.
    NewQuery { name: String, required: bool },
    /// Обработчик читает заголовок, которого запрос не шлёт.
    NewHeader { name: String },
    /// Роут за middleware из `[import.middleware]`, а запрос не шлёт его заголовок.
    MiddlewareHeader { middleware: String, name: String },
    /// Обработчик отвечает `shape`, а запрос это не проверяет.
    NoMatches { shape: String },
    /// В `shape` нет поля из структуры Go.
    ShapeMissingField {
        shape: String,
        field: String,
        ty: String,
    },
    /// Поля из `shape` нет в структуре Go.
    ShapeUnknownField { shape: String, field: String },
    /// Тип поля `shape` не подходит к типу в Go.
    ShapeFieldType {
        shape: String,
        field: String,
        was: String,
        now: String,
    },
}

impl ChangeKind {
    pub fn severity(&self) -> Severity {
        match self {
            ChangeKind::PathParam { .. }
            | ChangeKind::NewQuery {
                required: false, ..
            }
            | ChangeKind::NewHeader { .. }
            | ChangeKind::MiddlewareHeader { .. }
            | ChangeKind::NoMatches { .. }
            | ChangeKind::ShapeMissingField { .. } => Severity::Warning,
            _ => Severity::Error,
        }
    }
}

impl fmt::Display for ChangeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChangeKind::Method { was, now } => write!(f, "method changed in code: {was} → {now}"),
            ChangeKind::Path { was, now } => write!(f, "path changed in code: {was} → {now}"),
            ChangeKind::PathParam { was, now } => {
                write!(f, "path parameter renamed in code: {{{was}}} → {{{now}}}")
            }
            ChangeKind::NoBody { type_name } => {
                write!(
                    f,
                    "handler reads a {type_name} body, the request sends none"
                )
            }
            ChangeKind::MissingField { field, ty } => {
                write!(f, "required field `{field}` ({ty}) is missing from body")
            }
            ChangeKind::UnknownField { field, type_name } => {
                write!(f, "body field `{field}` is not in {type_name}")
            }
            ChangeKind::FieldType { field, ty, sent } => {
                write!(f, "body field `{field}` is a {sent}, code expects {ty}")
            }
            ChangeKind::NewQuery { name, required } => write!(
                f,
                "handler reads {}query parameter `{name}`, the request doesn't send it",
                if *required { "required " } else { "" }
            ),
            ChangeKind::NewHeader { name } => write!(
                f,
                "handler reads header `{name}`, the request doesn't send it"
            ),
            ChangeKind::MiddlewareHeader { middleware, name } => write!(
                f,
                "route is behind `{middleware}`, the request doesn't send header `{name}`"
            ),
            ChangeKind::NoMatches { shape } => write!(
                f,
                "handler responds with {shape}, the request doesn't check `body matches {shape}`"
            ),
            ChangeKind::ShapeMissingField { shape, field, ty } => {
                write!(f, "shape {shape} has no field `{field}` ({ty}) from code")
            }
            ChangeKind::ShapeUnknownField { shape, field } => {
                write!(f, "shape {shape} field `{field}` is not in code")
            }
            ChangeKind::ShapeFieldType {
                shape,
                field,
                was,
                now,
            } => write!(f, "shape {shape} field `{field}` is {was}, code has {now}"),
        }
    }
}

impl fmt::Display for JsonType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            JsonType::String => "string",
            JsonType::Number => "number",
            JsonType::Boolean => "boolean",
            JsonType::Array => "array",
            JsonType::Object => "object",
        })
    }
}

/// Собирает [`Change`] с местом в файле `file` (текст `src`) и ссылкой на Go.
pub(super) struct Sink<'a> {
    pub file: &'a Path,
    pub src: &'a str,
    pub go: GoRef,
    pub out: Vec<Change>,
}

impl Sink<'_> {
    pub fn push(&mut self, at: usize, kind: ChangeKind, fix: Option<Edit>) {
        let (line, col) = line_col(self.src, at);
        let message = kind.to_string();
        self.out.push(Change {
            id: format!("{}:{line}:{col}:{message}", self.file.display()),
            file: self.file.to_path_buf(),
            line,
            col,
            severity: kind.severity(),
            fixable: fix.is_some(),
            fix,
            diff: None,
            kind,
            message,
            go: self.go.clone(),
        });
    }
}

/// Расхождения запроса `req` (из файла `file` с текстом `src`) с роутом.
pub fn compare(
    route: &Route,
    req: &Request,
    src: &str,
    file: &Path,
    auth: &[AuthHeader],
) -> Vec<Change> {
    let mut sink = Sink {
        file,
        src,
        go: GoRef {
            file: route.source.clone(),
            line: route.line,
        },
        out: Vec::new(),
    };
    let target = req.target.span;
    let f = &req.fields;

    if route.method != ANY && req.method != route.method {
        let end = src[..target.start].trim_end().len();
        let start = end.saturating_sub(req.method.len());
        sink.push(
            start,
            ChangeKind::Method {
                was: req.method.clone(),
                now: route.method.clone(),
            },
            (src.get(start..end) == Some(req.method.as_str())).then(|| Edit {
                start,
                end,
                text: route.method.clone(),
                block: false,
            }),
        );
    }
    let (path, params) = request_path(req, src);
    let now = route.path.replace("{{", "{").replace("}}", "}");
    let code_params = route_params(&route.path);
    if key(&path.replace('{', "{{")) != key(&route.path) {
        // Новый путь с именами параметров запроса (по порядку): на них ссылаются вызовы.
        let fix = params.iter().all(Option::is_some).then(|| {
            let mut text = route.path.clone();
            for (i, code) in code_params.iter().enumerate() {
                let name = params.get(i).cloned().flatten().unwrap_or(code.clone());
                text = text.replacen(&format!("{{{{{code}}}}}"), &format!("{{{name}}}"), 1);
            }
            let raw = target.text(src);
            let len = raw.find(['?', '#']).unwrap_or(raw.len());
            Edit {
                start: target.start,
                end: target.start + len,
                text,
                block: false,
            }
        });
        sink.push(target.start, ChangeKind::Path { was: path, now }, fix);
    } else {
        for (was, now) in params.iter().zip(&code_params) {
            if let Some(was) = was
                && was != now
            {
                sink.push(
                    target.start,
                    ChangeKind::PathParam {
                        was: was.clone(),
                        now: now.clone(),
                    },
                    None,
                );
            }
        }
    }

    // Негативный тест: запрос неправильный нарочно — сверяем только, куда он идёт.
    if negative(req) {
        return sink.out;
    }
    let block = RequestBlock::new(req, src);
    let info = &route.info;
    if let Some(body) = &info.body {
        match &f.body {
            None => sink.push(
                req.span.start,
                ChangeKind::NoBody {
                    type_name: body.type_name.clone(),
                },
                Some(block.insert(src, &format!("body {}", body.example))),
            ),
            Some(Body::Value(Expr {
                kind: ExprKind::Object(kv),
                span,
            })) => {
                let mut cmp = BodyCmp {
                    fields: &body.fields,
                    type_name: &body.type_name,
                    example: Example::parse(&body.example),
                    src,
                    out: Vec::new(),
                };
                cmp.object(kv, "", *span);
                for (at, kind, fix) in cmp.out {
                    sink.push(at, kind, fix);
                }
            }
            // Переменная, файл, строка, форма — не сравниваем.
            Some(_) => {}
        }
    }

    let field_at = |name: &str| {
        f.order
            .iter()
            .find(|(k, _)| *k == name)
            .map_or(req.span.start, |(_, s)| s.start)
    };
    let sent: HashSet<String> = f
        .query
        .iter()
        .map(|e| e.key.clone())
        .chain(target_query(req))
        .collect();
    for q in &info.query {
        if !sent.contains(&q.name) && !code_params.contains(&q.name) {
            sink.push(
                field_at("query"),
                ChangeKind::NewQuery {
                    name: q.name.clone(),
                    required: q.required,
                },
                None,
            );
        }
    }
    for h in &info.headers {
        if !f.headers.iter().any(|e| e.key.eq_ignore_ascii_case(h)) {
            sink.push(
                field_at("headers"),
                ChangeKind::NewHeader { name: h.clone() },
                None,
            );
        }
    }
    for h in auth {
        if f.headers
            .iter()
            .any(|e| e.key.eq_ignore_ascii_case(&h.name))
        {
            continue;
        }
        let entry = header_entry(h);
        let fix = match f.order.iter().find(|(k, _)| *k == "headers") {
            Some((_, span)) => braces(src, *span).map(|(open, close)| {
                insert_entry(
                    src,
                    open,
                    close,
                    f.headers.last().map(|e| e.span.end),
                    &entry,
                )
            }),
            None => Some(block.insert(src, &format!("headers {{ {entry} }}"))),
        };
        sink.push(
            field_at("headers"),
            ChangeKind::MiddlewareHeader {
                middleware: h.middleware.clone(),
                name: h.name.clone(),
            },
            fix,
        );
    }
    if let Some(resp) = &info.response
        && !f.expect.iter().any(has_matches)
    {
        let check = format!("body matches {}", resp.shape);
        let fix = match f.order.iter().find(|(k, _)| *k == "expect") {
            Some((_, span)) => braces(src, *span).map(|(open, close)| {
                insert_entry(
                    src,
                    open,
                    close,
                    f.expect.last().map(|e| e.span.end),
                    &check,
                )
            }),
            None => Some(block.insert(src, &format!("expect {{ {check} }}"))),
        };
        sink.push(
            field_at("expect"),
            ChangeKind::NoMatches {
                shape: resp.shape.clone(),
            },
            fix,
        );
    }
    sink.out
}

/// Сравнение запроса `.http`: только тело (если это JSON), query и заголовки, без правок.
/// Метод и путь — то, по чему он найден.
pub fn compare_http(
    route: &Route,
    req: &crate::RequestFile,
    src: &str,
    file: &Path,
    auth: &[AuthHeader],
) -> Vec<Change> {
    let mut sink = Sink {
        file,
        src,
        go: GoRef {
            file: route.source.clone(),
            line: route.line,
        },
        out: Vec::new(),
    };
    let negative = req.directives.iter().any(|d| match d {
        crate::Directive::Assert(a) => {
            a.source.trim_start().starts_with("status")
                && a.expected
                    .as_f64()
                    .is_some_and(|n| (400.0..500.0).contains(&n))
        }
        _ => false,
    });
    if negative {
        return sink.out;
    }
    let line_at = src.find(&format!("{} ", req.method)).unwrap_or_default();
    let info = &route.info;
    if let Some(body) = &info.body {
        match req.body.as_deref().map(str::trim) {
            None | Some("") => sink.push(
                line_at,
                ChangeKind::NoBody {
                    type_name: body.type_name.clone(),
                },
                None,
            ),
            Some(text) => {
                if let Ok(v @ serde_json::Value::Object(_)) =
                    serde_json::from_str::<serde_json::Value>(text)
                {
                    let at = src.find(text).unwrap_or(line_at);
                    let span = Span::new(at, at + text.len());
                    if let ExprKind::Object(kv) = json_expr(&v, span).kind {
                        let mut cmp = BodyCmp {
                            fields: &body.fields,
                            type_name: &body.type_name,
                            example: None,
                            src,
                            out: Vec::new(),
                        };
                        cmp.object(&kv, "", span);
                        for (at, kind, _) in cmp.out {
                            sink.push(at, kind, None);
                        }
                    }
                }
            }
        }
    }
    let query: Vec<&str> = req
        .url
        .split_once('?')
        .map(|(_, q)| q.split('#').next().unwrap_or_default())
        .unwrap_or_default()
        .split('&')
        .filter_map(|kv| kv.split('=').next())
        .collect();
    let params = route_params(&route.path);
    for q in &info.query {
        if !query.contains(&q.name.as_str()) && !params.contains(&q.name) {
            sink.push(
                line_at,
                ChangeKind::NewQuery {
                    name: q.name.clone(),
                    required: q.required,
                },
                None,
            );
        }
    }
    for h in &info.headers {
        if !req.headers.iter().any(|x| x.name.eq_ignore_ascii_case(h)) {
            sink.push(line_at, ChangeKind::NewHeader { name: h.clone() }, None);
        }
    }
    for h in auth {
        if !req
            .headers
            .iter()
            .any(|x| x.name.eq_ignore_ascii_case(&h.name))
        {
            let kind = ChangeKind::MiddlewareHeader {
                middleware: h.middleware.clone(),
                name: h.name.clone(),
            };
            sink.push(line_at, kind, None);
        }
    }
    sink.out
}

/// JSON-значение как выражение `.routy` (все узлы — на месте тела).
fn json_expr(v: &serde_json::Value, span: Span) -> Expr {
    use serde_json::Value;
    let kind = match v {
        Value::Null => ExprKind::Null,
        Value::Bool(b) => ExprKind::Bool(*b),
        Value::Number(n) => ExprKind::Num(n.as_f64().unwrap_or_default()),
        Value::String(s) => ExprKind::Str(vec![StrPart::Lit(s.clone())]),
        Value::Array(items) => ExprKind::Array(items.iter().map(|i| json_expr(i, span)).collect()),
        Value::Object(m) => ExprKind::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), json_expr(v, span)))
                .collect(),
        ),
    };
    Expr { kind, span }
}

/// Блок `{ … }` запроса: куда вставлять новые поля.
struct RequestBlock {
    /// Позиции `{` и `}`, если блок есть
    braces: Option<(usize, usize)>,
    last_end: Option<usize>,
    /// Конец цели — сюда встаёт новый блок
    target_end: usize,
}

impl RequestBlock {
    fn new(req: &Request, src: &str) -> RequestBlock {
        let target_end = req.target.span.end;
        let braces = braces(src, Span::new(target_end, req.span.end.max(target_end)));
        RequestBlock {
            braces,
            last_end: req.fields.order.iter().map(|(_, s)| s.end).max(),
            target_end,
        }
    }

    fn insert(&self, src: &str, entry: &str) -> Edit {
        match self.braces {
            Some((open, close)) => insert_entry(src, open, close, self.last_end, entry),
            None => Edit {
                start: self.target_end,
                end: self.target_end,
                text: entry.to_string(),
                block: true,
            },
        }
    }
}

/// `{` и последняя `}` внутри `span`.
fn braces(src: &str, span: Span) -> Option<(usize, usize)> {
    let text = src.get(span.start..span.end)?;
    let open = span.start + text.find('{')?;
    let close = span.start + text.rfind('}')?;
    (close > open).then_some((open, close))
}

/// Вставка `entry` в блок `{ … }` со скобками в `open`/`close`: в многострочный — отдельной
/// строкой перед `}`, в однострочный — через `, ` после последнего элемента (`last_end`).
fn insert_entry(
    src: &str,
    open: usize,
    close: usize,
    last_end: Option<usize>,
    entry: &str,
) -> Edit {
    let inner = &src[open + 1..close];
    let at = |pos: usize, text: String| Edit {
        start: pos,
        end: pos,
        text,
        block: false,
    };
    if inner.contains('\n') {
        let line_start = src[..close].rfind('\n').map_or(0, |i| i + 1);
        let before_close = &src[line_start..close];
        if before_close.trim().is_empty() {
            return at(line_start, format!("{before_close}  {entry}\n"));
        }
        let pos = last_end.unwrap_or(close);
        let ls = src[..pos].rfind('\n').map_or(0, |i| i + 1);
        let indent: String = src[ls..]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        return at(pos, format!("\n{indent}{entry}"));
    }
    match last_end {
        Some(pos) if !inner.trim().is_empty() => at(pos, format!(", {entry}")),
        _ => at(open + 1, format!(" {entry},")),
    }
}

/// Удаление элемента `start..end` из блока вместе с разделителем.
fn remove_entry(src: &str, start: usize, end: usize) -> Edit {
    let ls = src[..start].rfind('\n').map_or(0, |i| i + 1);
    let le = src[end..].find('\n').map_or(src.len(), |i| end + i + 1);
    let rest = src[end..le].trim();
    let empty = |start, end| Edit {
        start,
        end,
        text: String::new(),
        block: false,
    };
    if src[ls..start].trim().is_empty() && (rest.is_empty() || rest == ",") {
        return empty(ls, le);
    }
    let after = &src[end..];
    if let Some(t) = after.trim_start_matches([' ', '\t']).strip_prefix(',') {
        let skip = after.len() - t.trim_start_matches([' ', '\t']).len();
        return empty(start, end + skip);
    }
    let before = src[..start].trim_end_matches([' ', '\t']);
    if before.ends_with(',') {
        return empty(before.len() - 1, end);
    }
    empty(start, end)
}

/// Начало ключа `key: value` по значению; для `{ qty }` — само значение.
fn key_start(src: &str, value: Span) -> usize {
    let before = src[..value.start].trim_end();
    let Some(before) = before.strip_suffix(':') else {
        return value.start;
    };
    let before = before.trim_end();
    if let Some(b) = before.strip_suffix('"') {
        return b.rfind('"').unwrap_or(b.len());
    }
    before
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '$'))
        .last()
        .map_or(value.start, |(i, _)| i)
}

/// Путь запроса для показа (`/users/{id}`, без query) и имена его параметров по порядку
/// (`None` — подстановка `${…}`).
fn request_path(req: &Request, src: &str) -> (String, Vec<Option<String>>) {
    let mut path = String::new();
    let mut params = Vec::new();
    for p in &req.target.parts {
        match p {
            TargetPart::Lit(l) => path.push_str(l),
            TargetPart::Param(n, _) => {
                path.push_str(&format!("{{{n}}}"));
                params.push(Some(n.clone()));
            }
            TargetPart::Expr(e) => {
                path.push_str(&format!("${{{}}}", e.span.text(src)));
                params.push(None);
            }
        }
    }
    let path = path
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .to_string();
    (path, params)
}

/// Query-параметры, записанные прямо в пути: `/users?page=1`.
fn target_query(req: &Request) -> Vec<String> {
    let mut q = String::new();
    let mut started = false;
    for p in &req.target.parts {
        match p {
            TargetPart::Lit(l) if !started => {
                if let Some(i) = l.find('?') {
                    started = true;
                    q.push_str(&l[i + 1..]);
                }
            }
            TargetPart::Lit(l) => q.push_str(l),
            _ if started => q.push('x'),
            _ => {}
        }
    }
    q.split('#')
        .next()
        .unwrap_or_default()
        .split('&')
        .filter_map(|kv| kv.split('=').next())
        .filter(|k| !k.is_empty())
        .map(String::from)
        .collect()
}

/// `/users/{{id}}/posts/{{post}}` → `["id", "post"]`.
fn route_params(path: &str) -> Vec<String> {
    path.split("{{")
        .skip(1)
        .filter_map(|s| s.split_once("}}"))
        .map(|(n, _)| n.to_string())
        .collect()
}

/// Проверка `… matches <shape>` где-нибудь в выражении.
fn has_matches(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Matches(_, crate::lang::ast::Pattern::Shape(_)) => true,
        ExprKind::Binary(_, a, b) => has_matches(a) || has_matches(b),
        ExprKind::Unary(_, a) => has_matches(a),
        _ => false,
    }
}

/// Негативный тест: ждёт `status == 4xx`, `status >= 400`, `status in [400, …]`.
fn negative(req: &Request) -> bool {
    fn is_status(e: &Expr) -> bool {
        matches!(&e.kind, ExprKind::Ident(n) if n == "status")
    }
    fn client_error(e: &Expr) -> bool {
        matches!(e.kind, ExprKind::Num(n) if (400.0..500.0).contains(&n))
    }
    fn check(e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Binary(BinOp::And | BinOp::Or, a, b) => check(a) || check(b),
            ExprKind::Binary(BinOp::Eq | BinOp::Ge | BinOp::Gt, a, b) => {
                is_status(a) && client_error(b) || is_status(b) && client_error(a)
            }
            ExprKind::Binary(BinOp::In, a, b) => {
                is_status(a)
                    && matches!(&b.kind, ExprKind::Array(items) if items.iter().any(client_error))
            }
            _ => false,
        }
    }
    req.fields.expect.iter().any(check)
}

/// Пример тела из кода, разобранный как `.routy` (порядок полей — как в структуре).
struct Example {
    src: String,
    body: Expr,
}

impl Example {
    fn parse(example: &str) -> Option<Example> {
        let src = format!("POST / {{\nbody {example}\n}}\n");
        let file = crate::lang::parse::parse(&src, None).ok()?;
        let body = file.items.into_iter().find_map(|i| match i {
            crate::lang::ast::Item::Request(r) => match r.fields.body {
                Some(Body::Value(e)) => Some(e),
                _ => None,
            },
            _ => None,
        })?;
        Some(Example { src, body })
    }

    /// Текст значения по имени поля: `address.city`, `items[].sku`.
    fn sample(&self, name: &str) -> Option<&str> {
        let mut v = &self.body;
        for seg in name.split('.') {
            let (k, arr) = match seg.strip_suffix("[]") {
                Some(k) => (k, true),
                None => (seg, false),
            };
            let ExprKind::Object(kv) = &v.kind else {
                return None;
            };
            v = &kv.iter().find(|(key, _)| key == k)?.1;
            if arr {
                let ExprKind::Array(items) = &v.kind else {
                    return None;
                };
                v = items.first()?;
            }
        }
        Some(v.span.text(&self.src))
    }
}

fn key_text(k: &str) -> String {
    if k.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        k.to_string()
    } else {
        serde_json::to_string(k).unwrap_or_default()
    }
}

struct BodyCmp<'a> {
    fields: &'a [Field],
    type_name: &'a str,
    /// Пример тела из кода — значения для новых полей; без него (`.http`) правок нет
    example: Option<Example>,
    src: &'a str,
    out: Vec<(usize, ChangeKind, Option<Edit>)>,
}

impl BodyCmp<'_> {
    /// Объект на уровне `prefix` (`""`, `user.`, `items[].`); `span` — сам объект.
    fn object(&mut self, kv: &[(String, Expr)], prefix: &str, span: Span) {
        for (k, v) in kv {
            let name = format!("{prefix}{k}");
            if let Some(f) = self.fields.iter().find(|f| f.name == name) {
                if let (Some(want), Some(sent)) = (f.json, sent_type(v))
                    && want != sent
                {
                    let fix = self.retype(v, want, &name);
                    self.out.push((
                        v.span.start,
                        ChangeKind::FieldType {
                            field: name.clone(),
                            ty: f.ty.clone(),
                            sent,
                        },
                        fix,
                    ));
                }
                continue;
            }
            let obj = format!("{name}.");
            let arr = format!("{name}[].");
            if self.has_prefix(&obj) {
                if let ExprKind::Object(inner) = &v.kind {
                    self.object(inner, &obj, v.span);
                }
            } else if self.has_prefix(&arr) {
                if let ExprKind::Array(items) = &v.kind {
                    for item in items {
                        if let ExprKind::Object(inner) = &item.kind {
                            self.object(inner, &arr, item.span);
                        }
                    }
                }
            } else {
                let start = key_start(self.src, v.span);
                let fix = self
                    .example
                    .is_some()
                    .then(|| remove_entry(self.src, start, v.span.end));
                self.out.push((
                    start,
                    ChangeKind::UnknownField {
                        field: name,
                        type_name: self.type_name.to_string(),
                    },
                    fix,
                ));
            }
        }
        // Обязательные поля этого уровня и тех вложенных, чьего родителя нет вовсе.
        for f in self.fields.iter().filter(|f| f.required) {
            let Some(rest) = f.name.strip_prefix(prefix) else {
                continue;
            };
            let first = rest.split(['.', '[']).next().unwrap_or(rest);
            if kv.iter().any(|(k, _)| k == first) {
                continue;
            }
            let fix = self
                .example
                .as_ref()
                .and_then(|ex| ex.sample(&format!("{prefix}{first}")))
                .and_then(|value| {
                    let (open, close) = braces(self.src, span)?;
                    let last = kv.last().map(|(_, v)| v.span.end);
                    let entry = format!("{}: {value}", key_text(first));
                    Some(insert_entry(self.src, open, close, last, &entry))
                });
            self.out.push((
                span.start,
                ChangeKind::MissingField {
                    field: f.name.clone(),
                    ty: f.ty.clone(),
                },
                fix,
            ));
        }
    }

    /// Литерал нужного типа вместо `v`: `"25"` → `25`, `1` → `"1"`, иначе пустое значение.
    /// Строка с подстановками и составные значения не трогаются.
    fn retype(&self, v: &Expr, want: JsonType, name: &str) -> Option<Edit> {
        let plain = match &v.kind {
            ExprKind::Str(parts) => match parts.as_slice() {
                [] => Some(String::new()),
                [StrPart::Lit(s)] => Some(s.clone()),
                _ => return None,
            },
            ExprKind::Num(_) | ExprKind::Bool(_) => None,
            _ => return None,
        };
        let text = match (want, &plain) {
            (JsonType::Number, Some(s)) if s.trim().parse::<f64>().is_ok() => s.trim().to_string(),
            (JsonType::Boolean, Some(s)) if s == "true" || s == "false" => s.clone(),
            (JsonType::String, None) => serde_json::to_string(v.span.text(self.src)).ok()?,
            _ => self.example.as_ref()?.sample(name)?.to_string(),
        };
        Some(Edit {
            start: v.span.start,
            end: v.span.end,
            text,
            block: false,
        })
    }

    fn has_prefix(&self, p: &str) -> bool {
        self.fields.iter().any(|f| f.name.starts_with(p))
    }
}

fn sent_type(e: &Expr) -> Option<JsonType> {
    Some(match e.kind {
        ExprKind::Str(_) => JsonType::String,
        ExprKind::Num(_) => JsonType::Number,
        ExprKind::Bool(_) => JsonType::Boolean,
        ExprKind::Array(_) => JsonType::Array,
        ExprKind::Object(_) => JsonType::Object,
        _ => return None,
    })
}
