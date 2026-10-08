//! Вычисление выражений `*.routy`. Значения — JSON (`serde_json::Value`). Вычисление асинхронное:
//! выражение может вызвать другой запрос (`Login().body.token`), а это HTTP.

use std::future::Future;
use std::pin::Pin;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde_json::{Map, Value};

use super::ast::*;
use super::parse::num_value;
use crate::error::{Error, Result};
use crate::expr::{AssertOutcome, value_to_var};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Всё, что выражение берёт снаружи: переменные, вызовы запросов, именованные формы.
pub trait Host: Send {
    /// Переменная из цепочки (`--var`, `save`, `ROUTY_*`, `env.toml`, keychain).
    fn var(&mut self, name: &str) -> Result<Option<Value>>;
    /// Вызов запроса или сценария; результат — ответ (`status`, `headers`, `body`, `duration`).
    fn call<'a>(
        &'a mut self,
        call: &'a Call,
        args: Vec<(String, Value)>,
    ) -> BoxFuture<'a, Result<Value>>;
    fn shape(&self, name: &str) -> Option<Shape>;
}

pub struct Eval<'h> {
    pub host: &'h mut dyn Host,
    /// Исходник файла — для текста выражений в ошибках.
    pub src: &'h str,
    /// Аргументы, параметры, `let`, имена ответа; ищутся с конца.
    pub locals: Vec<(String, Value)>,
    /// Если `Some`, неизвестные имена собираются сюда (и дают `null`), а не роняют вычисление:
    /// так при подготовке запроса видны сразу все недостающие переменные.
    pub missing: Option<Vec<String>>,
}

impl<'h> Eval<'h> {
    pub fn new(host: &'h mut dyn Host, src: &'h str) -> Self {
        Eval {
            host,
            src,
            locals: Vec::new(),
            missing: None,
        }
    }

    pub fn set(&mut self, name: &str, value: Value) {
        self.locals.push((name.to_string(), value));
    }

    fn fail(&self, e: &Expr, msg: impl Into<String>) -> Error {
        Error::expr(e.span.text(self.src), msg)
    }

    /// Значение имени: локальные → цепочка переменных. Не найдено — ошибка или `missing`.
    pub async fn lookup(&mut self, name: &str) -> Result<Value> {
        if let Some((_, v)) = self.locals.iter().rev().find(|(n, _)| n == name) {
            return Ok(v.clone());
        }
        if let Some(v) = self.host.var(name)? {
            return Ok(v);
        }
        match &mut self.missing {
            Some(missing) => {
                if !missing.iter().any(|m| m == name) {
                    missing.push(name.to_string());
                }
                Ok(Value::Null)
            }
            None => Err(Error::MissingVars(vec![name.to_string()])),
        }
    }

    pub fn eval<'s>(&'s mut self, e: &'s Expr) -> BoxFuture<'s, Result<Value>> {
        Box::pin(async move {
            Ok(match &e.kind {
                ExprKind::Null => Value::Null,
                ExprKind::Bool(b) => Value::Bool(*b),
                ExprKind::Num(n) => num_value(*n),
                ExprKind::Str(parts) => Value::String(self.text(parts).await?),
                ExprKind::Ident(name) => self.lookup(name).await?,
                ExprKind::Array(items) => {
                    let mut out = Vec::with_capacity(items.len());
                    for item in items {
                        out.push(self.eval(item).await?);
                    }
                    Value::Array(out)
                }
                ExprKind::Object(fields) => {
                    let mut out = Map::new();
                    for (k, v) in fields {
                        let v = self.eval(v).await?;
                        out.insert(k.clone(), v);
                    }
                    Value::Object(out)
                }
                ExprKind::Member(base, name) => {
                    let v = self.eval(base).await?;
                    member(&v, name, is_headers(base))
                }
                ExprKind::Index(base, index) => {
                    if matches!(&base.kind, ExprKind::Ident(n) if n == "vars")
                        && !self.locals.iter().any(|(n, _)| n == "vars")
                    {
                        let key = self.eval(index).await?;
                        let Value::String(name) = key else {
                            return Err(self.fail(index, "vars[…] takes a string"));
                        };
                        return self.lookup(&name).await;
                    }
                    let v = self.eval(base).await?;
                    let i = self.eval(index).await?;
                    index_value(&v, &i, is_headers(base))?
                }
                ExprKind::Builtin(name, args) => {
                    let mut values = Vec::with_capacity(args.len());
                    for a in args {
                        if matches!(a.kind, ExprKind::Lambda(..)) {
                            return Err(
                                self.fail(a, "`=>` is only allowed in .any/.all/.map/.filter")
                            );
                        }
                        values.push(self.eval(a).await?);
                    }
                    builtin(name, values).map_err(|m| self.fail(e, m))?
                }
                ExprKind::Method(recv, name, args) => {
                    let v = self.eval(recv).await?;
                    self.method(e, v, name, args).await?
                }
                ExprKind::Call(call) => {
                    let mut args = Vec::with_capacity(call.args.len());
                    for (name, a) in &call.args {
                        args.push((name.clone(), self.eval(a).await?));
                    }
                    self.host.call(call, args).await?
                }
                ExprKind::Lambda(..) => {
                    return Err(self.fail(e, "`=>` is only allowed in .any/.all/.map/.filter"));
                }
                ExprKind::Unary(op, inner) => {
                    let v = self.eval(inner).await?;
                    match (op, v) {
                        (UnOp::Not, Value::Bool(b)) => Value::Bool(!b),
                        (UnOp::Neg, Value::Number(n)) => num_value(-n.as_f64().unwrap_or(0.0)),
                        (UnOp::Typeof, v) => Value::String(type_name(&v).into()),
                        (UnOp::Not, v) => {
                            return Err(
                                self.fail(e, format!("`!` needs a boolean, got {}", show(&v)))
                            );
                        }
                        (UnOp::Neg, v) => {
                            return Err(
                                self.fail(e, format!("`-` needs a number, got {}", show(&v)))
                            );
                        }
                    }
                }
                ExprKind::Binary(op @ (BinOp::And | BinOp::Or), l, r) => {
                    let lv = self.eval(l).await?;
                    let Value::Bool(lb) = lv else {
                        return Err(self.fail(l, format!("expected a boolean, got {}", show(&lv))));
                    };
                    if (*op == BinOp::And) != lb {
                        return Ok(Value::Bool(lb));
                    }
                    let rv = self.eval(r).await?;
                    let Value::Bool(rb) = rv else {
                        return Err(self.fail(r, format!("expected a boolean, got {}", show(&rv))));
                    };
                    Value::Bool(rb)
                }
                ExprKind::Binary(op, l, r) => {
                    let lv = self.eval(l).await?;
                    let rv = self.eval(r).await?;
                    binary(*op, &lv, &rv).map_err(|m| self.fail(e, m))?
                }
                ExprKind::Matches(v, pattern) => {
                    let v = self.eval(v).await?;
                    Value::Bool(self.mismatch(&v, pattern, "")?.is_none())
                }
            })
        })
    }

    async fn text(&mut self, parts: &[StrPart]) -> Result<String> {
        let mut out = String::new();
        for p in parts {
            match p {
                StrPart::Lit(s) => out.push_str(s),
                StrPart::Expr(e) => out.push_str(&value_to_var(&self.eval(e).await?)),
            }
        }
        Ok(out)
    }

    async fn method(&mut self, e: &Expr, v: Value, name: &str, args: &[Expr]) -> Result<Value> {
        if let ("any" | "all" | "map" | "filter", Value::Array(items)) = (name, &v) {
            let [arg] = args else {
                return Err(self.fail(e, format!(".{name}() takes one argument: x => …")));
            };
            let ExprKind::Lambda(param, body) = &arg.kind else {
                return Err(self.fail(arg, format!(".{name}() takes a function: x => …")));
            };
            let mut out = Vec::new();
            for item in items {
                self.locals.push((param.clone(), item.clone()));
                let r = self.eval(body).await;
                self.locals.pop();
                let r = r?;
                let keep = match (name, &r) {
                    ("map", _) => {
                        out.push(r);
                        continue;
                    }
                    (_, Value::Bool(b)) => *b,
                    _ => {
                        return Err(
                            self.fail(body, format!("expected a boolean, got {}", show(&r)))
                        );
                    }
                };
                match name {
                    "any" if keep => return Ok(Value::Bool(true)),
                    "all" if !keep => return Ok(Value::Bool(false)),
                    "filter" if keep => out.push(item.clone()),
                    _ => {}
                }
            }
            return Ok(match name {
                "any" => Value::Bool(false),
                "all" => Value::Bool(true),
                _ => Value::Array(out),
            });
        }
        let mut values = Vec::with_capacity(args.len());
        for a in args {
            values.push(self.eval(a).await?);
        }
        method(&v, name, &values).map_err(|m| self.fail(e, m))
    }

    /// Первое несовпадение с образцом: `None` — значение подходит.
    pub fn mismatch(&self, v: &Value, pattern: &Pattern, path: &str) -> Result<Option<String>> {
        match pattern {
            Pattern::Regex(re) => Ok(match v {
                Value::String(s) if re.is_match(s) => None,
                other => Some(format!("{} does not match /{}/", show(other), re.as_str())),
            }),
            Pattern::Shape(s) => self.shape_mismatch(v, s, path, 0),
        }
    }

    fn shape_mismatch(
        &self,
        v: &Value,
        s: &Shape,
        path: &str,
        depth: usize,
    ) -> Result<Option<String>> {
        if depth > 64 {
            return Err(Error::Run(format!("{path}: shape nesting is too deep")));
        }
        let at = |p: &str| {
            if p.is_empty() {
                String::new()
            } else {
                format!("{p}: ")
            }
        };
        let ok = match s {
            Shape::Any => true,
            Shape::String => v.is_string(),
            Shape::Number => v.is_number(),
            Shape::Integer => v.as_f64().is_some_and(|n| n.fract() == 0.0),
            Shape::Boolean => v.is_boolean(),
            Shape::Null => v.is_null(),
            Shape::Literal(l) => deep_eq(v, l),
            Shape::Union(alts) => {
                for a in alts {
                    if self.shape_mismatch(v, a, path, depth + 1)?.is_none() {
                        return Ok(None);
                    }
                }
                false
            }
            Shape::Array(inner) => {
                let Value::Array(items) = v else {
                    return Ok(Some(format!(
                        "{}expected an array, got {}",
                        at(path),
                        show(v)
                    )));
                };
                for (i, item) in items.iter().enumerate() {
                    let p = format!("{path}[{i}]");
                    if let Some(m) = self.shape_mismatch(item, inner, &p, depth + 1)? {
                        return Ok(Some(m));
                    }
                }
                true
            }
            Shape::Object(fields) => {
                let Value::Object(map) = v else {
                    return Ok(Some(format!(
                        "{}expected an object, got {}",
                        at(path),
                        show(v)
                    )));
                };
                for f in fields {
                    let p = if path.is_empty() {
                        f.name.clone()
                    } else {
                        format!("{path}.{}", f.name)
                    };
                    match map.get(&f.name) {
                        None if f.optional => {}
                        None => return Ok(Some(format!("{p}: missing"))),
                        Some(fv) => {
                            if let Some(m) = self.shape_mismatch(fv, &f.shape, &p, depth + 1)? {
                                return Ok(Some(m));
                            }
                        }
                    }
                }
                true
            }
            Shape::Named(name, _) => {
                let Some(shape) = self.host.shape(name) else {
                    return Err(Error::Run(format!("unknown shape `{name}`")));
                };
                return self.shape_mismatch(v, &shape, path, depth + 1);
            }
            Shape::Schema(p) => {
                return Err(Error::Run(format!(
                    "schema(\"{p}\"): JSON Schema is not supported yet"
                )));
            }
        };
        Ok((!ok).then(|| {
            format!(
                "{}expected {}, got {}",
                at(path),
                describe_shape(s),
                show(v)
            )
        }))
    }

    /// Проверка из `expect`: не роняет прогон, а возвращает результат с пояснением.
    pub async fn check(&mut self, e: &Expr) -> AssertOutcome {
        let source = e.span.text(self.src).to_string();
        let outcome = |passed: bool, actual: Option<Value>, detail: Option<String>| AssertOutcome {
            source: source.clone(),
            passed,
            actual,
            detail,
        };
        match &e.kind {
            ExprKind::Binary(op, l, r) if op.is_comparison() => {
                let lv = match self.eval(l).await {
                    Ok(v) => v,
                    Err(err) => return outcome(false, None, Some(err.to_string())),
                };
                let rv = match self.eval(r).await {
                    Ok(v) => v,
                    Err(err) => return outcome(false, Some(lv), Some(err.to_string())),
                };
                match binary(*op, &lv, &rv) {
                    Ok(Value::Bool(true)) => outcome(true, Some(lv), None),
                    Ok(_) => {
                        let mut sides = Vec::new();
                        for (side, v) in [(l, &lv), (r, &rv)] {
                            if !is_literal(side) {
                                sides.push(format!("{} is {}", side.span.text(self.src), show(v)));
                            }
                        }
                        outcome(false, Some(lv), Some(sides.join(", ")))
                    }
                    Err(m) => outcome(false, Some(lv), Some(m)),
                }
            }
            ExprKind::Matches(v, pattern) => {
                let val = match self.eval(v).await {
                    Ok(val) => val,
                    Err(err) => return outcome(false, None, Some(err.to_string())),
                };
                let path = v.span.text(self.src).to_string();
                match self.mismatch(&val, pattern, &path) {
                    Ok(None) => outcome(true, Some(val), None),
                    Ok(Some(m)) => outcome(false, Some(val), Some(m)),
                    Err(err) => outcome(false, Some(val), Some(err.to_string())),
                }
            }
            _ => match self.eval(e).await {
                Ok(Value::Bool(b)) => outcome(b, Some(Value::Bool(b)), None),
                Ok(v) => {
                    let detail = format!("not a boolean: {}", show(&v));
                    outcome(false, Some(v), Some(detail))
                }
                Err(err) => outcome(false, None, Some(err.to_string())),
            },
        }
    }
}

fn is_literal(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Null | ExprKind::Bool(_) | ExprKind::Num(_) => true,
        ExprKind::Str(parts) => parts.iter().all(|p| matches!(p, StrPart::Lit(_))),
        ExprKind::Array(items) => items.iter().all(is_literal),
        ExprKind::Object(fields) => fields.iter().all(|(_, v)| is_literal(v)),
        _ => false,
    }
}

/// `headers.x` и `x.headers.y`: имена заголовков без учёта регистра.
fn is_headers(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Ident(n) | ExprKind::Member(_, n) => n == "headers",
        _ => false,
    }
}

fn member(v: &Value, name: &str, headers: bool) -> Value {
    match v {
        Value::Object(map) => {
            if let Some(x) = map.get(name) {
                return x.clone();
            }
            if headers {
                if let Some((_, x)) = map.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)) {
                    return x.clone();
                }
            }
            match name {
                "keys" => Value::Array(map.keys().map(|k| Value::String(k.clone())).collect()),
                "values" => Value::Array(map.values().cloned().collect()),
                _ => Value::Null,
            }
        }
        Value::Array(items) => match name {
            "length" => Value::from(items.len()),
            "first" => items.first().cloned().unwrap_or(Value::Null),
            "last" => items.last().cloned().unwrap_or(Value::Null),
            _ => Value::Null,
        },
        Value::String(s) if name == "length" => Value::from(s.chars().count()),
        _ => Value::Null,
    }
}

fn index_value(v: &Value, i: &Value, headers: bool) -> Result<Value, Error> {
    Ok(match (v, i) {
        (Value::Array(items), Value::Number(n)) => {
            let n = n.as_f64().unwrap_or(0.0);
            if n.fract() != 0.0 {
                return Err(Error::Run(format!("index must be an integer, got {n}")));
            }
            let n = n as i64;
            let idx = if n < 0 { items.len() as i64 + n } else { n };
            usize::try_from(idx)
                .ok()
                .and_then(|i| items.get(i))
                .cloned()
                .unwrap_or(Value::Null)
        }
        (Value::Object(_), Value::String(k)) => member(v, k, headers),
        (Value::Null, _) => Value::Null,
        (Value::Array(_) | Value::Object(_), other) => {
            return Err(Error::Run(format!("cannot index with {}", show(other))));
        }
        _ => Value::Null,
    })
}

pub fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Значение для сообщений: компактный JSON, длинное обрезается.
pub fn show(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 80 {
        let cut: String = s.chars().take(77).collect();
        format!("{cut}...")
    } else {
        s
    }
}

fn describe_shape(s: &Shape) -> String {
    match s {
        Shape::Any => "any".into(),
        Shape::String => "string".into(),
        Shape::Number => "number".into(),
        Shape::Integer => "integer".into(),
        Shape::Boolean => "boolean".into(),
        Shape::Null => "null".into(),
        Shape::Literal(v) => v.to_string(),
        Shape::Union(alts) => alts
            .iter()
            .map(describe_shape)
            .collect::<Vec<_>>()
            .join(" | "),
        Shape::Array(inner) => format!("[{}]", describe_shape(inner)),
        Shape::Object(_) => "an object".into(),
        Shape::Named(n, _) => n.clone(),
        Shape::Schema(p) => format!("schema(\"{p}\")"),
    }
}

/// Равенство без приведения типов; числа сравниваются по значению (`1 == 1.0`).
pub fn deep_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| deep_eq(a, b))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| deep_eq(v, w)))
        }
        _ => a == b,
    }
}

fn binary(op: BinOp, l: &Value, r: &Value) -> Result<Value, String> {
    use std::cmp::Ordering;
    let num = |v: &Value| v.as_f64();
    Ok(match op {
        BinOp::Eq => Value::Bool(deep_eq(l, r)),
        BinOp::Ne => Value::Bool(!deep_eq(l, r)),
        BinOp::In => Value::Bool(match r {
            Value::Array(items) => items.iter().any(|x| deep_eq(x, l)),
            Value::Object(map) => match l {
                Value::String(k) => map.contains_key(k),
                other => {
                    return Err(format!(
                        "`in` an object needs a string key, got {}",
                        show(other)
                    ));
                }
            },
            Value::String(s) => match l {
                Value::String(sub) => s.contains(sub.as_str()),
                other => return Err(format!("`in` a string needs a string, got {}", show(other))),
            },
            other => {
                return Err(format!(
                    "`in` needs an array, object or string, got {}",
                    show(other)
                ));
            }
        }),
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let ord = match (l, r) {
                (Value::Number(_), Value::Number(_)) => num(l).partial_cmp(&num(r)),
                (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
                _ => {
                    return Err(format!("cannot compare {} with {}", show(l), show(r)));
                }
            };
            Value::Bool(match op {
                BinOp::Lt => ord == Some(Ordering::Less),
                BinOp::Le => matches!(ord, Some(Ordering::Less | Ordering::Equal)),
                BinOp::Gt => ord == Some(Ordering::Greater),
                _ => matches!(ord, Some(Ordering::Greater | Ordering::Equal)),
            })
        }
        BinOp::Add => match (l, r) {
            (Value::Number(_), Value::Number(_)) => {
                num_value(num(l).unwrap_or(0.0) + num(r).unwrap_or(0.0))
            }
            (Value::String(_), _) | (_, Value::String(_)) => {
                Value::String(value_to_var(l) + &value_to_var(r))
            }
            (Value::Array(a), Value::Array(b)) => {
                Value::Array(a.iter().chain(b).cloned().collect())
            }
            _ => return Err(format!("cannot add {} and {}", show(l), show(r))),
        },
        BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
            let (Some(a), Some(b)) = (num(l), num(r)) else {
                return Err(format!(
                    "arithmetic needs numbers, got {} and {}",
                    show(l),
                    show(r)
                ));
            };
            if matches!(op, BinOp::Div | BinOp::Rem) && b == 0.0 {
                return Err("division by zero".into());
            }
            num_value(match op {
                BinOp::Sub => a - b,
                BinOp::Mul => a * b,
                BinOp::Div => a / b,
                _ => a % b,
            })
        }
        BinOp::And | BinOp::Or => unreachable!("short-circuit in eval"),
    })
}

fn method(v: &Value, name: &str, args: &[Value]) -> Result<Value, String> {
    let str_arg = |i: usize| match args.get(i) {
        Some(Value::String(s)) => Ok(s.as_str()),
        Some(other) => Err(format!(".{name}() takes a string, got {}", show(other))),
        None => Err(format!(".{name}() takes a string argument")),
    };
    let arity = |n: usize| {
        if args.len() == n {
            Ok(())
        } else {
            Err(format!(
                ".{name}() takes {n} argument(s), got {}",
                args.len()
            ))
        }
    };
    Ok(match (v, name) {
        (Value::String(s), "startsWith") => Value::Bool(s.starts_with(str_arg(0)?)),
        (Value::String(s), "endsWith") => Value::Bool(s.ends_with(str_arg(0)?)),
        (Value::String(s), "contains") => Value::Bool(s.contains(str_arg(0)?)),
        (Value::String(s), "lower") => {
            arity(0)?;
            Value::String(s.to_lowercase())
        }
        (Value::String(s), "upper") => {
            arity(0)?;
            Value::String(s.to_uppercase())
        }
        (Value::String(s), "trim") => {
            arity(0)?;
            Value::String(s.trim().to_string())
        }
        (Value::String(s), "split") => Value::Array(
            s.split(str_arg(0)?)
                .map(|p| Value::String(p.to_string()))
                .collect(),
        ),
        (Value::Array(items), "contains") => {
            arity(1)?;
            Value::Bool(items.iter().any(|x| deep_eq(x, &args[0])))
        }
        (Value::Object(map), "has") => Value::Bool(map.contains_key(str_arg(0)?)),
        (Value::Null, _) => {
            return Err(format!("cannot call .{name}() on null (missing value?)"));
        }
        (v, _) => {
            let available = match v {
                Value::String(_) => "startsWith, endsWith, contains, lower, upper, trim, split",
                Value::Array(_) => "contains, any, all, map, filter",
                Value::Object(_) => "has",
                _ => "none",
            };
            return Err(format!(
                "{} has no method .{name}() (available: {available})",
                type_name(v)
            ));
        }
    })
}

fn builtin(name: &str, args: Vec<Value>) -> Result<Value, String> {
    let arity = |n: usize| {
        if args.len() == n {
            Ok(())
        } else {
            Err(format!(
                "{name}() takes {n} argument(s), got {}",
                args.len()
            ))
        }
    };
    let int = |v: &Value| {
        v.as_f64()
            .filter(|n| n.fract() == 0.0)
            .map(|n| n as i64)
            .ok_or_else(|| format!("{name}() takes integers, got {}", show(v)))
    };
    Ok(match name {
        "uuid" => {
            arity(0)?;
            Value::String(crate::dynamic::uuid())
        }
        "now" => {
            arity(0)?;
            Value::from(unix_now())
        }
        "nowIso" => {
            arity(0)?;
            Value::String(rfc3339(unix_now()))
        }
        "randomInt" => {
            arity(2)?;
            let (min, max) = (int(&args[0])?, int(&args[1])?);
            if min > max {
                return Err("randomInt(min, max): min is greater than max".into());
            }
            Value::from(fastrand::i64(min..=max))
        }
        "randomString" => {
            arity(1)?;
            let n = usize::try_from(int(&args[0])?).map_err(|_| "randomString(n): n < 0")?;
            Value::String((0..n).map(|_| fastrand::alphanumeric()).collect())
        }
        "number" => {
            arity(1)?;
            match &args[0] {
                Value::Number(_) => args[0].clone(),
                Value::String(s) => s
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite())
                    .map(num_value)
                    .ok_or_else(|| format!("number(): `{s}` is not a number"))?,
                other => return Err(format!("number(): cannot convert {}", show(other))),
            }
        }
        "string" => {
            arity(1)?;
            Value::String(value_to_var(&args[0]))
        }
        "json" => {
            arity(1)?;
            Value::String(args[0].to_string())
        }
        "base64" => {
            arity(1)?;
            Value::String(base64::engine::general_purpose::STANDARD.encode(value_to_var(&args[0])))
        }
        "unbase64" => {
            arity(1)?;
            let Value::String(s) = &args[0] else {
                return Err("unbase64() takes a string".into());
            };
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(s.trim())
                .map_err(|e| format!("unbase64(): {e}"))?;
            Value::String(String::from_utf8(bytes).map_err(|_| "unbase64(): not UTF-8 text")?)
        }
        "file" => return Err("file() can only be used as `body` or a multipart value".into()),
        "schema" => return Err("schema() can only be used after `matches`".into()),
        other => {
            return Err(format!(
                "unknown function `{other}` (available: uuid, now, nowIso, randomInt, \
                 randomString, number, string, json, base64, unbase64, file)"
            ));
        }
    })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// `2026-10-08T12:00:00Z` без зависимостей: дата по алгоритму days-to-civil (H. Hinnant).
fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_dates() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_791_460_800), "2026-10-08T12:00:00Z");
    }
}
