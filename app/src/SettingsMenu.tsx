// Меню настроек в шапке окна: проверка обновлений и переключатели.
import { useEffect, useRef, useState } from "react";
import type { Updates } from "./updater";

interface Props {
  updates: Updates;
  autoUpdate: boolean;
  onAutoUpdate: (v: boolean) => void;
  persistHistory: boolean;
  onPersistHistory: (v: boolean) => void;
}

function updateStatus(u: Updates): string | null {
  const s = u.state;
  switch (s.kind) {
    case "checking":
      return "Checking…";
    case "latest":
      return "You have the latest version";
    case "available":
      return `Version ${s.update.version} is available`;
    case "downloading":
      return s.percent === null ? "Downloading…" : `Downloading ${s.percent}%`;
    case "ready":
      return `Version ${s.update.version} will be used after restart`;
    case "error":
      return s.message;
    default:
      return null;
  }
}

export function SettingsMenu(props: Props) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const { updates } = props;
  const status = updateStatus(updates);
  const busy = updates.state.kind === "checking" || updates.state.kind === "downloading";

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent | KeyboardEvent) => {
      if (e instanceof KeyboardEvent ? e.key === "Escape" : !root.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", close);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", close);
    };
  }, [open]);

  return (
    <div className="settings" ref={root}>
      <button className="icon" onClick={() => setOpen(!open)} title="Settings" aria-label="Settings" aria-expanded={open}>
        <svg viewBox="0 0 16 16" aria-hidden>
          <path d="M2 4h12M2 8h12M2 12h12" />
        </svg>
      </button>
      {open && (
        <div className="popover settings-menu" role="menu">
          <div className="menu-group">
            <button
              onClick={updates.state.kind === "available" ? updates.install : updates.state.kind === "ready" ? updates.restart : updates.check}
              disabled={busy}
            >
              {updates.state.kind === "available"
                ? "Update and restart"
                : updates.state.kind === "ready"
                  ? "Restart to update"
                  : "Check for updates"}
            </button>
            {status && <p className={"menu-note" + (updates.state.kind === "error" ? " bad" : "")}>{status}</p>}
          </div>
          <label className="check">
            <input type="checkbox" checked={props.autoUpdate} onChange={(e) => props.onAutoUpdate(e.target.checked)} />
            <span>
              Install updates automatically
              <small>Downloaded in the background, applied on the next start</small>
            </span>
          </label>
          <label className="check">
            <input type="checkbox" checked={props.persistHistory} onChange={(e) => props.onPersistHistory(e.target.checked)} />
            <span>
              Keep response history on disk
              <small>Outside the repo, in your user data folder. Responses may contain tokens</small>
            </span>
          </label>
        </div>
      )}
    </div>
  );
}
