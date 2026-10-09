// Типизированные обёртки над командами из src-tauri/src/lib.rs.
import { invoke } from "@tauri-apps/api/core";
import type { Entry, FlowOutcome } from "./types";

// Ответы и итоги запусков — в types.ts: их же показывает расширение VS Code.
export type { AssertOutcome, CallTrace, Entry, FlowOutcome, Header, RunOutcome } from "./types";

export interface ProjectInfo {
  root: string;
  id: string;
  envs: string[];
  default_env: string | null;
  /** false — каталог открыт без env.toml */
  has_config: boolean;
  files: string[];
  /** путь → метод из строки запроса */
  methods: Record<string, string>;
}

/** Результат запуска *.routy: ответ запроса или итог сценария. */
export interface RoutyResult {
  name: string;
  entry: Entry | null;
  flow: FlowOutcome | null;
}

export interface Callable {
  /** `Login` или `users.Create` */
  name: string;
  kind: "request" | "flow";
  params: string[];
  doc: string | null;
}

export interface Symbols {
  callables: Callable[];
  shapes: string[];
}

export interface ParseError {
  line: number | null;
  /** с 1; есть у ошибок *.routy */
  col: number | null;
  message: string;
}

export interface VarName {
  name: string;
  /** "env" — из env.toml, "saved" — из `> save` (значение не отдаём), "secret" — объявлен в `secrets`,
   *  "dynamic" — `$uuid` и т.п. (value — описание). */
  source: "env" | "saved" | "secret" | "dynamic";
  value: string | null;
}

export type VarSource = "override" | "saved" | "process_env" | "env" | "secret" | "dynamic";

export interface VarInfo {
  name: string;
  /** null — объявленный секрет без значения; у секретов — маска */
  value: string | null;
  source: VarSource | null;
  secret: boolean;
}

export interface Route {
  /** GET, POST, … или "ANY" (r.Handle, gin Any) */
  method: string;
  /** /users/{{id}} */
  path: string;
  /** Go-файл относительно сканируемого каталога */
  source: string;
  line: number;
  handler: string | null;
  router: string;
  /** Что передавать — из кода обработчика */
  info: RouteInfo;
}

export interface Field {
  name: string;
  /** тип в Go */
  ty: string;
  required: boolean;
  comment: string | null;
  /** тип в JSON, если известен */
  json: "string" | "number" | "boolean" | "array" | "object" | null;
}

export interface RouteInfo {
  summary: string | null;
  description: string[];
  query: Field[];
  headers: string[];
  body: { type_name: string; fields: Field[]; example: string } | null;
  /** что обработчик отвечает: тип Go и shape для `body matches` */
  response: { type_name: string; shape: string } | null;
}

/** Расхождение запроса или shape с кодом; `kind` и его поля — как `ChangeKind` в ядре */
export interface RouteChange {
  /** ключ для `importFix` */
  id: string;
  kind: string;
  file: string;
  line: number;
  col: number;
  severity: "error" | "warning";
  message: string;
  /** место в Go-коде, относительно сканируемого каталога */
  go: { file: string; line: number };
  /** исправляется автоматически; `diff` — что поменяется */
  fixable: boolean;
  diff?: string;
}

/** Shape для структуры Go из ответа */
export interface ShapeDef {
  name: string;
  go_type: string;
  source: string;
  line: number;
  shape: string;
}

export interface ImportPlan {
  files: number;
  new: { route: Route; file: string; content: string }[];
  existing: { route: Route; file: string; changes: RouteChange[] }[];
  stale: { file: string; line: number; method: string; url: string }[];
  /** файлы, где все запросы — к пропавшим роутам */
  prunable: string[];
  new_shapes: ShapeDef[];
  shape_changes: RouteChange[];
  shapes_file: string;
  warnings: string[];
}

export interface ImportReport {
  dir: string;
  plan: ImportPlan;
  created: string[];
}

export const api = {
  openProject: (dir: string) => invoke<ProjectInfo>("open_project", { dir }),
  initProject: () => invoke<ProjectInfo>("init_project"),
  refreshProject: () => invoke<ProjectInfo>("refresh_project"),
  readRequest: (path: string) => invoke<string>("read_request", { path }),
  writeRequest: (path: string, content: string) => invoke<void>("write_request", { path, content }),
  checkRequest: (content: string) => invoke<ParseError | null>("check_request", { content }),
  checkConfig: (content: string) => invoke<ParseError | null>("check_config", { content }),
  /** routy check для открытого *.routy (с текстом из редактора) */
  checkRouty: (path: string, content: string) => invoke<ParseError[]>("check_routy", { path, content }),
  routySymbols: () => invoke<Symbols>("routy_symbols"),
  /** Запрос или сценарий на строке line (с 1) */
  runRouty: (id: number, env: string | null, path: string, content: string, line: number) =>
    invoke<RoutyResult>("run_routy", { id, env, path, content, line }),
  /** Забыть кеш вызовов и cookies *.routy */
  resetRun: () => invoke<void>("reset_run"),
  renamePath: (from: string, to: string) => invoke<void>("rename_path", { from, to }),
  deletePath: (path: string) => invoke<void>("delete_path", { path }),
  /** id — от фронта, для cancelRequest */
  sendRequest: (id: number, env: string | null, path: string, content: string) =>
    invoke<Entry>("send_request", { id, env, path, content }),
  cancelRequest: (id: number) => invoke<void>("cancel_request", { id }),
  history: () => invoke<Entry[]>("history"),
  clearHistory: () => invoke<void>("clear_history"),
  setHistoryPersist: (persist: boolean) => invoke<void>("set_history_persist", { persist }),
  responseImage: (id: number) => invoke<string | null>("response_image", { id }),
  saveBody: (id: number, dest: string) => invoke<void>("save_body", { id, dest }),
  variables: (env: string | null) => invoke<VarInfo[]>("variables", { env }),
  clearSaved: (env: string | null) => invoke<void>("clear_saved", { env }),
  varNames: (env: string | null) => invoke<VarName[]>("var_names", { env }),
  setSecret: (env: string | null, name: string, value: string) => invoke<void>("set_secret", { env, name, value }),
  /** dir: null — каталог над api/; apply — создать недостающие файлы */
  importGo: (dir: string | null, apply: boolean) => invoke<ImportReport>("import_go", { dir, apply }),
  /** routy import go --fix [--prune]; ids — выбранные расхождения, null — все исправимые */
  importFix: (dir: string | null, ids: string[] | null, prune: boolean) =>
    invoke<ImportReport>("import_fix", { dir, ids, prune }),
};
