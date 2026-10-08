//! Тонкая прослойка между React-фронтом и `routy-core`. Вся логика — в core,
//! здесь только команды Tauri, состояние открытого проекта и слежение за файлами.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use base64::Engine;
use notify::{RecursiveMode, Watcher};
use routy_core::history::{Entry, History, history_path};
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

struct Session {
    project: Project,
    /// Раннер на окружение: в нём живут значения `> save` между запросами.
    runners: HashMap<String, Runner>,
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
}

/// Метод из строки запроса без полного разбора: файл с ошибкой тоже получает значок.
fn request_method(src: &str) -> String {
    src.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("//"))
        .and_then(|l| l.split_whitespace().next())
        .filter(|w| w.bytes().all(|b| b.is_ascii_uppercase()))
        .unwrap_or("GET")
        .to_string()
}

fn project_info(p: &Project) -> CmdResult<ProjectInfo> {
    let files: Vec<String> = discover::request_files(&p.root)
        .map_err(err)?
        .into_iter()
        .map(|f| f.to_string_lossy().replace('\\', "/"))
        .collect();
    let methods = files
        .iter()
        .map(|f| {
            let src = std::fs::read_to_string(p.root.join(f)).unwrap_or_default();
            (f.clone(), request_method(&src))
        })
        .collect();
    Ok(ProjectInfo {
        root: p.root.clone(),
        id: p.id(),
        envs: p.env_names(),
        default_env: p.resolve_env(None).ok(),
        has_config: p.has_config(),
        files,
        methods,
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

/// Удаляет файл запроса или все `*.http` в каталоге. Прочие файлы не трогаем:
/// каталог исчезает, только если в нём больше ничего не осталось.
#[tauri::command]
async fn delete_path(state: State<'_, AppState>, path: String) -> CmdResult<()> {
    let guard = state.session.lock().await;
    let s = guard.as_ref().ok_or("no project open")?;
    let root = &s.project.root;
    let full = inside(root, &path)?;
    if full.is_dir() {
        for f in discover::request_files(&full).map_err(err)? {
            let f = full.join(f);
            std::fs::remove_file(&f).map_err(err)?;
            prune_empty(&full, f.parent());
        }
        prune_empty(root, Some(&full));
    } else if full.extension().is_some_and(|e| e == discover::EXTENSION) {
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
    message: String,
}

fn parse_error(e: routy_core::Error) -> ParseError {
    match e {
        routy_core::Error::Parse { line, msg } => ParseError {
            line: Some(line),
            message: msg,
        },
        other => ParseError {
            line: None,
            message: other.to_string(),
        },
    }
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
    /// Созданные файлы (при `apply`), относительно проекта
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running Routy");
}
