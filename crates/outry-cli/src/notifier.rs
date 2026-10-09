//! Напоминание о новой версии, как у npm (update-notifier):
//! раз в сутки отвязанная копия `outry` в фоне спрашивает GitHub и пишет ответ в кэш,
//! а обычные команды только читают кэш и в конце печатают плашку в stderr.
//! Команда пользователя никогда не ждёт сети.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::update::{self, CURRENT};

/// Скрытая подкоманда, которой фоновая копия обновляет кэш.
pub const REFRESH_COMMAND: &str = "__refresh-update-cache";

const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    /// Unix-время последней попытки проверки (успешной или нет).
    checked_at: u64,
    /// Тег последнего релиза, например `v0.2.0`.
    latest: Option<String>,
}

fn cache_path() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("outry").join("update-check.json"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn load() -> Cache {
    cache_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(cache: &Cache) {
    let Some(path) = cache_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec(cache) {
        let _ = std::fs::write(path, json);
    }
}

/// Запомнить свежий тег (после `outry update` или фоновой проверки).
pub fn remember(tag: &str) {
    save(&Cache {
        checked_at: now(),
        latest: Some(tag.to_string()),
    });
}

fn enabled() -> bool {
    let off = |name| std::env::var_os(name).is_some_and(|v| !v.is_empty() && v != "0");
    if off("OUTRY_NO_UPDATE_NOTIFIER") || off("NO_UPDATE_NOTIFIER") || off("CI") {
        return false;
    }
    // Пайпы, `--json`, CI-логи — без плашек.
    if !std::io::stderr().is_terminal() {
        return false;
    }
    // Установлен пакетным менеджером — обновления его забота.
    match std::env::current_exe().and_then(|p| p.canonicalize()) {
        Ok(exe) => update::package_manager(&exe).is_none(),
        Err(_) => false,
    }
}

/// Вызывается в конце обычных команд.
pub fn after_command() {
    if !enabled() {
        return;
    }
    let mut cache = load();
    if now().saturating_sub(cache.checked_at) >= INTERVAL.as_secs() {
        // Отмечаем попытку сразу: если GitHub недоступен, не будем дёргать его на каждой команде.
        cache.checked_at = now();
        save(&cache);
        spawn_refresh();
    }
    if let Some(latest) = cache.latest.as_deref().map(|t| t.trim_start_matches('v')) {
        if update::is_newer(latest, CURRENT) {
            eprintln!("{}", banner(latest));
        }
    }
}

fn spawn_refresh() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = Command::new(exe);
    cmd.arg(REFRESH_COMMAND)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Своя группа процессов: иначе при закрытии терминала/ssh-сессии сразу после команды
    // фоновая проверка получает SIGHUP вместе с нами и не успевает записать кэш.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    // Не ждём: процесс переживёт текущую команду и сам запишет кэш.
    let _ = cmd.spawn();
}

/// Тело скрытой подкоманды.
pub fn refresh() {
    let Ok(rt) = tokio::runtime::Runtime::new() else {
        return;
    };
    rt.block_on(async {
        let Ok(client) = update::client() else { return };
        if let Ok(Ok(release)) =
            tokio::time::timeout(Duration::from_secs(15), update::fetch_latest(&client)).await
        {
            remember(&release.tag_name);
        }
    });
}

fn banner(latest: &str) -> String {
    let lines = [
        format!("Доступно обновление outry {CURRENT} → {latest}"),
        "Обновить: outry update".to_string(),
    ];
    let width = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) + 4;
    let color = std::env::var_os("NO_COLOR").is_none();
    let (y, g, r) = if color {
        ("\x1b[33m", "\x1b[32m", "\x1b[0m")
    } else {
        ("", "", "")
    };

    let mut out = format!("\n{y}╭{}╮{r}\n", "─".repeat(width));
    for line in &lines {
        let pad = width - 2 - line.chars().count();
        let text = line.replace("outry update", &format!("{g}outry update{r}"));
        out.push_str(&format!("{y}│{r}  {text}{}{y}│{r}\n", " ".repeat(pad)));
    }
    out.push_str(&format!("{y}╰{}╯{r}", "─".repeat(width)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_is_aligned() {
        // SAFETY: тест однопоточный по этой переменной; NO_COLOR убирает escape-коды из ширины.
        unsafe { std::env::set_var("NO_COLOR", "1") };
        let b = banner("0.2.0");
        let widths: Vec<usize> = b.trim().lines().map(|l| l.chars().count()).collect();
        assert!(widths.iter().all(|w| *w == widths[0]), "{b}\n{widths:?}");
        assert!(b.contains("0.2.0") && b.contains("outry update"));
    }
}
