; Роутер go-restful (github.com/emicklei/go-restful). Формат захватов — в src/import/go.rs.
; ws.Route(ws.GET("/{id}").To(h).Reads(T{}).Writes(T{})): роут — вызов ws.GET(...), остальное —
; звенья его цепочки: они начинаются там же, где ws.GET, и сливаются с ним в один роут.
; outry: import github.com/emicklei/go-restful

; ws.GET("/{id}")
(call_expression
  function: (selector_expression
    operand: (identifier) @receiver
    field: (field_identifier) @method)
  arguments: (argument_list . (_) @path .)
  (#any-of? @method "GET" "POST" "PUT" "PATCH" "DELETE" "HEAD" "OPTIONS")) @route

; ws.Method("GET").Path("/x")
(call_expression
  function: (selector_expression
    operand: (identifier) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @method .)
  (#eq? @_fn "Method")) @route

(call_expression
  function: (selector_expression
    operand: (call_expression)
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @path .)
  (#eq? @_fn "Path")) @route

; .To(h)
(call_expression
  function: (selector_expression field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @handler .)
  (#eq? @_fn "To")) @route

; .Reads(CreateUser{}) — тело
(call_expression
  function: (selector_expression field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @body)
  (#eq? @_fn "Reads")) @route

; .Writes(User{}), .Returns(200, "OK", User{}) — ответ
(call_expression
  function: (selector_expression field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @response)
  (#eq? @_fn "Writes")) @route

(call_expression
  function: (selector_expression field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @_code . (_) . (_) @response)
  (#eq? @_fn "Returns")
  (#match? @_code "^(20[0-2]|http[.]Status(OK|Created|Accepted))$")) @route

; .Param(ws.QueryParameter("page", "…").DataType("integer"))
(call_expression
  function: (selector_expression field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @query)
  (#eq? @_fn "Param")
  (#match? @query "^[A-Za-z_][A-Za-z0-9_]*[.]QueryParameter[(]")) @route

(call_expression
  function: (selector_expression field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @header)
  (#eq? @_fn "Param")
  (#match? @header "^[A-Za-z_][A-Za-z0-9_]*[.]HeaderParameter[(]")) @route

; .Filter(auth) в цепочке роута
(call_expression
  function: (selector_expression field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @middleware .)
  (#eq? @_fn "Filter")) @route

; ws.Path("/users") — префикс роутов ws (в той же функции)
(call_expression
  function: (selector_expression
    operand: (identifier) @group.var
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @group.path .)
  (#eq? @_fn "Path")) @group

; ws.Filter(auth) — для роутов ws дальше по коду
(call_expression
  function: (selector_expression
    operand: (identifier) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list (_)* @middleware)
  (#eq? @_fn "Filter")) @use
