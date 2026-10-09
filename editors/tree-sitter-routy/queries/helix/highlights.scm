; Helix capture names. In Helix the first matching pattern wins, so specific patterns come first
; and the generic `identifier` is last (the opposite of queries/highlights.scm).

(comment) @comment

; Requests
(method) @keyword
(path_text) @string.special.url
(path_param (identifier) @variable.parameter)
(path_param ["{" "}"] @punctuation.special)

["flow" "shape" "let"] @keyword
["expect" "save" "poll" "every" "for"] @keyword
["fresh" "in" "matches" "typeof"] @keyword.operator

["handler" "params" "only" "confirm" "redirects" "timeout" "cache"
 "query" "headers" "form" "multipart" "body"] @variable.other.member

(handler (identifier) @namespace)
(only_field (identifier) @constant)

(param name: (identifier) @variable.parameter)
(lambda parameter: (identifier) @variable.parameter)
(argument name: (identifier) @variable.parameter)

(flow name: (identifier) @function)

; Built-in functions before requests and flows: `uuid()` vs `Login()`.
(call_expression function: (identifier) @function.builtin
  (#match? @function.builtin "^(uuid|now|nowIso|randomInt|randomString|number|string|json|base64|unbase64|file)$"))
(call_expression function: (identifier) @function)
(call_expression function: (member_expression property: (property) @function)
  (#match? @function "^[A-Z]"))
(call_expression function: (member_expression property: (property) @function.method))

((identifier) @variable.builtin
  (#any-of? @variable.builtin "status" "headers" "body" "duration" "cookies" "env" "vars"))

(property) @variable.other.member

; Shapes
(shape_declaration name: (identifier) @type)
((type) @type.builtin
  (#any-of? @type.builtin "string" "number" "integer" "boolean" "any"))
(type) @type
(schema "schema" @function.builtin)

; Literals
(escape_sequence) @constant.character.escape
(interpolation ["${" "}"] @punctuation.special)
(string) @string
(regex) @string.regexp
(number) @constant.numeric
(duration) @constant.numeric
[(true) (false)] @constant.builtin.boolean
(null) @constant.builtin

["==" "!=" "<" "<=" ">" ">=" "&&" "||" "!" "+" "-" "*" "/" "%" "=" "=>" "|" "?"] @operator
["{" "}" "[" "]" "(" ")"] @punctuation.bracket
["," ":" "."] @punctuation.delimiter

(identifier) @variable
