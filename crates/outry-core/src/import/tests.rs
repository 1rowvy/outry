use super::*;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("outry-import-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scan(dir: &TempDir) -> Scan {
    go::scan(&dir.0, &go::RouterQuery::builtin()).unwrap()
}

fn routes(scan: &Scan) -> Vec<String> {
    scan.routes
        .iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect()
}

const CHI_MAIN: &str = r#"package main

import (
	"net/http"

	"github.com/go-chi/chi/v5"
	"example.com/svc/internal/users"
)

const apiPrefix = "/api"

func main() {
	r := chi.NewRouter()
	r.Get("/health", health)
	r.Route(apiPrefix+"/v1", func(r chi.Router) {
		r.Route("/articles", func(r chi.Router) {
			r.Get("/", listArticles)
			r.With(paginate).Post("/", createArticle)
			r.Route("/{articleID:[0-9]+}", func(r chi.Router) {
				r.Get("/", getArticle)
				r.Method(http.MethodPut, "/", updateArticle)
			})
		})
		r.Mount("/users", users.Routes())
	})
	r.Mount("/admin", adminRouter())
	admin2 := chi.NewRouter()
	admin2.Delete("/cache", flush)
	r.Mount("/ops", admin2)
	r.HandleFunc("/static/*", serve)
	m := map[string]int{}
	cache.Get("key", &m)
	http.ListenAndServe(":3000", r)
}

func adminRouter() http.Handler {
	r := chi.NewRouter()
	r.Get("/stats", stats)
	return r
}
"#;

const CHI_USERS: &str = r#"package users

import "github.com/go-chi/chi/v5"

func Routes() chi.Router {
	r := chi.NewRouter()
	r.Get("/", list)
	r.Get("/{id}", get)
	r.Route("/{id}/posts", func(r chi.Router) {
		registerPosts(r)
	})
	return r
}

func registerPosts(r chi.Router) {
	r.Get("/", listPosts)
}
"#;

#[test]
fn chi_routes_scopes_and_mounts() {
    let dir = TempDir::new("chi");
    dir.write("cmd/server/main.go", CHI_MAIN);
    dir.write("internal/users/routes.go", CHI_USERS);
    dir.write(
        "internal/users/routes_test.go",
        CHI_USERS.replace("/{id}", "/{nope}").as_str(),
    );
    let s = scan(&dir);
    assert_eq!(s.files, 2);
    assert_eq!(
        routes(&s),
        [
            "GET /admin/stats",
            "GET /api/v1/articles",
            "POST /api/v1/articles",
            "GET /api/v1/articles/{{articleID}}",
            "PUT /api/v1/articles/{{articleID}}",
            "GET /api/v1/users",
            "GET /api/v1/users/{{id}}",
            "GET /api/v1/users/{{id}}/posts",
            "GET /health",
            "DELETE /ops/cache",
            "ANY /static/{{path}}",
        ]
    );
    let health = s.routes.iter().find(|r| r.path == "/health").unwrap();
    assert_eq!(health.source, Path::new("cmd/server/main.go"));
    assert_eq!(health.line, 14);
    assert_eq!(health.handler.as_deref(), Some("health"));
    assert_eq!(health.router, "chi");
}

const GIN_MAIN: &str = r#"package main

import (
	"github.com/gin-gonic/gin"
	"example.com/svc/routes"
)

func main() {
	r := gin.Default()
	r.GET("/ping", func(c *gin.Context) { c.String(200, "pong") })
	v1 := r.Group("/v1")
	{
		v1.POST("/login", loginEndpoint)
		users := v1.Group("/users", auth)
		users.GET("/:id", getUser)
		users.Handle("DELETE", "/:id", deleteUser)
		users.Any("/:id/files/*filepath", files)
		routes.Register(v1)
	}
	var v2 = r.Group("/v2")
	v2.GET("", indexV2)
	r.Group("/inline").PATCH("/x", h)
	routes.Register(r.Group("/v3"))
	r.Run()
}
"#;

const GIN_ROUTES: &str = r#"package routes

import "github.com/gin-gonic/gin"

func Register(rg *gin.RouterGroup) {
	rg.GET("/orders", listOrders)
}
"#;

#[test]
fn gin_groups_and_functions() {
    let dir = TempDir::new("gin");
    dir.write("main.go", GIN_MAIN);
    dir.write("routes/routes.go", GIN_ROUTES);
    let s = scan(&dir);
    assert_eq!(
        routes(&s),
        [
            "PATCH /inline/x",
            "GET /ping",
            "POST /v1/login",
            "GET /v1/orders",
            "DELETE /v1/users/{{id}}",
            "GET /v1/users/{{id}}",
            "ANY /v1/users/{{id}}/files/{{filepath}}",
            "GET /v2",
            "GET /v3/orders",
        ]
    );
    let ping = s.routes.iter().find(|r| r.path == "/ping").unwrap();
    assert_eq!(ping.handler, None, "func literal is not a handler name");
}

#[test]
fn net_http_patterns_and_custom_queries() {
    let dir = TempDir::new("nethttp");
    dir.write(
        "main.go",
        r#"package main

import "net/http"

func main() {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /items/{id}", getItem)
	mux.HandleFunc("POST /items/{$}", createItem)
	mux.Handle("/files/{path...}", files)
	http.HandleFunc("/legacy", legacy)
	mux.HandleFunc(dynamicPath(), x)
	srv.Add("/custom", h)
}
"#,
    );
    let s = scan(&dir);
    assert_eq!(
        routes(&s),
        [
            "ANY /files/{{path}}",
            "POST /items",
            "GET /items/{{id}}",
            "ANY /legacy",
        ]
    );
    assert_eq!(s.warnings.len(), 1, "{:?}", s.warnings);
    assert!(s.warnings[0].contains("main.go:11"), "{:?}", s.warnings);

    let custom = go::RouterQuery::new(
        "custom",
        r#"(call_expression
             function: (selector_expression field: (field_identifier) @_fn)
             arguments: (argument_list . (_) @path . (_) @handler .)
             (#eq? @_fn "Add")) @route"#,
    )
    .unwrap();
    let s = go::scan(&dir.0, &[custom]).unwrap();
    assert_eq!(routes(&s), ["ANY /custom"]);
}

#[test]
fn bad_queries_are_reported() {
    assert!(go::RouterQuery::new("x", "(call_expression").is_err());
    let err = go::RouterQuery::new("x", "(identifier) @nme")
        .err()
        .unwrap();
    assert!(err.to_string().contains("@nme"), "{err}");
}

#[test]
fn normalizes_paths() {
    assert_eq!(normalize_route(""), "/");
    assert_eq!(normalize_route("/users/"), "/users");
    assert_eq!(normalize_route("/users/:id"), "/users/{{id}}");
    assert_eq!(
        normalize_route("/a/{id:[0-9]+}/{slug}.json"),
        "/a/{{id}}/{{slug}}.json"
    );
    assert_eq!(normalize_route("/re/{p:a/b}"), "/re/{{p}}");
    assert_eq!(normalize_route("/static/*"), "/static/{{path}}");
}

fn route(method: &str, path: &str) -> Route {
    Route {
        method: method.into(),
        path: path.into(),
        source: "main.go".into(),
        line: 7,
        handler: Some("h.Get".into()),
        router: "chi".into(),
        info: RouteInfo::default(),
        middleware: Vec::new(),
    }
}

fn middleware<'a>(scan: &'a Scan, method: &str, path: &str) -> Vec<&'a str> {
    let r = scan
        .routes
        .iter()
        .find(|r| r.method == method && r.path == path)
        .unwrap_or_else(|| panic!("no {method} {path} in {:?}", routes(scan)));
    r.middleware.iter().map(String::as_str).collect()
}

#[test]
fn gin_middleware() {
    let dir = TempDir::new("gin-mw");
    dir.write(
        "main.go",
        r#"package main

import (
	"github.com/gin-gonic/gin"
	"example.com/svc/routes"
	"example.com/svc/middleware"
)

func main() {
	r := gin.New()
	r.Use(gin.Logger(), gin.Recovery())
	r.GET("/health", health)
	api := r.Group("/api")
	api.POST("/login", login)
	authed := api.Group("", middleware.AuthRequired())
	authed.GET("/me", me)
	admin := authed.Group("/admin")
	admin.Use(middleware.RequireRole("admin"))
	admin.DELETE("/users/:id", deleteUser)
	authed.POST("/orders", rateLimit, createOrder)
	routes.Register(authed)
}
"#,
    );
    dir.write(
        "routes/routes.go",
        "package routes\n\nimport \"github.com/gin-gonic/gin\"\n\nfunc Register(rg *gin.RouterGroup) {\n\trg.GET(\"/reports\", reports)\n}\n",
    );
    let s = scan(&dir);
    let logs = ["gin.Logger()", "gin.Recovery()"];
    assert_eq!(middleware(&s, "GET", "/health"), logs);
    assert_eq!(middleware(&s, "POST", "/api/login"), logs);
    let auth = [&logs[..], &["middleware.AuthRequired()"]].concat();
    assert_eq!(middleware(&s, "GET", "/api/me"), auth);
    assert_eq!(middleware(&s, "GET", "/api/reports"), auth);
    assert_eq!(
        middleware(&s, "DELETE", "/api/admin/users/{{id}}"),
        [&auth[..], &["middleware.RequireRole(\"admin\")"]].concat()
    );
    assert_eq!(
        middleware(&s, "POST", "/api/orders"),
        [&auth[..], &["rateLimit"]].concat()
    );
    let orders = s.routes.iter().find(|r| r.path == "/api/orders").unwrap();
    assert_eq!(orders.handler.as_deref(), Some("createOrder"));
}

#[test]
fn chi_middleware() {
    let dir = TempDir::new("chi-mw");
    dir.write(
        "main.go",
        r#"package main

import "github.com/go-chi/chi/v5"

func main() {
	r := chi.NewRouter()
	r.Use(logger)
	r.Post("/login", login)
	r.Group(func(r chi.Router) {
		r.Use(jwtauth.Authenticator(tokenAuth))
		r.Get("/me", me)
		r.With(paginate).Get("/orders", listOrders)
	})
	r.Route("/admin", func(r chi.Router) {
		r.Use(adminOnly)
		r.Get("/stats", stats)
	})
	r.Get("/public", public)
}
"#,
    );
    let s = scan(&dir);
    assert_eq!(middleware(&s, "POST", "/login"), ["logger"]);
    assert_eq!(middleware(&s, "GET", "/public"), ["logger"]);
    let auth = ["logger", "jwtauth.Authenticator(tokenAuth)"];
    assert_eq!(middleware(&s, "GET", "/me"), auth);
    assert_eq!(
        middleware(&s, "GET", "/orders"),
        [&auth[..], &["paginate"]].concat()
    );
    assert_eq!(
        middleware(&s, "GET", "/admin/stats"),
        ["logger", "adminOnly"]
    );
}

#[test]
fn plan_creates_only_missing_files() {
    let dir = TempDir::new("plan");
    dir.write("env.toml", "[env.dev]\nbase = \"http://x\"\n");
    // Существующий файл с другим именем и другой переменной в параметре.
    dir.write("users/one.http", "GET {{base}}/users/{{user_id}}?full=1\n");
    // Занимает имя, но про другой роут — новый файл получит суффикс.
    dir.write("users/get.http", "GET {{base}}/old\n");
    // Чужой хост — не считается ни совпадением, ни пропавшим.
    dir.write("ext.http", "GET https://example.com/users\n");
    // .outry: совпадение по `handler`, даже если путь в коде уже другой; и по пути.
    dir.write(
        "orders.outry",
        "// List\nGET /v1/orders { handler: h.Orders }\n\n// One\nGET /orders/{order_id}\n",
    );
    let scan = Scan {
        files: 1,
        routes: vec![
            route("GET", "/users"),
            route("GET", "/users/{{id}}"),
            route("POST", "/users"),
            route("ANY", "/"),
            route("DELETE", "/users/{{id}}/posts/{{post}}"),
            Route {
                handler: Some("h.Orders".into()),
                ..route("GET", "/orders")
            },
            route("GET", "/orders/{{id}}"),
        ],
        shapes: vec![],
        warnings: vec![],
    };
    let plan = plan(&dir.0, scan, DEFAULT_BASE).unwrap();
    let new: Vec<_> = plan.new.iter().map(|f| f.file.clone()).collect();
    assert_eq!(
        new,
        [
            PathBuf::from("users/get-2.outry"),
            PathBuf::from("users/post.outry"),
            PathBuf::from("root/get.outry"),
            PathBuf::from("users/posts/delete-by-post.outry"),
        ]
    );
    let existing: Vec<_> = plan.existing.iter().map(|e| e.file.clone()).collect();
    assert_eq!(
        existing,
        ["users/one.http", "orders.outry", "orders.outry"].map(PathBuf::from)
    );
    assert_eq!(plan.stale.len(), 1);
    assert_eq!(plan.stale[0].file, Path::new("users/get.http"));

    let post = &plan.new[1].content;
    assert_eq!(
        post,
        "// chi main.go:7 → h.Get\nGet: POST /users {\n  handler: h.Get\n  body {}\n}\n"
    );
    let any = &plan.new[2].content;
    assert!(any.contains("// any method\nGet: GET / {\n"), "{any}");
    for f in &plan.new {
        crate::lang::parse::parse(&f.content, None).unwrap();
    }

    let created = apply(&dir.0, &plan).unwrap();
    assert_eq!(created, new);
    // Второй прогон: всё уже есть.
    let again = super::plan(
        &dir.0,
        Scan {
            files: 1,
            routes: plan.new.iter().map(|f| f.route.clone()).collect(),
            shapes: vec![],
            warnings: vec![],
        },
        DEFAULT_BASE,
    )
    .unwrap();
    assert!(again.new.is_empty());
    assert_eq!(again.existing.len(), 4);
    assert!(apply(&dir.0, &plan).unwrap().is_empty(), "never overwrites");
}

const DESCRIBE_MAIN: &str = r#"package main

import (
	"encoding/json"
	"net/http"

	"github.com/go-chi/chi/v5"
	"example.com/svc/dto"
)

func main() {
	r := chi.NewRouter()
	h := &Handler{}
	r.Post("/users", h.CreateUser)
	r.Get("/users", h.ListUsers)
	r.Put("/users/{id}", http.HandlerFunc(h.UpdateUser))
	r.Post("/inline", func(w http.ResponseWriter, r *http.Request) {
		var in struct {
			Text string `json:"text"`
		}
		json.NewDecoder(r.Body).Decode(&in)
	})
}

// CreateUser creates a user.
// Sends a welcome email.
func (h *Handler) CreateUser(w http.ResponseWriter, r *http.Request) {
	var req dto.CreateUser
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		return
	}
	_ = r.Header.Get("X-Tenant-ID")
}

// @Summary List users
// @Tags users
func (h *Handler) ListUsers(w http.ResponseWriter, r *http.Request) {
	q := r.URL.Query()
	page := q.Get("page")
	limit := r.URL.Query().Get("limit")
	rows, _ := h.db.Query("SELECT * FROM users WHERE x = ?", page)
	_, _ = limit, rows
}

func (h *Handler) UpdateUser(w http.ResponseWriter, r *http.Request) {
	req := &dto.UpdateUser{}
	decodeJSON(r, req)
	readJSON(r, &req)
}
"#;

const DESCRIBE_DTO: &str = r#"package dto

import "time"

type Status string

type Base struct {
	Note string `json:"note,omitempty"`
}

type CreateUser struct {
	Base
	Name     string            `json:"name" validate:"required,min=2"` // ФИО
	Email    string            `json:"email" validate:"required,email"`
	Age      *int              `json:"age,omitempty"`
	Status   Status            `json:"status"`
	Tags     []string          `json:"tags"`
	Address  Address           `json:"address"`
	Meta     map[string]string `json:"meta"`
	Born     time.Time         `json:"born"`
	Password string            `json:"-"`
	internal int
	Legacy   bool
}

type Address struct {
	City string `json:"city"`
}

type UpdateUser struct {
	Name string `json:"name"`
}
"#;

#[test]
fn describes_handlers() {
    let dir = TempDir::new("describe");
    dir.write("main.go", DESCRIBE_MAIN);
    dir.write("dto/dto.go", DESCRIBE_DTO);
    let s = scan(&dir);
    let get = |m: &str, p: &str| {
        s.routes
            .iter()
            .find(|r| r.method == m && r.path == p)
            .unwrap_or_else(|| panic!("{m} {p}: {:?}", routes(&s)))
            .info
            .clone()
    };

    let create = get("POST", "/users");
    assert_eq!(
        create.summary.as_deref(),
        Some("CreateUser creates a user.")
    );
    assert_eq!(create.description, ["Sends a welcome email."]);
    assert_eq!(create.headers, ["X-Tenant-ID"]);
    let body = create.body.unwrap();
    assert_eq!(body.type_name, "dto.CreateUser");
    let names: Vec<&str> = body.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "note",
            "name",
            "email",
            "age",
            "status",
            "tags",
            "address.city",
            "meta",
            "born",
            "Legacy"
        ]
    );
    let name = &body.fields[1];
    assert!(name.required);
    assert_eq!(name.comment.as_deref(), Some("ФИО"));
    assert!(!body.fields[3].required);
    let example: serde_json::Value = serde_json::from_str(&body.example).unwrap();
    assert_eq!(
        example,
        serde_json::json!({
            "note": "", "name": "", "email": "", "age": 0, "status": "", "tags": [""],
            "address": {"city": ""}, "meta": {}, "born": "2006-01-02T15:04:05Z", "Legacy": false
        })
    );
    assert!(
        body.example.starts_with("{\n  \"note\""),
        "field order kept"
    );
    assert!(
        body.example.contains("\"tags\": [\"\"],"),
        "{}",
        body.example
    );

    let list = get("GET", "/users");
    assert_eq!(list.summary.as_deref(), Some("List users"));
    assert!(list.description.is_empty(), "{:?}", list.description);
    let query: Vec<&str> = list.query.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(query, ["page", "limit"]);
    assert!(list.body.is_none());

    let update = get("PUT", "/users/{{id}}");
    assert_eq!(update.body.unwrap().type_name, "dto.UpdateUser");

    let inline = get("POST", "/inline");
    assert_eq!(inline.body.unwrap().example, "{\n  \"text\": \"\"\n}");

    // Всё это попадает в создаваемый файл.
    let route = s
        .routes
        .iter()
        .find(|r| r.method == "POST" && r.path == "/users")
        .unwrap();
    let text = content(route, DEFAULT_BASE, &[], &[]);
    assert!(text.starts_with("// CreateUser creates a user.\n// Sends a welcome email.\n// chi main.go:14 → h.CreateUser\n//\n"), "{text}");
    assert!(
        text.contains("// Headers: X-Tenant-ID\n// Body: dto.CreateUser\n"),
        "{text}"
    );
    assert!(
        text.contains("//   name          string             required  ФИО\n"),
        "{text}"
    );
    assert!(
        text.contains("POST /users {\n  handler: h.CreateUser\n  headers { X-Tenant-ID: \"\" }\n\n  body {\n    note: \"\",\n"),
        "{text}"
    );
    let file = crate::lang::parse::parse(&text, None).unwrap();
    let crate::lang::ast::Item::Request(r) = &file.items[0] else {
        panic!()
    };
    assert_eq!(r.name.as_deref(), Some("CreateUser"));

    let list = s
        .routes
        .iter()
        .find(|r| r.method == "GET" && r.path == "/users")
        .unwrap();
    let text = content(list, DEFAULT_BASE, &[], &[]);
    assert!(
        text.contains("GET /users {\n  handler: h.ListUsers\n\n  params {\n    page: null\n    limit: null\n  }\n\n  query {\n    page\n    limit\n  }\n}\n"),
        "{text}"
    );
    assert!(text.contains("\nListUsers: GET /users {\n"), "{text}");
}

#[test]
fn describes_gin_bindings() {
    let dir = TempDir::new("describe-gin");
    dir.write(
        "main.go",
        r#"package main

import "github.com/gin-gonic/gin"

type loginReq struct {
	Login    string `json:"login" binding:"required"`
	Password string `json:"password" binding:"required"`
}

type listQuery struct {
	Page int    `form:"page"`
	Sort string `form:"sort" binding:"required"`
}

func main() {
	r := gin.Default()
	r.POST("/login", login)
	r.GET("/items", listItems)
	r.POST("/session", session)
	r.POST("/avatar", avatar)
}

func session(c *gin.Context) {
	_ = c.PostForm("email")
	_ = c.DefaultPostForm("remember", "no")
}

func avatar(c *gin.Context) {
	_ = c.PostForm("caption")
	_, _ = c.FormFile("file")
}

func login(c *gin.Context) {
	var req loginReq
	if err := c.ShouldBindJSON(&req); err != nil {
		return
	}
}

func listItems(c *gin.Context) {
	var q listQuery
	_ = c.ShouldBindQuery(&q)
	_ = c.DefaultQuery("lang", "en")
	_ = c.GetHeader("Accept-Language")
}
"#,
    );
    let s = scan(&dir);
    let login = &s.routes.iter().find(|r| r.path == "/login").unwrap().info;
    let body = login.body.as_ref().unwrap();
    assert!(body.fields.iter().all(|f| f.required));
    assert_eq!(
        body.example,
        "{\n  \"login\": \"\",\n  \"password\": \"\"\n}"
    );

    let items = &s.routes.iter().find(|r| r.path == "/items").unwrap().info;
    let q: Vec<(&str, bool)> = items
        .query
        .iter()
        .map(|f| (f.name.as_str(), f.required))
        .collect();
    assert_eq!(q, [("page", false), ("sort", true), ("lang", false)]);
    assert_eq!(items.headers, ["Accept-Language"]);
    assert!(items.body.is_none());

    let form = |path: &str| {
        let info = &s.routes.iter().find(|r| r.path == path).unwrap().info;
        info.form
            .iter()
            .map(|f| format!("{} {}", f.name, f.ty))
            .collect::<Vec<_>>()
    };
    assert_eq!(form("/session"), ["email string", "remember string"]);
    assert_eq!(form("/avatar"), ["caption string", "file file"]);
}

#[test]
fn response_from_layered_service() {
    // handler.Get → service.Get → model.Order: метод сервиса называется так же, как обработчик.
    let dir = TempDir::new("layered");
    dir.write(
        "internal/model/models.go",
        r#"package model

type Order struct {
	ID   int    `json:"id"`
	Note *string `json:"note,omitempty"`
}
"#,
    );
    dir.write(
        "internal/service/order.go",
        r#"package service

import (
	"context"
	"example.com/internal/model"
)

type OrderService struct{}

func (s *OrderService) Get(ctx context.Context, id int) (*model.Order, error) {
	return nil, nil
}

func (s *OrderService) List(ctx context.Context) ([]model.Order, error) {
	return nil, nil
}
"#,
    );
    dir.write(
        "internal/handler/order.go",
        r#"package handler

import (
	"net/http"
	"github.com/gin-gonic/gin"
	"example.com/internal/service"
)

type OrderHandler struct {
	orders *service.OrderService
}

func (h *OrderHandler) Get(c *gin.Context) {
	o, err := h.orders.Get(c.Request.Context(), 1)
	if err != nil {
		c.JSON(http.StatusNotFound, gin.H{"error": "not found"})
		return
	}
	c.JSON(http.StatusOK, o)
}

func (h *OrderHandler) List(c *gin.Context) {
	list, _ := h.orders.List(c.Request.Context())
	c.JSON(http.StatusOK, list)
}

func Register(r *gin.Engine, h *OrderHandler) {
	r.GET("/orders/:id", h.Get)
	r.GET("/orders", h.List)
}
"#,
    );
    let s = scan(&dir);
    let shape = |path: &str| {
        let r = s.routes.iter().find(|r| r.path == path);
        let r = r.unwrap_or_else(|| panic!("{:?}", routes(&s)));
        r.info.response.as_ref().map(|r| r.shape.clone())
    };
    assert_eq!(shape("/orders/{{id}}").as_deref(), Some("Order"));
    assert_eq!(shape("/orders").as_deref(), Some("[Order]"));
    assert_eq!(s.shapes.len(), 1);
    assert_eq!(s.shapes[0].go_type, "model.Order");
    assert_eq!(s.shapes[0].shape, "{ id: integer, note?: string }");
}

#[test]
fn nested_named_structs_get_their_own_shapes() {
    // Page → Order → Item → Price: глубина не сбрасывалась, и Item становился `any`.
    let dir = TempDir::new("nested-shapes");
    dir.write(
        "main.go",
        r#"package main

import (
	"net/http"
	"github.com/gin-gonic/gin"
)

type Price struct {
	Amount float64 `json:"amount"`
}

type Item struct {
	SKU   string `json:"sku"`
	Price Price  `json:"price"`
}

type Order struct {
	Items []Item `json:"items"`
}

type Page struct {
	Items []Order `json:"items"`
}

func list(c *gin.Context) {
	c.JSON(http.StatusOK, Page{})
}

func main() {
	r := gin.New()
	r.GET("/orders", list)
}
"#,
    );
    let s = scan(&dir);
    let shapes: Vec<(&str, &str)> = s
        .shapes
        .iter()
        .map(|d| (d.name.as_str(), d.shape.as_str()))
        .collect();
    assert_eq!(
        shapes,
        [
            ("Page", "{ items: [Order] }"),
            ("Order", "{ items: [Item] }"),
            ("Item", "{ sku: string, price: Price }"),
            ("Price", "{ amount: number }"),
        ]
    );
}

#[test]
fn compares_requests_with_code() {
    let dir = TempDir::new("diff");
    dir.write(
        "svc/main.go",
        r#"package main

import (
	"encoding/json"
	"net/http"

	"github.com/go-chi/chi/v5"
)

type Status string

type CreateOrder struct {
	Item    string `json:"item" validate:"required"`
	Qty     int    `json:"qty" validate:"required"`
	Status  Status `json:"status"`
	Address struct {
		City string `json:"city" validate:"required"`
	} `json:"address"`
	Lines []struct {
		SKU string `json:"sku" validate:"required"`
	} `json:"lines"`
}

func main() {
	r := chi.NewRouter()
	r.Post("/v2/orders", createOrder)
	r.Get("/orders/{orderID}", getOrder)
	r.Put("/orders/{id}", updateOrder)
}

func createOrder(w http.ResponseWriter, r *http.Request) {
	var req CreateOrder
	json.NewDecoder(r.Body).Decode(&req)
	_ = r.Header.Get("X-Tenant")
}

func getOrder(w http.ResponseWriter, r *http.Request) {
	_ = r.URL.Query().Get("full")
	_ = r.URL.Query().Get("lang")
}

func updateOrder(w http.ResponseWriter, r *http.Request) {
	var req CreateOrder
	json.NewDecoder(r.Body).Decode(&req)
}
"#,
    );
    dir.write("api/env.toml", "[env.dev]\nbase = \"http://x\"\n");
    dir.write(
        "api/orders.outry",
        r#"// Create
PUT /orders {
  handler: createOrder
  headers {
    x-tenant: "t1"
  }
  body {
    qty: "25"
    status: 1
    color: "red"
    address: {}
    lines: [{ sku: "a" }, { qty: 1 }]
  }
}

// Create bad
POST /v2/orders {
  handler: createOrder
  body { qty: 1 }
  expect {
    status == 400
  }
}

// Get
GET /orders/{id}?full=1 {
  handler: getOrder
}

// Update
PUT /orders/{id} {
  handler: updateOrder
}
"#,
    );
    let plan = plan_go(&dir.0.join("svc"), &dir.0.join("api"), &[], DEFAULT_BASE).unwrap();
    let mut changes: Vec<String> = plan
        .existing
        .iter()
        .flat_map(|e| &e.changes)
        .map(|c| format!("{}:{} {}", c.file.display(), c.line, c.kind))
        .collect();
    changes.sort();
    assert_eq!(
        changes,
        [
            "orders.outry:10 body field `color` is not in CreateOrder",
            "orders.outry:11 required field `address.city` (string) is missing from body",
            "orders.outry:12 body field `lines[].qty` is not in CreateOrder",
            "orders.outry:12 required field `lines[].sku` (string) is missing from body",
            "orders.outry:2 method changed in code: PUT → POST",
            "orders.outry:2 path changed in code: /orders → /v2/orders",
            "orders.outry:26 handler reads query parameter `lang`, the request doesn't send it",
            "orders.outry:26 path parameter renamed in code: {id} → {orderID}",
            "orders.outry:31 handler reads a CreateOrder body, the request sends none",
            "orders.outry:7 required field `item` (string) is missing from body",
            "orders.outry:8 body field `qty` is a string, code expects int",
            "orders.outry:9 body field `status` is a number, code expects Status",
        ]
    );
    assert!(plan.stale.is_empty() && plan.new.is_empty());
    // JSON: вид изменения — в `kind`, рядом с полями.
    let method = plan
        .existing
        .iter()
        .flat_map(|e| &e.changes)
        .find(|c| matches!(c.kind, ChangeKind::Method { .. }))
        .unwrap();
    let json = serde_json::to_value(method).unwrap();
    assert_eq!(json["kind"], "method");
    assert_eq!(json["now"], "POST");
    assert_eq!(
        (json["line"].as_u64(), json["col"].as_u64()),
        (Some(2), Some(1))
    );

    let fixed = fixes(&plan, |_| true).unwrap();
    assert_eq!(fixed.len(), 1);
    assert_eq!(fixed[0].after, FIXED_ORDERS, "{}", fixed[0].diff);
    assert!(
        fixed[0]
            .diff
            .starts_with("--- a/orders.outry\n+++ b/orders.outry\n@@ -1,")
    );
    // Каждая правка — со своим diff; неисправимые — без.
    let param = plan
        .changes()
        .find(|c| matches!(c.kind, ChangeKind::PathParam { .. }))
        .unwrap();
    assert!(!param.fixable && param.diff.is_none());
    assert_eq!(param.severity, Severity::Warning);
    let ty = plan
        .changes()
        .find(|c| matches!(c.kind, ChangeKind::FieldType { .. }))
        .unwrap();
    assert!(ty.diff.as_deref().unwrap().contains("-    qty: \"25\"\n"));

    // Записываем и сканируем снова: исправимое исправлено.
    write_fixes(&dir.0.join("api"), &fixed).unwrap();
    let again = plan_go(&dir.0.join("svc"), &dir.0.join("api"), &[], DEFAULT_BASE).unwrap();
    let left: Vec<String> = again.changes().map(|c| c.message.clone()).collect();
    assert_eq!(
        left,
        [
            "handler reads query parameter `lang`, the request doesn't send it",
            "path parameter renamed in code: {id} → {orderID}",
        ]
    );
}

const FIXED_ORDERS: &str = r#"Create: POST /v2/orders {
  handler: createOrder
  headers { x-tenant: "t1" }

  body {
    qty: 25,
    status: "1",
    address: { city: "" },
    lines: [{ sku: "a" }, { sku: "" }],
    item: "",
  }
}

// Create bad
POST /v2/orders {
  handler: createOrder
  body { qty: 1 }
  expect {
    status == 400
  }
}

// Get
GET /orders/{id}?full=1 {
  handler: getOrder
}

Update: PUT /orders/{id} {
  handler: updateOrder

  body {
    item: "",
    qty: 0,
    status: "",
    address: { city: "" },
    lines: [{ sku: "" }],
  }
}
"#;

fn route_info<'a>(plan: &'a Plan, method: &str, path: &str) -> &'a RouteInfo {
    plan.new
        .iter()
        .map(|f| &f.route)
        .chain(plan.existing.iter().map(|e| &e.route))
        .find(|r| r.method == method && r.path == path)
        .map(|r| &r.info)
        .unwrap()
}

#[test]
fn responses_shapes_http_and_prune() {
    let dir = TempDir::new("shapes");
    dir.write(
        "svc/main.go",
        r#"package main

import (
	"encoding/json"
	"net/http"
	"time"

	"github.com/go-chi/chi/v5"
)

type Customer struct {
	Name string `json:"name"`
}

type Item struct {
	SKU string `json:"sku"`
}

type Order struct {
	ID       int64     `json:"id"`
	Total    float64   `json:"total"`
	Status   string    `json:"status"`
	Note     *string   `json:"note,omitempty"`
	Parent   *Order    `json:"parent"`
	Customer Customer  `json:"customer"`
	Items    []Item    `json:"items"`
	Created  time.Time `json:"created"`
}

type errorBody struct {
	Error string `json:"error"`
}

type createReq struct {
	SKU string `json:"sku" validate:"required"`
}

func main() {
	r := chi.NewRouter()
	r.Get("/orders/{id}", getOrder)
	r.Get("/orders", listOrders)
	r.Post("/orders", createOrder)
}

func load(r *http.Request) (*Order, error) { return nil, nil }

func getOrder(w http.ResponseWriter, r *http.Request) {
	o, err := load(r)
	if err != nil {
		writeJSON(w, http.StatusNotFound, errorBody{Error: "not found"})
		return
	}
	writeJSON(w, http.StatusOK, o)
}

func listOrders(w http.ResponseWriter, r *http.Request) {
	var out []Order
	_ = r.Header.Get("X-Tenant")
	json.NewEncoder(w).Encode(out)
}

func createOrder(w http.ResponseWriter, r *http.Request) {
	var req createReq
	json.NewDecoder(r.Body).Decode(&req)
	writeJSON(w, http.StatusCreated, &Order{})
}
"#,
    );
    dir.write("api/env.toml", "[env.dev]\nbase = \"http://x\"\n");
    dir.write(
        "api/shapes.outry",
        "shape Order {\n  id: integer,\n  total: string,\n  status: \"new\" | \"paid\",\n  extra: string,\n}\n",
    );
    dir.write(
        "api/orders/get.outry",
        "// Get order\nGET /orders/{id} {\n  expect { status == 200 }\n}\n",
    );
    dir.write(
        "api/orders/list.http",
        "GET {{base}}/orders\nX-Tenant: t\n\n> assert status == 200\n",
    );
    dir.write("api/legacy.http", "POST {{base}}/legacy\n");
    dir.write("api/old.outry", "// A\nGET /a\n\n// B\nGET /b\n");
    dir.write(
        "api/mixed.outry",
        "// C\nGET /c\n\n// Flow\nflow Smoke {\n  C()\n}\n",
    );
    let plan = plan_go(&dir.0.join("svc"), &dir.0.join("api"), &[], DEFAULT_BASE).unwrap();

    let get = route_info(&plan, "GET", "/orders/{{id}}");
    assert_eq!(get.response.as_ref().unwrap().shape, "Order");
    let list = route_info(&plan, "GET", "/orders");
    assert_eq!(list.response.as_ref().unwrap().shape, "[Order]");

    let new_shapes: Vec<(&str, &str)> = plan
        .new_shapes
        .iter()
        .map(|d| (d.name.as_str(), d.shape.as_str()))
        .collect();
    assert_eq!(
        new_shapes,
        [
            ("Customer", "{ name: string }"),
            ("Item", "{ sku: string }")
        ]
    );
    let mut shape_changes: Vec<String> = plan
        .shape_changes
        .iter()
        .map(|c| format!("{} {}", c.severity, c.message))
        .collect();
    shape_changes.sort();
    assert_eq!(
        shape_changes,
        [
            "error shape Order field `extra` is not in code",
            "error shape Order field `total` is string, code has number",
            "warning shape Order has no field `created` (string) from code",
            "warning shape Order has no field `customer` (Customer) from code",
            "warning shape Order has no field `items` ([Item]) from code",
            "warning shape Order has no field `note` (string) from code",
            "warning shape Order has no field `parent` (Order | null) from code",
        ]
    );
    assert_eq!(plan.shape_changes[0].go.file, Path::new("main.go"));

    let messages: Vec<String> = plan
        .existing
        .iter()
        .flat_map(|e| &e.changes)
        .map(|c| {
            format!(
                "{}:{} {}",
                c.file.display().to_string().replace('\\', "/"),
                c.line,
                c.message
            )
        })
        .collect();
    assert_eq!(
        messages,
        [
            "orders/get.outry:3 handler responds with Order, the request doesn't check `body matches Order`"
        ]
    );

    let new = &plan.new[0];
    assert!(
        new.content
            .contains("  body { sku: \"\" }\n  expect { body matches Order }\n"),
        "{}",
        new.content
    );
    assert!(
        new.content.contains("// Response: Order\n"),
        "{}",
        new.content
    );

    let stale: Vec<String> = plan
        .stale
        .iter()
        .map(|s| format!("{}:{}", s.file.display(), s.line))
        .collect();
    assert_eq!(
        stale,
        [
            "legacy.http:1",
            "mixed.outry:2",
            "old.outry:2",
            "old.outry:5"
        ]
    );
    assert_eq!(
        plan.prunable,
        [PathBuf::from("legacy.http"), PathBuf::from("old.outry")]
    );
    assert!(plan.has_errors());

    let fixed = fixes(&plan, |_| true).unwrap();
    let files: Vec<_> = fixed.iter().map(|f| f.file.clone()).collect();
    assert_eq!(
        files,
        ["orders/get.outry", "shapes.outry"].map(PathBuf::from)
    );
    assert_eq!(
        fixed[0].after,
        "GetOrder: GET /orders/{id} {\n  expect {\n    status == 200\n    body matches Order\n  }\n}\n"
    );
    assert_eq!(
        fixed[1].after,
        "shape Order {\n  id: integer,\n  total: number,\n  status: \"new\" | \"paid\",\n  note?: string,\n  parent: Order | null,\n  customer: Customer,\n  items: [Item],\n  created: string,\n}\n"
    );

    let root = dir.0.join("api");
    apply(&root, &plan).unwrap();
    write_fixes(&root, &fixed).unwrap_err(); // shapes.outry уже дописан — правки устарели
    let plan = plan_go(&dir.0.join("svc"), &root, &[], DEFAULT_BASE).unwrap();
    write_fixes(&root, &fixes(&plan, |_| true).unwrap()).unwrap();
    assert_eq!(prune(&root, &plan).unwrap(), plan.prunable);
    assert!(!root.join("old.outry").exists() && root.join("mixed.outry").exists());

    let plan = plan_go(&dir.0.join("svc"), &root, &[], DEFAULT_BASE).unwrap();
    assert!(plan.new_shapes.is_empty() && plan.new.is_empty());
    assert_eq!(
        plan.changes().count(),
        0,
        "{:#?}",
        plan.changes().collect::<Vec<_>>()
    );
    let shapes = std::fs::read_to_string(root.join("shapes.outry")).unwrap();
    assert!(
        shapes.contains("\n// main.Customer — main.go:11\nshape Customer { name: string }\n"),
        "{shapes}"
    );
    // Проект снова целиком проходит `outry check`.
    let ws = crate::lang::Workspace::load(&root).unwrap();
    assert!(ws.check().is_empty(), "{:?}", ws.check());
}

#[test]
fn middleware_headers_from_env_toml() {
    let dir = TempDir::new("mw-plan");
    dir.write(
        "env.toml",
        "[env.dev]\nbase = \"http://x\"\n\n[import.middleware]\nAuthRequired = { headers = { Authorization = \"Bearer ${Login().body.token}\" } }\n",
    );
    // Без заголовка, с блоком headers, негативный тест и .http.
    dir.write(
        "orders.outry",
        "// List\nGET /orders { handler: h.List }\n\n// One\nGET /orders/{id} {\n  handler: h.One\n  headers {\n    Accept: \"application/json\"\n  }\n}\n\n// No token\nGET /orders { expect { status == 401 } }\n",
    );
    dir.write("reports.http", "GET {{base}}/reports\n");
    let authed = |method: &str, path: &str, handler: &str| Route {
        handler: Some(handler.into()),
        middleware: vec!["gin.Logger()".into(), "middleware.AuthRequired()".into()],
        ..route(method, path)
    };
    let scan = Scan {
        files: 1,
        routes: vec![
            authed("GET", "/orders", "h.List"),
            authed("GET", "/orders/{{id}}", "h.One"),
            authed("GET", "/reports", "h.Reports"),
            authed("POST", "/orders", "h.Create"),
            Route {
                middleware: vec!["gin.Logger()".into()],
                ..route("POST", "/login")
            },
        ],
        shapes: vec![],
        warnings: vec![],
    };
    let plan = plan(&dir.0, scan, "base").unwrap();

    let create = plan.new.iter().find(|n| n.route.path == "/orders").unwrap();
    assert!(
        create
            .content
            .contains("// middleware: middleware.AuthRequired()\n"),
        "{}",
        create.content
    );
    assert!(!create.content.contains("gin.Logger"), "{}", create.content);
    assert!(
        create
            .content
            .contains("headers { Authorization: \"Bearer ${Login().body.token}\" }"),
        "{}",
        create.content
    );
    let login = plan.new.iter().find(|n| n.route.path == "/login").unwrap();
    assert!(!login.content.contains("headers"), "{}", login.content);
    assert!(!login.content.contains("middleware:"), "{}", login.content);

    let warnings: Vec<(String, usize, bool)> = plan
        .changes()
        .filter(|c| matches!(c.kind, ChangeKind::MiddlewareHeader { .. }))
        .map(|c| (slash(&c.file), c.line, c.fixable))
        .collect();
    assert_eq!(
        warnings,
        [
            ("orders.outry".to_string(), 2, true),
            ("orders.outry".to_string(), 7, true),
            ("reports.http".to_string(), 1, false),
        ]
    );
    let c = plan
        .changes()
        .find(|c| matches!(c.kind, ChangeKind::MiddlewareHeader { .. }))
        .unwrap();
    assert_eq!(
        c.message,
        "route is behind `middleware.AuthRequired()`, the request doesn't send header `Authorization`"
    );
    assert_eq!(c.severity, Severity::Warning);

    let fixed = fixes(&plan, |c| {
        matches!(c.kind, ChangeKind::MiddlewareHeader { .. })
    })
    .unwrap();
    assert_eq!(
        fixed[0].after,
        "List: GET /orders {\n  handler: h.List\n  headers { Authorization: \"Bearer ${Login().body.token}\" }\n}\n\nOne: GET /orders/{id} {\n  handler: h.One\n\n  headers {\n    Accept: \"application/json\"\n    Authorization: \"Bearer ${Login().body.token}\"\n  }\n}\n\n// No token\nGET /orders { expect { status == 401 } }\n"
    );
}

#[test]
fn concrete_paths_match_parametrized_routes() {
    assert!(fills_params("/users/{}", "/users/42"));
    assert!(fills_params("/users/{}/posts", "/users/{}/posts"));
    assert!(!fills_params("/users/{}", "/users/42/posts"));
    assert!(!fills_params("/users/me", "/users/42"));

    let dir = TempDir::new("concrete-paths");
    dir.write(
        "svc/main.go",
        r#"package main

import "github.com/gin-gonic/gin"

func main() {
	r := gin.New()
	r.GET("/users/:id", getUser)
	r.GET("/users/me", me)
}

func getUser(c *gin.Context) {}
func me(c *gin.Context)      {}
"#,
    );
    dir.write("api/env.toml", "[env.dev]\nbase = \"http://x\"\n");
    dir.write(
        "api/users.outry",
        "// Get user\nGET /users/42\n\n// Me\nGET /users/me\n",
    );
    let plan = plan_go(&dir.0.join("svc"), &dir.0.join("api"), &[], DEFAULT_BASE).unwrap();
    assert!(plan.stale.is_empty(), "{:?}", plan.stale);
    assert!(plan.new.is_empty());
    assert!(plan.existing.iter().all(|e| e.changes.is_empty()));
}
