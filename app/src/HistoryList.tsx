import { useState } from "react";
import type { Entry } from "./api";

function time(at: number): string {
  const d = new Date(at);
  const today = new Date().toDateString() === d.toDateString();
  return today ? d.toLocaleTimeString() : d.toLocaleString();
}

interface Props {
  entries: Entry[];
  /** Текущий файл — по умолчанию показываем только его историю. */
  file: string | null;
  openedId: number | null;
  persistent: boolean;
  onOpen: (e: Entry) => void;
  onClear: () => void;
}

export function HistoryList({ entries, file, openedId, persistent, onOpen, onClear }: Props) {
  const [all, setAll] = useState(file === null);
  const shown = all || file === null ? entries : entries.filter((e) => e.file === file);

  return (
    <div className="history">
      <div className="history-bar">
        {file !== null && (
          <div className="seg">
            <button className={all ? "" : "active"} onClick={() => setAll(false)}>This request</button>
            <button className={all ? "active" : ""} onClick={() => setAll(true)}>All</button>
          </div>
        )}
        <span className="spacer" />
        <button className="ghost" onClick={onClear} disabled={entries.length === 0}>Clear</button>
      </div>
      {shown.length === 0 ? (
        <p className="hint">
          No responses yet{persistent ? "" : ". History is kept in memory until the app closes — enable “Keep response history on disk” in the menu to keep it"}.
        </p>
      ) : (
        <ul className="history-list">
          {shown.map((e) => {
            const r = e.outcome.response;
            const passed = e.outcome.asserts.every((a) => a.passed) && e.outcome.save_misses.length === 0;
            return (
              <li key={e.id}>
                <button className={e.id === openedId ? "active" : ""} onClick={() => onOpen(e)} title={e.outcome.request.url}>
                  <span className={"h-status " + (r.status < 400 ? "ok" : "bad")}>{r.status}</span>
                  <span className={"method m-" + e.outcome.request.method.toLowerCase()}>{e.outcome.request.method}</span>
                  <span className="ellipsis h-file">{all || file === null ? e.file.replace(/\.(http|routy)$/, "") : e.outcome.request.url}</span>
                  {!passed && <span className="bad" title="Some checks failed">✗</span>}
                  <span className="h-meta">
                    {e.env} · {r.duration_ms} ms · {time(e.at)}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
