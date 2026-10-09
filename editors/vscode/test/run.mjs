// Интеграционный тест: настоящий VS Code с расширением, `routy lsp` из target/debug и
// локальный HTTP-сервер. VS Code: $VSCODE_PATH (локально — /usr/share/code/code) или скачанный.
//   cargo build -p routy-cli && npm run build && npm test
import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { runTests } from "@vscode/test-electron";
import * as esbuild from "esbuild";

const here = path.dirname(new URL(import.meta.url).pathname);
const ext = path.resolve(here, "..");
const exe = process.platform === "win32" ? "routy.exe" : "routy";
const routy = process.env.ROUTY_BIN ?? path.resolve(ext, "../../target/debug", exe);

await esbuild.build({
  entryPoints: [path.join(here, "suite.ts")],
  bundle: true,
  outfile: path.join(ext, "out/test/suite.js"),
  external: ["vscode"],
  format: "cjs",
  platform: "node",
  target: "node20",
});

// Эхо: метод, путь, заголовки и тело запроса; `/login` — токен.
const hits = [];
const server = createServer((req, res) => {
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    hits.push(`${req.method} ${req.url}`);
    const json = req.url === "/login" ? { token: "tok-1" } : { method: req.method, path: req.url, headers: req.headers, body };
    res.writeHead(req.url === "/login" ? 200 : 201, { "Content-Type": "application/json" });
    res.end(JSON.stringify(json));
  });
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const base = `http://127.0.0.1:${server.address().port}`;

const ws = mkdtempSync(path.join(tmpdir(), "routy-vscode-"));
const write = (rel, text) => {
  mkdirSync(path.dirname(path.join(ws, rel)), { recursive: true });
  writeFileSync(path.join(ws, rel), text);
};
write(".vscode/settings.json", JSON.stringify({ "routy.path": routy, "routy.keyring": false }));
write("api/env.toml", `default = "dev"\nsecrets = ["key"]\n\n[env.dev]\nbase = "${base}"\nkey = "0123456789abcdef"\n\n[env.staging]\nbase = "${base}"\n`);
write("api/auth/login.routy", "// Login\nPOST /login {\n  expect { status == 200 }\n}\n");
write(
  "api/orders.routy",
  '// Create order\nPOST /orders {\n  headers { Authorization: "Bearer ${Login().body.token}" }\n  body { sku: "A-1" }\n  expect { status == 201 }\n}\n',
);

let code = 0;
try {
  await runTests({
    vscodeExecutablePath: process.env.VSCODE_PATH || undefined,
    extensionDevelopmentPath: ext,
    extensionTestsPath: path.join(ext, "out/test/suite.js"),
    launchArgs: [ws, "--disable-extensions", "--skip-welcome", "--skip-release-notes", "--disable-workspace-trust"],
    extensionTestsEnv: { ROUTY_TEST_WS: ws },
  });
  if (!hits.includes("POST /orders")) throw new Error(`the request was not sent: ${hits.join(", ")}`);
} catch (e) {
  console.error(e);
  code = 1;
} finally {
  server.close();
}
process.exit(code);
