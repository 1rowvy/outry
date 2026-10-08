// Синхронизация с кодом: роуты из Go (как `routy import go`) против файлов проекта.
import { useState } from "react";
import type { Field, ImportReport, Route, RouteChange } from "./api";

interface Props {
  report: ImportReport | null;
  busy: boolean;
  onScan: () => void;
  onPickDir: () => void;
  onCreate: () => void;
  onOpen: (file: string) => void;
  /** Исправить выбранные расхождения (`null` — все исправимые) */
  onFix: (ids: string[] | null) => void;
  /** Удалить файлы без роутов в коде */
  onPrune: () => void;
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
  if (info.response) out.push(`response: ${info.response.shape}`);
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

/** `/users/{{id}}` (так ядро хранит параметры) → `/users/{id}`, как в роутере и `.routy`. */
const routePath = (p: string) => p.replaceAll("{{", "{").replaceAll("}}", "}");

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** Расхождения с кодом: место, текст, где в Go; у исправимых — diff и «Apply». */
function Changes({ changes, busy, onFix }: { changes: RouteChange[]; busy: boolean; onFix: Props["onFix"] }) {
  const [shown, setShown] = useState<Set<string>>(new Set());
  const toggle = (id: string) =>
    setShown((prev) => {
      const next = new Set(prev);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  return (
    <ul className="r-changes">
      {changes.map((c) => (
        <li key={c.id}>
          <div className="r-change">
            <span className={"r-sev " + c.severity} title={c.severity} />
            <span className="r-msg">
              <span className="r-at">
                {slash(c.file)}:{c.line}
              </span>{" "}
              {c.message}{" "}
              <span className="r-at" title="Where it is in the Go code">
                ← {slash(c.go.file)}:{c.go.line}
              </span>
            </span>
            {c.fixable && (
              <span className="r-actions">
                <button className="link" onClick={() => toggle(c.id)} aria-expanded={shown.has(c.id)}>
                  Diff
                </button>
                <button className="link" onClick={() => onFix([c.id])} disabled={busy}>
                  Apply
                </button>
              </span>
            )}
          </div>
          {shown.has(c.id) && c.diff && <Diff text={c.diff} />}
        </li>
      ))}
    </ul>
  );
}

function Diff({ text }: { text: string }) {
  return (
    <pre className="r-preview r-diff">
      {text
        .split("\n")
        .filter((l) => !l.startsWith("---") && !l.startsWith("+++"))
        .map((l, i) => (
          <div key={i} className={l.startsWith("+") ? "add" : l.startsWith("-") ? "del" : l.startsWith("@@") ? "hunk" : ""}>
            {l || " "}
          </div>
        ))}
    </pre>
  );
}

export function RoutesPanel({ report, busy, onScan, onPickDir, onCreate, onOpen, onFix, onPrune }: Props) {
  const plan = report?.plan;
  // Раскрытые новые роуты: показываем будущий файл целиком.
  const [open, setOpen] = useState<Set<string>>(new Set());
  const toggle = (file: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (!next.delete(file)) next.add(file);
      return next;
    });
  const changes = plan ? [...plan.existing.flatMap((e) => e.changes), ...plan.shape_changes] : [];
  const fixable = changes.filter((c) => c.fixable).length;
  const toCreate = plan ? plan.new.length + (plan.new_shapes.length > 0 ? 1 : 0) : 0;
  const changed = plan ? plan.existing.filter((e) => e.changes.length > 0).length : 0;
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
        {fixable > 0 && (
          <button className="ghost" onClick={() => onFix(null)} disabled={busy} title="routy import go --fix">
            Fix {fixable}
          </button>
        )}
        <button
          className="primary"
          onClick={onCreate}
          disabled={busy || toCreate === 0}
          title={plan && plan.new_shapes.length > 0 ? `New shapes go to ${plan.shapes_file}` : undefined}
        >
          {toCreate > 0 ? `Create ${plural(toCreate, "file")}` : "Create files"}
        </button>
      </div>
      {!plan ? (
        <p className="hint">{busy ? "Scanning…" : "Finds chi, gin and net/http routes in Go code"}</p>
      ) : (
        <div className="routes-body">
          <p className="routes-summary muted">
            {plan.files} Go files · {plan.new.length + plan.existing.length} routes · {plan.new.length} new ·{" "}
            {plan.existing.length} existing ({changed} changed) · {plan.stale.length} not in code
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
                  <RouteLine method={f.route.method} path={routePath(f.route.path)} file={f.file} route={f.route} />
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
                  <span className={"r-mark" + (e.changes.length ? " changed" : "")}>{e.changes.length ? "~" : "="}</span>
                  <RouteLine method={e.route.method} path={routePath(e.route.path)} file={e.file} route={e.route} />
                </button>
                {e.changes.length > 0 && (
                  <>
                    <span className="badge r-badge" title="The request differs from the code">
                      changed · {e.changes.length}
                    </span>
                    <Changes changes={e.changes} busy={busy} onFix={onFix} />
                  </>
                )}
              </li>
            ))}
          </ul>
          {(plan.new_shapes.length > 0 || plan.shape_changes.length > 0) && <h3>Response shapes</h3>}
          <ul className="history-list">
            {plan.new_shapes.map((d) => (
              <li key={d.name}>
                <button onClick={() => toggle("shape:" + d.name)} aria-expanded={open.has("shape:" + d.name)}>
                  <span className="r-mark new">+</span>
                  <span className="r-path r-shape">shape {d.name}</span>
                  <span className="r-sub ellipsis">
                    <span className="r-file">{slash(plan.shapes_file)}</span>
                    {"  ←  "}
                    {slash(d.source)}:{d.line} {d.go_type}
                  </span>
                </button>
                {open.has("shape:" + d.name) && (
                  <pre className="r-preview">
                    shape {d.name} {d.shape}
                  </pre>
                )}
              </li>
            ))}
          </ul>
          {plan.shape_changes.length > 0 && <Changes changes={plan.shape_changes} busy={busy} onFix={onFix} />}
          {plan.stale.length > 0 && (
            <h3 className="r-stale-head">
              Not in code
              {plan.prunable.length > 0 && (
                <button
                  className="link"
                  onClick={onPrune}
                  disabled={busy}
                  title={`Delete ${plan.prunable.map(slash).join(", ")}`}
                >
                  Remove {plural(plan.prunable.length, "file")}
                </button>
              )}
            </h3>
          )}
          <ul className="history-list">
            {plan.stale.map((s) => (
              <li key={s.file + s.method + s.url}>
                <button onClick={() => onOpen(slash(s.file))} title="No route with this method and path was found">
                  <span className="r-mark stale">−</span>
                  <RouteLine method={s.method} path={s.url} file={s.file} />
                </button>
              </li>
            ))}
          </ul>
          <p className="vars-note">
            A request matches a route by its <code>handler:</code>, or by method and path (after{" "}
            <code>{"{{base}}"}</code> in <code>.http</code>); path parameters match any name. Matched requests are
            compared with the handler; <b>Apply</b> changes only that spot of the file. CLI:{" "}
            <code>routy import go --check</code>, <code>--fix</code>
          </p>
        </div>
      )}
    </div>
  );
}
