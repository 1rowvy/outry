// Запускается внутри VS Code (test/run.mjs): расширение поднимает `routy lsp` для временного
// проекта, дальше — то, что видит пользователь.
import * as assert from "node:assert/strict";
import * as path from "node:path";
import * as vscode from "vscode";
import type { Api } from "../src/extension";

async function until<T>(what: string, f: () => T | undefined | null | false, ms = 20000): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = f();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 100));
  }
}

export async function run(): Promise<void> {
  const ws = process.env.ROUTY_TEST_WS!;
  const ext = vscode.extensions.getExtension<Api>("1rowvy.routy")!;
  const api = await ext.activate();
  const state = await until("routy/state", () => api.server.state);
  assert.equal(state.env, "dev");
  assert.deepEqual(state.envs, ["dev", "staging"]);
  assert.ok(state.items.some((i) => i.name === "CreateOrder" && i.method === "POST"));
  assert.equal(state.vars.find((v) => v.name === "key")?.value, "012…(16 chars)");

  // Дерево: папка auth, файл orders.routy.
  const roots = api.requests.getChildren();
  assert.deepEqual(
    roots.map((n) => (n.kind === "item" ? "?" : n.name)),
    ["auth", "orders.routy"],
  );

  const uri = vscode.Uri.file(path.join(ws, "api/orders.routy"));
  const doc = await vscode.workspace.openTextDocument(uri);
  const editor = await vscode.window.showTextDocument(doc);
  assert.equal(doc.languageId, "routy");

  // Code lens от сервера ведут на команды расширения.
  const got = await vscode.commands.executeCommand<vscode.CodeLens[]>("vscode.executeCodeLensProvider", uri);
  assert.deepEqual(
    got.map((l) => [l.command?.title, l.command?.command]),
    [
      ["▶ Send · dev", "routy.send"],
      ["in…", "routy.sendIn"],
      ["Copy as curl", "routy.copyCurl"],
    ],
  );

  // Copy as curl: Login() вызван, токен подставлен.
  await vscode.commands.executeCommand("routy.copyCurl", uri.toString(), 2);
  const curl = await vscode.env.clipboard.readText();
  assert.match(curl, /^curl -X POST http:\/\/127\.0\.0\.1:\d+\/orders \\/);
  assert.ok(curl.includes("-H 'Authorization: Bearer tok-1'"), curl);

  // Send с курсора: ответ — во вкладке рядом.
  editor.selection = new vscode.Selection(3, 0, 3, 0);
  await vscode.commands.executeCommand("routy.send");
  const shown = await until("response", () => {
    const c = api.panel.current;
    return c?.type === "result" || c?.type === "error" ? c : null;
  });
  assert.equal(shown.type, "result", JSON.stringify(shown));
  if (shown.type === "result") {
    assert.equal(shown.result.name, "CreateOrder");
    assert.equal(shown.result.passed, true);
    assert.equal((shown.result.outcome as unknown as { response: { status: number } }).response.status, 201);
  }
  // Вкладка и её заголовок доходят до tabGroups с задержкой (сначала — «CreateOrder · dev…»).
  const tab = await until("response tab", () =>
    vscode.window.tabGroups.all
      .flatMap((g) => g.tabs)
      .find(
        (t) =>
          t.input instanceof vscode.TabInputWebview &&
          t.input.viewType.endsWith("routy.response") &&
          t.label === "CreateOrder · dev",
      ),
  );
  assert.notEqual(tab.group.viewColumn, vscode.ViewColumn.One, "beside the file");
  // Webview отрисовал ResponseView из app/src.
  const rendered = await until("rendered response", () => api.panel.rendered);
  assert.match(rendered.text, /201\s*Created/);

  // Окружение из строки состояния.
  await api.server.selectEnv("staging");
  await until("env switch", () => api.server.state?.env === "staging");

  // Ошибки как в `routy check`.
  await editor.edit((e) => e.replace(new vscode.Range(2, 0, 2, 0), "  query { x: Logn() }\n"));
  const diags = await until("diagnostics", () => {
    const d = vscode.languages.getDiagnostics(uri);
    return d.length > 0 ? d : null;
  });
  assert.ok(diags.some((d) => d.message.includes("Logn")), diags.map((d) => d.message).join("\n"));
}
