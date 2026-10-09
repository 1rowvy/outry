// Итог сценария *.routy: проверки, где остановился, что сохранено, вызванные запросы.
import type { FlowOutcome } from "./types";
import { Checks } from "./ResponseView";
import { Trace } from "./Trace";

export function FlowView({ name, flow }: { name: string; flow: FlowOutcome }) {
  const failed = flow.checks.filter((c) => !c.passed).length;
  const passed = flow.error === null && failed === 0;
  return (
    <div className="response">
      <div className="response-head">
        <span className={"status " + (passed ? "ok" : "bad")}>
          <b>{passed ? "✓" : "✗"}</b> flow {name}
        </span>
        <span className="metric">{flow.calls.length} calls</span>
        {flow.checks.length > 0 && (
          <span className="metric">
            {flow.checks.length - failed}/{flow.checks.length} checks
          </span>
        )}
        {Object.keys(flow.saved).length > 0 && (
          <span className="saved">
            saved {Object.keys(flow.saved).map((k) => <code key={k}>{k}</code>)}
          </span>
        )}
      </div>
      <div className="tab-body">
        {flow.error && <div className="flow-error">✗ {flow.error}</div>}
        {flow.checks.length > 0 && <Checks asserts={flow.checks} misses={[]} />}
        <h3 className="flow-h">Trace</h3>
        <Trace calls={flow.calls} />
      </div>
    </div>
  );
}
