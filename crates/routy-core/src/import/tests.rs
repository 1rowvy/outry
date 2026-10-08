use super::*;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("routy-import-{name}-{}", std::process::id()));
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
    }
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
    let scan = Scan {
        files: 1,
        routes: vec![
            route("GET", "/users"),
            route("GET", "/users/{{id}}"),
            route("POST", "/users"),
            route("ANY", "/"),
            route("DELETE", "/users/{{id}}/posts/{{post}}"),
        ],
        warnings: vec![],
    };
    let plan = plan(&dir.0, scan, DEFAULT_BASE).unwrap();
    let new: Vec<_> = plan.new.iter().map(|f| f.file.clone()).collect();
    assert_eq!(
        new,
        [
            PathBuf::from("users/get-2.http"),
            PathBuf::from("users/post.http"),
            PathBuf::from("root/get.http"),
            PathBuf::from("users/posts/delete-by-post.http"),
        ]
    );
    assert_eq!(plan.existing.len(), 1);
    assert_eq!(plan.existing[0].file, Path::new("users/one.http"));
    assert_eq!(plan.stale.len(), 1);
    assert_eq!(plan.stale[0].file, Path::new("users/get.http"));

    let post = &plan.new[1].content;
    assert_eq!(post, "# chi main.go:7 → h.Get\nPOST {{base}}/users\n\n{}\n");
    crate::parse(post).unwrap();
    crate::parse(&plan.new[2].content).unwrap();
    assert!(
        plan.new[2]
            .content
            .contains("# any method\nGET {{base}}/\n")
    );

    let created = apply(&dir.0, &plan).unwrap();
    assert_eq!(created, new);
    // Второй прогон: всё уже есть.
    let again = super::plan(
        &dir.0,
        Scan {
            files: 1,
            routes: plan.new.iter().map(|f| f.route.clone()).collect(),
            warnings: vec![],
        },
        DEFAULT_BASE,
    )
    .unwrap();
    assert!(again.new.is_empty());
    assert_eq!(again.existing.len(), 4);
    assert!(apply(&dir.0, &plan).unwrap().is_empty(), "never overwrites");
}
