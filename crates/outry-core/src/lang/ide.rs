//! Подсказки редактора поверх `Workspace`: автодополнение, hover, символ под курсором
//! (для перехода к определению). Позиции — байтовые смещения в тексте файла; строки и
//! столбцы считает клиент (`outry lsp`).
//!
//! Автодополнение разбирает текст перед курсором лексически: пока имя дописывается, файл
//! почти никогда не разбирается целиком. Hover и определение работают по дереву.

use std::path::Path;

use super::ast::*;
use super::scope::{self, Binding};
use super::{ItemRef, Workspace, collect_named, visit_item};
use crate::vars::{Source, VarInfo, mask};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Keyword,
    /// Поле запроса: `headers`, `expect`, …
    Field,
    Method,
    Property,
    Function,
    Request,
    Flow,
    Variable,
    Param,
    Shape,
    Type,
    Constant,
    /// Окружение в `only: [...]`, заголовок в `headers`.
    Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub label: String,
    pub kind: Kind,
    pub detail: Option<String>,
    pub doc: Option<String>,
    /// Вставка в синтаксисе сниппетов LSP (`Login(email: $1)`), если она не равна `label`.
    pub snippet: Option<String>,
}

impl Completion {
    fn new(label: impl Into<String>, kind: Kind) -> Completion {
        Completion {
            label: label.into(),
            kind,
            detail: None,
            doc: None,
            snippet: None,
        }
    }

    fn detail(mut self, d: impl Into<String>) -> Completion {
        self.detail = Some(d.into());
        self
    }

    fn doc(mut self, d: impl Into<String>) -> Completion {
        self.doc = Some(d.into());
        self
    }

    fn snippet(mut self, s: impl Into<String>) -> Completion {
        self.snippet = Some(s.into());
        self
    }
}

/// Что знает редактор об окружении: его имя, все окружения проекта и переменные.
pub struct EnvInfo<'a> {
    pub name: &'a str,
    pub envs: &'a [String],
    pub vars: &'a [VarInfo],
}

pub const FIELDS: [(&str, &str); 12] = [
    ("handler", "server handler, written by `outry import go`"),
    ("params", "parameters of the request when it is called"),
    ("only", "environments the request may be sent in"),
    ("confirm", "ask before sending"),
    ("timeout", "request timeout, default 30s"),
    ("redirects", "follow redirects, default true"),
    ("cache", "keep the response of a call between runs"),
    ("query", "query parameters"),
    ("headers", "request headers"),
    ("body", "JSON, text or file(…) body"),
    ("form", "application/x-www-form-urlencoded body"),
    ("multipart", "multipart/form-data body"),
];

const STATEMENTS: [(&str, &str); 3] = [
    ("expect", "checks of the response, one per line"),
    ("save", "save name = expr — a variable for later requests"),
    ("poll", "poll <cond> every 1s for 30s"),
];

pub const BUILTINS: [(&str, &str, &str); 12] = [
    ("uuid", "", "Random UUID v4"),
    ("now", "", "Unix time, seconds"),
    ("nowIso", "", "Current time, RFC 3339"),
    (
        "randomInt",
        "min, max",
        "Random integer, both ends included",
    ),
    ("randomString", "n", "`n` random letters and digits"),
    (
        "number",
        "x",
        "Converts to a number; `number(\"abc\")` is an error",
    ),
    ("string", "x", "Converts to a string"),
    ("json", "x", "`x` as compact JSON text"),
    ("base64", "x", "Base64-encodes"),
    ("unbase64", "x", "Decodes Base64"),
    (
        "file",
        "path",
        "A file for `body` and `multipart`, relative to this file",
    ),
    (
        "schema",
        "path",
        "A JSON Schema for `matches`, relative to this file",
    ),
];

/// Поля и методы значений: имя, аргументы (`None` — свойство), на чём, описание.
const MEMBERS: [(&str, Option<&str>, &str); 17] = [
    (
        "length",
        None,
        "strings, arrays: number of characters or elements",
    ),
    ("first", None, "arrays: first element or null"),
    ("last", None, "arrays: last element or null"),
    ("keys", None, "objects: field names"),
    ("values", None, "objects: field values"),
    ("startsWith", Some("s"), "strings"),
    ("endsWith", Some("s"), "strings"),
    ("contains", Some("x"), "strings: substring; arrays: element"),
    ("lower", Some(""), "strings"),
    ("upper", Some(""), "strings"),
    ("trim", Some(""), "strings"),
    ("split", Some("sep"), "strings: array of parts"),
    ("has", Some("key"), "objects: has the field"),
    ("any", Some("x => …"), "arrays: some element matches"),
    ("all", Some("x => …"), "arrays: every element matches"),
    ("map", Some("x => …"), "arrays: new array of results"),
    ("filter", Some("x => …"), "arrays: matching elements"),
];

const RESPONSE_DOCS: [(&str, &str); 5] = [
    ("status", "Status code of the response, a number"),
    (
        "headers",
        "Response headers; names are case-insensitive: `headers.content-type`",
    ),
    ("body", "Parsed JSON body, or the text if it is not JSON"),
    ("duration", "Time to the full response, milliseconds"),
    ("cookies", "Cookies of the run's jar for this request's URL"),
];

const TYPES: [&str; 6] = ["string", "number", "integer", "boolean", "null", "any"];

const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

const HEADERS: [&str; 10] = [
    "Accept",
    "Authorization",
    "Cache-Control",
    "Content-Type",
    "Cookie",
    "If-None-Match",
    "Idempotency-Key",
    "Origin",
    "User-Agent",
    "X-Request-Id",
];

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Открытая скобка, строка или подстановка перед курсором.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Frame {
    /// `{`, `(` или `[` и где она.
    Open(char, usize),
    Str(&'static str),
    /// `${` внутри строки.
    Interp,
}

/// Что открыто перед `offset`. `None` — курсор в комментарии.
fn frames(text: &str, offset: usize) -> Option<Vec<Frame>> {
    let src = &text[..offset];
    let mut stack: Vec<Frame> = Vec::new();
    let mut i = 0;
    let mut last_word = "";
    while i < src.len() {
        let rest = &src[i..];
        if let Some(Frame::Str(q)) = stack.last().copied() {
            if let Some(esc) = rest.strip_prefix('\\') {
                i += 1 + esc.chars().next().map_or(0, char::len_utf8);
            } else if rest.starts_with("${") {
                stack.push(Frame::Interp);
                i += 2;
            } else if rest.starts_with(q) {
                stack.pop();
                i += q.len();
            } else {
                i += rest.chars().next().map_or(1, char::len_utf8);
            }
            continue;
        }
        if rest.starts_with("//") {
            i += rest.find('\n')?;
            continue;
        }
        if let Some(body) = rest.strip_prefix("/*") {
            i += body.find("*/")? + 4;
            continue;
        }
        let c = rest.chars().next().unwrap_or(' ');
        if c == '/' && last_word == "matches" {
            // `/re/flags`: скобки внутри не считаются.
            let mut j = 1;
            let rb = rest.as_bytes();
            while j < rb.len() && rb[j] != b'/' && rb[j] != b'\n' {
                j += if rb[j] == b'\\' { 2 } else { 1 };
            }
            i += (j + 1).min(rest.len());
            last_word = "";
            continue;
        }
        if is_ident_start(c) {
            let len = rest
                .char_indices()
                .find(|&(_, c)| !is_ident_char(c))
                .map_or(rest.len(), |(n, _)| n);
            last_word = &rest[..len];
            i += len;
            continue;
        }
        if !c.is_whitespace() {
            last_word = "";
        }
        match c {
            '"' if rest.starts_with("\"\"\"") => {
                stack.push(Frame::Str("\"\"\""));
                i += 3;
                continue;
            }
            '"' => stack.push(Frame::Str("\"")),
            '\'' => stack.push(Frame::Str("'")),
            '{' | '(' | '[' => stack.push(Frame::Open(c, i)),
            '}' | ')' | ']' => {
                if let Some(Frame::Interp) = stack.last() {
                    if c == '}' {
                        stack.pop();
                    }
                } else if matches!(stack.last(), Some(Frame::Open(..))) {
                    stack.pop();
                }
            }
            _ => {}
        }
        i += c.len_utf8();
    }
    Some(stack)
}

/// Слово перед `offset` (буквы, цифры, `_`) и где оно начинается.
fn word_before(text: &str, offset: usize) -> (usize, &str) {
    let before = &text[..offset];
    let start = before
        .char_indices()
        .rev()
        .take_while(|&(_, c)| is_ident_char(c) || c == '-')
        .last()
        .map_or(offset, |(i, _)| i);
    (start, &text[start..offset])
}

/// Строка, в которой `pos`, до `pos`.
fn line_before(text: &str, pos: usize) -> &str {
    let start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    &text[start..pos]
}

/// Слово сразу перед `pos` (пропуская пробелы): `expect {` → `expect`.
fn word_ending_at(text: &str, pos: usize) -> &str {
    let before = text[..pos].trim_end();
    let start = before
        .char_indices()
        .rev()
        .take_while(|&(_, c)| is_ident_char(c) || c == '.')
        .last()
        .map_or(before.len(), |(i, _)| i);
    &before[start..]
}

/// Чем начинается элемент верхнего уровня, внутри которого курсор.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ItemKind {
    Request,
    Flow,
    Shape,
    Other,
}

fn item_kind(text: &str, open: usize) -> ItemKind {
    let line = line_before(text, open).trim_start();
    let mut words = line.split_whitespace();
    let mut first = words.next().unwrap_or("");
    // `Login: POST /login {` — запрос с именем.
    if first.len() > 1 && first.ends_with(':') {
        first = words.next().unwrap_or("");
    }
    if first == "flow" {
        ItemKind::Flow
    } else if first == "shape" {
        ItemKind::Shape
    } else if first.len() >= 2 && first.chars().all(|c| c.is_ascii_uppercase()) {
        ItemKind::Request
    } else {
        ItemKind::Other
    }
}

/// Автодополнение в `text` (текст файла в редакторе) в `offset`.
/// Возвращает начало заменяемого слова и варианты; `None` — подсказывать нечего.
pub fn complete(
    ws: &Workspace,
    text: &str,
    offset: usize,
    env: &EnvInfo,
) -> Option<(usize, Vec<Completion>)> {
    let offset = offset.min(text.len());
    let stack = frames(text, offset)?;
    if matches!(stack.last(), Some(Frame::Str(_))) {
        return None;
    }
    let (start, word) = word_before(text, offset);
    // Имя с дефисом — только ключ заголовка или поле после `.`.
    let start = if word.contains('-') && !line_before(text, start).trim().is_empty() {
        offset - word.rsplit('-').next().map_or(0, str::len)
    } else {
        start
    };
    let before = line_before(text, start);
    let line_start = before.trim().is_empty();
    let opens: Vec<(char, usize)> = stack
        .iter()
        .filter_map(|f| match f {
            Frame::Open(c, at) => Some((*c, *at)),
            _ => None,
        })
        .collect();
    let in_interp = stack.contains(&Frame::Interp);
    let innermost = opens.last().copied();
    let kind = opens
        .first()
        .map_or(ItemKind::Other, |&(_, at)| item_kind(text, at));
    let mut out = Vec::new();

    // Верхний уровень: методы и объявления.
    if opens.is_empty() && !in_interp {
        // `Login: |` — после имени запроса только метод.
        let named = before
            .trim()
            .strip_suffix(':')
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '_'));
        if named {
            for m in METHODS {
                out.push(Completion::new(m, Kind::Keyword).snippet(format!("{m} /$1")));
            }
            return Some((start, out));
        }
        if !line_start {
            return None;
        }
        out.push(
            Completion::new("request", Kind::Keyword)
                .detail("named request")
                .snippet("${1:Name}: ${2:GET} /$0"),
        );
        for m in METHODS {
            out.push(Completion::new(m, Kind::Keyword).snippet(format!("{m} /$1")));
        }
        out.push(
            Completion::new("flow", Kind::Keyword)
                .detail("scenario")
                .snippet("flow ${1:Name} {\n  $0\n}"),
        );
        out.push(
            Completion::new("shape", Kind::Keyword)
                .detail("named shape")
                .snippet("shape ${1:Name} {\n  $0\n}"),
        );
        out.push(
            Completion::new("let", Kind::Keyword)
                .detail("file-level value")
                .snippet("let ${1:name} = $0"),
        );
        return Some((start, out));
    }

    // После `.`: вызов по папке (`users.Create`) или поле/метод значения.
    if text[..start].ends_with('.') {
        let chain = word_ending_at(text, start - 1);
        let folder_items: Vec<Completion> = ws
            .callables()
            .filter(|(r, _)| ws.sources[r.file].folder() == chain)
            .map(|(r, name)| callable(ws, r, name))
            .collect();
        if !chain.is_empty() && !folder_items.is_empty() {
            return Some((start, folder_items));
        }
        for (name, args, doc) in MEMBERS {
            let c = match args {
                None => Completion::new(name, Kind::Property).detail(doc),
                Some("") => Completion::new(name, Kind::Method)
                    .detail(doc)
                    .snippet(format!("{name}()")),
                Some(a) => Completion::new(name, Kind::Method)
                    .detail(format!("({a}) {doc}"))
                    .snippet(format!("{name}($1)")),
            };
            out.push(c);
        }
        return Some((start, out));
    }

    // Формы: после `matches` и внутри `shape`.
    let after_matches = word_ending_at(text, start) == "matches";
    if after_matches || (kind == ItemKind::Shape && !line_start) || before.trim_end().ends_with('|')
    {
        for t in TYPES {
            out.push(Completion::new(t, Kind::Type));
        }
        out.extend(shapes(ws));
        if after_matches {
            out.push(
                Completion::new("schema", Kind::Function)
                    .detail("JSON Schema file")
                    .snippet("schema(\"$1\")"),
            );
        }
        return Some((start, out));
    }
    if kind == ItemKind::Shape {
        return None; // имя поля формы
    }

    // `only: [dev, …]`.
    if innermost.is_some_and(|(c, at)| c == '[' && line_before(text, at).trim() == "only:") {
        out.extend(
            env.envs
                .iter()
                .map(|e| Completion::new(e, Kind::Value).detail("environment")),
        );
        return Some((start, out));
    }

    // Аргументы вызова: `Login(email: …)`.
    if let Some(('(', at)) = innermost {
        let callee = word_ending_at(text, at);
        let arg_name = text[at + 1..start]
            .trim_end()
            .chars()
            .last()
            .is_none_or(|c| c == ',' || c == '(');
        let last = callee.rsplit('.').next().unwrap_or("");
        if last.starts_with(|c: char| c.is_uppercase()) {
            let path: Vec<String> = callee.split('.').map(str::to_string).collect();
            if arg_name {
                let Ok(r) = ws.resolve(&path) else {
                    return None;
                };
                let given = &text[at + 1..start];
                for p in ws.params_of(r) {
                    if given.contains(&format!("{p}:")) {
                        continue;
                    }
                    let detail = param_default(ws, r, &p)
                        .map_or("parameter".to_string(), |d| format!("= {d}"));
                    out.push(
                        Completion::new(&p, Kind::Param)
                            .detail(detail)
                            .snippet(format!("{p}: $0")),
                    );
                }
                return Some((start, out));
            }
        }
    }

    // Начало строки в блоке запроса: поля.
    let block = innermost.map(|(_, at)| word_ending_at(text, at));
    if line_start && !in_interp {
        if opens.len() == 1 && kind == ItemKind::Request {
            for (f, doc) in FIELDS {
                let snippet = match f {
                    "handler" => "handler: $0",
                    "only" => "only: [$0]",
                    "confirm" => "confirm: true",
                    "timeout" => "timeout: ${1:10s}",
                    "redirects" => "redirects: false",
                    "cache" => "cache: ${1:30m}",
                    "body" => "body {\n  $0\n}",
                    _ => "",
                };
                let c = Completion::new(f, Kind::Field).detail(doc);
                out.push(if snippet.is_empty() {
                    c.snippet(format!("{f} {{\n  $0\n}}"))
                } else {
                    c.snippet(snippet)
                });
            }
        }
        if opens.len() == 1 && kind != ItemKind::Other {
            for (s, doc) in STATEMENTS {
                if s == "poll" && kind == ItemKind::Flow {
                    continue;
                }
                let snippet = match s {
                    "expect" => "expect {\n  $0\n}",
                    "save" => "save ${1:name} = $0",
                    _ => "poll $1 every ${2:1s} for ${3:30s}",
                };
                out.push(
                    Completion::new(s, Kind::Keyword)
                        .detail(doc)
                        .snippet(snippet),
                );
            }
            if kind == ItemKind::Flow {
                out.push(Completion::new("params", Kind::Field).snippet("params {\n  $0\n}"));
            }
        }
        match block {
            Some("headers") => {
                out.extend(HEADERS.iter().map(|h| {
                    Completion::new(*h, Kind::Value)
                        .detail("header")
                        .snippet(format!("{h}: $0"))
                }));
                return Some((start, out));
            }
            Some("params" | "query" | "form" | "multipart") => return Some((start, out)),
            _ => {}
        }
        if opens.len() == 1 && kind == ItemKind::Request {
            return Some((start, out));
        }
    }

    // Выражение.
    for k in ["fresh", "matches", "in", "typeof"] {
        out.push(Completion::new(k, Kind::Keyword));
    }
    for k in ["true", "false", "null"] {
        out.push(Completion::new(k, Kind::Constant));
    }
    for (name, args, doc) in BUILTINS {
        let snippet = if args.is_empty() {
            format!("{name}()")
        } else {
            format!("{name}($1)")
        };
        out.push(
            Completion::new(name, Kind::Function)
                .detail(format!("{name}({args})"))
                .doc(doc)
                .snippet(snippet),
        );
    }
    let mut seen = std::collections::HashSet::new();
    let names: Vec<(ItemRef, &str)> = ws.callables().collect();
    for &(r, name) in &names {
        let shown = if names.iter().filter(|(_, n)| *n == name).count() > 1 {
            let f = ws.sources[r.file].folder();
            if f.is_empty() {
                name.to_string()
            } else {
                format!("{f}.{name}")
            }
        } else {
            name.to_string()
        };
        if seen.insert(shown.clone()) {
            out.push(callable(ws, r, &shown));
        }
    }
    let item_start = opens.first().map_or(0, |&(_, at)| at);
    let mut locals = local_names(text, item_start, start);
    if kind == ItemKind::Request {
        for (n, doc) in RESPONSE_DOCS {
            out.push(
                Completion::new(n, Kind::Variable)
                    .detail("response")
                    .doc(doc),
            );
            locals.retain(|(l, _)| l != n);
        }
    }
    out.push(Completion::new("env", Kind::Variable).detail(format!("environment: {}", env.name)));
    for (n, what) in &locals {
        if seen.insert(n.clone()) {
            out.push(Completion::new(n, Kind::Variable).detail(*what));
        }
    }
    for v in env.vars {
        if !v.name.starts_with(is_ident_start) || !v.name.chars().all(is_ident_char) {
            continue;
        }
        if seen.insert(v.name.clone()) {
            out.push(
                Completion::new(&v.name, Kind::Variable)
                    .detail(var_detail(v))
                    .doc(format!("variable, {}", source_text(v.source))),
            );
        }
    }
    for name in saved_names(ws) {
        if seen.insert(name.clone()) {
            out.push(Completion::new(name, Kind::Variable).detail("saved by `save`"));
        }
    }
    Some((start, out))
}

fn shapes(ws: &Workspace) -> impl Iterator<Item = Completion> + '_ {
    ws.sources.iter().flat_map(|s| {
        s.file.items.iter().filter_map(|i| match i {
            Item::Shape(d) => Some(Completion::new(&d.name, Kind::Shape).detail("shape")),
            _ => None,
        })
    })
}

/// Вызов запроса или сценария: `label` — имя для вставки (с папкой при совпадениях).
fn callable(ws: &Workspace, r: ItemRef, label: &str) -> Completion {
    let params = ws.params_of(r);
    let item = ws.item(r);
    let (kind, detail, doc) = match item {
        Item::Request(req) => (
            Kind::Request,
            format!(
                "{} {}",
                req.method,
                req.target.span.text(&ws.sources[r.file].text)
            ),
            &req.doc,
        ),
        Item::Flow(f) => (Kind::Flow, "flow".to_string(), &f.doc),
        _ => (Kind::Request, String::new(), &Doc::default()),
    };
    let required: Vec<&String> = params
        .iter()
        .filter(|p| param_default(ws, r, p).is_none())
        .collect();
    let snippet = match required.as_slice() {
        [] => format!("{label}()"),
        list => {
            let args: Vec<String> = list
                .iter()
                .enumerate()
                .map(|(i, p)| format!("{p}: ${}", i + 1))
                .collect();
            format!("{label}({})", args.join(", "))
        }
    };
    let mut c = Completion::new(label, kind).detail(detail).snippet(snippet);
    let mut text = String::new();
    if let Some(t) = &doc.title {
        text.push_str(t);
    }
    if !doc.description.is_empty() {
        text.push_str("\n\n");
        text.push_str(&doc.description);
    }
    if !params.is_empty() {
        text.push_str(&format!("\n\nParameters: {}", params.join(", ")));
    }
    if !text.is_empty() {
        c = c.doc(text.trim().to_string());
    }
    c
}

/// Значение по умолчанию параметра `p` (текст), если оно есть.
fn param_default(ws: &Workspace, r: ItemRef, p: &str) -> Option<String> {
    let params = match ws.item(r) {
        Item::Request(req) => &req.fields.params,
        Item::Flow(f) => &f.params,
        _ => return None,
    };
    let d = params.iter().find(|x| x.name == p)?.default.as_ref()?;
    Some(d.span.text(&ws.sources[r.file].text).to_string())
}

/// Имена `let` файла, `params` и `{id}` элемента, шаги сценария — по тексту, без разбора.
fn local_names(text: &str, item_start: usize, cursor: usize) -> Vec<(String, &'static str)> {
    let mut out: Vec<(String, &'static str)> = Vec::new();
    let mut add = |n: &str, what: &'static str| {
        if !n.is_empty() && !out.iter().any(|(x, _)| x == n) {
            out.push((n.to_string(), what));
        }
    };
    let ident = |s: &str| -> String { s.chars().take_while(|&c| is_ident_char(c)).collect() };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("let ") {
            add(&ident(rest.trim_start()), "let");
        }
    }
    let item = &text[item_start.min(cursor)..cursor];
    let header_start = text[..item_start].rfind('\n').map_or(0, |i| i + 1);
    let header = &text[header_start..item_start];
    let mut rest = header;
    while let Some(i) = rest.find('{') {
        let name = ident(&rest[i + 1..]);
        if rest[i + 1 + name.len()..].starts_with('}') {
            add(&name, "path parameter");
        }
        rest = &rest[i + 1..];
    }
    if let Some(i) = item.find("params") {
        if let Some(open) = item[i..].find('{') {
            let body = &item[i + open + 1..];
            let body = &body[..body.find('}').unwrap_or(body.len())];
            for part in body.split([',', '\n']) {
                add(&ident(part.trim_start()), "parameter");
            }
        }
    }
    for line in item.lines() {
        let t = line.trim_start();
        let name = ident(t);
        let after = t[name.len()..].trim_start();
        if !name.is_empty()
            && after.starts_with('=')
            && !after.starts_with("==")
            && !after.starts_with("=>")
        {
            add(&name, "local");
        }
        if let Some(s) = t.strip_prefix("save ") {
            add(&ident(s.trim_start()), "local");
        }
    }
    out
}

/// Имена из `save` по всему проекту: их задаст прогон.
pub fn saved_names(ws: &Workspace) -> Vec<String> {
    let mut out = Vec::new();
    for s in &ws.sources {
        for item in &s.file.items {
            if let Item::Request(r) = item {
                out.extend(r.fields.saves.iter().map(|s| s.name.clone()));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Где в проекте сохраняется `name`: `save name = …` в запросах и сценариях.
pub fn save_sites(ws: &Workspace, name: &str) -> Vec<(usize, Span)> {
    let mut out = Vec::new();
    for (fi, s) in ws.sources.iter().enumerate() {
        for item in &s.file.items {
            match item {
                Item::Request(r) => out.extend(
                    r.fields
                        .saves
                        .iter()
                        .filter(|s| s.name == name)
                        .map(|s| (fi, s.span)),
                ),
                Item::Flow(f) => out.extend(f.steps.iter().filter_map(|st| match st {
                    Step::Save(s) if s.name == name => Some((fi, s.span)),
                    _ => None,
                })),
                _ => {}
            }
        }
    }
    out
}

fn source_text(s: Option<Source>) -> &'static str {
    match s {
        Some(Source::Override) => "set with --var",
        Some(Source::Saved) => "saved by `save`",
        Some(Source::ProcessEnv) => "from the OUTRY_* environment variable",
        Some(Source::Env) => "from env.toml",
        Some(Source::Secret) => "from the keychain",
        Some(Source::Dynamic) => "dynamic",
        None => "not set",
    }
}

fn var_detail(v: &VarInfo) -> String {
    match (&v.value, v.secret) {
        (Some(val), true) => mask(val),
        (Some(val), false) => truncate(val, 60),
        (None, _) => "not set".to_string(),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// На что указывает символ под курсором.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// Вызов запроса или сценария.
    Item(ItemRef),
    /// Вызов, имя которого не разрешилось.
    Unresolved(String),
    /// Именованная форма; `decl` — курсор на имени в её объявлении.
    Shape {
        name: String,
        decl: bool,
    },
    /// Имя в выражении и на что оно ссылается.
    Name(String, Binding),
    Builtin(String),
    Member(String),
    /// Значение `handler:`.
    Handler(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    pub span: Span,
    pub target: Target,
}

/// Начало и конец имени в `span`, перед `(`: `users.Create` в `users.Create(id: 1)`.
fn name_span(text: &str, span: Span) -> Span {
    let s = span.text(text);
    let end = s.find('(').unwrap_or(s.len());
    let name = s[..end].trim_end();
    Span::new(span.start, span.start + name.len())
}

/// Символ файла `file` в `offset`.
pub fn symbol_at(ws: &Workspace, file: usize, offset: usize) -> Option<Symbol> {
    let src = &ws.sources[file];
    let text = &src.text;
    let inside = |s: Span| s.start <= offset && offset <= s.end;
    let mut best: Option<Symbol> = None;
    let mut consider = |sym: Symbol| {
        if inside(sym.span)
            && best
                .as_ref()
                .is_none_or(|b| sym.span.end - sym.span.start < b.span.end - b.span.start)
        {
            best = Some(sym);
        }
    };

    for (index, item) in src.file.items.iter().enumerate() {
        if !inside(item.span()) {
            continue;
        }
        // Имя в объявлении `Login: POST …` — тот же запрос, что и в вызове `Login()`.
        if let Item::Request(Request {
            name_span: Some(span),
            ..
        }) = item
        {
            consider(Symbol {
                span: *span,
                target: Target::Item(ItemRef { file, item: index }),
            });
        }
        let mut shapes = Vec::new();
        visit_item(item, &mut |e| match &e.kind {
            ExprKind::Call(c) => {
                let span = name_span(text, e.span);
                let target = match ws.resolve(&c.path) {
                    Ok(r) => Target::Item(r),
                    Err(_) => Target::Unresolved(c.name()),
                };
                consider(Symbol { span, target });
            }
            ExprKind::Builtin(n, _) => consider(Symbol {
                span: name_span(text, e.span),
                target: Target::Builtin(n.clone()),
            }),
            ExprKind::Method(recv, n, _) | ExprKind::Member(recv, n) => {
                if let Some(at) = text[recv.span.end..e.span.end].find(n.as_str()) {
                    let start = recv.span.end + at;
                    consider(Symbol {
                        span: Span::new(start, start + n.len()),
                        target: Target::Member(n.clone()),
                    });
                }
            }
            ExprKind::Matches(_, Pattern::Shape(sh)) => collect_named(sh, &mut shapes),
            _ => {}
        });
        match item {
            Item::Shape(d) => {
                collect_named(&d.shape, &mut shapes);
                let head = d.span.text(text);
                if let Some(at) = head.find(&d.name) {
                    let start = d.span.start + at;
                    consider(Symbol {
                        span: Span::new(start, start + d.name.len()),
                        target: Target::Shape {
                            name: d.name.clone(),
                            decl: true,
                        },
                    });
                }
            }
            Item::Request(r) => {
                if let Some(h) = &r.fields.handler {
                    if let Some((_, span)) = r.fields.order.iter().find(|(k, _)| *k == "handler") {
                        if let Some(at) = span.text(text).find(h.as_str()) {
                            let start = span.start + at;
                            consider(Symbol {
                                span: Span::new(start, start + h.len()),
                                target: Target::Handler(h.clone()),
                            });
                        }
                    }
                }
            }
            _ => {}
        }
        for (name, span) in shapes {
            consider(Symbol {
                span,
                target: Target::Shape { name, decl: false },
            });
        }
    }
    for r in scope::refs(&src.file) {
        if r.binding != Binding::Lambda {
            consider(Symbol {
                span: r.span,
                target: Target::Name(r.name, r.binding),
            });
        }
    }
    best
}

/// Текст hover в Markdown для символа в `offset` файла `file`. `var` — значение переменной
/// текущего окружения `env`.
pub fn hover(
    ws: &Workspace,
    file: usize,
    offset: usize,
    env: &str,
    var: &mut dyn FnMut(&str) -> Option<VarInfo>,
) -> Option<(Span, String)> {
    let sym = symbol_at(ws, file, offset)?;
    let text = &ws.sources[file].text;
    let code = |s: &str| format!("```outry\n{s}\n```");
    let md = match &sym.target {
        Target::Item(r) => item_hover(ws, *r),
        Target::Unresolved(name) => format!("unknown request or flow `{name}`"),
        Target::Shape { name, .. } => {
            let decl = ws.sources.iter().find_map(|s| {
                s.file.items.iter().find_map(|i| match i {
                    Item::Shape(d) if &d.name == name => {
                        Some((s, d.span.text(&s.text).to_string()))
                    }
                    _ => None,
                })
            });
            match decl {
                Some((s, t)) => {
                    let lines: Vec<&str> = t.lines().collect();
                    let shown = if lines.len() > 30 {
                        format!("{}\n  …", lines[..30].join("\n"))
                    } else {
                        t
                    };
                    format!("{}\n\n{}", code(&shown), slash(&s.path))
                }
                None => format!("unknown shape `{name}`"),
            }
        }
        Target::Builtin(name) => {
            let (_, args, doc) = BUILTINS.iter().find(|(n, _, _)| n == name)?;
            format!("{}\n\n{doc}", code(&format!("{name}({args})")))
        }
        Target::Member(name) => {
            let (_, args, doc) = MEMBERS.iter().find(|(n, _, _)| n == name)?;
            match args {
                None => format!("`.{name}` — {doc}"),
                Some(a) => format!("`.{name}({a})` — {doc}"),
            }
        }
        Target::Handler(h) => format!("Server handler `{h}`; requests are matched to routes by it"),
        Target::Name(name, binding) => match binding {
            Binding::Response => {
                let (_, doc) = RESPONSE_DOCS.iter().find(|(n, _)| n == name)?;
                format!("`{name}` — {doc}")
            }
            Binding::Env => format!("`env` — the current environment: `{env}`"),
            Binding::Let(span) | Binding::Local(span) => code(span.text(text)),
            Binding::Param(span) => {
                let mut s = format!("parameter\n\n{}", code(span.text(text)));
                if let Some(v) = var(name).filter(|v| v.value.is_some()) {
                    s.push_str(&format!(
                        "\n\nwithout an argument or default: {}",
                        var_line(&v)
                    ));
                }
                s
            }
            Binding::PathParam(_) => {
                let mut s = format!("path parameter `{{{name}}}`");
                if let Some(v) = var(name).filter(|v| v.value.is_some()) {
                    s.push_str(&format!("\n\nwithout an argument: {}", var_line(&v)));
                }
                s
            }
            Binding::Var => match var(name) {
                Some(v) if v.value.is_some() => format!("variable `{name}`\n\n{}", var_line(&v)),
                _ if saved_names(ws).contains(name) => format!(
                    "variable `{name}` — not set in `{env}` yet; a `save` in the project sets it"
                ),
                _ => format!("variable `{name}` is not defined in `{env}`"),
            },
            Binding::Lambda => return None,
        },
    };
    Some((sym.span, md))
}

fn var_line(v: &VarInfo) -> String {
    let value = match (&v.value, v.secret) {
        (Some(val), true) => mask(val),
        (Some(val), false) => truncate(val, 200),
        (None, _) => return "not set".into(),
    };
    format!("`{}` — {}", value.replace('`', "'"), source_text(v.source))
}

fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Hover вызова: заголовок запроса, описание, параметры, файл.
fn item_hover(ws: &Workspace, r: ItemRef) -> String {
    let s = &ws.sources[r.file];
    let (head, doc) = match ws.item(r) {
        Item::Request(req) => (
            format!("{} {}", req.method, req.target.span.text(&s.text)),
            &req.doc,
        ),
        Item::Flow(f) => (format!("flow {}", f.name), &f.doc),
        _ => return String::new(),
    };
    let mut md = format!("```outry\n{head}\n```");
    if let Some(t) = &doc.title {
        md.push_str(&format!("\n\n**{t}**"));
    }
    if !doc.description.is_empty() {
        md.push_str(&format!("\n\n{}", doc.description.trim()));
    }
    let params = ws.params_of(r);
    if !params.is_empty() {
        let list: Vec<String> = params
            .iter()
            .map(|p| match param_default(ws, r, p) {
                Some(d) => format!("`{p} = {d}`"),
                None => format!("`{p}`"),
            })
            .collect();
        md.push_str(&format!("\n\nParameters: {}", list.join(", ")));
    }
    if let Item::Request(req) = ws.item(r) {
        if let Some(only) = &req.fields.only {
            md.push_str(&format!("\n\nOnly in: {}", only.join(", ")));
        }
    }
    md.push_str(&format!(
        "\n\n{}:{}",
        slash(&s.path),
        s.line(ws.item(r).span().start)
    ));
    md
}

/// Запросы и сценарии файла: элемент, имя и строка (с 1) — для кнопок «Send» / «Run flow».
pub fn runnables(ws: &Workspace, file: usize) -> Vec<(ItemRef, Option<&str>, usize, bool)> {
    let s = &ws.sources[file];
    s.file
        .items
        .iter()
        .enumerate()
        .filter_map(|(ii, item)| {
            let flow = match item {
                Item::Request(_) => false,
                Item::Flow(_) => true,
                _ => return None,
            };
            Some((
                ItemRef { file, item: ii },
                item.name(),
                s.line(item.span().start),
                flow,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ws(files: &[(&str, &str)]) -> Workspace {
        Workspace::from_sources(
            Path::new("/p"),
            files
                .iter()
                .map(|(p, t)| (PathBuf::from(p), t.to_string()))
                .collect(),
        )
    }

    const LOGIN: &str = "// Login\nPOST /login {\n  params {\n    email: \"a@b.c\"\n    password\n  }\n  body { email, password }\n  save token = body.token\n}\n";

    fn labels(text: &str, ws: &Workspace) -> Vec<String> {
        let at = text.find('|').expect("cursor");
        let text = text.replace('|', "");
        let vars = [VarInfo {
            name: "base".into(),
            value: Some("http://x".into()),
            source: Some(Source::Env),
            secret: false,
        }];
        let envs = ["dev".to_string(), "prod".to_string()];
        let env = EnvInfo {
            name: "dev",
            envs: &envs,
            vars: &vars,
        };
        complete(ws, &text, at, &env)
            .map(|(_, c)| c.into_iter().map(|c| c.label).collect())
            .unwrap_or_default()
    }

    #[test]
    fn completes_by_context() {
        let w = ws(&[
            ("auth/login.outry", LOGIN),
            ("shapes.outry", "shape User { id: string }\n"),
        ]);
        // Верх файла.
        assert!(labels("PO|", &w).contains(&"POST".into()));
        // После имени запроса — только метод; поля внутри именованного запроса.
        let l = labels("Health: |", &w);
        assert!(
            l.contains(&"GET".into()) && !l.contains(&"flow".into()),
            "{l:?}"
        );
        let l = labels("Health: GET /x {\n  he|\n}", &w);
        assert!(l.contains(&"headers".into()) && l.contains(&"expect".into()));
        // Поля запроса в начале строки.
        let l = labels("GET /x {\n  he|\n}", &w);
        assert!(l.contains(&"headers".into()) && l.contains(&"expect".into()));
        assert!(!l.contains(&"uuid".into()));
        // Выражение: вызовы, функции, ответ, переменные, сохранённые значения.
        let l = labels("GET /x {\n  expect { st| }\n}", &w);
        for want in ["Login", "uuid", "status", "base", "token", "env"] {
            assert!(l.contains(&want.into()), "{want} in {l:?}");
        }
        // Аргументы вызова.
        let l = labels("GET /x {\n  headers { A: Login(| }\n}", &w);
        assert_eq!(l, ["email", "password"]);
        let l = labels("GET /x {\n  headers { A: Login(email: \"x\", p| }\n}", &w);
        assert_eq!(l, ["password"]);
        // Поля и методы после точки; вызов по папке.
        assert!(labels("GET /x {\n  expect { body.items.| }\n}", &w).contains(&"length".into()));
        assert_eq!(labels("GET /x {\n  body { t: auth.| }\n}", &w), ["Login"]);
        // Формы.
        let l = labels("GET /x {\n  expect { body matches U| }\n}", &w);
        assert!(l.contains(&"User".into()) && l.contains(&"string".into()));
        // Окружения в only.
        assert_eq!(labels("GET /x {\n  only: [d|]\n}", &w), ["dev", "prod"]);
        // Параметры и let файла как локальные имена.
        let l = labels(
            "let shop = \"m\"\nGET /o/{id} {\n  params { page: 1 }\n  query { p: p| }\n}",
            &w,
        );
        for want in ["shop", "id", "page"] {
            assert!(l.contains(&want.into()), "{want} in {l:?}");
        }
        // В строке и комментарии — ничего.
        assert!(labels("GET /x {\n  body \"ab|\"\n}", &w).is_empty());
        assert!(labels("// Lo|\nGET /x", &w).is_empty());
        // В подстановке — выражение.
        assert!(
            labels("GET /x {\n  headers { A: \"Bearer ${Lo|}\" }\n}", &w).contains(&"Login".into())
        );
    }

    #[test]
    fn symbols_and_hover() {
        let main = "let shop = \"m\"\n// Get order\nGET /orders/{id} {\n  handler: orders.Get\n  headers { A: Login(password: \"x\").body.token }\n  query { s: shop, b: base }\n  expect {\n    body matches User\n    uuid() != \"\"\n  }\n}\n";
        let w = ws(&[
            ("login.outry", LOGIN),
            ("main.outry", main),
            ("shapes.outry", "shape User { id: string }\n"),
        ]);
        let fi = w.file_index(Path::new("main.outry")).unwrap();
        let at = |s: &str| main.find(s).unwrap() + 1;
        let target = |s: &str| symbol_at(&w, fi, at(s)).map(|s| s.target);

        let login = w.resolve(&["Login".into()]).unwrap();
        assert_eq!(target("Login("), Some(Target::Item(login)));
        // Имя в объявлении `Name:` — сам запрос.
        let named = "Ping: GET /ping\n";
        let w2 = ws(&[("ping.outry", named)]);
        let pi = w2.file_index(Path::new("ping.outry")).unwrap();
        let ping = w2.resolve(&["Ping".into()]).unwrap();
        assert_eq!(
            symbol_at(&w2, pi, 1).map(|s| s.target),
            Some(Target::Item(ping))
        );
        assert!(matches!(target("shop,"), Some(Target::Name(n, Binding::Let(_))) if n == "shop"));
        assert!(matches!(target("base }"), Some(Target::Name(n, Binding::Var)) if n == "base"));
        assert_eq!(target("body.token"), Some(Target::Member("body".into())));
        assert!(
            matches!(target("body matches"), Some(Target::Name(n, Binding::Response)) if n == "body")
        );
        assert!(
            matches!(target("{id}"), Some(Target::Name(n, Binding::PathParam(_))) if n == "id")
        );
        assert_eq!(
            target("User"),
            Some(Target::Shape {
                name: "User".into(),
                decl: false
            })
        );
        assert_eq!(target("uuid"), Some(Target::Builtin("uuid".into())));
        assert_eq!(target("token }"), Some(Target::Member("token".into())));
        assert_eq!(
            target("orders.Get"),
            Some(Target::Handler("orders.Get".into()))
        );

        let mut var = |n: &str| {
            (n == "base").then(|| VarInfo {
                name: n.into(),
                value: Some("http://x".into()),
                source: Some(Source::Env),
                secret: false,
            })
        };
        let (_, md) = hover(&w, fi, at("Login("), "dev", &mut var).unwrap();
        assert!(
            md.contains("POST /login") && md.contains("`email = \"a@b.c\"`"),
            "{md}"
        );
        let (_, md) = hover(&w, fi, at("base }"), "dev", &mut var).unwrap();
        assert!(md.contains("http://x") && md.contains("env.toml"), "{md}");
        let (_, md) = hover(&w, fi, at("User"), "dev", &mut var).unwrap();
        assert!(md.contains("shape User { id: string }"), "{md}");
    }

    #[test]
    fn secrets_are_masked() {
        let w = ws(&[("a.outry", "GET /x {\n  headers { A: token }\n}\n")]);
        let text = &w.sources[0].text;
        let mut var = |n: &str| {
            Some(VarInfo {
                name: n.into(),
                value: Some("supersecretvalue".into()),
                source: Some(Source::Secret),
                secret: true,
            })
        };
        let (_, md) = hover(&w, 0, text.find("token").unwrap(), "dev", &mut var).unwrap();
        assert!(
            !md.contains("supersecretvalue") && md.contains("keychain"),
            "{md}"
        );
    }
}
