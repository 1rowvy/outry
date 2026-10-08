import { useCallback, useEffect, useRef, useState, type MouseEvent } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  api,
  type Entry,
  type FlowOutcome,
  type ImportReport,
  type ParseError,
  type ProjectInfo,
  type Symbols,
  type VarInfo,
  type VarName,
} from "./api";
import { CodeEditor } from "./CodeEditor";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { FileTree, type TreeTarget } from "./FileTree";
import { FlowView } from "./FlowView";
import { HistoryList } from "./HistoryList";
import { ResponseView } from "./ResponseView";
import { RoutesPanel } from "./RoutesPanel";
import { SettingsMenu } from "./SettingsMenu";
import { TitleBar } from "./TitleBar";
import { UpdateBanner } from "./UpdateBanner";
import { useUpdates } from "./updater";
import { VarsPanel } from "./VarsPanel";

const LAST_PROJECT_KEY = "routy.lastProject";
const AUTO_UPDATE_KEY = "routy.autoUpdate";
const PERSIST_HISTORY_KEY = "routy.persistHistory";
/** + корень проекта: каталог с Go-кодом для синхронизации роутов */
const IMPORT_DIR_KEY = "routy.importDir:";
const CONFIG = "env.toml";
const CONFIG_EXAMPLE = `default = "dev"
secrets = ["token"]

[vars]
version = "v1"

[env.dev]
base = "http://localhost:8080"

[env.prod]
base = "https://api.example.com"`;
const NEW_REQUEST = "GET {{base}}/\n\n> assert status == 200\n";
const NEW_ROUTY = "GET / {\n  expect { status == 200 }\n}\n";
const REQUEST_EXT = /\.(http|routy)$/;

const isRouty = (path: string | null) => !!path?.endsWith(".routy");

// Метод из строки запроса — только для метки в шапке; разбирает ядро.
// В *.routy — запрос или сценарий, на котором стоит курсор.
function requestMethod(text: string, routy: boolean, cursor: number): string {
  const lines = text.split("\n");
  if (routy) {
    let found = "";
    for (let i = 0; i < lines.length; i++) {
      const m = /^([A-Z]{2,})\s|^(flow)\s/.exec(lines[i]);
      if (!m) continue;
      if (i + 1 > cursor && found) break;
      found = m[1] ?? "FLOW";
    }
    return found || "GET";
  }
  for (const line of lines) {
    const t = line.trim();
    if (!t || t.startsWith("#") || t.startsWith("//")) continue;
    return /^([A-Z]+)\s/.exec(t)?.[1] ?? "GET";
  }
  return "GET";
}

/** Последний ответ, ошибка и запрос в полёте — на каждый файл, чтобы запросы шли параллельно. */
interface Run {
  entry?: Entry;
  /** *.routy: итог сценария */
  flow?: FlowOutcome;
  /** *.routy: что запускали */
  name?: string;
  error?: string;
  /** id для отмены */
  pending?: number;
}

type View = "response" | "history" | "vars" | "routes";

/** Форма пути в сайдбаре: новый запрос или переименование/перенос. */
interface PathForm {
  from?: TreeTarget;
  value: string;
}

/** `users/create` → `users/create.routy` (или с расширением `ext`); каталоги — без расширения. */
function normalizePath(value: string, isDir: boolean, ext = ".routy"): string {
  const p = value.trim().replace(/^\/+|\/+$/g, "");
  return isDir || REQUEST_EXT.test(p) ? p : p + ext;
}

/** Путь после переименования `from` → `to` (файла или каталога). */
function movedPath(path: string, from: string, to: string): string | null {
  if (path === from) return to;
  if (path.startsWith(from + "/")) return to + path.slice(from.length);
  return null;
}

function storage(key: string, value?: string | null): string | null {
  try {
    if (value === undefined) return localStorage.getItem(key);
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    // localStorage может быть недоступен — это только удобство.
  }
  return null;
}

export default function App() {
  const [project, setProject] = useState<ProjectInfo | null>(null);
  const [env, setEnv] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [content, setContent] = useState("");
  const [savedContent, setSavedContent] = useState("");
  const [parseErrors, setParseErrors] = useState<ParseError[]>([]);
  const [cursor, setCursor] = useState(1);
  const [symbols, setSymbols] = useState<Symbols | null>(null);
  const [runs, setRuns] = useState<Record<string, Run>>({});
  const [opened, setOpened] = useState<Entry | null>(null);
  const [view, setView] = useState<View>("response");
  const [history, setHistory] = useState<Entry[]>([]);
  const [varList, setVarList] = useState<VarInfo[]>([]);
  /** Растёт после отправки и правок переменных — перечитать историю и переменные. */
  const [tick, setTick] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [pathForm, setPathForm] = useState<PathForm | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; items: MenuItem[] } | null>(null);
  const [secretForm, setSecretForm] = useState<{ name: string; value: string } | null>(null);
  const [version, setVersion] = useState<string | null>(null);
  const [vars, setVars] = useState<VarName[]>([]);
  const [routes, setRoutes] = useState<ImportReport | null>(null);
  const [routesBusy, setRoutesBusy] = useState(false);
  const [autoUpdate, setAutoUpdate] = useState(() => storage(AUTO_UPDATE_KEY) === "1");
  const [persistHistory, setPersistHistory] = useState(() => storage(PERSIST_HISTORY_KEY) === "1");
  const updates = useUpdates(autoUpdate);
  const nextId = useRef(0);

  const dirty = content !== savedContent;
  const run = selected ? runs[selected] : undefined;
  const shown = opened ?? run?.entry ?? null;
  const bump = () => setTick((t) => t + 1);
  const isConfig = selected === CONFIG;
  const routy = isRouty(selected);
  const parseError = parseErrors[0] ?? null;
  const method = requestMethod(content, routy, cursor);
  const cut = (selected ?? "").lastIndexOf("/") + 1;
  const dir = (selected ?? "").slice(0, cut);
  const name = (selected ?? "").slice(cut).replace(REQUEST_EXT, "");
  // Для обработчика событий файловой системы нужны актуальные значения без переподписки.
  const live = useRef({ selected, dirty });
  live.current = { selected, dirty };

  const applyProject = useCallback((info: ProjectInfo) => {
    setProject(info);
    setEnv(info.default_env);
    setSelected(null);
    setContent("");
    setSavedContent("");
    setRuns({});
    setOpened(null);
    setError(null);
    setRoutes(null);
  }, []);

  const openProject = useCallback(
    async (dir: string) => {
      try {
        applyProject(await api.openProject(dir));
        storage(LAST_PROJECT_KEY, dir);
      } catch (e) {
        setError(String(e));
      }
    },
    [applyProject],
  );

  const initProject = async () => {
    if (dirty && !window.confirm("You have unsaved changes. Continue?")) return;
    try {
      applyProject(await api.initProject());
    } catch (e) {
      setError(String(e));
    }
  };

  useEffect(() => {
    getVersion().then(setVersion, () => {});
  }, []);

  useEffect(() => {
    const last = storage(LAST_PROJECT_KEY);
    if (last) openProject(last);
  }, [openProject]);

  // Правки в редакторе снаружи (VS Code, git pull) сразу видны в GUI.
  useEffect(() => {
    let timer: number | undefined;
    const changed = new Set<string>();
    const unlisten = listen<string[]>("project-changed", (ev) => {
      ev.payload.forEach((p) => changed.add(p));
      clearTimeout(timer);
      timer = window.setTimeout(async () => {
        const paths = [...changed];
        changed.clear();
        try {
          const info = await api.refreshProject();
          setProject(info);
          setEnv((cur) => (cur && info.envs.includes(cur) ? cur : info.default_env));
          const { selected, dirty } = live.current;
          if (selected && !dirty && paths.includes(selected) && info.files.includes(selected)) {
            const text = await api.readRequest(selected);
            setContent(text);
            setSavedContent(text);
          }
        } catch (e) {
          setError(String(e));
        }
      }, 200);
    });
    return () => {
      clearTimeout(timer);
      unlisten.then((f) => f());
    };
  }, []);

  // Имена переменных для автодополнения и панель переменных; `> save` после отправки добавляет новые.
  useEffect(() => {
    if (!project) return setVars([]);
    api.varNames(env).then(setVars, () => setVars([]));
    api.variables(env).then(setVarList, () => setVarList([]));
  }, [project, env, tick]);

  // История на диске включается для каждого открытого проекта заново.
  const root = project?.root;
  useEffect(() => {
    if (!root) return;
    api.setHistoryPersist(persistHistory).then(bump, (e) => setError(String(e)));
  }, [root, persistHistory]);

  useEffect(() => {
    if (!root) return setHistory([]);
    api.history().then(setHistory, () => setHistory([]));
  }, [root, tick]);

  // Проверка на лету: синтаксис, а в *.routy — ещё имена вызовов, аргументы и формы (routy check).
  useEffect(() => {
    if (!selected) return;
    const one = (e: ParseError | null) => setParseErrors(e ? [e] : []);
    const t = window.setTimeout(() => {
      if (selected === CONFIG) api.checkConfig(content).then(one);
      else if (isRouty(selected)) api.checkRouty(selected, content).then(setParseErrors, (e) => one({ line: null, col: null, message: String(e) }));
      else api.checkRequest(content).then(one);
    }, 150);
    return () => clearTimeout(t);
  }, [content, selected]);

  // Имена запросов и сценариев для автодополнения в *.routy.
  useEffect(() => {
    if (!project || !routy) return;
    api.routySymbols().then(setSymbols, () => setSymbols(null));
  }, [project, routy, selected, savedContent]);

  const select = async (path: string) => {
    if (dirty && !window.confirm("You have unsaved changes. Open another file?")) return;
    try {
      const text = await api.readRequest(path);
      setSelected(path);
      setContent(text);
      setSavedContent(text);
      setOpened(null);
      if (path === CONFIG) setView("response");
    } catch (e) {
      setError(String(e));
    }
  };

  const save = useCallback(async () => {
    if (!selected) return;
    if (selected === CONFIG && parseError) return;
    try {
      await api.writeRequest(selected, content);
      setSavedContent(content);
    } catch (e) {
      setError(String(e));
    }
  }, [selected, content, parseError]);

  const send = useCallback(async (line?: number) => {
    if (!selected || selected === CONFIG || runs[selected]?.pending) return;
    const path = selected;
    const id = ++nextId.current;
    setRuns((r) => ({ ...r, [path]: { ...r[path], pending: id, error: undefined } }));
    setOpened(null);
    setView("response");
    try {
      if (isRouty(path)) {
        const res = await api.runRouty(id, env, path, content, line ?? cursor);
        setRuns((r) => ({ ...r, [path]: { entry: res.entry ?? undefined, flow: res.flow ?? undefined, name: res.name } }));
      } else {
        const entry = await api.sendRequest(id, env, path, content);
        setRuns((r) => ({ ...r, [path]: { entry } }));
      }
    } catch (e) {
      const msg = String(e);
      setRuns((r) => ({ ...r, [path]: { ...r[path], pending: undefined, error: msg === "cancelled" ? undefined : msg } }));
    } finally {
      bump();
    }
  }, [selected, runs, env, content, cursor]);

  const cancel = () => {
    if (run?.pending) api.cancelRequest(run.pending);
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.ctrlKey || e.metaKey;
      if (mod && e.key === "Enter") {
        e.preventDefault();
        send();
      } else if (mod && e.key.toLowerCase() === "s") {
        e.preventDefault();
        save();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [send, save]);

  const pickFolder = async () => {
    const dir = await open({ directory: true, title: "Project folder (with api/env.toml)" });
    if (typeof dir === "string") openProject(dir);
  };

  const refresh = async () => setProject(await api.refreshProject());

  /** Роуты из Go-кода; apply — создать недостающие файлы и пересканировать. */
  const syncRoutes = async (apply = false, dir?: string) => {
    if (!project) return;
    const key = IMPORT_DIR_KEY + project.root;
    setView("routes");
    setRoutesBusy(true);
    try {
      const source = dir ?? storage(key);
      if (apply) await api.importGo(source, true);
      const report = await api.importGo(source, false);
      if (dir) storage(key, dir);
      setRoutes(report);
      if (apply) await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setRoutesBusy(false);
    }
  };

  const pickRoutesDir = async () => {
    const dir = await open({ directory: true, title: "Folder with the service's Go code", defaultPath: routes?.dir });
    if (typeof dir === "string") syncRoutes(false, dir);
  };

  const submitPath = async () => {
    if (!pathForm) return;
    const from = pathForm.from;
    const ext = from?.isFile ? (from.path.match(REQUEST_EXT)?.[0] ?? ".routy") : ".routy";
    const path = normalizePath(pathForm.value, from ? !from.isFile : false, ext);
    if (!path || REQUEST_EXT.test(path) && path.replace(REQUEST_EXT, "") === "") return;
    try {
      if (!from) {
        if (project?.files.includes(path)) throw new Error(`${path} already exists`);
        await api.writeRequest(path, isRouty(path) ? NEW_ROUTY : NEW_REQUEST);
        setPathForm(null);
        await refresh();
        await select(path);
        return;
      }
      if (path !== from.path) {
        await api.renamePath(from.path, path);
        const move = (p: string) => movedPath(p, from.path, path) ?? p;
        setSelected((s) => s && move(s));
        setRuns((r) => Object.fromEntries(Object.entries(r).map(([k, v]) => [move(k), v])));
        await refresh();
      }
      setPathForm(null);
    } catch (e) {
      setError(String(e));
    }
  };

  const remove = async (t: TreeTarget) => {
    const what = t.isFile ? t.path : `${t.count} request${t.count === 1 ? "" : "s"} in ${t.path}/`;
    if (!window.confirm(`Delete ${what}?`)) return;
    try {
      await api.deletePath(t.path);
      if (selected && movedPath(selected, t.path, "") !== null) {
        setSelected(null);
        setContent("");
        setSavedContent("");
      }
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  const duplicate = async (path: string) => {
    const ext = path.match(REQUEST_EXT)?.[0] ?? ".routy";
    const base = path.replace(REQUEST_EXT, "");
    let copy = `${base}-copy${ext}`;
    for (let i = 2; project?.files.includes(copy); i++) copy = `${base}-copy${i}${ext}`;
    try {
      await api.writeRequest(copy, await api.readRequest(path));
      await refresh();
      await select(copy);
    } catch (e) {
      setError(String(e));
    }
  };

  const openMenu = (e: MouseEvent, t: TreeTarget | null) => {
    e.preventDefault();
    e.stopPropagation();
    const dirPrefix = t ? (t.isFile ? t.path.slice(0, t.path.lastIndexOf("/") + 1) : `${t.path}/`) : "";
    const items: MenuItem[] = [{ label: "New request", run: () => setPathForm({ value: dirPrefix }) }];
    if (t?.isFile) {
      items.unshift({ label: "Open", run: () => select(t.path) });
      items.push({ label: "Duplicate", run: () => duplicate(t.path) });
    }
    if (t) {
      items.push(
        { label: "Rename / Move…", run: () => setPathForm({ from: t, value: t.isFile ? t.path.replace(REQUEST_EXT, "") : t.path }) },
        { label: "Delete", danger: true, run: () => remove(t) },
      );
    }
    setMenu({ x: e.clientX, y: e.clientY, items });
  };

  const clearSaved = async () => {
    try {
      await api.clearSaved(env);
      bump();
    } catch (e) {
      setError(String(e));
    }
  };

  const resetRun = async () => {
    try {
      await api.resetRun();
    } catch (e) {
      setError(String(e));
    }
  };

  const clearHistory = async () => {
    try {
      await api.clearHistory();
      setOpened(null);
      setRuns((r) => Object.fromEntries(Object.entries(r).map(([k, v]) => [k, { pending: v.pending }])));
      bump();
    } catch (e) {
      setError(String(e));
    }
  };

  const toggleAutoUpdate = (v: boolean) => {
    setAutoUpdate(v);
    storage(AUTO_UPDATE_KEY, v ? "1" : null);
  };

  const togglePersistHistory = (v: boolean) => {
    setPersistHistory(v);
    storage(PERSIST_HISTORY_KEY, v ? "1" : null);
  };

  const saveSecret = async () => {
    if (!secretForm?.name || !secretForm.value) return;
    try {
      await api.setSecret(env, secretForm.name.trim(), secretForm.value);
      setSecretForm(null);
      bump();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="app">
      <TitleBar
        version={version}
        menu={
          <SettingsMenu
            updates={updates}
            autoUpdate={autoUpdate}
            onAutoUpdate={toggleAutoUpdate}
            persistHistory={persistHistory}
            onPersistHistory={togglePersistHistory}
          />
        }
      />
      {menu && <ContextMenu {...menu} onClose={() => setMenu(null)} />}
      <aside className="sidebar">
        <div className="project">
          <button className="project-name" onClick={pickFolder} title={project ? `${project.root}\nOpen another project` : undefined}>
            <span className="ellipsis">{project ? project.id : "Open project…"}</span>
            {project && <span className="muted">change</span>}
          </button>
          {project && project.envs.length > 0 && (
            <div className="seg envs" role="radiogroup" aria-label="Environment">
              {project.envs.map((e) => (
                <button key={e} role="radio" aria-checked={e === env} className={e === env ? "active" : ""} onClick={() => setEnv(e)}>
                  {e}
                </button>
              ))}
            </div>
          )}
          {project?.has_config && (
            <button className={"config-link" + (isConfig ? " active" : "")} onClick={() => select(CONFIG)} title="Environment variables">
              <span>env.toml</span>
              <span className="muted">variables</span>
            </button>
          )}
        </div>

        {project && (
          <>
            <div className="tree-head">
              <span>Requests</span>
              {project.has_config && (
                <button className="icon" onClick={() => syncRoutes()} title="Sync routes from Go code">
                  <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                    <path d="M13.5 6.5A5.5 5.5 0 0 0 3.2 4.8M2.5 9.5a5.5 5.5 0 0 0 10.3 1.7" />
                    <path d="M3 2v3h3M13 14v-3h-3" />
                  </svg>
                </button>
              )}
              <button className="icon" onClick={() => setPathForm(pathForm ? null : { value: "" })} title="New request">
                +
              </button>
            </div>
            {pathForm && (
              <form className="stack path-form" onSubmit={(e) => (e.preventDefault(), submitPath())}>
                <label className="muted">
                  {pathForm.from ? `Rename or move ${pathForm.from.isFile ? "request" : "folder"}` : "New request"}
                </label>
                <input
                  placeholder="users/create"
                  value={pathForm.value}
                  onChange={(e) => setPathForm({ ...pathForm, value: e.target.value })}
                  onKeyDown={(e) => e.key === "Escape" && setPathForm(null)}
                  autoFocus
                />
              </form>
            )}
            <div className="tree-wrap" onContextMenu={(e) => openMenu(e, null)}>
              <FileTree files={project.files} methods={project.methods} selected={selected} onSelect={select} onMenu={openMenu} />
            </div>
          </>
        )}

        <UpdateBanner updates={updates} />
        {project && (
          <div className="side-foot">
            {secretForm && (
              <form className="stack" onSubmit={(e) => (e.preventDefault(), saveSecret())}>
                <input placeholder="name" value={secretForm.name} onChange={(e) => setSecretForm({ ...secretForm, name: e.target.value })} autoFocus />
                <input placeholder="value" type="password" value={secretForm.value} onChange={(e) => setSecretForm({ ...secretForm, value: e.target.value })} />
                <button type="submit">Save for {env ?? "default"}</button>
              </form>
            )}
            <button
              className="ghost"
              onClick={() => setSecretForm(secretForm ? null : { name: "", value: "" })}
              title="Secrets are kept in the system keychain"
            >
              {secretForm ? "Cancel" : "Add secret"}
            </button>
          </div>
        )}
      </aside>

      <main className="main">
        {error && (
          <div className="error-bar" onClick={() => setError(null)}>
            {error}
          </div>
        )}

        {!project ? (
          <div className="panel empty">
            <h1>Open a project</h1>
            <p>
              Routy will find <code>api/env.toml</code> and every <code>*.routy</code> and <code>*.http</code> file in the
              folder.
            </p>
            <button className="primary" onClick={pickFolder}>Choose folder</button>
          </div>
        ) : (
          <>
            {!project.has_config && (
              <div className="init-bar">
                <span>
                  <code>{project.root}</code> has no <code>env.toml</code> — without it there are no environments or variables like{" "}
                  <code>{"{{base}}"}</code>.
                </span>
                <button className="primary" onClick={initProject}>
                  Create api/env.toml
                </button>
              </div>
            )}
            <div className="layout">
              <section className="panel editor">
                {selected ? (
                  <>
                    <div className="pane-head">
                      {isConfig ? (
                        <span className="method">ENV</span>
                      ) : (
                        <span className={"method m-" + method.toLowerCase()}>{method}</span>
                      )}
                      <span className="pane-title ellipsis" title={selected}>
                        <span className="muted">{dir}</span>
                        {name}
                      </span>
                      {dirty && <span className="dirty" title="Unsaved" />}
                      <span className="spacer" />
                      {isConfig ? (
                        <button className="primary" onClick={save} disabled={!dirty || !!parseError} title="Ctrl+S">
                          Save
                        </button>
                      ) : (
                        <>
                          <button onClick={save} disabled={!dirty} title="Ctrl+S">Save</button>
                          {run?.pending ? (
                            <button onClick={cancel} title="Cancel the request">Cancel</button>
                          ) : (
                            <button
                              className="primary"
                              onClick={() => send()}
                              disabled={!!parseError}
                              title={routy ? "Run the request or flow at the cursor (Ctrl+Enter)" : "Ctrl+Enter"}
                            >
                              {routy ? "Run" : "Send"}
                            </button>
                          )}
                        </>
                      )}
                    </div>
                    <CodeEditor
                      docKey={selected}
                      value={content}
                      onChange={setContent}
                      language={isConfig ? "toml" : routy ? "routy" : "http"}
                      errors={parseErrors}
                      vars={vars}
                      symbols={symbols}
                      onCursor={setCursor}
                      onRun={(line) => send(line)}
                    />
                    {parseError && (
                      <div className="parse-error">
                        {parseError.line !== null && `line ${parseError.line}: `}
                        {parseError.message}
                        {parseErrors.length > 1 && <span className="muted"> (+{parseErrors.length - 1} more)</span>}
                      </div>
                    )}
                  </>
                ) : (
                  <>
                    <div className="pane-head" />
                    <p className="hint">Select a request on the left</p>
                  </>
                )}
              </section>

              <section className="panel result">
                <div className="pane-head">
                  <div className="seg views">
                    <button className={view === "response" ? "active" : ""} onClick={() => setView("response")}>
                      {isConfig ? "Reference" : "Response"}
                      {run?.pending !== undefined && <span className="spinner" aria-label="Sending" />}
                    </button>
                    <button className={view === "history" ? "active" : ""} onClick={() => setView("history")}>
                      History
                    </button>
                    <button className={view === "vars" ? "active" : ""} onClick={() => setView("vars")}>
                      Variables
                    </button>
                    {project.has_config && (
                      <button className={view === "routes" ? "active" : ""} onClick={() => (routes ? setView("routes") : syncRoutes())}>
                        Routes
                      </button>
                    )}
                  </div>
                  <span className="spacer" />
                  {view === "response" && opened && (
                    <span className="from-history">
                      <span className="muted">from history</span>
                      <button className="ghost" onClick={() => setOpened(null)}>Latest</button>
                    </span>
                  )}
                </div>
                {view === "history" ? (
                  <HistoryList
                    key={selected ?? ""}
                    entries={history}
                    file={selected && !isConfig ? selected : null}
                    openedId={shown?.id ?? null}
                    persistent={persistHistory}
                    onOpen={(e) => {
                      setOpened(e);
                      setView("response");
                    }}
                    onClear={clearHistory}
                  />
                ) : view === "routes" ? (
                  <RoutesPanel
                    report={routes}
                    busy={routesBusy}
                    onScan={() => syncRoutes()}
                    onPickDir={pickRoutesDir}
                    onCreate={() => syncRoutes(true)}
                    onOpen={select}
                  />
                ) : view === "vars" ? (
                  <VarsPanel
                    env={env}
                    vars={varList}
                    onClearSaved={clearSaved}
                    onResetRun={resetRun}
                    onSetSecret={(name) => setSecretForm({ name, value: "" })}
                  />
                ) : isConfig ? (
                  <div className="tab-body config-help">
                    <p>
                      Shared variables go in <code>[vars]</code>, per-environment values in <code>[env.name]</code>;
                      they override the shared ones. <code>default</code> is the default environment.
                    </p>
                    <pre>{CONFIG_EXAMPLE}</pre>
                    <p>
                      Don't put tokens or passwords here — they belong in the system keychain: "Add secret" at the bottom
                      left, or a <code>ROUTY_NAME</code> environment variable. List their names in <code>secrets</code> and
                      the Variables tab will show which ones are missing.
                    </p>
                  </div>
                ) : (
                  <>
                    {run?.error && !opened && <div className="send-error">{run.error}</div>}
                    {run?.flow && !opened && !run.error ? (
                      <FlowView name={run.name ?? "flow"} flow={run.flow} />
                    ) : shown && (opened || !run?.error) ? (
                      <ResponseView key={shown.id} entry={shown} onError={setError} />
                    ) : (
                      !run?.error && (
                        <p className="hint">
                          {!selected ? (
                            "Responses appear here"
                          ) : run?.pending ? (
                            "Sending…"
                          ) : (
                            <>The response will appear here — <kbd>Ctrl</kbd> <kbd>Enter</kbd></>
                          )}
                        </p>
                      )
                    )}
                  </>
                )}
              </section>
            </div>
          </>
        )}
      </main>
    </div>
  );
}

