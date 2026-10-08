// Редактор на CodeMirror 6: подсветка .http/env.toml, ошибка разбора на строке,
// автодополнение `{{var}}` и директив.
import { useEffect, useRef } from "react";
import { EditorState, Prec, type Extension } from "@codemirror/state";
import {
  EditorView,
  drawSelection,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
  placeholder,
  tooltips,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { HighlightStyle, StreamLanguage, bracketMatching, syntaxHighlighting } from "@codemirror/language";
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
import { tags } from "@lezer/highlight";
import type { ParseError, VarName } from "./api";
import { httpLanguage } from "./httpLanguage";

export type EditorLanguage = "http" | "toml";

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

export const highlight = HighlightStyle.define([
  { tag: tags.keyword, color: "var(--accent)", fontWeight: "600" },
  { tag: tags.url, color: "var(--text)" },
  { tag: tags.special(tags.variableName), color: "var(--m-patch)", backgroundColor: "color-mix(in srgb, var(--m-patch) 12%, transparent)", borderRadius: "3px" },
  { tag: tags.propertyName, color: "var(--m-put)" },
  { tag: tags.string, color: "var(--m-get)" },
  { tag: [tags.number, tags.atom], color: "var(--m-delete)" },
  { tag: [tags.operator, tags.punctuation, tags.meta], color: "var(--muted)" },
  { tag: tags.comment, color: "var(--faint)", fontStyle: "italic" },
]);

export const theme = EditorView.theme({
  "&": { height: "100%", backgroundColor: "transparent", color: "var(--text)" },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": { fontFamily: "var(--mono)", fontSize: "12.5px", lineHeight: "1.65" },
  ".cm-content": { padding: "12px 0", caretColor: "var(--accent)" },
  ".cm-line": { padding: "0 16px 0 6px" },
  ".cm-gutters": { backgroundColor: "transparent", color: "var(--faint)", border: "none" },
  ".cm-lineNumbers .cm-gutterElement": { padding: "0 4px 0 12px", minWidth: "32px" },
  ".cm-activeLine": { backgroundColor: "var(--hover)" },
  ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--muted)" },
  ".cm-cursor": { borderLeftColor: "var(--accent)", borderLeftWidth: "2px" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": {
    backgroundColor: "color-mix(in srgb, var(--accent) 22%, transparent) !important",
  },
  ".cm-selectionMatch": { backgroundColor: "color-mix(in srgb, var(--accent) 12%, transparent)" },
  ".cm-matchingBracket": { backgroundColor: "var(--track)", outline: "1px solid var(--line)" },
  ".cm-placeholder": { color: "var(--faint)" },
  ".cm-lintRange-error": {
    backgroundImage: "none",
    textDecoration: "underline wavy var(--bad)",
    textDecorationSkipInk: "none",
    textUnderlineOffset: "3px",
  },
  ".cm-gutter-lint": { width: "12px" },
  ".cm-lint-marker": { width: "8px", height: "8px", content: "none" },
  ".cm-lint-marker-error": { content: "none", borderRadius: "50%", backgroundColor: "var(--bad)" },
  ".cm-tooltip": {
    backgroundColor: "var(--raised)",
    color: "var(--text)",
    border: "1px solid var(--line)",
    borderRadius: "8px",
    boxShadow: "var(--panel-shadow)",
    overflow: "hidden",
  },
  ".cm-tooltip-autocomplete > ul": { fontFamily: "var(--mono)", fontSize: "12px", maxHeight: "16em" },
  ".cm-tooltip-autocomplete > ul > li": { padding: "3px 10px" },
  ".cm-tooltip-autocomplete > ul > li[aria-selected]": {
    backgroundColor: "color-mix(in srgb, var(--accent) 18%, transparent)",
    color: "var(--text)",
  },
  ".cm-completionDetail": { color: "var(--faint)", fontStyle: "normal", marginLeft: "12px" },
  ".cm-completionIcon": { display: "none" },
  ".cm-tooltip-lint": { padding: "0" },
  ".cm-diagnostic": { padding: "6px 10px", fontFamily: "var(--mono)", fontSize: "12px" },
  ".cm-diagnostic-error": { borderLeft: "3px solid var(--bad)" },
  ".cm-panels": { backgroundColor: "var(--panel)", color: "var(--text)" },
  ".cm-panels.cm-panels-bottom": { borderTop: "1px solid var(--line)" },
  ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--line)" },
  ".cm-search": { fontFamily: "var(--sans)", fontSize: "12px", display: "flex", flexWrap: "wrap", alignItems: "center", gap: "4px 6px", padding: "6px 28px 6px 10px" },
  ".cm-search br": { display: "none" },
  ".cm-search input, .cm-search button": { margin: "0", padding: "2px 8px", fontSize: "12px" },
  ".cm-textfield, .cm-button": {
    backgroundColor: "var(--panel)",
    backgroundImage: "none",
    color: "var(--text)",
    border: "1px solid var(--line)",
    borderRadius: "6px",
  },
  ".cm-textfield:focus": { borderColor: "var(--accent)", outline: "none" },
  ".cm-button:active": { backgroundImage: "none", backgroundColor: "var(--hover)" },
  ".cm-search label": { display: "inline-flex", alignItems: "center", gap: "3px", color: "var(--muted)", fontSize: "12px" },
  ".cm-search label input": { accentColor: "var(--accent)" },
  ".cm-panel.cm-search [name=close]": { color: "var(--muted)", fontSize: "16px", top: "4px", right: "6px" },
});

function diagnostics(state: EditorState, error: ParseError | null): Diagnostic[] {
  if (!error || error.line === null) return [];
  const n = Math.min(Math.max(error.line, 1), state.doc.lines);
  const line = state.doc.line(n);
  const indent = line.text.length - line.text.trimStart().length;
  return [{ from: line.from + indent, to: line.to, severity: "error", message: error.message }];
}

interface Props {
  /** Меняется при открытии другого файла — сбрасывает историю правок. */
  docKey: string;
  value: string;
  onChange: (value: string) => void;
  language: EditorLanguage;
  error: ParseError | null;
  vars: VarName[];
}

export function CodeEditor({ docKey, value, onChange, language, error, vars }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  // Колбэки и данные для расширений — через ref, чтобы не пересоздавать редактор.
  const live = useRef({ onChange, vars });
  live.current = { onChange, vars };

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
      }),
    ];
    if (language === "http") {
      extensions.push(
        httpLanguage,
        autocompletion({ override: [completions(() => live.current.vars)], icons: false }),
        placeholder("GET {{base}}/path"),
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
    v.dispatch(setDiagnostics(v.state, diagnostics(v.state, error)));
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
    if (v) v.dispatch(setDiagnostics(v.state, diagnostics(v.state, error)));
  }, [error]);

  return <div className="code-editor" ref={host} />;
}
