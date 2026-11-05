//! Покрытие роутов запросами (`outry coverage`): какие ручки сервиса ничем не проверены.
//! Без HTTP, как `check`: роуты из кода, запросы проекта, их `expect`.
//!
//! Уровни роута: нет запроса → есть запрос, но без проверок → проверен (`expect` у самого
//! запроса или проверка его результата в сценарии / другом запросе). Коды ответа обработчика
//! (`c.JSON(404, …)`) отмечаются проверенными, если какой-то запрос роута проверяет
//! `status == 404` (или `status in [200, 404]`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{Known, Parsed, Project, Route, Scan, StatusCode, match_routes, scan_project};
use crate::discover;
use crate::error::Result;
use crate::lang::ast::{BinOp, Call, Expr, ExprKind, Item, Step};
use crate::lang::{ItemRef, Workspace};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Запроса к роуту нет
    None,
    /// Запрос есть, но ничего не проверяет
    Request,
    /// Есть запрос с `expect`
    Checked,
}

#[derive(Debug, Default, Serialize)]
pub struct Coverage {
    /// Сколько файлов Go разобрано
    pub files: usize,
    pub routes: Vec<RouteCoverage>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RouteCoverage {
    pub method: String,
    /// Путь как в коде: `/users/{id}`
    pub path: String,
    /// Регистрация роута, относительно каталога сканирования
    pub source: PathBuf,
    pub line: usize,
    pub handler: Option<String>,
    pub level: Level,
    /// Запросы роута; первым — тот, где `handler:`
    pub requests: Vec<Covering>,
    /// Коды ответа обработчика
    pub statuses: Vec<StatusCoverage>,
}

/// Запрос, который ходит в роут.
#[derive(Debug, Serialize)]
pub struct Covering {
    /// Относительно корня проекта
    pub file: PathBuf,
    pub line: usize,
    pub name: Option<String>,
    pub checked: bool,
}

#[derive(Debug, Serialize)]
pub struct StatusCoverage {
    #[serde(flatten)]
    pub status: StatusCode,
    pub checked: bool,
}

impl Coverage {
    /// Роутов на уровне `level` и выше.
    pub fn at_least(&self, level: Level) -> usize {
        self.routes.iter().filter(|r| r.level >= level).count()
    }

    /// Доля проверенных роутов в процентах; без роутов — 100.
    pub fn percent(&self) -> f64 {
        if self.routes.is_empty() {
            return 100.0;
        }
        self.at_least(Level::Checked) as f64 * 100.0 / self.routes.len() as f64
    }

    /// (проверено, всего) кодов ответа по всем роутам.
    pub fn statuses(&self) -> (usize, usize) {
        let all = self.routes.iter().flat_map(|r| &r.statuses);
        let total = all.clone().count();
        (all.filter(|s| s.checked).count(), total)
    }
}

/// Покрытие роутов `scan` запросами проекта в `project_root`.
pub fn coverage(project_root: &Path, scan: Scan, base: &str) -> Result<Coverage> {
    coverage_with(project_root, scan, base, &HashMap::new())
}

/// [`coverage`] по текстам из редактора: файлы из `overlay` берутся оттуда, а не с диска.
pub fn coverage_with(
    project_root: &Path,
    scan: Scan,
    base: &str,
    overlay: &HashMap<PathBuf, String>,
) -> Result<Coverage> {
    let Project {
        known,
        sources,
        warnings,
        ..
    } = scan_project(project_root, base, overlay)?;
    let outry: Vec<(PathBuf, String)> = sources
        .into_iter()
        .filter(|(p, _)| {
            p.extension()
                .is_some_and(|e| e == discover::OUTRY_EXTENSION)
        })
        .collect();
    let ws = Workspace::from_sources(project_root, outry);
    let checks = workspace_checks(&ws);
    let by_line: HashMap<(&Path, usize), ItemRef> = ws
        .sources
        .iter()
        .enumerate()
        .flat_map(|(fi, s)| {
            s.file
                .items
                .iter()
                .enumerate()
                .filter_map(move |(ii, item)| match item {
                    Item::Request(r) => Some((
                        (s.path.as_path(), s.line(r.span.start)),
                        ItemRef { file: fi, item: ii },
                    )),
                    _ => None,
                })
        })
        .collect();
    let empty = Checks::default();
    let known_checks: Vec<&Checks> = known
        .iter()
        .map(|k| match &k.parsed {
            Parsed::Outry(..) => by_line
                .get(&(k.file.as_path(), k.line))
                .and_then(|r| checks.get(r))
                .unwrap_or(&empty),
            Parsed::Http(..) => &empty,
        })
        .collect();
    let http_checks: Vec<Option<Checks>> = known.iter().map(http_checks).collect();

    let hits = match_routes(&scan.routes, &known);
    let routes = scan
        .routes
        .into_iter()
        .zip(hits)
        .map(|(route, hit)| {
            let of = |i: usize| http_checks[i].as_ref().unwrap_or(known_checks[i]);
            let requests: Vec<Covering> = hit
                .iter()
                .map(|&i| Covering {
                    file: known[i].file.clone(),
                    line: known[i].line,
                    name: match &known[i].parsed {
                        Parsed::Outry(r, _) => r.name.clone(),
                        Parsed::Http(..) => None,
                    },
                    checked: of(i).checked,
                })
                .collect();
            let level = if requests.is_empty() {
                Level::None
            } else if requests.iter().any(|r| r.checked) {
                Level::Checked
            } else {
                Level::Request
            };
            let statuses = route
                .info
                .statuses
                .iter()
                .map(|s| StatusCoverage {
                    checked: hit.iter().any(|&i| of(i).codes.contains(&s.code)),
                    status: s.clone(),
                })
                .collect();
            route_coverage(route, level, requests, statuses)
        })
        .collect();
    Ok(Coverage {
        files: scan.files,
        routes,
        warnings: scan.warnings.into_iter().chain(warnings).collect(),
    })
}

fn route_coverage(
    route: Route,
    level: Level,
    requests: Vec<Covering>,
    statuses: Vec<StatusCoverage>,
) -> RouteCoverage {
    RouteCoverage {
        path: route.path.replace("{{", "{").replace("}}", "}"),
        method: route.method,
        source: route.source,
        line: route.line,
        handler: route.handler,
        level,
        requests,
        statuses,
    }
}

/// Что проверяется про ответ запроса.
#[derive(Debug, Default)]
struct Checks {
    checked: bool,
    /// Коды из `status == 404`, `status in [200, 404]`
    codes: HashSet<u16>,
}

/// `.http`: `> assert …`, коды — из `> assert status == 404`.
fn http_checks(k: &Known) -> Option<Checks> {
    use crate::expr::{Op, Root};
    use crate::parser::Directive;
    let Parsed::Http(req, _) = &k.parsed else {
        return None;
    };
    let mut out = Checks::default();
    for d in &req.directives {
        if let Directive::Assert(a) = d {
            out.checked = true;
            if a.path.root == Root::Status
                && a.op == Op::Eq
                && let Some(code) = a.expected.as_u64().and_then(|c| u16::try_from(c).ok())
            {
                out.codes.insert(code);
            }
        }
    }
    Some(out)
}

/// Проверки всех запросов проекта: `expect` самого запроса, а ещё вызовы в `expect` других
/// запросов и сценариев (`GetOrder(id: x).body.status == "new"`) и результаты шагов сценария
/// (`order = CreateOrder()` … `expect { order.status == 201 }`).
fn workspace_checks(ws: &Workspace) -> HashMap<ItemRef, Checks> {
    let mut out: HashMap<ItemRef, Checks> = HashMap::new();
    let resolve = |c: &Call| ws.resolve(&c.path).ok();
    for (fi, src) in ws.sources.iter().enumerate() {
        for (ii, item) in src.file.items.iter().enumerate() {
            let this = ItemRef { file: fi, item: ii };
            let mut bound: HashMap<&str, ItemRef> = HashMap::new();
            let exprs: Vec<&Expr> = match item {
                Item::Request(r) => {
                    if !r.fields.expect.is_empty() {
                        out.entry(this).or_default().checked = true;
                    }
                    r.fields.expect.iter().collect()
                }
                Item::Flow(f) => {
                    let mut exprs = Vec::new();
                    for step in &f.steps {
                        match step {
                            Step::Bind { name, value, .. } => {
                                if let ExprKind::Call(c) = &value.kind
                                    && let Some(r) = resolve(c)
                                {
                                    bound.insert(name, r);
                                } else {
                                    bound.remove(name.as_str());
                                }
                            }
                            Step::Expect(es, _) => exprs.extend(es),
                            _ => {}
                        }
                    }
                    exprs
                }
                _ => continue,
            };
            let own = matches!(item, Item::Request(_)).then_some(this);
            // Чей ответ это выражение: `status` — свой, `order.status`, `Login().status`.
            let subject = |e: &Expr| -> Option<ItemRef> {
                match &e.kind {
                    ExprKind::Ident(n) if n == "status" => own,
                    ExprKind::Member(base, field) if field == "status" => match &base.kind {
                        ExprKind::Ident(n) => bound.get(n.as_str()).copied(),
                        ExprKind::Call(c) => resolve(c),
                        _ => None,
                    },
                    _ => None,
                }
            };
            for e in exprs {
                crate::lang::visit(e, &mut |x| match &x.kind {
                    ExprKind::Call(c) => {
                        if let Some(r) = resolve(c) {
                            out.entry(r).or_default().checked = true;
                        }
                    }
                    ExprKind::Ident(n) => {
                        if let Some(&r) = bound.get(n.as_str()) {
                            out.entry(r).or_default().checked = true;
                        }
                    }
                    ExprKind::Binary(op, a, b) => {
                        for (subj, value) in [(a, b), (b, a)] {
                            let Some(r) = subject(subj) else {
                                continue;
                            };
                            let codes = match (op, &value.kind) {
                                (BinOp::Eq, ExprKind::Num(n)) => vec![*n],
                                (BinOp::In, ExprKind::Array(items)) if std::ptr::eq(subj, a) => {
                                    items
                                        .iter()
                                        .filter_map(|i| match i.kind {
                                            ExprKind::Num(n) => Some(n),
                                            _ => None,
                                        })
                                        .collect()
                                }
                                _ => continue,
                            };
                            out.entry(r)
                                .or_default()
                                .codes
                                .extend(codes.into_iter().filter_map(code));
                        }
                    }
                    _ => {}
                });
            }
        }
    }
    out
}

fn code(n: f64) -> Option<u16> {
    (n.fract() == 0.0 && (100.0..600.0).contains(&n)).then_some(n as u16)
}
