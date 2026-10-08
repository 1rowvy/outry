// Синхронизация с кодом: роуты из Go (как `routy import go`) против файлов проекта.
import { useState } from "react";
import type { Field, ImportReport, Route } from "./api";

interface Props {
  report: ImportReport | null;
  busy: boolean;
  onScan: () => void;
  onPickDir: () => void;
  onCreate: () => void;
  onOpen: (file: string) => void;
}

const slash = (p: string) => p.replace(/\\/g, "/");

const names = (fields: Field[]) => fields.map((f) => f.name + (f.required ? "*" : "")).join(", ");

/** Что передавать: тело, query, заголовки — одной строкой. */
function needs(route: Route): string[] {
  const { info } = route;
  const out = [];
  if (info.body) out.push(`body ${info.body.type_name}: ${names(info.body.fields)}`);
  if (info.query.length) out.push(`query: ${names(info.query)}`);
  if (info.headers.length) out.push(`headers: ${info.headers.join(", ")}`);
  return out;
}

/** Строка роута: метод и путь, описание, что передавать, файл запроса и место в Go-коде. */
function RouteLine({ method, path, file, route }: { method: string; path: string; file: string; route?: Route }) {
  const summary = route?.info.summary;
  const params = route ? needs(route) : [];
  return (
    <>
      <span className={"method m-" + method.toLowerCase()}>{method}</span>
      <span className="r-path">{path}</span>
      {summary && <span className="r-summary">{summary}</span>}
      {params.length > 0 && (
        <span className="r-needs" title="* — required">
          {params.join(" · ")}
        </span>
      )}
      <span className="r-sub ellipsis">
        <span className="r-file">{slash(file)}</span>
        {route && (
          <span title={route.handler ?? undefined}>
            {"  ←  "}
            {slash(route.source)}:{route.line}
            {route.handler && ` ${route.handler}`}
          </span>
        )}
      </span>
    </>
  );
}

export function RoutesPanel({ report, busy, onScan, onPickDir, onCreate, onOpen }: Props) {
  const plan = report?.plan;
  // Раскрытые новые роуты: показываем будущий файл целиком.
  const [open, setOpen] = useState<Set<string>>(new Set());
  const toggle = (file: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (!next.delete(file)) next.add(file);
      return next;
    });
  return (
    <div className="vars routes">
      <div className="history-bar">
        <button className="ghost r-dir ellipsis" onClick={onPickDir} title="Folder with the service's Go code">
          {report ? report.dir : "Go source folder…"}
        </button>
        <span className="spacer" />
        <button className="ghost" onClick={onScan} disabled={busy}>
          {busy ? "Scanning…" : "Rescan"}
        </button>
        <button className="primary" onClick={onCreate} disabled={busy || !plan || plan.new.length === 0}>
          {plan && plan.new.length > 0 ? `Create ${plan.new.length} file${plan.new.length === 1 ? "" : "s"}` : "Create files"}
        </button>
      </div>
      {!plan ? (
        <p className="hint">{busy ? "Scanning…" : "Finds chi, gin and net/http routes in Go code"}</p>
      ) : (
        <div className="routes-body">
          <p className="routes-summary muted">
            {plan.files} Go files · {plan.new.length + plan.existing.length} routes · {plan.new.length} new ·{" "}
            {plan.existing.length} existing · {plan.stale.length} not in code
          </p>
          {plan.warnings.map((w) => (
            <p key={w} className="routes-warning">
              {w}
            </p>
          ))}
          {plan.new.length > 0 && <h3>New</h3>}
          <ul className="history-list">
            {plan.new.map((f) => (
              <li key={f.file}>
                <button onClick={() => toggle(f.file)} aria-expanded={open.has(f.file)} title="Show the file that will be created">
                  <span className="r-mark new">+</span>
                  <RouteLine method={f.route.method} path={f.route.path} file={f.file} route={f.route} />
                </button>
                {open.has(f.file) && <pre className="r-preview">{f.content}</pre>}
              </li>
            ))}
          </ul>
          {plan.existing.length > 0 && <h3>Already have a file</h3>}
          <ul className="history-list">
            {plan.existing.map((e) => (
              <li key={e.route.method + e.route.path}>
                <button onClick={() => onOpen(slash(e.file))}>
                  <span className="r-mark">=</span>
                  <RouteLine method={e.route.method} path={e.route.path} file={e.file} route={e.route} />
                </button>
              </li>
            ))}
          </ul>
          {plan.stale.length > 0 && <h3>Not in code</h3>}
          <ul className="history-list">
            {plan.stale.map((s) => (
              <li key={s.file}>
                <button onClick={() => onOpen(slash(s.file))} title="No route with this method and path was found">
                  <span className="r-mark stale">−</span>
                  <RouteLine method={s.method} path={s.url} file={s.file} />
                </button>
              </li>
            ))}
          </ul>
          <p className="vars-note">
            Existing files are never changed. A file matches a route by method and the path after{" "}
            <code>{"{{base}}"}</code>; path parameters match any variable. CLI: <code>routy import go ./</code>
          </p>
        </div>
      )}
    </div>
  );
}
