import type { Updates } from "./updater";

export function UpdateBanner({ updates }: { updates: Updates }) {
  const s = updates.state;
  if (s.kind === "available") {
    return (
      <div className="update-banner">
        <span>Version {s.update.version} is available</span>
        <button onClick={updates.install}>Update and restart</button>
      </div>
    );
  }
  if (s.kind === "downloading") {
    return (
      <div className="update-banner">
        <span>Version {s.update.version}</span>
        <span className="muted">{s.percent === null ? "Downloading…" : `Downloading ${s.percent}%`}</span>
      </div>
    );
  }
  if (s.kind === "ready") {
    return (
      <div className="update-banner">
        <span>Version {s.update.version} is installed</span>
        <button onClick={updates.restart}>Restart now</button>
      </div>
    );
  }
  if (s.kind === "error" && s.update) {
    return (
      <div className="update-banner">
        <span>Version {s.update.version} is available</span>
        <button onClick={updates.install}>Retry</button>
        <span className="bad">{s.message}</span>
      </div>
    );
  }
  return null;
}
