// Автообновление: спрашиваем latest.json из GitHub Releases, подпись проверяет плагин
// по pubkey из tauri.conf.json. При старте — тихо; из меню — с ответом «последняя версия».
import { useCallback, useEffect, useRef, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export type UpdateState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "latest" }
  | { kind: "available"; update: Update }
  | { kind: "downloading"; update: Update; percent: number | null }
  /** Установлено в фоне, применится после перезапуска. */
  | { kind: "ready"; update: Update }
  | { kind: "error"; message: string; update?: Update };

export interface Updates {
  state: UpdateState;
  check: () => Promise<void>;
  /** Скачать, установить и перезапуститься. */
  install: () => Promise<void>;
  restart: () => Promise<void>;
}

export function useUpdates(auto: boolean): Updates {
  const [state, setState] = useState<UpdateState>({ kind: "idle" });
  const autoRef = useRef(auto);
  autoRef.current = auto;
  const busy = useRef(false);

  const download = useCallback(async (update: Update, restart: boolean) => {
    let total = 0;
    let done = 0;
    setState({ kind: "downloading", update, percent: null });
    try {
      await update.downloadAndInstall((ev) => {
        if (ev.event === "Started") {
          total = ev.data.contentLength ?? 0;
        } else if (ev.event === "Progress") {
          done += ev.data.chunkLength;
          setState({ kind: "downloading", update, percent: total ? Math.round((done / total) * 100) : null });
        }
      });
    } catch (e) {
      setState({ kind: "error", message: String(e), update });
      return;
    }
    // На Windows установщик сам закрывает приложение; на macOS/Linux перезапускаем.
    if (restart) await relaunch();
    else setState({ kind: "ready", update });
  }, []);

  const run = useCallback(
    async (manual: boolean) => {
      if (busy.current || (import.meta.env.DEV && !manual)) return;
      busy.current = true;
      if (manual) setState({ kind: "checking" });
      try {
        const update = await check();
        if (!update) setState({ kind: manual ? "latest" : "idle" });
        else if (autoRef.current) await download(update, false);
        else setState({ kind: "available", update });
      } catch (e) {
        // Нет сети или ещё нет ни одного релиза — при старте это не повод мешать работе.
        if (manual) setState({ kind: "error", message: String(e) });
        else console.warn("update check failed", e);
      } finally {
        busy.current = false;
      }
    },
    [download],
  );

  useEffect(() => {
    run(false);
  }, [run]);

  return {
    state,
    check: () => run(true),
    install: async () => {
      if (state.kind === "available" || (state.kind === "error" && state.update)) await download(state.update!, true);
    },
    restart: () => relaunch(),
  };
}
