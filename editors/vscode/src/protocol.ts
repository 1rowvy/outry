// Что расширению отдаёт `routy lsp` сверх LSP (см. crates/routy-cli/src/lsp.rs, клиент с `routyUi`).

/** Запрос или сценарий проекта — `routy/state`. */
export interface Item {
  uri: string;
  /** путь относительно корня проекта, через `/` */
  path: string;
  name: string | null;
  flow: boolean;
  method: string;
  target: string;
  /** строка (с 1), где начинается элемент */
  line: number;
}

export interface Var {
  name: string;
  /** секреты — маской; `null` — значения нет */
  value: string | null;
  source: "override" | "saved" | "process_env" | "env" | "secret" | "dynamic" | null;
  secret: boolean;
}

/** Ответ на `routy/state`. */
export interface State {
  root: string;
  env: string;
  envs: string[];
  items: Item[];
  vars: Var[];
}

/** Ответ `routy.run`. `outcome` — `exec::Outcome` ядра: `kind` + RunOutcome / FlowOutcome. */
export interface RunResult {
  name: string;
  env: string;
  passed: boolean;
  outcome: { kind: "request" | "flow" } & Record<string, unknown>;
  /** тело ответа в base64, если оно не UTF-8 (картинка, архив) */
  raw?: string;
}
