//! Что передавать в роут axum: разбор обработчика. Типы видны в сигнатуре: `Json<T>` — тело,
//! `Query<T>` — query, `Form<T>` — форма, `TypedHeader<T>` и чтение `HeaderMap` — заголовки,
//! ответ — `Json<T>` в типе результата (в том числе внутри `Result`, кортежа) или
//! `Json(значение)` в теле. Поля — по serde: `rename`, `rename_all`, `skip*`, `default`,
//! `flatten`, `Option<T>` — необязательное. Коды ответа — `StatusCode::X` в теле и варианты
//! ошибок, которым `impl IntoResponse` сопоставляет код.

use std::cell::RefCell;
use std::collections::HashMap;

use tree_sitter::Node;

use super::{
    Index, Item, attributes, children, compact, last_segment, segments, status_path,
    string_literal, text, walk_tree,
};
use crate::import::sample::Json;
use crate::import::{Body, Field, Response, RouteInfo, ShapeDef, StatusCode};

/// Глубина вложенных типов.
const MAX_DEPTH: usize = 8;
/// Строк описания из комментария.
const MAX_DOC: usize = 6;

const UUID: &str = "00000000-0000-0000-0000-000000000000";
const DATETIME: &str = "2024-01-01T00:00:00Z";

/// Тип значения в JSON.
#[derive(Clone, Debug)]
enum Ty {
    /// Строка; пример значения для известных типов (`Uuid`, даты)
    Str(Option<&'static str>),
    Int,
    Num,
    Bool,
    Any,
    Unit,
    Arr(Box<Ty>),
    Map,
    Opt(Box<Ty>),
    /// Структура с именованными полями: объявление и обобщённые аргументы
    Struct {
        item: usize,
        args: Vec<Ty>,
    },
}

/// Поле структуры с учётом serde.
struct FieldTy {
    key: String,
    ty: Ty,
    /// Тип в Rust
    ty_text: String,
    /// `skip_serializing_if` — в ответе может не быть
    optional: bool,
    /// `default` у поля или структуры — в запросе можно не передавать
    default: bool,
    skip_ser: bool,
    skip_de: bool,
    comment: Option<String>,
}

/// Где читать тип: файл, модуль, обобщённые параметры.
#[derive(Clone)]
struct Cx<'s> {
    file: usize,
    module: &'s [String],
    generics: &'s HashMap<String, Ty>,
}

pub(super) struct Describer<'i, 'f> {
    idx: &'i Index<'f>,
    shapes: RefCell<Vec<ShapeDef>>,
}

impl<'i, 'f> Describer<'i, 'f> {
    pub(super) fn new(idx: &'i Index<'f>) -> Self {
        Describer {
            idx,
            shapes: RefCell::default(),
        }
    }

    pub(super) fn into_shapes(self) -> Vec<ShapeDef> {
        self.shapes.into_inner()
    }

    fn src(&self, file: usize) -> &'f str {
        &self.idx.files[file].src
    }

    /// Описание обработчика `fns[f]`.
    pub(super) fn describe(&self, f: usize) -> RouteInfo {
        let item = &self.idx.fns[f];
        let mut info = RouteInfo::default();
        let Some(decl) = self.idx.node(item) else {
            return info;
        };
        let src = self.src(item.file);
        doc(decl, src, &mut info);
        let generics = HashMap::new();
        let cx = Cx {
            file: item.file,
            module: &item.module,
            generics: &generics,
        };
        let body = decl.child_by_field_name("body");
        if let Some(params) = decl.child_by_field_name("parameters") {
            for p in children(params).filter(|p| p.kind() == "parameter") {
                let (Some(ty), pat) = (
                    p.child_by_field_name("type"),
                    p.child_by_field_name("pattern"),
                ) else {
                    continue;
                };
                self.extractor(&cx, ty, pat, body, &mut info, 0);
            }
        }
        if let Some(ret) = decl.child_by_field_name("return_type")
            && let Some(t) = self.response_type(&cx, ret, 0)
        {
            self.set_response(&cx, t, &mut info);
        }
        if info.response.is_none()
            && let Some(body) = body
        {
            self.response_in_body(&cx, body, &mut info);
        }
        if let Some(body) = body {
            self.statuses(item, body, &mut info);
        }
        info
    }

    /// Параметр-экстрактор: `Json<T>`, `Query<T>`, `Form<T>`, `TypedHeader<T>`, `HeaderMap`;
    /// обёртки (`Option`, `WithRejection`, `Valid`, …) раскрываются.
    fn extractor(
        &self,
        cx: &Cx,
        ty: Node,
        pat: Option<Node>,
        body: Option<Node>,
        info: &mut RouteInfo,
        depth: usize,
    ) {
        if depth > 3 {
            return;
        }
        let src = self.src(cx.file);
        let ty = unref(ty);
        let (name, args) = generic(ty, src);
        let first = args.first().copied();
        match name.as_str() {
            "Path" | "State" | "Extension" | "ConnectInfo" | "Host" | "OriginalUri"
            | "MatchedPath" | "Method" | "Request" | "Multipart" | "CookieJar" | "Bytes"
            | "String" | "Uri" | "Version" | "RawQuery" | "RawForm" | "RawPathParams" => {}
            "TypedHeader" => {
                if let Some(h) = first {
                    push_unique(&mut info.headers, header_case(&last_segment(text(h, src))));
                }
            }
            "HeaderMap" => {
                if let (Some(pat), Some(body)) = (pat, body)
                    && pat.kind() == "identifier"
                {
                    headers_read(body, text(pat, src), src, &mut info.headers);
                }
            }
            n if n.ends_with("Json") => {
                if let Some(t) = first {
                    let ty = self.ty(cx, t, 0);
                    if let Some((fields, example)) = self.fields(&ty, false, 0) {
                        info.body = Some(Body {
                            type_name: compact(text(t, src)),
                            fields,
                            example: example.pretty(),
                        });
                    }
                }
            }
            n if n.ends_with("Query") => {
                if let Some(t) = first {
                    let ty = self.ty(cx, t, 0);
                    if let Some((fields, _)) = self.fields(&ty, true, 0) {
                        info.query = fields;
                    }
                }
            }
            n if n.ends_with("Form") => {
                if let Some(t) = first {
                    let ty = self.ty(cx, t, 0);
                    if let Some((fields, _)) = self.fields(&ty, true, 0) {
                        info.form = fields;
                    }
                }
            }
            _ => {
                if let Some(t) = first {
                    self.extractor(cx, t, None, body, info, depth + 1);
                }
            }
        }
    }

    /// Тип ответа из типа результата: `Json<T>`, `Result<Json<T>, E>`, `(StatusCode, Json<T>)`.
    fn response_type<'n>(&self, cx: &Cx, ty: Node<'n>, depth: usize) -> Option<Node<'n>> {
        if depth > 4 {
            return None;
        }
        let src = self.src(cx.file);
        let ty = unref(ty);
        if ty.kind() == "tuple_type" {
            return children(ty).find_map(|t| self.response_type(cx, t, depth + 1));
        }
        let (name, args) = generic(ty, src);
        let first = *args.first()?;
        if name.ends_with("Json") {
            return Some(first);
        }
        if name == "Option" || name.ends_with("Result") {
            return self.response_type(cx, first, depth + 1);
        }
        None
    }

    fn set_response(&self, cx: &Cx, t: Node, info: &mut RouteInfo) {
        let ty = self.ty(cx, t, 0);
        let shape = self.shape(&ty, 0);
        if shape == "any" || shape == "{}" {
            return;
        }
        info.response = Some(Response {
            type_name: compact(text(t, self.src(cx.file))),
            shape,
        });
    }

    /// `impl IntoResponse`: первый `Json(значение)` в теле не рядом с кодом ошибки; тип —
    /// структура литерала или аннотация переменной.
    fn response_in_body(&self, cx: &Cx, body: Node, info: &mut RouteInfo) {
        let src = self.src(cx.file);
        let mut found: Option<Node> = None;
        walk_tree(body, &mut |n| {
            if found.is_some() || n.kind() != "call_expression" {
                return;
            }
            let Some(func) = n.child_by_field_name("function") else {
                return;
            };
            if !matches!(func.kind(), "identifier" | "scoped_identifier")
                || last_segment(text(func, src)) != "Json"
            {
                return;
            }
            if n.parent()
                .filter(|p| p.kind() == "tuple_expression")
                .is_some_and(|p| {
                    children(p).any(|c| status_path(text(c, src)).is_some_and(|code| code >= 400))
                })
            {
                return;
            }
            let Some(arg) = n
                .child_by_field_name("arguments")
                .and_then(|a| a.named_child(0))
            else {
                return;
            };
            found = match arg.kind() {
                "struct_expression" => arg.child_by_field_name("name"),
                "identifier" => let_type(body, text(arg, src), arg.start_byte(), src),
                _ => None,
            };
        });
        if let Some(t) = found {
            self.set_response(cx, t, info);
        }
    }

    /// Коды ответа: `StatusCode::X`, `StatusCode::from_u16(N)`, `Redirect::to` и варианты
    /// ошибок (`AppError::NotFound`), если их `IntoResponse` сопоставляет код.
    fn statuses(&self, item: &Item, body: Node, info: &mut RouteInfo) {
        let file = &self.idx.files[item.file];
        let src = &file.src;
        let mut found: Vec<StatusCode> = Vec::new();
        let mut add = |code: u16, n: Node| {
            if !found.iter().any(|s| s.code == code) {
                found.push(StatusCode {
                    code,
                    source: file.path.clone(),
                    line: n.start_position().row + 1,
                    implicit: false,
                });
            }
        };
        walk_tree(body, &mut |n| match n.kind() {
            "scoped_identifier" | "scoped_type_identifier" => {
                let t = text(n, src);
                if let Some(code) = status_path(t) {
                    return add(code, n);
                }
                let segs = segments(t);
                if let [.., owner, name] = segs.as_slice() {
                    if owner == "Redirect" {
                        match name.as_str() {
                            "to" => add(303, n),
                            "temporary" => add(307, n),
                            "permanent" => add(308, n),
                            _ => {}
                        }
                    } else if let Some(code) = self
                        .idx
                        .errors
                        .get(owner)
                        .and_then(|vs| vs.iter().find(|(v, _)| v == name))
                        .map(|&(_, c)| c)
                    {
                        add(code, n);
                    }
                }
            }
            "call_expression" => {
                let Some(func) = n.child_by_field_name("function") else {
                    return;
                };
                if !text(func, src).ends_with("StatusCode::from_u16") {
                    return;
                }
                if let Some(code) = n
                    .child_by_field_name("arguments")
                    .and_then(|a| a.named_child(0))
                    .and_then(|a| text(a, src).parse::<u16>().ok())
                    .filter(|c| (100..600).contains(c))
                {
                    add(code, n);
                }
            }
            _ => {}
        });
        if !found.iter().any(|s| (200..300).contains(&s.code)) {
            found.push(StatusCode {
                code: 200,
                source: file.path.clone(),
                line: body.start_position().row + 1,
                implicit: true,
            });
        }
        found.sort_by_key(|s| s.code);
        info.statuses = found;
    }

    /// Тип Rust → тип JSON.
    fn ty(&self, cx: &Cx, node: Node, depth: usize) -> Ty {
        if depth > MAX_DEPTH {
            return Ty::Any;
        }
        let src = self.src(cx.file);
        match node.kind() {
            "reference_type" | "pointer_type" => node
                .child_by_field_name("type")
                .map_or(Ty::Any, |t| self.ty(cx, t, depth + 1)),
            "primitive_type" => primitive(text(node, src)),
            "unit_type" => Ty::Unit,
            "array_type" => match node.child_by_field_name("element") {
                Some(el) if text(el, src) == "u8" => Ty::Arr(Box::new(Ty::Int)),
                Some(el) => Ty::Arr(Box::new(self.ty(cx, el, depth + 1))),
                None => Ty::Arr(Box::new(Ty::Any)),
            },
            "tuple_type" => Ty::Arr(Box::new(Ty::Any)),
            "type_identifier" | "scoped_type_identifier" => {
                self.named(cx, &segments(text(node, src)), &[], depth)
            }
            "generic_type" => {
                let Some(base) = node.child_by_field_name("type") else {
                    return Ty::Any;
                };
                let args = type_args(node);
                self.named(cx, &segments(text(base, src)), &args, depth)
            }
            _ => Ty::Any,
        }
    }

    fn named(&self, cx: &Cx, segs: &[String], args: &[Node], depth: usize) -> Ty {
        let Some(name) = segs.last() else {
            return Ty::Any;
        };
        if segs.len() == 1
            && let Some(t) = cx.generics.get(name)
        {
            return t.clone();
        }
        let arg = |i: usize| args.get(i).map_or(Ty::Any, |n| self.ty(cx, *n, depth + 1));
        match name.as_str() {
            "Option" => return Ty::Opt(Box::new(arg(0))),
            "Vec" | "VecDeque" | "HashSet" | "BTreeSet" | "IndexSet" | "LinkedList"
            | "BinaryHeap" | "SmallVec" => return Ty::Arr(Box::new(arg(0))),
            "HashMap" | "BTreeMap" | "IndexMap" => return Ty::Map,
            "Box" | "Arc" | "Rc" | "Cow" | "Json" => return arg(0),
            "String" | "str" | "char" => return Ty::Str(None),
            _ => {}
        }
        if let Some(i) = self.idx.resolve(&self.idx.types, cx.file, cx.module, segs) {
            return self.user_type(
                i,
                args.iter().map(|n| self.ty(cx, *n, depth + 1)).collect(),
                depth,
            );
        }
        match name.as_str() {
            "Uuid" | "Ulid" => Ty::Str(Some(UUID)),
            "DateTime" | "NaiveDateTime" | "OffsetDateTime" | "PrimitiveDateTime" | "Timestamp"
            | "Zoned" => Ty::Str(Some(DATETIME)),
            "NaiveDate" | "Date" => Ty::Str(Some("2024-01-01")),
            "NaiveTime" | "Time" => Ty::Str(Some("00:00:00")),
            "PathBuf" | "Url" | "Uri" | "IpAddr" | "Ipv4Addr" | "Ipv6Addr" | "SocketAddr"
            | "Decimal" | "BigDecimal" | "Email" | "SecretString" => Ty::Str(None),
            "Map" => Ty::Map,
            n if n.starts_with("NonZero") => Ty::Int,
            _ => Ty::Any,
        }
    }

    /// Объявленный в проекте тип: структура, перечисление, псевдоним.
    fn user_type(&self, i: usize, args: Vec<Ty>, depth: usize) -> Ty {
        let item = &self.idx.types[i];
        let Some(node) = self.idx.node(item) else {
            return Ty::Any;
        };
        let src = self.src(item.file);
        let generics = self.generics(item, node, &args, depth);
        let cx = Cx {
            file: item.file,
            module: &item.module,
            generics: &generics,
        };
        let serde = serde_attrs(&attributes(node, src));
        match node.kind() {
            "type_item" => node
                .child_by_field_name("type")
                .map_or(Ty::Any, |t| self.ty(&cx, t, depth + 1)),
            "struct_item" => match node.child_by_field_name("body") {
                Some(b) if b.kind() == "field_declaration_list" => {
                    if serde.iter().any(|(k, _)| k == "transparent") {
                        return children(b)
                            .find(|f| f.kind() == "field_declaration")
                            .and_then(|f| f.child_by_field_name("type"))
                            .map_or(Ty::Any, |t| self.ty(&cx, t, depth + 1));
                    }
                    Ty::Struct { item: i, args }
                }
                // `struct UserId(i64)` сериализуется как `i64`.
                Some(b) => {
                    let types: Vec<Node> = children(b)
                        .filter(|n| {
                            n.kind() != "visibility_modifier" && n.kind() != "attribute_item"
                        })
                        .collect();
                    match types.as_slice() {
                        [one] => self.ty(&cx, *one, depth + 1),
                        _ => Ty::Arr(Box::new(Ty::Any)),
                    }
                }
                None => Ty::Any,
            },
            "enum_item" => {
                let unit = node.child_by_field_name("body").is_some_and(|b| {
                    children(b)
                        .filter(|v| v.kind() == "enum_variant")
                        .all(|v| v.child_by_field_name("body").is_none())
                });
                if unit && !serde.iter().any(|(k, _)| k == "untagged") {
                    Ty::Str(None)
                } else {
                    Ty::Any
                }
            }
            _ => Ty::Any,
        }
    }

    /// Обобщённые параметры объявления → аргументы использования (или `T = Default`).
    fn generics(&self, item: &Item, decl: Node, args: &[Ty], depth: usize) -> HashMap<String, Ty> {
        let mut out = HashMap::new();
        let Some(params) = decl.child_by_field_name("type_parameters") else {
            return out;
        };
        let src = self.src(item.file);
        let params: Vec<Node> = children(params)
            .filter(|p| p.kind() == "type_parameter")
            .collect();
        for (i, p) in params.into_iter().enumerate() {
            let Some(name) = p.child_by_field_name("name") else {
                continue;
            };
            let ty = match (args.get(i), p.child_by_field_name("default_type")) {
                (Some(t), _) => t.clone(),
                (None, Some(d)) => {
                    let cx = Cx {
                        file: item.file,
                        module: &item.module,
                        generics: &out,
                    };
                    self.ty(&cx, d, depth + 1)
                }
                (None, None) => Ty::Any,
            };
            out.insert(text(name, src).to_string(), ty);
        }
        out
    }

    /// Поля структуры по serde (`flatten` раскрыт).
    fn struct_fields(&self, i: usize, args: &[Ty], depth: usize) -> Vec<FieldTy> {
        let mut out = Vec::new();
        if depth > MAX_DEPTH {
            return out;
        }
        let item = &self.idx.types[i];
        let Some(node) = self.idx.node(item) else {
            return out;
        };
        let Some(list) = node.child_by_field_name("body") else {
            return out;
        };
        let src = self.src(item.file);
        let generics = self.generics(item, node, args, depth);
        let cx = Cx {
            file: item.file,
            module: &item.module,
            generics: &generics,
        };
        let container = serde_attrs(&attributes(node, src));
        let rename_all = container
            .iter()
            .find(|(k, _)| k == "rename_all")
            .and_then(|(_, v)| v.clone());
        let container_default = container.iter().any(|(k, _)| k == "default");
        for decl in children(list).filter(|n| n.kind() == "field_declaration") {
            let (Some(name), Some(fty)) = (
                decl.child_by_field_name("name"),
                decl.child_by_field_name("type"),
            ) else {
                continue;
            };
            let attrs = serde_attrs(&attributes(decl, src));
            let has = |k: &str| attrs.iter().any(|(a, _)| a == k);
            if has("skip") {
                continue;
            }
            let ty = self.ty(&cx, fty, depth + 1);
            if has("flatten") {
                if let Ty::Struct { item, args } = &ty {
                    out.extend(self.struct_fields(*item, args, depth + 1));
                }
                continue;
            }
            let name = text(name, src).trim_start_matches("r#");
            let key = match attrs
                .iter()
                .find(|(k, _)| k == "rename")
                .and_then(|(_, v)| v.clone())
            {
                Some(k) => k,
                None => rename_all
                    .as_deref()
                    .map_or_else(|| name.to_string(), |r| rename(name, r)),
            };
            out.push(FieldTy {
                key,
                ty_text: compact(text(fty, src)),
                optional: has("skip_serializing_if"),
                default: container_default || has("default"),
                skip_ser: has("skip_serializing"),
                skip_de: has("skip_deserializing"),
                comment: field_comment(decl, src),
                ty,
            });
        }
        out
    }

    /// Shape: `string`, `[Order]`, `{ id: integer }`, имя структуры (её shape — отдельно).
    fn shape(&self, ty: &Ty, depth: usize) -> String {
        if depth > MAX_DEPTH {
            return "any".into();
        }
        match ty {
            Ty::Str(_) => "string".into(),
            Ty::Int => "integer".into(),
            Ty::Num => "number".into(),
            Ty::Bool => "boolean".into(),
            Ty::Any | Ty::Unit => "any".into(),
            Ty::Map => "{}".into(),
            Ty::Arr(t) => format!("[{}]", self.shape(t, depth + 1)),
            Ty::Opt(t) => match self.shape(t, depth + 1) {
                s if s == "any" => s,
                s => format!("{s} | null"),
            },
            Ty::Struct { item, args } if args.is_empty() => self.named_shape(*item),
            Ty::Struct { item, args } => self.struct_shape(*item, args, depth + 1),
        }
    }

    /// Структура без обобщённых параметров — отдельный `shape` с её именем.
    fn named_shape(&self, i: usize) -> String {
        let item = &self.idx.types[i];
        let rust_type = item.path().collect::<Vec<_>>().join("::");
        {
            let mut shapes = self.shapes.borrow_mut();
            if let Some(d) = shapes.iter().find(|d| d.go_type == rust_type) {
                return d.name.clone();
            }
            let mut name = upper_first(&item.name);
            if shapes.iter().any(|d| d.name == name) {
                name = format!(
                    "{}{name}",
                    upper_first(item.module.last().map_or("", String::as_str))
                );
            }
            let line = self
                .idx
                .node(item)
                .map_or(1, |n| n.start_position().row + 1);
            shapes.push(ShapeDef {
                name,
                go_type: rust_type.clone(),
                source: self.idx.files[item.file].path.clone(),
                line,
                shape: String::new(),
            });
        }
        // Реестр уже знает тип — циклы (`Node { children: Vec<Node> }`) не уйдут в рекурсию.
        let shape = self.struct_shape(i, &[], 0);
        let mut shapes = self.shapes.borrow_mut();
        let d = shapes
            .iter_mut()
            .find(|d| d.go_type == rust_type)
            .expect("registered above");
        d.shape = shape;
        d.name.clone()
    }

    fn struct_shape(&self, i: usize, args: &[Ty], depth: usize) -> String {
        let fields: Vec<String> = self
            .struct_fields(i, args, depth)
            .into_iter()
            .filter(|f| !f.skip_ser)
            .map(|f| {
                let k = if is_shape_key(&f.key) {
                    f.key.clone()
                } else {
                    serde_json::to_string(&f.key).unwrap_or_default()
                };
                let opt = if f.optional { "?" } else { "" };
                format!("{k}{opt}: {}", self.shape(&f.ty, depth + 1))
            })
            .collect();
        if fields.is_empty() {
            "{}".into()
        } else {
            format!("{{ {} }}", fields.join(", "))
        }
    }

    /// Поля для запроса (`skip_deserializing` — нет) и пример значения. Вложенные структуры —
    /// через точку: `user.email`, `items[].id`; `flat` — без вложенных (query, форма).
    fn fields(&self, ty: &Ty, flat: bool, depth: usize) -> Option<(Vec<Field>, Json)> {
        let Ty::Struct { item, args } = ty else {
            return None;
        };
        let mut fields = Vec::new();
        let mut obj = Vec::new();
        for f in self
            .struct_fields(*item, args, depth)
            .into_iter()
            .filter(|f| !f.skip_de)
        {
            let sample = self.sample(&f.ty, depth + 1);
            let json = sample.json_type();
            obj.push((f.key.clone(), sample));
            if !flat {
                let (inner, suffix) = match &f.ty {
                    Ty::Opt(t) => match t.as_ref() {
                        Ty::Arr(t) => (t.as_ref(), "[]"),
                        t => (t, ""),
                    },
                    Ty::Arr(t) => (t.as_ref(), "[]"),
                    t => (t, ""),
                };
                if matches!(inner, Ty::Struct { .. })
                    && depth < MAX_DEPTH
                    && let Some((subs, _)) = self.fields(inner, false, depth + 1)
                    && !subs.is_empty()
                {
                    fields.extend(subs.into_iter().map(|s| Field {
                        name: format!("{}{suffix}.{}", f.key, s.name),
                        ..s
                    }));
                    continue;
                }
            }
            fields.push(Field {
                name: f.key,
                required: !matches!(f.ty, Ty::Opt(_)) && !f.default,
                ty: f.ty_text,
                comment: f.comment,
                json,
            });
        }
        Some((fields, Json::Obj(obj)))
    }

    fn sample(&self, ty: &Ty, depth: usize) -> Json {
        if depth > MAX_DEPTH {
            return Json::Null;
        }
        match ty {
            Ty::Str(ex) => Json::Str(ex.unwrap_or_default().to_string()),
            Ty::Int | Ty::Num => Json::Num,
            Ty::Bool => Json::Bool,
            Ty::Any | Ty::Unit => Json::Null,
            Ty::Map => Json::Obj(Vec::new()),
            Ty::Arr(t) => match self.sample(t, depth + 1) {
                Json::Null => Json::Arr(Vec::new()),
                v => Json::Arr(vec![v]),
            },
            Ty::Opt(t) => self.sample(t, depth + 1),
            Ty::Struct { .. } => self
                .fields(ty, true, depth + 1)
                .map_or(Json::Null, |(_, v)| v),
        }
    }
}

/// `&T`, `&mut T` → `T`.
fn unref(ty: Node) -> Node {
    if ty.kind() == "reference_type"
        && let Some(t) = ty.child_by_field_name("type")
    {
        return unref(t);
    }
    ty
}

/// Имя типа (последний сегмент) и обобщённые аргументы-типы.
fn generic<'n>(ty: Node<'n>, src: &str) -> (String, Vec<Node<'n>>) {
    match ty.kind() {
        "generic_type" => (
            ty.child_by_field_name("type")
                .map(|b| last_segment(text(b, src)))
                .unwrap_or_default(),
            type_args(ty),
        ),
        _ => (last_segment(text(ty, src)), Vec::new()),
    }
}

/// Аргументы-типы `generic_type` без времён жизни и констант.
fn type_args(ty: Node) -> Vec<Node> {
    ty.child_by_field_name("type_arguments")
        .map(|a| {
            children(a)
                .filter(|n| !matches!(n.kind(), "lifetime" | "integer_literal" | "block"))
                .collect()
        })
        .unwrap_or_default()
}

fn primitive(name: &str) -> Ty {
    match name {
        "bool" => Ty::Bool,
        "str" | "char" => Ty::Str(None),
        "f32" | "f64" => Ty::Num,
        n if n.starts_with('i') || n.starts_with('u') => Ty::Int,
        _ => Ty::Any,
    }
}

/// Тип переменной `name` из `let name: T = …` или `let name = T { … }` до `at`.
fn let_type<'n>(body: Node<'n>, name: &str, at: usize, src: &str) -> Option<Node<'n>> {
    let mut found = None;
    walk_tree(body, &mut |n| {
        if n.kind() != "let_declaration" || n.end_byte() > at {
            return;
        }
        if n.child_by_field_name("pattern")
            .is_none_or(|p| text(p, src) != name)
        {
            return;
        }
        found = n.child_by_field_name("type").or_else(|| {
            n.child_by_field_name("value")
                .filter(|v| v.kind() == "struct_expression")
                .and_then(|v| v.child_by_field_name("name"))
        });
    });
    found
}

/// Заголовки, которые читают из `HeaderMap`: `headers.get("x-api-key")`,
/// `headers.get(header::AUTHORIZATION)`.
fn headers_read(body: Node, var: &str, src: &str, out: &mut Vec<String>) {
    walk_tree(body, &mut |n| {
        if n.kind() != "call_expression" {
            return;
        }
        let Some(func) = n
            .child_by_field_name("function")
            .filter(|f| f.kind() == "field_expression")
        else {
            return;
        };
        let (Some(recv), Some(method)) = (
            func.child_by_field_name("value"),
            func.child_by_field_name("field"),
        ) else {
            return;
        };
        if text(recv, src) != var
            || !matches!(text(method, src), "get" | "get_all" | "contains_key")
        {
            return;
        }
        let Some(arg) = n
            .child_by_field_name("arguments")
            .and_then(|a| a.named_child(0))
        else {
            return;
        };
        let name = match arg.kind() {
            "string_literal" | "raw_string_literal" => string_literal(arg, src),
            "identifier" | "scoped_identifier" => {
                let c = last_segment(text(arg, src));
                c.chars()
                    .all(|ch| ch.is_ascii_uppercase() || ch == '_' || ch.is_ascii_digit())
                    .then(|| {
                        c.split('_')
                            .map(|w| upper_first(&w.to_ascii_lowercase()))
                            .collect::<Vec<_>>()
                            .join("-")
                    })
            }
            _ => None,
        };
        if let Some(name) = name.filter(|s| !s.is_empty()) {
            push_unique(out, name);
        }
    });
}

/// `UserAgent` → `User-Agent`, `XRequestId` → `X-Request-Id`.
fn header_case(ty: &str) -> String {
    let mut out = String::new();
    for (i, c) in ty.chars().enumerate() {
        if i > 0 && c.is_ascii_uppercase() {
            out.push('-');
        }
        out.push(c);
    }
    out
}

/// `///` над объявлением: первая строка — имя, остальное — описание.
fn doc(decl: Node, src: &str, info: &mut RouteInfo) {
    let mut lines = doc_lines(decl, src);
    lines.retain(|l| !l.is_empty());
    if lines.is_empty() {
        return;
    }
    // Короткое `/// List users.` — это имя запроса, а не предложение.
    let first = lines.remove(0);
    let short = first.split_whitespace().count() <= 4 && first.matches('.').count() == 1;
    info.summary = Some(match first.strip_suffix('.') {
        Some(s) if short => s.to_string(),
        _ => first,
    });
    lines.truncate(MAX_DOC);
    info.description = lines;
}

fn doc_lines(decl: Node, src: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut prev = decl.prev_sibling();
    while let Some(p) = prev {
        match p.kind() {
            "attribute_item" => {}
            "line_comment" => {
                let t = text(p, src);
                match t.strip_prefix("///").filter(|_| !t.starts_with("////")) {
                    Some(l) => lines.push(l.trim().to_string()),
                    None => break,
                }
            }
            "block_comment" => {
                let t = text(p, src);
                let Some(inner) = t.strip_prefix("/**").and_then(|t| t.strip_suffix("*/")) else {
                    break;
                };
                for l in inner.lines().rev() {
                    lines.push(l.trim().trim_start_matches('*').trim().to_string());
                }
            }
            _ => break,
        }
        prev = p.prev_sibling();
    }
    lines.reverse();
    lines
}

/// Комментарий поля: `///` над ним или `//` в той же строке.
fn field_comment(decl: Node, src: &str) -> Option<String> {
    let doc: Vec<String> = doc_lines(decl, src)
        .into_iter()
        .filter(|l| !l.is_empty())
        .collect();
    if let Some(first) = doc.into_iter().next() {
        return Some(first);
    }
    let mut next = decl.next_sibling();
    while let Some(n) = next {
        if n.start_position().row != decl.end_position().row {
            return None;
        }
        if n.kind() == "line_comment" {
            let t = text(n, src).trim_start_matches('/').trim();
            return (!t.is_empty()).then(|| t.to_string());
        }
        next = n.next_sibling();
    }
    None
}

/// Пары `ключ [= "значение"]` из атрибутов `serde(…)`. `rename(serialize = "a")` —
/// значение для сериализации.
fn serde_attrs(attrs: &[String]) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    for a in attrs {
        let Some(inner) = a
            .strip_prefix("serde")
            .map(str::trim)
            .and_then(|s| s.strip_prefix('('))
            .and_then(|s| s.strip_suffix(')'))
        else {
            continue;
        };
        for part in split_top(inner) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Some((k, v)) = part.split_once('=')
                && !k.contains('(')
            {
                out.push((k.trim().to_string(), Some(unquote(v.trim()))));
            } else if let Some((k, rest)) = part.split_once('(') {
                let rest = rest.trim_end_matches(')');
                let value = split_top(rest).into_iter().find_map(|p| {
                    let (k, v) = p.split_once('=')?;
                    (k.trim() == "serialize").then(|| unquote(v.trim()))
                });
                out.push((k.trim().to_string(), value));
            } else {
                out.push((part.to_string(), None));
            }
        }
    }
    out
}

/// Делит по запятым верхнего уровня (не внутри скобок и строк).
fn split_top(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut depth, mut quoted) = (0i32, false);
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            '(' | '[' if !quoted => depth += 1,
            ')' | ']' if !quoted => depth -= 1,
            ',' if !quoted && depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur);
    out
}

fn unquote(s: &str) -> String {
    s.trim_matches('"').to_string()
}

/// Имя поля `snake_case` по правилу `rename_all`.
fn rename(name: &str, rule: &str) -> String {
    let words: Vec<String> = name
        .split('_')
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    match rule {
        "lowercase" => name.to_ascii_lowercase(),
        "UPPERCASE" => name.to_ascii_uppercase(),
        "PascalCase" => words.iter().map(|w| upper_first(w)).collect(),
        "camelCase" => words
            .iter()
            .enumerate()
            .map(|(i, w)| if i == 0 { w.clone() } else { upper_first(w) })
            .collect(),
        "SCREAMING_SNAKE_CASE" => words.join("_").to_ascii_uppercase(),
        "kebab-case" => words.join("-"),
        "SCREAMING-KEBAB-CASE" => words.join("-").to_ascii_uppercase(),
        _ => name.to_string(),
    }
}

fn push_unique(v: &mut Vec<String>, s: String) {
    if !v.iter().any(|x| x.eq_ignore_ascii_case(&s)) {
        v.push(s);
    }
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

/// `StatusCode::NOT_FOUND` → 404 (константы `http::StatusCode`).
pub(super) fn status_const(name: &str) -> Option<u16> {
    const CODES: &[(&str, u16)] = &[
        ("CONTINUE", 100),
        ("SWITCHING_PROTOCOLS", 101),
        ("PROCESSING", 102),
        ("OK", 200),
        ("CREATED", 201),
        ("ACCEPTED", 202),
        ("NON_AUTHORITATIVE_INFORMATION", 203),
        ("NO_CONTENT", 204),
        ("RESET_CONTENT", 205),
        ("PARTIAL_CONTENT", 206),
        ("MULTI_STATUS", 207),
        ("ALREADY_REPORTED", 208),
        ("IM_USED", 226),
        ("MULTIPLE_CHOICES", 300),
        ("MOVED_PERMANENTLY", 301),
        ("FOUND", 302),
        ("SEE_OTHER", 303),
        ("NOT_MODIFIED", 304),
        ("USE_PROXY", 305),
        ("TEMPORARY_REDIRECT", 307),
        ("PERMANENT_REDIRECT", 308),
        ("BAD_REQUEST", 400),
        ("UNAUTHORIZED", 401),
        ("PAYMENT_REQUIRED", 402),
        ("FORBIDDEN", 403),
        ("NOT_FOUND", 404),
        ("METHOD_NOT_ALLOWED", 405),
        ("NOT_ACCEPTABLE", 406),
        ("PROXY_AUTHENTICATION_REQUIRED", 407),
        ("REQUEST_TIMEOUT", 408),
        ("CONFLICT", 409),
        ("GONE", 410),
        ("LENGTH_REQUIRED", 411),
        ("PRECONDITION_FAILED", 412),
        ("PAYLOAD_TOO_LARGE", 413),
        ("URI_TOO_LONG", 414),
        ("UNSUPPORTED_MEDIA_TYPE", 415),
        ("RANGE_NOT_SATISFIABLE", 416),
        ("EXPECTATION_FAILED", 417),
        ("IM_A_TEAPOT", 418),
        ("MISDIRECTED_REQUEST", 421),
        ("UNPROCESSABLE_ENTITY", 422),
        ("LOCKED", 423),
        ("FAILED_DEPENDENCY", 424),
        ("TOO_EARLY", 425),
        ("UPGRADE_REQUIRED", 426),
        ("PRECONDITION_REQUIRED", 428),
        ("TOO_MANY_REQUESTS", 429),
        ("REQUEST_HEADER_FIELDS_TOO_LARGE", 431),
        ("UNAVAILABLE_FOR_LEGAL_REASONS", 451),
        ("INTERNAL_SERVER_ERROR", 500),
        ("NOT_IMPLEMENTED", 501),
        ("BAD_GATEWAY", 502),
        ("SERVICE_UNAVAILABLE", 503),
        ("GATEWAY_TIMEOUT", 504),
        ("HTTP_VERSION_NOT_SUPPORTED", 505),
        ("VARIANT_ALSO_NEGOTIATES", 506),
        ("INSUFFICIENT_STORAGE", 507),
        ("LOOP_DETECTED", 508),
        ("NOT_EXTENDED", 510),
        ("NETWORK_AUTHENTICATION_REQUIRED", 511),
    ];
    CODES.iter().find(|(n, _)| *n == name).map(|&(_, c)| c)
}
