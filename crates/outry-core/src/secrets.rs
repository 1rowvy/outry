//! Секреты живут в системном хранилище паролей, а не в файлах репозитория.
//! Ключ записи: `<project>/<env>/<name>`, сервис — `outry`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};

use crate::error::Result;

/// Первая ошибка доступа к системному хранилищу в этом процессе (нет D-Bus / Secret Service,
/// например в WSL или headless CI). `get` в таком случае молча отвечает «нет секрета»,
/// а CLI по этому значению объясняет, откуда взялись «undefined variable».
static UNAVAILABLE: OnceLock<String> = OnceLock::new();
/// Имена, которые искали в недоступном хранилище: для готовой команды в подсказке.
static MISSED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Почему системное хранилище паролей недоступно, если к нему уже обращались и не смогли.
pub fn unavailable() -> Option<&'static str> {
    UNAVAILABLE.get().map(String::as_str)
}

/// Переменные, которые не нашлись из-за недоступного хранилища.
pub fn missed() -> Vec<String> {
    MISSED.lock().unwrap().iter().cloned().collect()
}

pub trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<String>>;
    fn set(&self, name: &str, value: &str) -> Result<()>;
    /// `true`, если запись существовала.
    fn delete(&self, name: &str) -> Result<bool>;
}

/// Хранилище в памяти: для тестов и для сборок без фичи `secrets`.
#[derive(Default)]
pub struct MemoryStore(Mutex<BTreeMap<String, String>>);

impl<const N: usize> From<[(&str, &str); N]> for MemoryStore {
    fn from(items: [(&str, &str); N]) -> Self {
        MemoryStore(Mutex::new(
            items
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        ))
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, name: &str) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(name).cloned())
    }
    fn set(&self, name: &str, value: &str) -> Result<()> {
        self.0.lock().unwrap().insert(name.into(), value.into());
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<bool> {
        Ok(self.0.lock().unwrap().remove(name).is_some())
    }
}

#[cfg(feature = "secrets")]
pub use keyring_store::KeyringStore;

#[cfg(feature = "secrets")]
mod keyring_store {
    use super::SecretStore;
    use crate::error::{Error, Result};

    const SERVICE: &str = "outry";

    pub struct KeyringStore {
        prefix: String,
    }

    impl KeyringStore {
        pub fn new(project: &str, env: &str) -> Self {
            KeyringStore {
                prefix: format!("{project}/{env}"),
            }
        }

        fn entry(&self, name: &str) -> Result<keyring::Entry> {
            keyring::Entry::new(SERVICE, &format!("{}/{name}", self.prefix)).map_err(secret_err)
        }
    }

    fn secret_err(e: keyring::Error) -> Error {
        Error::Secret(e.to_string())
    }

    impl SecretStore for KeyringStore {
        fn get(&self, name: &str) -> Result<Option<String>> {
            match self.entry(name)?.get_password() {
                Ok(v) => Ok(Some(v)),
                Err(keyring::Error::NoEntry) => Ok(None),
                // Нет D-Bus / Secret Service (например, headless CI) — считаем, что секрета нет,
                // переменная придёт из `OUTRY_*` или будет ошибка «undefined variable».
                Err(
                    e @ (keyring::Error::PlatformFailure(_) | keyring::Error::NoStorageAccess(_)),
                ) => {
                    let _ = super::UNAVAILABLE.set(e.to_string());
                    super::MISSED.lock().unwrap().insert(name.to_string());
                    Ok(None)
                }
                Err(e) => Err(secret_err(e)),
            }
        }

        fn set(&self, name: &str, value: &str) -> Result<()> {
            self.entry(name)?.set_password(value).map_err(secret_err)
        }

        fn delete(&self, name: &str) -> Result<bool> {
            match self.entry(name)?.delete_credential() {
                Ok(()) => Ok(true),
                Err(keyring::Error::NoEntry) => Ok(false),
                Err(e) => Err(secret_err(e)),
            }
        }
    }
}
