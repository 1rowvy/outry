; Роутер chi (github.com/go-chi/chi). Формат захватов — в src/import/go.rs.
; outry: import github.com/go-chi/chi

; r.Get("/users/{id}", h), r.With(mw).Post(...)
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @method)
  arguments: (argument_list . (_) @path (_) @handler .)
  (#any-of? @method "Get" "Post" "Put" "Patch" "Delete" "Head" "Options" "Connect" "Trace")) @route

; r.Method("GET", "/users", h)
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @method . (_) @path (_) @handler .)
  (#any-of? @_fn "Method" "MethodFunc")) @route

; r.Handle("/users", h) — любой метод
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @path . (_) @handler .)
  (#any-of? @_fn "Handle" "HandleFunc")) @route

; r.Route("/users", func(r chi.Router) { ... })
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @group.path . (func_literal body: (block) @group.body) .)
  (#eq? @_fn "Route")) @group

; r.Group(func(r chi.Router) { r.Use(auth); ... }) — группа без префикса, для middleware
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (func_literal body: (block) @group.body) .)
  (#eq? @_fn "Group")) @group

; r.Use(auth) — middleware для роутов r дальше по коду; r.With(auth).Get(...) — для одного вызова
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list (_)* @middleware)
  (#any-of? @_fn "Use" "With")) @use

; r.Mount("/admin", adminRouter()), r.Mount("/users", users.Routes())
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

; admin := chi.NewRouter(); ...; r.Mount("/admin", admin)
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @group.path . (identifier) @group.var .)
  (#eq? @_fn "Mount")) @group

; users.Register(r) — роутер передан в функцию, её роуты получают префикс r
(call_expression
  function: [
    (identifier) @mount.func
    (selector_expression operand: (_) @mount.pkg field: (field_identifier) @mount.func)
  ]
  arguments: (argument_list [(identifier) (call_expression)] @receiver)) @mount
