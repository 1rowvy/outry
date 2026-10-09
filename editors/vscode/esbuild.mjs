// Сборка расширения (Node, CommonJS) в dist/extension.js; webview собирает vite (webview/vite.config.ts).
import { copyFileSync } from "node:fs";
import * as esbuild from "esbuild";

const watch = process.argv.includes("--watch");
const options = {
  entryPoints: ["src/extension.ts"],
  bundle: true,
  outfile: "dist/extension.js",
  external: ["vscode"],
  format: "cjs",
  platform: "node",
  target: "node20",
  sourcemap: watch,
  minify: !watch,
};

// vsce требует LICENSE рядом с package.json — тот же, что у репозитория.
copyFileSync("../../LICENSE", "LICENSE");

if (watch) {
  await (await esbuild.context(options)).watch();
} else {
  await esbuild.build(options);
}
