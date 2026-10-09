// Ответ в VS Code: те же компоненты, что в приложении; данные — сообщениями от расширения
// (src/panel.ts), сохранение тела — через него же.
import { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
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

type Message =
  | { type: "loading"; title: string }
  | { type: "error"; title: string; message: string }
  | { type: "result"; seq: number; file: string; result: RunResult };

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
  saveBody: async (_entry, name) => vscode.postMessage({ type: "save", name }),
  noImage: "The response has no image data.",
};

function App() {
  const [shown, setShown] = useState<Extract<Message, { type: "result" | "error" }> | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  useEffect(() => {
    const on = (e: MessageEvent<Message>) => {
      const m = e.data;
      if (m.type === "loading") {
        setBusy(m.title);
        return;
      }
      if (m.type === "result" && m.result.raw) raws.set(m.seq, m.result.raw);
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

createRoot(document.getElementById("root")!).render(<App />);
