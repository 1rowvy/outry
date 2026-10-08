//! Разрешение переменных. Приоритет (сверху вниз):
//!
//! 1. `--var name=value` из CLI / ручные значения в GUI;
//! 2. значения, сохранённые `> save` в этой сессии;
//! 3. переменные окружения процесса `ROUTY_<NAME>` (удобно для секретов в CI);
//! 4. активное окружение из `env.toml`, затем общая секция `[vars]`;
//! 5. системное хранилище паролей (секреты).

use std::collections::BTreeMap;

use crate::error::Result;
use crate::secrets::SecretStore;

#[derive(Default)]
pub struct Vars {
    pub overrides: BTreeMap<String, String>,
    pub saved: BTreeMap<String, String>,
    pub env: BTreeMap<String, String>,
    pub secrets: Option<Box<dyn SecretStore>>,
    /// Читать ли `ROUTY_*` из окружения процесса. Выключается в тестах.
    pub process_env: bool,
}

impl Vars {
    pub fn get(&self, name: &str) -> Result<Option<String>> {
        if let Some(v) = self.overrides.get(name).or_else(|| self.saved.get(name)) {
            return Ok(Some(v.clone()));
        }
        if self.process_env {
            if let Ok(v) = std::env::var(process_env_name(name)) {
                return Ok(Some(v));
            }
        }
        if let Some(v) = self.env.get(name) {
            return Ok(Some(v.clone()));
        }
        match &self.secrets {
            Some(store) => store.get(name),
            None => Ok(None),
        }
    }
}

/// `access-token` → `ROUTY_ACCESS_TOKEN`.
pub fn process_env_name(name: &str) -> String {
    let mut s = String::from("ROUTY_");
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
    fn env_names() {
        assert_eq!(process_env_name("access-token"), "ROUTY_ACCESS_TOKEN");
    }
}
