//! Что передавать в роут: разбор обработчика. Описание — из doc-комментария (и `@Summary` swag),
//! тело — из структуры, в которую декодируется запрос (`json.NewDecoder(r.Body).Decode(&req)`,
//! `c.ShouldBindJSON(&req)`, свои `decodeJSON(r, &req)`), query и заголовки — из чтения в коде
//! (`r.URL.Query().Get("page")`, `c.Query("page")`, `r.Header.Get("X-Id")`, `c.ShouldBindQuery(&q)`).

use std::cell::RefCell;
use std::collections::HashMap;

use tree_sitter::Node;

use super::{Func, GoFile, children, text};
use crate::import::sample::Json;
use crate::import::{Body, FORM_FILE, Field, Response, RouteInfo, ShapeDef, StatusCode};

/// Глубина вложенных структур в примере тела.
const MAX_DEPTH: usize = 6;
/// Строк описания из комментария.
const MAX_DOC: usize = 6;

/// Слова в имени функции, по которым вызов считается чтением тела в `&req`.
const BODY_CALLS: &[&str] = &[
    "decode",
    "bind",
    "unmarshal",
    "readjson",
    "parsejson",
    "bodyparser",
    "parsebody",
    "readentity",
];
/// Поля формы (application/x-www-form-urlencoded или multipart).
const FORM_CALLS: &[&str] = &[
    "PostForm",
    "DefaultPostForm",
    "GetPostForm",
    "PostFormArray",
    "GetPostFormArray",
    "PostFormValue",
];
/// Файлы multipart: `c.FormFile("file")`, `r.FormFile("file")`.
const FILE_CALLS: &[&str] = &["FormFile"];
const QUERY_CALLS: &[&str] = &[
    "Query",
    "DefaultQuery",
    "GetQuery",
    "QueryArray",
    "GetQueryArray",
    "QueryParam",
    "QueryInt",
    "QueryBool",
    "QueryFloat",
    "FormValue",
    "QueryParameter",
];
/// Query у контроллеров beego: `c.GetString("page")`. В gin те же имена читают контекст.
const BEEGO_QUERY_CALLS: &[&str] = &[
    "GetString",
    "GetStrings",
    "GetInt",
    "GetInt64",
    "GetUint64",
    "GetBool",
    "GetFloat",
];

pub(super) struct Describer<'a> {
    files: &'a [GoFile],
    funcs: &'a [Func],
    /// (пакет, имя) → (файл, байтовый диапазон type_spec)
    types: HashMap<(String, String), (usize, usize, usize)>,
    /// Shape для структур из ответов, по мере встречи
    shapes: RefCell<Vec<ShapeDef>>,
}

impl<'a> Describer<'a> {
    pub(super) fn new(files: &'a [GoFile], funcs: &'a [Func]) -> Self {
        let mut types = HashMap::new();
        for (i, f) in files.iter().enumerate() {
            for decl in children(f.tree.root_node()).filter(|n| n.kind() == "type_declaration") {
                for spec in
                    children(decl).filter(|n| matches!(n.kind(), "type_spec" | "type_alias"))
                {
                    if let Some(name) = spec.child_by_field_name("name") {
                        let key = (f.package.clone(), text(name, &f.src).to_string());
                        types.insert(key, (i, spec.start_byte(), spec.end_byte()));
                    }
                }
            }
        }
        Describer {
            files,
            funcs,
            types,
            shapes: RefCell::default(),
        }
    }

    /// Shape всех структур, встреченных в ответах.
    pub(super) fn into_shapes(self) -> Vec<ShapeDef> {
        self.shapes.into_inner()
    }

    /// Описание роута по выражению-обработчику в файле `file`; `router` — имя шаблона.
    pub(super) fn describe(&self, file: usize, handler: (usize, usize), router: &str) -> RouteInfo {
        let Some(node) = self.node(file, handler) else {
            return RouteInfo::default();
        };
        let Some((file, body, decl)) = self.target(file, node, 0) else {
            return RouteInfo::default();
        };
        self.describe_body(file, body, decl, router)
    }

    /// Описание роута, обработчик которого — функция или метод `funcs[f]` (контроллер beego,
    /// ресурс buffalo).
    pub(super) fn describe_func(&self, f: usize, router: &str) -> RouteInfo {
        let func = &self.funcs[f];
        let Some(decl) = self.decl(func) else {
            return RouteInfo::default();
        };
        let Some(body) = decl.child_by_field_name("body") else {
            return RouteInfo::default();
        };
        self.describe_body(func.file, body, Some(decl), router)
    }

    fn describe_body(
        &self,
        file: usize,
        body: Node<'a>,
        decl: Option<Node<'a>>,
        router: &str,
    ) -> RouteInfo {
        let mut info = RouteInfo::default();
        if let Some(decl) = decl {
            self.doc(file, decl, &mut info);
        }
        self.scan_body(file, body, router, &mut info);
        self.statuses(file, body, &mut info);
        info
    }

    /// Коды ответа в теле обработчика: константы `http.StatusNotFound` (и `fiber.Status…`)
    /// где угодно, числа 100–599 — аргументами вызовов-ответов (`c.JSON(404, …)`,
    /// `w.WriteHeader(201)`). Без успешного кода — неявный 200.
    fn statuses(&self, file: usize, body: Node<'a>, info: &mut RouteInfo) {
        let f = &self.files[file];
        let src = &f.src;
        let mut found: Vec<StatusCode> = Vec::new();
        let mut add = |code: u16, n: Node| {
            if !found.iter().any(|s| s.code == code) {
                found.push(StatusCode {
                    code,
                    source: f.path.clone(),
                    line: n.start_position().row + 1,
                    implicit: false,
                });
            }
        };
        walk(body, &mut |n| match n.kind() {
            "selector_expression" => {
                if let Some(code) = n
                    .child_by_field_name("field")
                    .and_then(|f| status_const(text(f, src)))
                {
                    add(code, n);
                }
            }
            "int_literal" => {
                let Some(code) = text(n, src)
                    .parse::<u16>()
                    .ok()
                    .filter(|c| (100..600).contains(c))
                else {
                    return;
                };
                let call = n
                    .parent()
                    .filter(|p| p.kind() == "argument_list")
                    .and_then(|p| p.parent())
                    .and_then(|c| c.child_by_field_name("function"));
                let name = match call {
                    Some(f) if f.kind() == "selector_expression" => {
                        f.child_by_field_name("field").map_or("", |f| text(f, src))
                    }
                    Some(f) if f.kind() == "identifier" => text(f, src),
                    _ => return,
                };
                let name = name.to_ascii_lowercase();
                if STATUS_CALLS.iter().any(|w| name.contains(w)) {
                    add(code, n);
                }
            }
            _ => {}
        });
        if !found.iter().any(|s| (200..300).contains(&s.code)) {
            found.push(StatusCode {
                code: 200,
                source: f.path.clone(),
                line: body.start_position().row + 1,
                implicit: true,
            });
        }
        found.sort_by_key(|s| s.code);
        info.statuses = found;
    }

    /// Тело запроса из значения типа (`User{}` в `.Reads(User{})`) в файле `file`.
    pub(super) fn body_of(&self, file: usize, value: (usize, usize)) -> Option<Body> {
        let v = self.node(file, value)?;
        let (f, ty, scope) = self.value_type(file, scope_of(v), v)?;
        let (fields, example) = self.fields(f, ty, scope, &["json"], false, 0)?;
        Some(Body {
            type_name: compact(text(ty, &self.files[f].src))
                .trim_start_matches(['*', '&'])
                .to_string(),
            fields,
            example: example.pretty(),
        })
    }

    /// Ответ из значения типа (`User{}` в `.Writes(User{})`).
    pub(super) fn response_of(&self, file: usize, value: (usize, usize)) -> Option<Response> {
        let v = self.node(file, value)?;
        let (f, ty, scope) = self.value_type(file, scope_of(v), v)?;
        let shape = self.shape_of(f, ty, scope, 0);
        (shape != "any").then(|| Response {
            type_name: compact(text(ty, &self.files[f].src))
                .trim_start_matches(['*', '&'])
                .to_string(),
            shape,
        })
    }

    fn node(&self, file: usize, (start, end): (usize, usize)) -> Option<Node<'a>> {
        self.files[file]
            .tree
            .root_node()
            .descendant_for_byte_range(start, end)
    }

    /// Объявление функции или метода.
    fn decl(&self, func: &Func) -> Option<Node<'a>> {
        let root = self.files[func.file].tree.root_node();
        let mut decl = root.descendant_for_byte_range(func.start, func.end)?;
        while !matches!(decl.kind(), "function_declaration" | "method_declaration") {
            decl = decl.parent()?;
        }
        Some(decl)
    }

    /// Тело функции-обработчика: литерал, функция/метод по имени, фабрика `h.Create()`,
    /// `http.HandlerFunc(h.Create)`. Возвращает файл, тело и объявление (для комментария).
    fn target(
        &self,
        file: usize,
        node: Node<'a>,
        depth: usize,
    ) -> Option<(usize, Node<'a>, Option<Node<'a>>)> {
        if depth > 4 {
            return None;
        }
        let src = &self.files[file].src;
        match node.kind() {
            "func_literal" => Some((file, node.child_by_field_name("body")?, None)),
            "parenthesized_expression" => self.target(file, node.named_child(0)?, depth + 1),
            "identifier" => self.func(file, text(node, src), None),
            "selector_expression" => {
                let field = text(node.child_by_field_name("field")?, src);
                let operand = node.child_by_field_name("operand")?;
                // (*UserController).Get — метод конкретного типа (beego `CtrlGet`)
                if operand.kind() == "parenthesized_expression" {
                    let ty = text(operand, src).trim_matches(['(', ')', '*', ' ']);
                    let (pkg, ty) = match ty.rsplit_once('.') {
                        Some((p, t)) => (Some(p), t),
                        None => (None, ty),
                    };
                    let (f, decl) = self.pick(file, field, pkg, Some(ty), false)?;
                    return Some((f, decl.child_by_field_name("body")?, Some(decl)));
                }
                let pkg = (operand.kind() == "identifier").then(|| text(operand, src));
                self.func(file, field, pkg)
            }
            "call_expression" => {
                let func = node.child_by_field_name("function")?;
                let name = match func.kind() {
                    "selector_expression" => text(func.child_by_field_name("field")?, src),
                    _ => text(func, src),
                };
                // http.HandlerFunc(h.Create) — преобразование типа, смотрим внутрь.
                if name == "HandlerFunc" || name == "Handler" {
                    let arg = node.child_by_field_name("arguments")?.named_child(0)?;
                    return self.target(file, arg, depth + 1);
                }
                self.target(file, func, depth + 1)
            }
            _ => None,
        }
    }

    /// Функция или метод `name`: из пакета `pkg`, если он есть, иначе из пакета файла, иначе любая.
    fn func(
        &self,
        file: usize,
        name: &str,
        pkg: Option<&str>,
    ) -> Option<(usize, Node<'a>, Option<Node<'a>>)> {
        let (f, decl) = self.pick(file, name, pkg, None, false)?;
        Some((f, decl.child_by_field_name("body")?, Some(decl)))
    }

    /// Объявление функции или метода `name` (см. `func`); `recv` — только методы этого типа;
    /// `returns` — только те, что что-то возвращают: `h.svc.Get(…)` в обработчике `Get` —
    /// метод сервиса, а не сам обработчик.
    fn pick(
        &self,
        file: usize,
        name: &str,
        pkg: Option<&str>,
        recv: Option<&str>,
        returns: bool,
    ) -> Option<(usize, Node<'a>)> {
        let own = &self.files[file].package;
        let candidates: Vec<(&Func, Node<'a>)> = self
            .funcs
            .iter()
            .filter(|f| f.name == name)
            .filter(|f| recv.is_none() || f.recv_type.as_deref() == recv)
            .filter_map(|f| {
                let decl = self.decl(f)?;
                (!returns || decl.child_by_field_name("result").is_some()).then_some((f, decl))
            })
            .collect();
        let (pick, decl) = pkg
            .and_then(|p| candidates.iter().find(|(f, _)| f.package == p))
            .or_else(|| candidates.iter().find(|(f, _)| &f.package == own))
            .or(candidates.first())?;
        Some((pick.file, *decl))
    }

    /// Тип результата вызова `call`: `T` или первый из `(T, error)`. Возвращает файл,
    /// узел типа и объявление функции (область для её локальных типов).
    fn call_result(&self, file: usize, call: Node<'a>) -> Option<(usize, Node<'a>, Node<'a>)> {
        let src = &self.files[file].src;
        let func = call.child_by_field_name("function")?;
        let func = match func.kind() {
            "generic_type" | "index_expression" => func.named_child(0)?,
            _ => func,
        };
        let (name, pkg) = match func.kind() {
            "identifier" => (text(func, src), None),
            "selector_expression" => {
                let operand = func.child_by_field_name("operand")?;
                let pkg = (operand.kind() == "identifier").then(|| text(operand, src));
                (text(func.child_by_field_name("field")?, src), pkg)
            }
            _ => return None,
        };
        let (f, decl) = self.pick(file, name, pkg, None, true)?;
        let mut result = decl.child_by_field_name("result")?;
        if result.kind() == "parameter_list" {
            // (T, error) — первый тип.
            result = children(result).next()?.child_by_field_name("type")?;
        }
        Some((f, result, decl))
    }

    /// Комментарий над объявлением: `@Summary`/`@Description` из swag или обычный текст.
    fn doc(&self, file: usize, decl: Node, info: &mut RouteInfo) {
        let src = &self.files[file].src;
        let mut lines = Vec::new();
        let mut row = decl.start_position().row;
        let mut prev = decl.prev_sibling();
        while let Some(c) =
            prev.filter(|c| c.kind() == "comment" && c.end_position().row + 1 >= row)
        {
            let t = text(c, src);
            let t = t
                .strip_prefix("//")
                .or_else(|| t.strip_prefix("/*").and_then(|t| t.strip_suffix("*/")))
                .unwrap_or(t);
            for l in t.lines().rev() {
                lines.push(l.trim().trim_start_matches('*').trim().to_string());
            }
            row = c.start_position().row;
            prev = c.prev_sibling();
        }
        lines.reverse();
        let mut text_lines = Vec::new();
        for l in lines {
            if let Some(s) = l.strip_prefix("@Summary") {
                info.summary = Some(s.trim().to_string());
            } else if let Some(d) = l.strip_prefix("@Description") {
                text_lines.push(d.trim().to_string());
            } else if !l.starts_with('@') && !l.is_empty() {
                text_lines.push(l);
            }
        }
        if info.summary.is_none() && !text_lines.is_empty() {
            info.summary = Some(text_lines.remove(0));
        }
        text_lines.truncate(MAX_DOC);
        info.description = text_lines;
    }

    fn scan_body(&self, file: usize, body: Node, router: &str, info: &mut RouteInfo) {
        let src = &self.files[file].src;
        // q := r.URL.Query()
        let mut query_vars = Vec::new();
        walk(body, &mut |n| {
            if n.kind() == "short_var_declaration"
                && let (Some(l), Some(r)) = (
                    n.child_by_field_name("left"),
                    n.child_by_field_name("right"),
                )
                && text(r, src).trim_end().ends_with("Query()")
                && let Some(id) = l.named_child(0)
            {
                query_vars.push(text(id, src).to_string());
            }
        });
        walk(body, &mut |n| {
            if n.kind() != "call_expression" {
                return;
            }
            let (Some(func), Some(args)) = (
                n.child_by_field_name("function"),
                n.child_by_field_name("arguments"),
            ) else {
                return;
            };
            let (name, operand) = match func.kind() {
                "selector_expression" => (
                    func.child_by_field_name("field")
                        .map_or("", |f| text(f, src)),
                    func.child_by_field_name("operand")
                        .map_or("", |o| text(o, src)),
                ),
                "identifier" => (text(func, src), ""),
                _ => return,
            };
            let first = args.named_child(0).and_then(|a| literal(a, src));
            if let Some(param) = first.filter(|p| is_param_name(p)) {
                let operand = operand.trim_end();
                // fiber: `c.Get("X-Id")` — заголовок запроса
                let fiber_header = router == "fiber"
                    && name == "Get"
                    && !operand.contains(['.', '('])
                    && !query_vars.iter().any(|v| v == operand);
                if name == "Get" && operand.ends_with("Header")
                    || fiber_header
                    || name == "GetHeader"
                    || name == "HeaderParameter"
                {
                    push_unique(&mut info.headers, param);
                    return;
                }
                let file = FILE_CALLS.contains(&name);
                if file || FORM_CALLS.contains(&name) {
                    if !info.form.iter().any(|f| f.name == param) {
                        info.form.push(Field {
                            name: param,
                            ty: if file { FORM_FILE } else { "string" }.into(),
                            required: false,
                            comment: None,
                            json: None,
                        });
                    }
                    return;
                }
                if name == "Get"
                    && (operand.ends_with("Query()") || query_vars.iter().any(|v| v == operand))
                    || QUERY_CALLS.contains(&name)
                    || router == "beego" && BEEGO_QUERY_CALLS.contains(&name)
                {
                    if !info.query.iter().any(|q| q.name == param) {
                        info.query.push(Field {
                            name: param,
                            ty: "string".into(),
                            required: false,
                            comment: None,
                            json: None,
                        });
                    }
                    return;
                }
            }
            let lname = name.to_ascii_lowercase();
            let is_query = lname.contains("query");
            // go-zero `httpx.Parse(r, &req)`: поля по тегам — `form` в query, `header`, `json` в тело
            let tagged = name == "Parse" && operand.trim_end().ends_with("httpx");
            if lname.contains("uri") || lname.contains("header") {
                return;
            }
            // fiber v3: `c.Bind().Body(&req)`, `c.Bind().JSON(&req)`
            let binder = bind_chain(operand);
            if !is_query && !tagged && !binder && !BODY_CALLS.iter().any(|w| lname.contains(w)) {
                return;
            }
            // c.ShouldBindJSON(&req), а иначе — v.Bind(c), где v — объект-валидатор.
            let receiver = (func.kind() == "selector_expression")
                .then(|| func.child_by_field_name("operand"))
                .flatten()
                .filter(|o| o.kind() == "identifier")
                .map(|o| text(o, src).to_string());
            let Some((file, ty, scope)) = children(args)
                .find_map(|a| bound_var(a, src))
                .and_then(|v| self.var_type(file, body, &v))
                .or_else(|| receiver.and_then(|v| self.var_type(file, body, &v)))
            else {
                return;
            };
            let src = &self.files[file].src;
            let mut add_query = |fields: Vec<Field>| {
                for f in fields {
                    if !info.query.iter().any(|q| q.name == f.name) {
                        info.query.push(f);
                    }
                }
            };
            if tagged {
                if let Some((fields, _)) = self.fields(file, ty, scope, &["form"], true, 0) {
                    add_query(fields);
                }
                if let Some((fields, _)) = self.fields(file, ty, scope, &["header"], true, 0) {
                    for f in fields {
                        push_unique(&mut info.headers, f.name);
                    }
                }
            } else if is_query {
                let tags = &["form", "query", "json"];
                if let Some((fields, _)) = self.fields(file, ty, scope, tags, false, 0) {
                    add_query(fields);
                }
                return;
            }
            if info.body.is_none()
                && let Some((fields, example)) = self.fields(file, ty, scope, &["json"], tagged, 0)
                && (!tagged || !fields.is_empty())
            {
                info.body = Some(Body {
                    type_name: compact(text(ty, src))
                        .trim_start_matches(['*', '&'])
                        .to_string(),
                    fields,
                    example: example.pretty(),
                });
            }
        });
        self.response(file, body, info);
    }

    /// Тип ответа: первый успешный `json.NewEncoder(w).Encode(x)`, `c.JSON(200, x)`,
    /// `render.JSON(w, r, x)`, свой `writeJSON(w, http.StatusOK, x)`, `resp.WriteEntity(x)`
    /// (go-restful), `c.Data["json"] = x` (beego). Ответы с кодом ошибки (в том числе у
    /// внешнего вызова: `c.Render(404, r.JSON(x))`) и `gin.H`/map пропускаются.
    fn response(&self, file: usize, body: Node<'a>, info: &mut RouteInfo) {
        let src = &self.files[file].src;
        walk(body, &mut |n| {
            if info.response.is_some() {
                return;
            }
            let value = match n.kind() {
                "call_expression" => self.response_arg(file, n),
                "assignment_statement" => n
                    .child_by_field_name("left")
                    .filter(|l| compact(text(*l, src)).ends_with(r#"Data["json"]"#))
                    .and(n.child_by_field_name("right"))
                    .and_then(|r| r.named_child(0)),
                _ => None,
            };
            let Some(value) = value else {
                return;
            };
            let Some((f, ty, scope)) = self.value_type(file, body, value) else {
                return;
            };
            let shape = self.shape_of(f, ty, scope, 0);
            if shape == "any" || shape == "{}" {
                return;
            }
            info.response = Some(Response {
                type_name: compact(text(ty, &self.files[f].src))
                    .trim_start_matches(['*', '&'])
                    .to_string(),
                shape,
            });
        });
    }

    /// Значение, которое вызов `n` отдаёт ответом, если это вызов-ответ с успешным кодом.
    fn response_arg(&self, file: usize, n: Node<'a>) -> Option<Node<'a>> {
        let src = &self.files[file].src;
        let func = n.child_by_field_name("function")?;
        let args = n.child_by_field_name("arguments")?;
        let (name, operand) = match func.kind() {
            "selector_expression" => (
                text(func.child_by_field_name("field")?, src),
                func.child_by_field_name("operand")
                    .map_or("", |o| text(o, src)),
            ),
            "identifier" => (text(func, src), ""),
            _ => return None,
        };
        let lname = name.to_ascii_lowercase();
        let encode = name == "Encode" && operand.contains("NewEncoder");
        let json = lname.contains("json")
            && !BODY_CALLS.iter().any(|w| lname.contains(w))
            && !bind_chain(operand);
        let entity = matches!(name, "WriteEntity" | "WriteHeaderAndEntity");
        if !encode && !json && !entity {
            return None;
        }
        let args: Vec<Node> = children(args).collect();
        let failed = |args: &[Node]| args.iter().any(|a| error_status(text(*a, src)));
        if failed(&args) {
            return None;
        }
        // fiber: c.Status(fiber.StatusNotFound).JSON(err)
        if let Some(status) = func
            .child_by_field_name("operand")
            .filter(|o| o.kind() == "call_expression")
            .and_then(|o| o.child_by_field_name("arguments"))
            && failed(&children(status).collect::<Vec<_>>())
        {
            return None;
        }
        // c.Render(http.StatusNotFound, r.JSON(err))
        if let Some(outer) = n
            .parent()
            .filter(|p| p.kind() == "argument_list")
            .map(|p| children(p).collect::<Vec<_>>())
            && failed(&outer)
        {
            return None;
        }
        // WriteHeaderAndJson(status, value, contentType)
        if name == "WriteHeaderAndJson" {
            return args.get(1).copied();
        }
        args.last().copied()
    }

    /// Тип выражения-значения в теле обработчика: переменная, `&T{…}`, `T{…}`, вызов функции.
    fn value_type(
        &self,
        file: usize,
        body: Node<'a>,
        v: Node<'a>,
    ) -> Option<(usize, Node<'a>, Node<'a>)> {
        let src = &self.files[file].src;
        match v.kind() {
            "identifier" => self.var_type(file, body, text(v, src)),
            "unary_expression" => self.value_type(file, body, v.child_by_field_name("operand")?),
            "composite_literal" => Some((file, v.child_by_field_name("type")?, body)),
            "call_expression" => self.call_result(file, v),
            _ => None,
        }
    }

    /// Shape для типа Go: `string`, `integer`, `[Order]`, `{ id: integer }`, имя структуры.
    /// Именованные структуры становятся отдельными shape (`self.shapes`).
    fn shape_of(&self, file: usize, ty: Node<'a>, scope: Node<'a>, depth: usize) -> String {
        if depth > MAX_DEPTH {
            return "any".into();
        }
        let src = &self.files[file].src;
        match ty.kind() {
            "pointer_type" | "parenthesized_type" => ty
                .named_child(0)
                .map_or("any".into(), |t| self.shape_of(file, t, scope, depth + 1)),
            "slice_type" | "array_type" => match ty.child_by_field_name("element") {
                Some(el) if text(el, src) == "byte" => "string".into(),
                Some(el) => format!("[{}]", self.shape_of(file, el, scope, depth + 1)),
                None => "[any]".into(),
            },
            "map_type" => "{}".into(),
            "struct_type" => self.struct_shape(file, ty, scope, depth),
            "type_identifier" => match text(ty, src) {
                "string" => "string".into(),
                "bool" => "boolean".into(),
                "int" | "int8" | "int16" | "int32" | "int64" | "uint" | "uint8" | "uint16"
                | "uint32" | "uint64" | "byte" | "rune" => "integer".into(),
                "float32" | "float64" => "number".into(),
                "any" | "error" => "any".into(),
                _ => self.named_shape(file, ty, scope, depth),
            },
            "qualified_type" => match compact(text(ty, src)).as_str() {
                "time.Time" | "uuid.UUID" => "string".into(),
                "json.Number" => "number".into(),
                "json.RawMessage" => "any".into(),
                _ => self.named_shape(file, ty, scope, depth),
            },
            _ => "any".into(),
        }
    }

    /// Именованный тип: структура → shape с её именем, `type Status string` → `string`.
    fn named_shape(&self, file: usize, ty: Node<'a>, scope: Node<'a>, depth: usize) -> String {
        let Some((f, resolved)) = self.resolve(file, ty, scope, depth) else {
            return "any".into();
        };
        if resolved.id() == ty.id() {
            return "any".into(); // чужой пакет — не знаем
        }
        if resolved.kind() != "struct_type" {
            return self.shape_of(f, resolved, resolved, depth + 1);
        }
        let name = type_name(ty, &self.files[file].src);
        let go_type = format!("{}.{name}", self.files[f].package);
        let shape_name = {
            let mut shapes = self.shapes.borrow_mut();
            if let Some(d) = shapes.iter().find(|d| d.go_type == go_type) {
                return d.name.clone();
            }
            let mut shape_name = upper_first(&name);
            if shapes.iter().any(|d| d.name == shape_name) {
                shape_name = format!("{}{shape_name}", upper_first(&self.files[f].package));
            }
            shapes.push(ShapeDef {
                name: shape_name.clone(),
                go_type: go_type.clone(),
                source: self.files[f].path.clone(),
                line: resolved.start_position().row + 1,
                shape: String::new(),
            });
            shape_name
        };
        // Глубина считается заново: от циклов защищает реестр `self.shapes`, а вложенные
        // именованные структуры (`Page → Order → Item`) не должны превращаться в `any`.
        let shape = self.struct_shape(f, resolved, resolved, 0);
        if let Some(d) = self
            .shapes
            .borrow_mut()
            .iter_mut()
            .find(|d| d.go_type == go_type)
        {
            d.shape = shape;
        }
        shape_name
    }

    /// `{ id: integer, note?: string, parent: Order | null }`: `omitempty` — необязательное поле,
    /// указатель без него — может быть `null`.
    fn struct_shape(&self, file: usize, st: Node<'a>, scope: Node<'a>, depth: usize) -> String {
        let fields: Vec<String> = self
            .struct_fields(file, st, scope, depth)
            .into_iter()
            .map(|(k, optional, shape)| {
                let k = if is_shape_key(&k) {
                    k
                } else {
                    serde_json::to_string(&k).unwrap_or_default()
                };
                format!("{k}{}: {shape}", if optional { "?" } else { "" })
            })
            .collect();
        if fields.is_empty() {
            "{}".into()
        } else {
            format!("{{ {} }}", fields.join(", "))
        }
    }

    fn struct_fields(
        &self,
        file: usize,
        st: Node<'a>,
        scope: Node<'a>,
        depth: usize,
    ) -> Vec<(String, bool, String)> {
        let mut out = Vec::new();
        if depth > MAX_DEPTH {
            return out;
        }
        let Some(list) = children(st).find(|n| n.kind() == "field_declaration_list") else {
            return out;
        };
        let src = &self.files[file].src;
        for decl in children(list).filter(|n| n.kind() == "field_declaration") {
            let Some(fty) = decl.child_by_field_name("type") else {
                continue;
            };
            let tag = decl
                .child_by_field_name("tag")
                .map(|t| literal_raw(t, src))
                .unwrap_or_default();
            let names: Vec<String> = {
                let mut c = decl.walk();
                decl.children_by_field_name("name", &mut c)
                    .map(|n| text(n, src).to_string())
                    .collect()
            };
            let json = tag_value(&tag, "json");
            let mut opts = json.as_deref().unwrap_or("").split(',');
            let key = opts.next().unwrap_or("").to_string();
            let opts: Vec<&str> = opts.collect();
            if key == "-" {
                continue;
            }
            if names.is_empty() && key.is_empty() {
                let inner = if fty.kind() == "pointer_type" {
                    fty.named_child(0)
                } else {
                    Some(fty)
                };
                if let Some((f, r)) = inner.and_then(|t| self.resolve(file, t, scope, depth + 1))
                    && r.kind() == "struct_type"
                {
                    out.extend(self.struct_fields(f, r, r, depth + 1));
                }
                continue;
            }
            let names = if names.is_empty() {
                vec![type_name(fty, src)]
            } else {
                names
            };
            let optional = opts.contains(&"omitempty") || opts.contains(&"omitzero");
            let mut shape = if opts.contains(&"string") {
                "string".to_string()
            } else {
                self.shape_of(file, fty, scope, depth + 1)
            };
            if fty.kind() == "pointer_type" && !optional && shape != "any" {
                shape.push_str(" | null");
            }
            for name in names {
                if !name.starts_with(|c: char| c.is_ascii_uppercase()) {
                    continue;
                }
                let k = if key.is_empty() { name } else { key.clone() };
                out.push((k, optional, shape.clone()));
            }
        }
        out
    }

    /// Поля структуры `ty` (тег — первый найденный из `tags`) и пример значения.
    /// `strict` — только поля с одним из `tags` (go-zero: `path`, `form`, `json` в одной структуре).
    fn fields(
        &self,
        file: usize,
        ty: Node,
        scope: Node,
        tags: &[&str],
        strict: bool,
        depth: usize,
    ) -> Option<(Vec<Field>, Json)> {
        let (file, ty) = self.resolve(file, ty, scope, depth)?;
        let list = children(ty).find(|n| n.kind() == "field_declaration_list")?;
        let src = &self.files[file].src;
        let mut fields = Vec::new();
        let mut obj = Vec::new();
        for decl in children(list).filter(|n| n.kind() == "field_declaration") {
            let Some(fty) = decl.child_by_field_name("type") else {
                continue;
            };
            let tag = decl
                .child_by_field_name("tag")
                .map(|t| literal_raw(t, src))
                .unwrap_or_default();
            let names: Vec<String> = {
                let mut c = decl.walk();
                decl.children_by_field_name("name", &mut c)
                    .map(|n| text(n, src).to_string())
                    .collect()
            };
            let tagged = tags.iter().find_map(|t| tag_value(&tag, t));
            if strict && tagged.is_none() && !names.is_empty() {
                continue;
            }
            // Встроенная структура без имени в теге — её поля на этом же уровне.
            if names.is_empty() && tagged.is_none() {
                if let Some((inner, Json::Obj(kv))) =
                    self.fields(file, fty, scope, tags, strict, depth + 1)
                {
                    fields.extend(inner);
                    obj.extend(kv);
                }
                continue;
            }
            let names = if names.is_empty() {
                vec![type_name(fty, src)]
            } else {
                names
            };
            let comment = decl
                .next_sibling()
                .filter(|c| {
                    c.kind() == "comment" && c.start_position().row == decl.end_position().row
                })
                .map(|c| text(c, src).trim_start_matches('/').trim().to_string());
            let required = ["binding", "validate"]
                .iter()
                .filter_map(|t| tag_value(&tag, t))
                .any(|v| v.split(',').any(|p| p == "required"));
            for name in names {
                if !name.starts_with(|c: char| c.is_ascii_uppercase()) && tagged.is_none() {
                    continue;
                }
                let key = match tagged.as_deref().map(|t| t.split(',').next().unwrap_or("")) {
                    Some("-") => continue,
                    Some("") | None => name,
                    Some(k) => k.to_string(),
                };
                let sample = self.sample(file, fty, scope, depth + 1);
                let json = sample.json_type();
                obj.push((key.clone(), sample));
                // Вложенная структура — её поля через точку: `user.email`, `items[].id`.
                if let Some((suffix, subs)) = self.nested(file, fty, scope, tags, depth + 1) {
                    fields.extend(subs.into_iter().map(|f| Field {
                        name: format!("{key}{suffix}.{}", f.name),
                        ..f
                    }));
                    continue;
                }
                fields.push(Field {
                    name: key,
                    ty: compact(text(fty, src)),
                    required,
                    comment: comment.clone(),
                    json,
                });
            }
        }
        Some((fields, Json::Obj(obj)))
    }

    /// Поля вложенной структуры (`T`, `*T`, `[]T`, `struct{…}`); `[]` — для срезов.
    fn nested(
        &self,
        file: usize,
        ty: Node<'a>,
        scope: Node<'a>,
        tags: &[&str],
        depth: usize,
    ) -> Option<(&'static str, Vec<Field>)> {
        let (ty, suffix) = match ty.kind() {
            "pointer_type" => (ty.named_child(0)?, ""),
            "slice_type" | "array_type" => (ty.child_by_field_name("element")?, "[]"),
            _ => (ty, ""),
        };
        let ty = if ty.kind() == "pointer_type" {
            ty.named_child(0)?
        } else {
            ty
        };
        let (f, resolved) = self.resolve(file, ty, scope, depth)?;
        if resolved.kind() != "struct_type" {
            return None;
        }
        let (subs, _) = self.fields(f, resolved, scope, tags, false, depth)?;
        (!subs.is_empty()).then_some((suffix, subs))
    }

    /// Тип переменной в теле обработчика; для `v := NewValidator()` — тип результата функции.
    /// Возвращает файл, узел типа и область для локальных типов.
    fn var_type(
        &self,
        file: usize,
        body: Node<'a>,
        name: &str,
    ) -> Option<(usize, Node<'a>, Node<'a>)> {
        let src = &self.files[file].src;
        let v = var_type(body, name, src)?;
        if v.kind() != "call_expression" {
            return Some((file, v, body));
        }
        self.call_result(file, v)
    }

    /// Тип → его определение: `*T` → `T`, имя → `type T struct{…}` (в теле обработчика,
    /// в пакете файла или в указанном пакете). Возвращает файл и узел типа.
    fn resolve(
        &self,
        file: usize,
        ty: Node<'a>,
        scope: Node<'a>,
        depth: usize,
    ) -> Option<(usize, Node<'a>)> {
        if depth > MAX_DEPTH {
            return None;
        }
        let src = &self.files[file].src;
        match ty.kind() {
            "pointer_type" | "parenthesized_type" => {
                self.resolve(file, ty.named_child(0)?, scope, depth + 1)
            }
            "generic_type" => self.resolve(file, ty.child_by_field_name("type")?, scope, depth + 1),
            "type_identifier" => {
                let name = text(ty, src);
                if let Some(local) = local_type(scope, name, src) {
                    return self.resolve(file, local, scope, depth + 1);
                }
                // Встроенный или чужой тип — сам идентификатор.
                let Some(&(f, s, e)) = self
                    .types
                    .get(&(self.files[file].package.clone(), name.to_string()))
                else {
                    return Some((file, ty));
                };
                let spec = self.files[f]
                    .tree
                    .root_node()
                    .descendant_for_byte_range(s, e)?;
                self.resolve(f, spec.child_by_field_name("type")?, spec, depth + 1)
            }
            "qualified_type" => {
                let pkg = text(ty.child_by_field_name("package")?, src);
                let name = text(ty.child_by_field_name("name")?, src);
                let Some(&(f, s, e)) = self.types.get(&(pkg.to_string(), name.to_string())) else {
                    return Some((file, ty));
                };
                let spec = self.files[f]
                    .tree
                    .root_node()
                    .descendant_for_byte_range(s, e)?;
                self.resolve(f, spec.child_by_field_name("type")?, spec, depth + 1)
            }
            _ => Some((file, ty)),
        }
    }

    /// Пример значения типа для JSON-тела.
    fn sample(&self, file: usize, ty: Node, scope: Node, depth: usize) -> Json {
        if depth > MAX_DEPTH {
            return Json::Null;
        }
        let src = &self.files[file].src;
        match ty.kind() {
            "pointer_type" => ty
                .named_child(0)
                .map_or(Json::Null, |t| self.sample(file, t, scope, depth)),
            "slice_type" | "array_type" => {
                let Some(el) = ty.child_by_field_name("element") else {
                    return Json::Arr(vec![]);
                };
                if text(el, src) == "byte" {
                    return Json::Str(String::new());
                }
                match self.sample(file, el, scope, depth + 1) {
                    Json::Null => Json::Arr(vec![]),
                    v => Json::Arr(vec![v]),
                }
            }
            "map_type" => Json::Obj(vec![]),
            "struct_type" => self
                .fields(file, ty, scope, &["json"], false, depth)
                .map_or(Json::Null, |(_, v)| v),
            "type_identifier" => match text(ty, src) {
                "string" => Json::Str(String::new()),
                "bool" => Json::Bool,
                "int" | "int8" | "int16" | "int32" | "int64" | "uint" | "uint8" | "uint16"
                | "uint32" | "uint64" | "float32" | "float64" | "byte" | "rune" => Json::Num,
                "any" | "error" => Json::Null,
                _ => self.sample_named(file, ty, scope, depth),
            },
            "qualified_type" => match compact(text(ty, src)).as_str() {
                "time.Time" => Json::Str("2006-01-02T15:04:05Z".into()),
                "uuid.UUID" => Json::Str("00000000-0000-0000-0000-000000000000".into()),
                "json.RawMessage" => Json::Null,
                "json.Number" => Json::Num,
                _ => self.sample_named(file, ty, scope, depth),
            },
            _ => Json::Null,
        }
    }

    /// Именованный тип: структура → объект, `type Status string` → значение базового типа.
    fn sample_named(&self, file: usize, ty: Node, scope: Node, depth: usize) -> Json {
        match self.resolve(file, ty, scope, depth) {
            Some((f, t)) if t.kind() == "struct_type" => self
                .fields(f, t, scope, &["json"], false, depth)
                .map_or(Json::Null, |(_, v)| v),
            Some((f, t)) if t.kind() != ty.kind() || t.id() != ty.id() => {
                self.sample(f, t, scope, depth + 1)
            }
            _ => Json::Null,
        }
    }
}

/// Тело функции, в которой узел (область для переменных и локальных типов), иначе — корень.
fn scope_of(node: Node) -> Node {
    let mut n = node;
    while let Some(p) = n.parent() {
        if matches!(
            p.kind(),
            "function_declaration" | "method_declaration" | "func_literal"
        ) && let Some(body) = p.child_by_field_name("body")
        {
            return body;
        }
        n = p;
    }
    n
}

fn walk<'t>(node: Node<'t>, f: &mut impl FnMut(Node<'t>)) {
    f(node);
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        walk(child, f);
    }
}

fn push_unique(v: &mut Vec<String>, s: String) {
    if !v.contains(&s) {
        v.push(s);
    }
}

fn is_param_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '[' | ']'))
}

/// Значение строкового литерала.
fn literal(node: Node, src: &str) -> Option<String> {
    matches!(
        node.kind(),
        "interpreted_string_literal" | "raw_string_literal"
    )
    .then(|| literal_raw(node, src))
}

fn literal_raw(node: Node, src: &str) -> String {
    let t = text(node, src);
    match node.kind() {
        "raw_string_literal" => t.trim_matches('`').to_string(),
        _ => super::unquote(t),
    }
}

/// `json:"name,omitempty"` → `name,omitempty`.
fn tag_value(tag: &str, key: &str) -> Option<String> {
    let start = tag.find(&format!("{key}:\""))? + key.len() + 2;
    // Ключ должен начинаться с начала тега или после пробела: `xjson:` не подходит.
    let before = tag[..start - key.len() - 2].chars().last();
    if before.is_some_and(|c| !c.is_whitespace()) {
        return None;
    }
    let rest = &tag[start..];
    Some(rest[..rest.find('"')?].to_string())
}

/// `&req` или `req` в аргументах вызова.
fn bound_var(arg: Node, src: &str) -> Option<String> {
    match arg.kind() {
        "unary_expression" => {
            let op = arg.child_by_field_name("operator")?;
            let operand = arg.child_by_field_name("operand")?;
            (text(op, src) == "&" && operand.kind() == "identifier")
                .then(|| text(operand, src).to_string())
        }
        _ => None,
    }
}

/// Тип переменной `name` в теле: `var req T`, `req := T{}`, `req := &T{}`, `req := new(T)`.
fn var_type<'t>(body: Node<'t>, name: &str, src: &str) -> Option<Node<'t>> {
    let mut found = None;
    walk(body, &mut |n| {
        if found.is_some() {
            return;
        }
        match n.kind() {
            "var_spec" => {
                let mut c = n.walk();
                if n.children_by_field_name("name", &mut c)
                    .any(|id| text(id, src) == name)
                {
                    found = n.child_by_field_name("type").or_else(|| {
                        n.child_by_field_name("value")
                            .and_then(|v| v.named_child(0))
                            .and_then(|v| value_type(v, src))
                    });
                }
            }
            "short_var_declaration" => {
                let (Some(l), Some(r)) = (
                    n.child_by_field_name("left"),
                    n.child_by_field_name("right"),
                ) else {
                    return;
                };
                let left: Vec<Node> = children(l).collect();
                let right: Vec<Node> = children(r).collect();
                if let Some(i) = left.iter().position(|id| text(*id, src) == name) {
                    found = right.get(i).and_then(|v| value_type(*v, src));
                }
            }
            _ => {}
        }
    });
    found
}

/// Тип выражения-значения: `T{}`, `&T{}`, `new(T)`.
fn value_type<'t>(v: Node<'t>, src: &str) -> Option<Node<'t>> {
    match v.kind() {
        "composite_literal" => v.child_by_field_name("type"),
        "unary_expression" => value_type(v.child_by_field_name("operand")?, src),
        // new(T) → T; другой вызов — сам вызов, тип результата ищет `Describer::var_type`.
        "call_expression" => {
            let f = v.child_by_field_name("function")?;
            if text(f, src) == "new" {
                return v.child_by_field_name("arguments")?.named_child(0);
            }
            Some(v)
        }
        _ => None,
    }
}

/// `type req struct{…}` внутри функции.
fn local_type<'t>(scope: Node<'t>, name: &str, src: &str) -> Option<Node<'t>> {
    let mut found = None;
    walk(scope, &mut |n| {
        if found.is_none()
            && n.kind() == "type_spec"
            && n.child_by_field_name("name")
                .is_some_and(|id| text(id, src) == name)
        {
            found = n.child_by_field_name("type");
        }
    });
    found
}

/// Слова в имени вызова, числовой аргумент которого — код ответа: `c.JSON(404, …)`,
/// `w.WriteHeader(201)`, `c.AbortWithStatus(401)`, свой `respond(w, 409, …)`.
const STATUS_CALLS: &[&str] = &[
    "json",
    "status",
    "string",
    "error",
    "header",
    "render",
    "redirect",
    "nocontent",
    "xml",
    "html",
    "data",
    "respond",
    "reply",
    "write",
    "abort",
    "blob",
    "send",
    "entity",
];

/// Константы `net/http` (у fiber, iris — те же имена).
const STATUS_CONSTS: &[(&str, u16)] = &[
    ("StatusContinue", 100),
    ("StatusSwitchingProtocols", 101),
    ("StatusProcessing", 102),
    ("StatusEarlyHints", 103),
    ("StatusOK", 200),
    ("StatusCreated", 201),
    ("StatusAccepted", 202),
    ("StatusNonAuthoritativeInfo", 203),
    ("StatusNoContent", 204),
    ("StatusResetContent", 205),
    ("StatusPartialContent", 206),
    ("StatusMultiStatus", 207),
    ("StatusAlreadyReported", 208),
    ("StatusIMUsed", 226),
    ("StatusMultipleChoices", 300),
    ("StatusMovedPermanently", 301),
    ("StatusFound", 302),
    ("StatusSeeOther", 303),
    ("StatusNotModified", 304),
    ("StatusUseProxy", 305),
    ("StatusTemporaryRedirect", 307),
    ("StatusPermanentRedirect", 308),
    ("StatusBadRequest", 400),
    ("StatusUnauthorized", 401),
    ("StatusPaymentRequired", 402),
    ("StatusForbidden", 403),
    ("StatusNotFound", 404),
    ("StatusMethodNotAllowed", 405),
    ("StatusNotAcceptable", 406),
    ("StatusProxyAuthRequired", 407),
    ("StatusRequestTimeout", 408),
    ("StatusConflict", 409),
    ("StatusGone", 410),
    ("StatusLengthRequired", 411),
    ("StatusPreconditionFailed", 412),
    ("StatusRequestEntityTooLarge", 413),
    ("StatusRequestURITooLong", 414),
    ("StatusUnsupportedMediaType", 415),
    ("StatusRequestedRangeNotSatisfiable", 416),
    ("StatusExpectationFailed", 417),
    ("StatusTeapot", 418),
    ("StatusMisdirectedRequest", 421),
    ("StatusUnprocessableEntity", 422),
    ("StatusLocked", 423),
    ("StatusFailedDependency", 424),
    ("StatusTooEarly", 425),
    ("StatusUpgradeRequired", 426),
    ("StatusPreconditionRequired", 428),
    ("StatusTooManyRequests", 429),
    ("StatusRequestHeaderFieldsTooLarge", 431),
    ("StatusUnavailableForLegalReasons", 451),
    ("StatusInternalServerError", 500),
    ("StatusNotImplemented", 501),
    ("StatusBadGateway", 502),
    ("StatusServiceUnavailable", 503),
    ("StatusGatewayTimeout", 504),
    ("StatusHTTPVersionNotSupported", 505),
    ("StatusVariantAlsoNegotiates", 506),
    ("StatusInsufficientStorage", 507),
    ("StatusLoopDetected", 508),
    ("StatusNotExtended", 510),
    ("StatusNetworkAuthenticationRequired", 511),
];

fn status_const(name: &str) -> Option<u16> {
    STATUS_CONSTS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|&(_, c)| c)
}

/// Операнд — привязка fiber v3 `c.Bind()`: `.Body(&req)` читает тело, а `.JSON(&req)` — не ответ.
fn bind_chain(operand: &str) -> bool {
    operand.trim_end().ends_with("Bind()")
}

/// Код ответа — ошибка: `http.StatusBadRequest`, `fiber.StatusNotFound`, `500`.
fn error_status(arg: &str) -> bool {
    const OK: &[&str] = &[
        "StatusOK",
        "StatusCreated",
        "StatusAccepted",
        "StatusNonAuthoritativeInfo",
        "StatusNoContent",
        "StatusResetContent",
        "StatusPartialContent",
    ];
    if let Some((_, name)) = arg.rsplit_once('.') {
        return status_const(name).is_some() && !OK.contains(&name);
    }
    arg.parse::<u16>().is_ok_and(|n| n >= 400)
}

fn upper_first(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

fn is_shape_key(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn type_name(ty: Node, src: &str) -> String {
    let t = compact(text(ty, src));
    let t = t.trim_start_matches('*');
    t.rsplit('.').next().unwrap_or(t).to_string()
}

fn compact(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
