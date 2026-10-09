mod lsp;
mod notifier;
mod update;

use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, bail};
use clap::{CommandFactory, Parser, Subcommand};
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
    /// Проверить файлы без отправки: синтаксис, а в *.routy — имена вызовов, аргументы, формы, циклы.
    /// С --env ещё и окружение: вызовы запросов, закрытых `only`, и недостающие переменные.
    Check {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        #[command(flatten)]
        env: EnvArgs,
        /// Считать переменную заданной: --var token=x (можно несколько раз)
        #[arg(long = "var", value_name = "NAME=VALUE", value_parser = parse_kv)]
        vars: Vec<(String, String)>,
        /// Не обращаться к системному хранилищу паролей (секреты только из ROUTY_*)
        #[arg(long)]
        no_keyring: bool,
    },
    /// Привести *.routy к одному виду (как gofmt): отступы, порядок полей, кавычки, переносы.
    /// Комментарии сохраняются
    Fmt {
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,
        /// Ничего не менять, только перечислить файлы не в каноническом виде (exit code 1)
        #[arg(long)]
        check: bool,
    },
    /// Переписать *.http в *.routy рядом (существующие *.routy не трогаются)
    Convert {
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,
        /// Только показать результат, ничего не записывать
        #[arg(long)]
        dry_run: bool,
        /// Удалить *.http после успешной записи *.routy
        #[arg(long)]
        rm: bool,
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
    /// Скрипт автодополнения для шелла. fish:
    /// `routy completions fish > ~/.config/fish/completions/routy.fish`, bash:
    /// `routy completions bash > ~/.local/share/bash-completion/completions/routy`, zsh: в `.zshrc`
    /// `source <(routy completions zsh)`
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Фоновая проверка новой версии (запускается самим routy)
    #[command(name = notifier::REFRESH_COMMAND, hide = true)]
    RefreshUpdateCache,
    /// Создать *.routy для роутов из кода сервиса (только отсутствующие запросы)
    Import {
        #[command(subcommand)]
        cmd: ImportCmd,
    },
    /// Языковой сервер (LSP) для *.routy через stdin/stdout: ошибки `routy check` и расхождения
    /// с Go-кодом, автодополнение, подсказки, переход к определению, «Send» над запросом.
    /// Запускается редактором, см. https://1rowvy.github.io/routy/guides/editors/
    Lsp {
        /// Окружение для переменных и запусков (по умолчанию — `default` из env.toml)
        #[arg(long)]
        env: Option<String>,
        /// Не обращаться к системному хранилищу паролей
        #[arg(long)]
        no_keyring: bool,
        /// Для совместимости с клиентами, которые передают --stdio; другого транспорта нет
        #[arg(long, hide = true)]
        stdio: bool,
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
    /// (r.Mount, users.Register(v1)) получают свои префиксы. Создаёт запросы для новых роутов,
    /// shape для типов ответов и показывает, чем существующие запросы расходятся с кодом.
    Go {
        /// Каталог с исходниками (рекурсивно, без vendor/ и *_test.go)
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// Только показать, что будет создано, исправлено и удалено
        #[arg(long)]
        dry_run: bool,
        /// Проверка для CI: ничего не пишет, отчёт по файлам; код выхода 1, если запросы
        /// разошлись с кодом (новые и пропавшие роуты, ошибки в запросах и shape)
        #[arg(long, conflicts_with_all = ["fix", "prune", "dry_run"])]
        check: bool,
        /// Исправить расхождения, которые правятся без потери смысла (с --dry-run — показать diff)
        #[arg(long)]
        fix: bool,
        /// Удалить файлы, в которых все запросы — к роутам, которых в коде больше нет
        #[arg(long)]
        prune: bool,
        /// Переменная с адресом сервиса в старых *.http: {{base}}/users (для сопоставления)
        #[arg(long, default_value = routy_core::import::DEFAULT_BASE)]
        base: String,
        /// Свой шаблон-запрос tree-sitter для роутера (можно несколько раз)
        #[arg(long = "query", value_name = "FILE.scm")]
        queries: Vec<PathBuf>,
        /// Каталог проекта с env.toml (по умолчанию ищется вверх от текущего)
        #[arg(long)]
        project: Option<PathBuf>,
        /// Формат вывода: text, github (аннотации GitHub Actions), json
        #[arg(long, value_enum, default_value_t = ImportFormat::Text)]
        format: ImportFormat,
        /// То же, что --format json
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum ImportFormat {
    Text,
    Github,
    Json,
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
    let notify = !matches!(
        cmd,
        Cmd::Update { .. } | Cmd::RefreshUpdateCache | Cmd::Lsp { .. } | Cmd::Completions { .. }
    );
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

/// Скрипт автодополнения. Скрытые команды (`__refresh-update-cache`) `clap_complete` всё равно
/// выдаёт — дополняем по копии без них. Закрытый stdout (`| head`) — не ошибка.
fn completions(shell: clap_complete::Shell) {
    let full = Cli::command();
    let mut cmd = clap::Command::new("routy")
        .version(env!("CARGO_PKG_VERSION"))
        .subcommands(full.get_subcommands().filter(|c| !c.is_hide_set()).cloned());
    let mut out = Vec::new();
    clap_complete::generate(shell, &mut cmd, "routy", &mut out);
    let _ = std::io::Write::write_all(&mut std::io::stdout(), &out);
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
        Cmd::Check {
            paths,
            env,
            vars,
            no_keyring,
        } => {
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
                let project = find_project(env.project.as_deref(), &routy_files[0])?;
                let (ws, files) = workspace_with(&project, &routy_files)?;
                let wanted: Vec<PathBuf> = files.iter().map(|f| ws.root.join(f)).collect();
                let cwd = std::env::current_dir().unwrap_or_default();
                let mut diagnostics = ws.check();
                if let Some(name) = &env.env {
                    let envs = project.env_names();
                    let opts = Options {
                        use_keyring: !no_keyring,
                        ..Options::default()
                    };
                    let mut runner = Runner::new(project, Some(name), opts)?;
                    runner.vars.overrides.extend(vars);
                    let mut has = |n: &str| matches!(runner.vars.get(n), Ok(Some(_)));
                    diagnostics.extend(ws.check_env(name, &envs, &mut has));
                } else if !vars.is_empty() {
                    bail!("--var only makes sense with --env");
                }
                for mut d in diagnostics {
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
        Cmd::Lsp {
            env, no_keyring, ..
        } => {
            lsp::run(lsp::Opts { env, no_keyring })?;
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Fmt { paths, check } => fmt(&paths, check),
        Cmd::Convert { paths, dry_run, rm } => convert(&paths, dry_run, rm),
        Cmd::Update { check } => {
            tokio::runtime::Runtime::new()?.block_on(update::run(check))?;
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Completions { shell } => {
            completions(shell);
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
fn fmt(paths: &[PathBuf], check: bool) -> anyhow::Result<ExitCode> {
    let mut files = Vec::new();
    for p in paths {
        if p.is_dir() {
            let found = discover::routy_files(p).with_context(|| p.display().to_string())?;
            files.extend(found.into_iter().map(|f| p.join(f)));
        } else if p.is_file() {
            files.push(p.clone());
        } else {
            bail!("{}: no such file or directory", p.display());
        }
    }
    let mut changed = 0;
    let mut broken = 0;
    for f in &files {
        let src = std::fs::read_to_string(f).with_context(|| f.display().to_string())?;
        let out = match routy_core::lang::fmt::format(&src) {
            Ok(out) => out,
            Err(e) => {
                eprintln!("{}:{e}", f.display());
                broken += 1;
                continue;
            }
        };
        if out == src {
            continue;
        }
        changed += 1;
        println!("{}", f.display());
        if !check {
            std::fs::write(f, out).with_context(|| f.display().to_string())?;
        }
    }
    Ok(if broken > 0 || (check && changed > 0) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn convert(paths: &[PathBuf], dry_run: bool, rm: bool) -> anyhow::Result<ExitCode> {
    let mut files = Vec::new();
    for p in paths {
        if p.is_dir() {
            let found = discover::request_files(p).with_context(|| p.display().to_string())?;
            files.extend(found.into_iter().map(|f| p.join(f)));
        } else if p.is_file() {
            files.push(p.clone());
        } else {
            bail!("{}: no such file or directory", p.display());
        }
    }
    if files.is_empty() {
        bail!("no *.http files");
    }
    let mut failed = 0;
    for f in &files {
        let to = f.with_extension(discover::ROUTY_EXTENSION);
        let src = std::fs::read_to_string(f).with_context(|| f.display().to_string())?;
        let stem = f.file_stem().unwrap_or_default().to_string_lossy();
        let out = match routy_core::lang::convert::convert(&src, &stem) {
            Ok(out) => out,
            Err(e) => {
                eprintln!("{}: {e}", f.display());
                failed += 1;
                continue;
            }
        };
        if dry_run {
            println!("{} → {}\n{out}", f.display(), to.display());
            continue;
        }
        if to.exists() {
            eprintln!("{}: {} already exists, skipped", f.display(), to.display());
            failed += 1;
            continue;
        }
        std::fs::write(&to, out).with_context(|| to.display().to_string())?;
        if rm {
            std::fs::remove_file(f).with_context(|| f.display().to_string())?;
        }
        println!("{} → {}", f.display(), to.display());
    }
    Ok(if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

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
        check,
        fix,
        prune,
        base,
        queries,
        project,
        format,
        json,
    } = cmd;
    let format = if json { ImportFormat::Json } else { format };
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
    use routy_core::import;
    let plan = import::plan_go(&dir, &project.root, &extra, &base)?;
    let cwd = std::env::current_dir().unwrap_or_default();
    let rel = |full: PathBuf| {
        full.strip_prefix(&cwd)
            .map(Path::to_path_buf)
            .unwrap_or(full)
            .display()
            .to_string()
            .replace('\\', "/")
    };
    // Файл проекта и файл Go — как их видно из текущего каталога.
    let shown = |p: &Path| rel(project.root.join(p));
    let go = |p: &Path| rel(dir.join(p));

    if check {
        let diags = import_diagnostics(&plan, &shown, &go);
        match format {
            ImportFormat::Json => println!("{}", serde_json::to_string_pretty(&plan)?),
            ImportFormat::Github => {
                print_github(&diags);
                for w in &plan.warnings {
                    println!(
                        "::warning title=routy import go::{}",
                        w.replace('\n', "%0A")
                    );
                }
            }
            ImportFormat::Text => {
                print_check(&plan, &diags);
                for w in &plan.warnings {
                    eprintln!("{} {w}", Style::stderr().yellow("warning:"));
                }
            }
        }
        return Ok(if plan.has_errors() {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        });
    }

    let fixes = if fix {
        import::fixes(&plan, |_| true)?
    } else {
        Vec::new()
    };
    let (mut created, mut removed) = (Vec::new(), Vec::new());
    if !dry_run {
        // Сначала правки, потом новые файлы: новые shape дописываются к уже исправленному файлу.
        import::write_fixes(&project.root, &fixes)?;
        created = import::apply(&project.root, &plan)?;
        if prune {
            removed = import::prune(&project.root, &plan)?;
        }
    }
    if format == ImportFormat::Json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
        return Ok(ExitCode::SUCCESS);
    }
    if format == ImportFormat::Github {
        print_github(&import_diagnostics(&plan, &shown, &go));
        return Ok(ExitCode::SUCCESS);
    }

    let st = Style::stdout();
    let width = plan
        .new
        .iter()
        .map(|f| shown(&f.file).len())
        .chain(plan.existing.iter().map(|e| shown(&e.file).len()))
        .chain(plan.stale.iter().map(|s| shown(&s.file).len()))
        .max()
        .unwrap_or(0);
    let route = |r: &import::Route| {
        let mut line = format!(
            "{} {}  {}",
            r.method,
            r.path.replace("{{", "{").replace("}}", "}"),
            st.dim(&format!("{}:{}", go(&r.source), r.line))
        );
        if let Some(summary) = &r.info.summary {
            line.push_str(&format!("  {summary}"));
        }
        line
    };
    let change = |c: &import::Change| {
        let at = format!("{}:{}:{}", shown(&c.file), c.line, c.col);
        let sev = match c.severity {
            import::Severity::Error => st.red("error  "),
            import::Severity::Warning => st.yellow("warning"),
        };
        println!("    {}  {sev}  {}", st.dim(&at), c.message);
    };
    for f in &plan.new {
        let file = format!("{:width$}", shown(&f.file));
        println!("{} {}  {}", st.green("+"), st.green(&file), route(&f.route));
    }
    for e in &plan.existing {
        if e.changes.is_empty() {
            println!(
                "{} {}  {}",
                st.dim("="),
                st.dim(&format!("{:width$}", shown(&e.file))),
                route(&e.route)
            );
            continue;
        }
        let file = format!("{:width$}", shown(&e.file));
        println!(
            "{} {}  {}",
            st.yellow("~"),
            st.yellow(&file),
            route(&e.route)
        );
        e.changes.iter().for_each(change);
    }
    for s in &plan.stale {
        let mark = if plan.prunable.contains(&s.file) {
            "(no such route in code; --prune removes the file)"
        } else {
            "(no such route in code)"
        };
        println!(
            "{} {:width$}  {} {}  {}",
            st.red("-"),
            shown(&s.file),
            s.method,
            s.url,
            st.red(mark)
        );
    }
    if !plan.new_shapes.is_empty() || !plan.shape_changes.is_empty() {
        println!();
        for d in &plan.new_shapes {
            println!(
                "{} {}  shape {}  {}",
                st.green("+"),
                st.green(&shown(&plan.shapes_file)),
                d.name,
                st.dim(&format!("{} {}:{}", d.go_type, go(&d.source), d.line))
            );
        }
        plan.shape_changes.iter().for_each(change);
    }
    for w in &plan.warnings {
        eprintln!("{} {w}", Style::stderr().red("warning:"));
    }
    for f in &fixes {
        println!();
        print_diff(&st, &f.diff);
    }

    let routes = plan.new.len() + plan.existing.len();
    let changed = plan
        .existing
        .iter()
        .filter(|e| !e.changes.is_empty())
        .count();
    println!(
        "\n{} Go files, {routes} routes: {} new, {} existing ({changed} changed), {} not in code",
        plan.files,
        plan.new.len(),
        plan.existing.len(),
        plan.stale.len()
    );
    let fixable = plan.changes().filter(|c| c.fixable).count();
    let fixed: usize = fixes.iter().map(|f| f.changes.len()).sum();
    if dry_run {
        if !plan.new.is_empty() || !plan.new_shapes.is_empty() || fixed > 0 {
            println!("dry run: nothing written");
        }
        if prune && !plan.prunable.is_empty() {
            println!("would remove {} files", plan.prunable.len());
        }
    } else {
        if !created.is_empty() {
            println!("created {} files", created.len());
        }
        if fixed > 0 {
            println!("fixed {fixed} differences in {} files", fixes.len());
        }
        if !removed.is_empty() {
            println!("removed {} files", removed.len());
        }
    }
    if !fix && fixable > 0 {
        println!("{fixable} differences can be fixed with --fix (preview: --fix --dry-run)");
    }
    if !prune && !plan.prunable.is_empty() {
        println!(
            "{} files have no routes in code: --prune removes them",
            plan.prunable.len()
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// Одна строка отчёта `--check`: место, важность, текст и где это в Go.
struct ImportDiag {
    file: String,
    line: usize,
    col: usize,
    error: bool,
    message: String,
    go: Option<String>,
}

fn import_diagnostics(
    plan: &routy_core::import::Plan,
    shown: &dyn Fn(&Path) -> String,
    go: &dyn Fn(&Path) -> String,
) -> Vec<ImportDiag> {
    use routy_core::import::Severity;
    let mut out: Vec<ImportDiag> = plan
        .changes()
        .map(|c| ImportDiag {
            file: shown(&c.file),
            line: c.line,
            col: c.col,
            error: c.severity == Severity::Error,
            message: c.message.clone(),
            go: Some(format!("{}:{}", go(&c.go.file), c.go.line)),
        })
        .collect();
    // Нового роута в проекте ещё нет — отмечаем строку в Go.
    out.extend(plan.new.iter().map(|f| ImportDiag {
        file: go(&f.route.source),
        line: f.route.line,
        col: 1,
        error: true,
        message: format!(
            "route {} {} has no request (`routy import go` creates {})",
            f.route.method,
            f.route.path.replace("{{", "{").replace("}}", "}"),
            shown(&f.file)
        ),
        go: None,
    }));
    out.extend(plan.new_shapes.iter().map(|d| ImportDiag {
        file: go(&d.source),
        line: d.line,
        col: 1,
        error: true,
        message: format!(
            "response type {} has no shape (`routy import go` adds `shape {}` to {})",
            d.go_type,
            d.name,
            shown(&plan.shapes_file)
        ),
        go: None,
    }));
    out.extend(plan.stale.iter().map(|s| ImportDiag {
        file: shown(&s.file),
        line: s.line,
        col: 1,
        error: true,
        message: format!("{} {}: no such route in code", s.method, s.url),
        go: None,
    }));
    out.sort_by(|a, b| (&a.file, a.line, a.col).cmp(&(&b.file, b.line, b.col)));
    out
}

/// Отчёт `--check` по файлам.
fn print_check(plan: &routy_core::import::Plan, diags: &[ImportDiag]) {
    let st = Style::stdout();
    let mut file = None;
    for d in diags {
        if file != Some(&d.file) {
            if file.is_some() {
                println!();
            }
            println!("{}", d.file);
            file = Some(&d.file);
        }
        let sev = if d.error {
            st.red("error  ")
        } else {
            st.yellow("warning")
        };
        let at = format!("{}:{}", d.line, d.col);
        let mut line = format!("  {:7} {sev}  {}", st.dim(&at), d.message);
        if let Some(go) = &d.go {
            line.push_str(&format!("  {}", st.dim(&format!("← {go}"))));
        }
        println!("{line}");
    }
    let errors = diags.iter().filter(|d| d.error).count();
    let warnings = diags.len() - errors;
    if !diags.is_empty() {
        println!();
    }
    let fixable = plan.changes().filter(|c| c.fixable).count();
    let mut summary = format!(
        "{} routes checked: {errors} errors, {warnings} warnings",
        plan.new.len() + plan.existing.len()
    );
    if fixable > 0 {
        summary.push_str(&format!("; {fixable} fixable with `routy import go --fix`"));
    }
    println!("{summary}");
}

/// Аннотации GitHub Actions: `::error file=…,line=…::текст`.
fn print_github(diags: &[ImportDiag]) {
    fn data(s: &str) -> String {
        s.replace('%', "%25")
            .replace('\r', "%0D")
            .replace('\n', "%0A")
    }
    fn prop(s: &str) -> String {
        data(s).replace(':', "%3A").replace(',', "%2C")
    }
    for d in diags {
        let mut message = d.message.clone();
        if let Some(go) = &d.go {
            message.push_str(&format!(" ({go})"));
        }
        println!(
            "::{} file={},line={},col={},title=routy import go::{}",
            if d.error { "error" } else { "warning" },
            prop(&d.file),
            d.line,
            d.col,
            data(&message)
        );
    }
}

fn print_diff(st: &Style, diff: &str) {
    for l in diff.lines() {
        if l.starts_with("+++") || l.starts_with("---") {
            println!("{}", st.dim(l));
        } else if l.starts_with('+') {
            println!("{}", st.green(l));
        } else if l.starts_with('-') {
            println!("{}", st.red(l));
        } else if l.starts_with("@@") {
            println!("{}", st.dim(l));
        } else {
            println!("{l}");
        }
    }
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
    fn yellow(&self, s: &str) -> String {
        self.paint("33", s)
    }
    fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
}
