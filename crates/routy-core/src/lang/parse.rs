//! Разбор `*.routy`. Лексер работает по требованию: путь после метода, регулярное выражение после
//! `matches`, имя заголовка и имя после `.` лексируются по-своему, поэтому отдельного потока
//! токенов нет — парсер читает следующий токен из текста в нужном режиме.

use std::time::Duration;

use super::ast::*;
use crate::error::{Error, Result};

/// Разбирает файл. `stem` — имя файла без расширения: им называется единственный запрос файла,
/// если над ним нет комментария.
pub fn parse(src: &str, stem: Option<&str>) -> Result<File> {
    let mut p = Parser::new(src, 0, src.len());
    let mut items = Vec::new();
    loop {
        p.skip_newlines()?;
        let (tok, span) = p.peek()?;
        let doc = p.doc_above(span.start);
        let item = match tok {
            Tok::Eof => break,
            Tok::Ident(w) if w == "let" => Item::Let(p.let_decl()?),
            Tok::Ident(w) if w == "shape" => Item::Shape(p.shape_decl()?),
            Tok::Ident(w) if w == "flow" => Item::Flow(p.flow(doc)?),
            Tok::Ident(w) if is_method(&w) => Item::Request(p.request(doc)?),
            other => {
                return p.err(
                    span.start,
                    format!(
                        "expected a request (`GET /path`), `flow`, `shape` or `let`, got {}",
                        describe(&other)
                    ),
                );
            }
        };
        items.push(item);
        let (tok, span) = p.peek()?;
        if !matches!(tok, Tok::Newline | Tok::Eof) {
            return p.err(
                span.start,
                format!("expected a new line, got {}", describe(&tok)),
            );
        }
    }

    let mut requests = items.iter_mut().filter_map(|i| match i {
        Item::Request(r) => Some(r),
        _ => None,
    });
    if let (Some(only), None) = (requests.next(), requests.next()) {
        if only.name.is_none() && only.doc.title.is_none() {
            only.name = stem.and_then(pascal_case);
        }
    }
    Ok(File {
        items,
        comments: p.comments,
    })
}

/// `Create order` → `CreateOrder`, `1-login` → `Login`. `None`, если букв нет.
pub fn pascal_case(s: &str) -> Option<String> {
    let mut out = String::new();
    for word in s.split(|c: char| !c.is_alphanumeric()) {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    let out = out.trim_start_matches(|c: char| !c.is_alphabetic());
    (!out.is_empty()).then(|| out.to_string())
}

const RESERVED: &[&str] = &[
    "let", "shape", "flow", "fresh", "save", "expect", "poll", "every", "for", "matches", "in",
    "typeof",
];

const TYPES: &[&str] = &["string", "number", "integer", "boolean", "any"];

fn is_method(w: &str) -> bool {
    w.len() >= 2 && w.bytes().all(|b| b.is_ascii_uppercase())
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn starts_upper(s: &str) -> bool {
    s.chars().next().is_some_and(char::is_uppercase)
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Num(f64),
    Dur(Duration),
    Str(Vec<RawPart>),
    P(&'static str),
    Newline,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
enum RawPart {
    Lit(String),
    /// Диапазон выражения внутри `${…}`.
    Interp(Span),
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Ident(s) => format!("`{s}`"),
        Tok::Num(n) => format!("`{n}`"),
        Tok::Dur(_) => "a duration".into(),
        Tok::Str(_) => "a string".into(),
        Tok::P(p) => format!("`{p}`"),
        Tok::Newline => "end of line".into(),
        Tok::Eof => "end of file".into(),
    }
}

const PUNCTS: &[&str] = &[
    "=>", "==", "!=", "<=", ">=", "&&", "||", "{", "}", "[", "]", "(", ")", ",", ":", ".", "=",
    "<", ">", "!", "+", "-", "*", "/", "%", "|", "?",
];

/// 1-based строка и столбец (в символах) байтовой позиции.
pub fn line_col(src: &str, pos: usize) -> (usize, usize) {
    let before = &src[..pos.min(src.len())];
    let line = before.matches('\n').count() + 1;
    let col = before
        .rfind('\n')
        .map_or(before, |i| &before[i + 1..])
        .chars()
        .count()
        + 1;
    (line, col)
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
    end: usize,
    comments: Vec<Comment>,
    /// Перед выражением стоял `fresh`: его забирает первый вызов запроса в цепочке.
    fresh: bool,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str, pos: usize, end: usize) -> Self {
        Parser {
            src,
            pos,
            end,
            comments: Vec::new(),
            fresh: false,
        }
    }

    fn err<T>(&self, at: usize, msg: impl Into<String>) -> Result<T> {
        let (line, col) = line_col(self.src, at);
        Err(Error::Syntax {
            line,
            col,
            msg: msg.into(),
        })
    }

    fn rest(&self) -> &'a str {
        &self.src[self.pos..self.end]
    }

    fn skip_spaces(&mut self) {
        let r = self.rest();
        self.pos += r.len() - r.trim_start_matches([' ', '\t', '\r', '\u{feff}']).len();
    }

    fn skip_trivia(&mut self) -> Result<()> {
        loop {
            self.skip_spaces();
            let r = self.rest();
            if let Some(text) = r.strip_prefix("//") {
                let len = text.find('\n').unwrap_or(text.len());
                self.comments.push(Comment {
                    span: Span::new(self.pos, self.pos + 2 + len),
                    text: text[..len].trim_end().to_string(),
                    block: false,
                });
                self.pos += 2 + len;
            } else if let Some(text) = r.strip_prefix("/*") {
                let Some(len) = text.find("*/") else {
                    return self.err(self.pos, "unclosed `/*` comment");
                };
                self.comments.push(Comment {
                    span: Span::new(self.pos, self.pos + 4 + len),
                    text: text[..len].to_string(),
                    block: true,
                });
                self.pos += 4 + len;
            } else {
                return Ok(());
            }
        }
    }

    fn lex(&mut self) -> Result<(Tok, Span)> {
        self.skip_trivia()?;
        let start = self.pos;
        let r = self.rest();
        let Some(c) = r.chars().next() else {
            return Ok((Tok::Eof, Span::new(start, start)));
        };
        let tok = if c == '\n' {
            self.pos += 1;
            Tok::Newline
        } else if c.is_ascii_digit() {
            self.lex_number()?
        } else if is_ident_start(c) {
            let len = r.find(|c| !is_ident_char(c)).unwrap_or(r.len());
            self.pos += len;
            Tok::Ident(r[..len].to_string())
        } else if c == '"' || c == '\'' {
            Tok::Str(self.lex_string()?)
        } else if let Some(p) = PUNCTS.iter().find(|p| r.starts_with(**p)) {
            self.pos += p.len();
            Tok::P(p)
        } else {
            return self.err(start, format!("unexpected character `{c}`"));
        };
        Ok((tok, Span::new(start, self.pos)))
    }

    fn peek(&mut self) -> Result<(Tok, Span)> {
        let (pos, n) = (self.pos, self.comments.len());
        let t = self.lex();
        self.pos = pos;
        self.comments.truncate(n);
        t
    }

    fn peek2(&mut self) -> Result<(Tok, Tok)> {
        let (pos, n) = (self.pos, self.comments.len());
        let a = self.lex().map(|t| t.0);
        let b = self.lex().map(|t| t.0);
        self.pos = pos;
        self.comments.truncate(n);
        Ok((a?, b?))
    }

    fn next(&mut self) -> Result<(Tok, Span)> {
        self.lex()
    }

    fn peek_is(&mut self, p: &str) -> Result<bool> {
        Ok(matches!(self.peek()?.0, Tok::P(x) if x == p))
    }

    fn peek_word(&mut self, w: &str) -> Result<bool> {
        Ok(matches!(self.peek()?.0, Tok::Ident(x) if x == w))
    }

    fn eat(&mut self, p: &str) -> Result<bool> {
        let yes = self.peek_is(p)?;
        if yes {
            self.next()?;
        }
        Ok(yes)
    }

    fn expect(&mut self, p: &str) -> Result<Span> {
        let (tok, span) = self.next()?;
        match tok {
            Tok::P(x) if x == p => Ok(span),
            other => self.err(
                span.start,
                format!("expected `{p}`, got {}", describe(&other)),
            ),
        }
    }

    fn expect_word(&mut self, w: &str) -> Result<()> {
        let (tok, span) = self.next()?;
        match tok {
            Tok::Ident(x) if x == w => Ok(()),
            other => self.err(
                span.start,
                format!("expected `{w}`, got {}", describe(&other)),
            ),
        }
    }

    fn skip_newlines(&mut self) -> Result<()> {
        while matches!(self.peek()?.0, Tok::Newline) {
            self.next()?;
        }
        Ok(())
    }

    fn ident(&mut self, what: &str) -> Result<(String, Span)> {
        let (tok, span) = self.next()?;
        match tok {
            Tok::Ident(name) if !RESERVED.contains(&name.as_str()) => Ok((name, span)),
            other => self.err(
                span.start,
                format!("expected {what}, got {}", describe(&other)),
            ),
        }
    }

    fn lex_number(&mut self) -> Result<Tok> {
        let r = self.rest();
        let b = r.as_bytes();
        let digits = |mut i: usize| {
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            i
        };
        let mut i = digits(0);
        if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
            i = digits(i + 1);
        }
        if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
            let mut j = i + 1;
            if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                j += 1;
            }
            if j < b.len() && b[j].is_ascii_digit() {
                i = digits(j);
            }
        }
        let num = &r[..i];
        let start = self.pos;
        self.pos += i;
        for (suffix, unit) in [("ms", 0.001), ("s", 1.0), ("m", 60.0), ("h", 3600.0)] {
            let after = &r[i..];
            if after.starts_with(suffix) && !after[suffix.len()..].starts_with(is_ident_char) {
                self.pos += suffix.len();
                let n: f64 = num.parse().unwrap_or_default();
                return Ok(Tok::Dur(Duration::from_secs_f64(n * unit)));
            }
        }
        if r[i..].starts_with(is_ident_char) {
            return self.err(start, format!("invalid number `{}`", &r[..i + 1]));
        }
        num.parse()
            .map(Tok::Num)
            .or_else(|_| self.err(start, format!("invalid number `{num}`")))
    }

    fn next_char(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn lex_string(&mut self) -> Result<Vec<RawPart>> {
        if self.rest().starts_with("\"\"\"") {
            return self.lex_triple();
        }
        let open = self.pos;
        let quote = self.next_char().unwrap_or('"');
        self.pos += 1;
        let mut parts = Vec::new();
        let mut lit = String::new();
        loop {
            match self.next_char() {
                None | Some('\n') => return self.err(open, "unclosed string"),
                Some(c) if c == quote => {
                    self.pos += 1;
                    break;
                }
                Some('\\') => lit.push(self.lex_escape()?),
                Some('$') if self.rest().starts_with("${") => {
                    self.lex_interp(&mut lit, &mut parts)?
                }
                Some(c) => {
                    lit.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
        if !lit.is_empty() || parts.is_empty() {
            parts.push(RawPart::Lit(lit));
        }
        Ok(parts)
    }

    fn lex_interp(&mut self, lit: &mut String, parts: &mut Vec<RawPart>) -> Result<()> {
        if !lit.is_empty() {
            parts.push(RawPart::Lit(std::mem::take(lit)));
        }
        let start = self.pos + 2;
        let end = self.interp_end(start)?;
        parts.push(RawPart::Interp(Span::new(start, end)));
        self.pos = end + 1;
        Ok(())
    }

    /// Позиция `}`, закрывающей `${`, начатую перед `start`.
    fn interp_end(&self, start: usize) -> Result<usize> {
        let mut depth = 0usize;
        let mut i = start;
        while i < self.end {
            let c = self.src[i..].chars().next().unwrap_or(' ');
            match c {
                '{' => depth += 1,
                '}' if depth == 0 => return Ok(i),
                '}' => depth -= 1,
                '"' | '\'' => {
                    let mut sub = Parser::new(self.src, i, self.end);
                    sub.lex_string()?;
                    i = sub.pos;
                    continue;
                }
                _ => {}
            }
            i += c.len_utf8();
        }
        self.err(start - 2, "unclosed `${`")
    }

    fn lex_escape(&mut self) -> Result<char> {
        let at = self.pos;
        self.pos += 1;
        let Some(c) = self.next_char() else {
            return self.err(at, "unfinished escape");
        };
        self.pos += c.len_utf8();
        Ok(match c {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            'b' => '\u{8}',
            'f' => '\u{c}',
            '"' | '\'' | '\\' | '/' | '$' => c,
            'u' => return self.lex_unicode(at),
            other => return self.err(at, format!("unknown escape `\\{other}`")),
        })
    }

    /// `\u{1F600}` или `\uXXXX` (как в JSON, с суррогатными парами).
    fn lex_unicode(&mut self, at: usize) -> Result<char> {
        let r = self.rest();
        let code = if let Some(inner) = r.strip_prefix('{') {
            let Some(end) = inner.find('}') else {
                return self.err(at, "unclosed `\\u{`");
            };
            self.pos += end + 2;
            u32::from_str_radix(&inner[..end], 16).ok()
        } else {
            let hex4 = |s: &str| {
                s.get(..4)
                    .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
            };
            let hi = hex4(r);
            self.pos += 4;
            match hi {
                Some(hi @ 0xD800..=0xDBFF) => {
                    let lo = self.rest().strip_prefix("\\u").and_then(hex4);
                    match lo {
                        Some(lo @ 0xDC00..=0xDFFF) => {
                            self.pos += 6;
                            Some(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00))
                        }
                        _ => None,
                    }
                }
                other => other,
            }
        };
        code.and_then(char::from_u32)
            .map_or_else(|| self.err(at, "invalid unicode escape"), Ok)
    }

    /// `"""…"""`: общий отступ строк убирается, первая пустая строка и последняя строка из
    /// одних пробелов — тоже.
    fn lex_triple(&mut self) -> Result<Vec<RawPart>> {
        let open = self.pos;
        let start = self.pos + 3;
        let mut i = start;
        loop {
            let r = &self.src[i..self.end];
            if r.is_empty() {
                return self.err(open, "unclosed `\"\"\"`");
            }
            if r.starts_with("\"\"\"") {
                break;
            }
            if r.starts_with("${") {
                i = self.interp_end(i + 2)? + 1;
            } else {
                let mut chars = r.chars();
                let c = chars.next().unwrap_or(' ');
                i += c.len_utf8();
                if c == '\\' {
                    i += chars.next().map_or(0, char::len_utf8);
                }
            }
        }
        let end = i;
        let raw = &self.src[start..end];
        let first_nl = raw
            .strip_prefix("\r\n")
            .or_else(|| raw.strip_prefix('\n'))
            .map(|r| raw.len() - r.len());
        let indent = raw[first_nl.unwrap_or(0)..]
            .split('\n')
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.len() - l.trim_start_matches([' ', '\t']).len())
            .min()
            .unwrap_or(0);

        self.pos = start + first_nl.unwrap_or(0);
        let mut at_line_start = first_nl.is_some();
        let mut parts = Vec::new();
        let mut lit = String::new();
        while self.pos < end {
            if at_line_start {
                at_line_start = false;
                let r = &self.src[self.pos..end];
                let ws = r.len() - r.trim_start_matches([' ', '\t']).len();
                self.pos += ws.min(indent);
                continue;
            }
            let r = &self.src[self.pos..end];
            if r.starts_with("${") {
                self.lex_interp(&mut lit, &mut parts)?;
            } else if r.starts_with('\\') {
                lit.push(self.lex_escape()?);
            } else if r.starts_with("\r\n") {
                self.pos += 1;
            } else {
                let c = r.chars().next().unwrap_or(' ');
                self.pos += c.len_utf8();
                lit.push(c);
                at_line_start = c == '\n';
            }
        }
        // Последняя строка из одних пробелов перед `"""` — не часть текста.
        if let Some(nl) = lit.rfind('\n') {
            if lit[nl + 1..].trim().is_empty() {
                lit.truncate(nl);
            }
        }
        if !lit.is_empty() || parts.is_empty() {
            parts.push(RawPart::Lit(lit));
        }
        self.pos = end + 3;
        Ok(parts)
    }

    fn sub_expr(&self, span: Span) -> Result<Expr> {
        let mut p = Parser::new(self.src, span.start, span.end);
        p.skip_newlines()?;
        let e = p.expr()?;
        p.skip_newlines()?;
        let (tok, at) = p.peek()?;
        if tok != Tok::Eof {
            return p.err(
                at.start,
                format!("unexpected {} in `${{…}}`", describe(&tok)),
            );
        }
        Ok(e)
    }

    fn str_parts(&self, raw: Vec<RawPart>) -> Result<Vec<StrPart>> {
        raw.into_iter()
            .map(|p| match p {
                RawPart::Lit(s) => Ok(StrPart::Lit(s)),
                RawPart::Interp(span) => self.sub_expr(span).map(StrPart::Expr),
            })
            .collect()
    }

    fn plain(&self, raw: Vec<RawPart>, span: Span) -> Result<String> {
        match raw.as_slice() {
            [RawPart::Lit(s)] => Ok(s.clone()),
            _ => self.err(span.start, "`${…}` is not allowed here"),
        }
    }

    /// Комментарий `//` из подряд идущих строк прямо над `pos`.
    fn doc_above(&self, pos: usize) -> Doc {
        let (mut want, _) = line_col(self.src, pos);
        let mut lines = Vec::new();
        for c in self.comments.iter().rev() {
            if c.block || c.span.end > pos {
                break;
            }
            let (line, _) = line_col(self.src, c.span.start);
            let line_start = self.src[..c.span.start].rfind('\n').map_or(0, |i| i + 1);
            let own_line = self.src[line_start..c.span.start].trim().is_empty();
            if line + 1 != want || !own_line {
                break;
            }
            lines.push(c.text.strip_prefix(' ').unwrap_or(&c.text).to_string());
            want = line;
        }
        lines.reverse();
        let mut lines = lines.into_iter();
        Doc {
            title: lines.next().filter(|t| !t.trim().is_empty()),
            description: lines.collect::<Vec<_>>().join("\n").trim().to_string(),
        }
    }

    // ---- элементы файла ----

    fn let_decl(&mut self) -> Result<Let> {
        let (_, start) = self.next()?;
        let (name, _) = self.ident("a name after `let`")?;
        self.expect("=")?;
        self.skip_newlines()?;
        let value = self.expr()?;
        Ok(Let {
            span: start.to(value.span),
            name,
            value,
        })
    }

    fn shape_decl(&mut self) -> Result<ShapeDecl> {
        let (_, start) = self.next()?;
        let (name, at) = self.ident("a shape name")?;
        if !starts_upper(&name) {
            return self.err(at.start, "shape names start with an upper-case letter");
        }
        if !self.peek_is("{")? {
            self.expect("=")?;
            self.skip_newlines()?;
        }
        let shape = self.shape()?;
        Ok(ShapeDecl {
            span: Span::new(start.start, self.pos),
            name,
            shape,
        })
    }

    fn flow(&mut self, doc: Doc) -> Result<Flow> {
        let (_, start) = self.next()?;
        let (name, at) = self.ident("a flow name")?;
        if !starts_upper(&name) {
            return self.err(at.start, "flow names start with an upper-case letter");
        }
        let mut params = None;
        let mut steps = Vec::new();
        self.block(|p| {
            let (tok, span) = p.peek()?;
            let word = match &tok {
                Tok::Ident(w) => w.as_str(),
                _ => "",
            };
            match word {
                "params" => {
                    if params.is_some() {
                        return p.err(span.start, "duplicate `params`");
                    }
                    p.next()?;
                    params = Some(p.params()?);
                }
                "expect" => {
                    p.next()?;
                    steps.push(Step::Expect(p.checks()?));
                }
                "save" => steps.push(Step::Save(p.save()?)),
                _ => {
                    if let (Tok::Ident(name), Tok::P("=")) = p.peek2()? {
                        p.next()?;
                        p.next()?;
                        p.skip_newlines()?;
                        let value = p.expr()?;
                        steps.push(Step::Bind {
                            name,
                            span: span.to(value.span),
                            value,
                        });
                    } else {
                        let e = p.expr()?;
                        if !matches!(e.kind, ExprKind::Call(_)) {
                            return p.err(
                                e.span.start,
                                "a step is a call, `name = …`, `expect { … }` or `save`",
                            );
                        }
                        steps.push(Step::Do(e));
                    }
                }
            }
            Ok(())
        })?;
        Ok(Flow {
            span: Span::new(start.start, self.pos),
            doc,
            name,
            params: params.unwrap_or_default(),
            steps,
        })
    }

    fn request(&mut self, doc: Doc) -> Result<Request> {
        let (tok, start) = self.next()?;
        let Tok::Ident(method) = tok else {
            unreachable!("request() is called on a method")
        };
        let target = self.target()?;
        let mut fields = Fields::default();
        if self.peek_is("{")? {
            self.block(|p| p.field(&mut fields))?;
        }
        Ok(Request {
            span: Span::new(start.start, self.pos),
            name: doc.title.as_deref().and_then(pascal_case),
            doc,
            method,
            target,
            fields,
        })
    }

    /// `{ элемент (, | перевод строки) … }`.
    fn block(&mut self, mut item: impl FnMut(&mut Self) -> Result<()>) -> Result<Span> {
        let open = self.expect("{")?;
        loop {
            while matches!(self.peek()?.0, Tok::Newline | Tok::P(",")) {
                self.next()?;
            }
            if self.eat("}")? {
                return Ok(Span::new(open.start, self.pos));
            }
            if self.peek()?.0 == Tok::Eof {
                return self.err(open.start, "unclosed `{`");
            }
            item(self)?;
            let (tok, span) = self.peek()?;
            match tok {
                Tok::Newline | Tok::P(",") | Tok::P("}") => {}
                Tok::Eof => return self.err(open.start, "unclosed `{`"),
                Tok::P("=") => {
                    return self.err(
                        span.start,
                        "expected `,` or a new line, got `=` (use `==` to compare)",
                    );
                }
                other => {
                    return self.err(
                        span.start,
                        format!("expected `,` or a new line, got {}", describe(&other)),
                    );
                }
            }
        }
    }

    /// Адрес запроса — одно «слово» без пробелов (внутри `{…}` пробелы можно) или строка.
    fn target(&mut self) -> Result<Target> {
        self.skip_spaces();
        let start = self.pos;
        if matches!(self.next_char(), Some('"' | '\'')) {
            let raw = self.lex_string()?;
            let parts = self
                .str_parts(raw)?
                .into_iter()
                .map(|p| match p {
                    StrPart::Lit(s) => TargetPart::Lit(s),
                    StrPart::Expr(e) => TargetPart::Expr(e),
                })
                .collect();
            return Ok(Target {
                kind: TargetKind::Str,
                parts,
                span: Span::new(start, self.pos),
            });
        }
        let mut depth = 0usize;
        let mut len = 0;
        for c in self.rest().chars() {
            match c {
                '{' => depth += 1,
                '}' => depth = depth.saturating_sub(1),
                c if c.is_whitespace() && depth == 0 => break,
                '\n' => break,
                _ => {}
            }
            len += c.len_utf8();
        }
        let text = &self.src[start..start + len];
        if text.contains("{{") {
            return self.err(
                start,
                "`{{var}}` is not valid in .routy: use `{var}` in a path or `${var}`",
            );
        }
        let kind = if text.starts_with('/') {
            TargetKind::Path
        } else if text.starts_with("http://") || text.starts_with("https://") {
            TargetKind::Url
        } else if text.is_empty() {
            return self.err(start, "expected a path or URL after the method");
        } else {
            return self.err(
                start,
                format!("expected `/path`, `https://…` or a string, got `{text}`"),
            );
        };

        let mut parts = Vec::new();
        let mut lit = String::new();
        let mut i = start;
        let end = start + len;
        while i < end {
            let r = &self.src[i..end];
            if r.starts_with("${") {
                let close = self.interp_end(i + 2)?;
                if close >= end {
                    return self.err(i, "unclosed `${`");
                }
                if !lit.is_empty() {
                    parts.push(TargetPart::Lit(std::mem::take(&mut lit)));
                }
                parts.push(TargetPart::Expr(self.sub_expr(Span::new(i + 2, close))?));
                i = close + 1;
            } else if let Some(inner) = r.strip_prefix('{') {
                let Some(close) = inner.find('}') else {
                    return self.err(i, "unclosed `{` in path");
                };
                let name = &inner[..close];
                if name.is_empty()
                    || !name.starts_with(is_ident_start)
                    || !name.chars().all(is_ident_char)
                {
                    return self.err(i, format!("invalid path parameter `{{{name}}}`"));
                }
                if !lit.is_empty() {
                    parts.push(TargetPart::Lit(std::mem::take(&mut lit)));
                }
                parts.push(TargetPart::Param(
                    name.to_string(),
                    Span::new(i, i + close + 2),
                ));
                i += close + 2;
            } else {
                let c = r.chars().next().unwrap_or(' ');
                lit.push(c);
                i += c.len_utf8();
            }
        }
        if !lit.is_empty() {
            parts.push(TargetPart::Lit(lit));
        }
        self.pos = end;
        Ok(Target {
            kind,
            parts,
            span: Span::new(start, end),
        })
    }

    fn field(&mut self, f: &mut Fields) -> Result<()> {
        if self.peek_word("save")? {
            let save = self.save()?;
            f.order.push(("save", save.span));
            f.saves.push(save);
            return Ok(());
        }
        let (tok, at) = self.next()?;
        let Tok::Ident(name) = tok else {
            return self.err(
                at.start,
                format!("expected a field name, got {}", describe(&tok)),
            );
        };
        let key: &'static str = match name.as_str() {
            "handler" => "handler",
            "params" => "params",
            "only" => "only",
            "confirm" => "confirm",
            "timeout" => "timeout",
            "redirects" => "redirects",
            "cache" => "cache",
            "query" => "query",
            "headers" => "headers",
            "body" => "body",
            "form" => "form",
            "multipart" => "multipart",
            "poll" => "poll",
            "expect" => "expect",
            other => {
                return self.err(
                    at.start,
                    format!(
                        "unknown field `{other}` (expected handler, params, only, confirm, timeout, \
                         redirects, cache, query, headers, body, form, multipart, poll, expect or save)"
                    ),
                );
            }
        };
        if f.order.iter().any(|(k, _)| *k == key) {
            return self.err(at.start, format!("duplicate `{key}`"));
        }
        if matches!(key, "body" | "form" | "multipart") && f.body.is_some() {
            return self.err(
                at.start,
                "a request has at most one of `body`, `form`, `multipart`",
            );
        }
        match key {
            "handler" => {
                self.expect(":")?;
                let mut path = vec![self.ident("a handler name")?.0];
                while self.eat(".")? {
                    path.push(self.ident("a handler name")?.0);
                }
                f.handler = Some(path.join("."));
            }
            "params" => f.params = self.params()?,
            "only" => {
                self.expect(":")?;
                let mut envs = Vec::new();
                self.list("[", "]", |p| {
                    let (tok, span) = p.next()?;
                    match tok {
                        Tok::Ident(e) => envs.push(e),
                        Tok::Str(raw) => envs.push(p.plain(raw, span)?),
                        other => {
                            return p.err(
                                span.start,
                                format!("expected an environment name, got {}", describe(&other)),
                            );
                        }
                    }
                    Ok(())
                })?;
                f.only = Some(envs);
            }
            "confirm" => {
                self.expect(":")?;
                f.confirm = self.boolean()?;
            }
            "redirects" => {
                self.expect(":")?;
                f.redirects = Some(self.boolean()?);
            }
            "timeout" => {
                self.expect(":")?;
                f.timeout = Some(self.duration()?);
            }
            "cache" => {
                self.expect(":")?;
                f.cache = Some(self.duration()?);
            }
            "query" => f.query = self.entries(false)?,
            "headers" => f.headers = self.entries(true)?,
            "body" => f.body = Some(Body::Value(self.expr()?)),
            "form" => f.body = Some(Body::Form(self.entries(false)?)),
            "multipart" => f.body = Some(Body::Multipart(self.entries(false)?)),
            "poll" => {
                let until = self.expr()?;
                self.expect_word("every")?;
                let every = self.duration()?;
                self.expect_word("for")?;
                let limit = self.duration()?;
                f.poll = Some(Poll {
                    span: Span::new(until.span.start, self.pos),
                    until,
                    every,
                    limit,
                });
            }
            "expect" => f.expect = self.checks()?,
            _ => unreachable!(),
        }
        f.order.push((key, Span::new(at.start, self.pos)));
        Ok(())
    }

    fn checks(&mut self) -> Result<Vec<Expr>> {
        let mut out = Vec::new();
        self.block(|p| {
            out.push(p.expr()?);
            Ok(())
        })?;
        Ok(out)
    }

    fn save(&mut self) -> Result<Save> {
        let (_, start) = self.next()?;
        let (name, _) = self.ident("a variable name after `save`")?;
        self.expect("=")?;
        self.skip_newlines()?;
        let value = self.expr()?;
        Ok(Save {
            span: start.to(value.span),
            name,
            value,
        })
    }

    fn params(&mut self) -> Result<Vec<Param>> {
        let mut out: Vec<Param> = Vec::new();
        self.block(|p| {
            let (name, span) = p.ident("a parameter name")?;
            if out.iter().any(|x| x.name == name) {
                return p.err(span.start, format!("duplicate parameter `{name}`"));
            }
            let default = if p.eat(":")? {
                p.skip_newlines()?;
                Some(p.expr()?)
            } else {
                None
            };
            out.push(Param {
                name,
                default,
                span: Span::new(span.start, p.pos),
            });
            Ok(())
        })?;
        Ok(out)
    }

    /// `key: value` для query/form/multipart (`header = false`) и заголовков.
    fn entries(&mut self, header: bool) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        self.block(|p| {
            p.skip_trivia()?;
            let start = p.pos;
            let key = if matches!(p.next_char(), Some('"' | '\'')) {
                let raw = p.lex_string()?;
                p.plain(raw, Span::new(start, p.pos))?
            } else if header {
                let r = p.rest();
                let len = r
                    .find(|c: char| !(c.is_alphanumeric() || matches!(c, '-' | '_' | '.')))
                    .unwrap_or(r.len());
                if len == 0 {
                    return p.err(start, "expected a header name");
                }
                p.pos += len;
                r[..len].to_string()
            } else {
                p.ident("a key")?.0
            };
            let key_span = Span::new(start, p.pos);
            let value = if p.eat(":")? {
                p.skip_newlines()?;
                p.expr()?
            } else if !header && key.chars().all(is_ident_char) {
                // Сокращение `{ page }` — то же, что `page: page`.
                Expr {
                    kind: ExprKind::Ident(key.clone()),
                    span: key_span,
                }
            } else {
                return p.err(p.pos, format!("expected `:` after `{key}`"));
            };
            out.push(Entry {
                key,
                span: key_span.to(value.span),
                value,
            });
            Ok(())
        })?;
        Ok(out)
    }

    fn list(
        &mut self,
        open: &str,
        close: &str,
        mut item: impl FnMut(&mut Self) -> Result<()>,
    ) -> Result<()> {
        let at = self.expect(open)?;
        loop {
            while matches!(self.peek()?.0, Tok::Newline | Tok::P(",")) {
                self.next()?;
            }
            if self.eat(close)? {
                return Ok(());
            }
            if self.peek()?.0 == Tok::Eof {
                return self.err(at.start, format!("unclosed `{open}`"));
            }
            item(self)?;
            let (tok, span) = self.peek()?;
            match tok {
                Tok::Newline | Tok::P(",") => {}
                Tok::P(x) if x == close => {}
                Tok::Eof => return self.err(at.start, format!("unclosed `{open}`")),
                other => {
                    return self.err(
                        span.start,
                        format!(
                            "expected `,`, a new line or `{close}`, got {}",
                            describe(&other)
                        ),
                    );
                }
            }
        }
    }

    fn boolean(&mut self) -> Result<bool> {
        let (tok, span) = self.next()?;
        match tok {
            Tok::Ident(w) if w == "true" => Ok(true),
            Tok::Ident(w) if w == "false" => Ok(false),
            other => self.err(
                span.start,
                format!("expected true or false, got {}", describe(&other)),
            ),
        }
    }

    fn duration(&mut self) -> Result<Duration> {
        let (tok, span) = self.next()?;
        match tok {
            Tok::Dur(d) => Ok(d),
            other => self.err(
                span.start,
                format!(
                    "expected a duration like 500ms, 1s, 2m, got {}",
                    describe(&other)
                ),
            ),
        }
    }

    // ---- выражения ----

    fn expr(&mut self) -> Result<Expr> {
        self.binary(0)
    }

    fn binop(&mut self, level: u8) -> Result<Option<BinOp>> {
        let (tok, _) = self.peek()?;
        Ok(match (level, tok) {
            (0, Tok::P("||")) => Some(BinOp::Or),
            (1, Tok::P("&&")) => Some(BinOp::And),
            (2, Tok::Ident(w)) if w == "in" => Some(BinOp::In),
            (3, Tok::P("==")) => Some(BinOp::Eq),
            (3, Tok::P("!=")) => Some(BinOp::Ne),
            (4, Tok::P("<")) => Some(BinOp::Lt),
            (4, Tok::P("<=")) => Some(BinOp::Le),
            (4, Tok::P(">")) => Some(BinOp::Gt),
            (4, Tok::P(">=")) => Some(BinOp::Ge),
            (5, Tok::P("+")) => Some(BinOp::Add),
            (5, Tok::P("-")) => Some(BinOp::Sub),
            (6, Tok::P("*")) => Some(BinOp::Mul),
            (6, Tok::P("/")) => Some(BinOp::Div),
            (6, Tok::P("%")) => Some(BinOp::Rem),
            _ => None,
        })
    }

    fn binary(&mut self, level: u8) -> Result<Expr> {
        if level > 6 {
            return self.unary();
        }
        let mut lhs = self.binary(level + 1)?;
        loop {
            if level == 2 && self.peek_word("matches")? {
                self.next()?;
                let pattern = self.pattern()?;
                lhs = Expr {
                    span: Span::new(lhs.span.start, self.pos),
                    kind: ExprKind::Matches(Box::new(lhs), pattern),
                };
                continue;
            }
            let Some(op) = self.binop(level)? else {
                return Ok(lhs);
            };
            self.next()?;
            self.skip_newlines()?;
            let rhs = self.binary(level + 1)?;
            lhs = Expr {
                span: lhs.span.to(rhs.span),
                kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
            };
        }
    }

    fn unary(&mut self) -> Result<Expr> {
        let (tok, span) = self.peek()?;
        let op = match tok {
            Tok::P("!") => UnOp::Not,
            Tok::P("-") => UnOp::Neg,
            Tok::Ident(w) if w == "typeof" => UnOp::Typeof,
            Tok::Ident(w) if w == "fresh" => {
                self.next()?;
                self.fresh = true;
                let e = self.postfix()?;
                if std::mem::take(&mut self.fresh) {
                    return self.err(e.span.start, "`fresh` must be followed by a request call");
                }
                return Ok(Expr {
                    kind: e.kind,
                    span: span.to(e.span),
                });
            }
            _ => return self.postfix(),
        };
        self.next()?;
        let e = self.unary()?;
        let span = span.to(e.span);
        Ok(match (op, e.kind) {
            (UnOp::Neg, ExprKind::Num(n)) => Expr {
                kind: ExprKind::Num(-n),
                span,
            },
            (op, kind) => Expr {
                kind: ExprKind::Unary(op, Box::new(Expr { kind, span: e.span })),
                span,
            },
        })
    }

    /// Имя после `.`: дефис внутри имени допустим (`headers.content-type`).
    fn member_name(&mut self) -> Result<(String, Span)> {
        self.skip_spaces();
        let start = self.pos;
        let r = self.rest();
        if !r.starts_with(is_ident_start) {
            return self.err(start, "expected a name after `.`");
        }
        let mut len = 0;
        let mut chars = r.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            let ok = is_ident_char(c)
                || (c == '-' && chars.peek().is_some_and(|&(_, n)| is_ident_char(n)));
            if !ok {
                break;
            }
            len = i + c.len_utf8();
        }
        self.pos += len;
        Ok((r[..len].to_string(), Span::new(start, self.pos)))
    }

    fn postfix(&mut self) -> Result<Expr> {
        let mut e = self.primary()?;
        loop {
            let (tok, _) = self.peek()?;
            match tok {
                Tok::P(".") => {
                    self.next()?;
                    let (name, _) = self.member_name()?;
                    if self.peek_is("(")? {
                        e = match ident_path(&e) {
                            Some(mut path) if starts_upper(&name) => {
                                path.push(name);
                                let fresh = std::mem::take(&mut self.fresh);
                                let args = self.call_args()?;
                                Expr {
                                    span: Span::new(e.span.start, self.pos),
                                    kind: ExprKind::Call(Call { path, args, fresh }),
                                }
                            }
                            _ => {
                                let args = self.method_args()?;
                                Expr {
                                    span: Span::new(e.span.start, self.pos),
                                    kind: ExprKind::Method(Box::new(e), name, args),
                                }
                            }
                        };
                    } else {
                        e = Expr {
                            span: Span::new(e.span.start, self.pos),
                            kind: ExprKind::Member(Box::new(e), name),
                        };
                    }
                }
                Tok::P("[") => {
                    self.next()?;
                    self.skip_newlines()?;
                    let index = self.expr()?;
                    self.skip_newlines()?;
                    let close = self.expect("]")?;
                    e = Expr {
                        span: e.span.to(close),
                        kind: ExprKind::Index(Box::new(e), Box::new(index)),
                    };
                }
                Tok::P("(") => {
                    let ExprKind::Ident(name) = &e.kind else {
                        return self.err(
                            e.span.start,
                            "only requests, flows and built-in functions can be called",
                        );
                    };
                    let name = name.clone();
                    e = if starts_upper(&name) {
                        let fresh = std::mem::take(&mut self.fresh);
                        let args = self.call_args()?;
                        Expr {
                            span: Span::new(e.span.start, self.pos),
                            kind: ExprKind::Call(Call {
                                path: vec![name],
                                args,
                                fresh,
                            }),
                        }
                    } else {
                        let args = self.method_args()?;
                        Expr {
                            span: Span::new(e.span.start, self.pos),
                            kind: ExprKind::Builtin(name, args),
                        }
                    };
                }
                _ => return Ok(e),
            }
        }
    }

    /// Именованные аргументы вызова запроса: `(email: "a", password)`.
    fn call_args(&mut self) -> Result<Vec<(String, Expr)>> {
        let mut args: Vec<(String, Expr)> = Vec::new();
        self.list("(", ")", |p| {
            let (name, span) = p.ident("an argument name (requests take named arguments)")?;
            if args.iter().any(|(n, _)| *n == name) {
                return p.err(span.start, format!("duplicate argument `{name}`"));
            }
            let value = if p.eat(":")? {
                p.skip_newlines()?;
                p.expr()?
            } else {
                Expr {
                    kind: ExprKind::Ident(name.clone()),
                    span,
                }
            };
            args.push((name, value));
            Ok(())
        })?;
        Ok(args)
    }

    /// Позиционные аргументы функций и методов; `x => …` — лямбда.
    fn method_args(&mut self) -> Result<Vec<Expr>> {
        let mut args = Vec::new();
        self.list("(", ")", |p| {
            if let (Tok::Ident(param), Tok::P("=>")) = p.peek2()? {
                let (_, start) = p.next()?;
                p.next()?;
                p.skip_newlines()?;
                let body = p.expr()?;
                args.push(Expr {
                    span: start.to(body.span),
                    kind: ExprKind::Lambda(param, Box::new(body)),
                });
            } else {
                args.push(p.expr()?);
            }
            Ok(())
        })?;
        Ok(args)
    }

    fn primary(&mut self) -> Result<Expr> {
        let (tok, span) = self.peek()?;
        let kind = match tok {
            Tok::Num(n) => {
                self.next()?;
                ExprKind::Num(n)
            }
            Tok::Str(_) => {
                let (Tok::Str(raw), _) = self.next()? else {
                    unreachable!()
                };
                ExprKind::Str(self.str_parts(raw)?)
            }
            Tok::Ident(w) => {
                if RESERVED.contains(&w.as_str()) {
                    return self.err(span.start, format!("unexpected keyword `{w}`"));
                }
                self.next()?;
                match w.as_str() {
                    "true" => ExprKind::Bool(true),
                    "false" => ExprKind::Bool(false),
                    "null" => ExprKind::Null,
                    _ => ExprKind::Ident(w),
                }
            }
            Tok::P("(") => {
                self.next()?;
                self.skip_newlines()?;
                let inner = self.expr()?;
                self.skip_newlines()?;
                self.expect(")")?;
                return Ok(Expr {
                    kind: inner.kind,
                    span: Span::new(span.start, self.pos),
                });
            }
            Tok::P("[") => {
                let mut items = Vec::new();
                self.list("[", "]", |p| {
                    items.push(p.expr()?);
                    Ok(())
                })?;
                ExprKind::Array(items)
            }
            Tok::P("{") => {
                let mut fields: Vec<(String, Expr)> = Vec::new();
                self.list("{", "}", |p| {
                    let (tok, kspan) = p.next()?;
                    let key = match tok {
                        Tok::Ident(k) => k,
                        Tok::Str(raw) => p.plain(raw, kspan)?,
                        other => {
                            return p.err(
                                kspan.start,
                                format!("expected a key, got {}", describe(&other)),
                            );
                        }
                    };
                    let value = if p.eat(":")? {
                        p.skip_newlines()?;
                        p.expr()?
                    } else if key.starts_with(is_ident_start) && key.chars().all(is_ident_char) {
                        Expr {
                            kind: ExprKind::Ident(key.clone()),
                            span: kspan,
                        }
                    } else {
                        return p.err(p.pos, format!("expected `:` after `{key}`"));
                    };
                    fields.push((key, value));
                    Ok(())
                })?;
                ExprKind::Object(fields)
            }
            Tok::Dur(_) => {
                return self.err(
                    span.start,
                    "durations are only allowed in timeout, cache and poll",
                );
            }
            other => {
                return self.err(
                    span.start,
                    format!("expected a value, got {}", describe(&other)),
                );
            }
        };
        Ok(Expr {
            kind,
            span: Span::new(span.start, self.pos),
        })
    }

    // ---- формы ----

    fn pattern(&mut self) -> Result<Pattern> {
        self.skip_spaces();
        if self.next_char() != Some('/') {
            return self.shape().map(Pattern::Shape);
        }
        let start = self.pos;
        let r = &self.rest()[1..];
        let mut end = None;
        let mut escaped = false;
        for (i, c) in r.char_indices() {
            match c {
                '\n' => break,
                '\\' if !escaped => escaped = true,
                '/' if !escaped => {
                    end = Some(i);
                    break;
                }
                _ => escaped = false,
            }
        }
        let Some(end) = end else {
            return self.err(start, "unclosed regular expression");
        };
        let source = &r[..end];
        let flags = &r[end + 1..];
        let flags = &flags[..flags
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(flags.len())];
        let mut b = regex::RegexBuilder::new(source);
        for f in flags.chars() {
            match f {
                'i' => b.case_insensitive(true),
                'm' => b.multi_line(true),
                's' => b.dot_matches_new_line(true),
                'x' => b.ignore_whitespace(true),
                other => return self.err(start, format!("unknown regex flag `{other}`")),
            };
        }
        let re = b
            .build()
            .or_else(|e| self.err(start, format!("invalid regular expression: {e}")))?;
        self.pos = start + 1 + end + 1 + flags.len();
        Ok(Pattern::Regex(re))
    }

    fn shape(&mut self) -> Result<Shape> {
        let mut alts = vec![self.shape_atom()?];
        while self.eat("|")? {
            self.skip_newlines()?;
            alts.push(self.shape_atom()?);
        }
        Ok(if alts.len() == 1 {
            alts.remove(0)
        } else {
            Shape::Union(alts)
        })
    }

    fn shape_atom(&mut self) -> Result<Shape> {
        let (tok, span) = self.peek()?;
        Ok(match tok {
            Tok::Ident(w) => {
                self.next()?;
                match w.as_str() {
                    "string" => Shape::String,
                    "number" => Shape::Number,
                    "integer" => Shape::Integer,
                    "boolean" => Shape::Boolean,
                    "any" => Shape::Any,
                    "null" => Shape::Null,
                    "true" => Shape::Literal(true.into()),
                    "false" => Shape::Literal(false.into()),
                    "schema" => {
                        self.expect("(")?;
                        let (tok, at) = self.next()?;
                        let Tok::Str(raw) = tok else {
                            return self.err(at.start, "expected a path string");
                        };
                        let path = self.plain(raw, at)?;
                        self.expect(")")?;
                        Shape::Schema(path)
                    }
                    _ if starts_upper(&w) => Shape::Named(w, span),
                    _ => {
                        return self.err(
                            span.start,
                            format!(
                                "unknown type `{w}` (expected {}, null, a literal or a shape name)",
                                TYPES.join(", ")
                            ),
                        );
                    }
                }
            }
            Tok::Str(_) => {
                let (Tok::Str(raw), at) = self.next()? else {
                    unreachable!()
                };
                Shape::Literal(self.plain(raw, at)?.into())
            }
            Tok::Num(n) => {
                self.next()?;
                Shape::Literal(num_value(n))
            }
            Tok::P("-") => {
                self.next()?;
                let (tok, at) = self.next()?;
                let Tok::Num(n) = tok else {
                    return self.err(at.start, "expected a number after `-`");
                };
                Shape::Literal(num_value(-n))
            }
            Tok::P("[") => {
                self.next()?;
                self.skip_newlines()?;
                let inner = self.shape()?;
                self.skip_newlines()?;
                self.expect("]")?;
                Shape::Array(Box::new(inner))
            }
            Tok::P("{") => {
                let mut fields = Vec::new();
                self.list("{", "}", |p| {
                    let (tok, at) = p.next()?;
                    let name = match tok {
                        Tok::Ident(k) => k,
                        Tok::Str(raw) => p.plain(raw, at)?,
                        other => {
                            return p.err(
                                at.start,
                                format!("expected a field name, got {}", describe(&other)),
                            );
                        }
                    };
                    let optional = p.eat("?")?;
                    p.expect(":")?;
                    p.skip_newlines()?;
                    let shape = p.shape()?;
                    fields.push(ShapeField {
                        name,
                        optional,
                        shape,
                    });
                    Ok(())
                })?;
                Shape::Object(fields)
            }
            other => {
                return self.err(
                    span.start,
                    format!("expected a shape, got {}", describe(&other)),
                );
            }
        })
    }
}

/// `users.Create` → `["users", "Create"]`, если выражение — цепочка имён.
fn ident_path(e: &Expr) -> Option<Vec<String>> {
    match &e.kind {
        ExprKind::Ident(n) => Some(vec![n.clone()]),
        ExprKind::Member(inner, n) => {
            let mut p = ident_path(inner)?;
            p.push(n.clone());
            Some(p)
        }
        _ => None,
    }
}

/// Целые числа — целыми, чтобы `201` не печаталось как `201.0`.
pub fn num_value(n: f64) -> serde_json::Value {
    if n.fract() == 0.0 && n.abs() < 9.0e15 {
        serde_json::Value::from(n as i64)
    } else {
        serde_json::Number::from_f64(n).map_or(serde_json::Value::Null, Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(src: &str) -> Request {
        match parse(src, Some("file")).unwrap().items.remove(0) {
            Item::Request(r) => r,
            other => panic!("{other:?}"),
        }
    }

    fn err(src: &str) -> (usize, usize, String) {
        match parse(src, None).unwrap_err() {
            Error::Syntax { line, col, msg } => (line, col, msg),
            e => panic!("{e}"),
        }
    }

    fn expr(src: &str) -> Expr {
        let file = parse(&format!("let x = {src}\n"), None).unwrap();
        match file.items.into_iter().next() {
            Some(Item::Let(l)) => l.value,
            other => panic!("{other:?}"),
        }
    }

    const EXAMPLE: &str = r#"// Create order
// Creates an order for the current user.
POST /orders/{shop} {
  only: [dev, staging]
  timeout: 10s

  query { page: 1, tags: ["a", "b"] }

  headers {
    Authorization: "Bearer ${Login().body.token}"
    Idempotency-Key: uuid()
  }

  body {
    "name": "Viktor",
    customer: CreateUser(name: "Bob").body.id,
    items: [{ sku: "A-1", qty: 2 }],
    email,
  }

  expect {
    status == 201
    body.items.length == 1
    headers.Location.startsWith("/orders/")
    body matches Order
    body.id matches /^\d+$/i
    body.items.all(i => i.qty > 0)
    fresh users.GetOrder(id: body.id).body.status == "new"
  }

  save order_id = body.id
}
"#;

    #[test]
    fn spec_example() {
        let r = one(EXAMPLE);
        assert_eq!(r.name.as_deref(), Some("CreateOrder"));
        assert_eq!(r.doc.description, "Creates an order for the current user.");
        assert_eq!(r.method, "POST");
        assert_eq!(r.target.kind, TargetKind::Path);
        assert!(
            matches!(&r.target.parts[..], [TargetPart::Lit(a), TargetPart::Param(p, _)] if a == "/orders/" && p == "shop")
        );
        let f = &r.fields;
        assert_eq!(
            f.only.as_deref(),
            Some(&["dev".to_string(), "staging".to_string()][..])
        );
        assert_eq!(f.timeout, Some(Duration::from_secs(10)));
        assert_eq!(f.query.len(), 2);
        assert_eq!(f.headers[1].key, "Idempotency-Key");
        let Some(Body::Value(body)) = &f.body else {
            panic!()
        };
        let ExprKind::Object(fields) = &body.kind else {
            panic!()
        };
        let keys: Vec<_> = fields.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["name", "customer", "items", "email"]);
        assert_eq!(f.expect.len(), 7);
        let ExprKind::Binary(BinOp::Eq, lhs, _) = &f.expect[6].kind else {
            panic!()
        };
        let ExprKind::Member(call, _) = &lhs.kind else {
            panic!()
        };
        let ExprKind::Member(call, _) = &call.kind else {
            panic!()
        };
        let ExprKind::Call(c) = &call.kind else {
            panic!("{call:?}")
        };
        assert_eq!(c.path, ["users", "GetOrder"]);
        assert!(c.fresh);
        assert_eq!(f.saves[0].name, "order_id");
        let order: Vec<_> = f.order.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            order,
            [
                "only", "timeout", "query", "headers", "body", "expect", "save"
            ]
        );
    }

    #[test]
    fn minimal_file_is_named_after_it() {
        let r = one("GET /health\n");
        assert_eq!(r.name.as_deref(), Some("File"));
        assert!(r.fields.order.is_empty());
        let r = one("GET https://example.com/x?a=1");
        assert_eq!(r.target.kind, TargetKind::Url);
        let r = one("GET \"${auth}/token\" { expect { status == 200 } }");
        assert_eq!(r.target.kind, TargetKind::Str);
        assert_eq!(pascal_case("1-create order"), Some("CreateOrder".into()));
        assert_eq!(pascal_case("получить заказ"), Some("ПолучитьЗаказ".into()));
    }

    #[test]
    fn doc_comment_must_touch_the_item() {
        let f = parse(
            "// file header\n\n// Login\nPOST /login\n\nGET /a // trailing\n// Me\n\nGET /me\n",
            None,
        )
        .unwrap();
        let names: Vec<_> = f
            .items
            .iter()
            .map(|i| i.name().map(str::to_string))
            .collect();
        assert_eq!(names, [Some("Login".into()), None, None]);
        assert_eq!(f.comments.len(), 4);
    }

    #[test]
    fn strings() {
        let s = |src: &str| match expr(src).kind {
            ExprKind::Str(parts) => parts,
            k => panic!("{k:?}"),
        };
        assert!(
            matches!(&s(r#""a\n\"b\" \u{41}é \$x""#)[..], [StrPart::Lit(t)] if t == "a\n\"b\" Aé $x")
        );
        assert!(matches!(&s(r"'it\'s'")[..], [StrPart::Lit(t)] if t == "it's"));
        let parts = s(r#""Hi, ${user.name + "!"}""#);
        assert!(matches!(&parts[..], [StrPart::Lit(a), StrPart::Expr(_)] if a == "Hi, "));
        let triple = s("\"\"\"\n    {\n      \"a\": ${x}\n    }\n    \"\"\"");
        let text: String = triple
            .iter()
            .map(|p| match p {
                StrPart::Lit(t) => t.clone(),
                StrPart::Expr(_) => "X".into(),
            })
            .collect();
        assert_eq!(text, "{\n  \"a\": X\n}");
    }

    #[test]
    fn expressions() {
        assert!(matches!(
            expr("1 + 2 * 3").kind,
            ExprKind::Binary(BinOp::Add, _, _)
        ));
        assert!(matches!(
            expr("a || b && c").kind,
            ExprKind::Binary(BinOp::Or, _, _)
        ));
        assert!(matches!(expr("-5").kind, ExprKind::Num(n) if n == -5.0));
        assert!(
            matches!(expr("headers.content-type").kind, ExprKind::Member(_, n) if n == "content-type")
        );
        assert!(matches!(
            expr("a.b - c").kind,
            ExprKind::Binary(BinOp::Sub, _, _)
        ));
        assert!(matches!(
            expr("typeof x == \"string\"").kind,
            ExprKind::Binary(BinOp::Eq, _, _)
        ));
        assert!(matches!(
            expr("x in [1, 2]").kind,
            ExprKind::Binary(BinOp::In, _, _)
        ));
        assert!(matches!(
            expr("a &&\n  b").kind,
            ExprKind::Binary(BinOp::And, _, _)
        ));
        assert!(
            matches!(expr("randomInt(1, 10)").kind, ExprKind::Builtin(n, a) if n == "randomInt" && a.len() == 2)
        );
        assert!(matches!(expr("Login(email)").kind, ExprKind::Call(c) if c.args[0].0 == "email"));
        assert!(matches!(
            expr("body matches { id: string, note?: string | null, items: [Item] }").kind,
            ExprKind::Matches(_, Pattern::Shape(Shape::Object(f))) if f.len() == 3 && f[1].optional
        ));
    }

    #[test]
    fn flows_shapes_lets() {
        let f = parse(
            "let shop = \"main\"\nshape Item { sku: string, qty: integer }\n\n// Checkout\nflow Checkout {\n  params { user: \"bob\" }\n  order = CreateOrder(shop)\n  Pay(order: order.body.id)\n  expect { GetOrder(id: order.body.id).body.status == \"paid\" }\n  save last = order.body.id\n}\n",
            None,
        )
        .unwrap();
        assert_eq!(f.items.len(), 3);
        let Item::Flow(fl) = &f.items[2] else {
            panic!()
        };
        assert_eq!(fl.name, "Checkout");
        assert_eq!(fl.doc.title.as_deref(), Some("Checkout"));
        assert_eq!(fl.params.len(), 1);
        assert!(matches!(
            &fl.steps[..],
            [
                Step::Bind { .. },
                Step::Do(_),
                Step::Expect(_),
                Step::Save(_)
            ]
        ));
    }

    #[test]
    fn errors_have_positions() {
        assert_eq!(err("GET /x {\n  nope: 1\n}\n").0, 2);
        let (line, col, msg) = err("GET {{base}}/x\n");
        assert_eq!((line, col), (1, 5));
        assert!(msg.contains("{var}"), "{msg}");
        assert_eq!(err("GET /x {\n  body \"abc\n}").0, 2);
        assert!(
            err("GET /x {\n  timeout: 1s\n  timeout: 2s\n}")
                .2
                .contains("duplicate")
        );
        assert!(
            err("GET /x {\n  body {}\n  form {}\n}")
                .2
                .contains("at most one")
        );
        assert!(
            err("GET /x {\n  expect { status == }\n}")
                .2
                .contains("expected a value")
        );
        assert!(
            err("GET /x {\n  expect { a b }\n}")
                .2
                .contains("expected `,` or a new line")
        );
        assert!(err("let x = Login(1)").2.contains("argument name"));
        assert!(err("let x = fresh uuid()").2.contains("fresh"));
        assert!(err("get /x").2.contains("expected a request"));
        assert!(err("GET /x {").2.contains("unclosed"));
        assert!(
            err("GET /x { expect { a matches /(/ } }")
                .2
                .contains("regular expression")
        );
    }

    #[test]
    fn line_col_counts_chars() {
        assert_eq!(line_col("ab\nцd", 5), (2, 2));
    }
}
