//! Значения из `> save` переживают перезапуск: они лежат в данных пользователя
//! (`~/.local/share/outry/state/…`), а не в репозитории — там часто токены.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    /// Для человека, который откроет файл: какому проекту он принадлежит.
    pub root: PathBuf,
    #[serde(default)]
    pub envs: BTreeMap<String, BTreeMap<String, String>>,
    /// Ответы вызовов с `cache: 30m` по окружениям, ключ — файл, запрос и аргументы.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cache: BTreeMap<String, BTreeMap<String, CachedCall>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedCall {
    /// Unix-время, после которого ответ устарел.
    pub expires: u64,
    pub value: serde_json::Value,
}

impl CachedCall {
    pub fn fresh(&self) -> bool {
        self.expires > unix_now()
    }
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

pub fn state_path(project_root: &Path) -> Option<PathBuf> {
    project_data_file(project_root, "state", "json")
}

/// `<data_local_dir>/outry/<kind>/<хеш корня проекта>.<ext>` — данные проекта вне репозитория.
pub fn project_data_file(project_root: &Path, kind: &str, ext: &str) -> Option<PathBuf> {
    let root = std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let hash = fnv1a(root.to_string_lossy().as_bytes());
    Some(
        dirs::data_local_dir()?
            .join("outry")
            .join(kind)
            .join(format!("{hash:016x}.{ext}")),
    )
}

impl State {
    pub fn load(path: &Path) -> Result<State> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// Стабильный между версиями Rust хеш (в отличие от `DefaultHasher`).
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("outry-state-test-{}", std::process::id()));
        let path = dir.join("s.json");
        let mut s = State::load(&path).unwrap();
        s.envs
            .entry("dev".into())
            .or_default()
            .insert("token".into(), "abc".into());
        s.save(&path).unwrap();
        assert_eq!(State::load(&path).unwrap().envs["dev"]["token"], "abc");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
