//! Дерево разобранного `*.routy`. У каждого узла — позиция в исходнике (`Span`), а комментарии
//! хранятся отдельно: этого хватает, чтобы `routy fmt` и `--fix` переписывали файл, не теряя их.

use std::time::Duration;

use serde_json::Value;

/// Байтовый диапазон в исходном тексте файла.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Span {
        Span { start, end }
    }

    pub fn to(self, other: Span) -> Span {
        Span::new(self.start, other.end)
    }

    pub fn text(self, src: &str) -> &str {
        src.get(self.start..self.end).unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct Comment {
    pub span: Span,
    /// Текст без `//` / `/* */`.
    pub text: String,
    pub block: bool,
}

#[derive(Debug, Clone, Default)]
pub struct File {
    pub items: Vec<Item>,
    pub comments: Vec<Comment>,
}

#[derive(Debug, Clone)]
// Элементов в файле единицы — экономить память ценой `Box` незачем.
#[allow(clippy::large_enum_variant)]
pub enum Item {
    Request(Request),
    Flow(Flow),
    Shape(ShapeDecl),
    Let(Let),
}

impl Item {
    pub fn span(&self) -> Span {
        match self {
            Item::Request(r) => r.span,
            Item::Flow(f) => f.span,
            Item::Shape(s) => s.span,
            Item::Let(l) => l.span,
        }
    }

    /// Имя, по которому элемент вызывают и запускают.
    pub fn name(&self) -> Option<&str> {
        match self {
            Item::Request(r) => r.name.as_deref(),
            Item::Flow(f) => Some(&f.name),
            Item::Shape(s) => Some(&s.name),
            Item::Let(l) => Some(&l.name),
        }
    }
}

/// Комментарий `//` прямо над запросом или сценарием.
#[derive(Debug, Clone, Default)]
pub struct Doc {
    /// Первая строка: `Create order`.
    pub title: Option<String>,
    /// Остальные строки.
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub span: Span,
    pub doc: Doc,
    /// `CreateOrder` — из первой строки комментария или имени файла.
    pub name: Option<String>,
    pub method: String,
    pub target: Target,
    pub fields: Fields,
}

#[derive(Debug, Clone)]
pub struct Target {
    pub kind: TargetKind,
    pub parts: Vec<TargetPart>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// `/users/{id}` — дописывается к `base`.
    Path,
    /// `https://…` как есть.
    Url,
    /// `"${auth_url}/token"`: подстановки не кодируются.
    Str,
}

#[derive(Debug, Clone)]
pub enum TargetPart {
    Lit(String),
    /// `{name}` — параметр пути.
    Param(String, Span),
    /// `${expr}`.
    Expr(Expr),
}

/// Поля запроса. `order` — какие поля были и где, в порядке исходника.
#[derive(Debug, Clone, Default)]
pub struct Fields {
    pub handler: Option<String>,
    pub params: Vec<Param>,
    pub only: Option<Vec<String>>,
    pub confirm: bool,
    pub timeout: Option<Duration>,
    pub redirects: Option<bool>,
    pub cache: Option<Duration>,
    pub query: Vec<Entry>,
    pub headers: Vec<Entry>,
    pub body: Option<Body>,
    pub poll: Option<Poll>,
    pub expect: Vec<Expr>,
    pub saves: Vec<Save>,
    pub order: Vec<(&'static str, Span)>,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub default: Option<Expr>,
    pub span: Span,
}

/// `key: value` в `query`, `headers`, `form`, `multipart`.
#[derive(Debug, Clone)]
pub struct Entry {
    pub key: String,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Body {
    /// `body { … }`, `body "text"`, `body file("…")`.
    Value(Expr),
    Form(Vec<Entry>),
    Multipart(Vec<Entry>),
}

#[derive(Debug, Clone)]
pub struct Poll {
    pub until: Expr,
    pub every: Duration,
    pub limit: Duration,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Save {
    pub name: String,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Flow {
    pub span: Span,
    pub doc: Doc,
    pub name: String,
    pub params: Vec<Param>,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone)]
pub enum Step {
    /// `order = CreateOrder(…)`.
    Bind {
        name: String,
        value: Expr,
        span: Span,
    },
    /// Вызов ради побочного эффекта: `Pay(order: id)`.
    Do(Expr),
    Expect(Vec<Expr>),
    Save(Save),
}

#[derive(Debug, Clone)]
pub struct ShapeDecl {
    pub span: Span,
    pub name: String,
    pub shape: Shape,
}

#[derive(Debug, Clone)]
pub struct Let {
    pub span: Span,
    pub name: String,
    pub value: Expr,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Null,
    Bool(bool),
    Num(f64),
    Str(Vec<StrPart>),
    Ident(String),
    Array(Vec<Expr>),
    Object(Vec<(String, Expr)>),
    Member(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    /// Встроенная функция: `uuid()`, `randomInt(1, 10)`.
    Builtin(String, Vec<Expr>),
    /// Метод значения: `.startsWith("x")`, `.map(i => i.id)`.
    Method(Box<Expr>, String, Vec<Expr>),
    /// Вызов запроса или сценария: `Login(email: "a")`, `users.Create()`.
    Call(Call),
    /// `x => expr` — только аргумент методов.
    Lambda(String, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Matches(Box<Expr>, Pattern),
}

#[derive(Debug, Clone)]
pub struct Call {
    /// `["users", "Create"]` для `users.Create()`.
    pub path: Vec<String>,
    pub args: Vec<(String, Expr)>,
    pub fresh: bool,
}

impl Call {
    pub fn name(&self) -> String {
        self.path.join(".")
    }
}

#[derive(Debug, Clone)]
pub enum StrPart {
    Lit(String),
    Expr(Expr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
    Typeof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    In,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

impl BinOp {
    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinOp::In | BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        )
    }
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Regex(regex::Regex),
    Shape(Shape),
}

#[derive(Debug, Clone)]
pub enum Shape {
    Any,
    String,
    Number,
    Integer,
    Boolean,
    Null,
    Literal(Value),
    Union(Vec<Shape>),
    Array(Box<Shape>),
    Object(Vec<ShapeField>),
    Named(String, Span),
    Schema(String),
}

#[derive(Debug, Clone)]
pub struct ShapeField {
    pub name: String,
    pub optional: bool,
    pub shape: Shape,
}
