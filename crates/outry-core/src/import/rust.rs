//! Роуты из исходников Rust (axum). В отличие от Go, шаблонов `.scm` нет: роутер axum —
//! цепочка вызовов-значений, и её проще вычислить, чем сопоставить с шаблоном.
//!
//! - Роутер — выражение: `Router::new()`, `.route(path, get(h).post(h2))`, `.nest(path, r)`,
//!   `.merge(r)`, `.layer(l)` / `.route_layer(l)`; прочие методы (`with_state`, `fallback`, …)
//!   роуты не меняют. Переменная — её последнее присваивание (`let app = …`, `app = app.…`)
//!   до места использования, вызов функции — её возвращаемое выражение, параметр функции —
//!   аргумент вызова (`fn routes(r: Router) -> Router { r.route(…) }`).
//! - Корни — цепочки с методами роутера, которые не вошли ни в какую другую цепочку: роуты
//!   `users::router()` из `.nest("/users", users::router())` получают префикс, а сама функция
//!   отдельно не считается.
//! - `.layer()` действует на роуты, добавленные до него (как в axum); middleware — снаружи
//!   внутрь: `from_fn(auth)` → `auth`, `CorsLayer::permissive()` → `CorsLayer`,
//!   слои `ServiceBuilder` — по отдельности.
//! - Пути: строки и строковые константы; `{id}`, `{*rest}` (axum 0.8) и `:id`, `*rest` (0.7).
//! - Имена (функции, типы, константы) разрешаются по модулям файла (`src/a/b.rs` → `a::b`,
//!   `mod x { … }`) и `use`; что не разрешилось однозначно — ищется по имени во всём каталоге.
//!   Код в `#[cfg(test)]`, `tests/`, `benches/`, `examples/` и `target/` не сканируется.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use tree_sitter::{Language, Node, Parser, Tree};

mod describe;

use super::{ANY, Route, Scan, normalize_route};
use crate::error::{Error, Result};

const SKIP_DIRS: &[&str] = &[
    "target",
    "tests",
    "benches",
    "examples",
    "vendor",
    "node_modules",
    "testdata",
];

/// Глубина вычисления выражений-роутеров (вложенные `nest`, переменные, вызовы).
const MAX_DEPTH: usize = 48;

/// Шаблон, нашедший роут (`Route::router`).
const ROUTER: &str = "axum";

/// Методы `Router`, по которым цепочка считается роутером.
const ROUTER_METHODS: &[&str] = &[
    "route",
    "nest",
    "merge",
    "layer",
    "route_layer",
    "with_state",
    "fallback",
    "nest_service",
    "route_service",
];

fn language() -> Language {
    tree_sitter_rust::LANGUAGE.into()
}

/// Все роуты axum в `*.rs` под `dir`.
pub fn scan(dir: &Path) -> Result<Scan> {
    let mut paths = Vec::new();
    walk(dir, &mut paths)?;
    paths.sort();
    let mut parser = Parser::new();
    parser
        .set_language(&language())
        .map_err(|e| Error::Import(e.to_string()))?;
    let mut files = Vec::new();
    for path in paths {
        let src = std::fs::read_to_string(&path).map_err(|e| Error::from(e).in_file(&path))?;
        let Some(tree) = parser.parse(&src, None) else {
            continue;
        };
        let rel = path.strip_prefix(dir).unwrap_or(&path).to_path_buf();
        files.push(RsFile::new(rel, src, tree));
    }
    let idx = Index::build(&files);
    Ok(idx.scan())
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).map_err(|e| Error::from(e).in_file(dir))? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            if !SKIP_DIRS.contains(&name.as_ref()) {
                walk(&path, out)?;
            }
        } else if name.ends_with(".rs") && name != "build.rs" {
            out.push(path);
        }
    }
    Ok(())
}

pub(super) struct RsFile {
    pub path: PathBuf,
    pub src: String,
    pub tree: Tree,
    /// Модуль файла относительно корня крейта: `src/api/users.rs` → `api::users`
    module: Vec<String>,
}

impl RsFile {
    fn new(path: PathBuf, src: String, tree: Tree) -> RsFile {
        RsFile {
            module: file_module(&path),
            path,
            src,
            tree,
        }
    }
}

/// `src/api/users.rs` → `api::users`, `src/api/mod.rs` → `api`, `src/main.rs` → корень.
fn file_module(rel: &Path) -> Vec<String> {
    let comps: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let start = comps.iter().rposition(|c| c == "src").map_or(0, |i| i + 1);
    let mut m: Vec<String> = comps[start.min(comps.len())..].to_vec();
    if let Some(last) = m.pop() {
        let stem = last.strip_suffix(".rs").unwrap_or(&last).to_string();
        let root = m.is_empty() && (stem == "main" || stem == "lib");
        if stem != "mod" && !root {
            m.push(stem);
        }
    }
    m
}

/// Объявление верхнего уровня (или в `mod`/`impl`): функция, тип.
pub(super) struct Item {
    pub file: usize,
    /// Модуль; у методов — ещё и тип: `handlers::UserHandler`
    pub module: Vec<String>,
    pub name: String,
    pub start: usize,
    pub end: usize,
}

impl Item {
    fn path(&self) -> impl Iterator<Item = &str> {
        self.module
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(self.name.as_str()))
    }
}

pub(super) struct Index<'f> {
    pub files: &'f [RsFile],
    pub fns: Vec<Item>,
    pub types: Vec<Item>,
    /// Строковые константы: имя → значение
    consts: HashMap<String, String>,
    /// Типы ошибок с `impl IntoResponse`: имя типа → (вариант, код ответа)
    pub errors: HashMap<String, Vec<(String, u16)>>,
    /// `use` каждого файла
    uses: Vec<Uses>,
    /// Конструкторы ошибок: (тип, функция, вариант) — `fn not_found() -> Self { Self::NotFound }`
    ctors: Vec<(String, String, String)>,
}

/// `use` файла: локальное имя → полный путь; `use a::b::*`.
#[derive(Default)]
struct Uses {
    names: HashMap<String, Vec<String>>,
    globs: Vec<Vec<String>>,
}

impl<'f> Index<'f> {
    fn build(files: &'f [RsFile]) -> Index<'f> {
        let mut idx = Index {
            files,
            fns: Vec::new(),
            types: Vec::new(),
            consts: HashMap::new(),
            errors: HashMap::new(),
            uses: Vec::new(),
            ctors: Vec::new(),
        };
        for (i, f) in files.iter().enumerate() {
            let mut uses = Uses::default();
            idx.collect(i, f.tree.root_node(), f.module.clone(), &mut uses);
            idx.uses.push(uses);
        }
        for (ty, func, variant) in std::mem::take(&mut idx.ctors) {
            let code = idx
                .errors
                .get(&ty)
                .and_then(|vs| vs.iter().find(|(v, _)| *v == variant))
                .map(|&(_, c)| c);
            if let Some(code) = code {
                idx.errors.entry(ty).or_default().push((func, code));
            }
        }
        idx
    }

    fn collect(&mut self, file: usize, container: Node, module: Vec<String>, uses: &mut Uses) {
        let src = &self.files[file].src;
        for node in children(container) {
            match node.kind() {
                "function_item" => {
                    if let Some(name) = node.child_by_field_name("name") {
                        self.fns.push(Item {
                            file,
                            module: module.clone(),
                            name: text(name, src).to_string(),
                            start: node.start_byte(),
                            end: node.end_byte(),
                        });
                    }
                }
                "struct_item" | "enum_item" | "type_item" | "union_item" => {
                    if let Some(name) = node.child_by_field_name("name") {
                        self.types.push(Item {
                            file,
                            module: module.clone(),
                            name: text(name, src).to_string(),
                            start: node.start_byte(),
                            end: node.end_byte(),
                        });
                    }
                }
                "const_item" | "static_item" => {
                    if let (Some(name), Some(value)) = (
                        node.child_by_field_name("name"),
                        node.child_by_field_name("value"),
                    ) && let Some(s) = string_literal(value, src)
                    {
                        self.consts.insert(text(name, src).to_string(), s);
                    }
                }
                "mod_item" => {
                    if cfg_test(node, src) {
                        continue;
                    }
                    if let (Some(name), Some(body)) = (
                        node.child_by_field_name("name"),
                        node.child_by_field_name("body"),
                    ) {
                        let mut m = module.clone();
                        m.push(text(name, src).to_string());
                        self.collect(file, body, m, uses);
                    }
                }
                "impl_item" => {
                    if cfg_test(node, src) {
                        continue;
                    }
                    let Some(ty) = node.child_by_field_name("type") else {
                        continue;
                    };
                    let ty_name = last_segment(text(ty, src));
                    if let Some(body) = node.child_by_field_name("body") {
                        let mut m = module.clone();
                        m.push(ty_name.clone());
                        let fns_before = self.fns.len();
                        self.collect(file, body, m, uses);
                        // Коды вариантов — из `match` в любом `impl` типа (`into_response`,
                        // свой `fn status_code`); конструкторы `Self::NotFound` — по имени.
                        for f in &self.fns[fns_before..] {
                            let Some(fbody) = container
                                .descendant_for_byte_range(f.start, f.end)
                                .and_then(|d| d.child_by_field_name("body"))
                            else {
                                continue;
                            };
                            let codes = error_codes(fbody, &ty_name, src);
                            if !codes.is_empty() {
                                self.errors
                                    .entry(ty_name.clone())
                                    .or_default()
                                    .extend(codes);
                            } else if let Some(v) =
                                tail(fbody).and_then(|t| variant_of(t, &ty_name, src))
                            {
                                self.ctors.push((ty_name.clone(), f.name.clone(), v));
                            }
                        }
                    }
                }
                "use_declaration" => {
                    if let Some(arg) = node.child_by_field_name("argument") {
                        collect_use(arg, &[], src, &module, uses);
                    }
                }
                _ => {}
            }
        }
    }

    /// Объявление по пути, как его пишут в файле `file` из модуля `module`.
    pub(super) fn resolve(
        &self,
        items: &[Item],
        file: usize,
        module: &[String],
        segs: &[String],
    ) -> Option<usize> {
        let first = segs.first()?;
        let mut candidates: Vec<Vec<String>> = Vec::new();
        if matches!(first.as_str(), "crate" | "self" | "super") {
            candidates.push(normalize_path(segs, module));
        } else if let Some(mut p) = self.uses[file].names.get(first).cloned() {
            p.extend(segs[1..].iter().cloned());
            candidates.push(p);
        } else {
            let mut p = module.to_vec();
            p.extend(segs.iter().cloned());
            candidates.push(p);
            if segs.len() == 1 {
                for mut g in self.uses[file].globs.clone() {
                    g.extend(segs.iter().cloned());
                    candidates.push(g);
                }
            }
            candidates.push(segs.to_vec());
        }
        for c in &candidates {
            let found: Vec<usize> = items
                .iter()
                .enumerate()
                .filter(|(_, it)| it.path().eq(c.iter().map(String::as_str)))
                .map(|(i, _)| i)
                .collect();
            if let Some(i) = prefer(&found, items, file) {
                return Some(i);
            }
        }
        // По имени: путь объявления — хвост пути из кода или наоборот (`my_crate::api::f`,
        // `api::f` при `use super::*`).
        let name = segs.last()?;
        let found: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, it)| &it.name == name)
            .filter(|(_, it)| {
                let path: Vec<&str> = it.path().collect();
                candidates.iter().any(|c| {
                    let c: Vec<&str> = c.iter().map(String::as_str).collect();
                    c.ends_with(&path) || path.ends_with(&c)
                })
            })
            .map(|(i, _)| i)
            .collect();
        prefer(&found, items, file)
    }

    /// Модуль, в котором узел файла: модуль файла + `mod x { … }` + тип `impl`.
    pub(super) fn module_at(&self, file: usize, node: Node) -> Vec<String> {
        let f = &self.files[file];
        let mut extra = Vec::new();
        let mut n = node.parent();
        while let Some(p) = n {
            match p.kind() {
                "mod_item" => {
                    if let Some(name) = p.child_by_field_name("name") {
                        extra.push(text(name, &f.src).to_string());
                    }
                }
                "impl_item" => {
                    if let Some(ty) = p.child_by_field_name("type") {
                        extra.push(last_segment(text(ty, &f.src)));
                    }
                }
                _ => {}
            }
            n = p.parent();
        }
        extra.reverse();
        let mut m = f.module.clone();
        m.extend(extra);
        m
    }

    pub(super) fn node(&self, item: &Item) -> Option<Node<'f>> {
        self.files[item.file]
            .tree
            .root_node()
            .descendant_for_byte_range(item.start, item.end)
            .and_then(|n| {
                // descendant_for_byte_range может вернуть узел меньше, если диапазоны совпали
                // с дочерним; поднимаемся до объявления.
                let mut n = n;
                while n.start_byte() != item.start || n.end_byte() != item.end {
                    n = n.parent()?;
                }
                while n
                    .parent()
                    .is_some_and(|p| p.start_byte() == item.start && p.end_byte() == item.end)
                {
                    n = n.parent()?;
                }
                Some(n)
            })
    }

    fn string(&self, file: usize, node: Node) -> Option<String> {
        let src = &self.files[file].src;
        match node.kind() {
            "string_literal" | "raw_string_literal" => string_literal(node, src),
            "identifier" | "scoped_identifier" => {
                self.consts.get(&last_segment(text(node, src))).cloned()
            }
            "reference_expression" | "parenthesized_expression" => node
                .child_by_field_name("value")
                .or_else(|| node.named_child(0))
                .and_then(|v| self.string(file, v)),
            _ => None,
        }
    }

    fn scan(&self) -> Scan {
        let mut ev = Eval {
            idx: self,
            consumed: HashSet::new(),
            stack: Vec::new(),
            frames: Vec::new(),
            warnings: Vec::new(),
        };
        let mut candidates = Vec::new();
        for (i, f) in self.files.iter().enumerate() {
            find_candidates(f.tree.root_node(), &f.src, &mut |n| candidates.push((i, n)));
        }
        // Предупреждения — только у корней: цепочка внутри функции-помощника без аргументов
        // (`fn route(path: &str, …)`) сама по себе путь не знает.
        let evaluated: Vec<(usize, Node, Vec<Raw>, Vec<String>)> = candidates
            .into_iter()
            .map(|(file, node)| {
                let routes = ev.router(file, node, 0);
                (file, node, routes, std::mem::take(&mut ev.warnings))
            })
            .collect();
        let mut warnings = Vec::new();
        let describer = describe::Describer::new(self);
        let mut infos: HashMap<usize, super::RouteInfo> = HashMap::new();
        let mut routes: Vec<Route> = Vec::new();
        let mut seen = HashSet::new();
        for (file, node, raws, warned) in evaluated {
            if ev
                .consumed
                .contains(&(file, node.start_byte(), node.end_byte()))
            {
                continue;
            }
            warnings.extend(warned);
            for r in raws {
                let path = normalize_route(&wildcards(&r.path));
                let handler = r.handler.as_ref().map(|h| self.handler_name(h));
                if !seen.insert((
                    r.method.clone(),
                    path.clone(),
                    r.file,
                    r.line,
                    handler.clone(),
                )) {
                    continue;
                }
                let info = match r.handler {
                    Some(Handler::Fn(f)) => infos
                        .entry(f)
                        .or_insert_with(|| describer.describe(f))
                        .clone(),
                    _ => super::RouteInfo::default(),
                };
                routes.push(Route {
                    method: r.method,
                    path,
                    source: self.files[r.file].path.clone(),
                    line: r.line,
                    handler,
                    router: ROUTER.into(),
                    info,
                    middleware: r.middleware,
                });
            }
        }
        routes.sort_by(|a, b| (&a.source, a.line, &a.method).cmp(&(&b.source, b.line, &b.method)));
        warnings.sort();
        warnings.dedup();
        Scan {
            files: self.files.len(),
            routes,
            shapes: describer.into_shapes(),
            warnings,
        }
    }

    /// `handlers::users::list` → `users.list`, метод `UserHandler::list` → `UserHandler.list`.
    fn handler_name(&self, h: &Handler) -> String {
        match h {
            Handler::Fn(i) => {
                let f = &self.fns[*i];
                match f.module.last() {
                    Some(m) => format!("{m}.{}", f.name),
                    None => f.name.clone(),
                }
            }
            Handler::Text(t) => t.clone(),
        }
    }
}

/// Из нескольких одинаковых — в том же файле, иначе единственное.
fn prefer(found: &[usize], items: &[Item], file: usize) -> Option<usize> {
    match found {
        [] => None,
        [one] => Some(*one),
        many => many.iter().copied().find(|&i| items[i].file == file),
    }
}

/// `use` → таблица имён. `prefix` — путь снаружи (`use a::{b, c::d}`).
fn collect_use(node: Node, prefix: &[String], src: &str, module: &[String], out: &mut Uses) {
    let with = |extra: &str| -> Vec<String> {
        let mut p = prefix.to_vec();
        p.extend(segments(extra));
        p
    };
    match node.kind() {
        "identifier" | "scoped_identifier" | "crate" | "super" => {
            let full = with(text(node, src));
            if let Some(name) = full.last().cloned() {
                out.names.insert(name, normalize_path(&full, module));
            }
        }
        "self" => {
            if let Some(name) = prefix.last() {
                out.names
                    .insert(name.clone(), normalize_path(prefix, module));
            }
        }
        "use_as_clause" => {
            if let (Some(path), Some(alias)) = (
                node.child_by_field_name("path"),
                node.child_by_field_name("alias"),
            ) {
                let full = with(text(path, src));
                out.names
                    .insert(text(alias, src).to_string(), normalize_path(&full, module));
            }
        }
        "scoped_use_list" => {
            let p = node
                .child_by_field_name("path")
                .map_or_else(|| prefix.to_vec(), |p| with(text(p, src)));
            if let Some(list) = node.child_by_field_name("list") {
                collect_use(list, &p, src, module, out);
            }
        }
        "use_list" => {
            for c in children(node) {
                collect_use(c, prefix, src, module, out);
            }
        }
        "use_wildcard" => {
            let t = text(node, src).trim_end_matches('*').trim_end_matches("::");
            out.globs.push(normalize_path(&with(t), module));
        }
        _ => {}
    }
}

/// `crate::a` → `a`, `self::a` → модуль + `a`, `super::a` → родитель + `a`; остальное как есть.
fn normalize_path(segs: &[String], module: &[String]) -> Vec<String> {
    match segs.first().map(String::as_str) {
        Some("crate") => segs[1..].to_vec(),
        Some("self") => {
            let mut p = module.to_vec();
            p.extend(segs[1..].iter().cloned());
            p
        }
        Some("super") => {
            let mut p = module.to_vec();
            let mut rest = segs;
            while rest.first().is_some_and(|s| s == "super") {
                p.pop();
                rest = &rest[1..];
            }
            p.extend(rest.iter().cloned());
            p
        }
        _ => segs.to_vec(),
    }
}

/// Коды ответа в `into_response` ошибки: ветки `match` `Self::NotFound => StatusCode::NOT_FOUND`.
fn error_codes(body: Node, ty: &str, src: &str) -> Vec<(String, u16)> {
    let mut out = Vec::new();
    walk_tree(body, &mut |n| {
        if n.kind() != "match_arm" {
            return;
        }
        let (Some(pattern), Some(value)) = (
            n.child_by_field_name("pattern"),
            n.child_by_field_name("value"),
        ) else {
            return;
        };
        let Some(code) = first_status(value, src) else {
            return;
        };
        let mut variants = Vec::new();
        walk_tree(pattern, &mut |p| match p.kind() {
            "scoped_identifier" | "scoped_type_identifier" => {
                let segs = segments(text(p, src));
                if let [.., owner, variant] = segs.as_slice()
                    && (owner == "Self" || owner == ty)
                {
                    variants.push(variant.clone());
                }
            }
            "identifier"
                if p.parent().is_some_and(|q| {
                    matches!(
                        q.kind(),
                        "match_pattern" | "tuple_struct_pattern" | "or_pattern"
                    )
                }) && text(p, src).starts_with(|c: char| c.is_ascii_uppercase()) =>
            {
                variants.push(text(p, src).to_string());
            }
            _ => {}
        });
        out.extend(variants.into_iter().map(|v| (v, code)));
    });
    out
}

/// Вариант `Self::X` / `Тип::X` (в том числе с полями), которым является выражение.
fn variant_of(node: Node, ty: &str, src: &str) -> Option<String> {
    let path = match node.kind() {
        "scoped_identifier" => node,
        "call_expression" => node.child_by_field_name("function")?,
        "struct_expression" => node.child_by_field_name("name")?,
        _ => return None,
    };
    match segments(text(path, src)).as_slice() {
        [owner, variant] if owner == "Self" || owner == ty => Some(variant.clone()),
        _ => None,
    }
}

/// Первый `StatusCode::X` в выражении.
fn first_status(node: Node, src: &str) -> Option<u16> {
    let mut found = None;
    walk_tree(node, &mut |n| {
        if found.is_none() && n.kind() == "scoped_identifier" {
            found = status_path(text(n, src));
        }
    });
    found
}

/// `StatusCode::NOT_FOUND`, `http::StatusCode::OK` → код.
pub(super) fn status_path(path: &str) -> Option<u16> {
    let segs = segments(path);
    match segs.as_slice() {
        [.., owner, name] if owner == "StatusCode" => describe::status_const(name),
        _ => None,
    }
}

/// Вызовы-цепочки с методами роутера, которые не продолжаются дальше (`a.route(…).layer(…)` —
/// одна цепочка, начиная с последнего вызова). Код тестов пропускается.
fn find_candidates<'t>(node: Node<'t>, src: &str, f: &mut impl FnMut(Node<'t>)) {
    match node.kind() {
        "mod_item" | "impl_item" | "function_item" if cfg_test(node, src) || is_test(node, src) => {
            return;
        }
        "call_expression" => {
            let is_router = node
                .child_by_field_name("function")
                .filter(|func| func.kind() == "field_expression")
                .and_then(|func| func.child_by_field_name("field"))
                .is_some_and(|m| ROUTER_METHODS.contains(&text(m, src)));
            let continued = node
                .parent()
                .is_some_and(|p| p.kind() == "field_expression");
            if is_router && !continued {
                f(node);
            }
        }
        _ => {}
    }
    for c in children(node) {
        find_candidates(c, src, f);
    }
}

/// `#[cfg(test)]` над объявлением.
fn cfg_test(node: Node, src: &str) -> bool {
    attributes(node, src)
        .iter()
        .any(|a| a.replace(' ', "").starts_with("cfg(test"))
}

/// `#[test]`, `#[tokio::test]`.
fn is_test(node: Node, src: &str) -> bool {
    node.kind() == "function_item"
        && attributes(node, src)
            .iter()
            .any(|a| a == "test" || a.ends_with("::test") || a.starts_with("tokio::test"))
}

/// Атрибуты над объявлением (без `#[` и `]`), ближние — последними.
pub(super) fn attributes(node: Node, src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut prev = node.prev_sibling();
    while let Some(p) = prev {
        match p.kind() {
            "attribute_item" => {
                if let Some(a) = p.named_child(0) {
                    out.push(text(a, src).trim().to_string());
                }
            }
            "line_comment" | "block_comment" => {}
            _ => break,
        }
        prev = p.prev_sibling();
    }
    out.reverse();
    out
}

#[derive(Clone)]
enum Handler {
    Fn(usize),
    /// Не нашли объявление: путь как в коде, через точку
    Text(String),
}

/// Роут относительно роутера, в котором объявлен.
#[derive(Clone)]
struct Raw {
    method: String,
    path: String,
    handler: Option<Handler>,
    middleware: Vec<String>,
    file: usize,
    line: usize,
}

/// Параметры вызванной функции: имя → аргумент в месте вызова.
struct Frame<'t> {
    file: usize,
    /// Начало объявления функции
    decl: usize,
    args: HashMap<String, (usize, Node<'t>)>,
}

struct Eval<'i, 'f> {
    idx: &'i Index<'f>,
    /// Цепочки, вошедшие в другие роутеры: (файл, начало, конец)
    consumed: HashSet<(usize, usize, usize)>,
    /// Вызываемые сейчас функции — от рекурсии
    stack: Vec<usize>,
    frames: Vec<Frame<'f>>,
    warnings: Vec<String>,
}

/// Что дал метод-роутер: метод, обработчик, его middleware.
type MethodRoute = (String, Option<Handler>, Vec<String>);

impl<'f> Eval<'_, 'f> {
    fn src(&self, file: usize) -> &'f str {
        &self.idx.files[file].src
    }

    /// Строка пути: литерал, константа, переменная или параметр (`fn route(path: &str, …)`).
    fn string(&mut self, file: usize, node: Node<'f>) -> Option<String> {
        if let Some(s) = self.idx.string(file, node) {
            return Some(s);
        }
        if node.kind() != "identifier" {
            return None;
        }
        match self.bound(file, node)? {
            Bound::Local(v) => self.string(file, v),
            Bound::Param(f, v) => self.as_caller(|ev| ev.string(f, v)),
        }
    }

    fn router(&mut self, file: usize, node: Node<'f>, depth: usize) -> Vec<Raw> {
        if depth > MAX_DEPTH {
            return Vec::new();
        }
        let src = self.src(file);
        match node.kind() {
            "call_expression" => {
                if depth > 0 {
                    self.consumed
                        .insert((file, node.start_byte(), node.end_byte()));
                }
                let Some(func) = node.child_by_field_name("function") else {
                    return Vec::new();
                };
                let args = args(node);
                if func.kind() == "field_expression" {
                    let (Some(recv), Some(name)) = (
                        func.child_by_field_name("value"),
                        func.child_by_field_name("field"),
                    ) else {
                        return Vec::new();
                    };
                    let mut base = self.router(file, recv, depth + 1);
                    match text(name, src) {
                        "route" => {
                            let (Some(&p), Some(&m)) = (args.first(), args.get(1)) else {
                                return base;
                            };
                            let Some(path) = self.string(file, p) else {
                                self.warn(file, p, "route path is not a string literal");
                                return base;
                            };
                            let line = p.start_position().row + 1;
                            for (method, handler, middleware) in
                                self.method_router(file, m, depth + 1)
                            {
                                base.push(Raw {
                                    method,
                                    path: path.clone(),
                                    handler,
                                    middleware,
                                    file,
                                    line,
                                });
                            }
                        }
                        "nest" => {
                            let (Some(&p), Some(&r)) = (args.first(), args.get(1)) else {
                                return base;
                            };
                            let Some(prefix) = self.string(file, p) else {
                                self.warn(file, p, "nest path is not a string literal");
                                return base;
                            };
                            for mut r in self.router(file, r, depth + 1) {
                                r.path = join(&prefix, &r.path);
                                base.push(r);
                            }
                        }
                        "merge" => {
                            if let Some(&r) = args.first() {
                                base.extend(self.router(file, r, depth + 1));
                            }
                        }
                        "layer" | "route_layer" => {
                            if let Some(&l) = args.first() {
                                let mw = self.layers(file, l, depth + 1);
                                for r in &mut base {
                                    r.middleware.splice(0..0, mw.iter().cloned());
                                }
                            }
                        }
                        _ => {}
                    }
                    return base;
                }
                // `Router::new()`, `users::router(state)`, `routes(app)`
                let segs = segments(text(func, src));
                if segs.iter().any(|s| s == "Router") {
                    return Vec::new();
                }
                let module = self.idx.module_at(file, node);
                match self.idx.resolve(&self.idx.fns, file, &module, &segs) {
                    Some(f) => self.call(file, &args, f, depth, |ev, file, tail, depth| {
                        ev.router(file, tail, depth)
                    }),
                    None => Vec::new(),
                }
            }
            "identifier" => match self.bound(file, node) {
                Some(Bound::Local(v)) => self.router(file, v, depth + 1),
                Some(Bound::Param(f, v)) => self.as_caller(|ev| ev.router(f, v, depth + 1)),
                None => Vec::new(),
            },
            "parenthesized_expression"
            | "reference_expression"
            | "await_expression"
            | "try_expression" => node
                .named_child(0)
                .map(|n| {
                    let n = node.child_by_field_name("value").unwrap_or(n);
                    self.router(file, n, depth + 1)
                })
                .unwrap_or_default(),
            "block" => match tail(node) {
                Some(t) => self.router(file, t, depth + 1),
                None => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// Вызов функции `f` с аргументами `args` (в файле `file`): `eval` — над её результатом.
    fn call<T: Default>(
        &mut self,
        file: usize,
        args: &[Node<'f>],
        f: usize,
        depth: usize,
        eval: impl FnOnce(&mut Self, usize, Node<'f>, usize) -> T,
    ) -> T {
        if self.stack.contains(&f) || self.stack.len() > 16 {
            return T::default();
        }
        let item = &self.idx.fns[f];
        let Some(decl) = self.idx.node(item) else {
            return T::default();
        };
        let Some(body) = decl.child_by_field_name("body") else {
            return T::default();
        };
        let Some(result) = tail(body).or_else(|| first_return(body)) else {
            return T::default();
        };
        let fsrc = self.src(item.file);
        let mut params = HashMap::new();
        if let Some(ps) = decl.child_by_field_name("parameters") {
            let mut i = 0;
            for p in children(ps) {
                match p.kind() {
                    "parameter" => {
                        if let Some(pat) = p.child_by_field_name("pattern")
                            && pat.kind() == "identifier"
                            && let Some(&a) = args.get(i)
                        {
                            params.insert(text(pat, fsrc).to_string(), (file, a));
                        }
                        i += 1;
                    }
                    "self_parameter" => {}
                    _ => {}
                }
            }
        }
        self.stack.push(f);
        self.frames.push(Frame {
            file: item.file,
            decl: decl.start_byte(),
            args: params,
        });
        let out = eval(self, item.file, result, depth + 1);
        self.frames.pop();
        self.stack.pop();
        out
    }

    /// Вычислить аргумент вызывающей функции: её параметры — снаружи текущего кадра.
    fn as_caller<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let frame = self.frames.pop();
        let out = f(self);
        if let Some(frame) = frame {
            self.frames.push(frame);
        }
        out
    }

    /// Значение переменной в месте `ident`: последнее `let`/присваивание до него в той же
    /// функции, иначе параметр вызванной функции.
    fn bound(&self, file: usize, ident: Node<'f>) -> Option<Bound<'f>> {
        let src = self.src(file);
        let name = text(ident, src);
        let func = enclosing_fn(ident)?;
        let body = func.child_by_field_name("body")?;
        let at = ident.start_byte();
        let mut best: Option<Node> = None;
        walk_tree(body, &mut |n| {
            if n.end_byte() > at {
                return;
            }
            let value = match n.kind() {
                "let_declaration" => n
                    .child_by_field_name("pattern")
                    .filter(|p| p.kind() == "identifier" && text(*p, src) == name)
                    .and(n.child_by_field_name("value")),
                "assignment_expression" => n
                    .child_by_field_name("left")
                    .filter(|l| l.kind() == "identifier" && text(*l, src) == name)
                    .and(n.child_by_field_name("right")),
                _ => None,
            };
            if let Some(v) = value
                && best.is_none_or(|b| b.start_byte() < v.start_byte())
            {
                best = Some(v);
            }
        });
        if let Some(v) = best {
            return Some(Bound::Local(v));
        }
        let frame = self.frames.last()?;
        if frame.file != file || frame.decl != func.start_byte() {
            return None;
        }
        frame.args.get(name).map(|&(f, n)| Bound::Param(f, n))
    }

    /// `get(h).post(h2)`, `axum::routing::get(h)`, `on(MethodFilter::GET, h)`, `any(h)`.
    fn method_router(&mut self, file: usize, node: Node<'f>, depth: usize) -> Vec<MethodRoute> {
        if depth > MAX_DEPTH {
            return Vec::new();
        }
        let src = self.src(file);
        match node.kind() {
            "call_expression" => {
                let Some(func) = node.child_by_field_name("function") else {
                    return Vec::new();
                };
                let args = args(node);
                if func.kind() == "field_expression" {
                    let (Some(recv), Some(name)) = (
                        func.child_by_field_name("value"),
                        func.child_by_field_name("field"),
                    ) else {
                        return Vec::new();
                    };
                    let mut base = self.method_router(file, recv, depth + 1);
                    let name = text(name, src);
                    match name {
                        "layer" | "route_layer" => {
                            if let Some(&l) = args.first() {
                                let mw = self.layers(file, l, depth + 1);
                                for r in &mut base {
                                    r.2.splice(0..0, mw.iter().cloned());
                                }
                            }
                        }
                        _ => base.extend(self.method_call(file, name, &args)),
                    }
                    return base;
                }
                let segs = segments(text(func, src));
                let Some(name) = segs.last() else {
                    return Vec::new();
                };
                let routing = segs.len() == 1 || segs.iter().any(|s| s == "routing" || s == "axum");
                let own = if routing {
                    self.method_call(file, name, &args)
                } else {
                    Vec::new()
                };
                if !own.is_empty() {
                    return own;
                }
                // Своя функция, возвращающая MethodRouter.
                let module = self.idx.module_at(file, node);
                match self.idx.resolve(&self.idx.fns, file, &module, &segs) {
                    Some(f) => self.call(file, &args, f, depth, |ev, file, tail, depth| {
                        ev.method_router(file, tail, depth)
                    }),
                    None => Vec::new(),
                }
            }
            "identifier" => match self.bound(file, node) {
                Some(Bound::Local(v)) => self.method_router(file, v, depth + 1),
                Some(Bound::Param(f, v)) => self.as_caller(|ev| ev.method_router(f, v, depth + 1)),
                None => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// Один вызов-метод: `get(h)`, `post_service(s)`, `on(filter, h)`.
    fn method_call(&self, file: usize, name: &str, args: &[Node<'f>]) -> Vec<MethodRoute> {
        let src = self.src(file);
        if name == "on" || name == "on_service" {
            let (Some(&filter), Some(&h)) = (args.first(), args.get(1)) else {
                return Vec::new();
            };
            let handler = (name == "on").then(|| self.handler(file, h)).flatten();
            return filter_methods(text(filter, src))
                .into_iter()
                .map(|m| (m, handler.clone(), Vec::new()))
                .collect();
        }
        let (base, service) = match name.strip_suffix("_service") {
            Some(b) => (b, true),
            None => (name, false),
        };
        let Some(method) = http_method(base) else {
            return Vec::new();
        };
        let handler = if service {
            None
        } else {
            args.first().and_then(|&h| self.handler(file, h))
        };
        vec![(method, handler, Vec::new())]
    }

    fn handler(&self, file: usize, node: Node<'f>) -> Option<Handler> {
        let src = self.src(file);
        match node.kind() {
            "identifier" | "scoped_identifier" => {
                let segs = segments(text(node, src));
                let module = self.idx.module_at(file, node);
                Some(
                    match self.idx.resolve(&self.idx.fns, file, &module, &segs) {
                        Some(f) => Handler::Fn(f),
                        None => Handler::Text(
                            segs.into_iter()
                                .filter(|s| !matches!(s.as_str(), "crate" | "self" | "super"))
                                .collect::<Vec<_>>()
                                .join("."),
                        ),
                    },
                )
            }
            "generic_function" => self.handler(file, node.child_by_field_name("function")?),
            // `h.with_state(s)`, `h.layer(l)`
            "call_expression" => {
                let func = node.child_by_field_name("function")?;
                if func.kind() != "field_expression" {
                    return None;
                }
                self.handler(file, func.child_by_field_name("value")?)
            }
            "parenthesized_expression" | "reference_expression" => {
                self.handler(file, node.named_child(0)?)
            }
            _ => None,
        }
    }

    /// Имена middleware из аргумента `.layer(…)`, снаружи внутрь.
    fn layers(&mut self, file: usize, node: Node<'f>, depth: usize) -> Vec<String> {
        if depth > MAX_DEPTH {
            return Vec::new();
        }
        let src = self.src(file);
        match node.kind() {
            "call_expression" => {
                let Some(func) = node.child_by_field_name("function") else {
                    return Vec::new();
                };
                let args = args(node);
                if func.kind() == "field_expression" {
                    let Some(recv) = func.child_by_field_name("value") else {
                        return Vec::new();
                    };
                    let name = func
                        .child_by_field_name("field")
                        .map_or("", |f| text(f, src));
                    let mut out = self.layers(file, recv, depth + 1);
                    // ServiceBuilder::new().layer(a).layer(b): первым добавленный — снаружи.
                    if name == "layer"
                        && let Some(&l) = args.first()
                    {
                        out.extend(self.layers(file, l, depth + 1));
                    }
                    return out;
                }
                let segs = segments(text(func, src));
                let Some(last) = segs.last().map(String::as_str) else {
                    return Vec::new();
                };
                let arg_name = |i: usize| -> Option<String> {
                    let a = *args.get(i)?;
                    matches!(a.kind(), "identifier" | "scoped_identifier")
                        .then(|| dotted(text(a, src)))
                };
                let name = match last {
                    "from_fn" | "map_request" | "map_response" => arg_name(0),
                    "from_fn_with_state" | "map_request_with_state" | "map_response_with_state" => {
                        arg_name(1)
                    }
                    "from_extractor" | "from_extractor_with_state" => func
                        .child_by_field_name("type_arguments")
                        .and_then(|t| t.named_child(0))
                        .map(|t| last_segment(text(t, src))),
                    "new" if segs.len() >= 2 && segs[segs.len() - 2] == "ServiceBuilder" => {
                        return Vec::new();
                    }
                    _ if segs.len() >= 2
                        && segs[segs.len() - 2].starts_with(|c: char| c.is_ascii_uppercase()) =>
                    {
                        Some(segs[segs.len() - 2].clone())
                    }
                    _ => Some(segs.join(".")),
                };
                name.or_else(|| Some(segs.join("."))).into_iter().collect()
            }
            "identifier" => match self.bound(file, node) {
                Some(Bound::Local(v)) => self.layers(file, v, depth + 1),
                Some(Bound::Param(f, v)) => self.as_caller(|ev| ev.layers(f, v, depth + 1)),
                None => vec![text(node, src).to_string()],
            },
            "struct_expression" => node
                .child_by_field_name("name")
                .map(|n| vec![last_segment(text(n, src))])
                .unwrap_or_default(),
            "macro_invocation" => node
                .child_by_field_name("macro")
                .map(|m| vec![dotted(text(m, src))])
                .unwrap_or_default(),
            _ => vec![compact(text(node, src))],
        }
    }

    fn warn(&mut self, file: usize, node: Node, msg: &str) {
        let f = &self.idx.files[file];
        self.warnings.push(format!(
            "{}:{}: {msg}, skipped",
            f.path.to_string_lossy().replace('\\', "/"),
            node.start_position().row + 1
        ));
    }
}

enum Bound<'t> {
    /// Значение в той же функции
    Local(Node<'t>),
    /// Аргумент вызова: файл вызывающего и узел
    Param(usize, Node<'t>),
}

/// `get` → `GET`, `any` → [`ANY`].
fn http_method(name: &str) -> Option<String> {
    Some(match name {
        "get" | "post" | "put" | "delete" | "patch" | "head" | "options" | "trace" | "connect" => {
            name.to_ascii_uppercase()
        }
        "any" => ANY.to_string(),
        _ => return None,
    })
}

/// `MethodFilter::GET.or(MethodFilter::POST)` → `GET`, `POST`.
fn filter_methods(filter: &str) -> Vec<String> {
    const ALL: &[&str] = &[
        "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "TRACE", "CONNECT",
    ];
    let mut out = Vec::new();
    for word in filter.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if ALL.contains(&word) && !out.iter().any(|m| m == word) {
            out.push(word.to_string());
        }
    }
    out
}

/// `{*rest}` (axum 0.8) → `{rest}`.
fn wildcards(path: &str) -> String {
    path.replace("{*", "{")
}

/// `nest("/api", …)` + `/users` → `/api/users`; `/` внутри `nest` — сам префикс.
fn join(prefix: &str, path: &str) -> String {
    let p = prefix.trim_end_matches('/');
    match path.trim_start_matches('/') {
        "" => {
            if p.is_empty() {
                "/".into()
            } else {
                p.into()
            }
        }
        rest => format!("{p}/{rest}"),
    }
}

fn args<'t>(call: Node<'t>) -> Vec<Node<'t>> {
    call.child_by_field_name("arguments")
        .map(|a| {
            children(a)
                .filter(|n| {
                    !matches!(
                        n.kind(),
                        "line_comment" | "block_comment" | "attribute_item"
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Выражение-результат блока: последнее без `;`.
pub(super) fn tail(block: Node) -> Option<Node> {
    let last = children(block)
        .filter(|n| !matches!(n.kind(), "line_comment" | "block_comment"))
        .last()?;
    if last.kind().ends_with("_statement")
        || last.kind().ends_with("_declaration")
        || last.kind().ends_with("_item")
    {
        // `return x;` последней инструкцией
        if last.kind() == "expression_statement"
            && let Some(r) = last
                .named_child(0)
                .filter(|r| r.kind() == "return_expression")
        {
            return r.named_child(0);
        }
        return None;
    }
    Some(last)
}

fn first_return(body: Node) -> Option<Node> {
    let mut found = None;
    walk_tree(body, &mut |n| {
        if found.is_none() && n.kind() == "return_expression" {
            found = n.named_child(0);
        }
    });
    found
}

fn enclosing_fn(node: Node) -> Option<Node> {
    let mut n = node.parent();
    while let Some(p) = n {
        if p.kind() == "function_item" {
            return Some(p);
        }
        n = p.parent();
    }
    None
}

pub(super) fn children(node: Node) -> impl Iterator<Item = Node> {
    (0..node.named_child_count()).filter_map(move |i| node.named_child(i as u32))
}

pub(super) fn text<'a>(node: Node, src: &'a str) -> &'a str {
    &src[node.byte_range()]
}

pub(super) fn walk_tree<'t>(node: Node<'t>, f: &mut impl FnMut(Node<'t>)) {
    f(node);
    for c in children(node) {
        walk_tree(c, f);
    }
}

/// Содержимое строкового литерала (обычного или `r#"…"#`).
pub(super) fn string_literal(node: Node, src: &str) -> Option<String> {
    if !matches!(node.kind(), "string_literal" | "raw_string_literal") {
        return None;
    }
    Some(
        children(node)
            .filter(|c| matches!(c.kind(), "string_content" | "escape_sequence"))
            .map(|c| text(c, src))
            .collect(),
    )
}

/// Путь по `::` без обобщённых аргументов: `Router::<S>::new` → `Router`, `new`.
pub(super) fn segments(path: &str) -> Vec<String> {
    let mut clean = String::new();
    let mut depth = 0;
    for c in path.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            _ if depth == 0 && !c.is_whitespace() => clean.push(c),
            _ => {}
        }
    }
    clean
        .split("::")
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_start_matches("r#").to_string())
        .collect()
}

pub(super) fn last_segment(path: &str) -> String {
    segments(path).pop().unwrap_or_default()
}

/// `auth::require_user` → `auth.require_user`.
fn dotted(path: &str) -> String {
    segments(path).join(".")
}

pub(super) fn compact(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests;
