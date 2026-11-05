; Роутер fiber (github.com/gofiber/fiber, v2 и v3). Формат захватов — в src/import/go.rs.
; outry: import github.com/gofiber/fiber

; app.Get("/users/:id", h), api.Post("/users", mw, h)
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @method)
  arguments: (argument_list . (_) @path (_)* @middleware (_) @handler .)
  (#any-of? @method "Get" "Post" "Put" "Patch" "Delete" "Head" "Options" "Connect" "Trace" "All")) @route

; app.Add("GET", "/users", h) (v2), app.Add([]string{"GET", "POST"}, "/users", h) (v3)
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @method . (_) @path (_)* @middleware (_) @handler .)
  (#eq? @_fn "Add")) @route

; api := app.Group("/api", mw...)
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

; api = app.Group("/api")
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

; var api = app.Group("/api")
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

; app.Group("/api").Get(...) — группа без переменной
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @group.path (_)* @middleware)
  (#eq? @_fn "Group")) @group

; app.Route("/users", func(r fiber.Router) { ... }) (v2)
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @group.path . (func_literal body: (block) @group.body))
  (#eq? @_fn "Route")) @group

; app.Use(logger.New()), api.Use(auth) — без пути
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list
    .
    [(identifier) (call_expression) (selector_expression) (func_literal)] @middleware
    (_)* @middleware)
  (#eq? @_fn "Use")) @use

; micro := fiber.New(); ...; app.Mount("/john", micro), app.Use("/john", micro)
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (interpreted_string_literal) @group.path . (identifier) @group.var .)
  (#any-of? @_fn "Mount" "Use")) @group

; app.Mount("/admin", admin.New()) — роуты функции получают префикс
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list
    .
    (_) @mount.path
    .
    (call_expression
      function: [
        (identifier) @mount.func
        (selector_expression operand: (_) @mount.pkg field: (field_identifier) @mount.func)
      ])
    .)
  (#eq? @_fn "Mount")) @mount

; users.Register(api), users.Register(app.Group("/users")) — роутер передан в функцию,
; её роуты получают префикс
(call_expression
  function: [
    (identifier) @mount.func
    (selector_expression operand: (_) @mount.pkg field: (field_identifier) @mount.func)
  ]
  arguments: (argument_list [(identifier) (call_expression)] @receiver)) @mount
