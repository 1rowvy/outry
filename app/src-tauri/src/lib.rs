//! Тонкая прослойка между React-фронтом и `routy-core`. Вся логика — в core,
//! здесь только команды Tauri, состояние открытого проекта и слежение за файлами.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use notify::{RecursiveMode, Watcher};
use routy_core::history::{Entry, History, history_path};
use routy_core::lang::exec::{FlowOutcome, Outcome, Run};
use routy_core::lang::{Workspace, ast::Item};
use routy_core::runner::Options;
use routy_core::vars::{VarInfo, mask};
use routy_core::{Project, Runner, discover, dynamic};
use serde::Serialize;
use tauri::{Emitter, State};
use tokio::sync::{Mutex, oneshot};

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Прогон `*.routy`: кеш вызовов и cookies живут, пока не сменили окружение или не сбросили.
/// Свой раннер — чтобы долгий сценарий не держал сессию; сохранённые значения синхронизируются
/// с раннером окружения до и после запуска.
struct LangRun {
    run: Run,
    runner: Runner,
}

struct Session {
    project: Project,
    /// Раннер на окружение: в нём живут значения `> save` между запросами.
    runners: HashMap<String, Runner>,
    /// Окружение прогона — снаружи мьютекса: он занят, пока идёт сценарий.
    lang: Option<(String, Arc<Mutex<LangRun>>)>,
    history: History,
    _watcher: Option<notify::RecommendedWatcher>,
}

impl Session {
    /// Раннер окружения; создаётся при первом обращении и подхватывает сохранённые `> save`.
    fn runner(&mut self, env: Option<&str>) -> CmdResult<&mut Runner> {
        let env = self.project.resolve_env(env).map_err(err)?;
        if !self.runners.contains_key(&env) {
            let mut r =
                Runner::new(self.project.clone(), Some(&env), Options::default()).map_err(err)?;
            r.load_saved().map_err(err)?;
            self.runners.insert(env.clone(), r);
        }
        Ok(self.runners.get_mut(&env).expect("inserted above"))
    }

    /// Прогон `*.routy` окружения; другое окружение — новый прогон.
    fn lang_run(&mut self, env: Option<&str>) -> CmdResult<(String, Arc<Mutex<LangRun>>)> {
        let env = self.project.resolve_env(env).map_err(err)?;
        if let Some((e, l)) = &self.lang {
            if *e == env {
                return Ok((env, l.clone()));
            }
        }
        let runner =
            Runner::new(self.project.clone(), Some(&env), Options::default()).map_err(err)?;
        let run = Run::new(Workspace::default(), Options::default().timeout).map_err(err)?;
        let l = Arc::new(Mutex::new(LangRun { run, runner }));
        self.lang = Some((env.clone(), l.clone()));
        Ok((env, l))
    }
}

fn is_routy(path: &str) -> bool {
    path.ends_with(&format!(".{}", discover::ROUTY_EXTENSION))
}

#[derive(Default)]
struct AppState {
    session: Mutex<Option<Session>>,
    /// Запросы в полёте (id от фронта → отмена). Отдельно от сессии: её блокировка
    /// на время HTTP не держится, поэтому запросы идут параллельно.
    inflight: std::sync::Mutex<HashMap<u64, oneshot::Sender<()>>>,
}

#[derive(Serialize)]
struct ProjectInfo {
    root: PathBuf,
    id: String,
    envs: Vec<String>,
    default_env: Option<String>,
    /// `false` — открыт каталог без env.toml; фронт предлагает его создать.
    has_config: bool,
    files: Vec<String>,
    /// Метод каждого файла (для значков в дереве)
    methods: HashMap<String, String>,
    /// Имена запросов и сценариев `*.routy` (подпись в дереве вместо имени файла)
    names: HashMap<String, Vec<String>>,
}

/// Метод из строки запроса без полного разбора: файл с ошибкой тоже получает значок.
/// В `*.routy` — первый запрос файла, а если в нём только сценарии — `FLOW`, только shape — `SHAPE`.
fn request_method(src: &str, routy: bool) -> String {
    let word = |l: &str| l.split_whitespace().next().unwrap_or_default().to_string();
    let mut lines = src
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("//"));
    let is_method = |w: &str| w.len() >= 2 && w.bytes().all(|b| b.is_ascii_uppercase());
    if routy {
        // `Login: POST /login` — метод после имени.
        let words: Vec<String> = lines
            .map(|l| match l.split_once(':') {
                Some((name, rest)) if !name.contains(char::is_whitespace) => word(rest),
                _ => word(l),
            })
            .collect();
        return match words.iter().find(|w| is_method(w)) {
            Some(m) => m.clone(),
            None if words.iter().any(|w| w == "flow") => "FLOW".into(),
            None if words.iter().any(|w| w == "shape") => "SHAPE".into(),
            None => "GET".into(),
        };
    }
    lines
        .next()
        .map(word)
        .filter(|w| w.bytes().all(|b| b.is_ascii_uppercase()))
        .unwrap_or_else(|| "GET".into())
}

/// Имена запросов и сценариев файла. Файл с ошибкой — по строкам `Name: METHOD …`.
fn item_names(src: &str, file: &str) -> Vec<String> {
    let stem = Path::new(file).file_stem().and_then(|s| s.to_str());
    match routy_core::lang::parse::parse(src, stem) {
        Ok(f) => f
            .items
            .iter()
            .filter(|i| matches!(i, Item::Request(_) | Item::Flow(_)))
            .filter_map(|i| i.name())
            .map(String::from)
            .collect(),
        Err(_) => src
            .lines()
            .filter_map(|l| l.split_once(':'))
            .filter(|(name, rest)| {
                name.chars().next().is_some_and(char::is_uppercase)
                    && name.chars().all(|c| c.is_alphanumeric() || c == '_')
                    && rest
                        .trim_start()
                        .starts_with(|c: char| c.is_ascii_uppercase())
            })
            .map(|(name, _)| name.to_string())
            .collect(),
    }
}

fn project_info(p: &Project) -> CmdResult<ProjectInfo> {
    let exts = [discover::EXTENSION, discover::ROUTY_EXTENSION];
    let files: Vec<String> = discover::files(&p.root, &exts)
        .map_err(err)?
        .into_iter()
        .map(|f| f.to_string_lossy().replace('\\', "/"))
        .collect();
    let mut methods = HashMap::new();
    let mut names = HashMap::new();
    for f in &files {
        let src = std::fs::read_to_string(p.root.join(f)).unwrap_or_default();
        methods.insert(f.clone(), request_method(&src, is_routy(f)));
        if is_routy(f) {
            names.insert(f.clone(), item_names(&src, f));
        }
    }
    Ok(ProjectInfo {
        root: p.root.clone(),
        id: p.id(),
        envs: p.env_names(),
        default_env: p.resolve_env(None).ok(),
        has_config: p.has_config(),
        files,
        methods,
        names,
    })
}

/// Путь от фронта — только относительный и только внутри проекта.
fn inside(root: &Path, rel: &str) -> CmdResult<PathBuf> {
    let rel = Path::new(rel);
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(format!(
            "path must be relative to the project: {}",
            rel.display()
        ));
    }
    Ok(root.join(rel))
}

fn watch(app: tauri::AppHandle, root: &Path) -> Option<notify::RecommendedWatcher> {
    let root_owned = root.to_path_buf();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(ev) = res else { return };
        if matches!(ev.kind, notify::EventKind::Access(_)) {
            return;
        }
        let paths: Vec<String> = ev
            .paths
            .iter()
            .filter_map(|p| p.strip_prefix(&root_owned).ok())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        if !paths.is_empty() {
            let _ = app.emit("project-changed", paths);
        }
    })
    .ok()?;
    w.watch(root, RecursiveMode::Recursive).ok()?;
    Some(w)
}

async fn open(app: tauri::AppHandle, state: &AppState, dir: &Path) -> CmdResult<ProjectInfo> {
    let project = Project::discover(dir).map_err(err)?;
    let info = project_info(&project)?;
    let watcher = watch(app, &project.root);
    *state.session.lock().await = Some(Session {
        project,
        runners: HashMap::new(),
        lang: None,
        history: History::default(),
        _watcher: watcher,
    });
    Ok(info)
}

#[tauri::command]
async fn open_project(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    dir: PathBuf,
) -> CmdResult<ProjectInfo> {
    open(app, &state, &dir).await
}

/// `routy init` для открытого каталога без env.toml: создаёт api/env.toml и
/// переоткрывает проект — корнем становится api/.
#[tauri::command]
async fn init_project(app: tauri::AppHandle, state: State<'_, AppState>) -> CmdResult<ProjectInfo> {
    let root = {
        let guard = state.session.lock().await;
        let s = guard.as_ref().ok_or("no project open")?;
        if s.project.has_config() {
            return Err("project already has env.toml".into());
        }
        s.project.root.clone()
    };
    let api = routy_core::project::init(&root).map_err(err)?;
    open(app, &state, &api).await
}

/// Перечитать список файлов и env.toml, не пересоздавая слежение.
#[tauri::command]
async fn refresh_project(state: State<'_, AppState>) -> CmdResult<ProjectInfo> {
    let mut guard = state.session.lock().await;
    let s = guard.as_mut().ok_or("no project open")?;
    let project = Project::load(&s.project.root).map_err(err)?;
    let c = (&project.config, &s.project.config);
    if c.0.env != c.1.env || c.0.vars != c.1.vars || c.0.secrets != c.1.secrets {
        s.runners.clear();
        s.lang = None;
    }
    s.project = project;
    project_info(&s.project)
}

#[tauri::command]
async fn read_request(state: State<'_, AppState>, path: String) -> CmdResult<String> {
    let guard = state.session.lock().await;
    let s = guard.as_ref().ok_or("no project open")?;
    std::fs::read_to_string(inside(&s.project.root, &path)?).map_err(err)
}

#[tauri::command]
async fn write_request(state: State<'_, AppState>, path: String, content: String) -> CmdResult<()> {
    let guard = state.session.lock().await;
    let s = guard.as_ref().ok_or("no project open")?;
    let full = inside(&s.project.root, &path)?;
    if let Some(dir) = full.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    std::fs::write(full, content).map_err(err)
}

/// Переименование или перенос файла запроса или каталога. Цель не перезаписывается.
#[tauri::command]
async fn rename_path(state: State<'_, AppState>, from: String, to: String) -> CmdResult<()> {
    let guard = state.session.lock().await;
    let s = guard.as_ref().ok_or("no project open")?;
    let root = &s.project.root;
    let (src, dst) = (inside(root, &from)?, inside(root, &to)?);
    if dst.exists() {
        return Err(format!("{to} already exists"));
    }
    if dst.starts_with(&src) {
        return Err("can't move a folder into itself".into());
    }
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    std::fs::rename(&src, &dst).map_err(err)?;
    prune_empty(root, src.parent());
    Ok(())
}

/// Удаляет файл запроса или все `*.http` и `*.routy` в каталоге. Прочие файлы не трогаем:
/// каталог исчезает, только если в нём больше ничего не осталось.
#[tauri::command]
async fn delete_path(state: State<'_, AppState>, path: String) -> CmdResult<()> {
    let guard = state.session.lock().await;
    let s = guard.as_ref().ok_or("no project open")?;
    let root = &s.project.root;
    let full = inside(root, &path)?;
    if full.is_dir() {
        let exts = [discover::EXTENSION, discover::ROUTY_EXTENSION];
        for f in discover::files(&full, &exts).map_err(err)? {
            let f = full.join(f);
            std::fs::remove_file(&f).map_err(err)?;
            prune_empty(&full, f.parent());
        }
        prune_empty(root, Some(&full));
    } else if full
        .extension()
        .is_some_and(|e| e == discover::EXTENSION || e == discover::ROUTY_EXTENSION)
    {
        std::fs::remove_file(&full).map_err(err)?;
        prune_empty(root, full.parent());
    } else {
        return Err(format!("not a request file: {path}"));
    }
    Ok(())
}

/// Удаляет пустые каталоги от `dir` вверх, не выше `root` (и не сам `root`).
fn prune_empty(root: &Path, mut dir: Option<&Path>) {
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

#[derive(Serialize)]
struct ParseError {
    line: Option<usize>,
    /// Столбец (с 1) — есть у ошибок `*.routy`.
    col: Option<usize>,
    message: String,
}

fn parse_error(e: routy_core::Error) -> ParseError {
    match e {
        routy_core::Error::Parse { line, msg } => ParseError {
            line: Some(line),
            col: None,
            message: msg,
        },
        routy_core::Error::Syntax { line, col, msg } => ParseError {
            line: Some(line),
            col: Some(col),
            message: msg,
        },
        other => ParseError {
            line: None,
            col: None,
            message: other.to_string(),
        },
    }
}

/// Все `*.routy` проекта, а файл `path` — с текстом из редактора.
fn workspace(root: &Path, path: &str, content: String) -> CmdResult<Workspace> {
    let mut ws = Workspace::load(root).map_err(err)?;
    ws.add(PathBuf::from(path), content);
    Ok(ws)
}

/// `routy check` для открытого `*.routy` на лету: синтаксис, имена вызовов, аргументы, формы, циклы.
#[tauri::command]
async fn check_routy(
    state: State<'_, AppState>,
    path: String,
    content: String,
) -> CmdResult<Vec<ParseError>> {
    let root = {
        let guard = state.session.lock().await;
        let s = guard.as_ref().ok_or("no project open")?;
        inside(&s.project.root, &path)?;
        s.project.root.clone()
    };
    let ws = workspace(&root, &path, content)?;
    let full = root.join(&path);
    Ok(ws
        .check()
        .into_iter()
        .filter(|d| d.path == full)
        .map(|d| ParseError {
            line: Some(d.line),
            col: Some(d.col),
            message: d.msg,
        })
        .collect())
}

#[derive(Serialize)]
struct Callable {
    /// Как вызывать: `Login` или `users.Create`, если имя не уникально
    name: String,
    kind: &'static str,
    params: Vec<String>,
    /// Первая строка описания
    doc: Option<String>,
}

#[derive(Serialize)]
struct Symbols {
    callables: Vec<Callable>,
    shapes: Vec<String>,
}

/// Запросы, сценарии и формы проекта — для автодополнения в `*.routy`.
#[tauri::command]
async fn routy_symbols(state: State<'_, AppState>) -> CmdResult<Symbols> {
    let root = {
        let guard = state.session.lock().await;
        guard
            .as_ref()
            .ok_or("no project open")?
            .project
            .root
            .clone()
    };
    let ws = Workspace::load(&root).map_err(err)?;
    let mut callables = Vec::new();
    for (r, name) in ws.callables() {
        let qualified = if ws.resolve(&[name.to_string()]).is_ok() {
            name.to_string()
        } else {
            let folder = ws.sources[r.file].folder();
            if folder.is_empty() {
                name.to_string()
            } else {
                format!("{folder}.{name}")
            }
        };
        let (kind, doc) = match ws.item(r) {
            Item::Flow(f) => ("flow", f.doc.description.lines().next().map(String::from)),
            Item::Request(q) => (
                "request",
                Some(format!(
                    "{} {}",
                    q.method,
                    q.target.span.text(&ws.sources[r.file].text)
                )),
            ),
            _ => continue,
        };
        callables.push(Callable {
            name: qualified,
            kind,
            params: ws.params_of(r),
            doc,
        });
    }
    let mut shapes: Vec<String> = ws
        .sources
        .iter()
        .flat_map(|s| &s.file.items)
        .filter_map(|i| match i {
            Item::Shape(d) => Some(d.name.clone()),
            _ => None,
        })
        .collect();
    shapes.sort();
    Ok(Symbols { callables, shapes })
}

#[derive(Serialize)]
struct RoutyResult {
    /// Имя запущенного запроса или сценария
    name: String,
    /// Запрос: ответ (он же запись истории)
    entry: Option<Entry>,
    /// Сценарий: проверки, сохранённое и trace
    flow: Option<FlowOutcome>,
}

/// Запрос или сценарий `*.routy` на строке `line` (текст — из редактора, даже несохранённый).
/// Сессия заблокирована только на подготовку и запись результата — HTTP идёт без неё.
#[tauri::command]
async fn run_routy(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: u64,
    env: Option<String>,
    path: String,
    content: String,
    line: usize,
) -> CmdResult<RoutyResult> {
    let (lang, root, saved, cached) = {
        let mut guard = state.session.lock().await;
        let s = guard.as_mut().ok_or("no project open")?;
        inside(&s.project.root, &path)?;
        let runner = s.runner(env.as_deref())?;
        let (saved, cached) = (runner.vars.saved.clone(), runner.cached_calls.clone());
        (
            s.lang_run(env.as_deref())?,
            s.project.root.clone(),
            saved,
            cached,
        )
    };
    let (env_name, lang) = lang;
    let ws = workspace(&root, &path, content)?;
    let full = root.join(&path);
    if let Some(d) = ws.errors.iter().find(|d| d.path == full) {
        return Err(format!("{}:{}: {}", d.line, d.col, d.msg));
    }
    let file = ws
        .file_index(Path::new(&path))
        .ok_or("file is not in the project")?;
    let item = ws
        .item_at(file, line)
        .ok_or("no request or flow in this file")?;
    let name = ws.item(item).name().unwrap_or("request").to_string();

    let mut guard = lang.lock().await;
    let l = &mut *guard;
    l.run.set_workspace(ws);
    l.runner.vars.saved = saved;
    l.runner.cached_calls = cached;
    l.run.confirm = Some(Box::new(move |name, req| {
        use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
        let dialog = app
            .dialog()
            .message(format!("{} {}\n\nin `{env_name}`", req.method, req.url))
            .title(format!("Send {name}?"))
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Send".into(),
                "Cancel".into(),
            ));
        tokio::task::block_in_place(|| dialog.blocking_show())
    }));

    let (cancel, cancelled) = oneshot::channel();
    state.inflight.lock().unwrap().insert(id, cancel);
    let result = tokio::select! {
        r = l.run.run_item(&mut l.runner, item) => Some(r),
        _ = cancelled => None,
    };
    state.inflight.lock().unwrap().remove(&id);
    l.run.confirm = None;
    let (saved, cached) = (l.runner.vars.saved.clone(), l.runner.cached_calls.clone());
    drop(guard);

    let mut guard = state.session.lock().await;
    let s = guard.as_mut().ok_or("no project open")?;
    let runner = s.runner(env.as_deref())?;
    runner.vars.saved = saved;
    runner.cached_calls = cached;
    runner.persist_saved().map_err(err)?;
    let env = runner.env.clone();
    match result.ok_or("cancelled")?.map_err(err)? {
        Outcome::Request(outcome) => Ok(RoutyResult {
            name,
            entry: Some(s.history.push(&path, &env, outcome).cloned().map_err(err)?),
            flow: None,
        }),
        Outcome::Flow(flow) => Ok(RoutyResult {
            name,
            entry: None,
            flow: Some(flow),
        }),
    }
}

/// Новый прогон `*.routy`: забыть кеш вызовов и cookies.
#[tauri::command]
async fn reset_run(state: State<'_, AppState>) -> CmdResult<()> {
    let mut guard = state.session.lock().await;
    let s = guard.as_mut().ok_or("no project open")?;
    s.lang = None;
    Ok(())
}

/// Проверка синтаксиса на лету, без отправки.
#[tauri::command]
fn check_request(content: String) -> Option<ParseError> {
    routy_core::parse(&content).err().map(parse_error)
}

/// То же для env.toml, открытого в редакторе.
#[tauri::command]
fn check_config(content: String) -> Option<ParseError> {
    routy_core::project::check_config(&content)
        .err()
        .map(parse_error)
}

/// Отправляет текущее содержимое редактора (даже несохранённое). Сессия заблокирована
/// только на подстановку переменных и разбор ответа — HTTP идёт без неё.
#[tauri::command]
async fn send_request(
    state: State<'_, AppState>,
    id: u64,
    env: Option<String>,
    path: String,
    content: String,
) -> CmdResult<Entry> {
    let file = routy_core::parse(&content).map_err(err)?;
    let (client, request) = {
        let mut guard = state.session.lock().await;
        let s = guard.as_mut().ok_or("no project open")?;
        let runner = s.runner(env.as_deref())?;
        (runner.client(), runner.resolve(&file).map_err(err)?)
    };

    let (cancel, cancelled) = oneshot::channel();
    state.inflight.lock().unwrap().insert(id, cancel);
    let result = tokio::select! {
        r = routy_core::runner::send(&client, &request) => Some(r),
        _ = cancelled => None,
    };
    state.inflight.lock().unwrap().remove(&id);
    let response = result.ok_or("cancelled")?.map_err(err)?;

    let mut guard = state.session.lock().await;
    let s = guard.as_mut().ok_or("no project open")?;
    let runner = s.runner(env.as_deref())?;
    let outcome = runner.apply(&file, request, response);
    if !outcome.saved.is_empty() {
        runner.persist_saved().map_err(err)?;
    }
    let env = runner.env.clone();
    s.history.push(&path, &env, outcome).cloned().map_err(err)
}

#[tauri::command]
fn cancel_request(state: State<'_, AppState>, id: u64) {
    if let Some(cancel) = state.inflight.lock().unwrap().remove(&id) {
        let _ = cancel.send(());
    }
}

/// История ответов, новые сверху.
#[tauri::command]
async fn history(state: State<'_, AppState>) -> CmdResult<Vec<Entry>> {
    let guard = state.session.lock().await;
    let s = guard.as_ref().ok_or("no project open")?;
    Ok(s.history.entries().iter().rev().cloned().collect())
}

#[tauri::command]
async fn clear_history(state: State<'_, AppState>) -> CmdResult<()> {
    let mut guard = state.session.lock().await;
    let s = guard.as_mut().ok_or("no project open")?;
    s.history.clear().map_err(err)
}

/// Хранить ли историю на диске (вне репозитория). Выключение удаляет файл.
#[tauri::command]
async fn set_history_persist(state: State<'_, AppState>, persist: bool) -> CmdResult<()> {
    let mut guard = state.session.lock().await;
    let s = guard.as_mut().ok_or("no project open")?;
    if !persist {
        return s.history.stop_persisting().map_err(err);
    }
    let path = history_path(&s.project.root).ok_or("no data directory")?;
    s.history.persist_to(path).map_err(err)
}

/// Картинка из ответа как data: URL. Сырые байты есть только у ответов этой сессии.
#[tauri::command]
async fn response_image(state: State<'_, AppState>, id: u64) -> CmdResult<Option<String>> {
    let guard = state.session.lock().await;
    let s = guard.as_ref().ok_or("no project open")?;
    let Some(e) = s.history.get(id) else {
        return Ok(None);
    };
    let r = &e.outcome.response;
    let mime = r
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-type"))
        .map(|h| {
            h.value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string()
        })
        .filter(|m| m.starts_with("image/"));
    Ok(match mime {
        Some(mime) if !r.raw.is_empty() => Some(format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&r.raw)
        )),
        _ => None,
    })
}

/// Сохраняет тело ответа в выбранный пользователем файл (путь — из диалога сохранения).
#[tauri::command]
async fn save_body(state: State<'_, AppState>, id: u64, dest: PathBuf) -> CmdResult<()> {
    let guard = state.session.lock().await;
    let s = guard.as_ref().ok_or("no project open")?;
    let r = &s
        .history
        .get(id)
        .ok_or("response is no longer in history")?
        .outcome
        .response;
    let bytes = if r.raw.is_empty() {
        r.body.as_bytes()
    } else {
        &r.raw
    };
    std::fs::write(dest, bytes).map_err(err)
}

/// Переменные окружения с источниками для панели. Секреты — только маской.
#[tauri::command]
async fn variables(state: State<'_, AppState>, env: Option<String>) -> CmdResult<Vec<VarInfo>> {
    let mut guard = state.session.lock().await;
    let s = guard.as_mut().ok_or("no project open")?;
    let mut list = s.runner(env.as_deref())?.variables().map_err(err)?;
    for v in list.iter_mut().filter(|v| v.secret) {
        v.value = v.value.as_deref().map(mask);
    }
    Ok(list)
}

/// Забыть значения `> save` окружения.
#[tauri::command]
async fn clear_saved(state: State<'_, AppState>, env: Option<String>) -> CmdResult<()> {
    let mut guard = state.session.lock().await;
    let s = guard.as_mut().ok_or("no project open")?;
    s.runner(env.as_deref())?.clear_saved().map_err(err)
}

#[derive(Serialize)]
struct VarName {
    name: String,
    /// `"env"` — из env.toml, `"saved"` — из `> save`, `"secret"` — объявлен в `secrets`,
    /// `"dynamic"` — `$uuid` и т.п.
    source: &'static str,
    /// Значение только для env.toml, для `dynamic` — описание: в saved часто токены.
    value: Option<String>,
}

/// Известные имена переменных окружения — для автодополнения `{{var}}`.
/// Секреты из хранилища перечислить нельзя — только объявленные в `secrets`.
#[tauri::command]
async fn var_names(state: State<'_, AppState>, env: Option<String>) -> CmdResult<Vec<VarName>> {
    let mut guard = state.session.lock().await;
    let s = guard.as_mut().ok_or("no project open")?;
    let declared = s.project.config.secrets.clone();
    let vars = &s.runner(env.as_deref())?.vars;
    let saved = vars.saved.keys().map(|name| VarName {
        name: name.clone(),
        source: "saved",
        value: None,
    });
    let from_env = vars
        .env
        .iter()
        .filter(|(name, _)| !vars.saved.contains_key(*name))
        .map(|(name, value)| VarName {
            name: name.clone(),
            source: "env",
            value: Some(value.clone()),
        });
    let secrets = declared
        .into_iter()
        .filter(|name| !vars.saved.contains_key(name) && !vars.env.contains_key(name))
        .map(|name| VarName {
            name,
            source: "secret",
            value: None,
        });
    let dynamic = dynamic::NAMES.iter().map(|(name, about)| VarName {
        name: name.to_string(),
        source: "dynamic",
        value: Some(about.to_string()),
    });
    Ok(saved
        .chain(from_env)
        .chain(secrets)
        .chain(dynamic)
        .collect())
}

#[tauri::command]
async fn set_secret(
    state: State<'_, AppState>,
    env: Option<String>,
    name: String,
    value: String,
) -> CmdResult<()> {
    let guard = state.session.lock().await;
    let s = guard.as_ref().ok_or("no project open")?;
    let env = s.project.resolve_env(env.as_deref()).map_err(err)?;
    let store = routy_core::runner::secret_store(&s.project, &env)
        .ok_or("no keyring support in this build")?;
    store.set(&name, &value).map_err(err)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
#[derive(Serialize)]
struct ImportReport {
    /// Каталог, который сканировался
    dir: PathBuf,
    plan: routy_core::import::Plan,
    /// Созданные (при `apply`), исправленные и удалённые файлы, относительно проекта
    created: Vec<String>,
}

/// Каталог с кодом по умолчанию: над `api/` — репозиторий, иначе сам проект.
fn default_source_dir(root: &Path) -> PathBuf {
    match root.file_name() {
        Some(n) if n == "api" => root.parent().unwrap_or(root).to_path_buf(),
        _ => root.to_path_buf(),
    }
}

/// Роуты из Go-кода в `dir` против файлов проекта (`routy import go`); `apply` — создать недостающие.
#[tauri::command]
async fn import_go(
    state: State<'_, AppState>,
    dir: Option<PathBuf>,
    apply: bool,
) -> CmdResult<ImportReport> {
    let root = {
        let guard = state.session.lock().await;
        let s = guard.as_ref().ok_or("no project open")?;
        if !s.project.has_config() {
            return Err("create env.toml first".into());
        }
        s.project.root.clone()
    };
    let dir = dir.unwrap_or_else(|| default_source_dir(&root));
    tauri::async_runtime::spawn_blocking(move || {
        use routy_core::import;
        let plan = import::plan_go(&dir, &root, &[], import::DEFAULT_BASE).map_err(err)?;
        let created = if apply {
            import::apply(&root, &plan).map_err(err)?
        } else {
            Vec::new()
        };
        Ok(ImportReport {
            dir,
            plan,
            created: created
                .iter()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .collect(),
        })
    })
    .await
    .map_err(err)?
}

/// Исправить расхождения с кодом (`routy import go --fix`): `ids` — выбранные (`Change::id`),
/// без них — все исправимые; `prune` — удалить файлы, где все запросы — к пропавшим роутам.
/// Возвращает новый план.
#[tauri::command]
async fn import_fix(
    state: State<'_, AppState>,
    dir: Option<PathBuf>,
    ids: Option<Vec<String>>,
    prune: bool,
) -> CmdResult<ImportReport> {
    let root = {
        let guard = state.session.lock().await;
        let s = guard.as_ref().ok_or("no project open")?;
        s.project.root.clone()
    };
    let dir = dir.unwrap_or_else(|| default_source_dir(&root));
    tauri::async_runtime::spawn_blocking(move || {
        use routy_core::import;
        let plan = import::plan_go(&dir, &root, &[], import::DEFAULT_BASE).map_err(err)?;
        let fixes = import::fixes(&plan, |c| {
            ids.as_ref().is_none_or(|ids| ids.contains(&c.id))
        })
        .map_err(err)?;
        import::write_fixes(&root, &fixes).map_err(err)?;
        let mut changed: Vec<String> = fixes
            .iter()
            .map(|f| f.file.to_string_lossy().replace('\\', "/"))
            .collect();
        if prune {
            changed.extend(
                import::prune(&root, &plan)
                    .map_err(err)?
                    .iter()
                    .map(|p| p.to_string_lossy().replace('\\', "/")),
            );
        }
        let plan = import::plan_go(&dir, &root, &[], import::DEFAULT_BASE).map_err(err)?;
        Ok(ImportReport {
            dir,
            plan,
            created: changed,
        })
    })
    .await
    .map_err(err)?
}

pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init());
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }
    builder
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            open_project,
            init_project,
            refresh_project,
            read_request,
            write_request,
            check_request,
            check_routy,
            routy_symbols,
            run_routy,
            reset_run,
            check_config,
            rename_path,
            delete_path,
            send_request,
            cancel_request,
            history,
            clear_history,
            set_history_persist,
            response_image,
            save_body,
            variables,
            clear_saved,
            var_names,
            set_secret,
            import_go,
            import_fix,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Routy");
}
