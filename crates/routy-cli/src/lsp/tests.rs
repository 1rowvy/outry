//! `routy lsp` целиком: клиент в памяти, временный проект с Go-кодом и локальный HTTP-сервер.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use serde_json::{Value, json};

use super::*;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("routy-lsp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn write(&self, rel: &str, content: &str) -> PathBuf {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
        if let Some(state) = routy_core::state::state_path(&self.0.join("api")) {
            let _ = std::fs::remove_file(state);
        }
    }
}

/// Отвечает `201` с JSON: метод и путь.
fn echo_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut parts = line.split_whitespace();
            let (method, path) = (
                parts.next().unwrap().to_string(),
                parts.next().unwrap().to_string(),
            );
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
                let (k, v) = h.split_once(':').unwrap();
                if k.eq_ignore_ascii_case("content-length") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let resp = json!({ "method": method, "path": path, "id": 7 }).to_string();
            write!(
                stream,
                "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp}",
                resp.len()
            )
            .unwrap();
        }
    });
    format!("http://{addr}")
}

struct Client {
    conn: Connection,
    server: Option<std::thread::JoinHandle<anyhow::Result<()>>>,
    next: i32,
    /// Последняя диагностика по каждому файлу.
    diagnostics: HashMap<Url, Vec<Diagnostic>>,
    /// Запросы сервера к клиенту, которые уже пришли.
    server_requests: Vec<Request>,
    messages: Vec<String>,
    /// Остальные уведомления сервера: метод и параметры.
    notes: Vec<(String, Value)>,
    /// Что ответить на `window/showMessageRequest`.
    answer: Option<&'static str>,
}

impl Client {
    fn start(root: &Path) -> Client {
        Client::start_with(root, json!({}))
    }

    /// `experimental` — возможности клиента вроде `routyUi`.
    fn start_with(root: &Path, experimental: Value) -> Client {
        let (server, conn) = Connection::memory();
        let handle = std::thread::spawn(move || {
            serve(
                server,
                Opts {
                    env: None,
                    no_keyring: true,
                },
            )
        });
        let mut c = Client {
            conn,
            server: Some(handle),
            next: 0,
            diagnostics: HashMap::new(),
            server_requests: Vec::new(),
            messages: Vec::new(),
            notes: Vec::new(),
            answer: None,
        };
        let init = json!({
            "processId": null,
            "rootUri": Url::from_file_path(root).unwrap(),
            "capabilities": {
                "window": { "showDocument": { "support": true } },
                "textDocument": { "completion": { "completionItem": { "snippetSupport": true } } },
                "experimental": experimental
            },
            "initializationOptions": { "keyring": false }
        });
        c.request("initialize", init);
        c.notify("initialized", json!({}));
        c
    }

    fn notify(&self, method: &str, params: Value) {
        self.conn
            .sender
            .send(Notification::new(method.into(), params).into())
            .unwrap();
    }

    fn recv(&mut self, deadline: Instant) -> Option<Message> {
        let left = deadline.checked_duration_since(Instant::now())?;
        let msg = self.conn.receiver.recv_timeout(left).ok()?;
        match &msg {
            Message::Notification(n) if n.method == "textDocument/publishDiagnostics" => {
                let p: PublishDiagnosticsParams = serde_json::from_value(n.params.clone()).unwrap();
                self.diagnostics.insert(p.uri, p.diagnostics);
            }
            Message::Notification(n) if n.method == "window/showMessage" => {
                let p: ShowMessageParams = serde_json::from_value(n.params.clone()).unwrap();
                self.messages.push(p.message);
            }
            Message::Notification(n) => self.notes.push((n.method.clone(), n.params.clone())),
            Message::Request(r) => {
                let result = match (r.method.as_str(), self.answer) {
                    ("window/showMessageRequest", Some(a)) => json!({ "title": a }),
                    _ => Value::Null,
                };
                self.conn
                    .sender
                    .send(Response::new_ok(r.id.clone(), result).into())
                    .unwrap();
                self.server_requests.push(r.clone());
            }
            _ => {}
        }
        Some(msg)
    }

    fn request_result(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next += 1;
        let id = RequestId::from(self.next);
        self.conn
            .sender
            .send(Request::new(id.clone(), method.into(), params).into())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match self.recv(deadline) {
                Some(Message::Response(r)) if r.id == id => {
                    return match r.error {
                        Some(e) => Err(e.message),
                        None => Ok(r.result.unwrap_or(Value::Null)),
                    };
                }
                Some(_) => {}
                None => panic!("no response to {method}"),
            }
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.request_result(method, params)
            .unwrap_or_else(|e| panic!("{method}: {e}"))
    }

    /// Ждёт, пока диагностика файла не станет такой, как нужно.
    fn wait_diagnostics(
        &mut self,
        uri: &Url,
        ok: impl Fn(&[Diagnostic]) -> bool,
    ) -> Vec<Diagnostic> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(d) = self.diagnostics.get(uri) {
                if ok(d) {
                    return d.clone();
                }
            }
            if self.recv(deadline).is_none() {
                panic!("diagnostics for {uri}: {:?}", self.diagnostics.get(uri));
            }
        }
    }

    fn open(&self, path: &Path, text: &str) -> Url {
        let uri = Url::from_file_path(path).unwrap();
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri, "languageId": "routy", "version": 1, "text": text } }),
        );
        uri
    }

    fn change(&self, uri: &Url, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": text }] }),
        );
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.request_result("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        if let Some(h) = self.server.take() {
            h.join().unwrap().unwrap();
        }
    }
}

/// Позиция `needle` в тексте (+`shift` символов) для запросов клиента.
fn at(text: &str, needle: &str, shift: usize) -> Value {
    let p = position(
        text,
        text.find(needle).unwrap_or_else(|| panic!("{needle}")) + shift,
    );
    json!({ "line": p.line, "character": p.character })
}

fn apply(text: &str, edits: &[TextEdit]) -> String {
    let mut edits = edits.to_vec();
    edits.sort_by_key(|e| std::cmp::Reverse((e.range.start.line, e.range.start.character)));
    let mut out = text.to_string();
    for e in edits {
        let (a, b) = (offset_of(&out, e.range.start), offset_of(&out, e.range.end));
        out.replace_range(a..b, &e.new_text);
    }
    out
}

const LOGIN: &str = "// Login\nPOST /login {\n  params {\n    email: \"a@b.c\"\n  }\n  body { email }\n  save token = body.id\n}\n";

#[test]
fn language_features() {
    let dir = TempDir::new("features");
    let base = echo_server();
    dir.write(
        "api/env.toml",
        &format!("default = \"dev\"\n\n[env.dev]\nbase = \"{base}\"\n\n[env.prod]\nbase = \"https://example.com\"\n"),
    );
    dir.write("api/auth/login.routy", LOGIN);
    let orders = dir.write("api/orders.routy", "");
    let mut c = Client::start(&dir.0);

    // Ошибки check, предупреждения check --env, «did you mean».
    let text = "// Create order\nPOST /orders {\n  headers { A: Logn().body.id }\n  body { x: missing }\n}\n";
    let uri = c.open(&orders, text);
    let diags = c.wait_diagnostics(&uri, |d| d.len() == 2);
    let unknown = diags.iter().find(|d| d.message.contains("Logn")).unwrap();
    assert_eq!(unknown.severity, Some(DiagnosticSeverity::ERROR));
    assert_eq!(unknown.range.start, Position::new(2, 15));
    assert_eq!(unknown.range.end, Position::new(2, 19));
    let missing = diags
        .iter()
        .find(|d| d.message.contains("missing"))
        .unwrap();
    assert_eq!(missing.severity, Some(DiagnosticSeverity::WARNING));
    let actions = c.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": unknown.range, "context": { "diagnostics": [unknown] } }),
    );
    let fixed = serde_json::from_value::<WorkspaceEdit>(actions[0]["edit"].clone()).unwrap();
    let edits = &fixed.changes.unwrap()[&uri];
    assert!(apply(text, edits).contains("A: Login().body.id"));

    // Исправили — ошибок нет.
    let text = "// Create order\nPOST /orders {\n  headers { A: Login().body.id }\n  body { x: token }\n}\n";
    c.change(&uri, text);
    c.wait_diagnostics(&uri, |d| d.is_empty());

    // Автодополнение: вызов со сниппетом, переменные окружения.
    let items = c.request(
        "textDocument/completion",
        json!({ "textDocument": { "uri": uri }, "position": at(text, "Login()", 3) }),
    );
    let login = items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["label"] == "Login")
        .unwrap();
    assert_eq!(login["textEdit"]["newText"], "Login()");
    assert_eq!(login["insertTextFormat"], 2);
    assert!(
        items
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["label"] == "base")
    );

    // Hover: вызов и переменная.
    let h = c.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri }, "position": at(text, "Login()", 1) }),
    );
    let md = h["contents"]["value"].as_str().unwrap();
    assert!(
        md.contains("POST /login") && md.contains("email = \"a@b.c\""),
        "{md}"
    );

    // Определение: вызов → файл запроса, сохранённое значение → его `save`.
    let d = c.request(
        "textDocument/definition",
        json!({ "textDocument": { "uri": uri }, "position": at(text, "Login()", 1) }),
    );
    assert!(
        d[0]["uri"]
            .as_str()
            .unwrap()
            .ends_with("api/auth/login.routy")
    );
    let d = c.request(
        "textDocument/definition",
        json!({ "textDocument": { "uri": uri }, "position": at(text, "token", 1) }),
    );
    assert!(d[0]["uri"].as_str().unwrap().ends_with("login.routy"));
    assert_eq!(d[0]["range"]["start"]["line"], 6);

    // Outline и code lens.
    let symbols = c.request(
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": uri } }),
    );
    assert_eq!(symbols[0]["name"], "CreateOrder");
    let lenses = c.request(
        "textDocument/codeLens",
        json!({ "textDocument": { "uri": uri } }),
    );
    let titles: Vec<&str> = lenses
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["command"]["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["▶ Send · dev", "env: dev"]);

    // «Send» есть и среди действий — для редакторов без code lens.
    let actions = c.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": { "start": { "line": 2, "character": 0 }, "end": { "line": 2, "character": 0 } }, "context": { "diagnostics": [] } }),
    );
    assert_eq!(actions[0]["title"], "▶ Send CreateOrder · dev");
    assert_eq!(actions[0]["command"]["arguments"][1], 3);

    // «Send»: Login вызывается, ответ — в файле, который открывает showDocument.
    let r = c.request("workspace/executeCommand", lenses[0]["command"].clone());
    assert_eq!(r["passed"], true, "{r}");
    assert_eq!(r["outcome"]["response"]["status"], 201);
    assert_eq!(r["outcome"]["calls"][0]["name"], "Login");
    let report = std::fs::read_to_string(r["file"].as_str().unwrap()).unwrap();
    assert!(
        report.contains("HTTP/1.1 201 Created") && report.contains("\"path\": \"/orders\""),
        "{report}"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !c
        .server_requests
        .iter()
        .any(|r| r.method == "window/showDocument")
    {
        assert!(c.recv(deadline).is_some(), "no showDocument");
    }
    assert!(
        c.messages
            .iter()
            .any(|m| m.starts_with("✓ CreateOrder · dev: 201 Created")),
        "{:?}",
        c.messages
    );

    // `confirm: true` спрашивает клиента; «Cancel» — не отправлять.
    let text = "// Danger\nDELETE /all {\n  confirm: true\n}\n";
    c.change(&uri, text);
    c.answer = Some("Cancel");
    let err = c
        .request_result(
            "workspace/executeCommand",
            json!({ "command": RUN_COMMAND, "arguments": [uri, 2] }),
        )
        .unwrap_err();
    assert!(err.contains("not confirmed"), "{err}");
    assert!(
        c.server_requests
            .iter()
            .any(|r| r.method == "window/showMessageRequest")
    );

    // Окружение: prod — другое значение `base`.
    c.request(
        "workspace/executeCommand",
        json!({ "command": ENV_COMMAND, "arguments": ["prod"] }),
    );
    let text = "GET /x {\n  query { b: base }\n}\n";
    c.change(&uri, text);
    let h = c.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri }, "position": at(text, "base", 1) }),
    );
    assert!(
        h["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("https://example.com")
    );
    let d = c.request(
        "textDocument/definition",
        json!({ "textDocument": { "uri": uri }, "position": at(text, "base", 1) }),
    );
    assert!(d[0]["uri"].as_str().unwrap().ends_with("api/env.toml"));
    assert_eq!(d[0]["range"]["start"]["line"], 6);

    // Форматирование.
    let text = "GET   /x {\nexpect {status==200}\n}\n";
    c.change(&uri, text);
    let edits: Vec<TextEdit> = serde_json::from_value(c.request(
        "textDocument/formatting",
        json!({ "textDocument": { "uri": uri }, "options": { "tabSize": 2, "insertSpaces": true } }),
    ))
    .unwrap();
    assert_eq!(
        apply(text, &edits),
        "GET /x {\n  expect { status == 200 }\n}\n"
    );
}

#[test]
fn differences_with_go_code() {
    let dir = TempDir::new("go");
    dir.write(
        "main.go",
        r#"package main

import (
	"net/http"

	"github.com/go-chi/chi/v5"
)

func main() {
	r := chi.NewRouter()
	r.Get("/v2/users/{id}", getUser)
}

func getUser(w http.ResponseWriter, r *http.Request) {}
"#,
    );
    dir.write("api/env.toml", "[env.dev]\nbase = \"http://x\"\n");
    let text = "// Get user\nGET /users/{id} {\n  handler: getUser\n}\n";
    let users = dir.write("api/users.routy", text);
    let mut c = Client::start(&dir.0);
    let uri = c.open(&users, text);
    let diags = c.wait_diagnostics(&uri, |d| {
        d.iter().any(|d| d.message.contains("path changed"))
    });
    let d = diags
        .iter()
        .find(|d| d.message.contains("path changed"))
        .unwrap();
    assert!(d.message.ends_with("(main.go:11)"), "{}", d.message);
    let go = &d.related_information.as_ref().unwrap()[0].location;
    assert!(go.uri.path().ends_with("/main.go"));
    assert_eq!(go.range.start.line, 10);

    let actions = c.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": d.range, "context": { "diagnostics": [d] } }),
    );
    assert_eq!(actions[0]["kind"], "quickfix");
    let edit: WorkspaceEdit = serde_json::from_value(actions[0]["edit"].clone()).unwrap();
    let fixed = apply(text, &edit.changes.unwrap()[&uri]);
    assert!(fixed.contains("GET /v2/users/{id} {"), "{fixed}");

    // Правка в редакторе (без сохранения) — сравнение по ней.
    c.change(&uri, &fixed);
    c.wait_diagnostics(&uri, |d| {
        !d.iter().any(|d| d.message.contains("path changed"))
    });

    // `handler:` → роут в Go.
    let def = c.request(
        "textDocument/definition",
        json!({ "textDocument": { "uri": uri }, "position": at(&fixed, "getUser", 2) }),
    );
    assert!(def[0]["uri"].as_str().unwrap().ends_with("/main.go"));
    assert_eq!(def[0]["range"]["start"]["line"], 10);
}

#[test]
fn positions_are_utf16() {
    let text = "a\nпривет 😀 x\n";
    let p = position(text, text.find('x').unwrap());
    assert_eq!((p.line, p.character), (1, 10));
    assert_eq!(offset_of(text, p), text.find('x').unwrap());
    assert_eq!(line_col_offset(text, 2, 10), text.find('x').unwrap());
    assert_eq!(
        plain("Login(email: $1, b: ${2:x})$0"),
        "Login(email: , b: x)"
    );
    let e = minimal_edit("abc привет def", "abc пока def").unwrap();
    assert_eq!(apply("abc привет def", &[e]), "abc пока def");
}

#[test]
fn folder_without_env_toml() {
    let dir = TempDir::new("plain");
    dir.write("reqs/login.routy", LOGIN);
    let text = "// Me\nGET https://example.com/me {\n  headers { A: Login().body.id }\n}\n";
    let me = dir.write("reqs/me.routy", text);
    let mut c = Client::start(&dir.0);
    let uri = c.open(&me, text);
    // Проект найден от файла: соседний Login виден, ошибок нет.
    c.wait_diagnostics(&uri, |d| d.is_empty());
    c.change(&uri, &text.replace("Login()", "Nope()"));
    let d = c.wait_diagnostics(&uri, |d| !d.is_empty());
    assert!(
        d[0].message.contains("unknown request or flow `Nope`"),
        "{d:?}"
    );
}

/// Расширение VS Code (`routyUi`): свои команды в code lens, итог без файла и сообщений,
/// `routy/state`, `routy/didChange`, curl и запуск в другом окружении.
#[test]
fn ui_client() {
    let dir = TempDir::new("ui");
    let base = echo_server();
    dir.write(
        "api/env.toml",
        &format!("default = \"dev\"\nsecrets = [\"key\"]\n\n[env.dev]\nbase = \"{base}\"\nkey = \"0123456789abcdef\"\n\n[env.staging]\nbase = \"{base}\"\n"),
    );
    dir.write("api/auth/login.routy", LOGIN);
    let orders = dir.write("api/orders.routy", "");
    let mut c = Client::start_with(&dir.0, json!({ "routyUi": true }));

    let text = "// Create order\nPOST /orders {\n  headers { T: Login().body.id }\n  body { x: 1 }\n}\n\nflow Checkout {\n  CreateOrder()\n}\n";
    let uri = c.open(&orders, text);
    c.wait_diagnostics(&uri, |d| d.is_empty());
    let lenses = c.request(
        "textDocument/codeLens",
        json!({ "textDocument": { "uri": uri } }),
    );
    let lenses: Vec<(&str, &str)> = lenses
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["command"]["title"].as_str().unwrap(),
                l["command"]["command"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        lenses,
        [
            ("▶ Send · dev", UI_SEND),
            ("in…", UI_SEND_IN),
            ("Copy as curl", UI_CURL),
            ("▶ Run flow · dev", UI_SEND),
            ("in…", UI_SEND_IN),
        ]
    );

    let state = c.request("routy/state", Value::Null);
    assert_eq!(state["env"], "dev");
    assert_eq!(state["envs"], json!(["dev", "staging"]));
    let items = state["items"].as_array().unwrap();
    let create = items.iter().find(|i| i["name"] == "CreateOrder").unwrap();
    assert_eq!(create["method"], "POST");
    assert_eq!(create["target"], "/orders");
    assert_eq!(create["line"], 2);
    assert_eq!(create["path"], "orders.routy");
    assert!(
        items
            .iter()
            .any(|i| i["name"] == "Checkout" && i["flow"] == true)
    );
    assert!(items.iter().any(|i| i["name"] == "Login"));
    let vars = state["vars"].as_array().unwrap();
    let key = vars.iter().find(|v| v["name"] == "key").unwrap();
    assert_eq!(key["secret"], true);
    assert_eq!(key["value"], "012…(16 chars)");

    // curl: Login() уходит, сам запрос — нет.
    let curl = c.request(
        "workspace/executeCommand",
        json!({ "command": CURL_COMMAND, "arguments": [uri, 2] }),
    );
    let curl = curl.as_str().unwrap();
    assert!(
        curl.starts_with(&format!("curl -X POST {base}/orders \\\n")),
        "{curl}"
    );
    assert!(
        curl.contains("-H 'T: 7'") && curl.contains("--data-raw '{\"x\":1}'"),
        "{curl}"
    );

    // Запуск в другом окружении: итог только в ответе.
    let r = c.request(
        "workspace/executeCommand",
        json!({ "command": RUN_COMMAND, "arguments": [uri, 2, "staging"] }),
    );
    assert_eq!(r["env"], "staging");
    assert_eq!(r["passed"], true, "{r}");
    assert_eq!(r["file"], Value::Null);
    let err = c
        .request_result(
            "workspace/executeCommand",
            json!({ "command": RUN_COMMAND, "arguments": [uri, 2, "nope"] }),
        )
        .unwrap_err();
    assert!(err.contains("no environment `nope`"), "{err}");

    c.request(
        "workspace/executeCommand",
        json!({ "command": ENV_COMMAND, "arguments": ["staging"] }),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !c
        .notes
        .iter()
        .any(|(m, p)| m == "routy/didChange" && p["env"] == "staging")
    {
        assert!(c.recv(deadline).is_some(), "no routy/didChange");
    }
    assert!(c.messages.is_empty(), "{:?}", c.messages);
    assert!(
        !c.server_requests
            .iter()
            .any(|r| r.method == "window/showDocument")
    );
}

/// `routy import go` в папке, где сервер уже запущен: проект находится по новому `env.toml`,
/// без открытых файлов.
#[test]
fn project_created_after_start() {
    let dir = TempDir::new("late");
    std::fs::create_dir(dir.0.join(".git")).unwrap();
    let mut c = Client::start_with(&dir.0, json!({ "routyUi": true }));
    assert_eq!(c.request("routy/state", Value::Null)["items"], json!([]));

    let env = dir.write("api/env.toml", "[env.dev]\nbase = \"http://localhost\"\n");
    let req = dir.write("api/v1/health.routy", "GET /health {}\n");
    let changes: Vec<Value> = [env, req]
        .iter()
        .map(|p| json!({ "uri": Url::from_file_path(p).unwrap(), "type": 1 }))
        .collect();
    c.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": changes }),
    );
    let state = c.request("routy/state", Value::Null);
    assert_eq!(state["envs"], json!(["dev"]));
    assert_eq!(state["items"][0]["path"], "v1/health.routy");
}
