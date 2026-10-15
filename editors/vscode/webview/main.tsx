// Ответ в VS Code: те же компоненты, что в приложении; данные — сообщениями от расширения
// (src/panel.ts), сохранение тела — через него же.
import { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { FileRunView, type FileRunItem } from "@app/FileRunView";
import { FlowView } from "@app/FlowView";
import { ResponseView, type ResponseHost } from "@app/ResponseView";
import type { Entry, FlowOutcome, RunOutcome } from "@app/types";
import "@app/styles.css";
import "./vscode.css";

interface RunResult {
  name: string;
  env: string;
  outcome: { kind: "request" | "flow" };
  raw?: string;
}

interface FileItem {
  name: string;
  method: string;
  line: number;
  running?: boolean;
  skipped?: boolean;
  result?: RunResult;
  error?: string;
}

type Message =
  | { type: "loading"; title: string }
  | { type: "error"; title: string; message: string }
  | { type: "result"; seq: number; file: string; result: RunResult }
  | { type: "file"; seq: number; file: string; title: string; items: FileItem[] };

/** Как FILE_IDS в src/panel.ts: номер ответа в прогоне файла — `seq * FILE_IDS + индекс`. */
const FILE_IDS = 10000;

const vscode = acquireVsCodeApi();

/** Тела не в UTF-8 (base64) по номеру ответа — для картинок. */
const raws = new Map<number, string>();

const host: ResponseHost = {
  image: async (entry, mime) => {
    const raw = raws.get(entry.id);
    if (raw) return `data:${mime};base64,${raw}`;
    if (mime === "image/svg+xml") return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(entry.outcome.response.body)}`;
    return null;
  },
  saveBody: async (entry, name) => vscode.postMessage({ type: "save", name, id: entry.id }),
  noImage: "The response has no image data.",
};

function App() {
  const [shown, setShown] = useState<Exclude<Message, { type: "loading" }> | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  useEffect(() => {
    const on = (e: MessageEvent<Message>) => {
      const m = e.data;
      if (m.type === "loading") {
        setBusy(m.title);
        return;
      }
      if (m.type === "result" && m.result.raw) raws.set(m.seq, m.result.raw);
      if (m.type === "file") {
        m.items.forEach((i, n) => i.result?.raw && raws.set(m.seq * FILE_IDS + n, i.result.raw));
      }
      setBusy(null);
      setShown(m);
    };
    window.addEventListener("message", on);
    vscode.postMessage({ type: "ready" });
    return () => window.removeEventListener("message", on);
  }, []);

  return (
    <>
      {busy && (
        <div className="busy">
          <span className="spinner" /> {busy}…
        </div>
      )}
      {shown?.type === "error" ? (
        <div className="send-error">
          ✗ {shown.title}: {shown.message}
        </div>
      ) : shown?.type === "file" ? (
        <FileRun key={shown.seq} run={shown} />
      ) : shown?.type === "result" ? (
        <Result key={shown.seq} seq={shown.seq} file={shown.file} result={shown.result} />
      ) : (
        !busy && <p className="hint">Send a request with ▶ Send above it or Ctrl+Enter — the response appears here.</p>
      )}
    </>
  );
}

function Result({ seq, file, result }: { seq: number; file: string; result: RunResult }) {
  const box = useRef<HTMLDivElement>(null);
  // Расширению — что отрисовано (строка статуса): по нему проверяет интеграционный тест.
  useEffect(() => {
    vscode.postMessage({ type: "shown", seq, text: box.current?.querySelector(".response-head")?.textContent ?? "" });
  }, [seq]);
  const entry: Entry = { id: seq, file, env: result.env, at: Date.now(), outcome: result.outcome as unknown as RunOutcome };
  return (
    <div className="result" ref={box}>
      {result.outcome.kind === "flow" ? (
        <FlowView name={result.name} flow={result.outcome as unknown as FlowOutcome} />
      ) : (
        <ResponseView entry={entry} host={host} />
      )}
    </div>
  );
}

function FileRun({ run }: { run: Extract<Message, { type: "file" }> }) {
  const items: FileRunItem[] = run.items.map((i, n) => {
    const outcome = i.result?.outcome;
    const entry: Entry | undefined =
      outcome?.kind === "request"
        ? { id: run.seq * FILE_IDS + n, file: run.file, env: i.result!.env, at: 0, outcome: outcome as unknown as RunOutcome }
        : undefined;
    const flow = outcome?.kind === "flow" ? (outcome as unknown as FlowOutcome) : undefined;
    return { name: i.name, method: i.method, line: i.line, running: i.running, skipped: i.skipped, error: i.error, entry, flow };
  });
  return (
    <div className="result">
      <FileRunView
        title={run.title}
        items={items}
        host={host}
        onReveal={(line) => vscode.postMessage({ type: "reveal", line })}
      />
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
