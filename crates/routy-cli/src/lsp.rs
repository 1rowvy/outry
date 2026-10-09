//! `routy lsp`: языковой сервер для `*.routy` (stdio). Вся логика — в `routy-core`
//! (`lang::Workspace::check`, `lang::ide`, `import::plan_with`); здесь только протокол.
//!
//! - диагностика: `routy check`, `check --env <текущее>` (предупреждения) и расхождения
//!   с Go-кодом (`routy import go --check`), у исправимых — Quick Fix;
//! - автодополнение, hover, переход к определению, outline, форматирование (`routy fmt`);
//! - code lens «▶ Send» / «▶ Run flow»: запрос уходит в отдельном потоке, ответ пишется в файл
//!   и открывается через `window/showDocument`, итог — в `window/showMessage`.
//!
//! Настройки (`initializationOptions` или `workspace/didChangeConfiguration` → `routy`):
//! `env`, `keyring` (по умолчанию true), `import` (сравнение с Go, по умолчанию true), `goDir`.
//!
//! Клиент с `capabilities.experimental.routyUi: true` (расширение VS Code) показывает ответы
//! сам: `routy.run` только возвращает итог (без файла и сообщений), code lens ведут на его
//! команды (`routy.send`, `routy.sendIn`, `routy.copyCurl`), окружение — в его строке
//! состояния. Для его панелей: запрос `routy/state` (окружения, запросы проекта, переменные)
//! и уведомление `routy/didChange`, когда они могли измениться.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use crossbeam_channel::{Receiver, Sender, select};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::*;
use routy_core::import::{self, Plan, Scan, Severity};
use routy_core::lang::exec::{Outcome, Run};
use routy_core::lang::ide::{self, Target};
use routy_core::lang::scope::Binding;
use routy_core::lang::{ItemRef, Workspace, ast};
use routy_core::runner::Options;
use routy_core::vars::{Source, VarInfo, mask};
use routy_core::{Project, Runner};
use serde_json::{Value, json};

pub const RUN_COMMAND: &str = "routy.run";
pub const ENV_COMMAND: &str = "routy.selectEnv";
pub const CURL_COMMAND: &str = "routy.curl";

/// Команды клиента с `routyUi` (их регистрирует расширение, не сервер).
const UI_SEND: &str = "routy.send";
const UI_SEND_IN: &str = "routy.sendIn";
const UI_CURL: &str = "routy.copyCurl";

/// Флаги командной строки `routy lsp`; настройки клиента их перекрывают.
#[derive(Debug, Clone, Default)]
pub struct Opts {
    pub env: Option<String>,
    pub no_keyring: bool,
}

pub fn run(opts: Opts) -> anyhow::Result<()> {
    let (conn, io) = Connection::stdio();
    serve(conn, opts)?;
    io.join()?;
    Ok(())
}

/// Сервер на готовом соединении (stdio или в памяти — в тестах). Возвращается после `exit`.
pub fn serve(conn: Connection, opts: Opts) -> anyhow::Result<()> {
    let (id, params) = conn.initialize_start()?;
    let init: InitializeParams = serde_json::from_value(params)?;
    let result = InitializeResult {
        capabilities: capabilities(),
        server_info: Some(ServerInfo {
            name: "routy".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
    };
    conn.initialize_finish(id, serde_json::to_value(result)?)?;
    let mut server = Server::new(conn.sender.clone(), &init, opts)?;
    server.start();
    server.main_loop(&conn)
}

fn capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::FULL),
                save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                ..Default::default()
            },
        )),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![".".into(), "(".into(), ",".into(), "[".into()]),
            ..Default::default()
        }),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        document_formatting_provider: Some(OneOf::Left(true)),
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![
                CodeActionKind::QUICKFIX,
                CodeActionKind::SOURCE_FIX_ALL,
            ]),
            ..Default::default()
        })),
        code_lens_provider: Some(CodeLensOptions {
            resolve_provider: Some(false),
        }),
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: vec![RUN_COMMAND.into(), ENV_COMMAND.into(), CURL_COMMAND.into()],
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// События не от клиента: готов разбор Go, закончился запуск.
enum Internal {
    Scanned(u64, Result<Scan, String>),
    RunDone,
}

/// Чем закончить ответ клиента на запрос сервера.
enum Pending {
    SelectEnv,
}

/// Ответы клиента на запросы из потока запусков (`confirm: true`).
type Waiting = Arc<Mutex<HashMap<RequestId, Sender<Response>>>>;

struct Client {
    snippets: bool,
    markdown: bool,
    show_document: bool,
    watch: bool,
    lens_refresh: bool,
    /// Расширение routy: ответы, окружение и панели — его (см. описание модуля).
    ui: bool,
}

struct Go {
    enabled: bool,
    dir: PathBuf,
    scan: Option<Scan>,
    /// Номер последнего запущенного разбора: результат старого отбрасывается.
    generation: u64,
    plan: Option<Plan>,
}

struct Server {
    out: Sender<Message>,
    client: Client,
    project: Project,
    env: String,
    keyring: bool,
    /// Только для значений переменных: запросы идут в потоке запусков со своим `Runner`.
    runner: Option<Runner>,
    var_cache: HashMap<String, Option<VarInfo>>,
    vars_list: Option<Vec<VarInfo>>,
    /// Открытые файлы: абсолютный путь → текст.
    docs: HashMap<PathBuf, String>,
    /// Читать `*.routy` проекта с диска. Нет, пока проект — просто рабочая папка без `env.toml`:
    /// обходить её целиком (это может быть весь `~`) незачем.
    from_disk: bool,
    ws: Workspace,
    published: HashSet<Url>,
    go: Go,
    internal: (Sender<Internal>, Receiver<Internal>),
    jobs: Sender<Job>,
    waiting: Waiting,
    pending: HashMap<RequestId, Pending>,
    ids: Arc<AtomicU64>,
}

impl Server {
    fn new(out: Sender<Message>, init: &InitializeParams, opts: Opts) -> anyhow::Result<Server> {
        let caps = serde_json::to_value(&init.capabilities)?;
        let flag = |p: &str| caps.pointer(p).and_then(Value::as_bool).unwrap_or(false);
        let client = Client {
            snippets: flag("/textDocument/completion/completionItem/snippetSupport"),
            markdown: caps
                .pointer("/textDocument/hover/contentFormat")
                .and_then(Value::as_array)
                .is_none_or(|f| f.iter().any(|k| k == "markdown")),
            show_document: flag("/window/showDocument/support"),
            watch: flag("/workspace/didChangeWatchedFiles/dynamicRegistration"),
            lens_refresh: flag("/workspace/codeLens/refreshSupport"),
            ui: flag("/experimental/routyUi"),
        };
        let settings = init.initialization_options.clone().unwrap_or(Value::Null);
        let setting = |k: &str| {
            settings
                .get(k)
                .or_else(|| settings.pointer(&format!("/routy/{k}")))
        };

        #[allow(deprecated)]
        let root = init
            .workspace_folders
            .as_ref()
            .and_then(|f| f.first())
            .map(|f| f.uri.clone())
            .or_else(|| init.root_uri.clone())
            .and_then(|u| u.to_file_path().ok())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let project = Project::discover(&root)?;
        let env = setting("env")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or(opts.env);
        let env = project
            .resolve_env(env.as_deref())
            .or_else(|_| project.resolve_env(None))?;
        let keyring =
            !opts.no_keyring && setting("keyring").and_then(Value::as_bool) != Some(false);
        let go_dir = setting("goDir")
            .and_then(Value::as_str)
            .map(|d| root.join(d))
            .unwrap_or_else(|| default_source_dir(&project.root));
        let go = Go {
            enabled: setting("import").and_then(Value::as_bool) != Some(false),
            dir: go_dir,
            scan: None,
            generation: 0,
            plan: None,
        };

        let waiting: Waiting = Arc::default();
        let ids = Arc::new(AtomicU64::new(0));
        let internal = crossbeam_channel::unbounded();
        let jobs = spawn_worker(
            out.clone(),
            internal.0.clone(),
            waiting.clone(),
            ids.clone(),
            client.show_document,
            client.ui,
        );
        let mut s = Server {
            out,
            client,
            project,
            env,
            keyring,
            runner: None,
            var_cache: HashMap::new(),
            vars_list: None,
            docs: HashMap::new(),
            from_disk: false,
            ws: Workspace::default(),
            published: HashSet::new(),
            go,
            internal,
            jobs,
            waiting,
            pending: HashMap::new(),
            ids,
        };
        s.load_runner();
        s.from_disk = s.project.has_config();
        s.reload_workspace();
        Ok(s)
    }

    /// После `initialized`: слежение за файлами, разбор Go, первая диагностика.
    fn start(&mut self) {
        if self.client.watch {
            let watchers = ["**/*.routy", "**/*.http", "**/env.toml", "**/*.go"]
                .iter()
                .map(|g| FileSystemWatcher {
                    glob_pattern: GlobPattern::String(g.to_string()),
                    kind: None,
                })
                .collect();
            let params = RegistrationParams {
                registrations: vec![Registration {
                    id: "routy-watch".into(),
                    method: "workspace/didChangeWatchedFiles".into(),
                    register_options: serde_json::to_value(
                        DidChangeWatchedFilesRegistrationOptions { watchers },
                    )
                    .ok(),
                }],
            };
            self.request("client/registerCapability", params);
        }
        self.rescan_go();
        self.publish();
    }

    fn main_loop(&mut self, conn: &Connection) -> anyhow::Result<()> {
        let internal = self.internal.1.clone();
        loop {
            select! {
                recv(conn.receiver) -> msg => {
                    let Ok(msg) = msg else { return Ok(()) };
                    match msg {
                        Message::Request(req) => {
                            if conn.handle_shutdown(&req)? {
                                return Ok(());
                            }
                            self.on_request(req);
                        }
                        Message::Notification(n) => {
                            if n.method == "exit" {
                                return Ok(());
                            }
                            self.on_notification(n);
                        }
                        Message::Response(r) => self.on_response(r),
                    }
                }
                recv(internal) -> ev => {
                    if let Ok(ev) = ev {
                        self.on_internal(ev);
                    }
                }
            }
        }
    }

    fn send(&self, msg: impl Into<Message>) {
        let _ = self.out.send(msg.into());
    }

    fn notify<P: serde::Serialize>(&self, method: &str, params: P) {
        self.send(Notification::new(method.into(), params));
    }

    /// Запрос клиенту; ответ придёт в `on_response`.
    fn request<P: serde::Serialize>(&self, method: &str, params: P) -> RequestId {
        let id = next_id(&self.ids);
        self.send(Request::new(id.clone(), method.into(), params));
        id
    }

    fn message(&self, typ: MessageType, text: impl Into<String>) {
        self.notify(
            "window/showMessage",
            ShowMessageParams {
                typ,
                message: text.into(),
            },
        );
    }

    fn log(&self, text: impl Into<String>) {
        self.notify(
            "window/logMessage",
            LogMessageParams {
                typ: MessageType::LOG,
                message: text.into(),
            },
        );
    }

    /// Окружение, запросы или переменные могли измениться: расширению — перечитать `routy/state`.
    fn changed(&self) {
        if self.client.ui {
            self.notify("routy/didChange", json!({ "env": self.env }));
        }
    }

    // ---------- проект, переменные, рабочее пространство ----------

    fn load_runner(&mut self) {
        let opts = Options {
            use_keyring: self.keyring,
            ..Options::default()
        };
        self.runner = match Runner::new(self.project.clone(), Some(&self.env), opts) {
            Ok(mut r) => {
                if let Err(e) = r.load_saved() {
                    self.log(format!("routy: saved values: {e}"));
                }
                Some(r)
            }
            Err(e) => {
                self.log(format!("routy: {e}"));
                None
            }
        };
        self.var_cache.clear();
        self.vars_list = None;
    }

    fn var(&mut self, name: &str) -> Option<VarInfo> {
        if let Some(v) = self.var_cache.get(name) {
            return v.clone();
        }
        let runner = self.runner.as_ref()?;
        let info = match runner.vars.lookup(name) {
            Ok(Some((value, source))) => Some(VarInfo {
                name: name.to_string(),
                secret: source == Source::Secret
                    || self.project.config.secrets.iter().any(|s| s == name),
                source: Some(source),
                value: Some(value),
            }),
            _ => None,
        };
        self.var_cache.insert(name.to_string(), info.clone());
        info
    }

    fn vars(&mut self) -> Vec<VarInfo> {
        if self.vars_list.is_none() {
            self.vars_list = Some(
                self.runner
                    .as_ref()
                    .and_then(|r| r.variables().ok())
                    .unwrap_or_default(),
            );
        }
        self.vars_list.clone().unwrap_or_default()
    }

    /// Путь файла в `Workspace`: относительно корня проекта или абсолютный, если файл вне его.
    fn rel(&self, abs: &Path) -> PathBuf {
        abs.strip_prefix(&self.project.root)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| abs.to_path_buf())
    }

    fn reload_workspace(&mut self) {
        let empty = || Workspace::from_sources(&self.project.root, Vec::new());
        let mut ws = if self.from_disk {
            Workspace::load(&self.project.root).unwrap_or_else(|e| {
                self.log(format!("routy: {e}"));
                empty()
            })
        } else {
            empty()
        };
        let mut docs: Vec<(&PathBuf, &String)> = self.docs.iter().collect();
        docs.sort();
        for (abs, text) in docs {
            ws.add(self.rel(abs), text.clone());
        }
        self.ws = ws;
    }

    /// Проект ищется от открытого файла, как в CLI, если у текущего нет `env.toml` или файл вне
    /// его. Файл вне проекта с `env.toml`, рядом с которым своего нет, проект не меняет.
    fn maybe_switch_project(&mut self, abs: &Path) {
        if self.project.has_config() && abs.starts_with(&self.project.root) {
            return;
        }
        let Ok(p) = Project::discover(abs) else {
            return;
        };
        if (self.project.has_config() && !p.has_config())
            || (p.root == self.project.root && self.from_disk)
        {
            return;
        }
        self.from_disk = true;
        self.go.dir = default_source_dir(&p.root);
        self.project = p;
        self.env = self
            .project
            .resolve_env(Some(&self.env))
            .or_else(|_| self.project.resolve_env(None))
            .unwrap_or_else(|_| "default".into());
        self.load_runner();
        self.reload_workspace();
        self.go.scan = None;
        self.rescan_go();
        self.changed();
    }

    fn set_env(&mut self, env: &str) {
        let names = self.project.env_names();
        if !names.is_empty() && !names.iter().any(|n| n == env) {
            self.message(
                MessageType::ERROR,
                format!("routy: no environment `{env}` (have: {})", names.join(", ")),
            );
            return;
        }
        self.env = env.to_string();
        self.load_runner();
        self.publish();
        if self.client.lens_refresh {
            self.request("workspace/codeLens/refresh", ());
        }
        if self.client.ui {
            self.changed();
        } else {
            self.message(MessageType::INFO, format!("routy: environment `{env}`"));
        }
    }

    fn rescan_go(&mut self) {
        if !self.go.enabled || !self.project.has_config() {
            return;
        }
        self.go.generation += 1;
        let generation = self.go.generation;
        let dir = self.go.dir.clone();
        let tx = self.internal.0.clone();
        std::thread::spawn(move || {
            let scan = import::go::scan(&dir, &import::go::RouterQuery::builtin())
                .map_err(|e| e.to_string());
            let _ = tx.send(Internal::Scanned(generation, scan));
        });
    }

    /// Сравнение с Go по текущим текстам (открытые файлы — из редактора).
    fn replan(&mut self) {
        self.go.plan = None;
        let Some(scan) = &self.go.scan else {
            return;
        };
        if scan.routes.is_empty() {
            return;
        }
        let overlay: HashMap<PathBuf, String> = self
            .docs
            .iter()
            .filter(|(abs, _)| abs.starts_with(&self.project.root))
            .map(|(abs, t)| (self.rel(abs), t.clone()))
            .collect();
        match import::plan_with(
            &self.project.root,
            scan.clone(),
            import::DEFAULT_BASE,
            &overlay,
        ) {
            Ok(plan) => self.go.plan = Some(plan),
            Err(e) => self.log(format!("routy: import go: {e}")),
        }
    }

    fn text_of(&self, abs: &Path) -> Option<String> {
        if let Some(t) = self.docs.get(abs) {
            return Some(t.clone());
        }
        if let Some(s) = self.ws.sources.iter().find(|s| s.full == abs) {
            return Some(s.text.clone());
        }
        std::fs::read_to_string(abs).ok()
    }

    // ---------- диагностика ----------

    fn publish(&mut self) {
        self.replan();
        let mut by_file: HashMap<PathBuf, Vec<Diagnostic>> = HashMap::new();
        let mut texts: HashMap<PathBuf, Option<String>> = HashMap::new();
        let mut text = |s: &Server, p: &Path| -> Option<String> {
            texts
                .entry(p.to_path_buf())
                .or_insert_with(|| s.text_of(p))
                .clone()
        };

        for d in self.ws.check() {
            let Some(t) = text(self, &d.path) else {
                continue;
            };
            let range = word_range(&t, line_col_offset(&t, d.line, d.col));
            by_file.entry(d.path.clone()).or_default().push(Diagnostic {
                range,
                severity: Some(DiagnosticSeverity::ERROR),
                source: Some("routy".into()),
                message: d.msg,
                ..Default::default()
            });
        }

        if self.runner.is_some() {
            let envs = self.project.env_names();
            let env = self.env.clone();
            let ws = std::mem::take(&mut self.ws);
            let mut has = |n: &str| self.var(n).is_some();
            let diags = ws.check_env(&env, &envs, &mut has);
            self.ws = ws;
            for d in diags {
                let Some(t) = text(self, &d.path) else {
                    continue;
                };
                let range = word_range(&t, line_col_offset(&t, d.line, d.col));
                by_file.entry(d.path.clone()).or_default().push(Diagnostic {
                    range,
                    severity: Some(DiagnosticSeverity::WARNING),
                    source: Some(format!("routy env {env}")),
                    message: d.msg,
                    ..Default::default()
                });
            }
        }

        if let Some(plan) = &self.go.plan {
            for c in plan.changes() {
                let abs = self.project.root.join(&c.file);
                let Some(t) = text(self, &abs) else {
                    continue;
                };
                let range = word_range(&t, line_col_offset(&t, c.line, c.col));
                let go = self.go.dir.join(&c.go.file);
                let related = Url::from_file_path(&go).ok().map(|uri| {
                    vec![DiagnosticRelatedInformation {
                        location: Location {
                            uri,
                            range: Range::new(
                                Position::new(c.go.line.saturating_sub(1) as u32, 0),
                                Position::new(c.go.line.saturating_sub(1) as u32, 0),
                            ),
                        },
                        message: "in the code".into(),
                    }]
                });
                by_file.entry(abs).or_default().push(Diagnostic {
                    range,
                    severity: Some(match c.severity {
                        Severity::Error => DiagnosticSeverity::ERROR,
                        Severity::Warning => DiagnosticSeverity::WARNING,
                    }),
                    code: Some(NumberOrString::String(code_of(&c.kind))),
                    source: Some("routy import go".into()),
                    message: format!("{} ({}:{})", c.message, slash(&c.go.file), c.go.line),
                    related_information: related,
                    data: c.fix.as_ref().map(|_| json!({ "id": c.id })),
                    ..Default::default()
                });
            }
            for s in &plan.stale {
                let abs = self.project.root.join(&s.file);
                let Some(t) = text(self, &abs) else {
                    continue;
                };
                let at = line_col_offset(&t, s.line, 1);
                by_file.entry(abs).or_default().push(Diagnostic {
                    range: word_range(&t, at),
                    severity: Some(DiagnosticSeverity::WARNING),
                    source: Some("routy import go".into()),
                    message: format!("{} {}: no such route in code", s.method, s.url),
                    ..Default::default()
                });
            }
        }

        // Открытые файлы — всегда, и пустым списком: клиент знает, что проверка была.
        for abs in self.docs.keys() {
            by_file.entry(abs.clone()).or_default();
        }
        let mut now = HashSet::new();
        for (path, diagnostics) in by_file {
            let Ok(uri) = Url::from_file_path(&path) else {
                continue;
            };
            now.insert(uri.clone());
            self.notify(
                "textDocument/publishDiagnostics",
                PublishDiagnosticsParams {
                    uri,
                    diagnostics,
                    version: None,
                },
            );
        }
        for uri in self.published.difference(&now) {
            self.notify(
                "textDocument/publishDiagnostics",
                PublishDiagnosticsParams {
                    uri: uri.clone(),
                    diagnostics: Vec::new(),
                    version: None,
                },
            );
        }
        self.published = now;
    }

    // ---------- уведомления ----------

    fn on_notification(&mut self, n: Notification) {
        match n.method.as_str() {
            "textDocument/didOpen" => {
                let Ok(p) = serde_json::from_value::<DidOpenTextDocumentParams>(n.params) else {
                    return;
                };
                let Ok(abs) = p.text_document.uri.to_file_path() else {
                    return;
                };
                self.maybe_switch_project(&abs);
                self.ws.add(self.rel(&abs), p.text_document.text.clone());
                self.docs.insert(abs, p.text_document.text);
                self.publish();
            }
            "textDocument/didChange" => {
                let Ok(p) = serde_json::from_value::<DidChangeTextDocumentParams>(n.params) else {
                    return;
                };
                let Ok(abs) = p.text_document.uri.to_file_path() else {
                    return;
                };
                let Some(change) = p.content_changes.into_iter().last() else {
                    return;
                };
                self.ws.add(self.rel(&abs), change.text.clone());
                self.docs.insert(abs, change.text);
                self.publish();
            }
            "textDocument/didClose" => {
                let Ok(p) = serde_json::from_value::<DidCloseTextDocumentParams>(n.params) else {
                    return;
                };
                if let Ok(abs) = p.text_document.uri.to_file_path() {
                    self.docs.remove(&abs);
                    self.reload_workspace();
                    self.publish();
                }
            }
            "workspace/didChangeWatchedFiles" => {
                let Ok(p) = serde_json::from_value::<DidChangeWatchedFilesParams>(n.params) else {
                    return;
                };
                let paths: Vec<PathBuf> = p
                    .changes
                    .iter()
                    .filter_map(|c| c.uri.to_file_path().ok())
                    .collect();
                let ext = |p: &PathBuf, e: &str| p.extension().is_some_and(|x| x == e);
                let config = paths
                    .iter()
                    .find(|p| p.file_name().is_some_and(|n| n == "env.toml"));
                // `env.toml` появился после старта (`routy import go` в пустой папке) — ищем
                // проект от него, а не только от открытых файлов.
                if let Some(path) = config.filter(|_| !self.project.has_config()) {
                    self.maybe_switch_project(path);
                } else if config.is_some() {
                    if let Ok(project) = Project::load(&self.project.root) {
                        self.project = project;
                    }
                    self.load_runner();
                }
                if paths.iter().any(|p| ext(p, "go")) {
                    self.rescan_go();
                }
                self.reload_workspace();
                self.publish();
                self.changed();
            }
            "workspace/didChangeConfiguration" => {
                let Ok(p) = serde_json::from_value::<DidChangeConfigurationParams>(n.params) else {
                    return;
                };
                let env = p
                    .settings
                    .pointer("/routy/env")
                    .or_else(|| p.settings.get("env"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if let Some(env) = env.filter(|e| *e != self.env) {
                    self.set_env(&env);
                }
            }
            _ => {}
        }
    }

    fn on_internal(&mut self, ev: Internal) {
        match ev {
            Internal::Scanned(generation, scan) => {
                if generation != self.go.generation {
                    return;
                }
                match scan {
                    Ok(scan) => self.go.scan = Some(scan),
                    Err(e) => self.log(format!("routy: import go: {e}")),
                }
                self.publish();
            }
            Internal::RunDone => {
                if let Some(r) = &mut self.runner {
                    let _ = r.load_saved();
                }
                self.var_cache.clear();
                self.vars_list = None;
                self.publish();
                self.changed();
            }
        }
    }

    fn on_response(&mut self, r: Response) {
        if let Some(tx) = self.waiting.lock().unwrap().remove(&r.id) {
            let _ = tx.send(r);
            return;
        }
        match self.pending.remove(&r.id) {
            Some(Pending::SelectEnv) => {
                let picked = r
                    .result
                    .and_then(|v| serde_json::from_value::<Option<MessageActionItem>>(v).ok())
                    .flatten();
                if let Some(item) = picked {
                    self.set_env(&item.title);
                }
            }
            None => {}
        }
    }

    // ---------- запросы ----------

    fn on_request(&mut self, req: Request) {
        let id = req.id.clone();
        let result = match req.method.as_str() {
            "textDocument/completion" => self.completion(req.params),
            "textDocument/hover" => self.hover(req.params),
            "textDocument/definition" => self.definition(req.params),
            "textDocument/documentSymbol" => self.symbols(req.params),
            "textDocument/formatting" => self.formatting(req.params),
            "textDocument/codeAction" => self.code_actions(req.params),
            "textDocument/codeLens" => self.code_lens(req.params),
            "routy/state" => self.state(),
            "workspace/executeCommand" => match self.execute(id.clone(), req.params) {
                // Ответ на запуск пришлёт поток запусков.
                Ok(None) => return,
                Ok(Some(v)) => Ok(v),
                Err(e) => Err(e),
            },
            _ => {
                self.send(Response::new_err(
                    id,
                    lsp_server::ErrorCode::MethodNotFound as i32,
                    format!("unknown method {}", req.method),
                ));
                return;
            }
        };
        self.send(match result {
            Ok(v) => Response::new_ok(id, v),
            Err(e) => Response::new_err(
                id,
                lsp_server::ErrorCode::RequestFailed as i32,
                e.to_string(),
            ),
        });
    }

    /// Открытый файл и его индекс в `Workspace` (если разобрался).
    fn doc(&self, uri: &Url) -> Option<(PathBuf, String, Option<usize>)> {
        let abs = uri.to_file_path().ok()?;
        let text = self.text_of(&abs)?;
        let fi = self.ws.file_index(&self.rel(&abs));
        Some((abs, text, fi))
    }

    fn completion(&mut self, params: Value) -> anyhow::Result<Value> {
        let p: CompletionParams = serde_json::from_value(params)?;
        let pos = p.text_document_position;
        let Some((_, text, _)) = self.doc(&pos.text_document.uri) else {
            return Ok(Value::Null);
        };
        let offset = offset_of(&text, pos.position);
        let vars = self.vars();
        let envs = self.project.env_names();
        let env = ide::EnvInfo {
            name: &self.env,
            envs: &envs,
            vars: &vars,
        };
        let Some((start, items)) = ide::complete(&self.ws, &text, offset, &env) else {
            return Ok(Value::Null);
        };
        let range = Range::new(position(&text, start), pos.position);
        let items: Vec<CompletionItem> = items
            .into_iter()
            .map(|c| {
                let (insert, format) = match (&c.snippet, self.client.snippets) {
                    (Some(s), true) => (s.clone(), InsertTextFormat::SNIPPET),
                    (Some(s), false) => (plain(s), InsertTextFormat::PLAIN_TEXT),
                    (None, _) => (c.label.clone(), InsertTextFormat::PLAIN_TEXT),
                };
                CompletionItem {
                    sort_text: Some(format!("{}{}", rank(c.kind), c.label)),
                    kind: Some(completion_kind(c.kind)),
                    detail: c.detail,
                    documentation: c.doc.map(|d| {
                        Documentation::MarkupContent(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: d,
                        })
                    }),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                        range,
                        new_text: insert,
                    })),
                    insert_text_format: Some(format),
                    label: c.label,
                    ..Default::default()
                }
            })
            .collect();
        Ok(serde_json::to_value(CompletionResponse::Array(items))?)
    }

    fn hover(&mut self, params: Value) -> anyhow::Result<Value> {
        let p: HoverParams = serde_json::from_value(params)?;
        let pos = p.text_document_position_params;
        let Some((_, text, Some(fi))) = self.doc(&pos.text_document.uri) else {
            return Ok(Value::Null);
        };
        if self.ws.sources[fi].text != text {
            return Ok(Value::Null);
        }
        let offset = offset_of(&text, pos.position);
        let ws = std::mem::take(&mut self.ws);
        let env = self.env.clone();
        let found = ide::hover(&ws, fi, offset, &env, &mut |n| self.var(n));
        self.ws = ws;
        let Some((span, md)) = found else {
            return Ok(Value::Null);
        };
        let contents = if self.client.markdown {
            HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: md,
            })
        } else {
            HoverContents::Markup(MarkupContent {
                kind: MarkupKind::PlainText,
                value: md,
            })
        };
        Ok(serde_json::to_value(Hover {
            contents,
            range: Some(Range::new(
                position(&text, span.start),
                position(&text, span.end),
            )),
        })?)
    }

    fn location(&self, abs: &Path, start: usize, end: usize) -> Option<Location> {
        let text = self.text_of(abs)?;
        Some(Location {
            uri: Url::from_file_path(abs).ok()?,
            range: Range::new(position(&text, start), position(&text, end)),
        })
    }

    fn line_location(path: &Path, line: usize) -> Option<Location> {
        let p = Position::new(line.saturating_sub(1) as u32, 0);
        Some(Location {
            uri: Url::from_file_path(path).ok()?,
            range: Range::new(p, p),
        })
    }

    fn definition(&mut self, params: Value) -> anyhow::Result<Value> {
        let p: GotoDefinitionParams = serde_json::from_value(params)?;
        let pos = p.text_document_position_params;
        let Some((abs, text, Some(fi))) = self.doc(&pos.text_document.uri) else {
            return Ok(Value::Null);
        };
        if self.ws.sources[fi].text != text {
            return Ok(Value::Null);
        }
        let Some(sym) = ide::symbol_at(&self.ws, fi, offset_of(&text, pos.position)) else {
            return Ok(Value::Null);
        };
        let mut out: Vec<Location> = Vec::new();
        let span_in = |s: &Server, span: ast::Span| s.location(&abs, span.start, span.start);
        match sym.target {
            Target::Item(r) => {
                let s = &self.ws.sources[r.file];
                let at = self.ws.item(r).span().start;
                out.extend(self.location(&s.full, at, at));
            }
            Target::Shape { name, decl: false } => {
                for s in &self.ws.sources {
                    for item in &s.file.items {
                        if let ast::Item::Shape(d) = item {
                            if d.name == name {
                                out.extend(self.location(&s.full, d.span.start, d.span.start));
                            }
                        }
                    }
                }
            }
            Target::Shape { name, decl: true } => {
                if let Some(scan) = &self.go.scan {
                    for d in scan.shapes.iter().filter(|d| d.name == name) {
                        out.extend(Self::line_location(&self.go.dir.join(&d.source), d.line));
                    }
                }
            }
            Target::Handler(h) => {
                if let Some(scan) = &self.go.scan {
                    for r in scan
                        .routes
                        .iter()
                        .filter(|r| r.handler.as_deref() == Some(&h))
                    {
                        out.extend(Self::line_location(&self.go.dir.join(&r.source), r.line));
                    }
                }
            }
            Target::Name(_, Binding::Let(span) | Binding::Local(span) | Binding::Param(span)) => {
                out.extend(span_in(self, span));
            }
            Target::Name(name, Binding::PathParam(span)) => {
                out.extend(span_in(self, span));
                self.var_definitions(&name, &mut out);
            }
            Target::Name(name, Binding::Var) => self.var_definitions(&name, &mut out),
            _ => {}
        }
        if out.is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::to_value(GotoDefinitionResponse::Array(out))?)
    }

    /// Переменная: строка в `env.toml` и все `save` с этим именем.
    fn var_definitions(&self, name: &str, out: &mut Vec<Location>) {
        if let Some(line) = self.project.var_line(&self.env, name) {
            out.extend(Self::line_location(
                &self.project.root.join(routy_core::project::CONFIG_FILE),
                line,
            ));
        }
        for (fi, span) in ide::save_sites(&self.ws, name) {
            out.extend(self.location(&self.ws.sources[fi].full, span.start, span.start));
        }
    }

    fn symbols(&mut self, params: Value) -> anyhow::Result<Value> {
        let p: DocumentSymbolParams = serde_json::from_value(params)?;
        let Some((_, text, Some(fi))) = self.doc(&p.text_document.uri) else {
            return Ok(Value::Null);
        };
        if self.ws.sources[fi].text != text {
            return Ok(Value::Null);
        }
        let s = &self.ws.sources[fi];
        #[allow(deprecated)]
        let symbols: Vec<DocumentSymbol> = s
            .file
            .items
            .iter()
            .map(|item| {
                let span = item.span();
                let (name, detail, kind) = match item {
                    ast::Item::Request(r) => (
                        r.name.clone().unwrap_or_else(|| r.method.clone()),
                        Some(format!("{} {}", r.method, r.target.span.text(&s.text))),
                        SymbolKind::FUNCTION,
                    ),
                    ast::Item::Flow(f) => (f.name.clone(), Some("flow".into()), SymbolKind::EVENT),
                    ast::Item::Shape(d) => {
                        (d.name.clone(), Some("shape".into()), SymbolKind::STRUCT)
                    }
                    ast::Item::Let(l) => (l.name.clone(), Some("let".into()), SymbolKind::VARIABLE),
                };
                let range = Range::new(position(&text, span.start), position(&text, span.end));
                let selection_range = match item {
                    ast::Item::Request(ast::Request {
                        name_span: Some(n), ..
                    }) => Range::new(position(&text, n.start), position(&text, n.end)),
                    _ => Range::new(range.start, range.start),
                };
                DocumentSymbol {
                    name,
                    detail,
                    kind,
                    tags: None,
                    deprecated: None,
                    range,
                    selection_range,
                    children: None,
                }
            })
            .collect();
        Ok(serde_json::to_value(DocumentSymbolResponse::Nested(
            symbols,
        ))?)
    }

    fn formatting(&mut self, params: Value) -> anyhow::Result<Value> {
        let p: DocumentFormattingParams = serde_json::from_value(params)?;
        let Some((_, text, _)) = self.doc(&p.text_document.uri) else {
            return Ok(Value::Null);
        };
        // Файл с ошибкой не форматируется: ошибка уже в диагностике.
        let Ok(pretty) = routy_core::lang::fmt::format(&text) else {
            return Ok(Value::Null);
        };
        Ok(serde_json::to_value(
            minimal_edit(&text, &pretty).into_iter().collect::<Vec<_>>(),
        )?)
    }

    fn code_actions(&mut self, params: Value) -> anyhow::Result<Value> {
        let p: CodeActionParams = serde_json::from_value(params)?;
        let uri = p.text_document.uri;
        let Some((abs, text, fi)) = self.doc(&uri) else {
            return Ok(Value::Null);
        };
        let mut actions: Vec<CodeActionOrCommand> = Vec::new();
        let edit = |edits: Vec<TextEdit>| WorkspaceEdit {
            changes: Some(HashMap::from([(uri.clone(), edits)])),
            ..Default::default()
        };
        let only_fix_all = p.context.only.as_ref().is_some_and(|k| {
            k.iter()
                .all(|k| k.as_str().starts_with(CodeActionKind::SOURCE.as_str()))
        });

        // Расхождения с кодом: правка из `import::fixes` по тексту, который видит редактор.
        if let Some(plan) = &self.go.plan {
            let rel = self.rel(&abs);
            if !only_fix_all {
                for d in &p.context.diagnostics {
                    let Some(id) = d
                        .data
                        .as_ref()
                        .and_then(|v| v.get("id"))
                        .and_then(Value::as_str)
                    else {
                        continue;
                    };
                    let Ok(fixes) = import::fixes(plan, |c| c.id == id) else {
                        continue;
                    };
                    let Some(f) = fixes
                        .into_iter()
                        .find(|f| f.file == rel && f.before == text)
                    else {
                        continue;
                    };
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: format!(
                            "Fix: {}",
                            d.message.split(" (").next().unwrap_or(&d.message)
                        ),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![d.clone()]),
                        edit: Some(edit(minimal_edit(&text, &f.after).into_iter().collect())),
                        is_preferred: Some(true),
                        ..Default::default()
                    }));
                }
            }
            let fixable = plan
                .changes()
                .filter(|c| c.file == rel && c.fix.is_some())
                .count();
            if fixable > 1 || (only_fix_all && fixable > 0) {
                if let Ok(fixes) = import::fixes(plan, |c| c.file == rel) {
                    if let Some(f) = fixes.into_iter().find(|f| f.before == text) {
                        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                            title: format!("Fix all {fixable} differences with the code"),
                            kind: Some(CodeActionKind::SOURCE_FIX_ALL),
                            edit: Some(edit(minimal_edit(&text, &f.after).into_iter().collect())),
                            ..Default::default()
                        }));
                    }
                }
            }
        }

        // «did you mean `X`?» у неизвестного имени вызова.
        if !only_fix_all {
            for d in &p.context.diagnostics {
                let Some(name) = d
                    .message
                    .strip_prefix("unknown request or flow `")
                    .and_then(|m| m.split_once("` (did you mean `"))
                    .and_then(|(_, rest)| rest.strip_suffix("`?)"))
                else {
                    continue;
                };
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: format!("Change to `{name}`"),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![d.clone()]),
                    edit: Some(edit(vec![TextEdit {
                        range: d.range,
                        new_text: name.to_string(),
                    }])),
                    is_preferred: Some(true),
                    ..Default::default()
                }));
            }
        }
        // «Send» запроса под курсором — для редакторов без code lens (Helix).
        let fi = fi.filter(|&fi| self.ws.sources[fi].text == text);
        if let (None, Some(fi)) = (&p.context.only, fi) {
            let line = p.range.start.line as usize + 1;
            let src = &self.ws.sources[fi];
            let item = self.ws.item_at(fi, line).map(|r| self.ws.item(r));
            if let Some(item) = item.filter(|i| {
                let span = i.span();
                (src.line(span.start)..=src.line(span.end)).contains(&line)
            }) {
                let verb = match item {
                    ast::Item::Flow(_) => "Run flow",
                    _ => "Send",
                };
                let title = format!(
                    "▶ {verb} {} · {}",
                    item.name().unwrap_or("request"),
                    self.env
                );
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    command: Some(Command {
                        title: title.clone(),
                        command: if self.client.ui { UI_SEND } else { RUN_COMMAND }.into(),
                        arguments: Some(vec![json!(uri), json!(line)]),
                    }),
                    title,
                    ..Default::default()
                }));
            }
        }
        Ok(serde_json::to_value(actions)?)
    }

    fn code_lens(&mut self, params: Value) -> anyhow::Result<Value> {
        let p: CodeLensParams = serde_json::from_value(params)?;
        let uri = p.text_document.uri;
        let Some((_, text, Some(fi))) = self.doc(&uri) else {
            return Ok(Value::Null);
        };
        if self.ws.sources[fi].text != text {
            return Ok(Value::Null);
        }
        let mut lenses = Vec::new();
        let lens = |line: usize, title: String, command: &str| {
            let at = Position::new(line.saturating_sub(1) as u32, 0);
            CodeLens {
                range: Range::new(at, at),
                command: Some(Command {
                    title,
                    command: command.into(),
                    arguments: Some(vec![json!(uri), json!(line)]),
                }),
                data: None,
            }
        };
        let several_envs = self.project.env_names().len() > 1;
        for (i, (_, _, line, flow)) in ide::runnables(&self.ws, fi).into_iter().enumerate() {
            let verb = if flow { "▶ Run flow" } else { "▶ Send" };
            if self.client.ui {
                lenses.push(lens(line, format!("{verb} · {}", self.env), UI_SEND));
                if several_envs {
                    lenses.push(lens(line, "in…".into(), UI_SEND_IN));
                }
                if !flow {
                    lenses.push(lens(line, "Copy as curl".into(), UI_CURL));
                }
                continue;
            }
            lenses.push(lens(line, format!("{verb} · {}", self.env), RUN_COMMAND));
            if i == 0 && several_envs {
                let mut env = lens(line, format!("env: {}", self.env), ENV_COMMAND);
                if let Some(c) = &mut env.command {
                    c.arguments = None;
                }
                lenses.push(env);
            }
        }
        Ok(serde_json::to_value(lenses)?)
    }

    /// `Ok(None)` — ответ отправит поток запусков.
    fn execute(&mut self, id: RequestId, params: Value) -> anyhow::Result<Option<Value>> {
        let p: ExecuteCommandParams = serde_json::from_value(params)?;
        match p.command.as_str() {
            RUN_COMMAND | CURL_COMMAND => {
                let usage = || format!("{} takes [uri, line, env?]", p.command);
                let uri: Url =
                    serde_json::from_value(p.arguments.first().cloned().with_context(usage)?)?;
                let line = p.arguments.get(1).and_then(Value::as_u64).unwrap_or(1) as usize;
                let env = match p.arguments.get(2).and_then(Value::as_str) {
                    None => self.env.clone(),
                    Some(env) => {
                        let names = self.project.env_names();
                        anyhow::ensure!(
                            names.is_empty() || names.iter().any(|n| n == env),
                            "no environment `{env}` (have: {})",
                            names.join(", ")
                        );
                        env.to_string()
                    }
                };
                let (_, _, fi) = self.doc(&uri).context("no such file")?;
                let fi = fi.context("the file has errors; fix them first")?;
                let item = self
                    .ws
                    .item_at(fi, line)
                    .context("no request or flow in this file")?;
                let name = self.ws.item(item).name().unwrap_or("request").to_string();
                self.jobs
                    .send(Job {
                        id,
                        curl: p.command == CURL_COMMAND,
                        ws: self.ws.clone(),
                        item,
                        name,
                        project: self.project.clone(),
                        env,
                        keyring: self.keyring,
                    })
                    .context("run thread stopped")?;
                Ok(None)
            }
            ENV_COMMAND => {
                if let Some(env) = p.arguments.first().and_then(Value::as_str) {
                    self.set_env(env);
                    return Ok(Some(Value::Null));
                }
                let envs = self.project.env_names();
                if envs.is_empty() {
                    self.message(MessageType::INFO, "routy: no environments in env.toml");
                    return Ok(Some(Value::Null));
                }
                let params = ShowMessageRequestParams {
                    typ: MessageType::INFO,
                    message: format!("routy: environment (now `{}`)", self.env),
                    actions: Some(
                        envs.iter()
                            .map(|e| MessageActionItem {
                                title: e.clone(),
                                properties: HashMap::new(),
                            })
                            .collect(),
                    ),
                };
                let rid = self.request("window/showMessageRequest", params);
                self.pending.insert(rid, Pending::SelectEnv);
                Ok(Some(Value::Null))
            }
            other => anyhow::bail!("unknown command {other}"),
        }
    }
}

impl Server {
    /// `routy/state`: окружения, запросы и сценарии проекта, переменные текущего окружения
    /// (секреты замаскированы).
    fn state(&mut self) -> anyhow::Result<Value> {
        let mut items = Vec::new();
        for (fi, src) in self.ws.sources.iter().enumerate() {
            let Ok(uri) = Url::from_file_path(&src.full) else {
                continue;
            };
            for (r, name, line, flow) in ide::runnables(&self.ws, fi) {
                let (method, target) = match self.ws.item(r) {
                    ast::Item::Request(req) => {
                        (req.method.as_str(), req.target.span.text(&src.text))
                    }
                    _ => ("", ""),
                };
                items.push(json!({
                    "uri": uri,
                    "path": slash(&src.path),
                    "name": name,
                    "flow": flow,
                    "method": method,
                    "target": target,
                    "line": line,
                }));
            }
        }
        let vars: Vec<Value> = self
            .vars()
            .into_iter()
            .map(|v| {
                let value = match (&v.value, v.secret) {
                    (Some(val), true) => Some(mask(val)),
                    (value, _) => value.clone(),
                };
                json!({ "name": v.name, "value": value, "source": v.source, "secret": v.secret })
            })
            .collect();
        Ok(json!({
            "root": self.project.root,
            "env": self.env,
            "envs": self.project.env_names(),
            "items": items,
            "vars": vars,
        }))
    }
}

fn next_id(ids: &AtomicU64) -> RequestId {
    RequestId::from(format!("routy-{}", ids.fetch_add(1, Ordering::Relaxed)))
}

fn default_source_dir(root: &Path) -> PathBuf {
    match root.file_name() {
        Some(n) if n == "api" => root.parent().unwrap_or(root).to_path_buf(),
        _ => root.to_path_buf(),
    }
}

fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn code_of(kind: &import::ChangeKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.get("kind").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default()
}

fn completion_kind(k: ide::Kind) -> CompletionItemKind {
    use ide::Kind;
    match k {
        Kind::Keyword => CompletionItemKind::KEYWORD,
        Kind::Field => CompletionItemKind::FIELD,
        Kind::Method => CompletionItemKind::METHOD,
        Kind::Property => CompletionItemKind::PROPERTY,
        Kind::Function => CompletionItemKind::FUNCTION,
        Kind::Request => CompletionItemKind::FUNCTION,
        Kind::Flow => CompletionItemKind::EVENT,
        Kind::Variable => CompletionItemKind::VARIABLE,
        Kind::Param => CompletionItemKind::PROPERTY,
        Kind::Shape => CompletionItemKind::STRUCT,
        Kind::Type => CompletionItemKind::TYPE_PARAMETER,
        Kind::Constant => CompletionItemKind::CONSTANT,
        Kind::Value => CompletionItemKind::VALUE,
    }
}

/// Порядок в списке: то, что пишут чаще, — выше.
fn rank(k: ide::Kind) -> u8 {
    use ide::Kind;
    match k {
        Kind::Param | Kind::Field | Kind::Value | Kind::Property | Kind::Method => 0,
        Kind::Variable | Kind::Shape => 1,
        Kind::Request | Kind::Flow => 2,
        Kind::Function | Kind::Type => 3,
        Kind::Keyword | Kind::Constant => 4,
    }
}

/// Сниппет без мест для курсора: `Login(email: $1)` → `Login(email: )`.
fn plain(snippet: &str) -> String {
    let mut out = String::new();
    let mut chars = snippet.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'{') {
            chars.next();
            while chars.peek().is_some_and(char::is_ascii_digit) {
                chars.next();
            }
            if chars.peek() == Some(&':') {
                chars.next();
            }
            for c in chars.by_ref() {
                if c == '}' {
                    break;
                }
                out.push(c);
            }
        } else {
            while chars.peek().is_some_and(char::is_ascii_digit) {
                chars.next();
            }
        }
    }
    out
}

// ---------- позиции: байты ↔ строки и UTF-16 ----------

fn position(text: &str, offset: usize) -> Position {
    let offset = floor_char(text, offset.min(text.len()));
    let before = &text[..offset];
    let line = before.matches('\n').count();
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let col: usize = text[line_start..offset].chars().map(char::len_utf16).sum();
    Position::new(line as u32, col as u32)
}

fn floor_char(text: &str, mut i: usize) -> usize {
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn offset_of(text: &str, pos: Position) -> usize {
    let mut start = 0;
    for _ in 0..pos.line {
        match text[start..].find('\n') {
            Some(i) => start += i + 1,
            None => return text.len(),
        }
    }
    let line_end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    let mut units = 0;
    for (i, c) in text[start..line_end].char_indices() {
        if units >= pos.character as usize {
            return start + i;
        }
        units += c.len_utf16();
    }
    line_end
}

/// Байтовое смещение для строки и столбца в символах (с 1), как в `routy check`.
fn line_col_offset(text: &str, line: usize, col: usize) -> usize {
    let mut start = 0;
    for _ in 1..line {
        match text[start..].find('\n') {
            Some(i) => start += i + 1,
            None => return text.len(),
        }
    }
    text[start..]
        .char_indices()
        .nth(col.saturating_sub(1))
        .map_or(text.len(), |(i, _)| start + i)
}

/// Слово (имя, `users.Create`, `{id}`) с началом в `offset` — или один символ.
fn word_range(text: &str, offset: usize) -> Range {
    let rest = &text[offset.min(text.len())..];
    let len = rest
        .char_indices()
        .find(|&(_, c)| !(c.is_alphanumeric() || c == '_' || c == '.' || c == '-'))
        .map_or(rest.len(), |(i, _)| i);
    let len = if len == 0 {
        rest.chars()
            .next()
            .filter(|c| *c != '\n')
            .map_or(0, char::len_utf8)
    } else {
        len
    };
    Range::new(position(text, offset), position(text, offset + len))
}

/// Одна правка от `before` к `after`: только изменившаяся середина.
fn minimal_edit(before: &str, after: &str) -> Option<TextEdit> {
    if before == after {
        return None;
    }
    let mut start = before
        .bytes()
        .zip(after.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !before.is_char_boundary(start) || !after.is_char_boundary(start) {
        start -= 1;
    }
    let max = before.len().min(after.len()) - start;
    let mut suffix = before
        .bytes()
        .rev()
        .zip(after.bytes().rev())
        .take(max)
        .take_while(|(a, b)| a == b)
        .count();
    while !before.is_char_boundary(before.len() - suffix)
        || !after.is_char_boundary(after.len() - suffix)
    {
        suffix -= 1;
    }
    Some(TextEdit {
        range: Range::new(
            position(before, start),
            position(before, before.len() - suffix),
        ),
        new_text: after[start..after.len() - suffix].to_string(),
    })
}

// ---------- запуск запросов ----------

struct Job {
    id: RequestId,
    /// `routy.curl`: только подготовить запрос и вернуть команду curl.
    curl: bool,
    ws: Workspace,
    item: ItemRef,
    name: String,
    project: Project,
    env: String,
    keyring: bool,
}

/// `Runner` и `exec::Run` одного окружения проекта.
type Sessions = HashMap<(PathBuf, String), (Runner, Run)>;

/// Поток запусков: свой `Runner` и `exec::Run` на окружение (кеш вызовов и cookies живут,
/// пока жив сервер — как в приложении), запросы по одному. `quiet` — клиент с `routyUi`
/// показывает итог сам: без файла ответа и сообщений.
fn spawn_worker(
    out: Sender<Message>,
    done: Sender<Internal>,
    waiting: Waiting,
    ids: Arc<AtomicU64>,
    show_document: bool,
    quiet: bool,
) -> Sender<Job> {
    let (tx, rx) = crossbeam_channel::unbounded::<Job>();
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        let mut sessions = Sessions::new();
        let show = |typ, message: String| {
            if !quiet {
                let _ = out.send(
                    Notification::new(
                        "window/showMessage".into(),
                        ShowMessageParams { typ, message },
                    )
                    .into(),
                );
            }
        };
        for job in rx {
            let result = if job.curl {
                curl_job(&rt, &mut sessions, &job, &out, &waiting, &ids).map(Value::String)
            } else {
                run_job(&rt, &mut sessions, &job, &out, &waiting, &ids).map(|outcome| {
                    let passed = outcome.passed();
                    let typ = if passed {
                        MessageType::INFO
                    } else {
                        MessageType::ERROR
                    };
                    show(typ, summary(&job.name, &job.env, &outcome));
                    let file = if quiet {
                        None
                    } else {
                        write_report(&job.name, &job.env, &outcome).ok()
                    };
                    if let (true, Some(uri)) = (
                        show_document,
                        file.as_deref().and_then(|f| Url::from_file_path(f).ok()),
                    ) {
                        let params = ShowDocumentParams {
                            uri,
                            external: Some(false),
                            take_focus: Some(false),
                            selection: None,
                        };
                        let _ = out.send(
                            Request::new(next_id(&ids), "window/showDocument".into(), params)
                                .into(),
                        );
                    }
                    let mut result = json!({
                        "name": job.name,
                        "env": job.env,
                        "passed": passed,
                        "file": file,
                        "outcome": outcome,
                    });
                    // Тело не UTF-8 (картинка, архив): в `body` оно испорчено, расширению — байты.
                    if let (true, Outcome::Request(o)) = (quiet, &outcome) {
                        if std::str::from_utf8(&o.response.raw).is_err() {
                            use base64::Engine;
                            result["raw"] = json!(
                                base64::engine::general_purpose::STANDARD.encode(&o.response.raw)
                            );
                        }
                    }
                    result
                })
            };
            let response = match result {
                Ok(v) => Response::new_ok(job.id, v),
                Err(e) => {
                    show(MessageType::ERROR, format!("✗ {}: {e:#}", job.name));
                    Response::new_err(
                        job.id,
                        lsp_server::ErrorCode::RequestFailed as i32,
                        format!("{e:#}"),
                    )
                }
            };
            let _ = out.send(response.into());
            if !job.curl {
                let _ = done.send(Internal::RunDone);
            }
        }
    });
    tx
}

/// Сессия окружения задания с текстами из задания и вопросом для `confirm: true`.
fn session<'s>(
    sessions: &'s mut Sessions,
    job: &Job,
    out: &Sender<Message>,
    waiting: &Waiting,
    ids: &Arc<AtomicU64>,
) -> anyhow::Result<&'s mut (Runner, Run)> {
    let s = match sessions.entry((job.project.root.clone(), job.env.clone())) {
        std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
        std::collections::hash_map::Entry::Vacant(e) => {
            let opts = Options {
                use_keyring: job.keyring,
                ..Options::default()
            };
            let runner = Runner::new(job.project.clone(), Some(&job.env), opts)?;
            let run = Run::new(job.ws.clone(), Duration::from_secs(30))?;
            e.insert((runner, run))
        }
    };
    s.0.load_saved()?;
    s.1.set_workspace(job.ws.clone());
    let (out, waiting, ids, env_name) =
        (out.clone(), waiting.clone(), ids.clone(), job.env.clone());
    s.1.confirm = Some(Box::new(move |name, req| {
        confirm(
            &out,
            &waiting,
            &ids,
            &format!("Send {name} ({} {}) in `{env_name}`?", req.method, req.url),
        )
    }));
    Ok(s)
}

fn run_job(
    rt: &tokio::runtime::Runtime,
    sessions: &mut Sessions,
    job: &Job,
    out: &Sender<Message>,
    waiting: &Waiting,
    ids: &Arc<AtomicU64>,
) -> anyhow::Result<Outcome> {
    let (runner, run) = session(sessions, job, out, waiting, ids)?;
    let outcome = rt.block_on(run.run_item(runner, job.item));
    run.confirm = None;
    runner.persist_saved()?;
    Ok(outcome?)
}

fn curl_job(
    rt: &tokio::runtime::Runtime,
    sessions: &mut Sessions,
    job: &Job,
    out: &Sender<Message>,
    waiting: &Waiting,
    ids: &Arc<AtomicU64>,
) -> anyhow::Result<String> {
    let (runner, run) = session(sessions, job, out, waiting, ids)?;
    let resolved = rt.block_on(run.resolve_item(runner, job.item));
    run.confirm = None;
    runner.persist_saved()?;
    let (req, body) = resolved?;
    Ok(routy_core::curl::command(&req, body.as_deref()))
}

/// `confirm: true`: спросить клиента и ждать ответа (главный цикл передаст его сюда).
fn confirm(out: &Sender<Message>, waiting: &Waiting, ids: &AtomicU64, message: &str) -> bool {
    let id = next_id(ids);
    let (tx, rx) = crossbeam_channel::bounded(1);
    waiting.lock().unwrap().insert(id.clone(), tx);
    let params = ShowMessageRequestParams {
        typ: MessageType::WARNING,
        message: message.to_string(),
        actions: Some(
            ["Send", "Cancel"]
                .iter()
                .map(|t| MessageActionItem {
                    title: t.to_string(),
                    properties: HashMap::new(),
                })
                .collect(),
        ),
    };
    if out
        .send(Request::new(id.clone(), "window/showMessageRequest".into(), params).into())
        .is_err()
    {
        return false;
    }
    let answer = rx.recv_timeout(Duration::from_secs(600));
    waiting.lock().unwrap().remove(&id);
    answer
        .ok()
        .and_then(|r| r.result)
        .and_then(|v| serde_json::from_value::<Option<MessageActionItem>>(v).ok())
        .flatten()
        .is_some_and(|a| a.title == "Send")
}

fn summary(name: &str, env: &str, o: &Outcome) -> String {
    let mark = if o.passed() { "✓" } else { "✗" };
    match o {
        Outcome::Request(r) => {
            let failed = r.asserts.iter().filter(|a| !a.passed).count();
            let mut s = format!(
                "{mark} {name} · {env}: {} {} · {}ms",
                r.response.status, r.response.status_text, r.response.duration_ms
            );
            if failed > 0 {
                s.push_str(&format!(" · {failed} checks failed"));
            }
            s
        }
        Outcome::Flow(f) => {
            let mut s = format!("{mark} flow {name} · {env}: {} requests", f.calls.len());
            if let Some(e) = &f.error {
                s.push_str(&format!(" · {e}"));
            }
            s
        }
    }
}

/// Ответ в файл `<cache>/routy/responses/<Name>.http`: итог и проверки — комментариями `#`,
/// дальше ответ как HTTP-сообщение (заголовки, тело; JSON — с отступами).
fn write_report(name: &str, env: &str, o: &Outcome) -> std::io::Result<PathBuf> {
    let dir = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("routy")
        .join("responses");
    std::fs::create_dir_all(&dir)?;
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = dir.join(format!("{safe}.http"));
    std::fs::write(&path, report(name, env, o))?;
    Ok(path)
}

fn report(name: &str, env: &str, o: &Outcome) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let mark = |ok: bool| if ok { "✓" } else { "✗" };
    let checks = |s: &mut String, list: &[routy_core::expr::AssertOutcome]| {
        for a in list {
            let _ = match (&a.detail, a.passed) {
                (_, true) => writeln!(s, "# ✓ {}", a.source),
                (Some(d), false) => writeln!(s, "# ✗ {} — {d}", a.source),
                (None, false) => writeln!(
                    s,
                    "# ✗ {} (actual: {})",
                    a.source,
                    a.actual
                        .as_ref()
                        .map_or("<missing>".into(), |v| v.to_string())
                ),
            };
        }
    };
    let calls = |s: &mut String, list: &[routy_core::lang::exec::CallTrace]| {
        for c in list {
            let info = match (c.cached, c.status, c.duration_ms) {
                (true, _, _) => "cached".to_string(),
                (_, Some(st), Some(ms)) => format!("{st} {ms}ms"),
                _ => String::new(),
            };
            let _ = writeln!(s, "# {}↳ {} {info}", "  ".repeat(c.depth), c.name);
        }
    };
    match o {
        Outcome::Request(r) => {
            let _ = writeln!(s, "# {} {name} · {env}", mark(o.passed()));
            let _ = writeln!(s, "# {} {}", r.request.method, r.request.url);
            calls(&mut s, &r.calls);
            checks(&mut s, &r.asserts);
            for miss in &r.save_misses {
                let _ = writeln!(s, "# ✗ save {miss} (no value in response)");
            }
            for k in r.saved.keys() {
                let _ = writeln!(s, "# → saved {k}");
            }
            let resp = &r.response;
            let _ = writeln!(
                s,
                "# {}ms, {} bytes\n\nHTTP/1.1 {} {}",
                resp.duration_ms, resp.size, resp.status, resp.status_text
            );
            for h in &resp.headers {
                let _ = writeln!(s, "{}: {}", h.name, h.value);
            }
            s.push('\n');
            match &resp.json {
                Some(v) => s.push_str(&serde_json::to_string_pretty(v).unwrap_or_default()),
                None => s.push_str(&resp.body),
            }
            s.push('\n');
        }
        Outcome::Flow(f) => {
            let _ = writeln!(s, "# {} flow {name} · {env}", mark(o.passed()));
            calls(&mut s, &f.calls);
            checks(&mut s, &f.checks);
            if let Some(e) = &f.error {
                let _ = writeln!(s, "# ✗ {e}");
            }
            for k in f.saved.keys() {
                let _ = writeln!(s, "# → saved {k}");
            }
        }
    }
    s
}

#[cfg(test)]
mod tests;
