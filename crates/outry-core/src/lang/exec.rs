//! Выполнение `*.outry`: подготовка запроса → HTTP (с опросом) → `expect` / `save`.
//! Вызовы других запросов кешируются на прогон, cookies живут столько же.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::cookie::{CookieStore, Jar};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::ast::*;
use super::eval::{BoxFuture, Eval, Host, show};
use super::{ItemRef, Source, Workspace};
use crate::error::{Error, Result};
use crate::expr::{AssertOutcome, value_to_var};
use crate::parser::Header;
use crate::runner::{ResolvedRequest, Response, RunOutcome, Runner, read_response};
use crate::state::{CachedCall, unix_now};

/// Запрос, вызванный по ходу выполнения: для trace в CLI и GUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallTrace {
    pub name: String,
    /// Вложенность: 0 — вызван прямо из запускаемого элемента.
    pub depth: usize,
    pub args: Value,
    /// Ответ взят из кеша прогона, запрос не отправлялся.
    pub cached: bool,
    pub status: Option<u16>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FlowOutcome {
    pub checks: Vec<AssertOutcome>,
    pub saved: BTreeMap<String, String>,
    pub calls: Vec<CallTrace>,
    /// Шаг, на котором сценарий остановился с ошибкой.
    pub error: Option<String>,
}

impl FlowOutcome {
    pub fn passed(&self) -> bool {
        self.error.is_none() && self.checks.iter().all(|c| c.passed)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum Outcome {
    Request(RunOutcome),
    Flow(FlowOutcome),
}

impl Outcome {
    pub fn passed(&self) -> bool {
        match self {
            Outcome::Request(o) => o.passed(),
            Outcome::Flow(o) => o.passed(),
        }
    }
}

/// Спросить перед отправкой запроса с `confirm: true`: имя и готовый запрос → отправлять ли.
pub type Confirm = Box<dyn FnMut(&str, &ResolvedRequest) -> bool + Send>;

/// Один прогон: кеш вызовов, cookies, вычисленные `let`, сохранённые значения с типами.
pub struct Run {
    ws: Arc<Workspace>,
    timeout: Duration,
    jar: Arc<Jar>,
    client: reqwest::Client,
    no_redirects: reqwest::Client,
    /// Ответы вызовов по `файл#имя:аргументы` — ключ переживает замену рабочего пространства.
    cache: HashMap<String, Value>,
    lets: HashMap<std::path::PathBuf, Vec<(String, Value)>>,
    /// `save` этого прогона: в `Vars` значения — строки, а здесь — как были (число, объект).
    typed: HashMap<String, Value>,
    pub confirm: Option<Confirm>,
}

impl Run {
    pub fn new(ws: Workspace, timeout: Duration) -> Result<Run> {
        let jar = Arc::new(Jar::default());
        let (client, no_redirects) = clients(&jar, timeout)?;
        Ok(Run {
            ws: Arc::new(ws),
            timeout,
            jar,
            client,
            no_redirects,
            cache: HashMap::new(),
            lets: HashMap::new(),
            typed: HashMap::new(),
            confirm: None,
        })
    }

    pub fn workspace(&self) -> &Workspace {
        &self.ws
    }

    /// Новые тексты файлов (правки в редакторе) без нового прогона: кеш вызовов, cookies и
    /// сохранённые значения остаются, `let` вычисляются заново.
    pub fn set_workspace(&mut self, ws: Workspace) {
        self.ws = Arc::new(ws);
        self.lets.clear();
    }

    /// Новый прогон: кеш вызовов, cookies и `let` — заново.
    pub fn reset(&mut self) -> Result<()> {
        self.jar = Arc::new(Jar::default());
        (self.client, self.no_redirects) = clients(&self.jar, self.timeout)?;
        self.cache.clear();
        self.lets.clear();
        self.typed.clear();
        Ok(())
    }

    /// Запускает запрос или сценарий. `Err` — запрос не удалось подготовить или отправить;
    /// упавшие проверки — в `Outcome`.
    pub async fn run_item(&mut self, runner: &mut Runner, r: ItemRef) -> Result<Outcome> {
        let ws = self.ws.clone();
        let mut ex = Exec {
            runner,
            run: self,
            stack: Vec::new(),
            trace: Vec::new(),
        };
        match ws.item(r) {
            Item::Request(req) => {
                // Упавший вызов — с именем запущенного в начале цепочки: `Me → Login: …`.
                let mut o = ex
                    .request(r, Vec::new())
                    .await
                    .map_err(|e| match (&e, &req.name) {
                        (Error::Call { .. }, Some(name)) => chained(name, e),
                        _ => e,
                    })?;
                o.calls = std::mem::take(&mut ex.trace);
                // Запуск без аргументов — то же, что вызов `Name()`: следующий вызов возьмёт ответ.
                if o.passed() {
                    let cookies = ex.cookies(&o.request.url);
                    let v = response_value(&o.response, cookies);
                    ex.remember(r, &Value::Object(Map::new()), v);
                }
                Ok(Outcome::Request(o))
            }
            Item::Flow(_) => {
                let mut o = ex.flow(r, Vec::new()).await?;
                o.calls = std::mem::take(&mut ex.trace);
                Ok(Outcome::Flow(o))
            }
            _ => Err(Error::Run("only requests and flows can be run".into())),
        }
    }

    /// Запрос в том виде, в каком ушёл бы, но без отправки — для «Copy as curl». Вызовы в
    /// выражениях (`Login().body.token`) выполняются (или берутся из кеша прогона); cookies
    /// прогона для адреса добавляются заголовком `Cookie`.
    pub async fn resolve_item(
        &mut self,
        runner: &mut Runner,
        r: ItemRef,
    ) -> Result<(ResolvedRequest, Option<Vec<u8>>)> {
        let ws = self.ws.clone();
        let Item::Request(req) = ws.item(r) else {
            return Err(Error::Run("only requests can be copied as curl".into()));
        };
        let src = &ws.sources[r.file];
        let name = req
            .name
            .clone()
            .unwrap_or_else(|| format!("{}:{}", src.path.display(), src.line(req.span.start)));
        let mut ex = Exec {
            runner,
            run: self,
            stack: Vec::new(),
            trace: Vec::new(),
        };
        ex.enter(r, &name)?;
        let locals = ex.base_locals(src).await?;
        let allowed = ws.params_of(r);
        let locals = ex
            .bind_params(src, &name, &req.fields.params, &allowed, Vec::new(), locals)
            .await?;
        let (mut request, body, _) = ex.prepare(src, req, locals).await?;
        let jar = reqwest::Url::parse(&request.url)
            .ok()
            .and_then(|u| ex.run.jar.cookies(&u));
        if let Some(cookie) = jar {
            if !request
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("cookie"))
            {
                request.headers.push(Header {
                    name: "Cookie".into(),
                    value: String::from_utf8_lossy(cookie.as_bytes()).into_owned(),
                });
            }
        }
        Ok((request, body))
    }
}

/// Ключ кеша вызовов (и `cache:` между прогонами): индексы элементов меняются от правок,
/// путь и имя — реже.
fn persist_key(ws: &Workspace, r: ItemRef, args: &Value) -> String {
    let name = match ws.item(r).name() {
        Some(n) => n.to_string(),
        None => format!("@{}", r.item),
    };
    format!("{}#{name}:{args}", ws.sources[r.file].path.display())
}

/// `cache: 30m` запроса — `None` у сценариев и запросов без него.
fn cache_ttl(ws: &Workspace, r: ItemRef) -> Option<Duration> {
    match ws.item(r) {
        Item::Request(req) => req.fields.cache,
        _ => None,
    }
}

fn clients(jar: &Arc<Jar>, timeout: Duration) -> Result<(reqwest::Client, reqwest::Client)> {
    let build = |policy| {
        reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(concat!("outry/", env!("CARGO_PKG_VERSION")))
            .cookie_provider(jar.clone())
            .redirect(policy)
            .build()
    };
    Ok((
        build(reqwest::redirect::Policy::limited(10))?,
        build(reqwest::redirect::Policy::none())?,
    ))
}

type Locals = Vec<(String, Value)>;

struct Exec<'r> {
    runner: &'r mut Runner,
    run: &'r mut Run,
    /// Выполняемые сейчас элементы — для поиска циклов.
    stack: Vec<(ItemRef, String)>,
    trace: Vec<CallTrace>,
}

impl Host for Exec<'_> {
    fn var(&mut self, name: &str) -> Result<Option<Value>> {
        if let Some(v) = self.runner.vars.overrides.get(name) {
            return Ok(Some(Value::String(v.clone())));
        }
        if let Some(v) = self.run.typed.get(name) {
            return Ok(Some(v.clone()));
        }
        Ok(self.runner.vars.get(name)?.map(Value::String))
    }

    fn call<'a>(
        &'a mut self,
        call: &'a Call,
        args: Vec<(String, Value)>,
    ) -> BoxFuture<'a, Result<Value>> {
        Box::pin(self.call_inner(call, args))
    }

    fn shape(&self, name: &str) -> Option<Shape> {
        self.run.ws.shape(name).cloned()
    }
}

/// Ошибка вызванного `name`: добавляет его в цепочку `A → B: …`.
fn chained(name: &str, e: Error) -> Error {
    match e {
        Error::Call { mut chain, msg } => {
            chain.insert(0, name.to_string());
            Error::Call { chain, msg }
        }
        other => Error::Call {
            chain: vec![name.to_string()],
            msg: other.to_string(),
        },
    }
}

fn check_text(c: &AssertOutcome) -> String {
    match &c.detail {
        Some(d) => format!("{} — {d}", c.source),
        None => c.source.clone(),
    }
}

impl Exec<'_> {
    fn enter(&mut self, r: ItemRef, name: &str) -> Result<()> {
        if let Some(i) = self.stack.iter().position(|(x, _)| *x == r) {
            let mut chain: Vec<&str> = self.stack[i..].iter().map(|(_, n)| n.as_str()).collect();
            chain.push(name);
            return Err(Error::Run(format!("call cycle: {}", chain.join(" → "))));
        }
        self.stack.push((r, name.to_string()));
        Ok(())
    }

    async fn call_inner(&mut self, call: &Call, args: Vec<(String, Value)>) -> Result<Value> {
        let name = call.name();
        let ws = self.run.ws.clone();
        let target = ws.resolve(&call.path).map_err(Error::Run)?;
        let args_obj = Value::Object(args.iter().cloned().collect::<Map<_, _>>());
        let key = persist_key(&ws, target, &args_obj);
        let is_flow = matches!(ws.item(target), Item::Flow(_));
        let depth = self.stack.len().saturating_sub(1);

        if !call.fresh && !is_flow {
            let stored = cache_ttl(&ws, target)
                .and_then(|_| self.runner.cached_calls.get(&key))
                .filter(|c| c.fresh())
                .map(|c| c.value.clone());
            if let Some(v) = stored {
                self.run.cache.entry(key.clone()).or_insert(v);
            }
            if let Some(v) = self.run.cache.get(&key) {
                self.trace.push(CallTrace {
                    name,
                    depth,
                    args: args_obj,
                    cached: true,
                    status: v["status"].as_u64().map(|s| s as u16),
                    duration_ms: None,
                });
                return Ok(v.clone());
            }
        }
        let idx = self.trace.len();
        self.trace.push(CallTrace {
            name: name.clone(),
            depth,
            args: args_obj,
            cached: false,
            status: None,
            duration_ms: None,
        });

        if is_flow {
            let out = self
                .flow(target, args)
                .await
                .map_err(|e| chained(&name, e))?;
            if let Some(err) = out.error {
                return Err(chained(&name, Error::Run(err)));
            }
            if let Some(c) = out.checks.iter().find(|c| !c.passed) {
                return Err(chained(&name, Error::Run(check_text(c))));
            }
            return Ok(Value::Null);
        }

        let out = self
            .request(target, args)
            .await
            .map_err(|e| chained(&name, e))?;
        self.trace[idx].status = Some(out.response.status);
        self.trace[idx].duration_ms = Some(out.response.duration_ms);
        if let Some(c) = out.asserts.iter().find(|c| !c.passed) {
            return Err(chained(&name, Error::Run(check_text(c))));
        }
        if let Some(m) = out.save_misses.first() {
            return Err(chained(&name, Error::Run(format!("save {m}"))));
        }
        let cookies = self.cookies(&out.request.url);
        let v = response_value(&out.response, cookies);
        let args_obj = std::mem::take(&mut self.trace[idx].args);
        self.remember(target, &args_obj, v.clone());
        self.trace[idx].args = args_obj;
        Ok(v)
    }

    /// Ответ вызова — в кеш прогона и, если у запроса есть `cache:`, в state-файл.
    fn remember(&mut self, r: ItemRef, args: &Value, v: Value) {
        let ws = self.run.ws.clone();
        let key = persist_key(&ws, r, args);
        if let Some(ttl) = cache_ttl(&ws, r) {
            self.runner.cached_calls.insert(
                key.clone(),
                CachedCall {
                    expires: unix_now() + ttl.as_secs().max(1),
                    value: v.clone(),
                },
            );
        }
        self.run.cache.insert(key, v);
    }

    /// `env` и `let` файла; `let` вычисляются один раз за прогон.
    async fn base_locals(&mut self, src: &Source) -> Result<Locals> {
        let mut locals = vec![("env".to_string(), Value::String(self.runner.env.clone()))];
        if let Some(lets) = self.run.lets.get(&src.path) {
            locals.extend(lets.iter().cloned());
            return Ok(locals);
        }
        let mut ev = eval_in(self, src);
        ev.locals = locals;
        let mut lets = Vec::new();
        for item in &src.file.items {
            if let Item::Let(l) = item {
                let v = ev.eval(&l.value).await?;
                ev.set(&l.name, v.clone());
                lets.push((l.name.clone(), v));
            }
        }
        drop(ev);
        self.run.lets.insert(src.path.clone(), lets.clone());
        let mut locals = vec![("env".to_string(), Value::String(self.runner.env.clone()))];
        locals.extend(lets);
        Ok(locals)
    }

    /// Аргументы и умолчания параметров. Параметр без того и другого берётся из переменных.
    async fn bind_params(
        &mut self,
        src: &Source,
        name: &str,
        params: &[Param],
        allowed: &[String],
        args: Vec<(String, Value)>,
        locals: Locals,
    ) -> Result<Locals> {
        for (a, _) in &args {
            if !allowed.contains(a) {
                return Err(Error::Run(format!("`{name}` has no parameter `{a}`")));
            }
        }
        let mut ev = eval_in(self, src);
        ev.locals = locals;
        for p in params {
            if args.iter().any(|(a, _)| *a == p.name) {
                continue;
            }
            if let Some(d) = &p.default {
                let v = ev.eval(d).await?;
                ev.set(&p.name, v);
            }
        }
        for (n, v) in args {
            ev.set(&n, v);
        }
        Ok(ev.locals)
    }

    async fn request(&mut self, r: ItemRef, args: Vec<(String, Value)>) -> Result<RunOutcome> {
        let ws = self.run.ws.clone();
        let src = &ws.sources[r.file];
        let Item::Request(req) = ws.item(r) else {
            return Err(Error::Run("not a request".into()));
        };
        let name = req
            .name
            .clone()
            .unwrap_or_else(|| format!("{}:{}", src.path.display(), src.line(req.span.start)));
        if let Some(only) = &req.fields.only {
            if !only.contains(&self.runner.env) {
                return Err(Error::Run(format!(
                    "{name} is limited to {} (current environment: {})",
                    only.join(", "),
                    self.runner.env
                )));
            }
        }
        self.enter(r, &name)?;
        let res = self.request_inner(&ws, r, req, &name, args).await;
        self.stack.pop();
        res
    }

    async fn request_inner(
        &mut self,
        ws: &Workspace,
        r: ItemRef,
        req: &Request,
        name: &str,
        args: Vec<(String, Value)>,
    ) -> Result<RunOutcome> {
        let src = &ws.sources[r.file];
        let locals = self.base_locals(src).await?;
        let allowed = ws.params_of(r);
        let locals = self
            .bind_params(src, name, &req.fields.params, &allowed, args, locals)
            .await?;
        let (request, body, locals) = self.prepare(src, req, locals).await?;

        if req.fields.confirm {
            match &mut self.run.confirm {
                Some(ask) => {
                    if !ask(name, &request) {
                        return Err(Error::Run(format!("{name}: not confirmed, not sent")));
                    }
                }
                None => {
                    return Err(Error::Run(format!(
                        "{name} asks for confirmation (`confirm: true`); run it interactively or with --yes"
                    )));
                }
            }
        }

        let timeout = req.fields.timeout.unwrap_or(self.run.timeout);
        let follow = req.fields.redirects.unwrap_or(true);
        let started = Instant::now();
        let (response, cookies) = loop {
            let response = self
                .send(&request, body.as_deref(), timeout, follow)
                .await?;
            let cookies = self.cookies(&request.url);
            let Some(poll) = &req.fields.poll else {
                break (response, cookies);
            };
            let mut ev = eval_in(self, src);
            ev.locals = locals.clone();
            set_response(&mut ev, &response, cookies.clone());
            match ev.eval(&poll.until).await? {
                Value::Bool(true) => break (response, cookies),
                Value::Bool(false) => {}
                other => {
                    return Err(Error::expr(
                        poll.until.span.text(&src.text),
                        format!("poll condition must be a boolean, got {}", show(&other)),
                    ));
                }
            }
            if started.elapsed() + poll.every > poll.limit {
                return Err(Error::Run(format!(
                    "poll: `{}` is still false after {}s (last status {})",
                    poll.until.span.text(&src.text),
                    poll.limit.as_secs_f64(),
                    response.status
                )));
            }
            tokio::time::sleep(poll.every).await;
        };

        let mut ev = eval_in(self, src);
        ev.locals = locals;
        set_response(&mut ev, &response, cookies);
        let mut asserts = Vec::new();
        for c in &req.fields.expect {
            asserts.push(ev.check(c).await);
        }
        let mut values = Vec::new();
        let mut save_misses = Vec::new();
        for s in &req.fields.saves {
            let text = s.value.span.text(&src.text);
            match ev.eval(&s.value).await {
                Ok(Value::Null) => save_misses.push(format!("{} = {text} (null)", s.name)),
                Ok(v) => values.push((s.name.clone(), v)),
                Err(e) => save_misses.push(format!("{} = {text}: {e}", s.name)),
            }
        }
        drop(ev);
        let saved = self.save(values);
        Ok(RunOutcome {
            request,
            response,
            saved,
            asserts,
            save_misses,
            calls: Vec::new(),
        })
    }

    fn save(&mut self, values: Vec<(String, Value)>) -> BTreeMap<String, String> {
        let mut saved = BTreeMap::new();
        for (n, v) in values {
            let text = value_to_var(&v);
            self.runner.vars.saved.insert(n.clone(), text.clone());
            self.run.typed.insert(n.clone(), v);
            saved.insert(n, text);
        }
        saved
    }

    /// Адрес, query, заголовки и тело. Все недостающие переменные — одной ошибкой.
    async fn prepare(
        &mut self,
        src: &Source,
        req: &Request,
        locals: Locals,
    ) -> Result<(ResolvedRequest, Option<Vec<u8>>, Locals)> {
        let mut ev = eval_in(self, src);
        ev.locals = locals;
        ev.missing = Some(Vec::new());

        let kind = req.target.kind;
        let mut url = String::new();
        if kind == TargetKind::Path {
            let base = ev.lookup("base").await?;
            if !base.is_null() {
                url.push_str(value_to_var(&base).trim_end_matches('/'));
            }
        }
        for part in &req.target.parts {
            match part {
                TargetPart::Lit(s) => url.push_str(s),
                TargetPart::Param(n, _) => {
                    let v = ev.lookup(n).await?;
                    url.push_str(&encode(&value_to_var(&v), false));
                }
                TargetPart::Expr(e) => {
                    let v = value_to_var(&ev.eval(e).await?);
                    if kind == TargetKind::Str {
                        url.push_str(&v);
                    } else {
                        url.push_str(&encode(&v, false));
                    }
                }
            }
        }

        let mut query = Vec::new();
        for e in &req.fields.query {
            let v = ev.eval(&e.value).await?;
            pairs(&mut query, &e.key, v);
        }
        let mut headers = Vec::new();
        for e in &req.fields.headers {
            let v = ev.eval(&e.value).await?;
            if !v.is_null() {
                headers.push(Header {
                    name: e.key.clone(),
                    value: value_to_var(&v),
                });
            }
        }
        let (body, content_type) = match &req.fields.body {
            None => (None, None),
            Some(Body::Value(e)) => match &e.kind {
                ExprKind::Builtin(f, args) if f == "file" => {
                    let (path, bytes) = read_file(&mut ev, src, e, args).await?;
                    (Some(bytes), Some(mime(&path).to_string()))
                }
                ExprKind::Str(_) => {
                    let v = value_to_var(&ev.eval(e).await?);
                    (
                        Some(v.into_bytes()),
                        Some("text/plain; charset=utf-8".into()),
                    )
                }
                _ => {
                    let v = ev.eval(e).await?;
                    (
                        Some(serde_json::to_vec(&v)?),
                        Some("application/json".into()),
                    )
                }
            },
            Some(Body::Form(entries)) => {
                let mut form = Vec::new();
                for e in entries {
                    let v = ev.eval(&e.value).await?;
                    pairs(&mut form, &e.key, v);
                }
                let text = form
                    .iter()
                    .map(|(k, v)| format!("{}={}", encode(k, true), encode(v, true)))
                    .collect::<Vec<_>>()
                    .join("&");
                (
                    Some(text.into_bytes()),
                    Some("application/x-www-form-urlencoded".into()),
                )
            }
            Some(Body::Multipart(entries)) => {
                let boundary = format!("outry-{}", crate::dynamic::uuid().replace('-', ""));
                let mut out = Vec::new();
                for e in entries {
                    let mut part = |head: String, data: &[u8]| {
                        out.extend_from_slice(format!("--{boundary}\r\n{head}\r\n\r\n").as_bytes());
                        out.extend_from_slice(data);
                        out.extend_from_slice(b"\r\n");
                    };
                    let key = e.key.replace('"', "%22");
                    if let ExprKind::Builtin(f, args) = &e.value.kind {
                        if f == "file" {
                            let (path, bytes) = read_file(&mut ev, src, &e.value, args).await?;
                            let name = std::path::Path::new(&path)
                                .file_name()
                                .map(|n| n.to_string_lossy().replace('"', "%22"))
                                .unwrap_or_default();
                            part(
                                format!(
                                    "Content-Disposition: form-data; name=\"{key}\"; filename=\"{name}\"\r\nContent-Type: {}",
                                    mime(&path)
                                ),
                                &bytes,
                            );
                            continue;
                        }
                    }
                    let mut values = Vec::new();
                    pairs(&mut values, &e.key, ev.eval(&e.value).await?);
                    for (_, v) in values {
                        part(
                            format!("Content-Disposition: form-data; name=\"{key}\""),
                            v.as_bytes(),
                        );
                    }
                }
                out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
                (
                    Some(out),
                    Some(format!("multipart/form-data; boundary={boundary}")),
                )
            }
        };

        let missing = ev.missing.take().unwrap_or_default();
        let locals = std::mem::take(&mut ev.locals);
        drop(ev);
        if !missing.is_empty() {
            return Err(Error::MissingVars(missing));
        }

        let mut parsed = reqwest::Url::parse(&url).map_err(|e| {
            Error::expr(
                req.target.span.text(&src.text),
                format!("invalid URL `{url}`: {e}"),
            )
        })?;
        if !query.is_empty() {
            let mut qp = parsed.query_pairs_mut();
            for (k, v) in &query {
                qp.append_pair(k, v);
            }
        }
        if let Some(ct) = content_type {
            if !headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("content-type"))
            {
                headers.push(Header {
                    name: "Content-Type".into(),
                    value: ct,
                });
            }
        }
        let request = ResolvedRequest {
            method: req.method.clone(),
            url: parsed.to_string(),
            headers,
            body: body
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).into_owned()),
        };
        Ok((request, body, locals))
    }

    async fn send(
        &mut self,
        req: &ResolvedRequest,
        body: Option<&[u8]>,
        timeout: Duration,
        follow: bool,
    ) -> Result<Response> {
        let client = if follow {
            &self.run.client
        } else {
            &self.run.no_redirects
        };
        let method = reqwest::Method::from_bytes(req.method.as_bytes())
            .map_err(|_| Error::expr(&req.method, "invalid HTTP method"))?;
        let mut b = client.request(method, &req.url).timeout(timeout);
        for h in &req.headers {
            b = b.header(&h.name, &h.value);
        }
        if let Some(body) = body {
            b = b.body(body.to_vec());
        }
        let started = Instant::now();
        read_response(b.send().await?, started).await
    }

    /// Cookies прогона для адреса: `{ "session": "abc" }`.
    fn cookies(&self, url: &str) -> Value {
        let mut out = Map::new();
        let header = reqwest::Url::parse(url)
            .ok()
            .and_then(|u| self.run.jar.cookies(&u));
        if let Some(h) = header {
            for pair in String::from_utf8_lossy(h.as_bytes()).split(';') {
                if let Some((k, v)) = pair.trim().split_once('=') {
                    out.insert(k.to_string(), Value::String(v.to_string()));
                }
            }
        }
        Value::Object(out)
    }

    async fn flow(&mut self, r: ItemRef, args: Vec<(String, Value)>) -> Result<FlowOutcome> {
        let ws = self.run.ws.clone();
        let Item::Flow(fl) = ws.item(r) else {
            return Err(Error::Run("not a flow".into()));
        };
        self.enter(r, &fl.name)?;
        let res = self.flow_inner(&ws, r, fl, args).await;
        self.stack.pop();
        res
    }

    async fn flow_inner(
        &mut self,
        ws: &Workspace,
        r: ItemRef,
        fl: &Flow,
        args: Vec<(String, Value)>,
    ) -> Result<FlowOutcome> {
        let src = &ws.sources[r.file];
        let locals = self.base_locals(src).await?;
        let allowed = ws.params_of(r);
        let locals = self
            .bind_params(src, &fl.name, &fl.params, &allowed, args, locals)
            .await?;

        let mut out = FlowOutcome::default();
        let mut values = Vec::new();
        let mut ev = eval_in(self, src);
        ev.locals = locals;
        for step in &fl.steps {
            match step {
                Step::Bind { name, value, .. } => match ev.eval(value).await {
                    Ok(v) => ev.set(name, v),
                    Err(e) => {
                        out.error = Some(e.to_string());
                        break;
                    }
                },
                Step::Do(e) => {
                    if let Err(e) = ev.eval(e).await {
                        out.error = Some(e.to_string());
                        break;
                    }
                }
                Step::Expect(checks, _) => {
                    let mut failed = false;
                    for c in checks {
                        let o = ev.check(c).await;
                        failed |= !o.passed;
                        out.checks.push(o);
                    }
                    if failed {
                        break;
                    }
                }
                Step::Save(s) => match ev.eval(&s.value).await {
                    Ok(Value::Null) => {
                        out.error = Some(format!(
                            "save {} = {}: value is null",
                            s.name,
                            s.value.span.text(&src.text)
                        ));
                        break;
                    }
                    Ok(v) => {
                        ev.set(&s.name, v.clone());
                        values.push((s.name.clone(), v));
                    }
                    Err(e) => {
                        out.error = Some(e.to_string());
                        break;
                    }
                },
            }
        }
        drop(ev);
        out.saved = self.save(values);
        Ok(out)
    }
}

fn eval_in<'a>(host: &'a mut dyn Host, src: &'a Source) -> Eval<'a> {
    let mut ev = Eval::new(host, &src.text);
    ev.dir = src.full.parent().map(Into::into).unwrap_or_default();
    ev
}

/// Имена ответа в выражениях: `status`, `headers`, `body`, `duration`, `cookies`.
fn set_response(ev: &mut Eval<'_>, r: &Response, cookies: Value) {
    if let Value::Object(map) = response_value(r, cookies) {
        for (k, v) in map {
            ev.set(&k, v);
        }
    }
}

/// Ответ как значение — то, что возвращает вызов запроса.
fn response_value(r: &Response, cookies: Value) -> Value {
    let mut headers = Map::new();
    for h in &r.headers {
        let name = h.name.to_ascii_lowercase();
        match headers.get_mut(&name) {
            Some(Value::String(prev)) => {
                prev.push_str(", ");
                prev.push_str(&h.value);
            }
            _ => {
                headers.insert(name, Value::String(h.value.clone()));
            }
        }
    }
    let mut out = Map::new();
    out.insert("status".into(), Value::from(r.status));
    out.insert("headers".into(), Value::Object(headers));
    out.insert(
        "body".into(),
        r.json
            .clone()
            .unwrap_or_else(|| Value::String(r.body.clone())),
    );
    out.insert("duration".into(), Value::from(r.duration_ms));
    out.insert("cookies".into(), cookies);
    Value::Object(out)
}

/// `file("./x.png")`: путь относительно файла `.outry` и содержимое.
async fn read_file(
    ev: &mut Eval<'_>,
    src: &Source,
    e: &Expr,
    args: &[Expr],
) -> Result<(String, Vec<u8>)> {
    let [arg] = args else {
        return Err(Error::expr(e.span.text(&src.text), "file() takes a path"));
    };
    let path = value_to_var(&ev.eval(arg).await?);
    let full = src
        .full
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join(&path);
    let bytes = std::fs::read(&full).map_err(|err| Error::from(err).in_file(&full))?;
    Ok((path, bytes))
}

/// Пары `key=value` для query и form: массив повторяет ключ, `null` пропускается.
fn pairs(out: &mut Vec<(String, String)>, key: &str, v: Value) {
    match v {
        Value::Null => {}
        Value::Array(items) => {
            for item in items {
                if !item.is_null() {
                    out.push((key.to_string(), value_to_var(&item)));
                }
            }
        }
        other => out.push((key.to_string(), value_to_var(&other))),
    }
}

/// Percent-encoding: всё, кроме `A-Z a-z 0-9 - . _ ~`. `form` — пробел как `+`.
fn encode(s: &str, form: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            b' ' if form => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn mime(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "json" => "application/json",
        "xml" => "application/xml",
        "txt" => "text/plain; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "csv" => "text/csv",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}
