//! Импорт роутов из кода сервиса: находим роуты, сравниваем с `*.http` проекта
//! и создаём файлы только для отсутствующих. Существующие файлы не трогаем никогда.
//!
//! Роут и файл совпадают, если совпадает метод и путь URL после `{{base}}`
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
    /// Файлы с `{{base}}/…`, которым нет роута в коде
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
    let prefix = format!("{{{{{base}}}}}");
    let mut known = Vec::new();
    let mut taken: HashSet<PathBuf> = HashSet::new();
    for rel in discover::request_files(project_root)? {
        taken.insert(rel.clone());
        let Ok(src) = std::fs::read_to_string(project_root.join(&rel)) else {
            continue;
        };
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
        known.push((rel, req.method, req.url.clone(), key(path)));
    }

    let mut out = Plan {
        files: scan.files,
        warnings: scan.warnings,
        ..Plan::default()
    };
    let mut matched = HashSet::new();
    for route in scan.routes {
        let k = key(&route.path);
        let hit = known
            .iter()
            .enumerate()
            .filter(|(_, (_, m, _, kk))| *kk == k && (route.method == ANY || *m == route.method))
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        if let Some(&first) = hit.first() {
            matched.extend(hit.iter().copied());
            out.existing.push(Existing {
                file: known[first].0.clone(),
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
        .map(|(_, (file, method, url, _))| Stale { file, method, url })
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

/// `GET /users` → `users/get.http`, `GET /users/{{id}}` → `users/get-by-id.http`,
/// `GET /users/{{id}}/posts` → `users/posts/get.http`, `GET /` → `root/get.http`.
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
    path.push(format!("{name}.{}", discover::EXTENSION));
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

/// `users/get.http` занят → `users/get-2.http`, …
fn free_name(want: &Path, taken: &HashSet<PathBuf>) -> PathBuf {
    if !taken.contains(want) {
        return want.to_path_buf();
    }
    let stem = want.file_stem().unwrap_or_default().to_string_lossy();
    (2..)
        .map(|n| want.with_file_name(format!("{stem}-{n}.{}", discover::EXTENSION)))
        .find(|p| !taken.contains(p))
        .expect("infinite range")
}

/// Текст нового файла: что это за роут и что в него передавать — комментарием,
/// пример тела — телом запроса.
fn content(route: &Route, base: &str) -> String {
    let info = &route.info;
    let mut head = Vec::new();
    if let Some(s) = &info.summary {
        head.push(s.clone());
    }
    head.extend(info.description.iter().cloned());
    let source = route.source.to_string_lossy().replace('\\', "/");
    let mut from = format!("{} {source}:{}", route.router, route.line);
    if let Some(h) = &route.handler {
        from.push_str(&format!(" → {h}"));
    }
    head.push(from);
    if route.method == ANY {
        head.push("any method".into());
    }

    let mut params = Vec::new();
    let path_vars: Vec<&str> = route
        .path
        .split("{{")
        .skip(1)
        .filter_map(|s| s.split("}}").next())
        .collect();
    if !path_vars.is_empty() {
        params.push(format!("Path: {}", path_vars.join(", ")));
    }
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

    let mut s = String::new();
    for l in &head {
        s.push_str(&format!("# {l}\n"));
    }
    if !params.is_empty() {
        s.push_str("#\n");
        for l in &params {
            s.push_str(&format!("# {l}\n"));
        }
    }
    let method = if route.method == ANY {
        "GET"
    } else {
        &route.method
    };
    s.push_str(&format!("{method} {{{{{base}}}}}{}\n", route.path));
    match &info.body {
        Some(b) => s.push_str(&format!("\n{}\n", b.example)),
        None if matches!(method, "POST" | "PUT" | "PATCH") => s.push_str("\n{}\n"),
        None => {}
    }
    s
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
