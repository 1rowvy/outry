// Подсветка *.outry для CodeMirror. Разбор приблизительный, только для цвета:
// правила формата (и ошибки) — в outry-core/src/lang/parse.rs.
import { StreamLanguage, type StringStream } from "@codemirror/language";
import { tags } from "@lezer/highlight";

type Frame = { kind: "str"; quote: string } | { kind: "interp"; depth: number };

interface State {
  stack: Frame[];
  comment: boolean;
  /** Адрес запроса после метода: 1 — ждём, 2 — внутри. */
  target: 0 | 1 | 2;
  /** После `matches` — может быть /regex/. */
  matches: boolean;
  /** После `.` — имя поля или метода. */
  member: boolean;
  /** После имени запроса `Login` в `Login: POST …` — ждём `:` и метод. */
  named: boolean;
}

export const KEYWORDS = ["let", "shape", "flow", "fresh", "save", "expect", "poll", "every", "for", "matches", "in", "typeof"];
export const FIELDS = ["handler", "params", "only", "confirm", "timeout", "redirects", "cache", "query", "headers", "body", "form", "multipart"];
export const BUILTINS = ["uuid", "now", "nowIso", "randomInt", "randomString", "number", "string", "json", "base64", "unbase64", "file", "schema"];
export const TYPES = ["string", "number", "integer", "boolean", "any"];

const top = (s: State) => s.stack[s.stack.length - 1];

function stringToken(stream: StringStream, state: State, quote: string): string {
  while (!stream.eol()) {
    if (stream.match("${", false)) return "string";
    if (stream.match(quote)) {
      state.stack.pop();
      return "string";
    }
    if (stream.next() === "\\") stream.next();
  }
  return "string";
}

function code(stream: StringStream, state: State): string | null {
  if (state.target && !top(state)) {
    state.target = 2;
    if (stream.match(/^\{[^}\s]*\}/)) return "param";
    if (stream.match(/^\$\{/)) {
      state.stack.push({ kind: "interp", depth: 0 });
      return "interp";
    }
    if (stream.match(/^[^\s{$]+/) || stream.match(/^\$/)) return "url";
  }
  if (stream.match("//")) {
    stream.skipToEnd();
    return "comment";
  }
  if (stream.match("/*")) {
    state.comment = true;
    return blockComment(stream, state);
  }
  if (state.matches && stream.match(/^\/(?:[^/\\\n]|\\.)+\/[a-z]*/)) {
    state.matches = false;
    return "regexp";
  }
  state.matches = false;
  for (const q of ['"""', '"', "'"]) {
    if (stream.match(q)) {
      state.stack.push({ kind: "str", quote: q });
      return stringToken(stream, state, q);
    }
  }
  if (stream.match(/^\d+(\.\d+)?(ms|s|m|h)\b/)) return "number";
  if (stream.match(/^\d+(\.\d+)?([eE][+-]?\d+)?/)) return "number";

  const member = state.member;
  state.member = false;
  // Имя заголовка с дефисами — только перед `:`; после `.` — `headers.content-type`.
  const dashed = /^[\p{L}_][\p{L}\p{N}_]*(-[\p{L}\p{N}_]+)+/u;
  if (stream.match(new RegExp(dashed.source + /(?=\s*:)/.source, "u"))) return "key";
  if (member && stream.match(dashed)) return "member";
  const word = stream.match(/^[\p{L}_][\p{L}\p{N}_]*/u) as RegExpMatchArray | null;
  if (word) {
    const w = word[0];
    if (member) return stream.match(/^\s*\(/, false) ? "methodCall" : "member";
    const lineStart = /^\s*$/.test(stream.string.slice(0, stream.start));
    if (stream.match(/^\??(?=\s*:)/, false) && !KEYWORDS.includes(w)) {
      return FIELDS.includes(w) && lineStart ? "field" : "key";
    }
    if (KEYWORDS.includes(w)) {
      if (w === "matches") state.matches = true;
      return "keyword";
    }
    if (w === "true" || w === "false" || w === "null") return "atom";
    // `body {`, `headers {`, `body "text"` — поле; `body.id`, `body matches` в expect — значение.
    if (FIELDS.includes(w) && lineStart && /^\s*([{["':]|file\s*\()/.test(stream.string.slice(stream.pos))) return "field";
    if (TYPES.includes(w)) return "type";
    if (stream.match(/^\s*\(/, false)) {
      if (/^\p{Lu}/u.test(w)) return "call";
      if (BUILTINS.includes(w)) return "builtin";
    }
    if (/^\p{Lu}/u.test(w)) return "call";
    return "variable";
  }
  if (stream.eat(".")) {
    state.member = true;
    return "punctuation";
  }
  if (stream.match(/^(==|!=|<=|>=|&&|\|\||=>|[<>=!+\-*/%|?])/)) return "operator";
  const c = stream.next();
  const t = top(state);
  if (t?.kind === "interp") {
    if (c === "{") t.depth++;
    else if (c === "}") {
      if (t.depth === 0) {
        state.stack.pop();
        return "interp";
      }
      t.depth--;
    }
  }
  return "punctuation";
}

function blockComment(stream: StringStream, state: State): string {
  while (!stream.eol()) {
    if (stream.match("*/")) {
      state.comment = false;
      return "comment";
    }
    stream.next();
  }
  return "comment";
}

export const outryLanguage = StreamLanguage.define<State>({
  name: "outry",
  startState: () => ({ stack: [], comment: false, target: 0, matches: false, member: false, named: false }),
  copyState: (s) => ({ ...s, stack: s.stack.map((f) => ({ ...f })) }),
  token(stream, state) {
    if (stream.sol()) state.target = 0;
    if (state.comment) return blockComment(stream, state);
    const t = top(state);
    if (t?.kind === "str") {
      if (stream.match("${")) {
        state.stack.push({ kind: "interp", depth: 0 });
        return "interp";
      }
      return stringToken(stream, state, t.quote);
    }
    if (stream.sol() && !t && stream.match(/^[\p{L}_][\p{L}\p{N}_]*(?=:\s+[A-Z]{2,}(?:\s|$))/u)) {
      state.named = true;
      return "requestName";
    }
    if (state.named) {
      if (stream.eat(":")) return "punctuation";
      if (stream.eatSpace()) return null;
      state.named = false;
      if (stream.match(/^[A-Z]{2,}(?=\s|$)/)) {
        state.target = 1;
        return "httpMethod";
      }
    }
    if (stream.sol() && !t && stream.match(/^[A-Z]{2,}(?=\s|$)/)) {
      state.target = 1;
      return "httpMethod";
    }
    if (stream.eatSpace()) {
      if (state.target === 2 && !t) state.target = 0;
      return null;
    }
    if (state.target === 1 && stream.peek() === '"') state.target = 0;
    return code(stream, state);
  },
  tokenTable: {
    httpMethod: tags.keyword,
    requestName: tags.definition(tags.function(tags.variableName)),
    url: tags.url,
    param: tags.special(tags.variableName),
    interp: tags.special(tags.brace),
    keyword: tags.keyword,
    field: tags.definitionKeyword,
    key: tags.propertyName,
    member: tags.propertyName,
    methodCall: tags.function(tags.propertyName),
    call: tags.function(tags.variableName),
    builtin: tags.standard(tags.function(tags.variableName)),
    type: tags.typeName,
    regexp: tags.regexp,
    string: tags.string,
    number: tags.number,
    atom: tags.atom,
    variable: tags.variableName,
    operator: tags.operator,
    punctuation: tags.punctuation,
    comment: tags.comment,
  },
  languageData: {
    commentTokens: { line: "//", block: { open: "/*", close: "*/" } },
    closeBrackets: { brackets: ["(", "[", "{", '"', "'"] },
  },
});
