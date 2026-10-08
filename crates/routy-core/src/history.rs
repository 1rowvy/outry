//! История ответов. В памяти — всегда; на диск — по желанию пользователя и вне репозитория
//! (`~/.local/share/routy/history/…`): в запросах и ответах бывают токены.
//! На диске — JSON Lines: запись в конец файла, без перезаписи всей истории на каждый запрос.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::runner::RunOutcome;
use crate::state::project_data_file;

/// Сколько последних ответов хранится на проект.
pub const LIMIT: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: u64,
    /// Файл запроса относительно корня проекта.
    pub file: String,
    pub env: String,
    /// Unix-время отправки в миллисекундах.
    pub at: u64,
    pub outcome: RunOutcome,
}

#[derive(Default)]
pub struct History {
    entries: VecDeque<Entry>,
    next_id: u64,
    disk: Option<Disk>,
}

struct Disk {
    path: PathBuf,
    /// Строк в файле; когда их вдвое больше `LIMIT`, файл переписывается.
    lines: usize,
}

pub fn history_path(project_root: &Path) -> Option<PathBuf> {
    project_data_file(project_root, "history", "jsonl")
}

impl History {
    pub fn is_persistent(&self) -> bool {
        self.disk.is_some()
    }

    /// Включает хранение на диске: подгружает сохранённую историю перед текущей.
    pub fn persist_to(&mut self, path: PathBuf) -> Result<()> {
        if self.disk.is_some() {
            return Ok(());
        }
        let loaded = match std::fs::read_to_string(&path) {
            Ok(src) => src
                .lines()
                .filter_map(|l| serde_json::from_str::<Entry>(l).ok())
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        let current = std::mem::take(&mut self.entries);
        self.next_id = 0;
        for mut e in loaded.into_iter().chain(current) {
            e.id = self.next_id;
            self.next_id += 1;
            self.entries.push_back(e);
        }
        self.trim();
        self.disk = Some(Disk { path, lines: 0 });
        self.rewrite()
    }

    /// Выключает хранение на диске и удаляет файл; в памяти история остаётся.
    pub fn stop_persisting(&mut self) -> Result<()> {
        match self.disk.take() {
            Some(d) => remove(&d.path),
            None => Ok(()),
        }
    }

    pub fn push(&mut self, file: &str, env: &str, outcome: RunOutcome) -> Result<&Entry> {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default();
        self.entries.push_back(Entry {
            id: self.next_id,
            file: file.to_string(),
            env: env.to_string(),
            at,
            outcome,
        });
        self.next_id += 1;
        self.trim();
        let overgrown = self.disk.as_ref().is_some_and(|d| d.lines >= 2 * LIMIT);
        if overgrown {
            self.rewrite()?;
        } else if let Some(d) = &mut self.disk {
            let entry = self.entries.back().expect("just pushed");
            let mut f = open(&d.path, true)?;
            writeln!(f, "{}", serde_json::to_string(entry)?)?;
            d.lines += 1;
        }
        Ok(self.entries.back().expect("just pushed"))
    }

    /// От старых к новым.
    pub fn entries(&self) -> &VecDeque<Entry> {
        &self.entries
    }

    pub fn get(&self, id: u64) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn clear(&mut self) -> Result<()> {
        self.entries.clear();
        if let Some(d) = &mut self.disk {
            d.lines = 0;
            remove(&d.path)?;
        }
        Ok(())
    }

    fn trim(&mut self) {
        while self.entries.len() > LIMIT {
            self.entries.pop_front();
        }
    }

    fn rewrite(&mut self) -> Result<()> {
        let Some(d) = &mut self.disk else {
            return Ok(());
        };
        let mut out = String::new();
        for e in &self.entries {
            out.push_str(&serde_json::to_string(e)?);
            out.push('\n');
        }
        let tmp = d.path.with_extension("jsonl.tmp");
        open(&tmp, false)?.write_all(out.as_bytes())?;
        std::fs::rename(&tmp, &d.path)?;
        d.lines = self.entries.len();
        Ok(())
    }
}

/// Файл только для владельца, как и `state`.
fn open(path: &Path, append: bool) -> Result<std::fs::File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut o = std::fs::OpenOptions::new();
    o.create(true).write(true);
    if append {
        o.append(true);
    } else {
        o.truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    Ok(o.open(path)?)
}

fn remove(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{ResolvedRequest, Response};

    fn outcome(status: u16) -> RunOutcome {
        RunOutcome {
            request: ResolvedRequest {
                method: "GET".into(),
                url: "http://x".into(),
                headers: vec![],
                body: None,
            },
            response: Response {
                status,
                status_text: String::new(),
                headers: vec![],
                body: "{}".into(),
                json: None,
                raw: vec![],
                duration_ms: 1,
                size: 2,
            },
            saved: Default::default(),
            asserts: vec![],
            save_misses: vec![],
            calls: vec![],
        }
    }

    #[test]
    fn memory_limit() {
        let mut h = History::default();
        for i in 0..LIMIT + 5 {
            h.push("a.http", "dev", outcome(i as u16)).unwrap();
        }
        assert_eq!(h.entries().len(), LIMIT);
        assert_eq!(h.entries()[0].outcome.response.status, 5);
        assert!(h.get(4).is_none() && h.get(5).is_some());
    }

    #[test]
    fn persists_and_reloads() {
        let dir = std::env::temp_dir().join(format!("routy-history-test-{}", std::process::id()));
        let path = dir.join("h.jsonl");
        let _ = std::fs::remove_dir_all(&dir);

        let mut h = History::default();
        h.push("a.http", "dev", outcome(200)).unwrap();
        h.persist_to(path.clone()).unwrap();
        for _ in 0..2 * LIMIT + 3 {
            h.push("b.http", "dev", outcome(201)).unwrap();
        }
        let mut again = History::default();
        again.push("c.http", "dev", outcome(500)).unwrap();
        again.persist_to(path.clone()).unwrap();
        assert_eq!(again.entries().len(), LIMIT);
        assert_eq!(again.entries().back().unwrap().file, "c.http");
        // Файл переписан при переполнении: в нём не больше 2 × LIMIT строк.
        let lines = std::fs::read_to_string(&path).unwrap().lines().count();
        assert_eq!(lines, LIMIT);

        again.stop_persisting().unwrap();
        assert!(!path.exists());
        assert_eq!(again.entries().len(), LIMIT);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
