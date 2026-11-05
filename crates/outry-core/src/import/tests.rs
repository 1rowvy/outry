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
    assert_eq!(normalize_route("/u/:id:int"), "/u/{{id}}");
    assert_eq!(normalize_route("/u/:id([0-9]+)"), "/u/{{id}}");
    assert_eq!(normalize_route("/u/:id?"), "/u/{{id}}");
    assert_eq!(normalize_route("/files/*.*"), "/files/{{path}}");
    assert_eq!(normalize_route("/u/:id<int>"), "/u/{{id}}");
    assert_eq!(normalize_route("/files/+"), "/files/{{path}}");
    assert_eq!(normalize_route("/files/*1"), "/files/{{path}}");
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
fn gin_mount_args_by_position() {
    let dir = TempDir::new("gin-mount-args");
    dir.write(
        "router/router.go",
        r#"package router

import "github.com/gin-gonic/gin"

func NewRouter(h *Handlers) *gin.Engine {
	r := gin.New()
	api := r.Group("/api")
	{
		v1 := api.Group("/v1")
		public := v1.Group("")
		protected := v1.Group("", middleware.RequireAuth(h.AuthHandler))

		RegisterUserRoutes(public, protected, h.UserHandler)
	}
	return r
}
"#,
    );
    dir.write(
        "router/users.go",
        r#"package router

import "github.com/gin-gonic/gin"

func RegisterUserRoutes(public, protected *gin.RouterGroup, h *UserHandler) {
	usersPublic := public.Group("/users")
	{
		usersPublic.GET("/:username", h.GetUser)
	}
	usersProtected := protected.Group("/users")
	{
		usersProtected.GET("/me", h.GetMe)
	}
}
"#,
    );
    let s = scan(&dir);
    assert!(middleware(&s, "GET", "/api/v1/users/{{username}}").is_empty());
    assert_eq!(
        middleware(&s, "GET", "/api/v1/users/me"),
        ["middleware.RequireAuth(h.AuthHandler)"]
    );
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

fn find<'a>(scan: &'a Scan, method: &str, path: &str) -> &'a Route {
    scan.routes
        .iter()
        .find(|r| r.method == method && r.path == path)
        .unwrap_or_else(|| panic!("no {method} {path} in {:?}", routes(scan)))
}

/// `"METHOD /path"` в порядке `scan`: по пути, затем по методу.
fn sorted(routes: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = routes.iter().map(|r| r.to_string()).collect();
    v.sort_by_key(|r| {
        let (m, p) = r.split_once(' ').unwrap();
        (p.to_string(), m.to_string())
    });
    v
}

fn query_names(r: &Route) -> Vec<&str> {
    r.info.query.iter().map(|f| f.name.as_str()).collect()
}

#[test]
fn go_zero_route_literals() {
    let dir = TempDir::new("gozero");
    dir.write(
        "internal/handler/routes.go",
        r#"package handler

import (
	"net/http"

	user "example.com/svc/internal/handler/user"
	"example.com/svc/internal/svc"
	"github.com/zeromicro/go-zero/rest"
)

func RegisterHandlers(server *rest.Server, serverCtx *svc.ServiceContext) {
	server.AddRoutes(
		[]rest.Route{
			{
				Method:  http.MethodPost,
				Path:    "/login",
				Handler: user.LoginHandler(serverCtx),
			},
			{
				Path:    "/users/:id",
				Method:  http.MethodGet,
				Handler: user.GetUserHandler(serverCtx),
			},
		},
		rest.WithPrefix("/api/v1"),
	)

	server.AddRoutes(
		rest.WithMiddlewares(
			[]rest.Middleware{serverCtx.Audit},
			[]rest.Route{
				{
					Method:  http.MethodDelete,
					Path:    "/users/:id",
					Handler: user.DeleteUserHandler(serverCtx),
				},
			}...,
		),
		rest.WithJwt(serverCtx.Config.Auth.AccessSecret),
		rest.WithPrefix("/api/v1"),
	)

	server.AddRoute(rest.Route{Method: http.MethodGet, Path: "/ping", Handler: ping})
	_ = http.Cookie{Path: "/not-a-route"}
}
"#,
    );
    dir.write(
        "internal/handler/user/getuserhandler.go",
        r#"package user

import (
	"net/http"

	"example.com/svc/internal/logic/user"
	"example.com/svc/internal/svc"
	"example.com/svc/internal/types"
	"github.com/zeromicro/go-zero/rest/httpx"
)

// Get a user by id
func GetUserHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.GetUserReq
		if err := httpx.Parse(r, &req); err != nil {
			httpx.ErrorCtx(r.Context(), w, err)
			return
		}
		l := user.NewGetUserLogic(r.Context(), svcCtx)
		resp, err := l.GetUser(&req)
		if err != nil {
			httpx.ErrorCtx(r.Context(), w, err)
		} else {
			httpx.OkJsonCtx(r.Context(), w, resp)
		}
	}
}

func LoginHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.LoginReq
		if err := httpx.Parse(r, &req); err != nil {
			return
		}
	}
}
"#,
    );
    dir.write(
        "internal/logic/user/getuserlogic.go",
        r#"package user

import "example.com/svc/internal/types"

type GetUserLogic struct{}

func (l *GetUserLogic) GetUser(req *types.GetUserReq) (resp *types.User, err error) {
	return
}
"#,
    );
    dir.write(
        "internal/types/types.go",
        r#"package types

type GetUserReq struct {
	ID     int64  `path:"id"`
	Fields string `form:"fields,optional"`
	Token  string `header:"X-Token"`
}

type LoginReq struct {
	Email    string `json:"email"`
	Password string `json:"password"`
}

type User struct {
	ID   int64  `json:"id"`
	Name string `json:"name"`
}
"#,
    );
    let s = scan(&dir);
    assert_eq!(
        routes(&s),
        [
            "POST /api/v1/login",
            "DELETE /api/v1/users/{{id}}",
            "GET /api/v1/users/{{id}}",
            "GET /ping",
        ]
    );
    assert!(s.routes.iter().all(|r| r.router == "go-zero"));
    assert_eq!(
        middleware(&s, "DELETE", "/api/v1/users/{{id}}"),
        [
            "rest.WithJwt(serverCtx.Config.Auth.AccessSecret)",
            "serverCtx.Audit"
        ]
    );
    assert!(middleware(&s, "GET", "/api/v1/users/{{id}}").is_empty());

    let get = find(&s, "GET", "/api/v1/users/{{id}}");
    assert_eq!(
        get.handler.as_deref(),
        Some("user.GetUserHandler(serverCtx)")
    );
    assert_eq!(get.info.summary.as_deref(), Some("Get a user by id"));
    assert_eq!(query_names(get), ["fields"], "only `form` tags are query");
    assert_eq!(get.info.headers, ["X-Token"]);
    assert!(get.info.body.is_none(), "no `json` tags — no body");
    assert_eq!(get.info.response.as_ref().unwrap().shape, "User");

    let login = find(&s, "POST", "/api/v1/login");
    let body = login.info.body.as_ref().unwrap();
    let names: Vec<&str> = body.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["email", "password"]);
}

#[test]
fn go_restful_builder_chains() {
    let dir = TempDir::new("restful");
    dir.write(
        "users/resource.go",
        r#"package users

import (
	"net/http"

	restful "github.com/emicklei/go-restful/v3"
)

type UserResource struct{}

type User struct {
	ID   string `json:"id"`
	Name string `json:"name"`
}

type CreateUser struct {
	Name string `json:"name" validate:"required"`
}

func (u UserResource) WebService() *restful.WebService {
	ws := new(restful.WebService)
	ws.Path("/users").
		Consumes(restful.MIME_JSON).
		Produces(restful.MIME_JSON)
	ws.Filter(logging)

	ws.Route(ws.GET("/{user-id}").To(u.findUser).
		Doc("get a user").
		Param(ws.PathParameter("user-id", "identifier of the user").DataType("string")).
		Param(ws.QueryParameter("fields", "fields to return").DataType("string")).
		Param(ws.HeaderParameter("X-Tenant", "tenant")).
		Writes(User{}).
		Returns(404, "Not Found", nil))

	ws.Route(ws.POST("").To(u.createUser).
		Filter(auth).
		Reads(CreateUser{}).
		Returns(http.StatusCreated, "Created", User{}))

	ws.Route(ws.Method("PUT").Path("/{user-id}").To(u.updateUser))
	return ws
}

func (u UserResource) findUser(req *restful.Request, resp *restful.Response) {
	_ = req.QueryParameter("verbose")
	resp.WriteEntity(User{ID: req.PathParameter("user-id")})
}

func (u UserResource) createUser(req *restful.Request, resp *restful.Response) {}

// Update a user
func (u UserResource) updateUser(req *restful.Request, resp *restful.Response) {
	usr := User{}
	if err := req.ReadEntity(&usr); err != nil {
		resp.WriteError(http.StatusBadRequest, err)
		return
	}
	resp.WriteHeaderAndEntity(http.StatusOK, usr)
}
"#,
    );
    let s = scan(&dir);
    assert_eq!(
        routes(&s),
        [
            "POST /users",
            "GET /users/{{user-id}}",
            "PUT /users/{{user-id}}"
        ]
    );
    let get = find(&s, "GET", "/users/{{user-id}}");
    assert_eq!(get.handler.as_deref(), Some("u.findUser"));
    assert_eq!(query_names(get), ["verbose", "fields"]);
    assert_eq!(get.info.headers, ["X-Tenant"]);
    assert_eq!(get.info.response.as_ref().unwrap().shape, "User");
    assert_eq!(middleware(&s, "GET", "/users/{{user-id}}"), ["logging"]);

    let post = find(&s, "POST", "/users");
    assert_eq!(middleware(&s, "POST", "/users"), ["logging", "auth"]);
    let body = post.info.body.as_ref().unwrap();
    assert_eq!(body.type_name, "CreateUser");
    assert!(body.fields[0].required);
    assert_eq!(post.info.response.as_ref().unwrap().shape, "User");

    let put = find(&s, "PUT", "/users/{{user-id}}");
    assert_eq!(put.info.summary.as_deref(), Some("Update a user"));
    assert_eq!(put.info.body.as_ref().unwrap().type_name, "User");
    assert_eq!(put.info.response.as_ref().unwrap().shape, "User");
}

#[test]
fn beego_controllers_namespaces_and_annotations() {
    let dir = TempDir::new("beego");
    dir.write(
        "routers/router.go",
        r#"package routers

import (
	"example.com/app/controllers"
	"github.com/beego/beego/v2/server/web"
	"github.com/beego/beego/v2/server/web/context"
)

func init() {
	web.Router("/", &controllers.MainController{})
	web.Router("/user/:id:int", &controllers.UserController{}, "get:GetOne;put,patch:Update")
	web.CtrlGet("/about", (*controllers.MainController).About)
	ns := web.NewNamespace("/v1",
		web.NSBefore(auth),
		web.NSNamespace("/object",
			web.NSInclude(&controllers.ObjectController{}),
		),
		web.NSRouter("/health", &controllers.HealthController{}, "*:Check"),
		web.NSGet("/ping", func(ctx *context.Context) {}),
	)
	web.AddNamespace(ns)
}
"#,
    );
    dir.write(
        "controllers/default.go",
        r#"package controllers

import (
	"encoding/json"

	beego "github.com/beego/beego/v2/server/web"
)

type MainController struct{ beego.Controller }

func (c *MainController) Get()  {}
func (c *MainController) Post() {}

// About page
func (c *MainController) About() {}

type UserController struct{ beego.Controller }

// GetOne returns a user
func (c *UserController) GetOne() {
	_ = c.GetString("fields")
	c.Data["json"] = User{}
	c.ServeJSON()
}

// Update a user
func (c *UserController) Update() {
	var u User
	json.Unmarshal(c.Ctx.Input.RequestBody, &u)
}

// Get is not a route: the mapping lists the methods
func (c *UserController) Get() {}

type ObjectController struct{ beego.Controller }

// @Title Get
// @router /:objectId [get]
func (o *ObjectController) Get() {}

// @router / [post]
func (o *ObjectController) Post() {}

func (o *ObjectController) helper() {}

type HealthController struct{ beego.Controller }

func (h *HealthController) Check() {}

type User struct {
	ID   int    `json:"id"`
	Name string `json:"name"`
}
"#,
    );
    let s = scan(&dir);
    assert_eq!(
        routes(&s),
        [
            "GET /",
            "POST /",
            "GET /about",
            "GET /user/{{id}}",
            "PATCH /user/{{id}}",
            "PUT /user/{{id}}",
            "ANY /v1/health",
            "POST /v1/object",
            "GET /v1/object/{{objectId}}",
            "GET /v1/ping",
        ]
    );
    assert_eq!(middleware(&s, "GET", "/v1/ping"), ["auth"]);
    assert_eq!(middleware(&s, "GET", "/v1/object/{{objectId}}"), ["auth"]);
    assert!(middleware(&s, "GET", "/").is_empty());

    let get = find(&s, "GET", "/user/{{id}}");
    assert_eq!(
        get.handler.as_deref(),
        Some("controllers.UserController.GetOne")
    );
    assert_eq!(get.info.summary.as_deref(), Some("GetOne returns a user"));
    assert_eq!(query_names(get), ["fields"]);
    assert_eq!(get.info.response.as_ref().unwrap().shape, "User");
    let put = find(&s, "PUT", "/user/{{id}}");
    assert_eq!(put.info.body.as_ref().unwrap().type_name, "User");

    let about = find(&s, "GET", "/about");
    assert_eq!(about.info.summary.as_deref(), Some("About page"));

    // Роут из аннотации показывается там, где аннотация.
    let object = find(&s, "GET", "/v1/object/{{objectId}}");
    assert_eq!(object.source, PathBuf::from("controllers/default.go"));
    assert_eq!(object.line, 38);
    assert_eq!(
        find(&s, "ANY", "/v1/health").handler.as_deref(),
        Some("controllers.HealthController.Check")
    );
}

#[test]
fn buffalo_resources_and_skip() {
    let dir = TempDir::new("buffalo");
    dir.write(
        "actions/app.go",
        r#"package actions

import "github.com/gobuffalo/buffalo"

var app *buffalo.App

func App() *buffalo.App {
	if app == nil {
		app = buffalo.New(buffalo.Options{})
		app.Use(Authorize)
		app.Middleware.Skip(Authorize, HomeHandler, UsersResource{}.List)
		app.GET("/", HomeHandler)
		app.Resource("/users", UsersResource{})
		api := app.Group("/api/v1")
		api.Use(apiKey)
		wr := WidgetsResource{}
		api.Resource("/categories", wr)
	}
	return app
}

func HomeHandler(c buffalo.Context) error { return nil }
"#,
    );
    dir.write(
        "actions/users.go",
        r#"package actions

import (
	"net/http"

	"github.com/gobuffalo/buffalo"
)

type User struct {
	ID   int    `json:"id"`
	Name string `json:"name"`
}

type Problem struct {
	Error string `json:"error"`
}

type UsersResource struct{ buffalo.Resource }

func (v UsersResource) List(c buffalo.Context) error {
	return c.Render(http.StatusOK, r.JSON([]User{}))
}

// Show a user
func (v UsersResource) Show(c buffalo.Context) error {
	if c.Param("user_id") == "" {
		return c.Render(http.StatusNotFound, r.JSON(Problem{}))
	}
	return c.Render(http.StatusOK, r.JSON(User{}))
}

func (v UsersResource) Create(c buffalo.Context) error {
	u := User{}
	if err := c.Bind(&u); err != nil {
		return err
	}
	return c.Render(http.StatusCreated, r.JSON(u))
}

type WidgetsResource struct{ buffalo.Resource }

func (v WidgetsResource) List(c buffalo.Context) error    { return nil }
func (v WidgetsResource) Destroy(c buffalo.Context) error { return nil }
"#,
    );
    let s = scan(&dir);
    assert_eq!(
        routes(&s),
        [
            "GET /",
            "GET /api/v1/categories",
            "DELETE /api/v1/categories/{{category_id}}",
            "GET /users",
            "POST /users",
            "GET /users/{{user_id}}",
        ]
    );
    assert!(middleware(&s, "GET", "/").is_empty(), "skipped");
    assert!(middleware(&s, "GET", "/users").is_empty(), "skipped");
    assert_eq!(middleware(&s, "GET", "/users/{{user_id}}"), ["Authorize"]);
    assert_eq!(
        middleware(&s, "DELETE", "/api/v1/categories/{{category_id}}"),
        ["Authorize", "apiKey"]
    );
    let show = find(&s, "GET", "/users/{{user_id}}");
    assert_eq!(show.handler.as_deref(), Some("UsersResource.Show"));
    assert_eq!(show.info.summary.as_deref(), Some("Show a user"));
    assert_eq!(show.info.response.as_ref().unwrap().shape, "User");
    assert_eq!(
        find(&s, "GET", "/users")
            .info
            .response
            .as_ref()
            .unwrap()
            .shape,
        "[User]"
    );
    assert_eq!(
        find(&s, "POST", "/users")
            .info
            .body
            .as_ref()
            .unwrap()
            .type_name,
        "User"
    );
    assert_eq!(
        find(&s, "GET", "/api/v1/categories").handler.as_deref(),
        Some("wr.List")
    );
}

#[test]
fn shared_handler_matches_by_method() {
    let dir = TempDir::new("shared-handler");
    dir.write(
        "svc/routers/router.go",
        r#"package routers

import "github.com/beego/beego/v2/server/web"

func init() {
	web.Router("/user/:id", &UserController{}, "put,patch:Update")
}

type UserController struct{ web.Controller }

func (c *UserController) Update() {}
"#,
    );
    dir.write("api/env.toml", "[env.dev]\nbase = \"http://x\"\n");
    dir.write(
        "api/user.outry",
        "Update: PUT /user/{id} { handler: UserController.Update }\n",
    );
    let plan = plan_go(&dir.0.join("svc"), &dir.0.join("api"), &[], DEFAULT_BASE).unwrap();
    assert_eq!(plan.existing.len(), 1);
    assert_eq!(plan.existing[0].route.method, "PUT");
    assert!(
        plan.existing[0].changes.is_empty(),
        "{:?}",
        plan.existing[0].changes
    );
    let new: Vec<&str> = plan.new.iter().map(|n| n.route.method.as_str()).collect();
    assert_eq!(new, ["PATCH"]);
}

#[test]
fn handler_status_codes() {
    let dir = TempDir::new("statuses");
    dir.write(
        "main.go",
        r#"package main

import (
	"net/http"

	"github.com/gin-gonic/gin"
)

func main() {
	r := gin.Default()
	r.GET("/orders/:id", getOrder)
	r.POST("/orders", createOrder)
	r.DELETE("/orders/:id", deleteOrder)
}

func getOrder(c *gin.Context) {
	if c.Param("id") == "" {
		c.JSON(http.StatusBadRequest, gin.H{"error": "id"})
		return
	}
	if missing {
		c.AbortWithStatus(404)
		return
	}
	c.JSON(http.StatusOK, Order{})
}

func createOrder(c *gin.Context) {
	if exists {
		http.Error(c.Writer, "exists", http.StatusConflict)
		return
	}
	c.JSON(201, Order{})
}

func deleteOrder(c *gin.Context) {
	strconv.ParseInt(c.Param("id"), 10, 64)
	time.Sleep(200)
}

type Order struct {
	ID int `json:"id"`
}
"#,
    );
    let s = scan(&dir);
    let codes = |method, path| -> Vec<(u16, usize, bool)> {
        find(&s, method, path)
            .info
            .statuses
            .iter()
            .map(|c| (c.code, c.line, c.implicit))
            .collect()
    };
    assert_eq!(
        codes("GET", "/orders/{{id}}"),
        [(200, 25, false), (400, 18, false), (404, 22, false)]
    );
    assert_eq!(
        codes("POST", "/orders"),
        [(201, 33, false), (409, 30, false)]
    );
    // Ни одного кода в коде: 200 по умолчанию, место — тело обработчика.
    assert_eq!(codes("DELETE", "/orders/{{id}}"), [(200, 36, true)]);
}

#[test]
fn coverage_levels_and_statuses() {
    let dir = TempDir::new("coverage");
    dir.write(
        "svc/main.go",
        r#"package main

import "net/http"

func main() {
	http.HandleFunc("GET /orders/{id}", getOrder)
	http.HandleFunc("POST /orders", createOrder)
	http.HandleFunc("DELETE /orders/{id}", deleteOrder)
	http.HandleFunc("GET /health", health)
	http.HandleFunc("GET /legacy", legacy)
}

func getOrder(w http.ResponseWriter, r *http.Request) {
	http.Error(w, "no", http.StatusNotFound)
	w.WriteHeader(http.StatusOK)
}

func createOrder(w http.ResponseWriter, r *http.Request) {
	w.WriteHeader(http.StatusCreated)
}

func deleteOrder(w http.ResponseWriter, r *http.Request) {
	w.WriteHeader(204)
}

func health(w http.ResponseWriter, r *http.Request) {}

func legacy(w http.ResponseWriter, r *http.Request) {
	http.Error(w, "gone", http.StatusGone)
}
"#,
    );
    dir.write("api/env.toml", "[env.dev]\nbase = \"http://x\"\n");
    dir.write(
        "api/orders.outry",
        r#"GetOrder: GET /orders/{id} {
  expect { status == 200 }
}

CreateOrder: POST /orders {
  body { sku: "A" }
}

DeleteOrder: DELETE /orders/{id}

flow Missing {
  order = CreateOrder()
  expect {
    order.status in [201, 409]
    GetOrder(id: 0).status == 404
  }
}
"#,
    );
    dir.write(
        "api/legacy.http",
        "GET {{base}}/legacy\n\n> assert status == 410\n",
    );
    let scan = scan_go(&dir.0.join("svc"), &[]).unwrap();
    let cov = coverage(&dir.0.join("api"), scan, DEFAULT_BASE).unwrap();
    let row = |method: &str, path: &str| {
        cov.routes
            .iter()
            .find(|r| r.method == method && r.path == path)
            .unwrap_or_else(|| panic!("no {method} {path}"))
    };
    let checked = |r: &RouteCoverage| -> Vec<(u16, bool)> {
        r.statuses
            .iter()
            .map(|s| (s.status.code, s.checked))
            .collect()
    };

    let get = row("GET", "/orders/{id}");
    assert_eq!(get.level, Level::Checked);
    assert_eq!(checked(get), [(200, true), (404, true)]);
    assert_eq!(get.requests[0].name.as_deref(), Some("GetOrder"));

    // Без своего `expect`, но проверен в сценарии.
    let create = row("POST", "/orders");
    assert_eq!(create.level, Level::Checked);
    assert_eq!(checked(create), [(201, true)]);

    let delete = row("DELETE", "/orders/{id}");
    assert_eq!(delete.level, Level::Request);
    assert_eq!(checked(delete), [(204, false)]);

    let health = row("GET", "/health");
    assert_eq!(health.level, Level::None);
    assert!(health.requests.is_empty());
    assert_eq!(checked(health), [(200, false)]);

    let legacy = row("GET", "/legacy");
    assert_eq!(legacy.level, Level::Checked);
    assert_eq!(checked(legacy), [(200, false), (410, true)]);

    assert_eq!(cov.at_least(Level::Request), 4);
    assert_eq!(cov.percent(), 60.0);
    assert_eq!(cov.statuses(), (4, 7));
    assert!(!cov.runs && cov.routes.iter().all(|r| r.executed.is_none()));

    // С журналом прогона: что реально отправлялось и какие коды пришли.
    let sent =
        |file: &str, line, name: Option<&str>, method: &str, url: &str, status, via: &[&str]| {
            crate::runner::Executed {
                file: file.into(),
                line,
                name: name.map(str::to_string),
                method: method.into(),
                url: url.into(),
                status,
                via: via.iter().map(|s| s.to_string()).collect(),
            }
        };
    let log = RunLog::new(
        "dev",
        vec![
            // Строка сдвинулась после прогона — находим по имени.
            sent(
                "orders.outry",
                99,
                Some("GetOrder"),
                "GET",
                "http://x/orders/0",
                404,
                &["Missing"],
            ),
            sent(
                "orders.outry",
                5,
                Some("CreateOrder"),
                "POST",
                "http://x/orders",
                201,
                &["Missing"],
            ),
            sent(
                "orders.outry",
                5,
                Some("CreateOrder"),
                "POST",
                "http://x/orders",
                201,
                &[],
            ),
            sent("legacy.http", 0, None, "GET", "http://x/legacy", 410, &[]),
            // Файла больше нет — по методу и пути URL.
            sent(
                "gone.outry",
                1,
                Some("Health"),
                "GET",
                "http://x/health?full=1",
                200,
                &[],
            ),
        ],
    );
    assert_eq!(log.requests.len(), 4, "duplicates merged, direct run kept");
    assert!(log.requests[1].via.is_empty());
    let text = serde_json::to_string(&log).unwrap();
    let log = RunLog::parse(&text).unwrap();
    assert!(RunLog::parse(&text.replace("\"version\":1", "\"version\":9")).is_err());

    let scan = scan_go(&dir.0.join("svc"), &[]).unwrap();
    let cov = coverage_runs(&dir.0.join("api"), scan, DEFAULT_BASE, &log.requests).unwrap();
    let row = |method: &str, path: &str| {
        cov.routes
            .iter()
            .find(|r| r.method == method && r.path == path)
            .unwrap()
    };
    let seen = |r: &RouteCoverage| -> Vec<(u16, Option<bool>)> {
        r.statuses.iter().map(|s| (s.status.code, s.seen)).collect()
    };
    assert!(cov.runs);
    let get = row("GET", "/orders/{id}");
    assert_eq!(get.executed, Some(true));
    assert_eq!(get.via, ["Missing"], "only called from the flow");
    assert_eq!(seen(get), [(200, Some(false)), (404, Some(true))]);
    let create = row("POST", "/orders");
    assert_eq!((create.executed, create.via.len()), (Some(true), 0));
    assert_eq!(row("DELETE", "/orders/{id}").executed, Some(false));
    assert_eq!(row("GET", "/health").executed, Some(true));
    assert_eq!(
        seen(row("GET", "/legacy")),
        [(200, Some(false)), (410, Some(true))]
    );
    assert_eq!(cov.executed(), 4);
    assert_eq!(cov.executed_percent(), 80.0);
}

#[test]
fn echo_routes_groups_middleware_and_handlers() {
    let dir = TempDir::new("echo");
    dir.write(
        "main.go",
        r#"package main

import (
	"net/http"

	"github.com/labstack/echo/v4"
	"github.com/labstack/echo/v4/middleware"
	"example.com/svc/users"
)

type createReq struct {
	Name string `json:"name"`
}

type Order struct {
	ID int `json:"id"`
}

func main() {
	e := echo.New()
	e.Use(middleware.Logger())
	e.GET("/health", health)
	e.Add(http.MethodPut, "/config", putConfig)
	e.Match([]string{"GET", http.MethodHead}, "/ping", ping)
	e.Any("/proxy/*", proxy)
	api := e.Group("/api", auth)
	api.POST("/orders", createOrder, rateLimit)
	api.GET("/orders/:id", getOrder)
	e.Group("/inline").DELETE("/x", h)
	users.Register(api.Group("/users"))
	e.Start(":8080")
}

func createOrder(c echo.Context) error {
	var req createReq
	if err := c.Bind(&req); err != nil {
		return c.JSON(http.StatusBadRequest, err)
	}
	_ = c.QueryParam("dry")
	_ = c.Request().Header.Get("X-Request-Id")
	return c.JSON(http.StatusCreated, Order{})
}

func getOrder(c echo.Context) error {
	if c.Param("id") == "" {
		return echo.NewHTTPError(http.StatusNotFound, "no order")
	}
	return c.JSON(http.StatusOK, Order{})
}
"#,
    );
    dir.write(
        "users/users.go",
        r#"package users

import "github.com/labstack/echo/v4"

func Register(g *echo.Group) {
	g.GET("/:id", get)
}
"#,
    );
    let s = scan(&dir);
    assert_eq!(
        routes(&s),
        sorted(&[
            "PUT /config",
            "GET /health",
            "DELETE /inline/x",
            "POST /api/orders",
            "GET /api/orders/{{id}}",
            "GET /api/users/{{id}}",
            "GET /ping",
            "HEAD /ping",
            "ANY /proxy/{{path}}",
        ])
    );
    assert_eq!(
        middleware(&s, "POST", "/api/orders"),
        ["middleware.Logger()", "auth", "rateLimit"]
    );
    assert_eq!(
        middleware(&s, "GET", "/api/users/{{id}}"),
        ["middleware.Logger()", "auth"]
    );

    let create = &find(&s, "POST", "/api/orders").info;
    assert_eq!(create.body.as_ref().unwrap().type_name, "createReq");
    assert_eq!(query_names(find(&s, "POST", "/api/orders")), ["dry"]);
    assert_eq!(create.headers, ["X-Request-Id"]);
    assert_eq!(create.response.as_ref().unwrap().type_name, "Order");
    let codes: Vec<u16> = create.statuses.iter().map(|c| c.code).collect();
    assert_eq!(codes, [201, 400]);
    let get = &find(&s, "GET", "/api/orders/{{id}}").info;
    let codes: Vec<u16> = get.statuses.iter().map(|c| c.code).collect();
    assert_eq!(codes, [200, 404]);
}

#[test]
fn fiber_routes_groups_mounts_and_handlers() {
    let dir = TempDir::new("fiber");
    dir.write(
        "main.go",
        r#"package main

import (
	"github.com/gofiber/fiber/v2"
	"github.com/gofiber/fiber/v2/middleware/logger"
	"example.com/svc/admin"
)

type createReq struct {
	Name string `json:"name"`
}

type Order struct {
	ID int `json:"id"`
}

func main() {
	app := fiber.New()
	app.Use(logger.New())
	app.Get("/health", health)
	app.Add(fiber.MethodPut, "/config", putConfig)
	app.All("/files/+", files)
	api := app.Group("/api", auth)
	api.Post("/orders", rateLimit, createOrder)
	api.Get("/orders/:id<int>/:part?", getOrder)
	api.Route("/users", func(r fiber.Router) {
		r.Get("/", listUsers)
	})
	micro := fiber.New()
	micro.Get("/doe", doe)
	app.Mount("/john", micro)
	app.Mount("/admin", admin.New())
	app.Listen(":3000")
}

func createOrder(c *fiber.Ctx) error {
	var req createReq
	if err := c.BodyParser(&req); err != nil {
		return c.Status(fiber.StatusBadRequest).JSON(fiber.Map{"error": err.Error()})
	}
	_ = c.Query("dry")
	_ = c.QueryInt("page")
	_ = c.Get("X-Request-Id")
	_ = cache.Get(c.Context(), "orders")
	db.Delete(&Order{}, 1)
	return c.Status(fiber.StatusCreated).JSON(Order{})
}

func getOrder(c *fiber.Ctx) error {
	if missing {
		return c.Status(fiber.StatusNotFound).JSON(Order{})
	}
	return c.JSON(Order{})
}
"#,
    );
    dir.write(
        "admin/admin.go",
        r#"package admin

import "github.com/gofiber/fiber/v2"

func New() *fiber.App {
	app := fiber.New()
	app.Get("/stats", stats)
	return app
}
"#,
    );
    dir.write(
        "v3/v3.go",
        r#"package v3

import "github.com/gofiber/fiber/v3"

type loginReq struct {
	Login string `json:"login"`
}

func Register(app *fiber.App) {
	app.Add([]string{fiber.MethodGet, fiber.MethodPost}, "/v3/both", both)
	app.Post("/v3/login", login)
}

func login(c fiber.Ctx) error {
	var req loginReq
	if err := c.Bind().Body(&req); err != nil {
		return err
	}
	return c.JSON(req)
}
"#,
    );
    let s = scan(&dir);
    assert!(s.warnings.is_empty(), "{:?}", s.warnings);
    let want = sorted(&[
        "GET /admin/stats",
        "POST /api/orders",
        "GET /api/orders/{{id}}/{{part}}",
        "GET /api/users",
        "PUT /config",
        "ANY /files/{{path}}",
        "GET /health",
        "GET /john/doe",
        "GET /v3/both",
        "POST /v3/both",
        "POST /v3/login",
    ]);
    assert_eq!(routes(&s), want);
    assert_eq!(
        middleware(&s, "POST", "/api/orders"),
        ["logger.New()", "auth", "rateLimit"]
    );

    let create = &find(&s, "POST", "/api/orders").info;
    assert_eq!(create.body.as_ref().unwrap().type_name, "createReq");
    assert_eq!(
        query_names(find(&s, "POST", "/api/orders")),
        ["dry", "page"]
    );
    assert_eq!(create.headers, ["X-Request-Id"]);
    assert_eq!(create.response.as_ref().unwrap().type_name, "Order");
    let codes: Vec<u16> = create.statuses.iter().map(|c| c.code).collect();
    assert_eq!(codes, [201, 400]);
    let get = &find(&s, "GET", "/api/orders/{{id}}/{{part}}").info;
    assert_eq!(get.response.as_ref().unwrap().type_name, "Order");
    let codes: Vec<u16> = get.statuses.iter().map(|c| c.code).collect();
    assert_eq!(codes, [200, 404]);

    let login = &find(&s, "POST", "/v3/login").info;
    assert_eq!(login.body.as_ref().unwrap().type_name, "loginReq");
    assert_eq!(login.response.as_ref().unwrap().type_name, "loginReq");
}
