; Роутер gin (github.com/gin-gonic/gin). Формат захватов — в src/import/go.rs.
; routy: import github.com/gin-gonic/gin

; r.GET("/users/:id", h), v1.POST("/users", mw, h)
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @method)
  arguments: (argument_list . (_) @path (_)* @middleware (_) @handler .)
  (#any-of? @method "GET" "POST" "PUT" "PATCH" "DELETE" "HEAD" "OPTIONS" "Any")) @route

; r.Handle("GET", "/users", h)
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @method . (_) @path (_)* @middleware (_) @handler .)
  (#eq? @_fn "Handle")) @route

; v1 := r.Group("/v1")
(short_var_declaration
  left: (expression_list . (identifier) @group.var)
  right: (expression_list
    .
    (call_expression
      function: (selector_expression
        operand: (_) @receiver
        field: (field_identifier) @_fn)
      arguments: (argument_list . (_) @group.path (_)* @middleware)) @group)
  (#eq? @_fn "Group"))

; v1 = r.Group("/v1")
(assignment_statement
  left: (expression_list . (identifier) @group.var)
  right: (expression_list
    .
    (call_expression
      function: (selector_expression
        operand: (_) @receiver
        field: (field_identifier) @_fn)
      arguments: (argument_list . (_) @group.path (_)* @middleware)) @group)
  (#eq? @_fn "Group"))

; var v1 = r.Group("/v1")
(var_spec
  name: (identifier) @group.var
  value: (expression_list
    .
    (call_expression
      function: (selector_expression
        operand: (_) @receiver
        field: (field_identifier) @_fn)
      arguments: (argument_list . (_) @group.path (_)* @middleware)) @group)
  (#eq? @_fn "Group"))

; r.Group("/v1").GET(...) — группа без переменной
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @group.path (_)* @middleware)
  (#eq? @_fn "Group")) @group

; r.Use(gin.Logger()), v1.Use(auth)
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list (_)* @middleware)
  (#eq? @_fn "Use")) @use

; users.Register(v1), users.Register(r.Group("/users")) — группа передана в функцию,
; её роуты получают префикс группы
(call_expression
  function: [
    (identifier) @mount.func
    (selector_expression operand: (_) @mount.pkg field: (field_identifier) @mount.func)
  ]
  arguments: (argument_list [(identifier) (call_expression)] @receiver)) @mount
