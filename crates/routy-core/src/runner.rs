//! Выполнение запросов: подстановка переменных → HTTP → `> save` / `> assert`.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::expr::{AssertOutcome, Subject, value_to_var};
use crate::parser::{Directive, Header, RequestFile};
use crate::project::Project;
use crate::state::{CachedCall, State, state_path};
use crate::template::render;
use crate::vars::{VarInfo, Vars};

#[derive(Debug, Clone)]
pub struct Options {
    pub timeout: Duration,
    /// Искать недостающие переменные в системном хранилище паролей.
    pub use_keyring: bool,
    /// Читать `ROUTY_*` из окружения процесса.
    pub process_env: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            timeout: Duration::from_secs(30),
            use_keyring: true,
            process_env: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<Header>,
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<Header>,
    pub body: String,
    #[serde(skip)]
    pub json: Option<Value>,
    /// Тело как есть — для картинок и сохранения в файл; `body` — его текст (lossy UTF-8).
    #[serde(skip)]
    pub raw: Vec<u8>,
    pub duration_ms: u64,
    pub size: usize,
}

impl Subject for Response {
    fn status(&self) -> u16 {
        self.status
    }
    fn duration_ms(&self) -> u64 {
        self.duration_ms
    }
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
    }
    fn body_json(&self) -> Option<&Value> {
        self.json.as_ref()
    }
    fn body_text(&self) -> &str {
        &self.body
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOutcome {
    pub request: ResolvedRequest,
    pub response: Response,
    pub saved: BTreeMap<String, String>,
    pub asserts: Vec<AssertOutcome>,
    /// `> save`, для которых в ответе не нашлось значения.
    pub save_misses: Vec<String>,
    /// Запросы, вызванные из `*.routy` по ходу этого запроса.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<crate::lang::exec::CallTrace>,
}

impl RunOutcome {
    pub fn passed(&self) -> bool {
        self.asserts.iter().all(|a| a.passed) && self.save_misses.is_empty()
    }
}

pub struct Runner {
    client: reqwest::Client,
    pub project: Project,
    pub env: String,
    pub vars: Vars,
    /// Ответы вызовов с `cache:` (`*.routy`) — живут в state-файле рядом с `save`.
    pub cached_calls: BTreeMap<String, CachedCall>,
}

impl Runner {
    pub fn new(project: Project, env: Option<&str>, opts: Options) -> Result<Runner> {
        let env = project.resolve_env(env)?;
        let mut vars = Vars {
            env: project.env_vars(&env),
            process_env: opts.process_env,
            ..Vars::default()
        };
        if opts.use_keyring {
            vars.secrets = secret_store(&project, &env);
        }
        let client = reqwest::Client::builder()
            .timeout(opts.timeout)
            .user_agent(concat!("routy/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Runner {
            client,
            project,
            env,
            vars,
            cached_calls: BTreeMap::new(),
        })
    }

    /// Подтягивает значения `> save` из прошлых запусков.
    pub fn load_saved(&mut self) -> Result<()> {
        if let Some(path) = state_path(&self.project.root) {
            let mut state = State::load(&path)?;
            self.vars.saved = state.envs.remove(&self.env).unwrap_or_default();
            self.cached_calls = state.cache.remove(&self.env).unwrap_or_default();
            self.cached_calls.retain(|_, c| c.fresh());
        }
        Ok(())
    }

    pub fn persist_saved(&self) -> Result<()> {
        let Some(path) = state_path(&self.project.root) else {
            return Ok(());
        };
        let mut state = State::load(&path)?;
        state.root = self.project.root.clone();
        state.envs.insert(self.env.clone(), self.vars.saved.clone());
        let mut cache = self.cached_calls.clone();
        cache.retain(|_, c| c.fresh());
        if cache.is_empty() {
            state.cache.remove(&self.env);
        } else {
            state.cache.insert(self.env.clone(), cache);
        }
        state.save(&path)
    }

    pub fn resolve(&self, file: &RequestFile) -> Result<ResolvedRequest> {
        // Собираем все недостающие переменные из всех частей запроса, а не только первую.
        let mut missing = Vec::new();
        let mut r = |s: &str| match render(s, |n| self.vars.get(n)) {
            Ok(v) => Ok(v),
            Err(Error::MissingVars(m)) => {
                for name in m {
                    if !missing.contains(&name) {
                        missing.push(name);
                    }
                }
                Ok(String::new())
            }
            Err(e) => Err(e),
        };
        let url = r(&file.url)?;
        let headers = file
            .headers
            .iter()
            .map(|h| {
                Ok(Header {
                    name: r(&h.name)?,
                    value: r(&h.value)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let body = file.body.as_deref().map(&mut r).transpose()?;
        if !missing.is_empty() {
            return Err(Error::MissingVars(missing));
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(Error::expr(
                &file.url,
                format!("URL must be absolute, got `{url}` (missing {{{{base}}}}?)"),
            ));
        }

        let mut headers = headers;
        let looks_json = body
            .as_deref()
            .is_some_and(|b| matches!(b.trim_start().chars().next(), Some('{' | '[')));
        if looks_json
            && !headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("content-type"))
        {
            headers.push(Header {
                name: "Content-Type".into(),
                value: "application/json".into(),
            });
        }
        Ok(ResolvedRequest {
            method: file.method.clone(),
            url,
            headers,
            body,
        })
    }

    /// HTTP-клиент раннера: дешёвый клон, чтобы отправлять запрос, не держа сам раннер
    /// (в GUI — без блокировки сессии, параллельно с другими запросами).
    pub fn client(&self) -> reqwest::Client {
        self.client.clone()
    }

    pub async fn send(&self, req: &ResolvedRequest) -> Result<Response> {
        send(&self.client, req).await
    }

    /// Запрос целиком. Сохранённые значения сразу доступны следующим запросам этого раннера.
    pub async fn run(&mut self, file: &RequestFile) -> Result<RunOutcome> {
        let request = self.resolve(file)?;
        let response = self.send(&request).await?;
        Ok(self.apply(file, request, response))
    }

    /// `> save` и `> assert` по полученному ответу.
    pub fn apply(
        &mut self,
        file: &RequestFile,
        request: ResolvedRequest,
        response: Response,
    ) -> RunOutcome {
        let mut saved = BTreeMap::new();
        let mut save_misses = Vec::new();
        let mut asserts = Vec::new();
        for d in &file.directives {
            match d {
                Directive::Save { var, path } => match path.eval(&response) {
                    Some(v) => {
                        let v = value_to_var(&v);
                        self.vars.saved.insert(var.clone(), v.clone());
                        saved.insert(var.clone(), v);
                    }
                    None => save_misses.push(format!("{var} = {}", path.source)),
                },
                Directive::Assert(a) => asserts.push(a.check(&response)),
            }
        }
        RunOutcome {
            request,
            response,
            saved,
            asserts,
            save_misses,
            calls: Vec::new(),
        }
    }

    pub async fn run_path(&mut self, path: &Path) -> Result<RunOutcome> {
        let src = std::fs::read_to_string(path).map_err(|e| Error::from(e).in_file(path))?;
        let file = crate::parse(&src).map_err(|e| e.in_file(path))?;
        self.run(&file).await.map_err(|e| e.in_file(path))
    }

    /// Итоговые значения переменных с источниками, включая объявленные в `secrets`.
    pub fn variables(&self) -> Result<Vec<VarInfo>> {
        self.vars.list(&self.project.config.secrets)
    }

    /// Забыть все значения `> save` и ответы `cache:` этого окружения (и на диске).
    pub fn clear_saved(&mut self) -> Result<()> {
        self.vars.saved.clear();
        self.cached_calls.clear();
        self.persist_saved()
    }
}

pub async fn send(client: &reqwest::Client, req: &ResolvedRequest) -> Result<Response> {
    let method = reqwest::Method::from_bytes(req.method.as_bytes())
        .map_err(|_| Error::expr(&req.method, "invalid HTTP method"))?;
    let mut builder = client.request(method, &req.url);
    for h in &req.headers {
        builder = builder.header(&h.name, &h.value);
    }
    if let Some(body) = &req.body {
        builder = builder.body(body.clone());
    }

    let started = Instant::now();
    read_response(builder.send().await?, started).await
}

/// Ответ целиком; `started` — момент отправки, для `duration_ms`.
pub(crate) async fn read_response(resp: reqwest::Response, started: Instant) -> Result<Response> {
    let status = resp.status();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| Header {
            name: k.to_string(),
            value: String::from_utf8_lossy(v.as_bytes()).into_owned(),
        })
        .collect();
    let bytes = resp.bytes().await?;
    let duration_ms = started.elapsed().as_millis() as u64;

    Ok(Response {
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or("").to_string(),
        headers,
        json: serde_json::from_slice(&bytes).ok(),
        body: String::from_utf8_lossy(&bytes).into_owned(),
        raw: bytes.to_vec(),
        duration_ms,
        size: bytes.len(),
    })
}

#[cfg(feature = "secrets")]
pub fn secret_store(project: &Project, env: &str) -> Option<Box<dyn crate::secrets::SecretStore>> {
    Some(Box::new(crate::secrets::KeyringStore::new(
        &project.id(),
        env,
    )))
}

#[cfg(not(feature = "secrets"))]
pub fn secret_store(_: &Project, _: &str) -> Option<Box<dyn crate::secrets::SecretStore>> {
    None
}
