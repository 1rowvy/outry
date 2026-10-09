// Ответы и итоги запусков, как их отдаёт ядро (serde): приложение — командами Tauri,
// `routy lsp` — в ответе `routy.run`. Без зависимостей от Tauri: файл собирается и в webview
// расширения VS Code (editors/vscode).

export interface Header {
  name: string;
  value: string;
}

export interface AssertOutcome {
  source: string;
  passed: boolean;
  actual: unknown;
  /** *.routy: почему не прошла — `body.total is 0` */
  detail?: string;
}

/** Запрос, вызванный из *.routy (`Login()`), — для вкладки Trace. */
export interface CallTrace {
  name: string;
  /** 0 — вызван прямо из запущенного */
  depth: number;
  args: Record<string, unknown>;
  /** ответ взят из кеша прогона */
  cached: boolean;
  status: number | null;
  duration_ms: number | null;
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
  /** *.routy: вызванные запросы */
  calls?: CallTrace[];
}

export interface FlowOutcome {
  checks: AssertOutcome[];
  saved: Record<string, string>;
  calls: CallTrace[];
  /** шаг, на котором сценарий остановился */
  error: string | null;
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
