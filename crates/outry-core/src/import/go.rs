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
//! - `@middleware` (сколько угодно) — middleware роута или группы: `v1.POST("/x", auth, h)`,
//!   `r.Group("/v1", auth)`. `@use` + `@receiver` + `@middleware` — подключение: отдельной
//!   инструкцией (`r.Use(auth)`) действует на роуты `@receiver` ниже в той же функции и том же
//!   блоке, в цепочке (`r.With(auth).Get(...)`) — только на этот вызов. Как и префиксы,
//!   middleware переходят в смонтированные функции. На каждый аргумент `(_)* @middleware`
//!   tree-sitter даёт отдельное совпадение — они сливаются по месту вызова.
//!
//! - Совпадения с одним `@route` (по началу узла) сливаются в один роут. Поэтому части роута
//!   можно искать отдельными шаблонами: звенья цепочки `ws.GET("/x").Doc("…").To(h)` начинаются
//!   там же, где `ws.GET`, а поля литерала `{Method: …, Path: …}` — в любом порядке.
//!   Роут получается, только если хоть одно совпадение дало `@path` (или `@include`).
//! - Типы из цепочки роута (go-restful): `@body` и `@response` — значение типа (`User{}`),
//!   `@query` и `@header` (сколько угодно) — имя параметра (строка или цепочка вызовов, тогда —
//!   первый аргумент самого внутреннего вызова). Перекрывают то, что найдено в обработчике.
//! - Контроллер вместо обработчика — роутов из одного вызова несколько. `@controller` — значение
//!   (`&UserController{}`, `new(T)`, переменная): с `@mapping` (`"get,post:Create;put:Update"`,
//!   `*` — любой метод) — по строке, без него — методы типа `Get`, `Post`, … (beego).
//!   `@include` (сколько угодно) — методы типа с комментарием `// @router /path [get,post]`.
//!   `@resource` — REST-ресурс buffalo: `List`, `Show`, `New`, `Create`, `Edit`, `Update`,
//!   `Destroy`, какие объявлены у типа; параметр — `{user_id}` из последнего сегмента пути.
//! - `@skip` + `@middleware` + `@skip.handler` (сколько угодно) — middleware не действует на
//!   роуты с этими обработчиками (`app.Middleware.Skip(auth, Home, ur.List)`).
//!
//! Захваты, начинающиеся с `_`, — служебные (для предикатов). Строка `; outry: import <путь>`
//! ограничивает запрос файлами, импортирующими пакет с таким префиксом (строк может быть несколько).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, Tree};

mod describe;

use super::{Route, RouteInfo, Scan};
use crate::error::{Error, Result};

/// Встроенные роутеры: имя → запрос.
pub const BUILTIN: &[(&str, &str)] = &[
    ("chi", include_str!("../../queries/chi.scm")),
    ("gin", include_str!("../../queries/gin.scm")),
    ("net/http", include_str!("../../queries/nethttp.scm")),
    ("go-zero", include_str!("../../queries/gozero.scm")),
    ("go-restful", include_str!("../../queries/restful.scm")),
    ("beego", include_str!("../../queries/beego.scm")),
    ("buffalo", include_str!("../../queries/buffalo.scm")),
];

/// Захваты, которых в одном совпадении (и роуте) может быть несколько.
const MULTI: &[&str] = &["middleware", "query", "header", "include", "skip.handler"];

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
    "middleware",
    "use",
    "body",
    "response",
    "query",
    "header",
    "controller",
    "mapping",
    "include",
    "resource",
    "skip",
    "skip.handler",
];

/// Действия REST-ресурса buffalo: метод типа → HTTP-метод и путь (`{}` — параметр).
const RESOURCE_ACTIONS: &[(&str, &str, &str)] = &[
    ("List", "GET", ""),
    ("New", "GET", "/new"),
    ("Create", "POST", ""),
    ("Show", "GET", "/{}"),
    ("Edit", "GET", "/{}/edit"),
    ("Update", "PUT", "/{}"),
    ("Destroy", "DELETE", "/{}"),
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
            .filter_map(|l| l.trim().strip_prefix("outry:"))
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
    /// Имена параметров по порядку (`_` — безымянный).
    params: Vec<String>,
    /// Тип получателя метода без `*`: `UserController`
    recv_type: Option<String>,
    /// `// @router /:id [get,post]` над методом (beego): путь, методы (пусто — любой), строка
    annotations: Vec<(String, Vec<String>, usize)>,
}

/// Что разбирать как обработчик роута.
#[derive(Clone, Copy)]
enum Target {
    /// Выражение в файле роута (байтовый диапазон)
    Expr(usize, usize),
    /// Функция или метод из `Index::funcs`
    Func(usize),
}

/// Тело, ответ, query и заголовки из цепочки роута (`.Reads(T{}).Writes(T{})`).
#[derive(Default)]
struct Declared {
    /// Байтовые диапазоны значений в файле роута
    body: Option<(usize, usize)>,
    response: Option<(usize, usize)>,
    query: Vec<String>,
    headers: Vec<String>,
}

/// `app.Middleware.Skip(auth, Home)`: middleware не действует на эти обработчики.
struct Skip {
    middleware: String,
    handlers: Vec<String>,
}

struct Group {
    site: Site,
    recv: Receiver,
    path: String,
    var: Option<String>,
    body: Option<(usize, usize)>,
    middleware: Vec<String>,
}

/// `r.Use(auth)` (`chain: false`) или `r.With(auth)` в цепочке вызовов (`chain: true`).
struct Use {
    site: Site,
    recv: Receiver,
    middleware: Vec<String>,
    chain: bool,
}

struct Mount {
    site: Site,
    recv: Receiver,
    path: String,
    func: String,
    pkg: Option<String>,
    /// Позиция `@receiver` среди аргументов — какому параметру функции он передан.
    arg: Option<usize>,
}

struct RawRoute {
    site: Site,
    recv: Receiver,
    method: Option<String>,
    path: String,
    handler: Option<String>,
    /// Обработчик — для разбора, что передавать
    target: Option<Target>,
    router: String,
    /// Где роут показывать: файл и строка (у `@router`-аннотаций — над методом контроллера)
    file: usize,
    line: usize,
    middleware: Vec<String>,
    declared: Declared,
}

/// Совпадения шаблонов с одним `@route` (по началу узла), слитые вместе.
struct RouteMatch<'q, 't> {
    router: &'q str,
    cap: HashMap<&'q str, Node<'t>>,
    multi: HashMap<&'q str, Vec<Node<'t>>>,
}

impl<'q, 't> RouteMatch<'q, 't> {
    fn merge(&mut self, cap: HashMap<&'q str, Node<'t>>, multi: HashMap<&'q str, Vec<Node<'t>>>) {
        // Узел роута — от совпадения с путём: по нему строка, область и порядок `Use`.
        let route = cap.get("path").and(cap.get("route")).copied();
        for (k, v) in cap {
            self.cap.entry(k).or_insert(v);
        }
        if let Some(r) = route {
            self.cap.insert("route", r);
        }
        for (k, nodes) in multi {
            let list = self.multi.entry(k).or_default();
            for n in nodes {
                if !list.iter().any(|m| m.id() == n.id()) {
                    list.push(n);
                }
            }
        }
    }

    fn list(&self, name: &str) -> &[Node<'t>] {
        self.multi.get(name).map_or(&[], Vec::as_slice)
    }
}

#[derive(Default)]
struct Index {
    /// Пути файлов относительно каталога сканирования.
    sources: Vec<PathBuf>,
    funcs: Vec<Func>,
    groups: Vec<Group>,
    mounts: Vec<Mount>,
    uses: Vec<Use>,
    routes: Vec<RawRoute>,
    skips: Vec<Skip>,
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
                        params: params(node, &f.src),
                        recv_type: recv_type(node, &f.src),
                        annotations: annotations(node, &f.src),
                    });
                }
            }
        }
        let consts = collect_consts(files);
        let mut cursor = QueryCursor::new();
        for (i, f) in files.iter().enumerate() {
            let ctx = Ctx {
                file: f,
                consts: &consts,
            };
            let mut pending: Vec<RouteMatch> = Vec::new();
            let mut by_start: HashMap<usize, usize> = HashMap::new();
            for q in queries.iter().filter(|q| q.applies(f)) {
                let names = q.query.capture_names();
                let mut matches = cursor.matches(&q.query, f.tree.root_node(), f.src.as_bytes());
                while let Some(m) = matches.next() {
                    let mut cap: HashMap<&str, Node> = HashMap::new();
                    let mut multi: HashMap<&str, Vec<Node>> = HashMap::new();
                    for c in m.captures {
                        let name = names[c.index as usize];
                        if MULTI.contains(&name) {
                            multi.entry(name).or_default().push(c.node);
                        } else {
                            cap.entry(name).or_insert(c.node);
                        }
                    }
                    let mw: Vec<String> = multi
                        .get("middleware")
                        .into_iter()
                        .flatten()
                        .filter_map(|&n| middleware_name(n, &f.src))
                        .collect();
                    if let Some(&node) = cap.get("route") {
                        match by_start.get(&node.start_byte()) {
                            Some(&k) => pending[k].merge(cap, multi),
                            None => {
                                by_start.insert(node.start_byte(), pending.len());
                                pending.push(RouteMatch {
                                    router: &q.name,
                                    cap,
                                    multi,
                                });
                            }
                        }
                    } else if cap.contains_key("skip") {
                        let handlers = multi
                            .get("skip.handler")
                            .into_iter()
                            .flatten()
                            .map(|&n| value_name(text(n, &f.src)))
                            .collect();
                        for m in mw {
                            ix.skips.push(Skip {
                                middleware: m,
                                handlers: Vec::clone(&handlers),
                            });
                        }
                    } else if let Some(&node) = cap.get("group") {
                        let site = ix.site(i, node);
                        ix.add_group(&ctx, site, &cap, mw);
                    } else if let Some(&node) = cap.get("use") {
                        let site = ix.site(i, node);
                        if let Some(u) = ix.uses.iter_mut().find(|u| u.site == site) {
                            add_all(&mut u.middleware, &mw);
                        } else if !mw.is_empty() {
                            ix.uses.push(Use {
                                site,
                                recv: receiver(cap.get("receiver").copied(), &f.src),
                                middleware: mw,
                                chain: node
                                    .parent()
                                    .is_none_or(|p| p.kind() != "expression_statement"),
                            });
                        }
                    } else if let Some(&node) = cap.get("mount") {
                        let site = ix.site(i, node);
                        ix.add_mount(&ctx, site, &cap);
                    }
                }
            }
            for m in pending {
                let site = ix.site(i, m.cap["route"]);
                ix.add_route(&ctx, site, &m);
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

    fn add_route(&mut self, ctx: &Ctx, site: Site, m: &RouteMatch) {
        let cap = &m.cap;
        let src = &ctx.file.src;
        let includes = m.list("include");
        let (mut path, line) = match cap.get("path") {
            Some(&path_node) => {
                let line = path_node.start_position().row + 1;
                let Some(path) = ctx.eval(path_node) else {
                    self.warnings.push(format!(
                        "{}:{line}: can't evaluate route path `{}`",
                        ctx.file.path.display(),
                        text(path_node, src)
                    ));
                    return;
                };
                (path, line)
            }
            // beego `web.NSInclude(&C{})`: пути — в аннотациях методов
            None if !includes.is_empty() => (String::new(), cap["route"].start_position().row + 1),
            None => return,
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
            .map(|&n| text(n, src))
            .filter(|t| !t.contains('\n') && t.len() <= 80)
            .map(str::to_string);
        let range = |n: &Node| (n.start_byte(), n.end_byte());
        let param = |n: &Node| ctx.eval(innermost_arg(*n)).filter(|p| !p.is_empty());
        let base = RawRoute {
            site,
            recv: receiver(cap.get("receiver").copied(), src),
            method,
            path: path.clone(),
            handler,
            target: cap
                .get("handler")
                .map(|n| Target::Expr(n.start_byte(), n.end_byte())),
            router: m.router.to_string(),
            file: site.file,
            line,
            middleware: m
                .list("middleware")
                .iter()
                .filter_map(|&n| middleware_name(n, src))
                .collect(),
            declared: Declared {
                body: cap.get("body").map(range),
                response: cap.get("response").map(range),
                query: m.list("query").iter().filter_map(param).collect(),
                headers: m.list("header").iter().filter_map(param).collect(),
            },
        };
        let expanded = if let Some(&c) = cap.get("controller") {
            let mapping = cap.get("mapping").and_then(|&n| ctx.eval(n));
            self.controller_routes(ctx, c, mapping.as_deref())
        } else if let Some(&r) = cap.get("resource") {
            self.resource_routes(ctx, r, &path)
        } else if !includes.is_empty() {
            includes
                .iter()
                .flat_map(|&c| self.annotated_routes(ctx, c))
                .collect()
        } else {
            self.routes.push(base);
            return;
        };
        for e in expanded {
            self.routes.push(RawRoute {
                site,
                recv: receiver(cap.get("receiver").copied(), src),
                method: e.method,
                path: join(&path, &e.path),
                handler: Some(e.handler),
                target: e.func.map(Target::Func),
                router: base.router.clone(),
                file: e.file.unwrap_or(site.file),
                line: e.line.unwrap_or(line),
                middleware: base.middleware.clone(),
                declared: Declared::default(),
            });
        }
    }

    /// Роуты контроллера beego: по строке `"get,post:Create;put:Update"` или методы `Get`, `Post`, ….
    fn controller_routes(&self, ctx: &Ctx, value: Node, mapping: Option<&str>) -> Vec<Expanded> {
        let Some(ty) = TypeRef::of(ctx, value) else {
            return Vec::new();
        };
        let methods = self.methods_of(&ty, &ctx.file.package);
        let find = |name: &str| {
            methods
                .iter()
                .copied()
                .find(|&f| self.funcs[f].name == name)
        };
        let mut out = Vec::new();
        match mapping {
            Some(mapping) => {
                for pair in mapping.split(';') {
                    let Some((verbs, name)) = pair.split_once(':') else {
                        continue;
                    };
                    let name = name.trim();
                    for verb in verbs.split(',').map(str::trim) {
                        out.push(Expanded::new(
                            verb_method(verb),
                            "",
                            ty.handler(name),
                            find(name),
                        ));
                    }
                }
            }
            None => {
                for &f in &methods {
                    let name = &self.funcs[f].name;
                    let m = name.to_ascii_uppercase();
                    if METHODS.contains(&m.as_str()) {
                        out.push(Expanded::new(Some(m), "", ty.handler(name), Some(f)));
                    }
                }
                // Тип не найден в коде — один роут на любой метод.
                if out.is_empty() {
                    out.push(Expanded::new(None, "", ty.label.clone(), None));
                }
            }
        }
        out
    }

    /// Методы контроллера с `// @router /path [get]` (beego `NSInclude`).
    fn annotated_routes(&self, ctx: &Ctx, value: Node) -> Vec<Expanded> {
        let Some(ty) = TypeRef::of(ctx, value) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for f in self.methods_of(&ty, &ctx.file.package) {
            let func = &self.funcs[f];
            for (path, verbs, line) in &func.annotations {
                let verbs: Vec<Option<String>> = if verbs.is_empty() {
                    vec![None]
                } else {
                    verbs.iter().map(|v| verb_method(v)).collect()
                };
                for method in verbs {
                    let mut e = Expanded::new(method, path, ty.handler(&func.name), Some(f));
                    e.file = Some(func.file);
                    e.line = Some(*line);
                    out.push(e);
                }
            }
        }
        out
    }

    /// REST-ресурс buffalo: действия, объявленные у типа.
    fn resource_routes(&self, ctx: &Ctx, value: Node, path: &str) -> Vec<Expanded> {
        let Some(ty) = TypeRef::of(ctx, value) else {
            return Vec::new();
        };
        let param = match singular(path) {
            s if s.is_empty() => "{id}".to_string(),
            s => format!("{{{s}_id}}"),
        };
        let methods = self.methods_of(&ty, &ctx.file.package);
        RESOURCE_ACTIONS
            .iter()
            .filter_map(|&(action, method, suffix)| {
                let f = methods
                    .iter()
                    .copied()
                    .find(|&f| self.funcs[f].name == action)?;
                Some(Expanded::new(
                    Some(method.to_string()),
                    &suffix.replace("{}", &param),
                    ty.handler(action),
                    Some(f),
                ))
            })
            .collect()
    }

    /// Методы типа: из пакета `ty.pkg` (или пакета файла), если там есть, иначе из любого.
    fn methods_of(&self, ty: &TypeRef, own: &str) -> Vec<usize> {
        let all: Vec<usize> = (0..self.funcs.len())
            .filter(|&f| self.funcs[f].recv_type.as_deref() == Some(ty.name.as_str()))
            .collect();
        let pkg = ty.pkg.as_deref().unwrap_or(own);
        let in_pkg: Vec<usize> = all
            .iter()
            .copied()
            .filter(|&f| self.funcs[f].package == pkg)
            .collect();
        if in_pkg.is_empty() { all } else { in_pkg }
    }

    fn add_group(
        &mut self,
        ctx: &Ctx,
        site: Site,
        cap: &HashMap<&str, Node>,
        middleware: Vec<String>,
    ) {
        // Без `@group.path` — группа только для middleware (chi `r.Group(func(r) {…})`).
        let path = match cap.get("group.path") {
            Some(&n) => match ctx.eval(n) {
                Some(p) => p,
                None => return,
            },
            None => String::new(),
        };
        let var = cap
            .get("group.var")
            .map(|&n| text(n, &ctx.file.src).to_string());
        let body = cap
            .get("group.body")
            .map(|n| (n.start_byte(), n.end_byte()));
        // Один и тот же вызов может попасть под несколько шаблонов (с переменной и без,
        // префикс и middleware отдельно) — сливаем.
        if let Some(g) = self.groups.iter_mut().find(|g| g.site == site) {
            if g.path.is_empty() {
                g.path = path;
            }
            g.var = g.var.take().or(var);
            g.body = g.body.or(body);
            add_all(&mut g.middleware, &middleware);
            return;
        }
        self.groups.push(Group {
            site,
            recv: receiver(cap.get("receiver").copied(), &ctx.file.src),
            path,
            var,
            body,
            middleware,
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
        let recv = cap.get("receiver").copied();
        let arg = recv.and_then(|n| {
            let list = n.parent().filter(|p| p.kind() == "argument_list")?;
            children(list)
                .filter(|c| c.kind() != "comment")
                .position(|c| c.id() == n.id())
        });
        self.mounts.push(Mount {
            site,
            recv: receiver(recv, &ctx.file.src),
            path,
            func: func.to_string(),
            pkg,
            arg,
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
            let mut info = match raw.target {
                Some(Target::Expr(s, e)) => describer.describe(raw.site.file, (s, e), &raw.router),
                Some(Target::Func(f)) => describer.describe_func(f, &raw.router),
                None => RouteInfo::default(),
            };
            let d = &raw.declared;
            if let Some(b) = d.body.and_then(|v| describer.body_of(raw.site.file, v)) {
                info.body = Some(b);
            }
            if let Some(resp) = d
                .response
                .and_then(|v| describer.response_of(raw.site.file, v))
            {
                info.response = Some(resp);
            }
            for q in &d.query {
                if !info.query.iter().any(|f| &f.name == q) {
                    info.query.push(super::Field {
                        name: q.clone(),
                        ty: "string".into(),
                        required: false,
                        comment: None,
                        json: None,
                    });
                }
            }
            add_all(&mut info.headers, &d.headers);
            for (prefix, mut middleware) in r.base(raw.site, &raw.recv, 0) {
                add_all(&mut middleware, &raw.middleware);
                if let Some(h) = &raw.handler {
                    let h = value_name(h);
                    middleware.retain(|m| {
                        !self
                            .skips
                            .iter()
                            .any(|s| &s.middleware == m && s.handlers.contains(&h))
                    });
                }
                let path = super::normalize_route(&join(&prefix, &raw.path));
                let method = raw.method.clone().unwrap_or_else(|| super::ANY.to_string());
                if seen.insert((method.clone(), path.clone())) {
                    routes.push(Route {
                        method,
                        path,
                        source: self.sources[raw.file].clone(),
                        line: raw.line,
                        handler: raw.handler.clone(),
                        router: raw.router.clone(),
                        info: info.clone(),
                        middleware,
                    });
                }
            }
        }
        routes.sort_by(|a, b| (&a.path, &a.method).cmp(&(&b.path, &b.method)));
        Scan {
            files: self.sources.len(),
            routes,
            shapes: describer.into_shapes(),
            warnings: self.warnings,
        }
    }
}

/// Префикс пути и middleware, через которые проходит роут.
type Base = (String, Vec<String>);

/// Префиксы и middleware роутов: группы, блоки `Route`, `Use`, монтирование функций.
struct Resolver<'a> {
    ix: &'a Index,
    /// Основания функции; `None` — ещё считаются (рекурсия).
    memo: HashMap<(usize, Option<usize>), Option<Vec<Base>>>,
}

const MAX_DEPTH: usize = 32;

impl Resolver<'_> {
    /// Префиксы и middleware для вызова на `recv` в точке `site`.
    fn base(&mut self, site: Site, recv: &Receiver, depth: usize) -> Vec<Base> {
        if depth > MAX_DEPTH {
            return vec![Base::default()];
        }
        let mut chain = Vec::new();
        // r.Group("/v1").GET(...), r.With(auth).Get(...)
        for &(start, end) in &recv.calls {
            let same = |s: &Site| s.file == site.file && s.start == start && s.end == end;
            if let Some(u) = self.ix.uses.iter().find(|u| u.chain && same(&u.site)) {
                chain.splice(0..0, u.middleware.iter().cloned());
            }
            if let Some(g) = self.ix.groups.iter().position(|g| same(&g.site)) {
                return with(self.full(g, depth + 1), &chain);
            }
        }
        // Сначала `r.Use(…)` выше по коду, потом `With` этого вызова.
        let mut local = Vec::new();
        if let Some(var) = &recv.var {
            for u in &self.ix.uses {
                if self.uses_apply(u, site, var) {
                    add_all(&mut local, &u.middleware);
                }
            }
        }
        add_all(&mut local, &chain);
        if let Some(var) = &recv.var
            && let Some(g) = self.lookup_var(site, var)
        {
            return with(self.full(g, depth + 1), &local);
        }
        let outer = if let Some(g) = self.scope(site) {
            self.full(g, depth + 1)
        } else {
            match site.func {
                Some(f) => {
                    // Получатель — параметр функции: основания только от аргумента на его месте
                    // (`Register(public, protected)`), а не от всех аргументов вызова.
                    let param = recv
                        .var
                        .as_ref()
                        .and_then(|v| self.ix.funcs[f].params.iter().position(|p| p == v));
                    self.func_bases(f, param, depth + 1)
                }
                None => vec![Base::default()],
            }
        };
        with(outer, &local)
    }

    fn full(&mut self, g: usize, depth: usize) -> Vec<Base> {
        let g = &self.ix.groups[g];
        self.base(g.site, &g.recv, depth)
            .into_iter()
            .map(|(p, mut mw)| {
                add_all(&mut mw, &g.middleware);
                (join(&p, &g.path), mw)
            })
            .collect()
    }

    /// `var.Use(…)` раньше `site` в той же функции и в блоке, где `site` (или на уровне функции).
    fn uses_apply(&self, u: &Use, site: Site, var: &str) -> bool {
        !u.chain
            && u.recv.calls.is_empty()
            && u.recv.var.as_deref() == Some(var)
            && u.site.file == site.file
            && u.site.func == site.func
            && u.site.end <= site.start
            && self.scope(u.site).is_none_or(|g| {
                self.ix.groups[g]
                    .body
                    .is_some_and(|(s, e)| s <= site.start && site.end <= e)
            })
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

    /// Основания функции `f` по её монтированиям; `param` — роуты на этом параметре.
    fn func_bases(&mut self, f: usize, param: Option<usize>, depth: usize) -> Vec<Base> {
        match self.memo.get(&(f, param)) {
            Some(Some(v)) => return v.clone(),
            Some(None) => return vec![Base::default()],
            None => {}
        }
        self.memo.insert((f, param), None);
        let func = &self.ix.funcs[f];
        let mut out = Vec::new();
        for (mi, m) in self.ix.mounts.iter().enumerate() {
            if m.func != func.name
                || m.site.func == Some(f)
                || (param.is_some() && m.arg.is_some() && m.arg != param)
                || !self.targets(mi).contains(&f)
            {
                continue;
            }
            let bases: Vec<Base> = self
                .base(m.site, &m.recv, depth)
                .into_iter()
                .map(|(p, mw)| (join(&p, &m.path), mw))
                .collect();
            // users.Register(r) без префикса и middleware ничего не меняет — и не дублирует роуты.
            for b in bases {
                if ((!b.0.is_empty() && b.0 != "/") || !b.1.is_empty()) && !out.contains(&b) {
                    out.push(b);
                }
            }
        }
        if out.is_empty() {
            out.push(Base::default());
        }
        self.memo.insert((f, param), Some(out.clone()));
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

/// Имена параметров функции или метода по порядку: `func F(a, b *gin.RouterGroup, _ int)`.
fn params(func: Node, src: &str) -> Vec<String> {
    let Some(list) = func.child_by_field_name("parameters") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for decl in children(list).filter(|c| c.kind() != "comment") {
        let mut cursor = decl.walk();
        let names: Vec<String> = decl
            .children_by_field_name("name", &mut cursor)
            .map(|n| text(n, src).to_string())
            .collect();
        if names.is_empty() {
            out.push("_".to_string());
        }
        out.extend(names);
    }
    out
}

/// Тип получателя метода без `*` и параметров типа: `func (c *UserController) Get()` → `UserController`.
fn recv_type(decl: Node, src: &str) -> Option<String> {
    let list = decl.child_by_field_name("receiver")?;
    let mut ty = children(list)
        .find(|c| c.kind() == "parameter_declaration")?
        .child_by_field_name("type")?;
    loop {
        ty = match ty.kind() {
            "pointer_type" | "parenthesized_type" => ty.named_child(0)?,
            "generic_type" => ty.child_by_field_name("type")?,
            "type_identifier" => return Some(text(ty, src).to_string()),
            _ => return None,
        };
    }
}

/// `// @router /:id [get,post]` в комментарии над функцией (beego): путь, методы, строка.
fn annotations(decl: Node, src: &str) -> Vec<(String, Vec<String>, usize)> {
    let mut out = Vec::new();
    let mut row = decl.start_position().row;
    let mut prev = decl.prev_sibling();
    while let Some(c) = prev.filter(|c| c.kind() == "comment" && c.end_position().row + 1 >= row) {
        for (i, l) in text(c, src).lines().enumerate() {
            let l = l.trim().trim_start_matches(['/', '*']).trim();
            let Some(rest) = l.strip_prefix("@router") else {
                continue;
            };
            let (path, verbs) = match rest.split_once('[') {
                Some((p, v)) => (p, v.split(']').next().unwrap_or("")),
                None => (rest, ""),
            };
            let verbs = verbs
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .collect();
            out.push((
                path.trim().to_string(),
                verbs,
                c.start_position().row + i + 1,
            ));
        }
        row = c.start_position().row;
        prev = c.prev_sibling();
    }
    out.reverse();
    out
}

/// Роут, развёрнутый из контроллера или ресурса.
struct Expanded {
    method: Option<String>,
    /// Путь относительно пути вызова
    path: String,
    handler: String,
    func: Option<usize>,
    /// Своё место (аннотация над методом), иначе — вызов
    file: Option<usize>,
    line: Option<usize>,
}

impl Expanded {
    fn new(method: Option<String>, path: &str, handler: String, func: Option<usize>) -> Self {
        Expanded {
            method,
            path: path.to_string(),
            handler,
            func,
            file: None,
            line: None,
        }
    }
}

/// Тип контроллера или ресурса из значения: `&controllers.UserController{}` →
/// пакет `controllers`, имя `UserController`; `label` — как значение записано (для `handler`).
struct TypeRef {
    pkg: Option<String>,
    name: String,
    label: String,
}

impl TypeRef {
    fn of(ctx: &Ctx, value: Node) -> Option<TypeRef> {
        let src = &ctx.file.src;
        let (pkg, name) = value_type_ref(value, src, enclosing_func(value), 0)?;
        Some(TypeRef {
            pkg,
            name,
            label: value_name(text(value, src)),
        })
    }

    fn handler(&self, method: &str) -> String {
        format!("{}.{method}", self.label)
    }
}

/// Тело функции (или литерала), в которой узел.
fn enclosing_func(node: Node) -> Option<Node> {
    let mut n = node.parent();
    while let Some(p) = n {
        if matches!(
            p.kind(),
            "function_declaration" | "method_declaration" | "func_literal"
        ) {
            return p.child_by_field_name("body");
        }
        n = p.parent();
    }
    None
}

/// Пакет и имя типа значения: `T{}`, `&pkg.T{}`, `new(T)`, переменная с таким значением в `scope`.
fn value_type_ref(
    v: Node,
    src: &str,
    scope: Option<Node>,
    depth: usize,
) -> Option<(Option<String>, String)> {
    if depth > 4 {
        return None;
    }
    match v.kind() {
        "unary_expression" | "parenthesized_expression" => value_type_ref(
            v.child_by_field_name("operand").or(v.named_child(0))?,
            src,
            scope,
            depth + 1,
        ),
        "composite_literal" => type_ref(v.child_by_field_name("type")?, src),
        "call_expression" if text(v.child_by_field_name("function")?, src) == "new" => {
            type_ref(v.child_by_field_name("arguments")?.named_child(0)?, src)
        }
        "identifier" => {
            let name = text(v, src);
            let mut found = None;
            find_var(scope?, name, src, &mut found);
            match found? {
                VarDef::Type(t) => type_ref(t, src),
                VarDef::Value(v) => value_type_ref(v, src, scope, depth + 1),
            }
        }
        _ => None,
    }
}

enum VarDef<'t> {
    Type(Node<'t>),
    Value(Node<'t>),
}

/// Первое объявление переменной `name`: `x := v`, `x = v`, `var x T`, `var x = v`.
fn find_var<'t>(node: Node<'t>, name: &str, src: &str, found: &mut Option<VarDef<'t>>) {
    if found.is_some() {
        return;
    }
    match node.kind() {
        "short_var_declaration" | "assignment_statement" => {
            if let (Some(l), Some(r)) = (
                node.child_by_field_name("left"),
                node.child_by_field_name("right"),
            ) && let Some(i) = children(l).position(|id| text(id, src) == name)
                && let Some(v) = children(r).nth(i)
            {
                *found = Some(VarDef::Value(v));
                return;
            }
        }
        "var_spec" => {
            let mut c = node.walk();
            if node
                .children_by_field_name("name", &mut c)
                .any(|id| text(id, src) == name)
            {
                *found = node
                    .child_by_field_name("type")
                    .map(VarDef::Type)
                    .or_else(|| {
                        node.child_by_field_name("value")
                            .and_then(|v| v.named_child(0))
                            .map(VarDef::Value)
                    });
                return;
            }
        }
        _ => {}
    }
    for c in children(node) {
        find_var(c, name, src, found);
    }
}

/// `T`, `*T`, `pkg.T` → пакет и имя.
fn type_ref(ty: Node, src: &str) -> Option<(Option<String>, String)> {
    match ty.kind() {
        "type_identifier" => Some((None, text(ty, src).to_string())),
        "qualified_type" => Some((
            Some(text(ty.child_by_field_name("package")?, src).to_string()),
            text(ty.child_by_field_name("name")?, src).to_string(),
        )),
        "pointer_type" | "parenthesized_type" => type_ref(ty.named_child(0)?, src),
        "generic_type" => type_ref(ty.child_by_field_name("type")?, src),
        _ => None,
    }
}

/// Значение как имя: `&pkg.T{}` → `pkg.T`, `T{}.List` → `T.List`; пробелы убраны.
fn value_name(s: &str) -> String {
    s.split_whitespace()
        .collect::<String>()
        .replace("{}", "")
        .trim_start_matches('&')
        .to_string()
}

/// Метод из mapping beego: `get` → `GET`, `*` и неизвестное → любой.
fn verb_method(verb: &str) -> Option<String> {
    let m = verb.trim().to_ascii_uppercase();
    METHODS.contains(&m.as_str()).then_some(m)
}

/// Имя параметра ресурса из последнего сегмента пути: `/users` → `user`, `/categories` → `category`.
fn singular(path: &str) -> String {
    let seg = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .replace('-', "_")
        .to_ascii_lowercase();
    if let Some(s) = seg.strip_suffix("ies") {
        format!("{s}y")
    } else if ["sses", "xes", "zes", "ches", "shes"]
        .iter()
        .any(|e| seg.ends_with(e))
    {
        seg[..seg.len() - 2].to_string()
    } else if seg.ends_with('s') && !seg.ends_with("ss") {
        seg[..seg.len() - 1].to_string()
    } else {
        seg
    }
}

/// Самый внутренний вызов цепочки — его первый аргумент: `ws.QueryParameter("page", "…").DataType("int")`
/// → `"page"`. Не вызов — сам узел.
fn innermost_arg(node: Node) -> Node {
    let mut n = node;
    loop {
        if n.kind() != "call_expression" {
            return n;
        }
        let inner = n
            .child_by_field_name("function")
            .filter(|f| f.kind() == "selector_expression")
            .and_then(|f| f.child_by_field_name("operand"))
            .filter(|o| o.kind() == "call_expression");
        match inner {
            Some(i) => n = i,
            None => {
                return n
                    .child_by_field_name("arguments")
                    .and_then(|a| a.named_child(0))
                    .unwrap_or(n);
            }
        }
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

/// Добавляет в `to` middleware из `from`, которых там ещё нет.
fn add_all(to: &mut Vec<String>, from: &[String]) {
    for m in from {
        if !to.contains(m) {
            to.push(m.clone());
        }
    }
}

/// Middleware `local` — после middleware каждого основания.
fn with(mut bases: Vec<Base>, local: &[String]) -> Vec<Base> {
    for (_, mw) in &mut bases {
        add_all(mw, local);
    }
    bases
}

/// Middleware как в коде, в одну строку: `auth`, `mw.RequireRole("admin")`. Функция-литерал
/// и слишком длинное выражение — не имя.
fn middleware_name(node: Node, src: &str) -> Option<String> {
    if node.kind() == "func_literal" {
        return None;
    }
    let text = text(node, src)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (text.len() <= 60).then_some(text)
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

    /// `Get` / `GET` / `"GET"` / `http.MethodGet` → `GET`, у beego ещё `NSGet`, `CtrlGet`,
    /// `NSCtrlGet`; `Any` и прочее → `None` (любой).
    fn method(&self, node: Node) -> Option<String> {
        let src = &self.file.src;
        let raw = match node.kind() {
            "interpreted_string_literal" | "raw_string_literal" => self.eval(node)?,
            "selector_expression" => text(node.child_by_field_name("field")?, src).to_string(),
            _ => text(node, src).to_string(),
        };
        let raw = ["Method", "NSCtrl", "Ctrl", "NS"]
            .iter()
            .find_map(|p| raw.strip_prefix(p).filter(|s| !s.is_empty()))
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
