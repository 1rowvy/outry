; Neovim (nvim-treesitter) and Zed capture names. Helix: queries/helix/highlights.scm.
; Later patterns win in Neovim, earlier ones in Helix — keep the generic `identifier` first here.

(identifier) @variable
(property) @property

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
 "query" "headers" "form" "multipart" "body"] @property

(handler (identifier) @module)
(only_field (identifier) @constant)

(param name: (identifier) @variable.parameter)
(save_statement name: (identifier) @variable)
(let_declaration name: (identifier) @variable)
(flow_binding name: (identifier) @variable)
(lambda parameter: (identifier) @variable.parameter)
(argument name: (identifier) @variable.parameter)

(flow name: (identifier) @function)

; `Login()` — a request or flow; lower-case names are built-in functions.
(call_expression function: (identifier) @function.call)
(call_expression function: (identifier) @function.builtin
  (#match? @function.builtin "^(uuid|now|nowIso|randomInt|randomString|number|string|json|base64|unbase64|file)$"))
(call_expression function: (member_expression property: (property) @function.method.call))
(call_expression function: (member_expression property: (property) @function.call)
  (#match? @function.call "^[A-Z]"))

; Names defined by the response and the run.
((identifier) @variable.builtin
  (#any-of? @variable.builtin "status" "headers" "body" "duration" "cookies" "env" "vars"))

; Shapes
(shape_declaration name: (identifier) @type.definition)
(type) @type
((type) @type.builtin
  (#any-of? @type.builtin "string" "number" "integer" "boolean" "any"))
(schema "schema" @function.builtin)

; Literals
(string) @string
(escape_sequence) @string.escape
(regex) @string.regexp
(number) @number
(duration) @number
[(true) (false)] @boolean
(null) @constant.builtin

(interpolation ["${" "}"] @punctuation.special)

["==" "!=" "<" "<=" ">" ">=" "&&" "||" "!" "+" "-" "*" "/" "%" "=" "=>" "|" "?"] @operator
["{" "}" "[" "]" "(" ")"] @punctuation.bracket
["," ":" "."] @punctuation.delimiter
