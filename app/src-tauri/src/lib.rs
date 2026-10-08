//! Тонкая прослойка между React-фронтом и `routy-core`. Вся логика — в core,
//! здесь только команды Tauri, состояние открытого проекта и слежение за файлами.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use notify::{RecursiveMode, Watcher};
use routy_core::runner::{Options, RunOutcome};
use routy_core::{Project, Runner, discover};
use serde::Serialize;
use tauri::{Emitter, State};
use tokio::sync::Mutex;

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

struct Session {
    project: Project,
    /// Раннер на окружение: в нём живут значения `> save` между запросами.
    runners: HashMap<String, Runner>,
    _watcher: Option<notify::RecommendedWatcher>,
}

#[derive(Default)]
struct AppState(Mutex<Option<Session>>);

#[derive(Serialize)]
struct ProjectInfo {
    root: PathBuf,
    id: String,
    envs: Vec<String>,
    default_env: Option<String>,
    files: Vec<String>,
}

fn project_info(p: &Project) -> CmdResult<ProjectInfo> {
    let files = discover::request_files(&p.root)
        .map_err(err)?
        .into_iter()
        .map(|f| f.to_string_lossy().replace('\\', "/"))
        .collect();
    Ok(ProjectInfo {
        root: p.root.clone(),
        id: p.id(),
        envs: p.env_names(),
        default_env: p.resolve_env(None).ok(),
        files,
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

#[tauri::command]
async fn open_project(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    dir: PathBuf,
) -> CmdResult<ProjectInfo> {
    let project = Project::discover(&dir).map_err(err)?;
    let info = project_info(&project)?;
    let watcher = watch(app, &project.root);
    *state.0.lock().await = Some(Session {
        project,
        runners: HashMap::new(),
        _watcher: watcher,
    });
    Ok(info)
}

/// Перечитать список файлов и env.toml, не пересоздавая слежение.
#[tauri::command]
async fn refresh_project(state: State<'_, AppState>) -> CmdResult<ProjectInfo> {
    let mut guard = state.0.lock().await;
    let s = guard.as_mut().ok_or("no project open")?;
    let project = Project::load(&s.project.root).map_err(err)?;
    if project.config.env != s.project.config.env || project.config.vars != s.project.config.vars {
        s.runners.clear();
    }
    s.project = project;
    project_info(&s.project)
}

#[tauri::command]
async fn read_request(state: State<'_, AppState>, path: String) -> CmdResult<String> {
    let guard = state.0.lock().await;
    let s = guard.as_ref().ok_or("no project open")?;
    std::fs::read_to_string(inside(&s.project.root, &path)?).map_err(err)
}

#[tauri::command]
async fn write_request(state: State<'_, AppState>, path: String, content: String) -> CmdResult<()> {
    let guard = state.0.lock().await;
    let s = guard.as_ref().ok_or("no project open")?;
    let full = inside(&s.project.root, &path)?;
    if let Some(dir) = full.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    std::fs::write(full, content).map_err(err)
}

#[derive(Serialize)]
struct ParseError {
    line: Option<usize>,
    message: String,
}

/// Проверка синтаксиса на лету, без отправки.
#[tauri::command]
fn check_request(content: String) -> Option<ParseError> {
    routy_core::parse(&content).err().map(|e| match e {
        routy_core::Error::Parse { line, msg } => ParseError {
            line: Some(line),
            message: msg,
        },
        other => ParseError {
            line: None,
            message: other.to_string(),
        },
    })
}

/// Отправляет текущее содержимое редактора (даже несохранённое).
#[tauri::command]
async fn send_request(
    state: State<'_, AppState>,
    env: Option<String>,
    content: String,
) -> CmdResult<RunOutcome> {
    let file = routy_core::parse(&content).map_err(err)?;
    let mut guard = state.0.lock().await;
    let s = guard.as_mut().ok_or("no project open")?;
    let env = s.project.resolve_env(env.as_deref()).map_err(err)?;
    if !s.runners.contains_key(&env) {
        let mut r = Runner::new(s.project.clone(), Some(&env), Options::default()).map_err(err)?;
        r.load_saved().map_err(err)?;
        s.runners.insert(env.clone(), r);
    }
    let runner = s.runners.get_mut(&env).expect("inserted above");
    let outcome = runner.run(&file).await.map_err(err)?;
    if !outcome.saved.is_empty() {
        runner.persist_saved().map_err(err)?;
    }
    Ok(outcome)
}

#[tauri::command]
async fn set_secret(
    state: State<'_, AppState>,
    env: Option<String>,
    name: String,
    value: String,
) -> CmdResult<()> {
    let guard = state.0.lock().await;
    let s = guard.as_ref().ok_or("no project open")?;
    let env = s.project.resolve_env(env.as_deref()).map_err(err)?;
    let store = routy_core::runner::secret_store(&s.project, &env)
        .ok_or("no keyring support in this build")?;
    store.set(&name, &value).map_err(err)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
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
            refresh_project,
            read_request,
            write_request,
            check_request,
            send_request,
            set_secret,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Routy");
}
