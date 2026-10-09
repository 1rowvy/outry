import { useEffect, useMemo, useRef, useState } from "react";
import type { AssertOutcome, Entry, Header } from "./types";
import { BodyViewer, type BodyLanguage, type BodyViewerHandle } from "./BodyViewer";
import { Trace } from "./Trace";

type Tab = "body" | "preview" | "headers" | "tests" | "trace" | "request";

function contentType(headers: Header[]): string {
  return headers.find((h) => h.name.toLowerCase() === "content-type")?.value.split(";")[0].trim().toLowerCase() ?? "";
}

function prettyJson(body: string): string | null {
  try {
    return JSON.stringify(JSON.parse(body), null, 2);
  } catch {
    return null;
  }
}

export function formatSize(bytes: number): string {
  return bytes < 1024 ? `${bytes} B` : bytes < 1024 * 1024 ? `${(bytes / 1024).toFixed(1)} KB` : `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

const EXTENSIONS: Record<string, string> = {
  "application/json": "json",
  "text/html": "html",
  "application/xml": "xml",
  "text/xml": "xml",
  "text/plain": "txt",
  "text/csv": "csv",
  "application/pdf": "pdf",
  "image/png": "png",
  "image/jpeg": "jpg",
  "image/gif": "gif",
  "image/webp": "webp",
  "image/svg+xml": "svg",
};

/** Имя файла для «Save»: последний сегмент URL или `response`, расширение — по Content-Type. */
export function suggestedName(url: string, mime: string): string {
  let base = "response";
  try {
    base = decodeURIComponent(new URL(url).pathname.split("/").filter(Boolean).pop() ?? "") || base;
  } catch {
    // URL уже проверен ядром; на всякий случай — имя по умолчанию.
  }
  const ext = EXTENSIONS[mime] ?? (mime.endsWith("+json") ? "json" : "");
  return ext && !base.includes(".") ? `${base}.${ext}` : base;
}

/** Что ответу нужно от оболочки: приложения или webview расширения VS Code. */
export interface ResponseHost {
  /** Картинка из ответа как `data:` URL; `null` — байтов нет. */
  image: (entry: Entry, mime: string) => Promise<string | null>;
  /** «Save body to file…»: спросить путь и записать тело. */
  saveBody: (entry: Entry, suggestedName: string) => Promise<void>;
  /** Почему картинки нет. */
  noImage: string;
}

export function ResponseView({ entry, host }: { entry: Entry; host: ResponseHost }) {
  const { outcome } = entry;
  const r = outcome.response;
  const mime = contentType(r.headers);
  const isImage = mime.startsWith("image/");
  const isHtml = mime === "text/html";
  const [tab, setTab] = useState<Tab>(isImage ? "preview" : "body");
  const [image, setImage] = useState<string | null>(null);
  const viewer = useRef<BodyViewerHandle>(null);

  const failed = outcome.asserts.filter((a) => !a.passed).length + outcome.save_misses.length;
  const checks = outcome.asserts.length + outcome.save_misses.length;
  const calls = outcome.calls ?? [];
  const tabs: Tab[] = [
    "body",
    ...(isImage || isHtml ? (["preview"] as Tab[]) : []),
    "headers",
    "tests",
    ...(calls.length > 0 ? (["trace"] as Tab[]) : []),
    "request",
  ];

  const [text, language] = useMemo((): [string, BodyLanguage] => {
    const pretty = prettyJson(r.body);
    if (pretty !== null) return [pretty, "json"];
    if (isHtml) return [r.body, "html"];
    if (mime.includes("xml")) return [r.body, "xml"];
    return [r.body, "text"];
  }, [r.body, mime, isHtml]);

  // Родитель задаёт key={entry.id}, так что вкладка сбрасывается на каждый ответ.
  useEffect(() => {
    if (isImage) host.image(entry, mime).then(setImage, () => setImage(null));
    // host не в зависимостях: для того же ответа картинка та же.
  }, [entry.id, isImage]);

  const saveBody = () => host.saveBody(entry, suggestedName(outcome.request.url, mime));

  return (
    <div className="response">
      <div className="response-head">
        <span className={"status " + (r.status < 400 ? "ok" : "bad")}>
          <b>{r.status}</b> {r.status_text}
        </span>
        <span className="metric">{r.duration_ms} ms</span>
        <span className="metric">{formatSize(r.size)}</span>
        <span className="spacer" />
        {tab === "body" && (
          <button className="icon" onClick={() => viewer.current?.find()} title="Find in response (Ctrl+F)" aria-label="Find">
            <svg viewBox="0 0 16 16" aria-hidden>
              <circle cx="7" cy="7" r="4.5" />
              <path d="M10.5 10.5 14 14" />
            </svg>
          </button>
        )}
        <button className="icon" onClick={saveBody} title="Save body to file…" aria-label="Save body to file">
          <svg viewBox="0 0 16 16" aria-hidden>
            <path d="M8 2v8M4.5 6.5 8 10l3.5-3.5M2.5 13.5h11" />
          </svg>
        </button>
      </div>
      <div className="response-tabs">
        <div className="seg">
          {tabs.map((t) => (
            <button key={t} className={tab === t ? "active" : ""} onClick={() => setTab(t)}>
              {t}
              {t === "tests" && checks > 0 && (
                <span className={"tab-count " + (failed ? "bad" : "ok")}>
                  {checks - failed}/{checks}
                </span>
              )}
              {t === "trace" && <span className="tab-count">{calls.length}</span>}
            </button>
          ))}
        </div>
        {Object.keys(outcome.saved).length > 0 && (
          <span className="saved">
            saved {Object.keys(outcome.saved).map((k) => <code key={k}>{k}</code>)}
          </span>
        )}
      </div>
      {tab === "body" && <BodyViewer ref={viewer} text={text} language={language} />}
      {tab === "preview" && (
        <div className="tab-body preview">
          {isImage ? (
            image ? (
              <img src={image} alt="Response" />
            ) : (
              <p className="muted">{host.noImage}</p>
            )
          ) : (
            // Без allow-scripts: страница из ответа не выполняет JS и не видит приложение.
            <iframe title="HTML preview" sandbox="" srcDoc={r.body} />
          )}
        </div>
      )}
      {tab === "headers" && (
        <div className="tab-body">
          <HeaderTable headers={r.headers} />
        </div>
      )}
      {tab === "tests" && (
        <div className="tab-body">
          {checks === 0 ? (
            <p className="muted">
              No checks. Add{" "}
              {entry.file.endsWith(".outry") ? (
                <code>expect {"{ status == 200 }"}</code>
              ) : (
                <>
                  to the end of the file: <code>&gt; assert status == 200</code>
                </>
              )}
            </p>
          ) : (
            <Checks asserts={outcome.asserts} misses={outcome.save_misses} />
          )}
        </div>
      )}
      {tab === "trace" && (
        <div className="tab-body">
          <Trace calls={calls} />
        </div>
      )}
      {tab === "request" && (
        <div className="tab-body">
          <pre>
            {`${outcome.request.method} ${outcome.request.url}\n`}
            {outcome.request.headers.map((h) => `${h.name}: ${h.value}\n`).join("")}
            {outcome.request.body ? `\n${outcome.request.body}` : ""}
          </pre>
        </div>
      )}
    </div>
  );
}

/** Проверки: `✓ status == 201`, у упавших — почему (`body.total is 0`) или фактическое значение. */
export function Checks({ asserts, misses }: { asserts: AssertOutcome[]; misses: string[] }) {
  return (
    <ul className="tests">
      {asserts.map((a, i) => (
        <li key={i} className={a.passed ? "ok" : "bad"}>
          {a.passed ? "✓" : "✗"} {a.source}
          {!a.passed && <span className="muted"> — {a.detail ?? `actual: ${JSON.stringify(a.actual ?? null)}`}</span>}
        </li>
      ))}
      {misses.map((m) => (
        <li key={m} className="bad">
          ✗ save {m} <span className="muted">— no value in the response</span>
        </li>
      ))}
    </ul>
  );
}

function HeaderTable({ headers }: { headers: Header[] }) {
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
