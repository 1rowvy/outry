// Запросы, вызванные из *.outry (`Login()`), с вложенностью: что ушло в сеть, что взято из кеша.
import type { CallTrace } from "./types";

function args(a: Record<string, unknown>): string {
  return Object.entries(a)
    .map(([k, v]) => `${k}: ${JSON.stringify(v)}`)
    .join(", ");
}

export function Trace({ calls }: { calls: CallTrace[] }) {
  if (calls.length === 0) return <p className="muted">No requests were called.</p>;
  return (
    <ul className="trace">
      {calls.map((c, i) => (
        <li key={i} style={{ paddingLeft: `${c.depth * 18}px` }}>
          <span className="t-name">
            {c.name}({args(c.args)})
          </span>
          {c.cached ? (
            <span className="badge t-cached" title="Taken from the run's cache, not sent">
              cached
            </span>
          ) : c.status !== null ? (
            <span className={"t-status " + (c.status < 400 ? "ok" : "bad")}>{c.status}</span>
          ) : null}
          {c.duration_ms !== null && <span className="muted">{c.duration_ms} ms</span>}
        </li>
      ))}
    </ul>
  );
}
