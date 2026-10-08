//! Сквозной тест: локальный HTTP-сервер → цепочка login → me с `save` и `assert`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;

use routy_core::runner::Options;
use routy_core::{Project, Runner, parse};

/// Отвечает на каждый запрос JSON-эхом: метод, путь, заголовок Authorization и тело.
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
            let (mut len, mut auth) = (0usize, String::new());
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
                let (k, v) = h.split_once(':').unwrap();
                match k.to_ascii_lowercase().as_str() {
                    "content-length" => len = v.trim().parse().unwrap(),
                    "authorization" => auth = v.trim().to_string(),
                    _ => {}
                }
            }
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let resp = serde_json::json!({
                "method": method,
                "path": path,
                "auth": auth,
                "body": String::from_utf8(body).unwrap(),
                "access_token": "tok-123",
            })
            .to_string();
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

fn runner(base: &str) -> Runner {
    let mut project = Project::load(PathBuf::from("/nonexistent")).unwrap();
    project.config = toml::from_str(&format!("[env.test]\nbase = \"{base}\"")).unwrap();
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

#[tokio::test]
async fn chain_with_save_and_assert() {
    let base = echo_server();
    let mut r = runner(&base);

    let login = parse(
        "POST {{base}}/login\n\n{\"user\": \"viktor\"}\n\n> save token = body.access_token\n> assert status == 201\n> assert body.method == POST\n",
    )
    .unwrap();
    let out = r.run(&login).await.unwrap();
    assert!(out.passed(), "{:?}", out.asserts);
    assert_eq!(out.saved["token"], "tok-123");
    assert!(
        out.request
            .headers
            .iter()
            .any(|h| h.value == "application/json")
    );

    let me = parse(
        "GET {{base}}/me\nAuthorization: Bearer {{token}}\n\n> assert body.auth == \"Bearer tok-123\"\n> assert body.path == /me\n> assert status == 200\n",
    )
    .unwrap();
    let out = r.run(&me).await.unwrap();
    assert_eq!(
        out.asserts.iter().map(|a| a.passed).collect::<Vec<_>>(),
        [true, true, false]
    );
    assert!(!out.passed());
}

#[tokio::test]
async fn missing_vars_are_all_reported() {
    let mut r = runner("http://unused");
    let req = parse("GET {{base}}/{{a}}\nX: {{b}}\n").unwrap();
    let err = r.run(&req).await.unwrap_err().to_string();
    assert!(err.contains("a, b"), "{err}");
}
