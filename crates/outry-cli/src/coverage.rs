//! `outry coverage`: таблица покрытия роутов, порог `--min` для CI.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, bail};
use outry_core::import::{self, Coverage, Level, RouteCoverage};
use serde::Serialize;

use super::{ImportDiag, Style, count, find_project, go_module, print_github, relative};

#[derive(clap::Args)]
pub struct Args {
    /// Каталог Go-сервиса (по умолчанию — где go.mod, вверх от проекта до .git)
    dir: Option<PathBuf>,
    /// Код выхода 1, если проверенных роутов меньше N процентов (для CI)
    #[arg(long, value_name = "N")]
    min: Option<f64>,
    /// Показать только дыры: роуты без запроса, без `expect` и с непроверенными кодами ответа
    #[arg(long)]
    uncovered: bool,
    /// Каталог проекта с env.toml (по умолчанию ищется вверх от текущего)
    #[arg(long)]
    project: Option<PathBuf>,
    /// Переменная с адресом сервиса в старых *.http: {{base}}/users (для сопоставления)
    #[arg(long, default_value = import::DEFAULT_BASE)]
    base: String,
    /// Свой шаблон-запрос tree-sitter для роутера (можно несколько раз)
    #[arg(long = "query", value_name = "FILE.scm")]
    queries: Vec<PathBuf>,
    /// Формат вывода: text, github (аннотации GitHub Actions), json
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
    /// То же, что --format json
    #[arg(long)]
    json: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum Format {
    Text,
    Github,
    Json,
}

pub fn run(args: Args) -> anyhow::Result<ExitCode> {
    let Args {
        dir,
        min,
        uncovered,
        project,
        base,
        queries,
        format,
        json,
    } = args;
    let format = if json { Format::Json } else { format };
    if let Some(m) = min
        && !(0.0..=100.0).contains(&m)
    {
        bail!("--min is a percentage from 0 to 100, got {m}");
    }
    let project = find_project(project.as_deref(), Path::new("."))?;
    if !project.has_config() {
        bail!(
            "no env.toml found from {}; run `outry init` first",
            project.root.display()
        );
    }
    let dir = match dir {
        Some(d) => d,
        None => go_module(&project.root).with_context(|| {
            format!(
                "no go.mod found from {} up to the repository root; pass the Go directory: \
                 `outry coverage path/to/service`",
                project.root.display()
            )
        })?,
    };
    let extra = queries
        .iter()
        .map(|q| {
            let src = std::fs::read_to_string(q).with_context(|| q.display().to_string())?;
            Ok((q.display().to_string(), src))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let scan = import::scan_go(&dir, &extra)?;
    let cov = import::coverage(&project.root, scan, &base)?;
    let below = min.filter(|&m| cov.percent() < m);

    let cwd = std::env::current_dir().unwrap_or_default();
    let rel = |p: &Path| relative(p, &cwd).display().to_string().replace('\\', "/");
    let shown = |p: &Path| rel(&project.root.join(p));
    let go = |p: &Path| rel(&dir.join(p));
    let rows: Vec<&RouteCoverage> = cov
        .routes
        .iter()
        .filter(|r| !uncovered || is_hole(r))
        .collect();

    match format {
        Format::Json => {
            #[derive(Serialize)]
            struct Out<'a> {
                percent: f64,
                checked: usize,
                requested: usize,
                total: usize,
                statuses_checked: usize,
                statuses_total: usize,
                min: Option<f64>,
                passed: bool,
                files: usize,
                routes: Vec<&'a RouteCoverage>,
                warnings: &'a [String],
            }
            let (statuses_checked, statuses_total) = cov.statuses();
            let out = Out {
                percent: (cov.percent() * 10.0).round() / 10.0,
                checked: cov.at_least(Level::Checked),
                requested: cov.at_least(Level::Request),
                total: cov.routes.len(),
                statuses_checked,
                statuses_total,
                min,
                passed: below.is_none(),
                files: cov.files,
                routes: rows,
                warnings: &cov.warnings,
            };
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        Format::Github => {
            print_github(&diagnostics(&rows, &shown, &go), "outry coverage");
            for w in &cov.warnings {
                println!("::warning title=outry coverage::{}", w.replace('\n', "%0A"));
            }
            if let Some(m) = below {
                println!(
                    "::error title=outry coverage::{} is below --min {m}%",
                    summary(&cov)
                );
            }
        }
        Format::Text => print_table(&cov, &rows, &shown, &go, below),
    }
    Ok(if below.is_some() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// Дыра: роут без проверок или с кодом ответа, который никто не проверяет.
fn is_hole(r: &RouteCoverage) -> bool {
    r.level < Level::Checked || r.statuses.iter().any(|s| !s.checked)
}

/// `3/5 routes checked (60%)`.
pub fn summary(cov: &Coverage) -> String {
    format!(
        "{}/{} routes checked ({})",
        cov.at_least(Level::Checked),
        cov.routes.len(),
        percent(cov.percent())
    )
}

fn percent(p: f64) -> String {
    let rounded = (p * 10.0).round() / 10.0;
    if rounded.fract() == 0.0 {
        format!("{rounded:.0}%")
    } else {
        format!("{rounded:.1}%")
    }
}

fn label(level: Level) -> &'static str {
    match level {
        Level::None => "no request",
        Level::Request => "no expect",
        Level::Checked => "checked",
    }
}

fn print_table(
    cov: &Coverage,
    rows: &[&RouteCoverage],
    shown: &dyn Fn(&Path) -> String,
    go: &dyn Fn(&Path) -> String,
    below: Option<f64>,
) {
    let st = Style::stdout();
    let place = |r: &RouteCoverage| match r.requests.first() {
        Some(q) => format!("{}:{}", shown(&q.file), q.line),
        None => format!("{}:{}", go(&r.source), r.line),
    };
    let codes = |r: &RouteCoverage| {
        let done = r.statuses.iter().filter(|s| s.checked).count();
        format!("{done}/{}", r.statuses.len())
    };
    let w_method = rows.iter().map(|r| r.method.len()).max().unwrap_or(0);
    let w_path = rows.iter().map(|r| r.path.len()).max().unwrap_or(0);
    let w_codes = rows.iter().map(|r| codes(r).len()).max().unwrap_or(0);
    let w_level = label(Level::None).len();
    for r in rows {
        let level = format!("{:w_level$}", label(r.level));
        let level = match r.level {
            Level::None => st.red(&level),
            Level::Request => st.yellow(&level),
            Level::Checked => st.green(&level),
        };
        let mut line = format!(
            "{:w_method$} {:w_path$}  {level}  codes {:>w_codes$}  {}",
            r.method,
            r.path,
            codes(r),
            st.dim(&place(r))
        );
        let missing: Vec<String> = r
            .statuses
            .iter()
            .filter(|s| !s.checked)
            .map(|s| s.status.code.to_string())
            .collect();
        if !missing.is_empty() && r.level > Level::None {
            line.push_str(&format!(
                "  {}",
                st.yellow(&format!("unchecked: {}", missing.join(", ")))
            ));
        }
        println!("{line}");
    }
    for w in &cov.warnings {
        eprintln!("{} {w}", Style::stderr().yellow("warning:"));
    }
    if !rows.is_empty() {
        println!();
    }
    let (sc, stotal) = cov.statuses();
    let without_expect = cov
        .routes
        .iter()
        .filter(|r| r.level == Level::Request)
        .count();
    let without_request = cov.routes.len() - cov.at_least(Level::Request);
    let text = format!(
        "{}: {without_expect} without expect, {without_request} without request; \
         {sc}/{stotal} status codes checked",
        summary(cov)
    );
    match below {
        Some(m) => {
            println!("{text}");
            println!("{}", st.red(&format!("coverage is below --min {m}%")));
        }
        None if without_request == 0 && without_expect == 0 => println!("{}", st.green(&text)),
        None => println!("{text}"),
    }
    if without_request > 0 {
        println!(
            "{} can be created with `outry import go`",
            count(without_request, "missing request")
        );
    }
}

/// Аннотации GitHub: роут без запроса — на регистрации в Go, без `expect` — на запросе,
/// непроверенный код ответа — на его месте в обработчике.
fn diagnostics(
    rows: &[&RouteCoverage],
    shown: &dyn Fn(&Path) -> String,
    go: &dyn Fn(&Path) -> String,
) -> Vec<ImportDiag> {
    let mut out = Vec::new();
    let warn = |file: String, line: usize, message: String| ImportDiag {
        file,
        line,
        col: 1,
        error: false,
        message,
        go: None,
    };
    for r in rows {
        let route = format!("{} {}", r.method, r.path);
        match (r.level, r.requests.first()) {
            (Level::None, _) | (_, None) => out.push(warn(
                go(&r.source),
                r.line,
                format!("route {route} has no request"),
            )),
            (Level::Request, Some(q)) => out.push(warn(
                shown(&q.file),
                q.line,
                format!("no request to {route} checks the response (`expect`)"),
            )),
            (Level::Checked, Some(_)) => {}
        }
        if r.level == Level::None {
            continue;
        }
        for s in r.statuses.iter().filter(|s| !s.checked) {
            out.push(warn(
                go(&s.status.source),
                s.status.line,
                format!(
                    "status {} of {route} is not checked (`expect {{ status == {} }}`)",
                    s.status.code, s.status.code
                ),
            ));
        }
    }
    out.sort_by(|a, b| (&a.file, a.line, a.col).cmp(&(&b.file, b.line, b.col)));
    out
}
