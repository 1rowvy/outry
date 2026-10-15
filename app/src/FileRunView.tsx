// Прогон всего файла *.outry: запросы и сценарии по порядку с итогом, под списком — выбранный ответ.
// Общий для приложения и VS Code (webview/), поэтому без Tauri.
import { useState } from "react";
import { FlowView } from "./FlowView";
import { ResponseView, type ResponseHost } from "./ResponseView";
import type { Entry, FlowOutcome } from "./types";

export interface FileRunItem {
  name: string;
  /** метод запроса или `FLOW` */
  method: string;
  /** строка (с 1) — для перехода к элементу */
  line: number;
  running?: boolean;
  /** прогон остановили до этого элемента */
  skipped?: boolean;
  entry?: Entry;
  flow?: FlowOutcome;
  error?: string;
}

const finished = (i: FileRunItem) => !!(i.entry || i.flow || i.error);

export function itemPassed(i: FileRunItem): boolean {
  if (i.error) return false;
  if (i.flow) return i.flow.error === null && i.flow.checks.every((c) => c.passed);
  if (i.entry) return i.entry.outcome.asserts.every((a) => a.passed) && i.entry.outcome.save_misses.length === 0;
  return true;
}

export function FileRunView({
  title,
  items,
  host,
  onReveal,
}: {
  title: string;
  items: FileRunItem[];
  host: ResponseHost;
  /** открыть элемент в редакторе */
  onReveal?: (line: number) => void;
}) {
  const [picked, setPicked] = useState<number | null>(null);
  const done = items.filter(finished).length;
  const failed = items.filter((i) => finished(i) && !itemPassed(i)).length;
  const busy = items.some((i) => i.running);
  // Пока ничего не выбрано — первый упавший, иначе последний готовый.
  let auto = items.findIndex((i) => finished(i) && !itemPassed(i));
  if (auto < 0) auto = items.map(finished).lastIndexOf(true);
  const sel = picked !== null && finished(items[picked]) ? picked : auto;
  const it = sel >= 0 ? items[sel] : null;

  return (
    <div className="response file-run">
      <div className="response-head">
        <span className={"status " + (failed ? "bad" : busy ? "" : "ok")}>
          <b>{busy ? <span className="spinner" /> : failed ? "✗" : "✓"}</b> {title}
        </span>
        <span className="metric">
          {done}/{items.length} run
        </span>
        {failed > 0 && <span className="metric r-failed">{failed} failed</span>}
      </div>
      <ul className="run-list">
        {items.map((i, n) => {
          const r = i.entry?.outcome.response;
          const ok = itemPassed(i);
          return (
            <li key={n}>
              <button
                className={n === sel ? "active" : ""}
                onClick={() => setPicked(n)}
                onDoubleClick={() => onReveal?.(i.line)}
                disabled={!finished(i)}
                title={onReveal ? "Double-click to go to the request" : undefined}
              >
                <span className={"r-icon " + (finished(i) ? (ok ? "ok" : "bad") : "muted")}>
                  {i.running ? <span className="spinner" /> : !finished(i) ? (i.skipped ? "–" : "·") : ok ? "✓" : "✗"}
                </span>
                <span className={"method m-" + i.method.toLowerCase()}>{i.method}</span>
                <span className="ellipsis r-name">{i.name}</span>
                {r && <span className={"h-status " + (r.status < 400 ? "ok" : "bad")}>{r.status}</span>}
                {i.flow && (
                  <span className="h-meta">
                    {i.flow.checks.filter((c) => c.passed).length}/{i.flow.checks.length} checks
                  </span>
                )}
                {r && <span className="h-meta">{r.duration_ms} ms</span>}
                {i.skipped && <span className="h-meta">skipped</span>}
              </button>
            </li>
          );
        })}
      </ul>
      <div className="run-detail">
        {it?.error ? (
          <div className="send-error">
            ✗ {it.name}: {it.error}
          </div>
        ) : it?.flow ? (
          <FlowView name={it.name} flow={it.flow} />
        ) : it?.entry ? (
          <ResponseView key={it.entry.id} entry={it.entry} host={host} />
        ) : (
          <p className="hint">Sending…</p>
        )}
      </div>
    </div>
  );
}
