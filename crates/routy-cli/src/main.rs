mod notifier;
mod update;

use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use routy_core::expr::AssertOutcome;
use routy_core::lang::ast::Item;
use routy_core::lang::exec::{CallTrace, FlowOutcome, Outcome, Run};
use routy_core::lang::{ItemRef, Workspace};
use routy_core::runner::{Options, RunOutcome};
use routy_core::vars::{self, Source};
use routy_core::{Project, Runner, discover};

#[derive(Parser)]
#[command(
    name = "routy",
    version,
    about = "API-клиент, где запросы — текстовые файлы в репозитории"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Выполнить запросы. Каталоги раскрываются во все *.http и *.routy по алфавиту; порядок важен
    /// для `save`. Можно указать запрос или сценарий по имени (`Checkout`, `users.Create`) или по
    /// строке (`api/orders.routy:12`).
    Run {
        #[arg(required = true, value_name = "PATH|NAME")]
        paths: Vec<PathBuf>,
        #[command(flatten)]
        env: EnvArgs,
        /// Переопределить переменную: --var id=42 (можно несколько раз)
        #[arg(long = "var", value_name = "NAME=VALUE", value_parser = parse_kv)]
        vars: Vec<(String, String)>,
        /// Остановиться на первом упавшем запросе
        #[arg(long)]
        fail_fast: bool,
        /// Печатать заголовки и тело ответа
        #[arg(short, long)]
        verbose: bool,
        /// Вывод в JSON Lines (по объекту на запрос) — для CI
        #[arg(long)]
        json: bool,
        /// Таймаут запроса в секундах
        #[arg(long, default_value_t = 30)]
        timeout: u64,
        /// Не читать и не сохранять значения `> save` между запусками
        #[arg(long)]
        fresh: bool,
        /// Не обращаться к системному хранилищу паролей (секреты только из ROUTY_*)
        #[arg(long)]
        no_keyring: bool,
        /// Отправлять запросы с `confirm: true` без вопроса
        #[arg(long)]
        yes: bool,
    },
    /// Проверить файлы без отправки: синтаксис, а в *.routy — имена вызовов, аргументы, формы, циклы
    Check {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// Показать итоговые значения переменных и откуда они взялись (секреты замаскированы)
    Vars {
        #[command(flatten)]
        env: EnvArgs,
        /// Переопределить переменную: --var id=42 (можно несколько раз)
        #[arg(long = "var", value_name = "NAME=VALUE", value_parser = parse_kv)]
        vars: Vec<(String, String)>,
        /// Не учитывать значения `> save` из прошлых запусков
        #[arg(long)]
        fresh: bool,
        /// Не обращаться к системному хранилищу паролей
        #[arg(long)]
        no_keyring: bool,
        /// Показать значения секретов целиком
        #[arg(long)]
        reveal: bool,
        /// Вывод в JSON
        #[arg(long)]
        json: bool,
    },
    /// Показать окружения проекта
    Envs {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// Секреты в системном хранилище паролей
    Secret {
        #[command(subcommand)]
        cmd: SecretCmd,
    },
    /// Обновить routy до последнего релиза с GitHub
    Update {
        /// Только проверить, есть ли новая версия
        #[arg(long)]
        check: bool,
    },
    /// Фоновая проверка новой версии (запускается самим routy)
    #[command(name = notifier::REFRESH_COMMAND, hide = true)]
    RefreshUpdateCache,
    /// Создать *.http для роутов из кода сервиса (только отсутствующие файлы)
    Import {
        #[command(subcommand)]
        cmd: ImportCmd,
    },
    /// Создать api/env.toml и пример запроса
    Init {
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
}

#[derive(Subcommand)]
enum SecretCmd {
    /// Сохранить секрет; значение читается из stdin
    Set {
        name: String,
        #[command(flatten)]
        env: EnvArgs,
    },
    /// Удалить секрет
    Rm {
        name: String,
        #[command(flatten)]
        env: EnvArgs,
    },
}

#[derive(Subcommand)]
enum ImportCmd {
    /// Роуты из Go: chi, gin, net/http. Сканируется весь каталог — роуты из других пакетов
    /// (r.Mount, users.Register(v1)) получают свои префиксы.
    Go {
        /// Каталог с исходниками (рекурсивно, без vendor/ и *_test.go)
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// Только показать, что будет создано
        #[arg(long)]
        dry_run: bool,
        /// Переменная с адресом сервиса в URL: {{base}}/users
        #[arg(long, default_value = routy_core::import::DEFAULT_BASE)]
        base: String,
        /// Свой шаблон-запрос tree-sitter для роутера (можно несколько раз)
        #[arg(long = "query", value_name = "FILE.scm")]
        queries: Vec<PathBuf>,
        /// Каталог проекта с env.toml (по умолчанию ищется вверх от текущего)
        #[arg(long)]
        project: Option<PathBuf>,
        /// Вывод плана в JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Args)]
struct EnvArgs {
    /// Окружение из env.toml (по умолчанию — `default` из конфига)
    #[arg(short, long)]
    env: Option<String>,
    /// Каталог проекта (по умолчанию ищется env.toml вверх от первого пути / текущего каталога)
    #[arg(long, global = true)]
    project: Option<PathBuf>,
}

fn parse_kv(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.trim().to_string(), v.to_string()))
        .ok_or_else(|| format!("expected NAME=VALUE, got `{s}`"))
}

fn main() -> ExitCode {
    let cmd = Cli::parse().cmd;
    let notify = !matches!(cmd, Cmd::Update { .. } | Cmd::RefreshUpdateCache);
    let code = match real_main(cmd) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{} {e:#}", Style::stderr().red("error:"));
            ExitCode::from(2)
        }
    };
    if notify {
        notifier::after_command();
    }
    code
}

fn real_main(cmd: Cmd) -> anyhow::Result<ExitCode> {
    match cmd {
        Cmd::Run {
            paths,
            env,
            vars,
            fail_fast,
            verbose,
            json,
            timeout,
            fresh,
            no_keyring,
            yes,
        } => {
            let targets = expand(&paths)?;
            let project = find_project(env.project.as_deref(), first_existing(&paths))?;
            let opts = Options {
                timeout: Duration::from_secs(timeout),
                use_keyring: !no_keyring,
                ..Options::default()
            };
            let mut runner = Runner::new(project, env.env.as_deref(), opts)?;
            if !fresh {
                runner.load_saved()?;
            }
            runner.vars.overrides.extend(vars);

            let jobs = jobs(&runner.project, &targets)?;
            let mut lang_run = None;
            if let Some(ws) = jobs.workspace {
                let mut run = Run::new(ws, Duration::from_secs(timeout))?;
                run.confirm = confirm_hook(yes, runner.env.clone());
                lang_run = Some(run);
            }
            let rt = tokio::runtime::Runtime::new()?;
            let opts = RunOpts {
                fail_fast,
                verbose,
                json,
            };
            let ok = rt.block_on(run_all(&mut runner, lang_run.as_mut(), &jobs.jobs, opts));
            if !fresh {
                runner.persist_saved()?;
            }
            Ok(if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Cmd::Check { paths } => {
            let mut ok = true;
            let targets = expand(&paths)?;
            let mut routy_files = Vec::new();
            for t in &targets {
                match t {
                    Target::Http(f) => {
                        let src =
                            std::fs::read_to_string(f).with_context(|| f.display().to_string())?;
                        if let Err(e) = routy_core::parse(&src) {
                            eprintln!("{}: {e}", f.display());
                            ok = false;
                        }
                    }
                    Target::Routy(f, _) => routy_files.push(f.clone()),
                    Target::Name(n) => {
                        bail!("`routy check` takes files and directories, got `{n}`")
                    }
                }
            }
            if !routy_files.is_empty() {
                let project = Project::discover(&routy_files[0])?;
                let (ws, files) = workspace_with(&project, &routy_files)?;
                let wanted: Vec<PathBuf> = files.iter().map(|f| ws.root.join(f)).collect();
                let cwd = std::env::current_dir().unwrap_or_default();
                for mut d in ws.check() {
                    if wanted.contains(&d.path) {
                        if let Ok(rel) = d.path.strip_prefix(&cwd) {
                            d.path = rel.to_path_buf();
                        }
                        eprintln!("{d}");
                        ok = false;
                    }
                }
            }
            Ok(if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Cmd::Vars {
            env,
            vars,
            fresh,
            no_keyring,
            reveal,
            json,
        } => {
            let project = find_project(env.project.as_deref(), Path::new("."))?;
            let opts = Options {
                use_keyring: !no_keyring,
                ..Options::default()
            };
            let mut runner = Runner::new(project, env.env.as_deref(), opts)?;
            if !fresh {
                runner.load_saved()?;
            }
            runner.vars.overrides.extend(vars);
            print_vars(&runner, reveal, json)?;
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Envs { dir } => {
            let p = Project::discover(&dir)?;
            let current = p.resolve_env(None).ok();
            println!("project {} ({})", p.id(), p.root.display());
            for name in p.env_names() {
                let mark = if current.as_deref() == Some(name.as_str()) {
                    "*"
                } else {
                    " "
                };
                println!("{mark} {name}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Secret { cmd } => secret(cmd),
        Cmd::Import { cmd } => import(cmd),
        Cmd::Init { dir } => init(&dir),
        Cmd::Update { check } => {
            tokio::runtime::Runtime::new()?.block_on(update::run(check))?;
            Ok(ExitCode::SUCCESS)
        }
        Cmd::RefreshUpdateCache => {
            notifier::refresh();
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Что запустить: файл `.http`, файл `.routy` (целиком или элемент на строке), имя.
enum Target {
    Http(PathBuf),
    Routy(PathBuf, Option<usize>),
    Name(String),
}

fn is_routy(p: &Path) -> bool {
    p.extension()
        .is_some_and(|e| e == discover::ROUTY_EXTENSION)
}

/// Каталоги → все *.http и *.routy внутри; файлы — как есть, в заданном порядке.
fn expand(paths: &[PathBuf]) -> anyhow::Result<Vec<Target>> {
    let file = |p: PathBuf| {
        if is_routy(&p) {
            Target::Routy(p, None)
        } else {
            Target::Http(p)
        }
    };
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            let exts = [discover::EXTENSION, discover::ROUTY_EXTENSION];
            let found = discover::files(p, &exts).with_context(|| p.display().to_string())?;
            if found.is_empty() {
                bail!("{}: no *.http or *.routy files", p.display());
            }
            out.extend(found.into_iter().map(|f| file(p.join(f))));
            continue;
        }
        if p.exists() {
            out.push(file(p.clone()));
            continue;
        }
        let s = p.to_string_lossy();
        if let Some((f, line)) = s.rsplit_once(':') {
            let f = PathBuf::from(f);
            if let (true, Ok(line)) = (f.is_file() && is_routy(&f), line.parse()) {
                out.push(Target::Routy(f, Some(line)));
                continue;
            }
        }
        let is_name = !s.is_empty()
            && s.split('.')
                .all(|w| w.chars().all(|c| c.is_alphanumeric() || c == '_') && !w.is_empty());
        if is_name {
            out.push(Target::Name(s.into_owned()));
        } else {
            bail!("{}: no such file or directory", p.display());
        }
    }
    Ok(out)
}

/// Первый путь, который существует, — от него ищется проект (имена запросов путями не являются).
fn first_existing(paths: &[PathBuf]) -> &Path {
    paths
        .iter()
        .find(|p| p.exists())
        .map_or(Path::new("."), |p| p.as_path())
}

/// Все *.routy проекта плюс переданные файлы вне его. Возвращает и пути переданных файлов
/// относительно корня проекта (как они записаны в `Workspace`).
fn workspace_with(
    project: &Project,
    files: &[PathBuf],
) -> anyhow::Result<(Workspace, Vec<PathBuf>)> {
    let mut ws = Workspace::load(&project.root)?;
    let root = std::fs::canonicalize(&project.root).unwrap_or_else(|_| project.root.clone());
    let mut rels = Vec::new();
    for f in files {
        let full = std::fs::canonicalize(f).with_context(|| f.display().to_string())?;
        let rel = match full.strip_prefix(&root) {
            Ok(rel) => rel.to_path_buf(),
            Err(_) => {
                let text = std::fs::read_to_string(f).with_context(|| f.display().to_string())?;
                ws.add(full.clone(), text);
                full
            }
        };
        rels.push(rel);
    }
    Ok((ws, rels))
}

enum Job {
    Http(PathBuf),
    Item { file: PathBuf, item: ItemRef },
    Fail { label: String, error: String },
}

struct Jobs {
    jobs: Vec<Job>,
    /// Есть, если среди целей есть *.routy или имена.
    workspace: Option<Workspace>,
}

fn jobs(project: &Project, targets: &[Target]) -> anyhow::Result<Jobs> {
    let routy: Vec<PathBuf> = targets
        .iter()
        .filter_map(|t| match t {
            Target::Routy(f, _) => Some(f.clone()),
            _ => None,
        })
        .collect();
    let needs_ws = targets.iter().any(|t| !matches!(t, Target::Http(_)));
    if !needs_ws {
        return Ok(Jobs {
            jobs: targets
                .iter()
                .filter_map(|t| match t {
                    Target::Http(f) => Some(Job::Http(f.clone())),
                    _ => None,
                })
                .collect(),
            workspace: None,
        });
    }
    let (ws, rels) = workspace_with(project, &routy)?;
    let mut rels = rels.into_iter();
    let mut jobs = Vec::new();
    for t in targets {
        match t {
            Target::Http(f) => jobs.push(Job::Http(f.clone())),
            Target::Name(n) => {
                let path: Vec<String> = n.split('.').map(str::to_string).collect();
                match ws.resolve(&path) {
                    Ok(item) => jobs.push(Job::Item {
                        file: ws.sources[item.file].full.clone(),
                        item,
                    }),
                    Err(error) => jobs.push(Job::Fail {
                        label: n.clone(),
                        error,
                    }),
                }
            }
            Target::Routy(f, line) => {
                let rel = rels.next().unwrap_or_default();
                let Some(fi) = ws.file_index(&rel) else {
                    let full = ws.root.join(&rel);
                    let error = ws
                        .errors
                        .iter()
                        .find(|d| d.path == full)
                        .map_or_else(|| "cannot parse".to_string(), |d| d.to_string());
                    jobs.push(Job::Fail {
                        label: f.display().to_string(),
                        error,
                    });
                    continue;
                };
                let src = &ws.sources[fi];
                let mut found = false;
                for (ii, item) in src.file.items.iter().enumerate() {
                    if !matches!(item, Item::Request(_) | Item::Flow(_)) {
                        continue;
                    }
                    if let Some(line) = line {
                        let span = item.span();
                        if !(src.line(span.start)..=src.line(span.end)).contains(line) {
                            continue;
                        }
                    }
                    found = true;
                    jobs.push(Job::Item {
                        file: f.clone(),
                        item: ItemRef { file: fi, item: ii },
                    });
                }
                if let (false, Some(line)) = (found, line) {
                    jobs.push(Job::Fail {
                        label: format!("{}:{line}", f.display()),
                        error: "no request or flow on this line".into(),
                    });
                }
            }
        }
    }
    Ok(Jobs {
        jobs,
        workspace: Some(ws),
    })
}

/// `confirm: true`: `--yes` — отправлять, в терминале — спросить, иначе — отказ с подсказкой.
fn confirm_hook(yes: bool, env: String) -> Option<routy_core::lang::exec::Confirm> {
    if yes {
        return Some(Box::new(|_, _| true));
    }
    if !std::io::stdin().is_terminal() {
        return None;
    }
    Some(Box::new(move |name, req| {
        eprint!(
            "Send {name} ({} {}) in `{env}`? [y/N] ",
            req.method, req.url
        );
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer).is_ok()
            && matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
    }))
}

fn find_project(explicit: Option<&Path>, first_path: &Path) -> anyhow::Result<Project> {
    Ok(match explicit {
        Some(dir) => Project::load(dir)?,
        None => Project::discover(first_path)?,
    })
}

struct RunOpts {
    fail_fast: bool,
    verbose: bool,
    json: bool,
}

async fn run_all(
    runner: &mut Runner,
    mut lang: Option<&mut Run>,
    jobs: &[Job],
    opts: RunOpts,
) -> bool {
    let st = Style::stdout();
    let (mut passed, mut failed) = (0, 0);
    for job in jobs {
        let (file, name, result) = match job {
            Job::Http(f) => (
                f.display().to_string(),
                None,
                runner.run_path(f).await.map(Outcome::Request),
            ),
            Job::Item { file, item } => {
                let run = lang.as_deref_mut().expect("workspace for .routy jobs");
                let name = run.workspace().item(*item).name().map(str::to_string);
                (
                    file.display().to_string(),
                    name,
                    run.run_item(runner, *item).await,
                )
            }
            Job::Fail { label, error } => (
                label.clone(),
                None,
                Err(routy_core::Error::Run(error.clone())),
            ),
        };
        let ok = matches!(&result, Ok(o) if o.passed());
        if ok {
            passed += 1
        } else {
            failed += 1
        }

        if opts.json {
            let line = match &result {
                Ok(o) => {
                    serde_json::json!({ "file": file, "name": name, "passed": ok, "outcome": o })
                }
                Err(e) => serde_json::json!({
                    "file": file, "name": name, "passed": false, "error": format!("{e:#}")
                }),
            };
            println!("{line}");
        } else {
            let label = match &name {
                Some(n) => format!("{file}  {n}"),
                None => file,
            };
            match &result {
                Ok(Outcome::Request(o)) => print_outcome(&st, &label, o, opts.verbose),
                Ok(Outcome::Flow(o)) => print_flow(&st, &label, o),
                Err(e) => println!("{} {label}\n    {}", st.red("✗"), st.red(&format!("{e:#}"))),
            }
        }
        if !ok && opts.fail_fast {
            break;
        }
    }
    if !opts.json && jobs.len() > 1 {
        let summary = format!("{passed} passed, {failed} failed");
        println!(
            "\n{}",
            if failed == 0 {
                st.green(&summary)
            } else {
                st.red(&summary)
            }
        );
    }
    failed == 0
}

fn mark(st: &Style, ok: bool) -> String {
    if ok { st.green("✓") } else { st.red("✗") }
}

fn print_checks(st: &Style, checks: &[AssertOutcome]) {
    for a in checks {
        if a.passed {
            println!("    {} {}", st.green("✓"), a.source);
        } else if let Some(detail) = &a.detail {
            println!(
                "    {} {}  {}",
                st.red("✗"),
                a.source,
                st.dim(&format!("— {detail}"))
            );
        } else {
            let actual = a
                .actual
                .as_ref()
                .map_or("<missing>".to_string(), |v| v.to_string());
            println!(
                "    {} {}  {}",
                st.red("✗"),
                a.source,
                st.dim(&format!("(actual: {actual})"))
            );
        }
    }
}

fn print_calls(st: &Style, calls: &[CallTrace]) {
    for c in calls {
        let indent = "  ".repeat(c.depth);
        let info = match (c.cached, c.status, c.duration_ms) {
            (true, _, _) => "cached".to_string(),
            (_, Some(s), Some(ms)) => format!("{s}  {ms}ms"),
            _ => String::new(),
        };
        println!("    {indent}{} {}  {}", st.dim("↳"), c.name, st.dim(&info));
    }
}

fn print_outcome(st: &Style, label: &str, o: &RunOutcome, verbose: bool) {
    let r = &o.response;
    let status = format!("{} {}", r.status, r.status_text);
    let status = if r.status < 400 {
        st.green(&status)
    } else {
        st.red(&status)
    };
    println!(
        "{} {label}  {} {}  {status}  {}",
        mark(st, o.passed()),
        o.request.method,
        st.dim(&o.request.url),
        st.dim(&format!("{}ms {}B", r.duration_ms, r.size))
    );
    print_calls(st, &o.calls);
    print_checks(st, &o.asserts);
    for miss in &o.save_misses {
        println!(
            "    {} save {miss}  {}",
            st.red("✗"),
            st.dim("(no value in response)")
        );
    }
    for name in o.saved.keys() {
        println!("    {} saved {name}", st.dim("→"));
    }
    if verbose {
        for h in &r.headers {
            println!("    {}", st.dim(&format!("{}: {}", h.name, h.value)));
        }
        let body = match &r.json {
            Some(v) => serde_json::to_string_pretty(v).unwrap_or_else(|_| r.body.clone()),
            None => r.body.clone(),
        };
        for line in body.lines() {
            println!("    {line}");
        }
    }
}

fn print_flow(st: &Style, label: &str, o: &FlowOutcome) {
    println!("{} {label}  {}", mark(st, o.passed()), st.dim("flow"));
    print_calls(st, &o.calls);
    print_checks(st, &o.checks);
    if let Some(e) = &o.error {
        println!("    {} {}", st.red("✗"), st.red(e));
    }
    for name in o.saved.keys() {
        println!("    {} saved {name}", st.dim("→"));
    }
}

fn print_vars(runner: &Runner, reveal: bool, json: bool) -> anyhow::Result<()> {
    let mut list = runner.variables()?;
    if !reveal {
        for v in list.iter_mut().filter(|v| v.secret) {
            v.value = v.value.as_deref().map(vars::mask);
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&list)?);
        return Ok(());
    }
    let st = Style::stdout();
    println!(
        "{}",
        st.dim(&format!(
            "project {} · env {}",
            runner.project.id(),
            runner.env
        ))
    );
    let width = list.iter().map(|v| v.name.len()).max().unwrap_or(0);
    for v in &list {
        let source = match v.source {
            Some(Source::Override) => "--var",
            Some(Source::Saved) => "saved",
            Some(Source::ProcessEnv) => "ROUTY_*",
            Some(Source::Env) => "env.toml",
            Some(Source::Secret) => "keychain",
            Some(Source::Dynamic) => "dynamic",
            None => "missing",
        };
        let value = match &v.value {
            Some(value) => value.clone(),
            None => st.red(&format!(
                "not set: routy secret set {} --env {}",
                v.name, runner.env
            )),
        };
        println!(
            "{:width$}  {}  {value}",
            v.name,
            st.dim(&format!("{source:9}"))
        );
    }
    Ok(())
}

fn secret(cmd: SecretCmd) -> anyhow::Result<ExitCode> {
    let (name, env, set) = match cmd {
        SecretCmd::Set { name, env } => (name, env, true),
        SecretCmd::Rm { name, env } => (name, env, false),
    };
    let project = find_project(env.project.as_deref(), Path::new("."))?;
    let env_name = project.resolve_env(env.env.as_deref())?;
    let Some(store) = routy_core::runner::secret_store(&project, &env_name) else {
        bail!("this build has no system keyring support; use ROUTY_* environment variables");
    };
    if set {
        if std::io::stdin().is_terminal() {
            eprint!("value for {name} ({}/{env_name}): ", project.id());
        }
        let mut value = String::new();
        std::io::stdin().read_to_string(&mut value)?;
        let value = value.trim_end_matches(['\r', '\n']);
        if value.is_empty() {
            bail!("empty value");
        }
        store.set(&name, value)?;
        eprintln!("saved {name} for {}/{env_name}", project.id());
    } else if store.delete(&name)? {
        eprintln!("removed {name} for {}/{env_name}", project.id());
    } else {
        eprintln!("{name} not found for {}/{env_name}", project.id());
    }
    Ok(ExitCode::SUCCESS)
}

fn import(cmd: ImportCmd) -> anyhow::Result<ExitCode> {
    let ImportCmd::Go {
        dir,
        dry_run,
        base,
        queries,
        project,
        json,
    } = cmd;
    let project = find_project(project.as_deref(), Path::new("."))?;
    if !project.has_config() {
        bail!(
            "no env.toml found from {}; run `routy init` first",
            project.root.display()
        );
    }
    let extra = queries
        .iter()
        .map(|q| {
            let src = std::fs::read_to_string(q).with_context(|| q.display().to_string())?;
            Ok((q.display().to_string(), src))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let plan = routy_core::import::plan_go(&dir, &project.root, &extra, &base)?;
    let created = if dry_run {
        Vec::new()
    } else {
        routy_core::import::apply(&project.root, &plan)?
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
        return Ok(ExitCode::SUCCESS);
    }

    let st = Style::stdout();
    let shown = |p: &Path| {
        let full = project.root.join(p);
        full.strip_prefix(std::env::current_dir().unwrap_or_default())
            .map(Path::to_path_buf)
            .unwrap_or(full)
            .display()
            .to_string()
    };
    let width = plan
        .new
        .iter()
        .map(|f| shown(&f.file).len())
        .chain(plan.existing.iter().map(|e| shown(&e.file).len()))
        .chain(plan.stale.iter().map(|s| shown(&s.file).len()))
        .max()
        .unwrap_or(0);
    let route = |r: &routy_core::import::Route| {
        let mut line = format!(
            "{} {}  {}",
            r.method,
            r.path,
            st.dim(&format!("{}:{}", r.source.display(), r.line))
        );
        if let Some(summary) = &r.info.summary {
            line.push_str(&format!("  {summary}"));
        }
        line
    };
    for f in &plan.new {
        let file = format!("{:width$}", shown(&f.file));
        println!("{} {}  {}", st.green("+"), st.green(&file), route(&f.route));
    }
    for e in &plan.existing {
        println!(
            "{} {}  {}",
            st.dim("="),
            st.dim(&format!("{:width$}", shown(&e.file))),
            route(&e.route)
        );
    }
    for s in &plan.stale {
        println!(
            "{} {:width$}  {} {}  {}",
            st.red("-"),
            shown(&s.file),
            s.method,
            s.url,
            st.red("(no such route in code)")
        );
    }
    for w in &plan.warnings {
        eprintln!("{} {w}", Style::stderr().red("warning:"));
    }
    let routes = plan.new.len() + plan.existing.len();
    let summary = format!(
        "{} Go files, {routes} routes: {} new, {} existing, {} not in code",
        plan.files,
        plan.new.len(),
        plan.existing.len(),
        plan.stale.len()
    );
    println!("\n{summary}");
    if dry_run {
        if !plan.new.is_empty() {
            println!("dry run: nothing written");
        }
    } else if !created.is_empty() {
        println!("created {} files", created.len());
    }
    Ok(ExitCode::SUCCESS)
}

fn init(dir: &Path) -> anyhow::Result<ExitCode> {
    let api = routy_core::project::init(dir)?;
    println!("created {}", api.display());
    Ok(ExitCode::SUCCESS)
}

/// ANSI-цвета, только если вывод — терминал и не задан NO_COLOR.
struct Style(bool);

impl Style {
    fn stdout() -> Self {
        Style(std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none())
    }
    fn stderr() -> Self {
        Style(std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none())
    }
    fn paint(&self, code: &str, s: &str) -> String {
        if self.0 {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    fn red(&self, s: &str) -> String {
        self.paint("31", s)
    }
    fn green(&self, s: &str) -> String {
        self.paint("32", s)
    }
    fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
}
