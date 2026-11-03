; Роутер buffalo (github.com/gobuffalo/buffalo). Формат захватов — в src/import/go.rs.
; outry: import github.com/gobuffalo/buffalo

; app.GET("/", HomeHandler), api.POST("/login", login)
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @method)
  arguments: (argument_list . (_) @path . (_) @handler .)
  (#any-of? @method "GET" "POST" "PUT" "PATCH" "DELETE" "HEAD" "OPTIONS" "ANY")) @route

; app.Resource("/users", UsersResource{}) — List, Show, New, Create, Edit, Update, Destroy
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @path . (_) @resource .)
  (#eq? @_fn "Resource")) @route

; api := app.Group("/api/v1")
(short_var_declaration
  left: (expression_list . (identifier) @group.var)
  right: (expression_list
    .
    (call_expression
      function: (selector_expression
        operand: (_) @receiver
        field: (field_identifier) @_fn)
      arguments: (argument_list . (_) @group.path .)) @group)
  (#eq? @_fn "Group"))

; api = app.Group("/api/v1")
(assignment_statement
  left: (expression_list . (identifier) @group.var)
  right: (expression_list
    .
    (call_expression
      function: (selector_expression
        operand: (_) @receiver
        field: (field_identifier) @_fn)
      arguments: (argument_list . (_) @group.path .)) @group)
  (#eq? @_fn "Group"))

; app.Group("/api").GET(...) — группа без переменной
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @group.path .)
  (#eq? @_fn "Group")) @group

; app.Use(forceSSL(), auth)
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list (_)* @middleware)
  (#eq? @_fn "Use")) @use

; app.Middleware.Skip(Authorize, HomeHandler, ur.List)
(call_expression
  function: (selector_expression
    operand: (selector_expression field: (field_identifier) @_m)
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @middleware (_)* @skip.handler)
  (#eq? @_m "Middleware")
  (#eq? @_fn "Skip")) @skip

; actions.Register(app), users.Routes(api.Group("/users")) — роуты функции получают префикс
(call_expression
  function: [
    (identifier) @mount.func
    (selector_expression operand: (_) @mount.pkg field: (field_identifier) @mount.func)
  ]
  arguments: (argument_list [(identifier) (call_expression)] @receiver)) @mount
