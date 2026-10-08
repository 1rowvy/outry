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

export const api = {
  openProject: (dir: string) => invoke<ProjectInfo>("open_project", { dir }),
  initProject: () => invoke<ProjectInfo>("init_project"),
  refreshProject: () => invoke<ProjectInfo>("refresh_project"),
  readRequest: (path: string) => invoke<string>("read_request", { path }),
  writeRequest: (path: string, content: string) => invoke<void>("write_request", { path, content }),
  checkRequest: (content: string) => invoke<ParseError | null>("check_request", { content }),
  checkConfig: (content: string) => invoke<ParseError | null>("check_config", { content }),
  sendRequest: (env: string | null, content: string) => invoke<RunOutcome>("send_request", { env, content }),
  setSecret: (env: string | null, name: string, value: string) => invoke<void>("set_secret", { env, name, value }),
};
