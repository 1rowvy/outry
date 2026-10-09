// Редактор на CodeMirror 6: подсветка .http/.routy/env.toml, ошибки на строке,
// автодополнение `{{var}}` и директив (.http), имён, вызовов и полей (.routy), ▶ у запросов .routy.
import { useEffect, useRef } from "react";
import { EditorState, Prec, type Extension } from "@codemirror/state";
import {
  EditorView,
  GutterMarker,
  drawSelection,
  gutter,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
  placeholder,
  tooltips,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { StreamLanguage, bracketMatching, syntaxHighlighting } from "@codemirror/language";
import { toml } from "@codemirror/legacy-modes/mode/toml";
import {
  autocompletion,
  closeBrackets,
  closeBracketsKeymap,
  completionKeymap,
  type Completion,
  type CompletionContext,
  type CompletionResult,
} from "@codemirror/autocomplete";
import { lintGutter, setDiagnostics, type Diagnostic } from "@codemirror/lint";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import type { ParseError, Symbols, VarName } from "./api";
import { highlight, theme } from "./editorTheme";
import { httpLanguage } from "./httpLanguage";
import { BUILTINS, FIELDS, KEYWORDS, TYPES, routyLanguage } from "./routyLanguage";

export type EditorLanguage = "http" | "toml" | "routy";

const PATHS: Completion[] = [
  { label: "status", type: "property", detail: "HTTP status" },
  { label: "duration", type: "property", detail: "ms" },
  { label: "body", type: "property", detail: "JSON body or text" },
  { label: "headers.", type: "property", detail: "response header" },
];
const OPERATORS = ["==", "!=", "<", "<=", ">", ">=", "contains", "exists"].map(
  (label): Completion => ({ label, type: "keyword" }),
);
const DIRECTIVES: Completion[] = [
  { label: "save", type: "keyword", apply: "save ", detail: "<name> = <path>" },
  { label: "assert", type: "keyword", apply: "assert ", detail: "<path> <op> <value>" },
];

function completions(vars: () => VarName[]) {
  return (ctx: CompletionContext): CompletionResult | null => {
    const v = ctx.matchBefore(/\{\{[\w$.-]*/);
    if (v) {
      const closed = ctx.state.sliceDoc(ctx.pos, ctx.pos + 2) === "}}";
      return {
        from: v.from + 2,
        options: vars().map((x) => ({
          label: x.name,
          type: "variable",
          detail: x.source === "saved" || x.source === "secret" ? x.source : x.value ?? undefined,
          apply: closed ? x.name : `${x.name}}}`,
        })),
        validFor: /^[\w$.-]*$/,
      };
    }
    const line = ctx.state.doc.lineAt(ctx.pos);
    const before = line.text.slice(0, ctx.pos - line.from);
    // `>` в начале строки → save/assert; дальше — пути и операторы assert.
    let m = /^>\s*(\w*)$/.exec(before);
    if (m) return { from: ctx.pos - m[1].length, options: DIRECTIVES, validFor: /^\w*$/ };
    m = /^>\s*assert\s+([\w.-]*)$/.exec(before);
    if (m) return { from: ctx.pos - m[1].length, options: PATHS, validFor: /^[\w.-]*$/ };
    m = /^>\s*assert\s+\S+\s+(\w*)$/.exec(before);
    if (m) return { from: ctx.pos - m[1].length, options: OPERATORS, validFor: /^\w*$/ };
    m = /^>\s*save\s+[\w-]+\s*=\s*([\w.-]*)$/.exec(before);
    if (m) return { from: ctx.pos - m[1].length, options: PATHS, validFor: /^[\w.-]*$/ };
    return null;
  };
}

const BUILTIN_DOCS: Record<string, string> = {
  uuid: "random UUID v4",
  now: "Unix time, seconds",
  nowIso: "current time, RFC 3339",
  randomInt: "(min, max) both included",
  randomString: "(n) letters and digits",
  number: "(x) to number",
  string: "(x) to string",
  json: "(x) as JSON text",
  base64: "(x) encode",
  unbase64: "(x) decode",
  file: '("./path") file contents',
  schema: '("./x.json") JSON Schema',
};
const MEMBERS: Completion[] = [
  ...["length", "first", "last", "keys", "values"].map((label): Completion => ({ label, type: "property" })),
  ...["startsWith", "endsWith", "contains", "lower", "upper", "trim", "split", "has", "any", "all", "map", "filter"].map(
    (label): Completion => ({ label, type: "method", apply: `${label}()` }),
  ),
];
const RESPONSE: Completion[] = [
  { label: "status", type: "variable", detail: "response" },
  { label: "headers", type: "variable", detail: "response" },
  { label: "body", type: "variable", detail: "response" },
  { label: "duration", type: "variable", detail: "response, ms" },
  { label: "cookies", type: "variable", detail: "run's cookie jar" },
  { label: "env", type: "variable", detail: "environment name" },
];

/** Автодополнение *.routy: поля в начале строки, вызовы запросов, функции, переменные, формы. */
function routyCompletions(vars: () => VarName[], symbols: () => Symbols | null) {
  return (ctx: CompletionContext): CompletionResult | null => {
    const word = ctx.matchBefore(/[\p{L}_][\p{L}\p{N}_-]*$/u) ?? (ctx.explicit ? { from: ctx.pos, to: ctx.pos, text: "" } : null);
    if (!word) return null;
    const before = ctx.state.sliceDoc(Math.max(0, word.from - 1), word.from);
    if (before === ".") return { from: word.from, options: MEMBERS, validFor: /^[\p{L}\p{N}_]*$/u };
    const line = ctx.state.doc.lineAt(ctx.pos);
    const lineStart = /^\s*$/.test(line.text.slice(0, word.from - line.from));
    const afterMatches = /matches\s+$/.test(line.text.slice(0, word.from - line.from));
    const options: Completion[] = [];
    const sym = symbols();
    if (afterMatches) {
      options.push(...TYPES.map((label): Completion => ({ label, type: "type" })));
      options.push(...(sym?.shapes ?? []).map((label): Completion => ({ label, type: "class", detail: "shape" })));
      return { from: word.from, options, validFor: /^[\p{L}\p{N}_]*$/u };
    }
    if (lineStart) {
      options.push(...FIELDS.map((label): Completion => ({ label, type: "keyword", detail: "field" })));
      options.push(...["expect", "save", "poll", "let", "shape", "flow"].map((label): Completion => ({ label, type: "keyword" })));
    } else {
      options.push(...KEYWORDS.filter((k) => ["fresh", "matches", "in", "typeof"].includes(k)).map((label): Completion => ({ label, type: "keyword" })));
      options.push({ label: "true", type: "constant" }, { label: "false", type: "constant" }, { label: "null", type: "constant" });
    }
    options.push(
      ...BUILTINS.map((label): Completion => ({ label, type: "function", detail: BUILTIN_DOCS[label], apply: `${label}()` })),
      ...(sym?.callables ?? []).map(
        (c): Completion => ({
          label: c.name,
          type: c.kind === "flow" ? "class" : "function",
          detail: c.params.length ? `(${c.params.join(", ")})` : c.doc ?? c.kind,
          info: c.doc ?? undefined,
          apply: c.params.length ? `${c.name}(${c.params[0]}: )` : `${c.name}()`,
        }),
      ),
      ...RESPONSE,
      ...vars()
        .filter((v) => !v.name.startsWith("$") && /^[\p{L}_][\p{L}\p{N}_]*$/u.test(v.name))
        .map((v): Completion => ({
          label: v.name,
          type: "variable",
          detail: v.source === "saved" || v.source === "secret" ? v.source : v.value ?? undefined,
        })),
    );
    return { from: word.from, options, validFor: /^[\p{L}\p{N}_-]*$/u };
  };
}

class RunMarker extends GutterMarker {
  toDOM() {
    const el = document.createElement("span");
    el.className = "cm-run-marker";
    el.textContent = "▶";
    el.title = "Run (Ctrl+Enter)";
    return el;
  }
}
const runMarker = new RunMarker();
/** Строка, с которой начинается запрос (`GET /x`) или сценарий (`flow X {`). */
const RUNNABLE = /^([A-Z]{2,}\s|flow\s)/;

function diagnostics(state: EditorState, errors: ParseError[]): Diagnostic[] {
  return errors
    .filter((e) => e.line !== null)
    .map((e) => {
      const n = Math.min(Math.max(e.line!, 1), state.doc.lines);
      const line = state.doc.line(n);
      const indent = line.text.length - line.text.trimStart().length;
      // Со столбцом — от него до конца слова, иначе вся строка.
      const from = e.col ? line.from + Math.min(e.col - 1, line.length) : line.from + indent;
      const rest = line.text.slice(from - line.from);
      const word = /^[\p{L}\p{N}_.$-]+/u.exec(rest)?.[0].length ?? 0;
      const to = e.col ? Math.max(from + Math.max(word, 1), from) : line.to;
      return { from, to: Math.min(to, line.to), severity: "error", message: e.message };
    });
}

interface Props {
  /** Меняется при открытии другого файла — сбрасывает историю правок. */
  docKey: string;
  value: string;
  onChange: (value: string) => void;
  language: EditorLanguage;
  errors: ParseError[];
  vars: VarName[];
  /** *.routy: запросы и сценарии проекта для автодополнения */
  symbols?: Symbols | null;
  /** Строка курсора (с 1) — что запускать по Ctrl+Enter */
  onCursor?: (line: number) => void;
  /** ▶ на полях: запустить элемент на строке */
  onRun?: (line: number) => void;
}

export function CodeEditor({ docKey, value, onChange, language, errors, vars, symbols, onCursor, onRun }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  // Колбэки и данные для расширений — через ref, чтобы не пересоздавать редактор.
  const live = useRef({ onChange, vars, symbols, onCursor, onRun });
  live.current = { onChange, vars, symbols, onCursor, onRun };

  const makeState = (doc: string): EditorState => {
    const extensions: Extension[] = [
      lineNumbers(),
      highlightActiveLineGutter(),
      highlightSpecialChars(),
      history(),
      drawSelection(),
      EditorState.allowMultipleSelections.of(true),
      bracketMatching(),
      closeBrackets(),
      highlightActiveLine(),
      highlightSelectionMatches(),
      EditorView.lineWrapping,
      // Панель (overflow: hidden) обрезает подсказки — выносим их в body.
      tooltips({ parent: document.body }),
      lintGutter(),
      // Ctrl+Enter и Ctrl+S ловит App на window; здесь только гасим штатные действия CodeMirror.
      Prec.highest(keymap.of([{ key: "Mod-Enter", run: () => true }, { key: "Mod-s", run: () => true }])),
      keymap.of([...closeBracketsKeymap, ...defaultKeymap, ...searchKeymap, ...historyKeymap, ...completionKeymap, indentWithTab]),
      syntaxHighlighting(highlight),
      theme,
      EditorView.updateListener.of((u) => {
        if (u.docChanged) live.current.onChange(u.state.doc.toString());
        if (u.docChanged || u.selectionSet) live.current.onCursor?.(u.state.doc.lineAt(u.state.selection.main.head).number);
      }),
    ];
    if (language === "http") {
      extensions.push(
        httpLanguage,
        autocompletion({ override: [completions(() => live.current.vars)], icons: false }),
        placeholder("GET {{base}}/path"),
      );
    } else if (language === "routy") {
      extensions.push(
        routyLanguage,
        autocompletion({ override: [routyCompletions(() => live.current.vars, () => live.current.symbols ?? null)], icons: false }),
        placeholder("GET /path"),
        gutter({
          class: "cm-run-gutter",
          lineMarker: (v, block) => (RUNNABLE.test(v.state.doc.lineAt(block.from).text) ? runMarker : null),
          lineMarkerChange: (u) => u.docChanged,
          domEventHandlers: {
            mousedown: (v, block) => {
              const line = v.state.doc.lineAt(block.from);
              if (!RUNNABLE.test(line.text)) return false;
              live.current.onRun?.(line.number);
              return true;
            },
          },
        }),
      );
    } else {
      extensions.push(StreamLanguage.define(toml));
    }
    return EditorState.create({ doc, extensions });
  };

  useEffect(() => {
    const v = new EditorView({ state: makeState(value), parent: host.current! });
    view.current = v;
    return () => {
      v.destroy();
      view.current = null;
    };
  }, []);

  // Другой файл — новое состояние (история правок не переносится).
  useEffect(() => {
    const v = view.current;
    if (!v) return;
    v.setState(makeState(value));
    v.dispatch(setDiagnostics(v.state, diagnostics(v.state, errors)));
    live.current.onCursor?.(1);
  }, [docKey, language]);

  // Текст поменялся снаружи (перезагрузка файла с диска).
  useEffect(() => {
    const v = view.current;
    if (!v) return;
    const cur = v.state.doc.toString();
    if (cur !== value) v.dispatch({ changes: { from: 0, to: cur.length, insert: value } });
  }, [value]);

  useEffect(() => {
    const v = view.current;
    if (v) v.dispatch(setDiagnostics(v.state, diagnostics(v.state, errors)));
  }, [errors]);

  return <div className="code-editor" ref={host} />;
}
