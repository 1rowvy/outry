//! Разрешение переменных. Приоритет (сверху вниз):
//!
//! 1. `--var name=value` из CLI / ручные значения в GUI;
//! 2. значения, сохранённые `> save` в этой сессии;
//! 3. переменные окружения процесса `OUTRY_<NAME>` (удобно для секретов в CI);
//! 4. активное окружение из `env.toml`, затем общая секция `[vars]`;
//! 5. системное хранилище паролей (секреты).
//!
//! Имена на `$` — динамические (`{{$uuid}}`), см. `dynamic.rs`.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::dynamic;
use crate::error::Result;
use crate::secrets::SecretStore;

#[derive(Default)]
pub struct Vars {
    pub overrides: BTreeMap<String, String>,
    pub saved: BTreeMap<String, String>,
    pub env: BTreeMap<String, String>,
    pub secrets: Option<Box<dyn SecretStore>>,
    /// Читать ли `OUTRY_*` из окружения процесса. Выключается в тестах.
    pub process_env: bool,
}

/// Откуда пришло значение переменной.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Override,
    Saved,
    ProcessEnv,
    Env,
    Secret,
    Dynamic,
}

/// Переменная для панели в GUI и `outry vars`.
#[derive(Debug, Clone, Serialize)]
pub struct VarInfo {
    pub name: String,
    /// `None` — значения нет (объявленный, но не заданный секрет).
    pub value: Option<String>,
    pub source: Option<Source>,
    /// Значение не показывать целиком: из хранилища паролей или объявлено в `secrets`.
    pub secret: bool,
}

impl Vars {
    pub fn get(&self, name: &str) -> Result<Option<String>> {
        Ok(self.lookup(name)?.map(|(v, _)| v))
    }

    /// Значение вместе с источником.
    pub fn lookup(&self, name: &str) -> Result<Option<(String, Source)>> {
        if dynamic::is_dynamic(name) {
            return Ok(Some((dynamic::eval(name)?, Source::Dynamic)));
        }
        if let Some(v) = self.overrides.get(name) {
            return Ok(Some((v.clone(), Source::Override)));
        }
        if let Some(v) = self.saved.get(name) {
            return Ok(Some((v.clone(), Source::Saved)));
        }
        if self.process_env {
            if let Ok(v) = std::env::var(process_env_name(name)) {
                return Ok(Some((v, Source::ProcessEnv)));
            }
        }
        if let Some(v) = self.env.get(name) {
            return Ok(Some((v.clone(), Source::Env)));
        }
        match &self.secrets {
            Some(store) => Ok(store.get(name)?.map(|v| (v, Source::Secret))),
            None => Ok(None),
        }
    }

    /// Все известные переменные: из `--var`, `> save`, `env.toml` и объявленные секреты.
    /// `OUTRY_*` и хранилище паролей перечислить нельзя — они видны только для этих имён.
    pub fn list(&self, declared_secrets: &[String]) -> Result<Vec<VarInfo>> {
        let names: BTreeSet<&String> = self
            .overrides
            .keys()
            .chain(self.saved.keys())
            .chain(self.env.keys())
            .chain(declared_secrets)
            .collect();
        names
            .into_iter()
            .map(|name| {
                let found = self.lookup(name)?;
                Ok(VarInfo {
                    name: name.clone(),
                    secret: declared_secrets.contains(name)
                        || matches!(found, Some((_, Source::Secret))),
                    source: found.as_ref().map(|(_, s)| *s),
                    value: found.map(|(v, _)| v),
                })
            })
            .collect()
    }
}

/// Значение секрета для показа: первые символы и длина.
pub fn mask(value: &str) -> String {
    let n = value.chars().count();
    if n <= 8 {
        "•".repeat(n.max(1))
    } else {
        format!("{}…({n} chars)", value.chars().take(3).collect::<String>())
    }
}

/// `access-token` → `OUTRY_ACCESS_TOKEN`.
pub fn process_env_name(name: &str) -> String {
    let mut s = String::from("OUTRY_");
    s.extend(name.chars().map(|c| {
        if c.is_ascii_alphanumeric() {
            c.to_ascii_uppercase()
        } else {
            '_'
        }
    }));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemoryStore;

    #[test]
    fn priority() {
        let mut v = Vars::default();
        v.env.insert("a".into(), "env".into());
        v.secrets = Some(Box::new(MemoryStore::from([
            ("a", "secret"),
            ("s", "secret"),
        ])));
        assert_eq!(v.get("a").unwrap().as_deref(), Some("env"));
        assert_eq!(v.get("s").unwrap().as_deref(), Some("secret"));
        v.saved.insert("a".into(), "saved".into());
        assert_eq!(v.get("a").unwrap().as_deref(), Some("saved"));
        v.overrides.insert("a".into(), "cli".into());
        assert_eq!(v.get("a").unwrap().as_deref(), Some("cli"));
        assert_eq!(v.get("zzz").unwrap(), None);
    }

    #[test]
    fn sources_and_list() {
        let mut v = Vars::default();
        v.env.insert("base".into(), "http://x".into());
        v.saved.insert("id".into(), "7".into());
        v.secrets = Some(Box::new(MemoryStore::from([("token", "abcdefghijk")])));
        assert_eq!(v.lookup("id").unwrap().unwrap().1, Source::Saved);
        assert_eq!(v.lookup("$uuid").unwrap().unwrap().1, Source::Dynamic);
        assert!(v.lookup("$nope").is_err());

        let list = v.list(&["token".into(), "missing".into()]).unwrap();
        let names: Vec<_> = list.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["base", "id", "missing", "token"]);
        assert_eq!(list[0].source, Some(Source::Env));
        assert!(!list[0].secret);
        assert_eq!((list[2].source, list[2].secret), (None, true));
        assert_eq!(list[3].source, Some(Source::Secret));
        assert_eq!(mask(list[3].value.as_deref().unwrap()), "abc…(11 chars)");
        assert_eq!(mask("abc"), "•••");
    }

    #[test]
    fn env_names() {
        assert_eq!(process_env_name("access-token"), "OUTRY_ACCESS_TOKEN");
    }
}
