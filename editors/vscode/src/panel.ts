// Ответ во вкладке рядом с файлом: те же ResponseView / FlowView, что в приложении (webview/).
import { randomBytes } from "node:crypto";
import * as vscode from "vscode";
import type { RunResult } from "./protocol";

/** Сообщения в webview — см. webview/main.tsx. */
export type ToWebview =
  | { type: "loading"; title: string }
  | { type: "error"; title: string; message: string }
  | { type: "result"; seq: number; file: string; result: RunResult };

type FromWebview = { type: "ready" } | { type: "save"; name: string } | { type: "shown"; seq: number; text: string };

export class ResponsePanel implements vscode.Disposable {
  private panel: vscode.WebviewPanel | null = null;
  private ready = false;
  /** Последнее сообщение: его получает и только что открытый webview. */
  private last: ToWebview | null = null;
  private seq = 0;

  constructor(private readonly ctx: vscode.ExtensionContext) {}

  /** Строка статуса отрисованного ответа — из webview (для тестов). */
  rendered: { seq: number; text: string } | null = null;

  /** Что показано сейчас (или показывается, пока идёт запрос). */
  get current(): ToWebview | null {
    return this.last;
  }

  loading(title: string): void {
    this.post({ type: "loading", title }, `${title}…`);
  }

  error(title: string, message: string): void {
    this.post({ type: "error", title, message }, title);
  }

  result(result: RunResult, file: string): void {
    this.post({ type: "result", seq: ++this.seq, file, result }, `${result.name} · ${result.env}`);
  }

  private post(msg: ToWebview, title: string): void {
    this.last = msg;
    const panel = this.open();
    panel.title = title;
    if (this.ready) {
      void panel.webview.postMessage(msg);
    }
  }

  private open(): vscode.WebviewPanel {
    if (this.panel) {
      if (!this.panel.visible) {
        this.panel.reveal(undefined, true);
      }
      return this.panel;
    }
    const root = vscode.Uri.joinPath(this.ctx.extensionUri, "dist", "webview");
    const panel = vscode.window.createWebviewPanel(
      "routy.response",
      "Response",
      { viewColumn: vscode.ViewColumn.Beside, preserveFocus: true },
      { enableScripts: true, retainContextWhenHidden: true, localResourceRoots: [root] },
    );
    panel.iconPath = vscode.Uri.joinPath(this.ctx.extensionUri, "media", "file.svg");
    panel.webview.html = html(panel.webview, root);
    panel.webview.onDidReceiveMessage((m: FromWebview) => void this.receive(m));
    panel.onDidDispose(() => {
      this.panel = null;
      this.ready = false;
    });
    this.panel = panel;
    return panel;
  }

  private async receive(m: FromWebview): Promise<void> {
    if (m.type === "ready") {
      this.ready = true;
      if (this.last) {
        void this.panel?.webview.postMessage(this.last);
      }
      return;
    }
    if (m.type === "shown") {
      this.rendered = { seq: m.seq, text: m.text };
      return;
    }
    if (m.type === "save" && this.last?.type === "result") {
      const { result } = this.last;
      const body = (result.outcome as { response?: { body: string } }).response?.body ?? "";
      const folder = vscode.workspace.workspaceFolders?.[0]?.uri;
      const dest = await vscode.window.showSaveDialog({
        title: "Save response body",
        defaultUri: folder ? vscode.Uri.joinPath(folder, m.name) : undefined,
      });
      if (dest) {
        const bytes = result.raw ? Buffer.from(result.raw, "base64") : Buffer.from(body, "utf8");
        await vscode.workspace.fs.writeFile(dest, bytes);
      }
    }
  }

  dispose(): void {
    this.panel?.dispose();
  }
}

function html(webview: vscode.Webview, root: vscode.Uri): string {
  const nonce = randomBytes(16).toString("base64");
  const src = (f: string) => webview.asWebviewUri(vscode.Uri.joinPath(root, f)).toString();
  // CodeMirror вставляет <style> — отсюда 'unsafe-inline' для стилей. Скрипты — только свой.
  const csp = [
    "default-src 'none'",
    `style-src ${webview.cspSource} 'unsafe-inline'`,
    `script-src 'nonce-${nonce}'`,
    `img-src ${webview.cspSource} data:`,
    `font-src ${webview.cspSource}`,
  ].join("; ");
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="${csp}">
<link rel="stylesheet" href="${src("webview.css")}">
<title>Response</title>
</head>
<body>
<div id="root"></div>
<script nonce="${nonce}" src="${src("webview.js")}"></script>
</body>
</html>`;
}
