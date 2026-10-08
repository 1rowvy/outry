// Подсветка .http для CodeMirror. Разбор здесь приблизительный, только для цвета:
// правила формата (и ошибки) — в routy-core/src/parser.rs.
import { StreamLanguage, type StringStream } from "@codemirror/language";
import { tags } from "@lezer/highlight";

type Section = "start" | "headers" | "body";
type Line = "request" | "header" | "directive" | "body";

interface State {
  section: Section;
  line: Line;
  /** Внутри строки JSON-тела, разорванной `{{var}}`. */
  inString: boolean;
  /** В строке заголовка уже прошли `:`. */
  afterColon: boolean;
}

const OPERATORS = /^(==|!=|<=|>=|<|>|=|contains\b|exists\b)/;

function variable(stream: StringStream): boolean {
  return !!stream.match(/^\{\{[^}\n]*(\}\})?/);
}

/** Читает до `{{` или конца строки. */
function until(stream: StringStream, stop: RegExp) {
  while (!stream.eol() && !stream.match("{{", false) && !stream.match(stop, false)) stream.next();
}

function bodyToken(stream: StringStream, state: State): string | null {
  if (state.inString || stream.peek() === '"') {
    if (!state.inString) {
      stream.next();
      state.inString = true;
    }
    while (!stream.eol()) {
      if (stream.match("{{", false)) return "string";
      const c = stream.next();
      if (c === "\\") stream.next();
      else if (c === '"') {
        state.inString = false;
        return stream.match(/^\s*:/, false) ? "key" : "string";
      }
    }
    return "string";
  }
  if (stream.match(/^-?\d+(\.\d+)?([eE][+-]?\d+)?/)) return "number";
  if (stream.match(/^(true|false|null)\b/)) return "atom";
  stream.next();
  return null;
}

function directiveToken(stream: StringStream): string | null {
  if (stream.match(/^(save|assert)\b/)) return "keyword";
  if (stream.match(OPERATORS)) return "operator";
  if (stream.match(/^"(?:[^"\\]|\\.)*"?/)) return "string";
  if (stream.match(/^-?\d+(\.\d+)?\b/)) return "number";
  if (stream.match(/^(true|false|null)\b/)) return "atom";
  if (stream.match(/^[\w$-]+/)) return "path";
  stream.next();
  return null;
}

export const httpLanguage = StreamLanguage.define<State>({
  name: "http",
  startState: () => ({ section: "start", line: "request", inString: false, afterColon: false }),
  copyState: (s) => ({ ...s }),
  blankLine: (state) => {
    if (state.section === "headers") state.section = "body";
  },
  token(stream, state) {
    if (stream.sol()) {
      state.inString = false;
      state.afterColon = false;
      if (state.section !== "body" && stream.match(/^\s*(#|\/\/)/)) {
        stream.skipToEnd();
        return "comment";
      }
      if (state.section !== "start" && stream.match(/^>/)) {
        state.line = "directive";
        return "directiveMark";
      }
      if (state.section === "start") {
        if (stream.eatSpace()) return null;
        state.section = "headers";
        state.line = "request";
        if (stream.match(/^[A-Za-z]+(?=\s)/)) return "method";
      } else {
        state.line = state.section === "headers" ? "header" : "body";
        if (state.line === "header" && stream.match(/^[^:{\s][^:{]*(?=:)/)) return "headerName";
      }
    }
    if (stream.eatSpace()) return null;
    if (variable(stream)) return "var";

    switch (state.line) {
      case "request":
        until(stream, /^\s/);
        return "url";
      case "header":
        if (!state.afterColon && stream.eat(":")) {
          state.afterColon = true;
          return "punctuation";
        }
        until(stream, /^$/);
        return "headerValue";
      case "directive":
        return directiveToken(stream);
      case "body":
        return bodyToken(stream, state);
    }
  },
  tokenTable: {
    method: tags.keyword,
    url: tags.url,
    var: tags.special(tags.variableName),
    headerName: tags.propertyName,
    headerValue: tags.string,
    punctuation: tags.punctuation,
    directiveMark: tags.meta,
    keyword: tags.keyword,
    operator: tags.operator,
    path: tags.propertyName,
    key: tags.propertyName,
    string: tags.string,
    number: tags.number,
    atom: tags.atom,
    comment: tags.comment,
  },
  languageData: {
    commentTokens: { line: "#" },
  },
});
