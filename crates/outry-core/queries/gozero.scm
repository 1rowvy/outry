; Роутер go-zero (github.com/zeromicro/go-zero/rest). Формат захватов — в src/import/go.rs.
; Роуты — литералы структур, обычно в сгенерированном goctl `routes.go`:
; server.AddRoutes([]rest.Route{{Method: http.MethodGet, Path: "/users/:id", Handler: h}}, rest.WithPrefix("/v1"))
; Поля литерала ищутся отдельными шаблонами: совпадения с одним @route сливаются, порядок полей любой.
; outry: import github.com/zeromicro/go-zero/rest

; []rest.Route{{Path: "/users/:id", …}}
(composite_literal
  type: (slice_type element: (qualified_type name: (type_identifier) @_t))
  body: (literal_value
    (literal_element
      (literal_value
        (keyed_element
          key: (literal_element (identifier) @_k)
          value: (literal_element (_) @path))) @route))
  (#eq? @_t "Route")
  (#eq? @_k "Path"))

(composite_literal
  type: (slice_type element: (qualified_type name: (type_identifier) @_t))
  body: (literal_value
    (literal_element
      (literal_value
        (keyed_element
          key: (literal_element (identifier) @_k)
          value: (literal_element (_) @method))) @route))
  (#eq? @_t "Route")
  (#eq? @_k "Method"))

(composite_literal
  type: (slice_type element: (qualified_type name: (type_identifier) @_t))
  body: (literal_value
    (literal_element
      (literal_value
        (keyed_element
          key: (literal_element (identifier) @_k)
          value: (literal_element (_) @handler))) @route))
  (#eq? @_t "Route")
  (#eq? @_k "Handler"))

; server.AddRoute(rest.Route{Method: …, Path: …, Handler: …})
(composite_literal
  type: (qualified_type name: (type_identifier) @_t)
  body: (literal_value
    (keyed_element
      key: (literal_element (identifier) @_k)
      value: (literal_element (_) @path))) @route
  (#eq? @_t "Route")
  (#eq? @_k "Path"))

(composite_literal
  type: (qualified_type name: (type_identifier) @_t)
  body: (literal_value
    (keyed_element
      key: (literal_element (identifier) @_k)
      value: (literal_element (_) @method))) @route
  (#eq? @_t "Route")
  (#eq? @_k "Method"))

(composite_literal
  type: (qualified_type name: (type_identifier) @_t)
  body: (literal_value
    (keyed_element
      key: (literal_element (identifier) @_k)
      value: (literal_element (_) @handler))) @route
  (#eq? @_t "Route")
  (#eq? @_k "Handler"))

; server.AddRoutes(routes, opts...) — опции действуют на роуты в аргументах
(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list) @group.body
  (#any-of? @_fn "AddRoutes" "AddRoute")) @group

; rest.WithPrefix("/v1")
(call_expression
  function: (selector_expression field: (field_identifier) @_fn)
  arguments: (argument_list
    (call_expression
      function: (selector_expression field: (field_identifier) @_opt)
      arguments: (argument_list . (_) @group.path)))
  (#any-of? @_fn "AddRoutes" "AddRoute")
  (#eq? @_opt "WithPrefix")) @group

; rest.WithJwt(secret), rest.WithSignature(…) — как middleware
(call_expression
  function: (selector_expression field: (field_identifier) @_fn)
  arguments: (argument_list
    (call_expression
      function: (selector_expression field: (field_identifier) @_opt)) @middleware)
  (#any-of? @_fn "AddRoutes" "AddRoute")
  (#any-of? @_opt "WithJwt" "WithJwtTransition" "WithSignature")) @group

; rest.WithMiddlewares([]rest.Middleware{a, b}, routes...)
(call_expression
  function: (selector_expression field: (field_identifier) @_fn)
  arguments: (argument_list
    .
    (composite_literal body: (literal_value (literal_element (_) @middleware)))) @group.body
  (#eq? @_fn "WithMiddlewares")) @group

; rest.WithMiddleware(m, routes...)
(call_expression
  function: (selector_expression field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @middleware) @group.body
  (#eq? @_fn "WithMiddleware")) @group
