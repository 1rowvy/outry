// Где взять `outry`: настройка `outry.path` → бинарь из платформенного VSIX → PATH и места,
// куда его ставит install.sh (у VS Code, запущенного из меню, PATH бывает короче, чем в терминале).
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";

const EXE = process.platform === "win32" ? "outry.exe" : "outry";

export function findBinary(extensionPath: string): string | null {
  const configured = vscode.workspace.getConfiguration("outry").get<string>("path")?.trim();
  if (configured) {
    return configured.replace(/^~(?=$|[\\/])/, homedir());
  }
  const bundled = path.join(extensionPath, "bin", EXE);
  if (existsSync(bundled)) {
    return bundled;
  }
  const dirs = (process.env.PATH ?? "").split(path.delimiter).filter(Boolean);
  dirs.push(path.join(homedir(), ".local", "bin"), path.join(homedir(), ".cargo", "bin"), "/usr/local/bin", "/opt/homebrew/bin");
  for (const dir of dirs) {
    const candidate = path.join(dir, EXE);
    if (existsSync(candidate)) {
      return candidate;
    }
  }
  return null;
}
