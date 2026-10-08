//! Сквозной тест формата `*.routy`: локальный HTTP-сервер, вызовы запросов с кешем, cookies,
//! сценарии, опрос, ошибки с цепочкой вызовов и `routy check`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use routy_core::lang::exec::{Outcome, Run};
use routy_core::lang::{ItemRef, Workspace};
use routy_core::runner::Options;
use routy_core::{Error, Project, Runner};
use serde_json::{Value, json};

type Hits = Arc<Mutex<HashMap<String, usize>>>;

/// `/login` выдаёт токен и cookie, `/poll` готов с третьего раза, `/fail` — 500,
/// остальное — эхо запроса с номером обращения `n`.
fn server() -> (String, Hits) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let hits: Hits = Arc::default();
    let counter = hits.clone();
    std::thread::spawn(move || {
        let mut total = 0;
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut parts = line.split_whitespace();
            let method = parts.next().unwrap().to_string();
            let target = parts.next().unwrap().to_string();
            let (path, query) = target.split_once('?').unwrap_or((&target, ""));
            let mut headers = serde_json::Map::new();
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
                let (k, v) = h.split_once(':').unwrap();
                let k = k.trim().to_ascii_lowercase();
                if k == "content-length" {
                    len = v.trim().parse().unwrap();
                }
                headers.insert(k, json!(v.trim()));
            }
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let body = String::from_utf8(body).unwrap();

            total += 1;
            let n = {
                let mut hits = counter.lock().unwrap();
                let c = hits.entry(path.to_string()).or_default();
                *c += 1;
                *c
            };
            let mut extra = String::new();
            let (status, resp) = match path {
                "/login" => {
                    extra.push_str("Set-Cookie: session=s1; Path=/\r\n");
                    ("200 OK", json!({ "token": format!("tok-{n}") }))
                }
                "/poll" => ("200 OK", json!({ "done": n >= 3, "n": n })),
                "/fail" => ("500 Internal Server Error", json!({ "error": "boom" })),
                _ => (
                    if method == "POST" {
                        "201 Created"
                    } else {
                        "200 OK"
                    },
                    json!({
                        "method": method,
                        "path": path,
                        "query": query,
                        "headers": headers,
                        "body": serde_json::from_str::<Value>(&body).unwrap_or(json!(body)),
                        "n": total,
                    }),
                ),
            };
            let resp = resp.to_string();
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{resp}",
                resp.len()
            )
            .unwrap();
        }
    });
    (format!("http://{addr}"), hits)
}

fn runner(base: &str) -> Runner {
    let mut project = Project::load(PathBuf::from("/nonexistent")).unwrap();
    project.config = toml::from_str(&format!(
        "default = \"test\"\n[env.test]\nbase = \"{base}\"\n[env.prod]\n"
    ))
    .unwrap();
    Runner::new(
        project,
        None,
        Options {
            use_keyring: false,
            process_env: false,
            ..Options::default()
        },
    )
    .unwrap()
}

fn workspace(files: &[(&str, &str)]) -> Workspace {
    let ws = Workspace::from_sources(
        Path::new("/nonexistent"),
        files
            .iter()
            .map(|(p, t)| (PathBuf::from(p), t.to_string()))
            .collect(),
    );
    assert!(ws.errors.is_empty(), "{:?}", ws.errors);
    ws
}

fn find(ws: &Workspace, name: &str) -> ItemRef {
    ws.resolve(&[name.to_string()]).unwrap()
}

async fn exec(run: &mut Run, r: &mut Runner, name: &str) -> routy_core::Result<Outcome> {
    let item = find(run.workspace(), name);
    run.run_item(r, item).await
}

const LOGIN: &str = r#"// Login
POST /login {
  params { user: "bob" }
  body { user }
  expect { status == 200 }
}
"#;

const ORDERS: &str = r#"let shop = "main"

// Create order
POST /orders/{shop} {
  headers { Authorization: "Bearer ${Login().body.token}" }
  query { tags: ["a", "b"], skip: null }
  body {
    items: [{ sku: "A-1", qty: 2 }],
    who: Login().body.token,
  }
  expect {
    status == 201
    body.path == "/orders/${shop}"
    body.headers.authorization == "Bearer tok-1"
    body.headers.content-type == "application/json"
    body.body == { items: [{ sku: "A-1", qty: 2 }], who: "tok-1" }
    body.query == "tags=a&tags=b"
    body.headers.cookie == "session=s1"
    cookies.session == "s1"
    body matches { method: "POST" | "PUT", n: integer, note?: string }
    body.body.items.all(i => i.qty > 0)
  }
  save order_id = body.n
}

// Get order
GET /orders/{id}

// Checkout
flow Checkout {
  order = CreateOrder(shop: "x")
  expect { order.body.path == "/orders/x" }
  got = GetOrder(id: order.body.n)
  expect { got.body.path == "/orders/${order.body.n}" }
  save last = order.body.n
}
"#;

#[tokio::test]
async fn calls_are_cached_per_run_and_cookies_flow() {
    let (base, hits) = server();
    let mut r = runner(&base);
    let mut run = Run::new(
        workspace(&[("auth/login.routy", LOGIN), ("orders.routy", ORDERS)]),
        Duration::from_secs(5),
    )
    .unwrap();

    let Outcome::Request(o) = exec(&mut run, &mut r, "CreateOrder").await.unwrap() else {
        panic!()
    };
    let failed: Vec<_> = o.asserts.iter().filter(|a| !a.passed).collect();
    assert!(failed.is_empty(), "{failed:#?}");
    assert_eq!(
        hits.lock().unwrap()["/login"],
        1,
        "Login() is sent once per run"
    );
    assert_eq!(o.calls.len(), 2);
    assert!(!o.calls[0].cached && o.calls[1].cached);
    assert!(o.saved.contains_key("order_id"));

    let Outcome::Flow(f) = exec(&mut run, &mut r, "Checkout").await.unwrap() else {
        panic!()
    };
    assert!(f.passed(), "{f:#?}");
    assert_eq!(f.checks.len(), 2);
    assert_eq!(hits.lock().unwrap()["/login"], 1);
    assert!(r.vars.saved.contains_key("last"));

    run.reset().unwrap();
    exec(&mut run, &mut r, "Login").await.unwrap();
    assert_eq!(hits.lock().unwrap()["/login"], 2, "reset starts a new run");
}

#[tokio::test]
async fn failed_call_reports_the_chain() {
    let (base, _) = server();
    let mut r = runner(&base);
    let ws = workspace(&[(
        "x.routy",
        r#"
// Fail
GET /fail { expect { status == 200 } }

// Broken
GET /x { headers { X: Fail().body.error } }

// Checked
GET /x { expect { Fail().status == 500 } }

// Fresh
GET /x {
  expect {
    body.n > 1000
    fresh Counter().body.n != fresh Counter().body.n
    Counter().body.n == Counter().body.n
  }
}

// Counter
GET /counter
"#,
    )]);
    let mut run = Run::new(ws, Duration::from_secs(5)).unwrap();

    let err = exec(&mut run, &mut r, "Broken").await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "Broken → Fail: status == 200 — status is 500"
    );
    assert!(matches!(err, Error::Call { .. }));

    let Outcome::Request(o) = exec(&mut run, &mut r, "Checked").await.unwrap() else {
        panic!()
    };
    assert_eq!(
        o.asserts[0].detail.as_deref(),
        Some("Fail: status == 200 — status is 500")
    );

    let Outcome::Request(o) = exec(&mut run, &mut r, "Fresh").await.unwrap() else {
        panic!()
    };
    let n = o.response.json.as_ref().unwrap()["n"].clone();
    assert_eq!(o.asserts[0].detail, Some(format!("body.n is {n}")));
    assert!(
        o.asserts[1].passed && o.asserts[2].passed,
        "{:?}",
        o.asserts
    );
}

#[tokio::test]
async fn missing_vars_only_and_poll() {
    let (base, hits) = server();
    let mut r = runner(&base);
    let ws = workspace(&[(
        "x.routy",
        r#"
// Missing
GET /x/{id} { headers { A: a, B: "${b}" } }

// Prod only
GET /x { only: [prod] }

// Calls prod
GET /x { headers { A: ProdOnly().status } }

// Wait
GET /poll {
  poll body.done every 10ms for 2s
  expect { body.n == 3 }
}

// Too slow
GET /poll { poll body.n > 100 every 10ms for 30ms }
"#,
    )]);
    let mut run = Run::new(ws, Duration::from_secs(5)).unwrap();

    let err = exec(&mut run, &mut r, "Missing").await.unwrap_err();
    assert!(
        matches!(&err, Error::MissingVars(v) if v == &["id", "a", "b"]),
        "{err}"
    );

    let err = exec(&mut run, &mut r, "ProdOnly")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("limited to prod"), "{err}");
    let err = exec(&mut run, &mut r, "CallsProd")
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.starts_with("CallsProd → ProdOnly: ProdOnly is limited to prod"),
        "{err}"
    );

    let Outcome::Request(o) = exec(&mut run, &mut r, "Wait").await.unwrap() else {
        panic!()
    };
    assert!(o.passed(), "{:?}", o.asserts);
    assert_eq!(hits.lock().unwrap()["/poll"], 3);

    let err = exec(&mut run, &mut r, "TooSlow")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("still false"), "{err}");
}

#[test]
fn check_finds_names_args_shapes_and_cycles() {
    let ws = Workspace::from_sources(
        Path::new("/p"),
        vec![
            (
                "users/create.routy".into(),
                "// Create\nPOST /users {\n  params { name }\n}\n".into(),
            ),
            (
                "orders/create.routy".into(),
                "// Create\nPOST /orders\n".into(),
            ),
            (
                "a.routy".into(),
                r#"
// A
GET /a/{id} { headers { X: B().status } }

// B
GET /b { expect { A(id: 1).status == 200 } }

// C
GET /c {
  headers {
    X: Create().status
    Y: users.Create(nam: "x").status
    Z: Nope().status
  }
  expect { body matches Missing }
}

shape Order { id: string }
"#
                .into(),
            ),
            ("bad.routy".into(), "GET /x {\n".into()),
        ],
    );
    let errors: Vec<String> = ws.check().iter().map(|e| e.to_string()).collect();
    let has = |s: &str| errors.iter().any(|e| e.contains(s));
    assert!(has("/p/bad.routy:1:8: unclosed `{`"), "{errors:#?}");
    assert!(
        has("`Create` is ambiguous, call it with its folder: users.Create, orders.Create")
            || has("`Create` is ambiguous, call it with its folder: orders.Create, users.Create"),
        "{errors:#?}"
    );
    assert!(
        has("`users.Create` has no parameter `nam` (parameters: name)"),
        "{errors:#?}"
    );
    assert!(has("unknown request or flow `Nope`"), "{errors:#?}");
    assert!(has("unknown shape `Missing`"), "{errors:#?}");
    assert!(has("call cycle: A → B → A"), "{errors:#?}");
    assert_eq!(errors.len(), 6, "{errors:#?}");
}

#[tokio::test]
async fn cache_between_runs_multipart_and_schema() {
    let (base, hits) = server();
    let mut r = runner(&base);
    let dir = std::env::temp_dir().join(format!("routy-lang-test-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("schemas")).unwrap();
    std::fs::write(dir.join("note.txt"), "hello").unwrap();
    std::fs::write(
        dir.join("schemas/echo.json"),
        r#"{ "type": "object", "required": ["method"], "properties": { "method": { "enum": ["GET"] } } }"#,
    )
    .unwrap();
    let ws = Workspace::from_sources(
        &dir,
        vec![
            (
                "a.routy".into(),
                r#"
// Login
POST /login { cache: 1h }

// Uses login
GET /x { headers { T: Login().body.token } }

// Upload
POST /upload {
  multipart {
    title: "Note"
    tags: ["a", "b"]
    doc: file("./note.txt")
  }
  expect {
    body.headers.content-type.startsWith("multipart/form-data; boundary=")
    body.body.contains("name=\"doc\"; filename=\"note.txt\"\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nhello\r\n")
    body.body.contains("name=\"tags\"\r\n\r\nb\r\n")
    body matches Echo
    body matches schema("./schemas/echo.json")
    !(body matches Get)
  }
}
"#
                .into(),
            ),
            (
                "schemas/shapes.routy".into(),
                "shape Echo { method: string, path: \"/upload\" }\nshape Get = schema(\"./echo.json\")\n".into(),
            ),
        ],
    );
    assert!(ws.errors.is_empty(), "{:?}", ws.errors);
    assert!(ws.check().is_empty(), "{:?}", ws.check());
    let mut run = Run::new(ws, Duration::from_secs(5)).unwrap();

    let Outcome::Request(o) = exec(&mut run, &mut r, "Upload").await.unwrap() else {
        panic!()
    };
    assert!(!o.asserts[4].passed);
    assert_eq!(
        o.asserts[4].detail.as_deref(),
        Some("body.method: expected one of \"GET\", got \"POST\"")
    );
    assert!(
        o.asserts[..4]
            .iter()
            .chain(&o.asserts[5..])
            .all(|a| a.passed),
        "{:#?}",
        o.asserts
    );

    exec(&mut run, &mut r, "UsesLogin").await.unwrap();
    assert_eq!(hits.lock().unwrap()["/login"], 1);
    assert_eq!(r.cached_calls.len(), 1);

    // Новый прогон (кеш прогона пуст), но ответ Login живёт в `cache:` раннера.
    run.reset().unwrap();
    let Outcome::Request(o) = exec(&mut run, &mut r, "UsesLogin").await.unwrap() else {
        panic!()
    };
    assert_eq!(hits.lock().unwrap()["/login"], 1, "cached between runs");
    assert!(o.calls[0].cached);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn check_env_finds_only_and_missing_vars() {
    let ws = workspace(&[(
        "a.routy",
        r#"
let shop = "main"

// Login
POST /login {
  params { user: "bob", password }
  body { user, password, shop }
  save token = body.token
}

// Seed
POST /seed { only: [dev, stagin] }

// Order
POST /orders {
  headers { A: "${api_key}", B: vars["x-trace"] }
  body { t: Login().body.token, s: Seed().status, ok: [1].all(i => i > 0) }
  expect { body.id == token && status == 201 }
}

// Flow
flow Buy {
  params { qty: 1 }
  o = Order()
  expect { o.status == qty && missing_one == 1 }
}
"#,
    )]);
    let envs = vec!["dev".to_string(), "prod".to_string(), "staging".to_string()];
    let mut has = |n: &str| n == "base";
    let errors: Vec<String> = ws
        .check_env("prod", &envs, &mut has)
        .iter()
        .map(|e| e.to_string())
        .collect();
    let want = [
        "/nonexistent/a.routy:12:1: `only` names unknown environment `stagin` (did you mean `staging`?)",
        "/nonexistent/a.routy:16:19: variable `api_key` is not defined in prod",
        "/nonexistent/a.routy:16:33: variable `x-trace` is not defined in prod",
        "/nonexistent/a.routy:17:13: `Login` needs password — pass it or define the variable in prod",
        "/nonexistent/a.routy:17:36: `Seed` is limited to dev, stagin and cannot be called in prod",
        "/nonexistent/a.routy:25:31: variable `missing_one` is not defined in prod",
    ];
    assert_eq!(errors, want);

    let mut none = |_: &str| false;
    let errors = ws.check_env("dev", &envs, &mut none);
    assert!(
        errors
            .iter()
            .any(|e| e.msg.contains("`base` is not defined in dev")),
        "{errors:#?}"
    );
}

#[tokio::test]
async fn editor_workflow_keeps_the_run() {
    let (base, hits) = server();
    let mut r = runner(&base);
    let a = "// Login\nPOST /login\n\n// Me\nGET /me {\n  headers { T: Login().body.token }\n}\n";
    let ws = workspace(&[("a.routy", a)]);
    assert_eq!(ws.item_at(0, 1), Some(ItemRef { file: 0, item: 0 }));
    assert_eq!(ws.item_at(0, 3), Some(ItemRef { file: 0, item: 0 }));
    assert_eq!(ws.item_at(0, 6), Some(ItemRef { file: 0, item: 1 }));
    let mut run = Run::new(ws, Duration::from_secs(5)).unwrap();
    run.run_item(&mut r, ItemRef { file: 0, item: 1 })
        .await
        .unwrap();

    // Правка в редакторе: новый текст файла, элементы сдвинулись — Login() всё ещё из кеша.
    let edited = format!("// Health\nGET /health\n\n{a}");
    run.set_workspace(workspace(&[("a.routy", &edited)]));
    let me = run.workspace().item_at(0, 9).unwrap();
    assert_eq!(me.item, 2);
    let Outcome::Request(o) = run.run_item(&mut r, me).await.unwrap() else {
        panic!()
    };
    assert!(o.calls[0].cached);
    assert_eq!(hits.lock().unwrap()["/login"], 1);
}
