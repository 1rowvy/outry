// Панель переменных: итоговое значение и откуда оно пришло (как `outry vars`).
import type { VarInfo, VarSource } from "./api";

const SOURCES: Record<VarSource, { label: string; title: string }> = {
  override: { label: "override", title: "Set manually" },
  saved: { label: "saved", title: "Captured by > save" },
  process_env: { label: "OUTRY_*", title: "Process environment variable" },
  env: { label: "env.toml", title: "[env.<name>] or [vars] in env.toml" },
  secret: { label: "keychain", title: "System keychain" },
  dynamic: { label: "dynamic", title: "Generated on every request" },
};

interface Props {
  env: string | null;
  vars: VarInfo[];
  onClearSaved: () => void;
  /** *.outry: новый прогон — забыть кеш вызовов и cookies */
  onResetRun: () => void;
  onSetSecret: (name: string) => void;
}

export function VarsPanel({ env, vars, onClearSaved, onResetRun, onSetSecret }: Props) {
  const saved = vars.filter((v) => v.source === "saved").length;
  return (
    <div className="vars">
      <div className="history-bar">
        <span className="muted">
          Environment <b>{env ?? "default"}</b>
        </span>
        <span className="spacer" />
        <button className="ghost" onClick={onResetRun} title="Forget responses of called requests (Login() and the like) and cookies — the next .outry run starts fresh">
          New run
        </button>
        <button className="ghost" onClick={onClearSaved} disabled={saved === 0} title="Forget values captured by save in this environment">
          Clear saved{saved > 0 && ` (${saved})`}
        </button>
      </div>
      {vars.length === 0 ? (
        <p className="hint">No variables. Add them to env.toml.</p>
      ) : (
        <table className="vars-table">
          <tbody>
            {vars.map((v) => (
              <tr key={v.name}>
                <td className="v-name">{v.name}</td>
                <td>
                  {v.source ? (
                    <span className={"badge src-" + v.source} title={SOURCES[v.source].title}>
                      {SOURCES[v.source].label}
                    </span>
                  ) : (
                    <span className="badge src-missing">missing</span>
                  )}
                </td>
                <td className={"v-value" + (v.secret ? " masked" : "")}>
                  {v.value ?? (
                    <button className="link" onClick={() => onSetSecret(v.name)}>
                      Set secret…
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <p className="vars-note">
        First match wins: override → saved → <code>OUTRY_NAME</code> → env.toml → keychain. Keychain secrets and{" "}
        <code>OUTRY_*</code> are only shown for names that appear in env.toml or its <code>secrets</code> list. Also
        available: <code>{"{{$uuid}}"}</code>, <code>{"{{$timestamp}}"}</code>, <code>{"{{$randomInt 1 100}}"}</code>.
      </p>
    </div>
  );
}
