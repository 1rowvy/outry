; Стандартный net/http: mux.HandleFunc("GET /users/{id}", h) (Go 1.22+) и http.HandleFunc("/path", h).
; Метод — из начала шаблона пути, без метода — любой.
; outry: import net/http

(call_expression
  function: (selector_expression
    operand: (_) @receiver
    field: (field_identifier) @_fn)
  arguments: (argument_list . (_) @path . (_) @handler .)
  (#any-of? @_fn "Handle" "HandleFunc")) @route
