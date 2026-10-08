use std::path::PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("line {line}: {msg}")]
    Parse { line: usize, msg: String },

    // Вложенная ошибка уже в тексте, поэтому не `#[source]`: иначе anyhow печатает её дважды.
    #[error("{path}: {inner}")]
    InFile { path: PathBuf, inner: Box<Error> },

    #[error("undefined variable(s): {}", .0.join(", "))]
    MissingVars(Vec<String>),

    #[error("expression `{expr}`: {msg}")]
    Expr { expr: String, msg: String },

    #[error("unknown environment `{0}`")]
    UnknownEnv(String),

    #[error("invalid env.toml: {0}")]
    Config(#[from] toml::de::Error),

    #[error("http: {0}")]
    Http(#[from] reqwest::Error),

    #[error("import: {0}")]
    Import(String),

    #[error("secret store: {0}")]
    Secret(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl Error {
    pub(crate) fn parse(line: usize, msg: impl Into<String>) -> Self {
        Error::Parse {
            line,
            msg: msg.into(),
        }
    }

    pub(crate) fn expr(expr: &str, msg: impl Into<String>) -> Self {
        Error::Expr {
            expr: expr.to_string(),
            msg: msg.into(),
        }
    }

    pub fn in_file(self, path: impl Into<PathBuf>) -> Self {
        Error::InFile {
            path: path.into(),
            inner: Box::new(self),
        }
    }
}
