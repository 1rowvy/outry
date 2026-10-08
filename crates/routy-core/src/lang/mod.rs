//! Формат `*.routy`: C-подобный, декларативный, запрос можно вызвать как функцию.
//! Спецификация: `docs/src/content/docs/reference/routy-format.mdx`.
//!
//! ```text
//! // Create order
//! POST /orders/{shop} {
//!   headers { Authorization: "Bearer ${Login().body.token}" }
//!   body { items: [{ sku: "A-1", qty: 2 }] }
//!   expect { status == 201 }
//!   save order_id = body.id
//! }
//! ```
//!
//! `parse` — текст → `ast::File`; `Workspace` — все файлы проекта, имена и статическая проверка
//! (`routy check`); `exec::Run` — выполнение с кешем вызовов и cookies на прогон.

pub mod ast;
pub mod convert;
mod env_check;
pub mod eval;
pub mod exec;
pub mod fmt;
pub mod parse;
pub mod schema;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

pub use parse::{line_col, parse};

use crate::discover;
use crate::error::{Error, Result};
use ast::*;

/// Разобранный файл проекта.
#[derive(Debug, Clone)]
pub struct Source {
    /// Путь относительно корня проекта.
    pub path: PathBuf,
    /// Путь для сообщений и чтения `file(…)`: корень проекта + `path`.
    pub full: PathBuf,
    pub text: String,
    pub file: File,
}

impl Source {
    /// Папка в нотации вызова: `users/admin/x.routy` → `users.admin`.
    pub fn folder(&self) -> String {
        self.path
            .parent()
            .map(|p| {
                p.components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(".")
            })
            .unwrap_or_default()
    }

    pub fn line(&self, pos: usize) -> usize {
        line_col(&self.text, pos).0
    }

    fn error_at(&self, pos: usize, msg: impl Into<String>) -> Diagnostic {
        let (line, col) = line_col(&self.text, pos);
        Diagnostic {
            path: self.full.clone(),
            line,
            col,
            msg: msg.into(),
        }
    }
}

/// Ошибка `routy check`: `api/orders.routy:3:5: unknown request or flow `Nope``.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub msg: String,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}",
            self.path.display(),
            self.line,
            self.col,
            self.msg
        )
    }
}

/// Ссылка на элемент: индекс файла и элемента в нём.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemRef {
    pub file: usize,
    pub item: usize,
}

fn absolute_schemas(s: &mut Shape, dir: &Path) {
    match s {
        Shape::Schema(p) => *p = dir.join(&*p).to_string_lossy().into_owned(),
        Shape::Union(alts) => alts.iter_mut().for_each(|a| absolute_schemas(a, dir)),
        Shape::Array(inner) => absolute_schemas(inner, dir),
        Shape::Object(fields) => fields
            .iter_mut()
            .for_each(|f| absolute_schemas(&mut f.shape, dir)),
        _ => {}
    }
}

/// Все `*.routy` проекта: по ним разрешаются имена вызовов и форм.
#[derive(Debug, Default)]
pub struct Workspace {
    pub root: PathBuf,
    pub sources: Vec<Source>,
    /// Файлы, которые не разобрались; их элементы недоступны.
    pub errors: Vec<Diagnostic>,
    shapes: HashMap<String, Shape>,
}

impl Workspace {
    /// Читает все `*.routy` под корнем проекта.
    pub fn load(root: &Path) -> Result<Workspace> {
        let mut files = Vec::new();
        for rel in discover::routy_files(root)? {
            let text = std::fs::read_to_string(root.join(&rel))
                .map_err(|e| Error::from(e).in_file(root.join(&rel)))?;
            files.push((rel, text));
        }
        Ok(Workspace::from_sources(root, files))
    }

    /// Из готовых текстов (пути относительные к `root`).
    pub fn from_sources(root: &Path, files: Vec<(PathBuf, String)>) -> Workspace {
        let mut ws = Workspace {
            root: root.to_path_buf(),
            ..Workspace::default()
        };
        for (path, text) in files {
            ws.add(path, text);
        }
        ws
    }

    /// Добавляет (или заменяет) файл; возвращает его индекс, если он разобрался.
    /// Замена сдвигает индексы файлов после него: полученные раньше `ItemRef` устаревают.
    pub fn add(&mut self, path: PathBuf, text: String) -> Option<usize> {
        self.sources.retain(|s| s.path != path);
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
        let parsed = parse(&text, stem.as_deref());
        let idx = match parsed {
            Ok(file) => {
                let full = self.root.join(&path);
                self.errors.retain(|d| d.path != full);
                self.sources.push(Source {
                    path,
                    full,
                    text,
                    file,
                });
                Some(self.sources.len() - 1)
            }
            Err(e) => {
                let (line, col, msg) = match e {
                    Error::Syntax { line, col, msg } => (line, col, msg),
                    other => (1, 1, other.to_string()),
                };
                let full = self.root.join(&path);
                self.errors.retain(|d| d.path != full);
                self.errors.push(Diagnostic {
                    path: full,
                    line,
                    col,
                    msg,
                });
                None
            }
        };
        self.shapes = self
            .sources
            .iter()
            .flat_map(|s| {
                // `schema("./x.json")` в именованной форме — относительно её файла, а не вызывающего.
                let dir = s.full.parent().map(Path::to_path_buf).unwrap_or_default();
                s.file.items.iter().filter_map(move |i| match i {
                    Item::Shape(d) => {
                        let mut shape = d.shape.clone();
                        absolute_schemas(&mut shape, &dir);
                        Some((d.name.clone(), shape))
                    }
                    _ => None,
                })
            })
            .collect();
        idx
    }

    pub fn shape(&self, name: &str) -> Option<&Shape> {
        self.shapes.get(name)
    }

    pub fn item(&self, r: ItemRef) -> &Item {
        &self.sources[r.file].file.items[r.item]
    }

    /// Запрос или сценарий файла `file` на строке `line` (с 1). Строка между элементами —
    /// ближайший элемент выше; выше первого — первый.
    pub fn item_at(&self, file: usize, line: usize) -> Option<ItemRef> {
        let src = &self.sources[file];
        let runnable: Vec<(usize, &Item)> = src
            .file
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| matches!(i, Item::Request(_) | Item::Flow(_)))
            .collect();
        let at = runnable
            .iter()
            .rev()
            .find(|(_, i)| src.line(i.span().start) <= line)
            .or(runnable.first())?;
        Some(ItemRef { file, item: at.0 })
    }

    /// Индекс файла по пути относительно корня.
    pub fn file_index(&self, rel: &Path) -> Option<usize> {
        self.sources.iter().position(|s| s.path == rel)
    }

    /// Запросы и сценарии, которые можно вызвать по имени.
    pub fn callables(&self) -> impl Iterator<Item = (ItemRef, &str)> {
        self.sources.iter().enumerate().flat_map(|(fi, s)| {
            s.file
                .items
                .iter()
                .enumerate()
                .filter(|(_, i)| matches!(i, Item::Request(_) | Item::Flow(_)))
                .filter_map(move |(ii, i)| Some((ItemRef { file: fi, item: ii }, i.name()?)))
        })
    }

    /// `Login` или `users.Create`: имя уникально в проекте, при совпадении — с папкой.
    pub fn resolve(&self, path: &[String]) -> std::result::Result<ItemRef, String> {
        let Some((name, folder)) = path.split_last() else {
            return Err("empty name".into());
        };
        let folder = folder.join(".");
        let found: Vec<ItemRef> = self
            .callables()
            .filter(|(r, n)| {
                n == name && (folder.is_empty() || self.sources[r.file].folder() == folder)
            })
            .map(|(r, _)| r)
            .collect();
        match found.as_slice() {
            [one] => Ok(*one),
            [] => {
                let close = self
                    .callables()
                    .map(|(_, n)| n)
                    .filter(|n| distance(n, name) <= 2)
                    .min_by_key(|n| distance(n, name));
                Err(match close {
                    Some(c) => format!(
                        "unknown request or flow `{}` (did you mean `{c}`?)",
                        path.join(".")
                    ),
                    None => format!("unknown request or flow `{}`", path.join(".")),
                })
            }
            many => Err(format!(
                "`{name}` is ambiguous, call it with its folder: {}",
                many.iter()
                    .map(|r| {
                        let f = self.sources[r.file].folder();
                        if f.is_empty() {
                            name.clone()
                        } else {
                            format!("{f}.{name}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    /// Параметры, которые принимает элемент: `{name}` из пути и `params`.
    pub fn params_of(&self, r: ItemRef) -> Vec<String> {
        match self.item(r) {
            Item::Request(req) => {
                let mut out: Vec<String> = req
                    .target
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        TargetPart::Param(n, _) => Some(n.clone()),
                        _ => None,
                    })
                    .collect();
                for p in &req.fields.params {
                    if !out.contains(&p.name) {
                        out.push(p.name.clone());
                    }
                }
                out
            }
            Item::Flow(f) => f.params.iter().map(|p| p.name.clone()).collect(),
            _ => Vec::new(),
        }
    }

    /// Статическая проверка без отправки: имена, аргументы вызовов, формы, циклы.
    pub fn check(&self) -> Vec<Diagnostic> {
        let mut errors = self.errors.clone();

        // Одинаковые имена в одной папке.
        let mut seen: BTreeMap<(String, String), ItemRef> = BTreeMap::new();
        for (r, name) in self.callables() {
            let key = (self.sources[r.file].folder(), name.to_string());
            if let Some(first) = seen.get(&key) {
                let s = &self.sources[r.file];
                errors.push(s.error_at(
                    self.item(r).span().start,
                    format!(
                        "`{name}` is already defined in {}",
                        self.sources[first.file].full.display()
                    ),
                ));
            } else {
                seen.insert(key, r);
            }
        }
        let mut shape_seen: HashMap<&str, &Path> = HashMap::new();
        for s in &self.sources {
            for item in &s.file.items {
                if let Item::Shape(d) = item {
                    if let Some(first) = shape_seen.insert(&d.name, &s.full) {
                        errors.push(s.error_at(
                            d.span.start,
                            format!(
                                "shape `{}` is already defined in {}",
                                d.name,
                                first.display()
                            ),
                        ));
                    }
                }
            }
        }

        // Вызовы и формы; заодно граф вызовов для поиска циклов.
        let mut graph: BTreeMap<ItemRef, Vec<(ItemRef, usize)>> = BTreeMap::new();
        for (fi, s) in self.sources.iter().enumerate() {
            for (ii, item) in s.file.items.iter().enumerate() {
                let me = ItemRef { file: fi, item: ii };
                let mut calls = Vec::new();
                let mut shapes = Vec::new();
                let mut schemas = Vec::new();
                visit_item(item, &mut |e| match &e.kind {
                    ExprKind::Call(c) => calls.push((c, e.span)),
                    ExprKind::Matches(_, Pattern::Shape(sh)) => {
                        collect_named(sh, &mut shapes);
                        collect_schemas(sh, e.span, &mut schemas);
                    }
                    _ => {}
                });
                if let Item::Shape(d) = item {
                    collect_named(&d.shape, &mut shapes);
                    collect_schemas(&d.shape, d.span, &mut schemas);
                }
                for (name, span) in shapes {
                    if self.shape(&name).is_none() {
                        errors.push(s.error_at(span.start, format!("unknown shape `{name}`")));
                    }
                }
                let dir = s.full.parent().unwrap_or(Path::new("."));
                for (path, span) in schemas {
                    let problem = match std::fs::read_to_string(dir.join(path)) {
                        Err(e) => Some(e.to_string()),
                        Ok(text) => serde_json::from_str::<serde_json::Value>(&text)
                            .err()
                            .map(|e| format!("invalid JSON: {e}")),
                    };
                    if let Some(p) = problem {
                        errors.push(s.error_at(span.start, format!("schema(\"{path}\"): {p}")));
                    }
                }
                for (call, span) in calls {
                    let target = match self.resolve(&call.path) {
                        Ok(t) => t,
                        Err(msg) => {
                            errors.push(s.error_at(span.start, msg));
                            continue;
                        }
                    };
                    let params = self.params_of(target);
                    for (arg, _) in &call.args {
                        if !params.contains(arg) {
                            errors.push(s.error_at(
                                span.start,
                                format!(
                                    "`{}` has no parameter `{arg}` (parameters: {})",
                                    call.name(),
                                    if params.is_empty() {
                                        "none".to_string()
                                    } else {
                                        params.join(", ")
                                    }
                                ),
                            ));
                        }
                    }
                    graph.entry(me).or_default().push((target, span.start));
                }
            }
        }
        errors.extend(self.cycles(&graph));
        errors.sort_by(|a, b| (&a.path, a.line, a.col).cmp(&(&b.path, b.line, b.col)));
        errors
    }

    fn name_of(&self, r: ItemRef) -> String {
        self.item(r).name().unwrap_or("?").to_string()
    }

    fn cycles(&self, graph: &BTreeMap<ItemRef, Vec<(ItemRef, usize)>>) -> Vec<Diagnostic> {
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            Active,
            Done,
        }
        fn dfs(
            ws: &Workspace,
            graph: &BTreeMap<ItemRef, Vec<(ItemRef, usize)>>,
            node: ItemRef,
            marks: &mut HashMap<ItemRef, Mark>,
            stack: &mut Vec<ItemRef>,
            out: &mut Vec<Diagnostic>,
        ) {
            marks.insert(node, Mark::Active);
            stack.push(node);
            for &(next, pos) in graph.get(&node).into_iter().flatten() {
                match marks.get(&next) {
                    Some(Mark::Active) => {
                        let from = stack.iter().position(|r| *r == next).unwrap_or(0);
                        let mut chain: Vec<String> =
                            stack[from..].iter().map(|r| ws.name_of(*r)).collect();
                        chain.push(ws.name_of(next));
                        out.push(
                            ws.sources[node.file]
                                .error_at(pos, format!("call cycle: {}", chain.join(" → "))),
                        );
                    }
                    Some(Mark::Done) => {}
                    None => dfs(ws, graph, next, marks, stack, out),
                }
            }
            stack.pop();
            marks.insert(node, Mark::Done);
        }
        let mut marks = HashMap::new();
        let mut out = Vec::new();
        for &node in graph.keys() {
            if !marks.contains_key(&node) {
                dfs(self, graph, node, &mut marks, &mut Vec::new(), &mut out);
            }
        }
        out
    }
}

/// Расстояние Левенштейна — для подсказки «did you mean».
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

fn collect_named(s: &Shape, out: &mut Vec<(String, Span)>) {
    match s {
        Shape::Named(n, span) => out.push((n.clone(), *span)),
        Shape::Union(alts) => alts.iter().for_each(|a| collect_named(a, out)),
        Shape::Array(inner) => collect_named(inner, out),
        Shape::Object(fields) => fields.iter().for_each(|f| collect_named(&f.shape, out)),
        _ => {}
    }
}

fn collect_schemas<'a>(s: &'a Shape, at: Span, out: &mut Vec<(&'a str, Span)>) {
    match s {
        Shape::Schema(p) => out.push((p, at)),
        Shape::Union(alts) => alts.iter().for_each(|a| collect_schemas(a, at, out)),
        Shape::Array(inner) => collect_schemas(inner, at, out),
        Shape::Object(fields) => fields
            .iter()
            .for_each(|f| collect_schemas(&f.shape, at, out)),
        _ => {}
    }
}

/// Обходит все выражения элемента, включая вложенные.
pub fn visit_item<'a>(item: &'a Item, f: &mut impl FnMut(&'a Expr)) {
    match item {
        Item::Request(r) => {
            for p in &r.target.parts {
                if let TargetPart::Expr(e) = p {
                    visit(e, f);
                }
            }
            let fl = &r.fields;
            for p in &fl.params {
                if let Some(d) = &p.default {
                    visit(d, f);
                }
            }
            for e in fl.query.iter().chain(&fl.headers) {
                visit(&e.value, f);
            }
            match &fl.body {
                Some(Body::Value(e)) => visit(e, f),
                Some(Body::Form(es) | Body::Multipart(es)) => {
                    es.iter().for_each(|e| visit(&e.value, f))
                }
                None => {}
            }
            if let Some(p) = &fl.poll {
                visit(&p.until, f);
            }
            fl.expect.iter().for_each(|e| visit(e, f));
            fl.saves.iter().for_each(|s| visit(&s.value, f));
        }
        Item::Flow(fl) => {
            for p in &fl.params {
                if let Some(d) = &p.default {
                    visit(d, f);
                }
            }
            for step in &fl.steps {
                match step {
                    Step::Bind { value, .. } => visit(value, f),
                    Step::Do(e) => visit(e, f),
                    Step::Expect(es, _) => es.iter().for_each(|e| visit(e, f)),
                    Step::Save(s) => visit(&s.value, f),
                }
            }
        }
        Item::Let(l) => visit(&l.value, f),
        Item::Shape(_) => {}
    }
}

pub fn visit<'a>(e: &'a Expr, f: &mut impl FnMut(&'a Expr)) {
    f(e);
    match &e.kind {
        ExprKind::Str(parts) => {
            for p in parts {
                if let StrPart::Expr(x) = p {
                    visit(x, f);
                }
            }
        }
        ExprKind::Array(items) => items.iter().for_each(|x| visit(x, f)),
        ExprKind::Object(fields) => fields.iter().for_each(|(_, x)| visit(x, f)),
        ExprKind::Member(x, _) | ExprKind::Lambda(_, x) | ExprKind::Unary(_, x) => visit(x, f),
        ExprKind::Matches(x, _) => visit(x, f),
        ExprKind::Index(a, b) | ExprKind::Binary(_, a, b) => {
            visit(a, f);
            visit(b, f);
        }
        ExprKind::Builtin(_, args) => args.iter().for_each(|x| visit(x, f)),
        ExprKind::Method(recv, _, args) => {
            visit(recv, f);
            args.iter().for_each(|x| visit(x, f));
        }
        ExprKind::Call(c) => c.args.iter().for_each(|(_, x)| visit(x, f)),
        ExprKind::Null | ExprKind::Bool(_) | ExprKind::Num(_) | ExprKind::Ident(_) => {}
    }
}
