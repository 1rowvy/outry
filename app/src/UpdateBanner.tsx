import { useEffect, useState } from "react";
import type { Update } from "@tauri-apps/plugin-updater";
import { findUpdate, installUpdate } from "./updater";

export function UpdateBanner() {
  const [update, setUpdate] = useState<Update | null>(null);
  const [progress, setProgress] = useState<number | null | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    findUpdate().then(setUpdate);
  }, []);

  if (!update) return null;

  const install = async () => {
    setError(null);
    try {
      await installUpdate(update, setProgress);
    } catch (e) {
      setProgress(undefined);
      setError(String(e));
    }
  };

  return (
    <div className="update-banner">
      <span>Доступна версия {update.version}</span>
      {progress === undefined ? (
        <button onClick={install}>Обновить и перезапустить</button>
      ) : (
        <span className="muted">{progress === null ? "Загрузка…" : `Загрузка ${progress}%`}</span>
      )}
      {error && <span className="bad">{error}</span>}
    </div>
  );
}
