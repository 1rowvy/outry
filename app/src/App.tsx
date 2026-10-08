import { useCallback, useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { api, type ParseError, type ProjectInfo, type RunOutcome } from "./api";
import { FileTree } from "./FileTree";
import { ResponseView } from "./ResponseView";
import { UpdateBanner } from "./UpdateBanner";

const LAST_PROJECT_KEY = "routy.lastProject";
const NEW_REQUEST = "GET {{base}}/\n\n> assert status == 200\n";

// Метод из строки запроса — только для метки в шапке; разбирает ядро.
function requestMethod(text: string): string {
  for (const line of text.split("\n")) {
    const t = line.trim();
    if (!t || t.startsWith("#") || t.startsWith("//")) continue;
    return /^([A-Z]+)\s/.exec(t)?.[1] ?? "GET";
  }
  return "GET";
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
  const [parseError, setParseError] = useState<ParseError | null>(null);
  const [outcome, setOutcome] = useState<RunOutcome | null>(null);
  const [sendError, setSendError] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [newPath, setNewPath] = useState<string | null>(null);
  const [secretForm, setSecretForm] = useState<{ name: string; value: string } | null>(null);
  const [version, setVersion] = useState<string | null>(null);

  const dirty = content !== savedContent;
  const method = requestMethod(content);
  const cut = (selected ?? "").lastIndexOf("/") + 1;
  const dir = (selected ?? "").slice(0, cut);
  const name = (selected ?? "").slice(cut).replace(/\.http$/, "");
  // Для обработчика событий файловой системы нужны актуальные значения без переподписки.
  const live = useRef({ selected, dirty });
  live.current = { selected, dirty };

  const applyProject = useCallback((info: ProjectInfo) => {
    setProject(info);
    setEnv(info.default_env);
    setSelected(null);
    setContent("");
    setSavedContent("");
    setOutcome(null);
    setError(null);
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
    if (dirty && !window.confirm("Есть несохранённые изменения. Продолжить?")) return;
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

  // Проверка синтаксиса на лету.
  useEffect(() => {
    if (!selected) return;
    const t = window.setTimeout(() => api.checkRequest(content).then(setParseError), 150);
    return () => clearTimeout(t);
  }, [content, selected]);

  const select = async (path: string) => {
    if (dirty && !window.confirm("Есть несохранённые изменения. Открыть другой файл?")) return;
    try {
      const text = await api.readRequest(path);
      setSelected(path);
      setContent(text);
      setSavedContent(text);
      setOutcome(null);
      setSendError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  const save = useCallback(async () => {
    if (!selected) return;
    try {
      await api.writeRequest(selected, content);
      setSavedContent(content);
    } catch (e) {
      setError(String(e));
    }
  }, [selected, content]);

  const send = useCallback(async () => {
    if (!selected || sending) return;
    setSending(true);
    setSendError(null);
    try {
      setOutcome(await api.sendRequest(env, content));
    } catch (e) {
      setOutcome(null);
      setSendError(String(e));
    } finally {
      setSending(false);
    }
  }, [selected, sending, env, content]);

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
    const dir = await open({ directory: true, title: "Каталог проекта (с api/env.toml)" });
    if (typeof dir === "string") openProject(dir);
  };

  const createRequest = async () => {
    if (!newPath) return;
    const path = newPath.trim().replace(/^\/+/, "").replace(/(\.http)?$/, ".http");
    try {
      await api.writeRequest(path, NEW_REQUEST);
      setNewPath(null);
      setProject(await api.refreshProject());
      await select(path);
    } catch (e) {
      setError(String(e));
    }
  };

  const saveSecret = async () => {
    if (!secretForm?.name || !secretForm.value) return;
    try {
      await api.setSecret(env, secretForm.name.trim(), secretForm.value);
      setSecretForm(null);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <span>routy</span>
          {version && <span className="muted">{version}</span>}
        </div>

        <div className="project">
          <button className="project-name" onClick={pickFolder} title={project ? `${project.root}\nОткрыть другой проект` : undefined}>
            <span className="ellipsis">{project ? project.id : "Открыть проект…"}</span>
            {project && <span className="muted">сменить</span>}
          </button>
          {project && project.envs.length > 0 && (
            <div className="seg envs" role="radiogroup" aria-label="Окружение">
              {project.envs.map((e) => (
                <button key={e} role="radio" aria-checked={e === env} className={e === env ? "active" : ""} onClick={() => setEnv(e)}>
                  {e}
                </button>
              ))}
            </div>
          )}
        </div>

        {project && (
          <>
            <div className="tree-head">
              <span>Запросы</span>
              <button className="icon" onClick={() => setNewPath(newPath === null ? "" : null)} title="Новый запрос">
                +
              </button>
            </div>
            {newPath !== null && (
              <form className="stack" onSubmit={(e) => (e.preventDefault(), createRequest())}>
                <input placeholder="users/create" value={newPath} onChange={(e) => setNewPath(e.target.value)} autoFocus />
              </form>
            )}
            <div className="tree-wrap">
              <FileTree files={project.files} selected={selected} onSelect={select} />
            </div>
          </>
        )}

        <UpdateBanner />
        {project && (
          <div className="side-foot">
            {secretForm && (
              <form className="stack" onSubmit={(e) => (e.preventDefault(), saveSecret())}>
                <input placeholder="имя" value={secretForm.name} onChange={(e) => setSecretForm({ ...secretForm, name: e.target.value })} autoFocus />
                <input placeholder="значение" type="password" value={secretForm.value} onChange={(e) => setSecretForm({ ...secretForm, value: e.target.value })} />
                <button type="submit">Сохранить для {env ?? "default"}</button>
              </form>
            )}
            <button
              className="ghost"
              onClick={() => setSecretForm(secretForm ? null : { name: "", value: "" })}
              title="Секреты хранятся в системном хранилище паролей"
            >
              {secretForm ? "Отмена" : "Добавить секрет"}
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
            <h1>Откройте проект</h1>
            <p>
              Routy найдёт <code>api/env.toml</code> и все <code>*.http</code> файлы в каталоге.
            </p>
            <button className="primary" onClick={pickFolder}>Выбрать каталог</button>
          </div>
        ) : (
          <>
            {!project.has_config && (
              <div className="init-bar">
                <span>
                  В <code>{project.root}</code> нет <code>env.toml</code> — без него нет окружений и переменных вроде{" "}
                  <code>{"{{base}}"}</code>.
                </span>
                <button className="primary" onClick={initProject}>
                  Создать api/env.toml
                </button>
              </div>
            )}
            <div className="layout">
              <section className="panel editor">
                {selected ? (
                  <>
                    <div className="pane-head">
                      <span className={"method m-" + method.toLowerCase()}>{method}</span>
                      <span className="pane-title ellipsis" title={selected}>
                        <span className="muted">{dir}</span>
                        {name}
                      </span>
                      {dirty && <span className="dirty" title="Не сохранено" />}
                      <span className="spacer" />
                      <button onClick={save} disabled={!dirty} title="Ctrl+S">Сохранить</button>
                      <button className="primary" onClick={send} disabled={sending || !!parseError} title="Ctrl+Enter">
                        {sending ? "Отправка…" : "Отправить"}
                      </button>
                    </div>
                    <textarea value={content} onChange={(e) => setContent(e.target.value)} spellCheck={false} />
                    {parseError && (
                      <div className="parse-error">
                        {parseError.line !== null && `строка ${parseError.line}: `}
                        {parseError.message}
                      </div>
                    )}
                  </>
                ) : (
                  <>
                    <div className="pane-head" />
                    <p className="hint">Выберите запрос слева</p>
                  </>
                )}
              </section>

              <section className="panel result">
                {!outcome && <div className="pane-head" />}
                {sendError && <div className="send-error">{sendError}</div>}
                {outcome ? <ResponseView outcome={outcome} /> : !sendError && (
                  <p className="hint">
                    {sending ? "Отправка…" : <>Ответ появится здесь — <kbd>Ctrl</kbd> <kbd>Enter</kbd></>}
                  </p>
                )}
              </section>
            </div>
          </>
        )}
      </main>
    </div>
  );
}

