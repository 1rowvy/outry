; Роутер echo (github.com/labstack/echo). Формат захватов — в src/import/go.rs.
; outry: import github.com/labstack/echo

; e.GET("/users/:id", h), g.POST("/users", h, mw...) — middleware после обработчика
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @method)
  arguments: (argument_list . (_) @path . (_) @handler (_)* @middleware)
  (#any-of? @method "GET" "POST" "PUT" "PATCH" "DELETE" "HEAD" "OPTIONS" "CONNECT" "TRACE" "Any")) @route

; e.Add("GET", "/users", h, mw...)
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @method . (_) @path . (_) @handler (_)* @middleware)
  (#eq? @_fn "Add")) @route

; e.Match([]string{"GET", "POST"}, "/users", h, mw...) — роут на каждый метод
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (composite_literal) @method . (_) @path . (_) @handler (_)* @middleware)
  (#eq? @_fn "Match")) @route

; g := e.Group("/v1", mw...)
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

; g = e.Group("/v1")
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

; var g = e.Group("/v1")
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

; e.Group("/v1").GET(...) — группа без переменной
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @group.path (_)* @middleware)
  (#eq? @_fn "Group")) @group

; e.Use(middleware.Logger()), g.Use(auth)
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list (_)* @middleware)
  (#eq? @_fn "Use")) @use

; users.Register(g), users.Register(e.Group("/users")) — группа передана в функцию,
; её роуты получают префикс группы
(call_expression
  function: [
    (identifier) @mount.func
    (selector_expression operand: (_) @mount.pkg field: (field_identifier) @mount.func)
  ]
  arguments: (argument_list [(identifier) (call_expression)] @receiver)) @mount
