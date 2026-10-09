//! `routy fmt`: один канонический вид `*.routy`, как `gofmt`. Файл печатается заново из дерева,
//! комментарии расставляются по позициям: над элементом, в конце его строки или в конце блока.
//! Каждый элемент файла после печати разбирается снова и сравнивается с исходным; если смысл
//! или комментарии разошлись, элемент остаётся как был — `fmt` не может испортить файл.

use std::time::Duration;

use super::ast::*;
use super::parse::{RESERVED, is_ident_char, is_ident_start, parse};
use crate::error::Result;

const WIDTH: usize = 80;
const INDENT: &str = "  ";

/// Файл в каноническом виде. Ошибка — только если файл не разбирается.
pub fn format(src: &str) -> Result<String> {
    let src = src.replace("\r\n", "\n");
    let file = parse(&src, None)?;
    let mut p = Printer {
        src: &src,
        comments: &file.comments,
        used: vec![false; file.comments.len()],
        kept: Vec::new(),
    };
    let mut out = p.file(&file);
    out.push('\n');
    Ok(out)
}

/// Как `format`, но ещё и элементы, оставленные как были (для тестов форматтера).
#[cfg(test)]
fn format_kept(src: &str) -> (String, Vec<String>) {
    let file = parse(src, None).unwrap();
    let mut p = Printer {
        src,
        comments: &file.comments,
        used: vec![false; file.comments.len()],
        kept: Vec::new(),
    };
    let out = p.file(&file);
    (out, p.kept)
}

struct Printer<'a> {
    src: &'a str,
    comments: &'a [Comment],
    used: Vec<bool>,
    /// Элементы, которые не удалось напечатать без потерь, — их текст как был.
    kept: Vec<String>,
}

/// Готовый к печати элемент блока: `span` — где он был в исходнике (для комментариев).
struct El {
    span: Span,
    text: String,
    /// Пустая строка перед элементом: `None` — как было в исходнике.
    blank: Option<bool>,
}

impl El {
    fn new(span: Span, text: String) -> El {
        El {
            span,
            text,
            blank: None,
        }
    }
}

fn width(s: &str) -> usize {
    s.chars().count()
}

/// Колонка после `s`, напечатанного с колонки `col`.
fn end_col(col: usize, s: &str) -> usize {
    match s.rfind('\n') {
        Some(i) => width(&s[i + 1..]),
        None => col + width(s),
    }
}

fn ind(level: usize) -> String {
    INDENT.repeat(level)
}

fn is_ident(s: &str) -> bool {
    s.starts_with(is_ident_start) && s.chars().all(is_ident_char)
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    escape_into(&mut out, s, false);
    out.push('"');
    out
}

/// `triple` — для `"""…"""`: переводы строк и кавычки остаются как есть.
fn escape_into(out: &mut String, s: &str, triple: bool) {
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' if !triple => out.push_str("\\\""),
            '\n' if !triple => out.push_str("\\n"),
            '\t' if !triple => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '$' if chars.peek() == Some(&'{') => out.push_str("\\$"),
            c if c.is_control() && c != '\n' && c != '\t' => {
                out.push_str(&format!("\\u{{{:x}}}", c as u32))
            }
            c => out.push(c),
        }
    }
}

fn duration(d: Duration) -> String {
    if d.subsec_nanos() % 1_000_000 != 0 {
        return format!("{}ms", d.as_secs_f64() * 1000.0);
    }
    let ms = d.as_millis();
    for (unit, n) in [("h", 3_600_000), ("m", 60_000), ("s", 1000)] {
        if ms > 0 && ms % n == 0 {
            return format!("{}{unit}", ms / n);
        }
    }
    format!("{ms}ms")
}

/// Приоритет для скобок: чем больше, тем крепче держится.
fn prec(e: &Expr) -> u8 {
    match &e.kind {
        ExprKind::Lambda(..) => 0,
        ExprKind::Binary(op, ..) => op_prec(*op),
        ExprKind::Matches(..) => 3,
        ExprKind::Unary(..) => 8,
        _ => 9,
    }
}

fn op_prec(op: BinOp) -> u8 {
    match op {
        BinOp::Or => 1,
        BinOp::And => 2,
        BinOp::In => 3,
        BinOp::Eq | BinOp::Ne => 4,
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 5,
        BinOp::Add | BinOp::Sub => 6,
        BinOp::Mul | BinOp::Div | BinOp::Rem => 7,
    }
}

fn op_text(op: BinOp) -> &'static str {
    match op {
        BinOp::Or => "||",
        BinOp::And => "&&",
        BinOp::In => "in",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
    }
}

fn paren(s: String, need: bool) -> String {
    if need { format!("({s})") } else { s }
}

/// Порядок полей запроса — как в таблице спецификации.
fn field_rank(key: &str) -> usize {
    [
        "handler",
        "params",
        "only",
        "confirm",
        "timeout",
        "redirects",
        "cache",
        "query",
        "headers",
        "body",
        "poll",
        "expect",
        "save",
    ]
    .iter()
    .position(|k| *k == key || (*k == "body" && matches!(key, "form" | "multipart")))
    .unwrap_or(usize::MAX)
}

impl Printer<'_> {
    fn text(&self, span: Span) -> &str {
        span.text(self.src)
    }

    fn has_comments(&self, span: Span) -> bool {
        self.comments
            .iter()
            .any(|c| c.span.start >= span.start && c.span.start < span.end)
    }

    /// Сколько переводов строки в пробелах прямо перед `pos`.
    fn gap_newlines(&self, pos: usize) -> usize {
        self.src[..pos]
            .chars()
            .rev()
            .take_while(|c| c.is_whitespace())
            .filter(|c| *c == '\n')
            .count()
    }

    fn comment_text(&self, c: &Comment) -> String {
        let t = self.text(c.span);
        if c.block {
            t.to_string()
        } else {
            t.trim_end().to_string()
        }
    }

    // ---- раскладка блоков с комментариями ----

    /// Строки блока на уровне `level`. `els` — в порядке вывода; комментарии из `range`, которые
    /// не внутри элементов, встают над следующим элементом, в конец строки предыдущего
    /// или в конец блока.
    fn layout(&mut self, range: Span, els: &[El], level: usize, comma: bool) -> String {
        let mut by_src: Vec<usize> = (0..els.len()).collect();
        by_src.sort_by_key(|&i| els[i].span.start);
        let mut leading: Vec<Vec<usize>> = vec![Vec::new(); els.len()];
        let mut trailing: Vec<Vec<usize>> = vec![Vec::new(); els.len()];
        let mut dangling = Vec::new();
        for (ci, c) in self.comments.iter().enumerate() {
            let inside_range = c.span.start >= range.start && c.span.start < range.end;
            let inside_el = els
                .iter()
                .any(|e| c.span.start >= e.span.start && c.span.start < e.span.end);
            if self.used[ci] || !inside_range || inside_el {
                continue;
            }
            self.used[ci] = true;
            let prev = by_src
                .iter()
                .rev()
                .find(|&&i| els[i].span.end <= c.span.start);
            if let Some(&p) = prev {
                if !self.src[els[p].span.end..c.span.start].contains('\n') {
                    trailing[p].push(ci);
                    continue;
                }
            }
            match by_src.iter().find(|&&i| els[i].span.start >= c.span.end) {
                Some(&n) => leading[n].push(ci),
                None => dangling.push(ci),
            }
        }

        let pad = ind(level);
        let mut lines: Vec<String> = Vec::new();
        let push = |lines: &mut Vec<String>, blank: bool, text: String| {
            if blank && !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push(text);
        };
        for (i, el) in els.iter().enumerate() {
            let mut first = true;
            for &ci in &leading[i] {
                let c = &self.comments[ci];
                let blank = match el.blank {
                    Some(b) if first => b,
                    _ => self.gap_newlines(c.span.start) >= 2,
                };
                first = false;
                push(&mut lines, blank, format!("{pad}{}", self.comment_text(c)));
            }
            let blank = match el.blank {
                Some(b) if first => b,
                _ => self.gap_newlines(el.span.start) >= 2,
            };
            let mut text = format!("{pad}{}", el.text);
            if comma {
                text.push(',');
            }
            for &ci in &trailing[i] {
                text.push(' ');
                text.push_str(&self.comment_text(&self.comments[ci]));
            }
            push(&mut lines, blank, text);
        }
        for ci in dangling {
            let c = &self.comments[ci];
            let blank = self.gap_newlines(c.span.start) >= 2;
            push(&mut lines, blank, format!("{pad}{}", self.comment_text(c)));
        }
        lines.join("\n")
    }

    /// `{ … }` блока запроса (`params`, `headers`, `expect`): одна строка, если элемент один,
    /// без комментариев и влезает, иначе по элементу на строку.
    fn block(&mut self, range: Span, els: Vec<El>, level: usize, col: usize) -> String {
        if !self.has_comments(range) {
            if els.is_empty() {
                return "{}".into();
            }
            if let [one] = els.as_slice() {
                if !one.text.contains('\n') && col + width(&one.text) + 4 <= WIDTH {
                    return format!("{{ {} }}", one.text);
                }
            }
        }
        let body = self.layout(range, &els, level + 1, false);
        format!("{{\n{body}\n{}}}", ind(level))
    }

    /// Список выражений `{ … }`, `[ … ]`, `( … )`, уже не влезший в строку: по элементу
    /// на строку, с висячей запятой.
    fn list(&mut self, open: &str, close: &str, range: Span, els: Vec<El>, level: usize) -> String {
        if els.is_empty() && !self.has_comments(range) {
            return format!("{open}{close}");
        }
        let body = self.layout(range, &els, level + 1, true);
        format!("{open}\n{body}\n{}{close}", ind(level))
    }

    // ---- файл и элементы ----

    fn file(&mut self, f: &File) -> String {
        // Старый вид: имя из первой строки комментария. Оно переезжает в `Name: METHOD`,
        // а сама строка комментария больше не печатается.
        for item in &f.items {
            if let Item::Request(r) = item
                && let Some(span) = legacy_title(r)
                && let Some(ci) = self.comments.iter().position(|c| c.span == span)
            {
                self.used[ci] = true;
            }
        }
        let mut els = Vec::new();
        let mut prev: Option<&Item> = None;
        for item in &f.items {
            let text = self.item_checked(item);
            // Пустая строка между элементами; подряд идущие `let` (и формы) — как в исходнике.
            let same_kind = matches!(
                (prev, item),
                (Some(Item::Let(_)), Item::Let(_)) | (Some(Item::Shape(_)), Item::Shape(_))
            );
            let blank = if same_kind { None } else { Some(true) };
            els.push(El {
                span: item.span(),
                text,
                blank,
            });
            prev = Some(item);
        }
        self.layout(Span::new(0, self.src.len()), &els, 0, false)
    }

    /// Элемент в каноническом виде — или как был, если печать изменила бы смысл.
    fn item_checked(&mut self, item: &Item) -> String {
        let text = self.item(item);
        let original = self.text(item.span()).to_string();
        if text == original || same_meaning(&original, &text) {
            text
        } else {
            self.kept.push(text);
            original
        }
    }

    fn item(&mut self, item: &Item) -> String {
        match item {
            Item::Let(l) => {
                let head = format!("let {} = ", l.name);
                let v = self.expr(&l.value, 0, width(&head));
                format!("{head}{v}")
            }
            Item::Shape(d) => {
                let head = if matches!(d.shape, Shape::Object(_)) {
                    format!("shape {} ", d.name)
                } else {
                    format!("shape {} = ", d.name)
                };
                let s = self.shape(&d.shape, 0, width(&head));
                format!("{head}{s}")
            }
            Item::Request(r) => self.request(r),
            Item::Flow(f) => self.flow(f),
        }
    }

    fn request(&mut self, r: &Request) -> String {
        let name = match (&r.name, r.name_span.is_some() || legacy_title(r).is_some()) {
            (Some(n), true) => format!("{n}: "),
            _ => String::new(),
        };
        let head = format!("{name}{} {}", r.method, self.text(r.target.span));
        let range = Span::new(r.target.span.end, r.span.end);
        let fl = &r.fields;
        let mut order: Vec<(&str, Span)> = fl.order.clone();
        order.sort_by_key(|(k, _)| field_rank(k));
        if order.is_empty() && !self.has_comments(range) {
            return head;
        }
        let mut saves = fl.saves.iter();
        let mut els = Vec::new();
        for (key, span) in order {
            let text = match key {
                "handler" => format!("handler: {}", fl.handler.clone().unwrap_or_default()),
                "params" => {
                    let b = self.params(&fl.params, span, 1, 2 + 7);
                    format!("params {b}")
                }
                "only" => {
                    let envs: Vec<String> = fl
                        .only
                        .iter()
                        .flatten()
                        .map(|e| if is_ident(e) { e.clone() } else { quote(e) })
                        .collect();
                    format!("only: [{}]", envs.join(", "))
                }
                "confirm" => format!("confirm: {}", fl.confirm),
                "redirects" => format!("redirects: {}", fl.redirects.unwrap_or(true)),
                "timeout" => format!("timeout: {}", duration(fl.timeout.unwrap_or_default())),
                "cache" => format!("cache: {}", duration(fl.cache.unwrap_or_default())),
                "query" => format!("query {}", self.entries(&fl.query, false, span, 1, 2 + 6)),
                "headers" => format!(
                    "headers {}",
                    self.entries(&fl.headers, true, span, 1, 2 + 8)
                ),
                "body" | "form" | "multipart" => match &fl.body {
                    Some(Body::Value(e)) => format!("body {}", self.expr(e, 1, 2 + 5)),
                    Some(Body::Form(es)) => {
                        format!("form {}", self.entries(es, false, span, 1, 2 + 5))
                    }
                    Some(Body::Multipart(es)) => {
                        format!("multipart {}", self.entries(es, false, span, 1, 2 + 10))
                    }
                    None => String::new(),
                },
                "poll" => {
                    let Some(p) = &fl.poll else { continue };
                    let until = self.expr(&p.until, 1, 2 + 5);
                    format!(
                        "poll {until} every {} for {}",
                        duration(p.every),
                        duration(p.limit)
                    )
                }
                "expect" => format!("expect {}", self.checks(&fl.expect, span, 1, 2 + 7)),
                "save" => {
                    let Some(s) = saves.next() else { continue };
                    self.save(s, 1)
                }
                _ => continue,
            };
            els.push(El::new(span, text));
        }
        // Поля переставляются, поэтому пустые строки не из исходника: ими отделены многострочные поля.
        for i in 0..els.len() {
            let multi = |j: usize| els[j].text.contains('\n');
            els[i].blank = Some(i > 0 && (multi(i) || multi(i - 1)));
        }
        let body = self.layout(range, &els, 1, false);
        format!("{head} {{\n{body}\n}}")
    }

    fn save(&mut self, s: &Save, level: usize) -> String {
        let head = format!("save {} = ", s.name);
        let col = level * 2 + width(&head);
        format!("{head}{}", self.expr(&s.value, level, col))
    }

    fn params(&mut self, params: &[Param], range: Span, level: usize, col: usize) -> String {
        let els = params
            .iter()
            .map(|p| {
                let text = match &p.default {
                    Some(d) => {
                        let v = self.expr(d, level + 1, (level + 1) * 2 + width(&p.name) + 2);
                        format!("{}: {v}", p.name)
                    }
                    None => p.name.clone(),
                };
                El::new(p.span, text)
            })
            .collect();
        self.block(range, els, level, col)
    }

    fn entries(
        &mut self,
        entries: &[Entry],
        header: bool,
        range: Span,
        level: usize,
        col: usize,
    ) -> String {
        let els = entries
            .iter()
            .map(|e| {
                let bare = if header {
                    !e.key.is_empty()
                        && e.key
                            .chars()
                            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
                } else {
                    is_ident(&e.key) && !RESERVED.contains(&e.key.as_str())
                };
                let key = if bare { e.key.clone() } else { quote(&e.key) };
                let short =
                    !header && bare && matches!(&e.value.kind, ExprKind::Ident(n) if *n == e.key);
                let text = if short {
                    key
                } else {
                    let v = self.expr(&e.value, level + 1, (level + 1) * 2 + width(&key) + 2);
                    format!("{key}: {v}")
                };
                El::new(e.span, text)
            })
            .collect();
        self.block(range, els, level, col)
    }

    fn checks(&mut self, checks: &[Expr], range: Span, level: usize, col: usize) -> String {
        let els: Vec<El> = checks
            .iter()
            .map(|c| El::new(c.span, self.expr(c, level + 1, (level + 1) * 2)))
            .collect();
        if els.len() > 1 {
            let body = self.layout(range, &els, level + 1, false);
            return format!("{{\n{body}\n{}}}", ind(level));
        }
        self.block(range, els, level, col)
    }

    fn flow(&mut self, f: &Flow) -> String {
        let head = format!("flow {}", f.name);
        let range = Span::new(f.span.start, f.span.end);
        let mut els = Vec::new();
        if let Some(span) = f.params_span {
            let b = self.params(&f.params, span, 1, 2 + 7);
            els.push(El::new(span, format!("params {b}")));
        }
        for step in &f.steps {
            let el = match step {
                Step::Bind { name, value, span } => {
                    let v = self.expr(value, 1, 2 + width(name) + 3);
                    El::new(*span, format!("{name} = {v}"))
                }
                Step::Do(e) => El::new(e.span, self.expr(e, 1, 2)),
                Step::Expect(checks, span) => {
                    let b = self.checks(checks, *span, 1, 2 + 7);
                    El::new(*span, format!("expect {b}"))
                }
                Step::Save(s) => El::new(s.span, self.save(s, 1)),
            };
            els.push(el);
        }
        if els.is_empty() && !self.has_comments(range) {
            return format!("{head} {{}}");
        }
        let body = self.layout(range, &els, 1, false);
        format!("{head} {{\n{body}\n}}")
    }

    // ---- выражения ----

    /// Выражение с колонки `col` на уровне `level`: в одну строку, если влезает, иначе списки
    /// (объекты, массивы, аргументы) разворачиваются по элементу на строку.
    fn expr(&mut self, e: &Expr, level: usize, col: usize) -> String {
        if let Some(f) = self.flat(e) {
            if col + width(&f) <= WIDTH {
                return f;
            }
        }
        self.broken(e, level, col)
    }

    fn child(&mut self, e: &Expr, need_paren: bool, level: usize, col: usize) -> String {
        let col = col + usize::from(need_paren);
        paren(self.expr(e, level, col), need_paren)
    }

    fn broken(&mut self, e: &Expr, level: usize, col: usize) -> String {
        match &e.kind {
            ExprKind::Object(fields) => {
                let els = fields
                    .iter()
                    .map(|(k, v)| {
                        let key = if is_ident(k) { k.clone() } else { quote(k) };
                        let text = if is_ident(k) && matches!(&v.kind, ExprKind::Ident(n) if n == k)
                        {
                            key
                        } else {
                            let val = self.expr(v, level + 1, (level + 1) * 2 + width(&key) + 2);
                            format!("{key}: {val}")
                        };
                        El::new(v.span, text)
                    })
                    .collect();
                self.list("{", "}", e.span, els, level)
            }
            ExprKind::Array(items) => {
                let els = items
                    .iter()
                    .map(|x| El::new(x.span, self.expr(x, level + 1, (level + 1) * 2)))
                    .collect();
                self.list("[", "]", e.span, els, level)
            }
            ExprKind::Call(c) => {
                let head = format!("{}{}", if c.fresh { "fresh " } else { "" }, c.name());
                let els = c
                    .args
                    .iter()
                    .map(|(n, v)| {
                        let text = if matches!(&v.kind, ExprKind::Ident(x) if x == n) {
                            n.clone()
                        } else {
                            let val = self.expr(v, level + 1, (level + 1) * 2 + width(n) + 2);
                            format!("{n}: {val}")
                        };
                        El::new(v.span, text)
                    })
                    .collect();
                let args = self.list("(", ")", e.span, els, level);
                format!("{head}{args}")
            }
            ExprKind::Builtin(name, args) => {
                let els = args
                    .iter()
                    .map(|x| El::new(x.span, self.expr(x, level + 1, (level + 1) * 2)))
                    .collect();
                let args = self.list("(", ")", e.span, els, level);
                format!("{name}{args}")
            }
            ExprKind::Method(recv, name, args) => {
                let r = self.child(recv, prec(recv) < 9, level, col);
                let range = Span::new(recv.span.end, e.span.end);
                let els = args
                    .iter()
                    .map(|x| El::new(x.span, self.expr(x, level + 1, (level + 1) * 2)))
                    .collect();
                let args = self.list("(", ")", range, els, level);
                format!("{r}.{name}{args}")
            }
            ExprKind::Member(base, name) => {
                let b = self.child(base, prec(base) < 9, level, col);
                format!("{b}.{name}")
            }
            ExprKind::Index(base, idx) => {
                let b = self.child(base, prec(base) < 9, level, col);
                let i = self.expr(idx, level, end_col(col, &b) + 1);
                format!("{b}[{i}]")
            }
            ExprKind::Binary(op, l, r) => {
                let p = op_prec(*op);
                let ls = self.child(l, prec(l) < p, level, col);
                let op = op_text(*op);
                let rcol = end_col(col, &ls) + op.len() + 2;
                let rs = self.child(r, prec(r) <= p, level, rcol);
                format!("{ls} {op} {rs}")
            }
            ExprKind::Unary(op, x) => {
                let o = match op {
                    UnOp::Not => "!",
                    UnOp::Neg => "-",
                    UnOp::Typeof => "typeof ",
                };
                let xs = self.child(x, prec(x) < 8, level, col + o.len());
                let sep = if *op == UnOp::Neg && xs.starts_with('-') {
                    " "
                } else {
                    ""
                };
                format!("{o}{sep}{xs}")
            }
            ExprKind::Matches(l, pat) => {
                let ls = self.child(l, prec(l) < 3, level, col);
                let pcol = end_col(col, &ls) + " matches ".len();
                let ps = match pat {
                    Pattern::Regex(_) => self.regex_text(l, e),
                    Pattern::Shape(s) => self.shape(s, level, pcol),
                };
                format!("{ls} matches {ps}")
            }
            ExprKind::Lambda(p, body) => {
                let b = self.expr(body, level, col + width(p) + 4);
                format!("{p} => {b}")
            }
            ExprKind::Str(parts) => self.string(e, parts, level),
            _ => self
                .flat(e)
                .unwrap_or_else(|| self.text(e.span).to_string()),
        }
    }

    /// Одной строкой; `None` — внутри комментарии или многострочный текст.
    fn flat(&self, e: &Expr) -> Option<String> {
        if self.has_comments(e.span) {
            return None;
        }
        let list = |items: &mut dyn Iterator<Item = Option<String>>| -> Option<String> {
            let parts: Option<Vec<String>> = items.collect();
            Some(parts?.join(", "))
        };
        Some(match &e.kind {
            ExprKind::Null => "null".into(),
            ExprKind::Bool(b) => b.to_string(),
            ExprKind::Num(_) => self
                .text(e.span)
                .chars()
                .filter(|c| !matches!(c, '(' | ')') && !c.is_whitespace())
                .collect(),
            ExprKind::Str(parts) => {
                if self.is_triple(e)
                    && parts
                        .iter()
                        .any(|p| matches!(p, StrPart::Lit(s) if s.contains('\n')))
                {
                    return None;
                }
                self.dq_string(parts)?
            }
            ExprKind::Ident(n) => n.clone(),
            ExprKind::Array(items) => {
                format!("[{}]", list(&mut items.iter().map(|x| self.flat(x)))?)
            }
            ExprKind::Object(fields) => {
                if fields.is_empty() {
                    return Some("{}".into());
                }
                let inner = list(&mut fields.iter().map(|(k, v)| {
                    if is_ident(k) && matches!(&v.kind, ExprKind::Ident(n) if n == k) {
                        return Some(k.clone());
                    }
                    let key = if is_ident(k) { k.clone() } else { quote(k) };
                    Some(format!("{key}: {}", self.flat(v)?))
                }))?;
                format!("{{ {inner} }}")
            }
            ExprKind::Member(base, name) => {
                format!("{}.{name}", self.flat_child(base, prec(base) < 9)?)
            }
            ExprKind::Index(base, idx) => {
                format!(
                    "{}[{}]",
                    self.flat_child(base, prec(base) < 9)?,
                    self.flat(idx)?
                )
            }
            ExprKind::Builtin(name, args) => {
                format!("{name}({})", list(&mut args.iter().map(|x| self.flat(x)))?)
            }
            ExprKind::Method(recv, name, args) => format!(
                "{}.{name}({})",
                self.flat_child(recv, prec(recv) < 9)?,
                list(&mut args.iter().map(|x| self.flat(x)))?
            ),
            ExprKind::Call(c) => {
                let args = list(&mut c.args.iter().map(|(n, v)| {
                    if matches!(&v.kind, ExprKind::Ident(x) if x == n) {
                        Some(n.clone())
                    } else {
                        Some(format!("{n}: {}", self.flat(v)?))
                    }
                }))?;
                format!(
                    "{}{}({args})",
                    if c.fresh { "fresh " } else { "" },
                    c.name()
                )
            }
            ExprKind::Lambda(p, body) => format!("{p} => {}", self.flat(body)?),
            ExprKind::Unary(op, x) => {
                let xs = self.flat_child(x, prec(x) < 8)?;
                match op {
                    UnOp::Not => format!("!{xs}"),
                    UnOp::Neg if xs.starts_with('-') => format!("- {xs}"),
                    UnOp::Neg => format!("-{xs}"),
                    UnOp::Typeof => format!("typeof {xs}"),
                }
            }
            ExprKind::Binary(op, l, r) => {
                let p = op_prec(*op);
                format!(
                    "{} {} {}",
                    self.flat_child(l, prec(l) < p)?,
                    op_text(*op),
                    self.flat_child(r, prec(r) <= p)?
                )
            }
            ExprKind::Matches(l, pat) => {
                let ps = match pat {
                    Pattern::Regex(_) => self.regex_text(l, e),
                    Pattern::Shape(s) => self.flat_shape(s)?,
                };
                format!("{} matches {ps}", self.flat_child(l, prec(l) < 3)?)
            }
        })
    }

    fn flat_child(&self, e: &Expr, need_paren: bool) -> Option<String> {
        Some(paren(self.flat(e)?, need_paren))
    }

    fn is_triple(&self, e: &Expr) -> bool {
        self.text(e.span).starts_with("\"\"\"")
    }

    fn dq_string(&self, parts: &[StrPart]) -> Option<String> {
        let mut out = String::from("\"");
        for p in parts {
            match p {
                StrPart::Lit(s) => escape_into(&mut out, s, false),
                StrPart::Expr(x) => out.push_str(&format!("${{{}}}", self.flat(x)?)),
            }
        }
        out.push('"');
        Some(out)
    }

    /// Строка; `"""…"""` с переводами строк печатается блоком с отступом уровня.
    fn string(&self, e: &Expr, parts: &[StrPart], level: usize) -> String {
        let multiline = parts
            .iter()
            .any(|p| matches!(p, StrPart::Lit(s) if s.contains('\n')));
        if !(self.is_triple(e) && multiline) {
            return self
                .dq_string(parts)
                .unwrap_or_else(|| self.text(e.span).to_string());
        }
        let mut content = String::new();
        for p in parts {
            match p {
                StrPart::Lit(s) => {
                    escape_into(&mut content, &s.replace("\"\"\"", "\"\"\\\""), true)
                }
                StrPart::Expr(x) => match self.flat(x) {
                    Some(f) => content.push_str(&format!("${{{f}}}")),
                    None => return self.text(e.span).to_string(),
                },
            }
        }
        let pad = ind(level + 1);
        let mut out = String::from("\"\"\"\n");
        for line in content.split('\n') {
            if !line.is_empty() {
                out.push_str(&pad);
                out.push_str(line);
            }
            out.push('\n');
        }
        out.push_str(&ind(level));
        out.push_str("\"\"\"");
        out
    }

    /// `/re/flags` после `matches` — как в исходнике.
    fn regex_text(&self, lhs: &Expr, e: &Expr) -> String {
        let rest = &self.src[lhs.span.end..e.span.end];
        let Some(at) = rest.find("matches") else {
            return String::new();
        };
        let r = rest[at + "matches".len()..].trim_start();
        let mut end = 1;
        let mut escaped = false;
        for (i, c) in r.char_indices().skip(1) {
            match c {
                '\\' if !escaped => escaped = true,
                '/' if !escaped => {
                    end = i + 1;
                    break;
                }
                _ => escaped = false,
            }
        }
        let flags = r[end..]
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .count();
        r[..end + flags].to_string()
    }

    // ---- формы ----

    fn shape(&self, s: &Shape, level: usize, col: usize) -> String {
        if let Some(f) = self.flat_shape(s) {
            if col + width(&f) <= WIDTH {
                return f;
            }
        }
        match s {
            Shape::Object(fields) => {
                let pad = ind(level + 1);
                let mut out = String::from("{\n");
                for f in fields {
                    let key = self.shape_key(f);
                    let v = self.shape(&f.shape, level + 1, (level + 1) * 2 + width(&key) + 2);
                    out.push_str(&format!("{pad}{key}: {v},\n"));
                }
                out.push_str(&ind(level));
                out.push('}');
                out
            }
            Shape::Array(inner) => format!("[{}]", self.shape(inner, level, col + 1)),
            Shape::Union(alts) => alts
                .iter()
                .map(|a| self.shape(a, level, col))
                .collect::<Vec<_>>()
                .join(" | "),
            other => self.flat_shape(other).unwrap_or_default(),
        }
    }

    fn shape_key(&self, f: &ShapeField) -> String {
        let name = if is_ident(&f.name) {
            f.name.clone()
        } else {
            quote(&f.name)
        };
        if f.optional { format!("{name}?") } else { name }
    }

    fn flat_shape(&self, s: &Shape) -> Option<String> {
        Some(match s {
            Shape::Any => "any".into(),
            Shape::String => "string".into(),
            Shape::Number => "number".into(),
            Shape::Integer => "integer".into(),
            Shape::Boolean => "boolean".into(),
            Shape::Null => "null".into(),
            Shape::Literal(serde_json::Value::String(v)) => quote(v),
            Shape::Literal(v) => v.to_string(),
            Shape::Union(alts) => alts
                .iter()
                .map(|a| self.flat_shape(a))
                .collect::<Option<Vec<_>>>()?
                .join(" | "),
            Shape::Array(inner) => format!("[{}]", self.flat_shape(inner)?),
            Shape::Object(fields) if fields.is_empty() => "{}".into(),
            Shape::Object(fields) => {
                let inner = fields
                    .iter()
                    .map(|f| {
                        Some(format!(
                            "{}: {}",
                            self.shape_key(f),
                            self.flat_shape(&f.shape)?
                        ))
                    })
                    .collect::<Option<Vec<_>>>()?;
                format!("{{ {} }}", inner.join(", "))
            }
            Shape::Named(n, _) => n.clone(),
            Shape::Schema(p) => format!("schema({})", quote(p)),
        })
    }
}

/// Комментарий, из первой строки которого взято имя запроса без `Name:` (старые файлы).
pub(crate) fn legacy_title(r: &Request) -> Option<Span> {
    let from_title = r.name_span.is_none()
        && r.name.is_some()
        && r.name.as_deref()
            == r.doc
                .title
                .as_deref()
                .and_then(super::parse::pascal_case)
                .as_deref();
    from_title.then_some(r.doc.title_span).flatten()
}

/// Тот же элемент и те же комментарии — без учёта позиций и порядка полей.
fn same_meaning(a: &str, b: &str) -> bool {
    match (parse(a, None), parse(b, None)) {
        (Ok(a), Ok(b)) => normalized(&a) == normalized(&b),
        _ => false,
    }
}

fn normalized(f: &File) -> (String, Vec<String>) {
    let items: Vec<Item> = f
        .items
        .iter()
        .cloned()
        .map(|mut i| {
            if let Item::Request(r) = &mut i {
                r.fields.order.clear();
                // Имя проверяется отдельно: элемент разбирается без комментария над ним, а
                // старое имя из комментария печатается как `Name:`.
                r.name = None;
                r.name_span = None;
            }
            if let Item::Flow(fl) = &mut i {
                fl.params_span = None;
            }
            i
        })
        .collect();
    let spans = regex::Regex::new(
        r"Span \{ start: \d+, end: \d+ \}|Some\(Span \{ start: \d+, end: \d+ \}\)",
    )
    .expect("valid regex");
    let debug = spans.replace_all(&format!("{items:?}"), "_").into_owned();
    let mut comments: Vec<String> = f
        .comments
        .iter()
        .map(|c| format!("{}{}", c.block, c.text.trim_end()))
        .collect();
    comments.sort();
    (debug, comments)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(src: &str) -> String {
        let out = format(src).unwrap();
        assert_eq!(format(&out).unwrap(), out, "not idempotent:\n{out}");
        out
    }

    #[test]
    fn canonical_request() {
        let src = r#"
// Create order
// Creates an order.
POST /orders/{shop}   {
  expect { status==201
    body.items.length == 1 }
  save order_id=body.id
  headers {Authorization: 'Bearer ${token}'  , "X-Weird Header": "ok"}
  only:[dev,staging]
  body {"name":'Viktor', email:email, tags:["a","b"],}   // trailing


  timeout: 10000ms
}
GET /health
"#;
        let want = r#"// Creates an order.
CreateOrder: POST /orders/{shop} {
  only: [dev, staging]
  timeout: 10s

  headers {
    Authorization: "Bearer ${token}"
    "X-Weird Header": "ok"
  }

  body { name: "Viktor", email, tags: ["a", "b"] } // trailing

  expect {
    status == 201
    body.items.length == 1
  }

  save order_id = body.id
}

GET /health
"#;
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn long_values_break_and_comments_stay() {
        let src = r#"let shop = "main"
let admin = {email: "admin@example.com", role: "admin", permissions: ["read", "write", "delete"]}
// Login
POST /login {
  body {
    // who
    email: admin.email, password: secret_password_from_somewhere, remember_me_for_a_long_time: true,
    /* block */ extra: [1, 2]
  }
}
"#;
        let want = r#"let shop = "main"
let admin = {
  email: "admin@example.com",
  role: "admin",
  permissions: ["read", "write", "delete"],
}

Login: POST /login {
  body {
    // who
    email: admin.email,
    password: secret_password_from_somewhere,
    remember_me_for_a_long_time: true,
    /* block */
    extra: [1, 2],
  }
}
"#;
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn expressions_keep_meaning() {
        let src = r#"
// X
GET /x {
  expect {
    (a+b)*c==d&&!(x||y)
    -(-n) == typeof(body).length
    body matches /^a\/b$/i
    body matches {id:string, note?: "a"|null, items: [Item]}
    fresh Login().body.token != Login(email).body.token
    body.items.all(i=>i.qty>0) && "x" in ["x"]
  }
  poll body.done every 1500ms for 2m
}
flow Buy {
  params { qty: 1 }
  o = X()
  expect { o.status == 200 }
  save last = o.body.id
}
shape Item = {sku: string} | null
"#;
        let want = r#"X: GET /x {
  poll body.done every 1500ms for 2m

  expect {
    (a + b) * c == d && !(x || y)
    - -n == typeof body.length
    body matches /^a\/b$/i
    body matches { id: string, note?: "a" | null, items: [Item] }
    fresh Login().body.token != Login(email).body.token
    body.items.all(i => i.qty > 0) && "x" in ["x"]
  }
}

flow Buy {
  params { qty: 1 }
  o = X()
  expect { o.status == 200 }
  save last = o.body.id
}

shape Item = { sku: string } | null
"#;
        assert_eq!(fmt(src), want);
    }

    #[test]
    fn explicit_names() {
        // Явное имя остаётся, комментарий над ним — описание.
        let src =
            "// Logs in.\nLogin:   POST /login {\n  expect {status==200}\n}\nHealth: GET /health\n";
        let want = "// Logs in.\nLogin: POST /login {\n  expect { status == 200 }\n}\n\nHealth: GET /health\n";
        assert_eq!(fmt(src), want);
        // Старый вид: имя из первой строки комментария переезжает в `Name:`, остальное остаётся;
        // безымянный запрос из одного `GET` не трогается.
        let src = "// Get user\n// By id.\nGET /users/{id}\n\nGET /health\n";
        assert_eq!(
            fmt(src),
            "// By id.\nGetUser: GET /users/{id}\n\nGET /health\n"
        );
    }

    #[test]
    fn triple_strings_are_reindented() {
        let src = "// T\nPOST /t {\n      body \"\"\"\n          hello\n            ${name}\n          \"\"\"\n}\n";
        let want = "T: POST /t {\n  body \"\"\"\n    hello\n      ${name}\n  \"\"\"\n}\n";
        assert_eq!(fmt(src), want);
    }

    /// Все примеры из спецификации печатаются без отката к исходнику.
    #[test]
    fn spec_examples() {
        let spec = include_str!("../../../../docs/src/content/docs/reference/routy-format.mdx");
        let mut n = 0;
        for block in spec.split("```routy").skip(1) {
            let body = block.split_once('\n').map(|(_, b)| b).unwrap_or_default();
            let code = body.split("```").next().unwrap_or_default();
            if parse(code, None).is_err() {
                continue; // фрагменты вроде одного `query { … }`
            }
            let (out, kept) = format_kept(code);
            assert!(
                kept.is_empty(),
                "kept as is:\n{code}\nprinted:\n{}",
                kept.join("\n")
            );
            let again = format(&out).unwrap();
            assert_eq!(again.trim_end(), out, "not idempotent");
            n += 1;
        }
        assert!(n >= 8, "{n}");
    }

    #[test]
    fn unformattable_item_is_left_as_is() {
        // Комментарий внутри формы не на чем держать — элемент остаётся как был.
        let src = "shape A {\n  id: string, // the id\n}\n";
        assert_eq!(fmt(src), src);
    }
}
