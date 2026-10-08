//! Поиск файлов запросов в проекте.

use std::path::{Path, PathBuf};

pub const EXTENSION: &str = "http";

const SKIP_DIRS: &[&str] = &["node_modules", "target", "vendor", "dist"];

/// Все `*.http` под `root`, отсортированные, пути относительные к `root`.
pub fn request_files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let ty = entry.file_type()?;
        if ty.is_dir() {
            if !SKIP_DIRS.contains(&name.as_ref()) {
                walk(root, &path, out)?;
            }
        } else if path.extension().is_some_and(|e| e == EXTENSION) {
            out.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
        }
    }
    Ok(())
}
