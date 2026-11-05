use super::*;
use crate::import::{Plan, RouteInfo, plan};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("outry-rust-{name}-{}", std::process::id()));
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

const MAIN: &str = r#"use axum::{Router, middleware, routing::get};
use tower_http::trace::TraceLayer;

mod error;
mod handlers;
mod models;

const API: &str = "/api/v1";

#[tokio::main]
async fn main() {
    let state = AppState::default();
    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .nest(API, api_router())
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    let app = app.merge(handlers::admin::routes(Router::new()));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

fn api_router() -> Router<AppState> {
    let users = Router::new()
        .route("/", get(handlers::users::list).post(handlers::users::create))
        .route(
            "/{id}",
            get(handlers::users::get).delete(handlers::users::delete),
        )
        .route_layer(middleware::from_fn(auth::require_user));
    Router::new()
        .nest("/users", users)
        .route("/files/{*path}", get(handlers::files::serve))
}

#[cfg(test)]
mod tests {
    fn app() -> Router {
        Router::new().route("/test-only", get(h))
    }
}
"#;

const USERS: &str = r#"use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
};

use crate::error::AppError;
use crate::models::{CreateUser, Page, User};

#[derive(Deserialize)]
pub struct ListParams {
    /// Page number
    pub page: Option<u32>,
    pub per_page: u32, // items per page
    #[serde(rename = "q")]
    pub search: Option<String>,
}

/// List users.
/// Supports paging.
#[tracing::instrument]
pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
    headers: HeaderMap,
) -> Result<Json<Page<User>>, AppError> {
    let tenant = headers.get("x-tenant-id");
    let auth = headers.get(header::AUTHORIZATION);
    Ok(Json(Page { items: vec![], total: 0 }))
}

pub async fn create(Json(body): Json<CreateUser>) -> Result<(StatusCode, Json<User>), AppError> {
    if body.email.is_empty() {
        return Err(AppError::Validation("email".into()));
    }
    Ok((StatusCode::CREATED, Json(user)))
}

pub async fn get(Path(id): Path<i64>) -> Result<Json<User>, AppError> {
    let user = find(id).ok_or(AppError::NotFound)?;
    Ok(Json(user))
}

pub async fn delete(Path(id): Path<i64>) -> StatusCode {
    StatusCode::NO_CONTENT
}
"#;

const MODELS: &str = r#"use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: i64,
    pub full_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    pub manager: Option<Box<User>>,
    pub role: Role,
    pub created_at: DateTime<Utc>,
    #[serde(skip)]
    pub password_hash: String,
    pub external_id: UserId,
}

pub struct UserId(pub Uuid);

#[derive(Serialize)]
pub enum Role {
    Admin,
    Member,
}

#[derive(Deserialize)]
pub struct CreateUser {
    pub email: String,
    pub full_name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub address: Option<Address>,
}

#[derive(Deserialize)]
pub struct Address {
    pub city: String,
    pub zip: String,
}

#[derive(Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u64,
}
"#;

const ERROR: &str = r#"pub enum AppError {
    NotFound,
    Validation(String),
    Internal(anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match self {
            AppError::NotFound => StatusCode::NOT_FOUND,
            Self::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        status.into_response()
    }
}
"#;

const ADMIN: &str = r#"use axum::{Json, Router, middleware::from_fn, response::IntoResponse, routing::get};
use tower::ServiceBuilder;
use tower_http::cors::CorsLayer;

pub fn routes(router: Router<AppState>) -> Router<AppState> {
    router.route("/admin/stats", get(stats)).layer(
        ServiceBuilder::new()
            .layer(from_fn(log_requests))
            .layer(CorsLayer::permissive()),
    )
}

async fn stats() -> impl IntoResponse {
    Json(Stats { users: 1 })
}

#[derive(Serialize)]
struct Stats {
    users: u64,
}
"#;

fn service() -> TempDir {
    let dir = TempDir::new("svc");
    dir.write("Cargo.toml", "[package]\nname = \"svc\"\n");
    dir.write("src/main.rs", MAIN);
    dir.write("src/error.rs", ERROR);
    dir.write("src/models.rs", MODELS);
    dir.write(
        "src/handlers/mod.rs",
        "pub mod admin;\npub mod files;\npub mod users;\n",
    );
    dir.write("src/handlers/users.rs", USERS);
    dir.write("src/handlers/admin.rs", ADMIN);
    dir.write(
        "src/handlers/files.rs",
        "pub async fn serve(Path(path): Path<String>) -> impl IntoResponse { path }\n",
    );
    dir.write("target/debug/build/x/out/gen.rs", MAIN);
    dir
}

fn find<'a>(scan: &'a Scan, method: &str, path: &str) -> &'a Route {
    scan.routes
        .iter()
        .find(|r| r.method == method && r.path == path)
        .unwrap_or_else(|| panic!("no {method} {path} in {:#?}", routes(scan)))
}

fn routes(scan: &Scan) -> Vec<String> {
    let mut out: Vec<String> = scan
        .routes
        .iter()
        .map(|r| {
            format!(
                "{} {} {} [{}]",
                r.method,
                r.path,
                r.handler.as_deref().unwrap_or("-"),
                r.middleware.join(", ")
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn axum_routes_prefixes_and_layers() {
    let dir = service();
    let scan = scan(&dir.0).unwrap();
    assert_eq!(
        routes(&scan),
        [
            "DELETE /api/v1/users/{{id}} users.delete [TraceLayer, auth.require_user]",
            "GET /admin/stats admin.stats [log_requests, CorsLayer]",
            "GET /api/v1/files/{{path}} files.serve [TraceLayer]",
            "GET /api/v1/users users.list [TraceLayer, auth.require_user]",
            "GET /api/v1/users/{{id}} users.get [TraceLayer, auth.require_user]",
            "GET /health - [TraceLayer]",
            "POST /api/v1/users users.create [TraceLayer, auth.require_user]",
        ],
        "{:?}",
        scan.warnings
    );
    let list = find(&scan, "GET", "/api/v1/users");
    assert_eq!(list.source, Path::new("src/main.rs"));
    assert_eq!(list.line, 25);
    assert_eq!(list.router, "axum");
}

fn info<'a>(scan: &'a Scan, method: &str, path: &str) -> &'a RouteInfo {
    &find(scan, method, path).info
}

#[test]
fn axum_handlers_are_described() {
    let dir = service();
    let scan = scan(&dir.0).unwrap();

    let list = info(&scan, "GET", "/api/v1/users");
    assert_eq!(list.summary.as_deref(), Some("List users"));
    assert_eq!(list.description, ["Supports paging."]);
    let query: Vec<(&str, bool, Option<&str>)> = list
        .query
        .iter()
        .map(|f| (f.name.as_str(), f.required, f.comment.as_deref()))
        .collect();
    assert_eq!(
        query,
        [
            ("page", false, Some("Page number")),
            ("per_page", true, Some("items per page")),
            ("q", false, None),
        ]
    );
    assert_eq!(list.headers, ["x-tenant-id", "Authorization"]);
    let resp = list.response.as_ref().unwrap();
    assert_eq!(resp.type_name, "Page<User>");
    assert_eq!(resp.shape, "{ items: [User], total: integer }");
    let codes: Vec<u16> = list.statuses.iter().map(|s| s.code).collect();
    assert_eq!(codes, [200]);
    assert!(list.statuses[0].implicit);

    let create = info(&scan, "POST", "/api/v1/users");
    let body = create.body.as_ref().unwrap();
    assert_eq!(body.type_name, "CreateUser");
    let fields: Vec<(&str, bool)> = body
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.required))
        .collect();
    assert_eq!(
        fields,
        [
            ("email", true),
            ("full_name", true),
            ("tags", false),
            ("address.city", true),
            ("address.zip", true),
        ]
    );
    assert_eq!(
        body.example,
        "{\n  \"email\": \"\",\n  \"full_name\": \"\",\n  \"tags\": [\"\"],\n  \"address\": {\n    \"city\": \"\",\n    \"zip\": \"\"\n  }\n}"
    );
    assert_eq!(create.response.as_ref().unwrap().shape, "User");
    let codes: Vec<u16> = create.statuses.iter().map(|s| s.code).collect();
    assert_eq!(codes, [201, 422]);

    let get = info(&scan, "GET", "/api/v1/users/{{id}}");
    let codes: Vec<(u16, bool)> = get.statuses.iter().map(|s| (s.code, s.implicit)).collect();
    assert_eq!(codes, [(200, true), (404, false)]);

    let delete = info(&scan, "DELETE", "/api/v1/users/{{id}}");
    let codes: Vec<u16> = delete.statuses.iter().map(|s| s.code).collect();
    assert_eq!(codes, [204]);

    let stats = info(&scan, "GET", "/admin/stats");
    assert_eq!(stats.response.as_ref().unwrap().shape, "Stats");

    let user = scan.shapes.iter().find(|s| s.name == "User").unwrap();
    assert_eq!(user.go_type, "models::User");
    assert_eq!(user.source, Path::new("src/models.rs"));
    assert_eq!(
        user.shape,
        "{ id: integer, fullName: string, avatarUrl?: string | null, manager: User | null, \
         role: string, createdAt: string, externalId: string }"
    );
}

#[test]
fn axum_plan_creates_requests() {
    let dir = service();
    let project = TempDir::new("svc-api");
    project.write("env.toml", "[vars]\nbase = \"http://localhost:3000\"\n");
    let plan: Plan = plan(&project.0, scan(&dir.0).unwrap(), "base").unwrap();
    let file = plan
        .new
        .iter()
        .find(|f| f.route.path == "/api/v1/users" && f.route.method == "POST")
        .unwrap();
    assert_eq!(file.file, Path::new("api/v1/users/post.outry"));
    assert!(
        file.content.contains("handler: users.create"),
        "{}",
        file.content
    );
    assert!(file.content.contains("full_name: \"\""), "{}", file.content);
    assert!(
        file.content.contains("CreateUsers: POST /api/v1/users {"),
        "{}",
        file.content
    );
    assert!(
        file.content.contains("body matches User"),
        "{}",
        file.content
    );
    let names: Vec<&str> = plan.new_shapes.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["User", "Stats"]);
}

#[test]
fn axum_helpers_defaults_and_error_constructors() {
    let dir = TempDir::new("edge");
    dir.write(
        "src/main.rs",
        r#"use axum::routing::{MethodFilter, get, on, post};

#[tokio::main]
async fn main() {
    let app = Router::new()
        .merge(route("/items/:id", get(show)))
        .merge(route("/files/*rest", on(MethodFilter::GET.or(MethodFilter::HEAD), files)));
    let app = add_admin(app);
    axum::serve(listener, app).await.unwrap();
}

fn route(path: &str, method_router: MethodRouter<()>) -> Router {
    Router::new().route(path, method_router)
}

fn add_admin(router: Router) -> Router {
    router.nest("/admin", Router::new().route("/items", post(create)))
}

struct ItemBody<T = Item> {
    item: T,
}

#[derive(Serialize)]
struct Item {
    id: u64,
}

async fn show() -> Result<Json<ItemBody>, Error> {
    Err(Error::gone())
}

async fn create() -> Result<Json<ItemBody>, Error> {
    Err(Error::NotFound)
}

async fn files() {}

enum Error {
    NotFound,
    Gone,
}

impl Error {
    fn gone() -> Self {
        Self::Gone
    }

    fn status(&self) -> StatusCode {
        match self {
            Error::NotFound => StatusCode::NOT_FOUND,
            Error::Gone => StatusCode::GONE,
        }
    }
}
"#,
    );
    let scan = scan(&dir.0).unwrap();
    assert_eq!(
        routes(&scan),
        [
            "GET /files/{{rest}} files []",
            "GET /items/{{id}} show []",
            "HEAD /files/{{rest}} files []",
            "POST /admin/items create []",
        ],
        "{:?}",
        scan.warnings
    );
    assert!(scan.warnings.is_empty(), "{:?}", scan.warnings);
    let show = info(&scan, "GET", "/items/{{id}}");
    assert_eq!(show.response.as_ref().unwrap().shape, "ItemBody");
    let codes: Vec<u16> = show.statuses.iter().map(|s| s.code).collect();
    assert_eq!(codes, [200, 410]);
    let create = info(&scan, "POST", "/admin/items");
    let codes: Vec<u16> = create.statuses.iter().map(|s| s.code).collect();
    assert_eq!(codes, [200, 404]);
    let body = scan.shapes.iter().find(|s| s.name == "ItemBody").unwrap();
    assert_eq!(body.shape, "{ item: Item }");
}
