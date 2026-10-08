//! Роуты из исходников Go: tree-sitter разбирает файлы, шаблоны-запросы из `queries/*.scm`
//! находят регистрацию роутов. Новый роутер — новый `.scm`, без кода на Rust.
//!
//! Захваты в запросах:
//!
//! - `@route` — вызов регистрации роута; `@path` — путь, `@method` — метод (имя функции `Get`/`GET`,
//!   строка `"GET"` или `http.MethodGet`; без него метод берётся из пути `"GET /x"`, иначе — любой),
//!   `@handler` — обработчик (для комментария), `@receiver` — роутер, на котором вызван.
//! - `@group` + `@group.path` — префикс. С `@group.var` префикс получает переменная
//!   (`v1 := r.Group("/v1")`), с `@group.body` — всё внутри блока (`r.Route("/x", func(r) {...})`).
//!   Вызов-группа без переменной работает и как `@receiver`: `r.Group("/v1").GET(...)`.
//! - `@mount` + `@mount.func` (+ `@mount.pkg`) — роуты функции с этим именем получают префикс
//!   `@receiver` и `@mount.path`: `r.Mount("/admin", admin.Routes())`, `users.Register(v1)`.
//!
//! Захваты, начинающиеся с `_`, — служебные (для предикатов). Строка `; routy: import <путь>`
//! ограничивает запрос файлами, импортирующими пакет с таким префиксом.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, Tree};

mod describe;

use super::{Route, Scan};
use crate::error::{Error, Result};

/// Встроенные роутеры: имя → запрос.
pub const BUILTIN: &[(&str, &str)] = &[
    ("chi", include_str!("../../queries/chi.scm")),
    ("gin", include_str!("../../queries/gin.scm")),
    ("net/http", include_str!("../../queries/nethttp.scm")),
];

const CAPTURES: &[&str] = &[
    "route",
    "path",
    "method",
    "handler",
    "receiver",
    "group",
    "group.path",
    "group.var",
    "group.body",
    "mount",
    "mount.func",
    "mount.pkg",
    "mount.path",
];

const SKIP_DIRS: &[&str] = &["vendor", "testdata", "node_modules"];

/// Методы, которые можно указать в `@method`. Остальное (`Any`, `Handle`) — любой метод.
const METHODS: &[&str] = &[
    "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "CONNECT", "TRACE",
];

pub struct RouterQuery {
    pub name: String,
    query: Query,
    imports: Vec<String>,
}

impl RouterQuery {
    pub fn new(name: &str, src: &str) -> Result<RouterQuery> {
        let query = Query::new(&language(), src)
            .map_err(|e| Error::Import(format!("query {name}: {e}")))?;
        for c in query.capture_names() {
            if !c.starts_with('_') && !CAPTURES.contains(c) {
                return Err(Error::Import(format!(
                    "query {name}: unknown capture @{c} (expected one of: {})",
                    CAPTURES.join(", ")
                )));
            }
        }
        let imports = src
            .lines()
            .filter_map(|l| l.trim().strip_prefix(';'))
            .filter_map(|l| l.trim().strip_prefix("routy:"))
            .filter_map(|l| l.trim().strip_prefix("import"))
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        Ok(RouterQuery {
            name: name.to_string(),
            query,
            imports,
        })
    }

    pub fn builtin() -> Vec<RouterQuery> {
        BUILTIN
            .iter()
            .map(|(name, src)| RouterQuery::new(name, src).expect("builtin query compiles"))
            .collect()
    }

    fn applies(&self, file: &GoFile) -> bool {
        self.imports.is_empty()
            || file
                .imports
                .iter()
                .any(|i| self.imports.iter().any(|p| i.starts_with(p.as_str())))
    }
}

fn language() -> Language {
    tree_sitter_go::LANGUAGE.into()
}

/// Все роуты в `*.go` под `dir` (без `_test.go`, `vendor/`, `testdata/`).
pub fn scan(dir: &Path, queries: &[RouterQuery]) -> Result<Scan> {
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
        files.push(GoFile::new(rel, src, tree));
    }
    Ok(Index::build(&files, queries).routes(&files))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).map_err(|e| Error::from(e).in_file(dir))? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name.starts_with('_') {
            continue;
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            if !SKIP_DIRS.contains(&name.as_ref()) {
                walk(&path, out)?;
            }
        } else if name.ends_with(".go") && !name.ends_with("_test.go") {
            out.push(path);
        }
    }
    Ok(())
}

struct GoFile {
    path: PathBuf,
    src: String,
    tree: Tree,
    package: String,
    imports: Vec<String>,
}

impl GoFile {
    fn new(path: PathBuf, src: String, tree: Tree) -> GoFile {
        let mut package = String::new();
        let mut imports = Vec::new();
        let root = tree.root_node();
        for node in children(root) {
            match node.kind() {
                "package_clause" => {
                    if let Some(id) = children(node).find(|n| n.kind() == "package_identifier") {
                        package = text(id, &src).to_string();
                    }
                }
                "import_declaration" => collect_imports(node, &src, &mut imports),
                _ => {}
            }
        }
        GoFile {
            path,
            src,
            tree,
            package,
            imports,
        }
    }
}

fn collect_imports(node: Node, src: &str, out: &mut Vec<String>) {
    if node.kind() == "import_spec" {
        if let Some(p) = node.child_by_field_name("path") {
            out.push(text(p, src).trim_matches(['"', '`']).to_string());
        }
        return;
    }
    for c in children(node) {
        collect_imports(c, src, out);
    }
}

fn children(node: Node) -> impl Iterator<Item = Node> {
    (0..node.named_child_count()).filter_map(move |i| node.named_child(i as u32))
}

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    &src[node.byte_range()]
}

/// Место в коде: файл, байтовый диапазон, объемлющая функция верхнего уровня.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Site {
    file: usize,
    start: usize,
    end: usize,
    func: Option<usize>,
}

/// На чём вызван метод: цепочка вызовов `r.With(mw).Group("/x")` (диапазоны, снаружи внутрь)
/// и переменная в её основании.
#[derive(Default)]
struct Receiver {
    calls: Vec<(usize, usize)>,
    var: Option<String>,
}

struct Func {
    file: usize,
    start: usize,
    end: usize,
    name: String,
    package: String,
}

struct Group {
    site: Site,
    recv: Receiver,
    path: String,
    var: Option<String>,
    body: Option<(usize, usize)>,
}

struct Mount {
    site: Site,
    recv: Receiver,
    path: String,
    func: String,
    pkg: Option<String>,
}

struct RawRoute {
    site: Site,
    recv: Receiver,
    method: Option<String>,
    path: String,
    handler: Option<String>,
    /// Байтовый диапазон выражения-обработчика — для разбора, что передавать
    handler_range: Option<(usize, usize)>,
    router: String,
    line: usize,
}

#[derive(Default)]
struct Index {
    /// Пути файлов относительно каталога сканирования.
    sources: Vec<PathBuf>,
    funcs: Vec<Func>,
    groups: Vec<Group>,
    mounts: Vec<Mount>,
    routes: Vec<RawRoute>,
    warnings: Vec<String>,
}

impl Index {
    fn build(files: &[GoFile], queries: &[RouterQuery]) -> Index {
        let mut ix = Index {
            sources: files.iter().map(|f| f.path.clone()).collect(),
            ..Index::default()
        };
        for (i, f) in files.iter().enumerate() {
            for node in children(f.tree.root_node()) {
                if matches!(node.kind(), "function_declaration" | "method_declaration")
                    && let Some(name) = node.child_by_field_name("name")
                {
                    ix.funcs.push(Func {
                        file: i,
                        start: node.start_byte(),
                        end: node.end_byte(),
                        name: text(name, &f.src).to_string(),
                        package: f.package.clone(),
                    });
                }
            }
        }
        let consts = collect_consts(files);
        let mut seen_routes = HashSet::new();
        let mut cursor = QueryCursor::new();
        for (i, f) in files.iter().enumerate() {
            for q in queries.iter().filter(|q| q.applies(f)) {
                let names = q.query.capture_names();
                let mut matches = cursor.matches(&q.query, f.tree.root_node(), f.src.as_bytes());
                while let Some(m) = matches.next() {
                    let mut cap: HashMap<&str, Node> = HashMap::new();
                    for c in m.captures {
                        cap.entry(names[c.index as usize]).or_insert(c.node);
                    }
                    let ctx = Ctx {
                        file: f,
                        consts: &consts,
                    };
                    if let Some(&node) = cap.get("route") {
                        let site = ix.site(i, node);
                        if seen_routes.insert((i, site.start)) {
                            ix.add_route(&ctx, site, &cap, &q.name);
                        }
                    } else if let Some(&node) = cap.get("group") {
                        let site = ix.site(i, node);
                        ix.add_group(&ctx, site, &cap);
                    } else if let Some(&node) = cap.get("mount") {
                        let site = ix.site(i, node);
                        ix.add_mount(&ctx, site, &cap);
                    }
                }
            }
        }
        ix
    }

    fn site(&self, file: usize, node: Node) -> Site {
        let start = node.start_byte();
        let func = self
            .funcs
            .iter()
            .position(|f| f.file == file && f.start <= start && start < f.end);
        Site {
            file,
            start,
            end: node.end_byte(),
            func,
        }
    }

    fn add_route(&mut self, ctx: &Ctx, site: Site, cap: &HashMap<&str, Node>, router: &str) {
        let Some(&path_node) = cap.get("path") else {
            return;
        };
        let line = path_node.start_position().row + 1;
        let Some(mut path) = ctx.eval(path_node) else {
            self.warnings.push(format!(
                "{}:{line}: can't evaluate route path `{}`",
                ctx.file.path.display(),
                text(path_node, &ctx.file.src)
            ));
            return;
        };
        let mut method = cap.get("method").and_then(|&n| ctx.method(n));
        // net/http: "GET /users/{id}"
        if let Some((m, rest)) = path.split_once(' ')
            && METHODS.contains(&m)
        {
            method.get_or_insert_with(|| m.to_string());
            path = rest.trim_start().to_string();
        }
        // Не роут: cache.Get("key", &v) и т.п.
        if !path.is_empty() && !path.starts_with('/') {
            return;
        }
        let handler = cap
            .get("handler")
            .filter(|n| n.kind() != "func_literal")
            .map(|&n| text(n, &ctx.file.src))
            .filter(|t| !t.contains('\n') && t.len() <= 80)
            .map(str::to_string);
        self.routes.push(RawRoute {
            site,
            recv: receiver(cap.get("receiver").copied(), &ctx.file.src),
            method,
            path,
            handler,
            handler_range: cap.get("handler").map(|n| (n.start_byte(), n.end_byte())),
            router: router.to_string(),
            line,
        });
    }

    fn add_group(&mut self, ctx: &Ctx, site: Site, cap: &HashMap<&str, Node>) {
        let Some(path) = cap.get("group.path").and_then(|&n| ctx.eval(n)) else {
            return;
        };
        let var = cap
            .get("group.var")
            .map(|&n| text(n, &ctx.file.src).to_string());
        let body = cap
            .get("group.body")
            .map(|n| (n.start_byte(), n.end_byte()));
        // Один и тот же вызов может попасть под два шаблона (с переменной и без) — сливаем.
        if let Some(g) = self.groups.iter_mut().find(|g| g.site == site) {
            g.var = g.var.take().or(var);
            g.body = g.body.or(body);
            return;
        }
        self.groups.push(Group {
            site,
            recv: receiver(cap.get("receiver").copied(), &ctx.file.src),
            path,
            var,
            body,
        });
    }

    fn add_mount(&mut self, ctx: &Ctx, site: Site, cap: &HashMap<&str, Node>) {
        let Some(func) = cap.get("mount.func").map(|&n| text(n, &ctx.file.src)) else {
            return;
        };
        let path = match cap.get("mount.path") {
            Some(&n) => match ctx.eval(n) {
                Some(p) => p,
                None => return,
            },
            None => String::new(),
        };
        let pkg = cap
            .get("mount.pkg")
            .filter(|n| n.kind() == "identifier")
            .map(|&n| text(n, &ctx.file.src).to_string());
        self.mounts.push(Mount {
            site,
            recv: receiver(cap.get("receiver").copied(), &ctx.file.src),
            path,
            func: func.to_string(),
            pkg,
        });
    }

    fn routes(self, files: &[GoFile]) -> Scan {
        let describer = describe::Describer::new(files, &self.funcs);
        let mut r = Resolver {
            ix: &self,
            memo: HashMap::new(),
        };
        let mut seen = HashSet::new();
        let mut routes = Vec::new();
        for raw in &self.routes {
            let info = raw
                .handler_range
                .map(|h| describer.describe(raw.site.file, h))
                .unwrap_or_default();
            for prefix in r.base(raw.site, &raw.recv, 0) {
                let path = super::normalize_route(&join(&prefix, &raw.path));
                let method = raw.method.clone().unwrap_or_else(|| super::ANY.to_string());
                if seen.insert((method.clone(), path.clone())) {
                    routes.push(Route {
                        method,
                        path,
                        source: self.sources[raw.site.file].clone(),
                        line: raw.line,
                        handler: raw.handler.clone(),
                        router: raw.router.clone(),
                        info: info.clone(),
                    });
                }
            }
        }
        routes.sort_by(|a, b| (&a.path, &a.method).cmp(&(&b.path, &b.method)));
        Scan {
            files: self.sources.len(),
            routes,
            warnings: self.warnings,
        }
    }
}

/// Префиксы роутов: группы, блоки `Route`, монтирование функций.
struct Resolver<'a> {
    ix: &'a Index,
    /// Префиксы функции; `None` — ещё считаются (рекурсия).
    memo: HashMap<usize, Option<Vec<String>>>,
}

const MAX_DEPTH: usize = 32;

impl Resolver<'_> {
    /// Префиксы для вызова на `recv` в точке `site`.
    fn base(&mut self, site: Site, recv: &Receiver, depth: usize) -> Vec<String> {
        if depth > MAX_DEPTH {
            return vec![String::new()];
        }
        // r.Group("/v1").GET(...)
        for &(start, end) in &recv.calls {
            if let Some(g) = self.ix.groups.iter().position(|g| {
                g.site.file == site.file && g.site.start == start && g.site.end == end
            }) {
                return self.full(g, depth + 1);
            }
        }
        if let Some(var) = &recv.var
            && let Some(g) = self.lookup_var(site, var)
        {
            return self.full(g, depth + 1);
        }
        if let Some(g) = self.scope(site) {
            return self.full(g, depth + 1);
        }
        match site.func {
            Some(f) => self.func_prefixes(f, depth + 1),
            None => vec![String::new()],
        }
    }

    fn full(&mut self, g: usize, depth: usize) -> Vec<String> {
        let g = &self.ix.groups[g];
        self.base(g.site, &g.recv, depth)
            .into_iter()
            .map(|p| join(&p, &g.path))
            .collect()
    }

    /// Группа в переменной `var` той же функции: последняя объявленная до `site`,
    /// иначе — объявленная после (`r.Mount("/admin", admin)` в конце функции).
    fn lookup_var(&self, site: Site, var: &str) -> Option<usize> {
        let candidates = self.ix.groups.iter().enumerate().filter(|(_, g)| {
            g.site.file == site.file
                && g.site.func == site.func
                && g.site.start != site.start
                && g.var.as_deref() == Some(var)
        });
        let (before, after): (Vec<_>, Vec<_>) =
            candidates.partition(|(_, g)| g.site.start < site.start);
        before.last().or(after.first()).map(|(i, _)| *i)
    }

    /// Самый внутренний блок `@group.body`, содержащий `site`.
    fn scope(&self, site: Site) -> Option<usize> {
        self.ix
            .groups
            .iter()
            .enumerate()
            .filter_map(|(i, g)| {
                let (s, e) = g.body?;
                (g.site.file == site.file && s <= site.start && site.end <= e).then_some((i, e - s))
            })
            .min_by_key(|&(_, len)| len)
            .map(|(i, _)| i)
    }

    fn func_prefixes(&mut self, f: usize, depth: usize) -> Vec<String> {
        match self.memo.get(&f) {
            Some(Some(v)) => return v.clone(),
            Some(None) => return vec![String::new()],
            None => {}
        }
        self.memo.insert(f, None);
        let func = &self.ix.funcs[f];
        let mut out = Vec::new();
        for (mi, m) in self.ix.mounts.iter().enumerate() {
            if m.func != func.name || m.site.func == Some(f) || !self.targets(mi).contains(&f) {
                continue;
            }
            let prefixes: Vec<String> = self
                .base(m.site, &m.recv, depth)
                .into_iter()
                .map(|p| join(&p, &m.path))
                .collect();
            // users.Register(r) без префикса ничего не меняет — и не дублирует роуты.
            for p in prefixes {
                if !p.is_empty() && p != "/" && !out.contains(&p) {
                    out.push(p);
                }
            }
        }
        if out.is_empty() {
            out.push(String::new());
        }
        self.memo.insert(f, Some(out.clone()));
        out
    }

    /// Функции, на которые указывает монтирование: по имени, при `pkg.Func` — из пакета `pkg`, если есть.
    fn targets(&self, mount: usize) -> Vec<usize> {
        let m = &self.ix.mounts[mount];
        let by_name: Vec<usize> = (0..self.ix.funcs.len())
            .filter(|&i| self.ix.funcs[i].name == m.func)
            .collect();
        if let Some(pkg) = &m.pkg {
            let in_pkg: Vec<usize> = by_name
                .iter()
                .copied()
                .filter(|&i| &self.ix.funcs[i].package == pkg)
                .collect();
            if !in_pkg.is_empty() {
                return in_pkg;
            }
        }
        by_name
    }
}

fn receiver(node: Option<Node>, src: &str) -> Receiver {
    let mut r = Receiver::default();
    let mut node = node;
    while let Some(n) = node {
        match n.kind() {
            "identifier" => {
                r.var = Some(text(n, src).to_string());
                break;
            }
            "call_expression" => {
                r.calls.push((n.start_byte(), n.end_byte()));
                node = n
                    .child_by_field_name("function")
                    .filter(|f| f.kind() == "selector_expression")
                    .and_then(|f| f.child_by_field_name("operand"));
            }
            "parenthesized_expression" => node = n.named_child(0),
            _ => break,
        }
    }
    r
}

/// `a` + `b` как пути: `/v1` + `/users` → `/v1/users`, `/v1` + `/` → `/v1/`.
fn join(a: &str, b: &str) -> String {
    if b.is_empty() {
        return a.to_string();
    }
    let b = b.strip_prefix('/').unwrap_or(b);
    format!("{}/{b}", a.trim_end_matches('/'))
}

/// Строковые константы пакетов: (пакет, имя) → значение.
type Consts = HashMap<(String, String), String>;

fn collect_consts(files: &[GoFile]) -> Consts {
    let empty = Consts::new();
    let mut out = Consts::new();
    for f in files {
        let ctx = Ctx {
            file: f,
            consts: &empty,
        };
        for decl in children(f.tree.root_node()).filter(|n| n.kind() == "const_declaration") {
            for spec in children(decl).filter(|n| n.kind() == "const_spec") {
                let names: Vec<Node> = {
                    let mut c = spec.walk();
                    spec.children_by_field_name("name", &mut c).collect()
                };
                let Some(values) = spec.child_by_field_name("value") else {
                    continue;
                };
                for (name, value) in names.iter().zip(children(values)) {
                    if let Some(v) = ctx.eval(value) {
                        out.insert((f.package.clone(), text(*name, &f.src).to_string()), v);
                    }
                }
            }
        }
    }
    out
}

struct Ctx<'a> {
    file: &'a GoFile,
    consts: &'a Consts,
}

impl Ctx<'_> {
    /// Значение строкового выражения: литералы, `+`, константы пакета.
    fn eval(&self, node: Node) -> Option<String> {
        let src = &self.file.src;
        match node.kind() {
            "interpreted_string_literal" => Some(unquote(text(node, src))),
            "raw_string_literal" => Some(text(node, src).trim_matches('`').to_string()),
            "parenthesized_expression" => self.eval(node.named_child(0)?),
            "binary_expression" => {
                let op = node.child_by_field_name("operator")?;
                if text(op, src) != "+" {
                    return None;
                }
                let l = self.eval(node.child_by_field_name("left")?)?;
                let r = self.eval(node.child_by_field_name("right")?)?;
                Some(l + &r)
            }
            "identifier" => self
                .consts
                .get(&(self.file.package.clone(), text(node, src).to_string()))
                .cloned(),
            "selector_expression" => {
                let pkg = node.child_by_field_name("operand")?;
                let name = node.child_by_field_name("field")?;
                self.consts
                    .get(&(text(pkg, src).to_string(), text(name, src).to_string()))
                    .cloned()
            }
            _ => None,
        }
    }

    /// `Get` / `GET` / `"GET"` / `http.MethodGet` → `GET`; `Any` и прочее → `None` (любой).
    fn method(&self, node: Node) -> Option<String> {
        let src = &self.file.src;
        let raw = match node.kind() {
            "interpreted_string_literal" | "raw_string_literal" => self.eval(node)?,
            "selector_expression" => text(node.child_by_field_name("field")?, src).to_string(),
            _ => text(node, src).to_string(),
        };
        let raw = raw
            .strip_prefix("Method")
            .filter(|s| !s.is_empty())
            .unwrap_or(&raw);
        let m = raw.to_ascii_uppercase();
        METHODS.contains(&m.as_str()).then_some(m)
    }
}

fn unquote(s: &str) -> String {
    let inner = s
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(c) => out.push(c),
            None => {}
        }
    }
    out
}
