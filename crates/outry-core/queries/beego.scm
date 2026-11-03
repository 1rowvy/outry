; Роутер beego (github.com/beego/beego/v2/server/web, раньше github.com/astaxie/beego).
; Формат захватов — в src/import/go.rs.
; outry: import github.com/beego/beego
; outry: import github.com/astaxie/beego

; web.Router("/user/:id", &controllers.UserController{}) — методы контроллера Get, Post, …
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @path . (_) @controller .)
  (#any-of? @_fn "Router" "NSRouter")) @route

; web.Router("/user", &UserController{}, "get:GetAll;post,put:Save")
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @path . (_) @controller . (_) @mapping)
  (#any-of? @_fn "Router" "NSRouter")) @route

; web.Get("/x", func(ctx *context.Context) {…}), ns.Post(...), web.NSGet(...),
; web.CtrlGet("/x", (*UserController).Get), web.NSCtrlPost(...)
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @method)
  arguments: (argument_list . (_) @path . (_) @handler .)
  (#match? @method "^(NS)?(Ctrl)?(Get|Post|Put|Patch|Delete|Head|Options|Any)$")) @route

; web.Include(&UserController{}), web.NSInclude(&A{}, &B{}) — роуты из `// @router /path [get]`
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list (_)* @include)
  (#any-of? @_fn "Include" "NSInclude")) @route

; web.NewNamespace("/v1", web.NSNamespace("/user", …), web.NSRouter(…)) — префикс для вложенных
(call_expression
  function: (selector_expression field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @group.path) @group.body
  (#any-of? @_fn "NewNamespace" "NSNamespace")) @group

; ns := web.NewNamespace("/v1"); ns.Router(...)
(short_var_declaration
  left: (expression_list . (identifier) @group.var)
  right: (expression_list
    .
    (call_expression
      function: (selector_expression field: (field_identifier) @_fn)
      arguments: (argument_list . (_) @group.path)) @group)
  (#eq? @_fn "NewNamespace"))

; web.NSBefore(auth) — middleware для роутов namespace
(call_expression
  function: (selector_expression field: (field_identifier) @_fn)
  arguments: (argument_list
    (call_expression
      function: (selector_expression field: (field_identifier) @_b)
      arguments: (argument_list (_)* @middleware)))
  (#any-of? @_fn "NewNamespace" "NSNamespace")
  (#eq? @_b "NSBefore")) @group
