//! `outry coverage`: таблица покрытия роутов, порог `--min` для CI.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, bail};
use outry_core::import::{self, Coverage, Level, RouteCoverage};
use serde::Serialize;

use super::{ImportDiag, Style, count, find_project, import_command, print_github, relative};

#[derive(clap::Args)]
pub struct Args {
    /// Каталог сервиса на Go или Rust (по умолчанию — где go.mod или Cargo.toml, вверх от
    /// проекта до .git)
    dir: Option<PathBuf>,
    /// Код выхода 1, если проверенных роутов меньше N процентов (для CI)
    #[arg(long, value_name = "N")]
    min: Option<f64>,
    /// Журнал `outry run --coverage-out FILE`: отметить роуты, запросы к которым реально
    /// отправлялись (можно несколько раз — журналы объединяются)
    #[arg(long = "from", value_name = "FILE")]
    from: Vec<PathBuf>,
    /// Код выхода 1, если выполненных роутов меньше N процентов (нужен --from)
    #[arg(long, value_name = "N")]
    min_executed: Option<f64>,
    /// Показать только дыры: роуты без запроса, без `expect`, с непроверенными кодами ответа и
    /// (с --from) невыполненные
    #[arg(long)]
    uncovered: bool,
    /// Каталог проекта с env.toml (по умолчанию ищется вверх от текущего)
    #[arg(long)]
    project: Option<PathBuf>,
    /// Переменная с адресом сервиса в старых *.http: {{base}}/users (для сопоставления)
    #[arg(long, default_value = import::DEFAULT_BASE)]
    base: String,
    /// Свой шаблон-запрос tree-sitter для Go-роутера (можно несколько раз)
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
        from,
        min_executed,
        uncovered,
        project,
        base,
        queries,
        format,
        json,
    } = args;
    let format = if json { Format::Json } else { format };
    for (flag, value) in [("--min", min), ("--min-executed", min_executed)] {
        if let Some(m) = value
            && !(0.0..=100.0).contains(&m)
        {
            bail!("{flag} is a percentage from 0 to 100, got {m}");
        }
    }
    if min_executed.is_some() && from.is_empty() {
        bail!(
            "--min-executed needs a run log: `outry run api --coverage-out run.json`, then `--from run.json`"
        );
    }
    let mut runs = Vec::new();
    for f in &from {
        let text = std::fs::read_to_string(f).with_context(|| f.display().to_string())?;
        let log =
            import::RunLog::parse(&text).map_err(|e| anyhow::anyhow!("{}: {e}", f.display()))?;
        runs.extend(log.requests);
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
        None => import::service_dir(&project.root).with_context(|| {
            format!(
                "no go.mod or Cargo.toml found from {} up to the repository root; pass the \
                 service directory: `outry coverage path/to/service`",
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
    let scan = import::scan_source(&dir, &extra)?;
    let cov = if from.is_empty() {
        import::coverage(&project.root, scan, &base)?
    } else {
        import::coverage_runs(&project.root, scan, &base, &runs)?
    };
    let below = min.filter(|&m| cov.percent() < m);
    let below_executed = min_executed.filter(|&m| cov.executed_percent() < m);

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
                #[serde(skip_serializing_if = "Option::is_none")]
                executed: Option<usize>,
                #[serde(skip_serializing_if = "Option::is_none")]
                executed_percent: Option<f64>,
                #[serde(skip_serializing_if = "Option::is_none")]
                min_executed: Option<f64>,
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
                executed: cov.runs.then(|| cov.executed()),
                executed_percent: cov
                    .runs
                    .then(|| (cov.executed_percent() * 10.0).round() / 10.0),
                min_executed,
                passed: below.is_none() && below_executed.is_none(),
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
            if let Some(m) = below_executed {
                println!(
                    "::error title=outry coverage::{} is below --min-executed {m}%",
                    executed_summary(&cov)
                );
            }
        }
        Format::Text => print_table(
            &cov,
            &rows,
            &shown,
            &go,
            (below, below_executed),
            import_command(&dir),
        ),
    }
    Ok(if below.is_some() || below_executed.is_some() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// Дыра: роут без проверок или с кодом ответа, который никто не проверяет.
fn is_hole(r: &RouteCoverage) -> bool {
    r.level < Level::Checked || r.statuses.iter().any(|s| !s.checked) || r.executed == Some(false)
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

/// `2/5 routes executed (40%)`.
fn executed_summary(cov: &Coverage) -> String {
    format!(
        "{}/{} routes executed ({})",
        cov.executed(),
        cov.routes.len(),
        percent(cov.executed_percent())
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
    (below, below_executed): (Option<f64>, Option<f64>),
    import_cmd: &str,
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
    let w_ran = "not run".len();
    for r in rows {
        let level = format!("{:w_level$}", label(r.level));
        let level = match r.level {
            Level::None => st.red(&level),
            Level::Request => st.yellow(&level),
            Level::Checked => st.green(&level),
        };
        let ran = match r.executed {
            Some(true) => format!("{}  ", st.green(&format!("{:w_ran$}", "ran"))),
            Some(false) => format!("{}  ", st.red(&format!("{:w_ran$}", "not run"))),
            None => String::new(),
        };
        let mut line = format!(
            "{:w_method$} {:w_path$}  {level}  {ran}codes {:>w_codes$}  {}",
            r.method,
            r.path,
            codes(r),
            st.dim(&place(r))
        );
        if !r.via.is_empty() {
            line.push_str(&format!(
                "  {}",
                st.dim(&format!("via {}", r.via.join(" → ")))
            ));
        }
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
    let mut text = format!(
        "{}: {without_expect} without expect, {without_request} without request; \
         {sc}/{stotal} status codes checked",
        summary(cov)
    );
    if cov.runs {
        text.push_str(&format!("; {}", executed_summary(cov)));
    }
    let complete = without_request == 0
        && without_expect == 0
        && (!cov.runs || cov.executed() == cov.routes.len());
    if below.is_some() || below_executed.is_some() {
        println!("{text}");
        if let Some(m) = below {
            println!("{}", st.red(&format!("coverage is below --min {m}%")));
        }
        if let Some(m) = below_executed {
            println!(
                "{}",
                st.red(&format!("executed routes are below --min-executed {m}%"))
            );
        }
    } else if complete {
        println!("{}", st.green(&text));
    } else {
        println!("{text}");
    }
    if without_request > 0 {
        println!(
            "{} can be created with `{import_cmd}`",
            count(without_request, "missing request")
        );
    }
}

/// Аннотации GitHub: роут без запроса — на регистрации в коде, без `expect` — на запросе,
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
        if let (Some(false), Some(q)) = (r.executed, r.requests.first()) {
            out.push(warn(
                shown(&q.file),
                q.line,
                format!("route {route} was not executed by `outry run`"),
            ));
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
