//! `routy check --env prod`: то, что зависит от окружения, — без отправки запросов.
//! Вызовы запросов, закрытых `only` для этого окружения, и переменные, которых в нём нет.

use std::collections::BTreeSet;

use super::ast::*;
use super::{Diagnostic, ItemRef, Source, Workspace, distance};

/// Имена ответа в `expect`, `save` и `poll`.
const RESPONSE: [&str; 5] = ["status", "headers", "body", "duration", "cookies"];

impl Workspace {
    /// `has_var` — есть ли переменная в цепочке окружения (`--var`, `ROUTY_*`, `env.toml`, keychain).
    /// Имена из `save` где-либо в проекте считаются определёнными: их задаст прогон.
    pub fn check_env(
        &self,
        env: &str,
        envs: &[String],
        has_var: &mut dyn FnMut(&str) -> bool,
    ) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut saved = BTreeSet::new();
        for s in &self.sources {
            for item in &s.file.items {
                match item {
                    Item::Request(r) => saved.extend(r.fields.saves.iter().map(|s| s.name.clone())),
                    Item::Flow(f) => {
                        for step in &f.steps {
                            if let Step::Save(s) = step {
                                saved.insert(s.name.clone());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut defined = |name: &str| saved.contains(name) || has_var(name);

        for (fi, s) in self.sources.iter().enumerate() {
            let mut scope = Scope::default();
            scope.push("env");
            for item in &s.file.items {
                if let Item::Let(l) = item {
                    let before = scope.missing.len();
                    scope.expr(&l.value);
                    scope.report(s, before, &mut defined, env, &mut out);
                    scope.push(&l.name);
                }
            }
            let file_scope = scope.names.clone();

            for (ii, item) in s.file.items.iter().enumerate() {
                let me = ItemRef { file: fi, item: ii };
                let mut scope = Scope {
                    names: file_scope.clone(),
                    ..Scope::default()
                };
                match item {
                    Item::Request(r) => {
                        if let Some(only) = &r.fields.only {
                            for e in only {
                                if !envs.contains(e) {
                                    let close = envs
                                        .iter()
                                        .filter(|n| distance(n, e) <= 2)
                                        .min_by_key(|n| distance(n, e));
                                    let hint = close
                                        .map(|c| format!(" (did you mean `{c}`?)"))
                                        .unwrap_or_default();
                                    out.push(s.error_at(
                                        r.span.start,
                                        format!("`only` names unknown environment `{e}`{hint}"),
                                    ));
                                }
                            }
                            if !only.iter().any(|e| e == env) {
                                // В этом окружении запрос не уходит — и проверять его нечего.
                                continue;
                            }
                        }
                        if r.target.kind == TargetKind::Path && !defined("base") {
                            out.push(s.error_at(
                                r.target.span.start,
                                format!(
                                    "`base` is not defined in {env} (paths are appended to it)"
                                ),
                            ));
                        }
                        scope.request(r);
                    }
                    Item::Flow(f) => scope.flow(f),
                    _ => continue,
                }
                scope.report(s, 0, &mut defined, env, &mut out);
                self.check_calls(me, &scope.calls, env, &mut defined, &mut out);
            }
        }
        out.sort_by(|a, b| (&a.path, a.line, a.col).cmp(&(&b.path, b.line, b.col)));
        out.dedup_by(|a, b| a.path == b.path && a.line == b.line && a.msg == b.msg);
        out
    }

    /// Вызовы из `me`: закрытые `only` и параметры без значения, которые придут из переменных.
    fn check_calls(
        &self,
        me: ItemRef,
        calls: &[(Call, Span)],
        env: &str,
        defined: &mut dyn FnMut(&str) -> bool,
        out: &mut Vec<Diagnostic>,
    ) {
        let s = &self.sources[me.file];
        for (call, span) in calls {
            let Ok(target) = self.resolve(&call.path) else {
                continue; // это уже ошибка `check()`
            };
            let params: &[Param] = match self.item(target) {
                Item::Request(r) => {
                    if let Some(only) = &r.fields.only {
                        if !only.iter().any(|e| e == env) {
                            out.push(s.error_at(
                                span.start,
                                format!(
                                    "`{}` is limited to {} and cannot be called in {env}",
                                    call.name(),
                                    only.join(", ")
                                ),
                            ));
                            continue;
                        }
                    }
                    &r.fields.params
                }
                Item::Flow(f) => &f.params,
                _ => continue,
            };
            let missing: Vec<String> = self
                .params_of(target)
                .into_iter()
                .filter(|p| !call.args.iter().any(|(a, _)| a == p))
                .filter(|p| !params.iter().any(|d| d.name == *p && d.default.is_some()))
                .filter(|p| !defined(p))
                .collect();
            if !missing.is_empty() {
                out.push(s.error_at(
                    span.start,
                    format!(
                        "`{}` needs {} — pass {} or define {} in {env}",
                        call.name(),
                        missing.join(", "),
                        if missing.len() == 1 { "it" } else { "them" },
                        if missing.len() == 1 {
                            "the variable"
                        } else {
                            "the variables"
                        },
                    ),
                ));
            }
        }
    }
}

/// Свободные имена выражений с учётом областей видимости: что не объявлено, то ищется в переменных.
#[derive(Default)]
struct Scope {
    names: Vec<String>,
    missing: Vec<(String, Span)>,
    calls: Vec<(Call, Span)>,
}

impl Scope {
    fn push(&mut self, n: &str) {
        self.names.push(n.to_string());
    }

    fn has(&self, n: &str) -> bool {
        self.names.iter().any(|x| x == n)
    }

    fn report(
        &mut self,
        s: &Source,
        from: usize,
        defined: &mut dyn FnMut(&str) -> bool,
        env: &str,
        out: &mut Vec<Diagnostic>,
    ) {
        let mut seen = BTreeSet::new();
        for (name, span) in self.missing.drain(from..) {
            if seen.insert(name.clone()) && !defined(&name) {
                out.push(s.error_at(
                    span.start,
                    format!("variable `{name}` is not defined in {env}"),
                ));
            }
        }
    }

    fn params(&mut self, params: &[Param]) {
        for p in params {
            if let Some(d) = &p.default {
                self.expr(d);
            }
        }
        for p in params {
            self.push(&p.name);
        }
    }

    fn request(&mut self, r: &Request) {
        for p in &r.target.parts {
            if let TargetPart::Param(n, _) = p {
                self.push(n);
            }
        }
        self.params(&r.fields.params);
        for p in &r.target.parts {
            if let TargetPart::Expr(e) = p {
                self.expr(e);
            }
        }
        let fl = &r.fields;
        for e in fl.query.iter().chain(&fl.headers) {
            self.expr(&e.value);
        }
        match &fl.body {
            Some(Body::Value(e)) => self.expr(e),
            Some(Body::Form(es) | Body::Multipart(es)) => {
                es.iter().for_each(|e| self.expr(&e.value))
            }
            None => {}
        }
        for n in RESPONSE {
            self.push(n);
        }
        if let Some(p) = &fl.poll {
            self.expr(&p.until);
        }
        fl.expect.iter().for_each(|e| self.expr(e));
        fl.saves.iter().for_each(|s| self.expr(&s.value));
    }

    fn flow(&mut self, f: &Flow) {
        self.params(&f.params);
        for step in &f.steps {
            match step {
                Step::Bind { name, value, .. } => {
                    self.expr(value);
                    self.push(name);
                }
                Step::Do(e) => self.expr(e),
                Step::Expect(es, _) => es.iter().for_each(|e| self.expr(e)),
                Step::Save(s) => {
                    self.expr(&s.value);
                    self.push(&s.name);
                }
            }
        }
    }

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Ident(n) => {
                if !self.has(n) {
                    self.missing.push((n.clone(), e.span));
                }
            }
            ExprKind::Index(base, idx) if matches!(&base.kind, ExprKind::Ident(n) if n == "vars" && !self.has(n)) => {
                match &idx.kind {
                    ExprKind::Str(parts) => match parts.as_slice() {
                        [StrPart::Lit(n)] => self.missing.push((n.clone(), e.span)),
                        [] => {}
                        _ => parts.iter().for_each(|p| {
                            if let StrPart::Expr(x) = p {
                                self.expr(x);
                            }
                        }),
                    },
                    _ => self.expr(idx),
                }
            }
            ExprKind::Lambda(p, body) => {
                self.push(p);
                self.expr(body);
                self.names.pop();
            }
            ExprKind::Call(c) => {
                self.calls.push((c.clone(), e.span));
                c.args.iter().for_each(|(_, x)| self.expr(x));
            }
            ExprKind::Str(parts) => {
                for p in parts {
                    if let StrPart::Expr(x) = p {
                        self.expr(x);
                    }
                }
            }
            ExprKind::Array(items) => items.iter().for_each(|x| self.expr(x)),
            ExprKind::Object(fields) => fields.iter().for_each(|(_, x)| self.expr(x)),
            ExprKind::Member(x, _) | ExprKind::Unary(_, x) | ExprKind::Matches(x, _) => {
                self.expr(x)
            }
            ExprKind::Index(a, b) | ExprKind::Binary(_, a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Builtin(_, args) => args.iter().for_each(|x| self.expr(x)),
            ExprKind::Method(recv, _, args) => {
                self.expr(recv);
                args.iter().for_each(|x| self.expr(x));
            }
            ExprKind::Null | ExprKind::Bool(_) | ExprKind::Num(_) => {}
        }
    }
}
