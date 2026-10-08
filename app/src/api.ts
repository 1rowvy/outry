// Типизированные обёртки над командами из src-tauri/src/lib.rs.
import { invoke } from "@tauri-apps/api/core";

export interface ProjectInfo {
  root: string;
  id: string;
  envs: string[];
  default_env: string | null;
  /** false — каталог открыт без env.toml */
  has_config: boolean;
  files: string[];
}

export interface Header {
  name: string;
  value: string;
}

export interface AssertOutcome {
  source: string;
  passed: boolean;
  actual: unknown;
}

export interface RunOutcome {
  request: { method: string; url: string; headers: Header[]; body: string | null };
  response: {
    status: number;
    status_text: string;
    headers: Header[];
    body: string;
    duration_ms: number;
    size: number;
  };
  saved: Record<string, string>;
  asserts: AssertOutcome[];
  save_misses: string[];
}

export interface ParseError {
  line: number | null;
  message: string;
}

/** Запись истории — она же результат отправки. */
export interface Entry {
  id: number;
  file: string;
  env: string;
  /** Unix-время в миллисекундах */
  at: number;
  outcome: RunOutcome;
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
}

export interface ImportPlan {
  files: number;
  new: { route: Route; file: string; content: string }[];
  existing: { route: Route; file: string }[];
  stale: { file: string; method: string; url: string }[];
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
};
