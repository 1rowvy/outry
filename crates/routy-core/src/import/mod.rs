//! Импорт роутов из кода сервиса: находим роуты, сравниваем с запросами проекта (`*.routy` и
//! `*.http`) и создаём `*.routy` только для отсутствующих. Существующие файлы не трогаем никогда.
//!
//! Роут и запрос совпадают по `handler:`, а без него — по методу и пути после `base`
//! (параметры сравниваются как «любое значение»: `/users/{id}` ~ `{{base}}/users/{{user_id}}`).

pub mod go;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::discover;
use crate::error::{Error, Result};

/// Метод «любой» (`r.Handle`, gin `Any`): файл создаётся с GET, совпадает с любым методом.
pub const ANY: &str = "ANY";

/// Переменная с адресом сервиса в генерируемых файлах (как в `routy init`).
pub const DEFAULT_BASE: &str = "base";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Route {
    /// `GET`, `POST`, … или [`ANY`]
    pub method: String,
    /// Путь с параметрами в виде переменных: `/users/{{id}}`
    pub path: String,
    /// Файл с регистрацией роута, относительно каталога сканирования
    pub source: PathBuf,
    pub line: usize,
    pub handler: Option<String>,
    /// Какой шаблон нашёл роут: `chi`, `gin`, `net/http`, …
    pub router: String,
    /// Что передавать: из кода обработчика
    pub info: RouteInfo,
}

/// Описание роута из кода обработчика.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RouteInfo {
    /// Первая строка doc-комментария или `@Summary`
    pub summary: Option<String>,
    pub description: Vec<String>,
    /// Query-параметры, которые читает обработчик
    pub query: Vec<Field>,
    /// Заголовки, которые читает обработчик
    pub headers: Vec<String>,
    /// JSON-тело, в которое декодируется запрос
    pub body: Option<Body>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Body {
    /// Тип в Go: `dto.CreateUser`
    pub type_name: String,
    pub fields: Vec<Field>,
    /// Пример тела с пустыми значениями
    pub example: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Field {
    /// Имя в JSON / query
    pub name: String,
    /// Тип в Go
    pub ty: String,
    /// `binding:"required"` / `validate:"required"`
    pub required: bool,
    /// Комментарий к полю в структуре
    pub comment: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct Scan {
    /// Сколько файлов разобрано
    pub files: usize,
    pub routes: Vec<Route>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct Plan {
    pub files: usize,
    /// Роуты без файла — будут созданы
    pub new: Vec<NewFile>,
    /// Роуты, для которых файл уже есть
    pub existing: Vec<Existing>,
    /// Запросы к `base`, которым нет роута в коде
    pub stale: Vec<Stale>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct NewFile {
    pub route: Route,
    /// Относительно корня проекта (каталога с `env.toml`)
    pub file: PathBuf,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct Existing {
    pub route: Route,
    pub file: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct Stale {
    pub file: PathBuf,
    pub method: String,
    pub url: String,
}

/// Запрос проекта, с которым сравниваются роуты.
struct Known {
    file: PathBuf,
    method: String,
    url: String,
    key: String,
    handler: Option<String>,
}

/// Запросы из `*.http` (`{{base}}/…`) и `*.routy` (`/…`) проекта.
fn known_requests(
    project_root: &Path,
    base: &str,
    taken: &mut HashSet<PathBuf>,
) -> Result<Vec<Known>> {
    let prefix = format!("{{{{{base}}}}}");
    let mut known = Vec::new();
    let exts = [discover::EXTENSION, discover::ROUTY_EXTENSION];
    for rel in discover::files(project_root, &exts)? {
        // `get.http` занимает и `get.routy`: два файла с одним именем в дереве путают.
        taken.insert(rel.with_extension(discover::ROUTY_EXTENSION));
        taken.insert(rel.clone());
        let Ok(src) = std::fs::read_to_string(project_root.join(&rel)) else {
            continue;
        };
        if rel
            .extension()
            .is_some_and(|e| e == discover::ROUTY_EXTENSION)
        {
            let Ok(file) = crate::lang::parse::parse(&src, None) else {
                continue;
            };
            for item in &file.items {
                let crate::lang::ast::Item::Request(r) = item else {
                    continue;
                };
                use crate::lang::ast::{TargetKind, TargetPart};
                if r.target.kind != TargetKind::Path {
                    continue;
                }
                let mut path = String::new();
                for p in &r.target.parts {
                    match p {
                        TargetPart::Lit(l) => path.push_str(l),
                        _ => path.push_str("{{x}}"),
                    }
                }
                let path = path
                    .split(['?', '#'])
                    .next()
                    .unwrap_or_default()
                    .to_string();
                known.push(Known {
                    file: rel.clone(),
                    method: r.method.clone(),
                    url: r.target.span.text(&src).to_string(),
                    key: key(&path),
                    handler: r.fields.handler.clone(),
                });
            }
            continue;
        }
        let Ok(req) = crate::parse(&src) else {
            continue;
        };
        let Some(path) = req.url.strip_prefix(&prefix) else {
            continue;
        };
        let path = path.split(['?', '#']).next().unwrap_or_default();
        if !path.is_empty() && !path.starts_with('/') {
            continue; // {{base}}x — не наш случай
        }
        known.push(Known {
            file: rel,
            method: req.method,
            url: req.url.clone(),
            key: key(path),
            handler: None,
        });
    }
    Ok(known)
}

/// Сканирует Go-код в `src_dir` встроенными шаблонами и `extra` (имя, текст `.scm`)
/// и сравнивает с файлами проекта в `project_root`.
pub fn plan_go(
    src_dir: &Path,
    project_root: &Path,
    extra: &[(String, String)],
    base: &str,
) -> Result<Plan> {
    let mut queries = go::RouterQuery::builtin();
    for (name, src) in extra {
        queries.push(go::RouterQuery::new(name, src)?);
    }
    let scan = go::scan(src_dir, &queries)?;
    plan(project_root, scan, base)
}

/// Раскладывает найденные роуты на новые и уже существующие, находит файлы без роутов.
pub fn plan(project_root: &Path, scan: Scan, base: &str) -> Result<Plan> {
    let mut taken: HashSet<PathBuf> = HashSet::new();
    let known = known_requests(project_root, base, &mut taken)?;

    let mut out = Plan {
        files: scan.files,
        warnings: scan.warnings,
        ..Plan::default()
    };
    let mut matched = HashSet::new();
    for route in scan.routes {
        let k = key(&route.path);
        let by_handler: Vec<usize> = known
            .iter()
            .enumerate()
            .filter(|(_, r)| r.handler.is_some() && r.handler == route.handler)
            .map(|(i, _)| i)
            .collect();
        let hit = if by_handler.is_empty() {
            known
                .iter()
                .enumerate()
                .filter(|(_, r)| {
                    r.handler.is_none()
                        && r.key == k
                        && (route.method == ANY || r.method == route.method)
                })
                .map(|(i, _)| i)
                .collect::<Vec<_>>()
        } else {
            by_handler
        };
        if let Some(&first) = hit.first() {
            matched.extend(hit.iter().copied());
            out.existing.push(Existing {
                file: known[first].file.clone(),
                route,
            });
            continue;
        }
        let file = free_name(&file_name(&route), &taken);
        taken.insert(file.clone());
        out.new.push(NewFile {
            content: content(&route, base),
            file,
            route,
        });
    }
    out.stale = known
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !matched.contains(i))
        .map(|(_, k)| Stale {
            file: k.file,
            method: k.method,
            url: k.url,
        })
        .collect();
    Ok(out)
}

/// Создаёт файлы из `plan.new`. Уже существующие пропускает. Возвращает созданные пути.
pub fn apply(project_root: &Path, plan: &Plan) -> Result<Vec<PathBuf>> {
    let mut created = Vec::new();
    for f in &plan.new {
        let path = project_root.join(&f.file);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error::from(e).in_file(dir))?;
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                use std::io::Write;
                file.write_all(f.content.as_bytes())
                    .map_err(|e| Error::from(e).in_file(&path))?;
                created.push(f.file.clone());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(Error::from(e).in_file(&path)),
        }
    }
    Ok(created)
}

/// Путь роутера → путь с переменными: `{id}`, `{id:[0-9]+}`, `{path...}`, `:id`, `*path` → `{{id}}`.
/// Без завершающего `/` (кроме корня).
pub fn normalize_route(path: &str) -> String {
    let mut out = String::new();
    for seg in segments(path) {
        let seg = if let Some(name) = seg.strip_prefix(':') {
            format!("{{{{{name}}}}}")
        } else if let Some(name) = seg.strip_prefix('*') {
            let name = if name.is_empty() { "path" } else { name };
            format!("{{{{{name}}}}}")
        } else {
            braces_to_vars(seg)
        };
        if !seg.is_empty() {
            out.push('/');
            out.push_str(&seg);
        }
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

/// Сегменты пути по `/`, но не внутри `{…}` (в регулярках chi бывает `/`).
fn segments(path: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0usize, 0);
    for (i, c) in path.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            '/' if depth == 0 => {
                out.push(&path[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&path[start..]);
    out.into_iter().filter(|s| !s.is_empty()).collect()
}

/// `{id:[0-9]+}.json` → `{{id}}.json`; `{$}` (net/http «ровно этот путь») → пусто.
fn braces_to_vars(seg: &str) -> String {
    let mut out = String::new();
    let mut rest = seg;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let mut depth = 1;
        let close = after.char_indices().find_map(|(i, c)| {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(i)
        });
        let Some(close) = close else {
            out.push_str(&rest[open..]);
            return out;
        };
        let inner = &after[..close];
        let name = inner
            .split(':')
            .next()
            .unwrap_or(inner)
            .trim_end_matches("...");
        if name != "$" {
            out.push_str(&format!("{{{{{}}}}}", name.trim()));
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Ключ для сравнения: сегменты с переменными → `{}`, без завершающего `/`.
fn key(path: &str) -> String {
    let segs: Vec<&str> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| if s.contains("{{") { "{}" } else { s })
        .collect();
    format!("/{}", segs.join("/"))
}

/// `GET /users` → `users/get.routy`, `GET /users/{{id}}` → `users/get-by-id.routy`,
/// `GET /users/{{id}}/posts` → `users/posts/get.routy`, `GET /` → `root/get.routy`.
fn file_name(route: &Route) -> PathBuf {
    let segs: Vec<&str> = route.path.split('/').filter(|s| !s.is_empty()).collect();
    let last_static = segs.iter().rposition(|s| !s.contains("{{"));
    let mut path = PathBuf::new();
    for s in segs.iter().take(last_static.map_or(0, |i| i + 1)) {
        if !s.contains("{{") {
            path.push(sanitize(s));
        }
    }
    if path.as_os_str().is_empty() {
        path.push("root");
    }
    let method = if route.method == ANY {
        "get"
    } else {
        &route.method
    };
    let mut name = method.to_ascii_lowercase();
    let params: Vec<String> = segs
        .iter()
        .skip(last_static.map_or(0, |i| i + 1))
        .map(|s| sanitize(&s.replace("{{", "").replace("}}", "")))
        .collect();
    if !params.is_empty() {
        name.push_str("-by-");
        name.push_str(&params.join("-"));
    }
    path.push(format!("{name}.{}", discover::ROUTY_EXTENSION));
    path
}

fn sanitize(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    match out.trim_matches('.') {
        "" => "_".into(),
        s => s.into(),
    }
}

/// `users/get.routy` занят → `users/get-2.routy`, …
fn free_name(want: &Path, taken: &HashSet<PathBuf>) -> PathBuf {
    if !taken.contains(want) {
        return want.to_path_buf();
    }
    let stem = want.file_stem().unwrap_or_default().to_string_lossy();
    (2..)
        .map(|n| want.with_file_name(format!("{stem}-{n}.{}", discover::ROUTY_EXTENSION)))
        .find(|p| !taken.contains(p))
        .expect("infinite range")
}

/// Текст нового `*.routy`: имя и описание из doc-комментария обработчика, откуда роут, что в
/// него передавать; `handler:`, query-параметры как `params`, пример тела из структуры.
fn content(route: &Route, _base: &str) -> String {
    let info = &route.info;
    let func = route
        .handler
        .as_deref()
        .and_then(|h| h.rsplit('.').next())
        .filter(|f| is_go_ident(f));
    let (title, mut doc) = title(route, func);
    let source = route.source.to_string_lossy().replace('\\', "/");
    let mut from = format!("{} {source}:{}", route.router, route.line);
    if let Some(h) = &route.handler {
        from.push_str(&format!(" → {h}"));
    }
    doc.push(from);
    if route.method == ANY {
        doc.push("any method".into());
    }
    let mut params = Vec::new();
    if !info.query.is_empty() {
        params.push("Query:".into());
        params.extend(field_lines(&info.query));
    }
    if !info.headers.is_empty() {
        params.push(format!("Headers: {}", info.headers.join(", ")));
    }
    if let Some(b) = &info.body {
        params.push(format!("Body: {}", b.type_name));
        params.extend(field_lines(&b.fields));
    }

    let mut s = format!("// {title}\n");
    for l in &doc {
        s.push_str(&format!("// {l}\n"));
    }
    if !params.is_empty() {
        s.push_str("//\n");
        for l in &params {
            s.push_str(&format!("// {l}\n"));
        }
    }
    let method = if route.method == ANY {
        "GET"
    } else {
        &route.method
    };
    let path = route.path.replace("{{", "{").replace("}}", "}");
    let mut fields = Vec::new();
    if let Some(h) = &route.handler {
        if h.split('.').all(is_go_ident) {
            fields.push(format!("handler: {h}"));
        }
    }
    // Query — параметры запроса: `ListUsers(page: 2)`; `null` в query не уходит.
    let query: Vec<&str> = info
        .query
        .iter()
        .map(|f| f.name.as_str())
        .filter(|n| is_param(n) && !path.contains(&format!("{{{n}}}")))
        .collect();
    if !query.is_empty() {
        let defaults: Vec<String> = query.iter().map(|n| format!("{n}: null")).collect();
        fields.push(format!("params {{\n{}\n}}", defaults.join("\n")));
        fields.push(format!("query {{\n{}\n}}", query.join("\n")));
    }
    match &info.body {
        Some(b) => fields.push(format!("body {}", b.example)),
        None if matches!(method, "POST" | "PUT" | "PATCH") => fields.push("body {}".into()),
        None => {}
    }
    if fields.is_empty() {
        s.push_str(&format!("{method} {path}\n"));
    } else {
        s.push_str(&format!("{method} {path} {{\n{}\n}}\n", fields.join("\n")));
    }
    crate::lang::fmt::format(&s).unwrap_or(s)
}

/// Имя запроса и описание. `CreateUser creates a user.` (doc-комментарий Go) → `Create user`
/// и сам комментарий в описании; короткий `@Summary` — сам и есть имя.
fn title(route: &Route, func: Option<&str>) -> (String, Vec<String>) {
    let info = &route.info;
    let mut doc: Vec<String> = Vec::new();
    let summary = info
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let short = summary.filter(|s| {
        let words = s.split_whitespace().count();
        (1..=4).contains(&words)
            && !s.ends_with('.')
            && func.is_none_or(|f| !s.starts_with(&format!("{f} ")))
    });
    let title = match (short, func) {
        (Some(s), _) => s.to_string(),
        (None, Some(f)) => {
            doc.extend(summary.map(String::from));
            words(f)
        }
        (None, None) => {
            doc.extend(summary.map(String::from));
            let segs: Vec<&str> = route
                .path
                .split('/')
                .filter(|s| !s.is_empty() && !s.contains("{{"))
                .collect();
            let method = if route.method == ANY {
                "get"
            } else {
                &route.method
            };
            let mut t = method.to_ascii_lowercase();
            if let Some(last) = segs.last() {
                t.push(' ');
                t.push_str(last);
            }
            if route.path.ends_with("}}") {
                t.push_str(" by id");
            }
            capitalize(&t)
        }
    };
    doc.extend(info.description.iter().cloned());
    (title, doc)
}

/// `CreateUser` → `Create user`, `getHTTPStatus` → `Get http status`.
fn words(ident: &str) -> String {
    let chars: Vec<char> = ident.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
        let prev_lower = i > 0 && chars[i - 1].is_lowercase();
        if i > 0 && c.is_uppercase() && (prev_lower || next_lower) {
            out.push(' ');
        }
        if c == '_' {
            out.push(' ');
            continue;
        }
        out.extend(c.to_lowercase());
    }
    capitalize(
        out.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .as_str(),
    )
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn is_go_ident(s: &str) -> bool {
    s.starts_with(|c: char| c.is_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// Имя query-параметра годится в имя параметра запроса.
fn is_param(s: &str) -> bool {
    is_go_ident(s) && !crate::lang::parse::RESERVED.contains(&s)
}

/// Поля столбцами: `  name     string  required  ФИО`.
fn field_lines(fields: &[Field]) -> Vec<String> {
    let w_name = fields.iter().map(|f| f.name.len()).max().unwrap_or(0);
    let w_ty = fields
        .iter()
        .map(|f| f.ty.chars().count())
        .max()
        .unwrap_or(0);
    fields
        .iter()
        .map(|f| {
            let mut l = format!("  {:w_name$}  {:w_ty$}", f.name, f.ty);
            if f.required {
                l.push_str("  required");
            }
            if let Some(c) = &f.comment {
                l.push_str(&format!("  {c}"));
            }
            l.trim_end().to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests;
