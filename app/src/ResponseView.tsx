import { useState } from "react";
import type { RunOutcome } from "./api";

type Tab = "body" | "headers" | "tests" | "request";

function prettyBody(body: string): string {
  try {
    return JSON.stringify(JSON.parse(body), null, 2);
  } catch {
    return body;
  }
}

function formatSize(bytes: number): string {
  return bytes < 1024 ? `${bytes} B` : `${(bytes / 1024).toFixed(1)} KB`;
}

export function ResponseView({ outcome }: { outcome: RunOutcome }) {
  const [tab, setTab] = useState<Tab>("body");
  const r = outcome.response;
  const failed = outcome.asserts.filter((a) => !a.passed).length + outcome.save_misses.length;
  const checks = outcome.asserts.length + outcome.save_misses.length;

  return (
    <div className="response">
      <div className="response-meta">
        <span className={"status " + (r.status < 400 ? "ok" : "bad")}>
          {r.status} {r.status_text}
        </span>
        <span className="meta-pill">{r.duration_ms} ms</span>
        <span className="meta-pill">{formatSize(r.size)}</span>
        {Object.keys(outcome.saved).length > 0 && (
          <span className="meta-pill">saved: {Object.keys(outcome.saved).join(", ")}</span>
        )}
      </div>
      <div className="tabs">
        {(["body", "headers", "tests", "request"] as Tab[]).map((t) => (
          <button key={t} className={tab === t ? "active" : ""} onClick={() => setTab(t)}>
            {t === "tests" && checks > 0 ? `tests ${checks - failed}/${checks}` : t}
          </button>
        ))}
      </div>
      <div className="tab-body">
        {tab === "body" && <pre>{prettyBody(r.body)}</pre>}
        {tab === "headers" && <HeaderTable headers={r.headers} />}
        {tab === "tests" &&
          (checks === 0 ? (
            <p className="muted">Нет проверок. Добавьте в конец файла: <code>&gt; assert status == 200</code></p>
          ) : (
            <ul className="tests">
              {outcome.asserts.map((a, i) => (
                <li key={i} className={a.passed ? "ok" : "bad"}>
                  {a.passed ? "✓" : "✗"} {a.source}
                  {!a.passed && <span className="muted"> — actual: {JSON.stringify(a.actual ?? null)}</span>}
                </li>
              ))}
              {outcome.save_misses.map((m) => (
                <li key={m} className="bad">✗ save {m} <span className="muted">— нет значения в ответе</span></li>
              ))}
            </ul>
          ))}
        {tab === "request" && (
          <pre>
            {`${outcome.request.method} ${outcome.request.url}\n`}
            {outcome.request.headers.map((h) => `${h.name}: ${h.value}\n`).join("")}
            {outcome.request.body ? `\n${outcome.request.body}` : ""}
          </pre>
        )}
      </div>
    </div>
  );
}

function HeaderTable({ headers }: { headers: { name: string; value: string }[] }) {
  return (
    <table className="headers">
      <tbody>
        {headers.map((h, i) => (
          <tr key={i}>
            <td>{h.name}</td>
            <td>{h.value}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
