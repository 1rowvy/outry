// Тело ответа: CodeMirror только для чтения — подсветка JSON/HTML/XML и поиск (Ctrl+F).
import { useEffect, useImperativeHandle, useRef, type Ref } from "react";
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView, drawSelection, highlightSpecialChars, keymap, lineNumbers, tooltips } from "@codemirror/view";
import { defaultKeymap } from "@codemirror/commands";
import { StreamLanguage, bracketMatching, syntaxHighlighting } from "@codemirror/language";
import { json } from "@codemirror/legacy-modes/mode/javascript";
import { html, xml } from "@codemirror/legacy-modes/mode/xml";
import { highlightSelectionMatches, openSearchPanel, search, searchKeymap } from "@codemirror/search";
import { highlight, theme } from "./CodeEditor";

export type BodyLanguage = "json" | "html" | "xml" | "text";

export interface BodyViewerHandle {
  find: () => void;
}

const LANGUAGES: Record<BodyLanguage, Extension> = {
  json: StreamLanguage.define(json),
  html: StreamLanguage.define(html),
  xml: StreamLanguage.define(xml),
  text: [],
};

// Поиск сверху, а не снизу: снизу его перекрывает край панели при длинном теле.
const searchTop = search({ top: true });

export function BodyViewer({ text, language, ref }: { text: string; language: BodyLanguage; ref?: Ref<BodyViewerHandle> }) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);

  useImperativeHandle(ref, () => ({
    find: () => {
      if (view.current) {
        openSearchPanel(view.current);
      }
    },
  }));

  useEffect(() => {
    const v = new EditorView({
      parent: host.current!,
      state: EditorState.create({
        doc: text,
        extensions: [
          lineNumbers(),
          highlightSpecialChars(),
          drawSelection(),
          bracketMatching(),
          highlightSelectionMatches(),
          EditorState.readOnly.of(true),
          EditorView.lineWrapping,
          tooltips({ parent: document.body }),
          searchTop,
          keymap.of([...searchKeymap, ...defaultKeymap]),
          syntaxHighlighting(highlight),
          theme,
          LANGUAGES[language],
        ],
      }),
    });
    view.current = v;
    return () => {
      v.destroy();
      view.current = null;
    };
  }, [text, language]);

  return <div className="code-editor body-viewer" ref={host} />;
}
