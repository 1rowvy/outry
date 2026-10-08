use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use routy_core::runner::{Options, RunOutcome};
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
    /// Выполнить запросы. Каталоги раскрываются во все *.http по алфавиту; порядок важен для `> save`.
    Run {
        #[arg(required = true)]
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
    },
    /// Проверить синтаксис файлов без отправки
    Check {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
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
    match real_main() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{} {e:#}", Style::stderr().red("error:"));
            ExitCode::from(2)
        }
    }
}

fn real_main() -> anyhow::Result<ExitCode> {
    match Cli::parse().cmd {
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
        } => {
            let files = expand(&paths)?;
            let project = find_project(env.project.as_deref(), &paths[0])?;
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

            let rt = tokio::runtime::Runtime::new()?;
            let ok = rt.block_on(run_all(&mut runner, &files, fail_fast, verbose, json));
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
            for f in expand(&paths)? {
                let src = std::fs::read_to_string(&f).with_context(|| f.display().to_string())?;
                if let Err(e) = routy_core::parse(&src) {
                    eprintln!("{}: {e}", f.display());
                    ok = false;
                }
            }
            Ok(if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
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
        Cmd::Init { dir } => init(&dir),
    }
}

/// Каталоги → все *.http внутри; файлы — как есть, в заданном порядке.
fn expand(paths: &[PathBuf]) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            let found = discover::request_files(p).with_context(|| p.display().to_string())?;
            if found.is_empty() {
                bail!("{}: no *.{} files", p.display(), discover::EXTENSION);
            }
            out.extend(found.into_iter().map(|f| p.join(f)));
        } else if p.exists() {
            out.push(p.clone());
        } else {
            bail!("{}: no such file or directory", p.display());
        }
    }
    Ok(out)
}

fn find_project(explicit: Option<&Path>, first_path: &Path) -> anyhow::Result<Project> {
    Ok(match explicit {
        Some(dir) => Project::load(dir)?,
        None => Project::discover(first_path)?,
    })
}

async fn run_all(
    runner: &mut Runner,
    files: &[PathBuf],
    fail_fast: bool,
    verbose: bool,
    json: bool,
) -> bool {
    let st = Style::stdout();
    let (mut passed, mut failed) = (0, 0);
    for f in files {
        let result = runner.run_path(f).await;
        let ok = matches!(&result, Ok(o) if o.passed());
        if ok {
            passed += 1
        } else {
            failed += 1
        }

        if json {
            let line = match &result {
                Ok(o) => serde_json::json!({ "file": f, "passed": ok, "outcome": o }),
                Err(e) => {
                    serde_json::json!({ "file": f, "passed": false, "error": format!("{e:#}") })
                }
            };
            println!("{line}");
        } else {
            match &result {
                Ok(o) => print_outcome(&st, f, o, verbose),
                Err(e) => println!(
                    "{} {}\n    {}",
                    st.red("✗"),
                    f.display(),
                    st.red(&format!("{e:#}"))
                ),
            }
        }
        if !ok && fail_fast {
            break;
        }
    }
    if !json && files.len() > 1 {
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

fn print_outcome(st: &Style, file: &Path, o: &RunOutcome, verbose: bool) {
    let r = &o.response;
    let mark = if o.passed() {
        st.green("✓")
    } else {
        st.red("✗")
    };
    let status = format!("{} {}", r.status, r.status_text);
    let status = if r.status < 400 {
        st.green(&status)
    } else {
        st.red(&status)
    };
    println!(
        "{mark} {}  {} {}  {status}  {}",
        file.display(),
        o.request.method,
        st.dim(&o.request.url),
        st.dim(&format!("{}ms {}B", r.duration_ms, r.size))
    );
    for a in &o.asserts {
        if a.passed {
            println!("    {} {}", st.green("✓"), a.source);
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

fn init(dir: &Path) -> anyhow::Result<ExitCode> {
    let api = dir.join("api");
    let config = api.join(routy_core::project::CONFIG_FILE);
    if config.exists() {
        bail!("{} already exists", config.display());
    }
    std::fs::create_dir_all(api.join("health"))?;
    std::fs::write(
        &config,
        "# Окружения Routy. Секреты сюда не пишем: `routy secret set token --env dev`.\n\
         default = \"dev\"\n\n[env.dev]\nbase = \"http://localhost:8080\"\n\n[env.prod]\nbase = \"https://api.example.com\"\n",
    )?;
    std::fs::write(
        api.join("health/get.http"),
        "GET {{base}}/health\n\n> assert status == 200\n",
    )?;
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
