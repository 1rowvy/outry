// Автообновление: при старте спрашиваем latest.json из GitHub Releases,
// подпись проверяет плагин по pubkey из tauri.conf.json.
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export async function findUpdate(): Promise<Update | null> {
  if (import.meta.env.DEV) return null;
  try {
    return await check();
  } catch (e) {
    // Нет сети или ещё нет ни одного релиза — не повод мешать работе.
    console.warn("update check failed", e);
    return null;
  }
}

export async function installUpdate(update: Update, onProgress: (percent: number | null) => void) {
  let total = 0;
  let done = 0;
  await update.downloadAndInstall((ev) => {
    if (ev.event === "Started") {
      total = ev.data.contentLength ?? 0;
      onProgress(total ? 0 : null);
    } else if (ev.event === "Progress") {
      done += ev.data.chunkLength;
      onProgress(total ? Math.round((done / total) * 100) : null);
    }
  });
  // На Windows установщик сам закрывает приложение; на macOS/Linux перезапускаем.
  await relaunch();
}
