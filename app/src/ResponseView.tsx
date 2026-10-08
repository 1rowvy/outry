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
      <div className="response-head">
        <span className={"status " + (r.status < 400 ? "ok" : "bad")}>
          <b>{r.status}</b> {r.status_text}
        </span>
        <span className="metric">{r.duration_ms} ms</span>
        <span className="metric">{formatSize(r.size)}</span>
        <span className="spacer" />
        <div className="seg">
          {(["body", "headers", "tests", "request"] as Tab[]).map((t) => (
            <button key={t} className={tab === t ? "active" : ""} onClick={() => setTab(t)}>
              {t}
              {t === "tests" && checks > 0 && (
                <span className={"tab-count " + (failed ? "bad" : "ok")}>
                  {checks - failed}/{checks}
                </span>
              )}
            </button>
          ))}
        </div>
      </div>
      {Object.keys(outcome.saved).length > 0 && (
        <div className="saved">
          saved: {Object.keys(outcome.saved).map((k) => <code key={k}>{k}</code>)}
        </div>
      )}
      <div className="tab-body">
        {tab === "body" && <pre>{prettyBody(r.body)}</pre>}
        {tab === "headers" && <HeaderTable headers={r.headers} />}
        {tab === "tests" &&
          (checks === 0 ? (
            <p className="muted">No checks. Add to the end of the file: <code>&gt; assert status == 200</code></p>
          ) : (
            <ul className="tests">
              {outcome.asserts.map((a, i) => (
                <li key={i} className={a.passed ? "ok" : "bad"}>
                  {a.passed ? "✓" : "✗"} {a.source}
                  {!a.passed && <span className="muted"> — actual: {JSON.stringify(a.actual ?? null)}</span>}
                </li>
              ))}
              {outcome.save_misses.map((m) => (
                <li key={m} className="bad">✗ save {m} <span className="muted">— no value in the response</span></li>
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
