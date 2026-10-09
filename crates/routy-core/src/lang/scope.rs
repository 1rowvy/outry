//! Области видимости `*.routy`: на что ссылается каждое имя в выражениях. Что не объявлено
//! в файле, то ищется в переменных — это и проверяет `check --env`, и показывает редактор.

use super::ast::*;

/// Имена ответа в `expect`, `save` и `poll`.
pub const RESPONSE: [&str; 5] = ["status", "headers", "body", "duration", "cookies"];

/// Откуда берётся значение имени.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    /// `{id}` в пути запроса.
    PathParam(Span),
    /// Поле `params { … }` запроса или сценария.
    Param(Span),
    /// `let` файла.
    Let(Span),
    /// Шаг сценария `x = …` или его `save`.
    Local(Span),
    /// Параметр `x => …`.
    Lambda,
    /// `status`, `headers`, `body`, `duration`, `cookies`.
    Response,
    /// Имя текущего окружения.
    Env,
    /// Не объявлено в файле: цепочка переменных.
    Var,
}

/// Имя в выражении. `vars["x-y"]` — ссылка на переменную `x-y` со всем выражением как `span`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub name: String,
    pub span: Span,
    pub binding: Binding,
}

#[derive(Default)]
pub(crate) struct Scope {
    names: Vec<(String, Binding)>,
    pub refs: Vec<Ref>,
    pub calls: Vec<(Call, Span)>,
}

impl Scope {
    /// Область файла: `env` и все `let` (значения `let` — в `refs`).
    pub fn file(file: &File) -> Scope {
        let mut scope = Scope::default();
        scope.push("env", Binding::Env);
        for item in &file.items {
            if let Item::Let(l) = item {
                scope.expr(&l.value);
                scope.push(&l.name, Binding::Let(l.span));
            }
        }
        scope
    }

    /// Новая область для элемента: имена файла, без ссылок.
    pub fn child(&self) -> Scope {
        Scope {
            names: self.names.clone(),
            ..Scope::default()
        }
    }

    fn push(&mut self, n: &str, b: Binding) {
        self.names.push((n.to_string(), b));
    }

    fn lookup(&self, n: &str) -> Binding {
        self.names
            .iter()
            .rev()
            .find(|(x, _)| x == n)
            .map_or(Binding::Var, |(_, b)| *b)
    }

    fn reference(&mut self, name: &str, span: Span) {
        let binding = self.lookup(name);
        self.refs.push(Ref {
            name: name.to_string(),
            span,
            binding,
        });
    }

    fn params(&mut self, params: &[Param]) {
        for p in params {
            if let Some(d) = &p.default {
                self.expr(d);
            }
        }
        for p in params {
            self.push(&p.name, Binding::Param(p.span));
        }
    }

    pub fn request(&mut self, r: &Request) {
        for p in &r.target.parts {
            if let TargetPart::Param(n, span) = p {
                self.push(n, Binding::PathParam(*span));
            }
        }
        self.params(&r.fields.params);
        for p in &r.target.parts {
            match p {
                TargetPart::Expr(e) => self.expr(e),
                // Значение `{id}` без `params` и аргумента — из переменных.
                TargetPart::Param(n, span) => {
                    if !r.fields.params.iter().any(|d| &d.name == n) {
                        self.refs.push(Ref {
                            name: n.clone(),
                            span: *span,
                            binding: Binding::PathParam(*span),
                        });
                    }
                }
                TargetPart::Lit(_) => {}
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
            self.push(n, Binding::Response);
        }
        if let Some(p) = &fl.poll {
            self.expr(&p.until);
        }
        fl.expect.iter().for_each(|e| self.expr(e));
        fl.saves.iter().for_each(|s| self.expr(&s.value));
    }

    pub fn flow(&mut self, f: &Flow) {
        self.params(&f.params);
        for step in &f.steps {
            match step {
                Step::Bind { name, value, span } => {
                    self.expr(value);
                    self.push(name, Binding::Local(*span));
                }
                Step::Do(e) => self.expr(e),
                Step::Expect(es, _) => es.iter().for_each(|e| self.expr(e)),
                Step::Save(s) => {
                    self.expr(&s.value);
                    self.push(&s.name, Binding::Local(s.span));
                }
            }
        }
    }

    pub fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Ident(n) => self.reference(n, e.span),
            ExprKind::Index(base, idx) if matches!(&base.kind, ExprKind::Ident(n) if n == "vars" && self.lookup(n) == Binding::Var) => {
                match &idx.kind {
                    ExprKind::Str(parts) => match parts.as_slice() {
                        [StrPart::Lit(n)] => self.refs.push(Ref {
                            name: n.clone(),
                            span: e.span,
                            binding: Binding::Var,
                        }),
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
                self.push(p, Binding::Lambda);
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

/// Все имена файла и на что они ссылаются, в порядке обхода.
pub fn refs(file: &File) -> Vec<Ref> {
    let top = Scope::file(file);
    let mut out = top.refs.clone();
    for item in &file.items {
        let mut scope = top.child();
        match item {
            Item::Request(r) => scope.request(r),
            Item::Flow(f) => scope.flow(f),
            _ => continue,
        }
        out.extend(scope.refs);
    }
    out
}
