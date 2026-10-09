// Webview ответа: компоненты из app/src (ResponseView, FlowView, BodyViewer) — один файл
// dist/webview/webview.js + webview.css. React и CodeMirror — из node_modules расширения
// (dedupe), чтобы app/node_modules для сборки не требовался.
import path from "node:path";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const here = path.dirname(new URL(import.meta.url).pathname);

export default defineConfig({
  root: here,
  plugins: [react()],
  resolve: {
    alias: { "@app": path.resolve(here, "../../../app/src") },
    dedupe: [
      "react",
      "react-dom",
      "@codemirror/commands",
      "@codemirror/language",
      "@codemirror/legacy-modes",
      "@codemirror/search",
      "@codemirror/state",
      "@codemirror/view",
      "@lezer/highlight",
    ],
  },
  build: {
    outDir: path.resolve(here, "../dist/webview"),
    emptyOutDir: true,
    cssCodeSplit: false,
    // React + CodeMirror одним файлом — ~600 КБ, грузится локально.
    chunkSizeWarningLimit: 1000,
    rollupOptions: {
      input: path.resolve(here, "main.tsx"),
      output: {
        format: "iife",
        entryFileNames: "webview.js",
        assetFileNames: "webview[extname]",
      },
    },
  },
});
