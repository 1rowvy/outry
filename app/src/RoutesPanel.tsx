// Синхронизация с кодом: роуты из Go (как `routy import go`) против файлов проекта.
import type { ImportReport, Route } from "./api";

interface Props {
  report: ImportReport | null;
  busy: boolean;
  onScan: () => void;
  onPickDir: () => void;
  onCreate: () => void;
  onOpen: (file: string) => void;
}

const slash = (p: string) => p.replace(/\\/g, "/");

function RouteLine({ route }: { route: Route }) {
  const method = route.method === "ANY" ? "ANY" : route.method;
  return (
    <>
      <span className={"method m-" + method.toLowerCase()}>{method}</span>
      <span className="r-path ellipsis">{route.path}</span>
      <span className="h-meta" title={route.handler ?? undefined}>
        {slash(route.source)}:{route.line}
      </span>
    </>
  );
}

export function RoutesPanel({ report, busy, onScan, onPickDir, onCreate, onOpen }: Props) {
  const plan = report?.plan;
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
          Create {plan?.new.length || ""} files
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
              <li key={f.file} title={f.content}>
                <div className="r-row">
                  <span className="r-mark new">+</span>
                  <RouteLine route={f.route} />
                  <span className="h-file r-file ellipsis">{slash(f.file)}</span>
                </div>
              </li>
            ))}
          </ul>
          {plan.existing.length > 0 && <h3>Already have a file</h3>}
          <ul className="history-list">
            {plan.existing.map((e) => (
              <li key={e.route.method + e.route.path}>
                <button onClick={() => onOpen(slash(e.file))}>
                  <span className="r-mark">=</span>
                  <RouteLine route={e.route} />
                  <span className="h-file r-file ellipsis">{slash(e.file)}</span>
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
                  <span className={"method m-" + s.method.toLowerCase()}>{s.method}</span>
                  <span className="r-path ellipsis">{s.url}</span>
                  <span className="h-file r-file ellipsis">{slash(s.file)}</span>
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
