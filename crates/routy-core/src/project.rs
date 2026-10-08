//! Проект — каталог с `env.toml` (обычно `api/`). Формат:
//!
//! ```toml
//! project = "my-api"   # пространство имён для секретов; по умолчанию — имя каталога
//! default = "dev"      # окружение по умолчанию
//! secrets = ["token"]  # какие переменные — секреты: GUI подскажет, если их нет
//!
//! [vars]               # общие для всех окружений
//! version = "v1"
//!
//! [env.dev]
//! base = "http://localhost:8080"
//!
//! [env.prod]
//! base = "https://api.example.com"
//! ```
//!
//! Секреты (токены, пароли) сюда не пишутся: `routy secret set <name> --env <env>`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};

pub const CONFIG_FILE: &str = "env.toml";

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub project: Option<String>,
    pub default: Option<String>,
    /// Имена секретов. Значения — в хранилище паролей или `ROUTY_*`, не в файле.
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub vars: BTreeMap<String, toml::Value>,
    #[serde(default)]
    pub env: BTreeMap<String, BTreeMap<String, toml::Value>>,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    pub config: Config,
}

impl Project {
    pub fn load(root: impl Into<PathBuf>) -> Result<Project> {
        let root = root.into();
        let path = root.join(CONFIG_FILE);
        let config = match std::fs::read_to_string(&path) {
            Ok(src) => toml::from_str(&src).map_err(|e| Error::from(e).in_file(&path))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(e) => return Err(Error::from(e).in_file(&path)),
        };
        Ok(Project { root, config })
    }

    /// Ищет `env.toml` (или `api/env.toml`) вверх от `start` — файла или каталога.
    /// Не нашли — проектом считается сам каталог `start`, без окружений.
    pub fn discover(start: &Path) -> Result<Project> {
        let start = std::path::absolute(start)?;
        let start_dir = if start.is_dir() {
            start.clone()
        } else {
            start.parent().unwrap_or(&start).to_path_buf()
        };
        for dir in start_dir.ancestors() {
            if dir.join(CONFIG_FILE).is_file() {
                return Project::load(dir);
            }
            let api = dir.join("api");
            if api.join(CONFIG_FILE).is_file() {
                return Project::load(api);
            }
            if dir.join(".git").exists() {
                break;
            }
        }
        Project::load(start_dir)
    }

    /// Имя для пространства секретов и сохранённого состояния.
    pub fn id(&self) -> String {
        if let Some(p) = &self.config.project {
            return p.clone();
        }
        let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
        match name(&self.root).as_deref() {
            Some("api") => self.root.parent().and_then(name),
            other => other.map(str::to_string),
        }
        .unwrap_or_else(|| "default".into())
    }

    pub fn env_names(&self) -> Vec<String> {
        self.config.env.keys().cloned().collect()
    }

    /// Выбранное окружение: явно указанное → `default` → первое по алфавиту → `default`.
    pub fn resolve_env(&self, requested: Option<&str>) -> Result<String> {
        if let Some(name) = requested {
            if !self.config.env.is_empty() && !self.config.env.contains_key(name) {
                return Err(Error::UnknownEnv(name.into()));
            }
            return Ok(name.into());
        }
        if let Some(d) = &self.config.default {
            if !self.config.env.contains_key(d) {
                return Err(Error::UnknownEnv(d.clone()));
            }
            return Ok(d.clone());
        }
        Ok(self
            .config
            .env
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "default".into()))
    }

    /// Переменные окружения `env` поверх общих `[vars]`.
    pub fn env_vars(&self, env: &str) -> BTreeMap<String, String> {
        let mut out: BTreeMap<String, String> = self
            .config
            .vars
            .iter()
            .map(|(k, v)| (k.clone(), toml_to_string(v)))
            .collect();
        if let Some(e) = self.config.env.get(env) {
            out.extend(e.iter().map(|(k, v)| (k.clone(), toml_to_string(v))));
        }
        out
    }
}

impl Project {
    /// Есть ли у проекта `env.toml` (или он открыт «как есть», без окружений).
    pub fn has_config(&self) -> bool {
        self.root.join(CONFIG_FILE).is_file()
    }
}

/// Проверяет текст `env.toml`, ничего не загружая — для редактора в GUI.
/// Ошибка с известной позицией — `Error::Parse` с номером строки (с 1).
pub fn check_config(src: &str) -> Result<()> {
    toml::from_str::<Config>(src)
        .map(|_| ())
        .map_err(|e| match e.span() {
            Some(span) => Error::parse(src[..span.start].matches('\n').count() + 1, e.message()),
            None => Error::from(e),
        })
}

const INIT_CONFIG: &str = "\
# Routy environments. Secret values don't go here: `routy secret set token --env dev`.
default = \"dev\"
# secrets = [\"token\"]

[env.dev]
base = \"http://localhost:8080\"

[env.prod]
base = \"https://api.example.com\"
";

const INIT_EXAMPLE: &str = "GET {{base}}/health\n\n> assert status == 200\n";

/// Создаёт `api/env.toml` и пример запроса. Если `dir` сам называется `api`,
/// файлы кладутся прямо в него. Возвращает каталог проекта (тот, где `env.toml`).
pub fn init(dir: &Path) -> Result<PathBuf> {
    let api = if dir.file_name().is_some_and(|n| n == "api") {
        dir.to_path_buf()
    } else {
        dir.join("api")
    };
    let config = api.join(CONFIG_FILE);
    if config.exists() {
        return Err(Error::from(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "already exists",
        ))
        .in_file(config));
    }
    std::fs::create_dir_all(&api)?;
    std::fs::write(&config, INIT_CONFIG)?;
    let example = api.join("health").join("get.http");
    if !example.exists() {
        std::fs::create_dir_all(example.parent().expect("has parent"))?;
        std::fs::write(example, INIT_EXAMPLE)?;
    }
    Ok(api)
}

fn toml_to_string(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(src: &str) -> Project {
        Project {
            root: PathBuf::from("/x/myapp/api"),
            config: toml::from_str(src).unwrap(),
        }
    }

    #[test]
    fn envs_and_vars() {
        let p = project(
            r#"
            default = "dev"
            secrets = ["token"]
            [vars]
            version = "v1"
            port = 8080
            [env.dev]
            base = "http://localhost"
            [env.prod]
            base = "https://prod"
            version = "v2"
            "#,
        );
        assert_eq!(p.id(), "myapp");
        assert_eq!(p.resolve_env(None).unwrap(), "dev");
        assert!(p.resolve_env(Some("stage")).is_err());
        let prod = p.env_vars("prod");
        assert_eq!(prod["base"], "https://prod");
        assert_eq!(prod["version"], "v2");
        assert_eq!(prod["port"], "8080");
        assert_eq!(p.config.secrets, ["token"]);
    }

    #[test]
    fn no_envs() {
        let p = project("project = \"svc\"");
        assert_eq!(p.id(), "svc");
        assert_eq!(p.resolve_env(None).unwrap(), "default");
        assert_eq!(p.resolve_env(Some("anything")).unwrap(), "anything");
    }

    #[test]
    fn init_creates_loadable_project() {
        let dir = std::env::temp_dir().join(format!("routy-init-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let api = init(&dir).unwrap();
        assert_eq!(api, dir.join("api"));
        let p = Project::discover(&dir).unwrap();
        assert_eq!(p.root, api);
        assert!(p.has_config());
        assert_eq!(p.resolve_env(None).unwrap(), "dev");
        assert!(api.join("health/get.http").is_file());
        assert!(init(&dir).is_err(), "second init must not overwrite");

        // Открыли сам каталог api/ без env.toml — не создаём api/api/.
        std::fs::remove_file(api.join(CONFIG_FILE)).unwrap();
        assert_eq!(init(&api).unwrap(), api);

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn check_config_reports_line() {
        assert!(check_config(INIT_CONFIG).is_ok());
        match check_config("default = \"dev\"\n\n[env.dev]\nbase = \n") {
            Err(Error::Parse { line, .. }) => assert_eq!(line, 4),
            other => panic!("expected parse error, got {other:?}"),
        }
        match check_config("defualt = \"dev\"") {
            Err(Error::Parse { line, msg }) => {
                assert_eq!(line, 1);
                assert!(msg.contains("defualt"), "{msg}");
            }
            other => panic!("expected parse error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(toml::from_str::<Config>("defualt = \"dev\"").is_err());
    }
}
