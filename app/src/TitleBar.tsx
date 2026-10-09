// Своя шапка окна вместо системной: окно без декораций (на macOS — Overlay со
// штатными кнопками-светофором поверх, см. tauri.macos.conf.json).
import { useEffect, useState, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import logo from "../app-icon.svg";

const isMac = navigator.userAgent.includes("Mac");
const win = getCurrentWindow();

export function TitleBar({ version, menu }: { version: string | null; menu?: ReactNode }) {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    if (isMac) return;
    const sync = () => win.isMaximized().then(setMaximized, () => {});
    sync();
    const unlisten = win.onResized(sync);
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  return (
    <header className={"titlebar" + (isMac ? " mac" : "")} data-tauri-drag-region>
      <div className="brand" data-tauri-drag-region>
        <img src={logo} alt="" draggable={false} />
        <span>outry</span>
        {version && <span className="version">v{version}</span>}
      </div>
      {menu}
      {!isMac && (
        <div className="window-controls">
          <button onClick={() => win.minimize()} title="Minimize" aria-label="Minimize">
            <svg viewBox="0 0 10 10"><path d="M1 5h8" /></svg>
          </button>
          <button
            onClick={() => win.toggleMaximize()}
            title={maximized ? "Restore" : "Maximize"}
            aria-label={maximized ? "Restore" : "Maximize"}
          >
            {maximized ? (
              <svg viewBox="0 0 10 10"><path d="M3 1h6v6M1 3h6v6H1z" /></svg>
            ) : (
              <svg viewBox="0 0 10 10"><path d="M1 1h8v8H1z" /></svg>
            )}
          </button>
          <button className="close" onClick={() => win.close()} title="Close" aria-label="Close">
            <svg viewBox="0 0 10 10"><path d="M1 1l8 8M9 1l-8 8" /></svg>
          </button>
        </div>
      )}
    </header>
  );
}
