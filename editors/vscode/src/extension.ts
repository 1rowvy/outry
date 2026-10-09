// Расширение Outry для VS Code: LSP-клиент к `outry lsp` и интерфейс вокруг него — ответ во
// вкладке, окружение в строке состояния, запросы и переменные в боковой панели.
// Правил формата здесь нет: всё считает `outry` (crates/outry-cli/src/lsp.rs).
import * as path from "node:path";
import * as vscode from "vscode";
import { ResponsePanel } from "./panel";
import { Server, message } from "./server";
import { RequestsView, VariablesView, type Node } from "./views";

/** Что запускать: файл и строка (с 1). */
interface Target {
  uri: string;
  line: number;
}

let server: Server | null = null;

/** Что `activate` отдаёт наружу — для интеграционных тестов (test/suite.ts). */
export interface Api {
  server: Server;
  panel: ResponsePanel;
  requests: RequestsView;
}

export async function activate(ctx: vscode.ExtensionContext): Promise<Api> {
  const output = vscode.window.createOutputChannel("Outry");
  const srv = new Server(ctx, output);
  server = srv;
  const panel = new ResponsePanel(ctx);

  const status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 50);
  status.command = "outry.selectEnvironment";
  status.name = "Outry environment";
  const updateStatus = () => {
    const state = srv.state;
    if (!state) {
      status.hide();
      return;
    }
    status.text = `$(server-environment) ${state.env}`;
    status.tooltip = state.envs.length > 1 ? "Outry environment — click to switch" : "Outry environment";
    status.show();
  };
  srv.onDidChange(updateStatus);

  /** Аргументы code lens — `(uri, line)`, меню дерева — узел, палитра — курсор редактора. */
  const target = (arg?: unknown, line?: unknown): Target | null => {
    if (typeof arg === "string" && typeof line === "number") {
      return { uri: arg, line };
    }
    const node = arg as Node | undefined;
    if (node?.kind === "item") {
      return { uri: node.item.uri, line: node.item.line };
    }
    const editor = vscode.window.activeTextEditor;
    if (editor?.document.languageId === "outry") {
      return { uri: editor.document.uri.toString(), line: editor.selection.active.line + 1 };
    }
    void vscode.window.showInformationMessage("Outry: open a .outry file and put the cursor on a request.");
    return null;
  };

  /** Имя для заголовка, пока ответа нет: элемент из `outry/state` на этой строке или выше. */
  const nameAt = (t: Target): string => {
    const items = (srv.state?.items ?? []).filter((i) => sameFile(i.uri, t.uri) && i.line <= t.line);
    const item = items.sort((a, b) => b.line - a.line)[0];
    return item ? (item.name ?? `${item.method} ${item.target}`) : "request";
  };

  const pickEnv = async (placeHolder: string): Promise<string | undefined> => {
    const state = srv.state;
    if (!state || state.envs.length === 0) {
      void vscode.window.showInformationMessage("Outry: no environments in env.toml.");
      return undefined;
    }
    const picked = await vscode.window.showQuickPick(
      state.envs.map((e) => ({ label: e, description: e === state.env ? "current" : undefined })),
      { placeHolder },
    );
    return picked?.label;
  };

  const send = async (t: Target, env?: string) => {
    const name = nameAt(t);
    const where = env ?? srv.state?.env ?? "";
    panel.loading(`${name} · ${where}`);
    await vscode.window.withProgress(
      { location: vscode.ProgressLocation.Window, title: `Outry: ${name} · ${where}` },
      async () => {
        try {
          const result = await srv.run(t.uri, t.line, env);
          panel.result(result, vscode.Uri.parse(t.uri).fsPath);
        } catch (e) {
          panel.error(`${name} · ${where}`, message(e));
        }
      },
    );
  };

  const requests = new RequestsView(srv);
  const variables = new VariablesView(srv);
  ctx.subscriptions.push(
    output,
    srv,
    panel,
    status,
    vscode.window.registerTreeDataProvider("outry.requests", requests),
    vscode.window.registerTreeDataProvider("outry.variables", variables),

    vscode.commands.registerCommand("outry.send", async (arg?: unknown, line?: unknown) => {
      const t = target(arg, line);
      if (t) {
        await send(t);
      }
    }),
    vscode.commands.registerCommand("outry.sendIn", async (arg?: unknown, line?: unknown) => {
      const t = target(arg, line);
      const env = t && (await pickEnv("Send in environment"));
      if (t && env) {
        await send(t, env);
      }
    }),
    vscode.commands.registerCommand("outry.copyCurl", async (arg?: unknown, line?: unknown) => {
      const t = target(arg, line);
      if (!t) {
        return;
      }
      try {
        const curl = await srv.curl(t.uri, t.line);
        await vscode.env.clipboard.writeText(curl);
        void vscode.window.setStatusBarMessage(`$(check) Outry: ${nameAt(t)} copied as curl`, 3000);
      } catch (e) {
        void vscode.window.showErrorMessage(`Outry: ${message(e)}`);
      }
    }),
    vscode.commands.registerCommand("outry.selectEnvironment", async () => {
      const env = await pickEnv("Outry environment");
      if (env && env !== srv.state?.env) {
        try {
          await srv.selectEnv(env);
        } catch (e) {
          void vscode.window.showErrorMessage(`Outry: ${message(e)}`);
        }
      }
    }),
    vscode.commands.registerCommand("outry.refresh", () => srv.refresh()),
    vscode.commands.registerCommand("outry.restart", () => srv.restart()),
    vscode.commands.registerCommand("outry.showOutput", () => output.show()),

    vscode.workspace.onDidChangeConfiguration(async (e) => {
      if (["path", "keyring", "import", "goDir"].some((k) => e.affectsConfiguration(`outry.${k}`))) {
        await srv.restart();
      } else if (e.affectsConfiguration("outry.env")) {
        const env = vscode.workspace.getConfiguration("outry").get<string>("env");
        if (env && srv.state?.envs.includes(env)) {
          await srv.selectEnv(env);
        }
      }
    }),
  );

  await srv.start();
  return { server: srv, panel, requests };
}

export async function deactivate(): Promise<void> {
  await server?.stop();
}

/** URI от сервера и от VS Code кодируют путь по-разному (`C:` и `c%3A`) — сравниваем пути. */
function sameFile(a: string, b: string): boolean {
  const norm = (u: string) => {
    const p = path.normalize(vscode.Uri.parse(u).fsPath);
    return process.platform === "linux" ? p : p.toLowerCase();
  };
  return norm(a) === norm(b);
}
