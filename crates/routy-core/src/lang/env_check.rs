//! `routy check --env prod`: то, что зависит от окружения, — без отправки запросов.
//! Вызовы запросов, закрытых `only` для этого окружения, и переменные, которых в нём нет.

use std::collections::BTreeSet;

use super::ast::*;
use super::scope::{Binding, Ref, Scope};
use super::{Diagnostic, ItemRef, Source, Workspace, distance};

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
            let top = Scope::file(&s.file);
            report(s, &top.refs, &mut defined, env, &mut out);

            for (ii, item) in s.file.items.iter().enumerate() {
                let me = ItemRef { file: fi, item: ii };
                let mut scope = top.child();
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
                report(s, &scope.refs, &mut defined, env, &mut out);
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

/// Переменные, которых нет в окружении: по одной ошибке на имя.
fn report(
    s: &Source,
    refs: &[Ref],
    defined: &mut dyn FnMut(&str) -> bool,
    env: &str,
    out: &mut Vec<Diagnostic>,
) {
    let mut seen = BTreeSet::new();
    for r in refs.iter().filter(|r| r.binding == Binding::Var) {
        if seen.insert(r.name.as_str()) && !defined(&r.name) {
            out.push(s.error_at(
                r.span.start,
                format!("variable `{}` is not defined in {env}", r.name),
            ));
        }
    }
}
