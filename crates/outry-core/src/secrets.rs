//! Секреты живут в системном хранилище паролей, а не в файлах репозитория.
//! Ключ записи: `<project>/<env>/<name>`, сервис — `outry`.
//! Где системного хранилища нет (WSL, сервер без GUI) — запасной файл пользователя
//! `<config_dir>/outry/secrets.toml` (права 600), см. [`FileStore`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::error::{Error, Result};

/// Первая ошибка доступа к системному хранилищу в этом процессе (нет D-Bus / Secret Service,
/// например в WSL или headless CI). Тогда секреты читаются и пишутся в [`FileStore`].
static UNAVAILABLE: OnceLock<String> = OnceLock::new();
/// `(env, name)`, которых нет ни в недоступном системном хранилище, ни в запасном файле:
/// для готовой команды в подсказке CLI.
static MISSED: Mutex<BTreeSet<(String, String)>> = Mutex::new(BTreeSet::new());

/// Почему системное хранилище паролей недоступно, если к нему уже обращались и не смогли.
pub fn unavailable() -> Option<&'static str> {
    UNAVAILABLE.get().map(String::as_str)
}

/// `(env, name)` секретов, не найденных из-за недоступного системного хранилища.
pub fn missed() -> Vec<(String, String)> {
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

/// Запасной файл секретов: `[<project>.<env>] name = "value"`. Открытый текст, поэтому
/// только вне репозитория и только для владельца (на Unix — 600).
pub struct FileStore {
    path: PathBuf,
    project: String,
    env: String,
}

impl FileStore {
    pub fn new(path: PathBuf, project: &str, env: &str) -> Self {
        FileStore {
            path,
            project: project.into(),
            env: env.into(),
        }
    }

    /// `<config_dir>/outry/secrets.toml` (`~/.config/outry/secrets.toml` на Linux).
    pub fn default_path() -> Option<PathBuf> {
        Some(dirs::config_dir()?.join("outry").join("secrets.toml"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load(&self) -> Result<toml::Table> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => text
                .parse()
                .map_err(|e| Error::Secret(format!("{}: {e}", self.path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(toml::Table::new()),
            Err(e) => Err(e.into()),
        }
    }

    fn save(&self, table: &toml::Table) -> Result<()> {
        let dir = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let text = format!(
            "# Outry secrets for machines without a system keychain (WSL, headless).\n\
             # Written by `outry secret set`; keep this file private.\n\n{}",
            toml::to_string(table).map_err(|e| Error::Secret(e.to_string()))?
        );
        // Через временный файл: при сбое старые секреты не теряются; права — до записи значений.
        let tmp = self.path.with_extension("toml.tmp");
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        std::io::Write::write_all(&mut opts.open(&tmp)?, text.as_bytes())?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    fn section<'a>(&self, table: &'a toml::Table) -> Option<&'a toml::Table> {
        table
            .get(&self.project)?
            .as_table()?
            .get(&self.env)?
            .as_table()
    }
}

impl SecretStore for FileStore {
    fn get(&self, name: &str) -> Result<Option<String>> {
        let table = self.load()?;
        Ok(self
            .section(&table)
            .and_then(|s| s.get(name))
            .and_then(|v| v.as_str())
            .map(str::to_string))
    }

    fn set(&self, name: &str, value: &str) -> Result<()> {
        let mut table = self.load()?;
        let section = table
            .entry(self.project.clone())
            .or_insert_with(|| toml::Table::new().into())
            .as_table_mut()
            .and_then(|p| {
                p.entry(self.env.clone())
                    .or_insert_with(|| toml::Table::new().into())
                    .as_table_mut()
            })
            .ok_or_else(|| {
                Error::Secret(format!(
                    "{}: `{}.{}` is not a table",
                    self.path.display(),
                    self.project,
                    self.env
                ))
            })?;
        section.insert(name.into(), value.into());
        self.save(&table)
    }

    fn delete(&self, name: &str) -> Result<bool> {
        let mut table = self.load()?;
        let removed = table
            .get_mut(&self.project)
            .and_then(|p| p.as_table_mut())
            .and_then(|p| p.get_mut(&self.env))
            .and_then(|e| e.as_table_mut())
            .is_some_and(|e| e.remove(name).is_some());
        if removed {
            self.save(&table)?;
        }
        Ok(removed)
    }
}

#[cfg(feature = "secrets")]
pub use keyring_store::KeyringStore;

#[cfg(feature = "secrets")]
mod keyring_store {
    use super::{FileStore, SecretStore};
    use crate::error::{Error, Result};

    const SERVICE: &str = "outry";

    /// Системное хранилище паролей; если его нет — запасной [`FileStore`].
    pub struct KeyringStore {
        prefix: String,
        env: String,
        fallback: Option<FileStore>,
    }

    impl KeyringStore {
        pub fn new(project: &str, env: &str) -> Self {
            KeyringStore {
                prefix: format!("{project}/{env}"),
                env: env.into(),
                fallback: FileStore::default_path().map(|p| FileStore::new(p, project, env)),
            }
        }

        fn entry(&self, name: &str) -> Result<keyring::Entry> {
            keyring::Entry::new(SERVICE, &format!("{}/{name}", self.prefix)).map_err(secret_err)
        }

        fn fallback(&self, e: keyring::Error) -> Result<&FileStore> {
            let _ = super::UNAVAILABLE.set(e.to_string());
            self.fallback.as_ref().ok_or_else(|| secret_err(e))
        }
    }

    fn secret_err(e: keyring::Error) -> Error {
        Error::Secret(e.to_string())
    }

    /// Нет D-Bus / Secret Service и т. п.: системного хранилища здесь просто нет.
    fn unavailable(e: &keyring::Error) -> bool {
        matches!(
            e,
            keyring::Error::PlatformFailure(_) | keyring::Error::NoStorageAccess(_)
        )
    }

    impl SecretStore for KeyringStore {
        fn get(&self, name: &str) -> Result<Option<String>> {
            match self.entry(name)?.get_password() {
                Ok(v) => Ok(Some(v)),
                // Файл мог остаться с тех пор, как системного хранилища не было.
                Err(keyring::Error::NoEntry) => match &self.fallback {
                    Some(file) => file.get(name),
                    None => Ok(None),
                },
                // Системного хранилища нет — запасной файл; не нашлось и там — секрета нет,
                // переменная придёт из `OUTRY_*` или будет ошибка «undefined variable»
                // (CLI подскажет команду по `missed()`).
                Err(e) if unavailable(&e) => {
                    let found = match self.fallback(e) {
                        Ok(file) => file.get(name)?,
                        Err(_) => None,
                    };
                    if found.is_none() {
                        super::MISSED
                            .lock()
                            .unwrap()
                            .insert((self.env.clone(), name.to_string()));
                    }
                    Ok(found)
                }
                Err(e) => Err(secret_err(e)),
            }
        }

        fn set(&self, name: &str, value: &str) -> Result<()> {
            match self.entry(name)?.set_password(value) {
                Ok(()) => Ok(()),
                Err(e) if unavailable(&e) => self.fallback(e)?.set(name, value),
                Err(e) => Err(secret_err(e)),
            }
        }

        fn delete(&self, name: &str) -> Result<bool> {
            let in_keyring = match self.entry(name)?.delete_credential() {
                Ok(()) => true,
                Err(keyring::Error::NoEntry) => false,
                Err(e) if unavailable(&e) => {
                    self.fallback(e)?;
                    false
                }
                Err(e) => return Err(secret_err(e)),
            };
            let in_file = match &self.fallback {
                Some(file) => file.delete(name)?,
                None => false,
            };
            Ok(in_keyring || in_file)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("outry-secrets-{}", fastrand::u64(..)));
        let path = dir.join("secrets.toml");
        let dev = FileStore::new(path.clone(), "shop", "dev");
        let prod = FileStore::new(path.clone(), "shop", "prod");

        assert_eq!(dev.get("token").unwrap(), None);
        dev.set("token", "a\"b").unwrap();
        prod.set("token", "p").unwrap();
        assert_eq!(dev.get("token").unwrap().as_deref(), Some("a\"b"));
        assert_eq!(prod.get("token").unwrap().as_deref(), Some("p"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        assert!(dev.delete("token").unwrap());
        assert!(!dev.delete("token").unwrap());
        assert_eq!(dev.get("token").unwrap(), None);
        assert_eq!(prod.get("token").unwrap().as_deref(), Some("p"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
