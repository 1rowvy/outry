// `routy lsp` и всё, что расширение у него спрашивает: состояние для панелей, запуск, curl.
import { execFile } from "node:child_process";
import * as vscode from "vscode";
import {
  ErrorCodes,
  ExecuteCommandRequest,
  LanguageClient,
  ResponseError,
  type ClientCapabilities,
  type FeatureState,
  type LanguageClientOptions,
  type StaticFeature,
} from "vscode-languageclient/node";
import { findBinary } from "./binary";
import type { RunResult, State } from "./protocol";

/** Первая версия с `experimental.routyUi`; со старой `routy lsp` либо нет, либо панели пустые. */
const MIN_VERSION = [0, 6, 0];

/** Выбранное в строке состояния окружение — на рабочую папку. */
const ENV_KEY = "routy.env";

/** `experimental.routyUi`: ответы, окружение и панели показывает расширение, а не сервер. */
class UiFeature implements StaticFeature {
  fillClientCapabilities(capabilities: ClientCapabilities): void {
    capabilities.experimental = { ...(capabilities.experimental as object | undefined), routyUi: true };
  }
  initialize(): void {}
  getState(): FeatureState {
    return { kind: "static" };
  }
  clear(): void {}
}

export class Server implements vscode.Disposable {
  private client: LanguageClient | null = null;
  private timer: NodeJS.Timeout | undefined;
  private warnedOld = false;
  private readonly changed = new vscode.EventEmitter<void>();
  /** Окружения, запросы или переменные изменились (или сервер перезапущен). */
  readonly onDidChange = this.changed.event;
  state: State | null = null;

  constructor(
    private readonly ctx: vscode.ExtensionContext,
    private readonly output: vscode.OutputChannel,
  ) {}

  get running(): boolean {
    return this.client?.isRunning() ?? false;
  }

  async start(): Promise<void> {
    const bin = findBinary(this.ctx.extensionPath);
    if (!bin) {
      const pick = await vscode.window.showErrorMessage(
        "Routy: the `routy` binary was not found. Install it or set `routy.path`.",
        "Install",
        "Settings",
      );
      if (pick === "Install") {
        void vscode.env.openExternal(vscode.Uri.parse("https://1rowvy.github.io/routy/install/"));
      } else if (pick === "Settings") {
        void vscode.commands.executeCommand("workbench.action.openSettings", "routy.path");
      }
      return;
    }
    if (!(await this.checkVersion(bin))) {
      return;
    }
    const cfg = vscode.workspace.getConfiguration("routy");
    const options: LanguageClientOptions = {
      documentSelector: [{ scheme: "file", language: "routy" }],
      initializationOptions: {
        env: this.ctx.workspaceState.get<string>(ENV_KEY) || cfg.get<string>("env") || undefined,
        keyring: cfg.get<boolean>("keyring"),
        import: cfg.get<boolean>("import"),
        goDir: cfg.get<string>("goDir") || undefined,
      },
      outputChannel: this.output,
    };
    const client = new LanguageClient("routy", "Routy", { command: bin, args: ["lsp"] }, options);
    client.registerFeature(new UiFeature());
    client.onNotification("routy/didChange", () => this.scheduleRefresh());
    this.output.appendLine(`routy: ${bin}`);
    try {
      await client.start();
    } catch (e) {
      void vscode.window.showErrorMessage(`Routy: could not start \`${bin} lsp\`: ${message(e)}`);
      return;
    }
    this.client = client;
    await this.refresh();
  }

  /** Старый `routy` падает на `lsp` сразу, и клиент пишет только `write EPIPE` — проверяем заранее. */
  private async checkVersion(bin: string): Promise<boolean> {
    const out = await new Promise<string>((resolve) =>
      execFile(bin, ["--version"], { timeout: 10_000 }, (err, stdout) => resolve(err ? "" : stdout)),
    );
    const found = /(\d+)\.(\d+)\.(\d+)/.exec(out)?.slice(1).map(Number);
    if (found && !older(found, MIN_VERSION)) {
      return true;
    }
    const what = found ? `is ${found.join(".")}` : "did not report its version";
    this.output.appendLine(`routy: ${bin} ${what}, need ${MIN_VERSION.join(".")}+`);
    const pick = await vscode.window.showErrorMessage(
      `Routy: \`${bin}\` ${what}; the extension needs ${MIN_VERSION.join(".")} or newer.`,
      "Update",
      "Settings",
    );
    if (pick === "Update") {
      const term = vscode.window.createTerminal("routy update");
      term.show();
      term.sendText(`"${bin}" update`);
    } else if (pick === "Settings") {
      void vscode.commands.executeCommand("workbench.action.openSettings", "routy.path");
    }
    return false;
  }

  async stop(): Promise<void> {
    const client = this.client;
    this.client = null;
    this.state = null;
    this.changed.fire();
    if (client) {
      await client.stop().catch(() => undefined);
    }
  }

  async restart(): Promise<void> {
    await this.stop();
    await this.start();
  }

  /** `routy/didChange` приходит пачками (сохранение нескольких файлов) — перечитываем один раз. */
  private scheduleRefresh(): void {
    clearTimeout(this.timer);
    this.timer = setTimeout(() => void this.refresh(), 150);
  }

  async refresh(): Promise<void> {
    if (!this.client) {
      return;
    }
    try {
      this.state = await this.client.sendRequest<State>("routy/state");
    } catch (e) {
      this.state = null;
      if (e instanceof ResponseError && e.code === ErrorCodes.MethodNotFound && !this.warnedOld) {
        this.warnedOld = true;
        void vscode.window.showWarningMessage(
          "Routy: this `routy` is older than the extension — panels and the response view need a newer one. Run `routy update`.",
        );
      }
    }
    this.changed.fire();
  }

  private command<T>(command: string, args: unknown[]): Promise<T> {
    if (!this.client) {
      return Promise.reject(new Error("the language server is not running"));
    }
    return this.client.sendRequest(ExecuteCommandRequest.type, { command, arguments: args });
  }

  /** Запрос или сценарий на строке `line` (с 1); `env` — другое окружение, не текущее. */
  run(uri: string, line: number, env?: string): Promise<RunResult> {
    return this.command("routy.run", env ? [uri, line, env] : [uri, line]);
  }

  curl(uri: string, line: number, env?: string): Promise<string> {
    return this.command("routy.curl", env ? [uri, line, env] : [uri, line]);
  }

  async selectEnv(env: string): Promise<void> {
    await this.command("routy.selectEnv", [env]);
    await this.ctx.workspaceState.update(ENV_KEY, env);
  }

  dispose(): void {
    clearTimeout(this.timer);
    void this.stop();
    this.changed.dispose();
  }
}

export function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function older(a: number[], b: number[]): boolean {
  for (let i = 0; i < b.length; i++) {
    if (a[i] !== b[i]) {
      return a[i] < b[i];
    }
  }
  return false;
}
