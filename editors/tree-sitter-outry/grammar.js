/**
 * Tree-sitter grammar for Outry `*.outry` request files — for highlighting in Neovim, Helix, Zed.
 * The format is specified in docs/src/content/docs/reference/outry-format.mdx; the reference parser
 * (and every error message) is crates/outry-core/src/lang/parse.rs. This grammar is more lenient:
 * it only has to colour valid files and survive half-typed ones.
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

const PREC = {
  lambda: -1,
  or: 1,
  and: 2,
  test: 3, // in, matches
  equal: 4,
  compare: 5,
  add: 6,
  multiply: 7,
  unary: 8,
  postfix: 9,
};

const IDENT = /[\p{L}_][\p{L}\p{N}_]*/u;

/** Elements separated by commas or new lines (new lines are extras), trailing comma allowed. */
const list = (rule) => repeat(seq(rule, optional(',')));

module.exports = grammar({
  name: 'outry',

  extras: ($) => [/\s/, $.comment],

  word: ($) => $.identifier,


  rules: {
    source_file: ($) => repeat($._item),

    _item: ($) => choice($.request, $.flow, $.shape_declaration, $.let_declaration),

    comment: (_) =>
      token(choice(seq('//', /[^\n]*/), seq('/*', /[^*]*\*+([^/*][^*]*\*+)*/, '/'))),

    // ---------- requests ----------

    // `Login: POST /login { … }` — the name is optional (older files name requests by comment).
    request: ($) =>
      seq(
        optional(seq(field('name', $.identifier), token.immediate(':'))),
        field('method', $.method),
        field('target', $._target),
        optional($.request_block),
      ),

    // Wins over `identifier` for all-caps words: `GET` is a method, not a request name.
    method: (_) => token(prec(1, /[A-Z][A-Z0-9_-]+/)),

    _target: ($) => choice($.path, $.url, $.string),

    // `/orders/{shop}?x=${y}`: one token without spaces, with parameters and interpolations.
    path: ($) => seq(alias(token(seq('/', /[^\s{$]*/)), $.path_text), repeat($._path_part)),
    url: ($) =>
      seq(alias(token(seq(/https?:\/\//, /[^\s{$]*/)), $.path_text), repeat($._path_part)),
    _path_part: ($) =>
      choice(
        alias(token.immediate(/[^\s{$]+/), $.path_text),
        $.path_param,
        alias($._path_interpolation, $.interpolation),
        alias(token.immediate('$'), $.path_text),
      ),
    path_param: ($) =>
      seq(token.immediate('{'), alias(token.immediate(IDENT), $.identifier), token.immediate('}')),
    _path_interpolation: ($) => seq(token.immediate(prec(2, '${')), $._expression, '}'),

    request_block: ($) => seq('{', repeat(choice($._field, ',')), '}'),

    _field: ($) =>
      choice(
        $.handler_field,
        $.params_field,
        $.only_field,
        $.flag_field,
        $.duration_field,
        $.entries_field,
        $.body_field,
        $.poll_statement,
        $.expect_block,
        $.save_statement,
      ),

    handler_field: ($) =>
      seq('handler', ':', field('value', alias($._dotted, $.handler))),
    _dotted: ($) => seq($.identifier, repeat(seq('.', $.identifier))),

    params_field: ($) => seq('params', '{', list($.param), '}'),
    param: ($) => seq(field('name', $.identifier), optional(seq(':', field('default', $._expression)))),

    only_field: ($) => seq('only', ':', '[', list($.identifier), ']'),

    flag_field: ($) => seq(field('name', choice('confirm', 'redirects')), ':', choice($.true, $.false)),

    duration_field: ($) => seq(field('name', choice('timeout', 'cache')), ':', $.duration),

    entries_field: ($) =>
      seq(field('name', choice('query', 'headers', 'form', 'multipart')), '{', list($.entry), '}'),
    entry: ($) =>
      seq(field('key', choice(alias($.header_name, $.property), $.string)), optional(seq(':', field('value', $._expression)))),
    header_name: (_) => /[\p{L}_][\p{L}\p{N}_]*(-[\p{L}\p{N}_]+)*/u,

    body_field: ($) => seq('body', field('value', $._expression)),

    poll_statement: ($) =>
      seq('poll', field('condition', $._expression), 'every', $.duration, 'for', $.duration),

    expect_block: ($) => seq('expect', '{', list($._expression), '}'),

    save_statement: ($) => seq('save', field('name', $.identifier), '=', field('value', $._expression)),

    // ---------- flows, shapes, lets ----------

    flow: ($) =>
      seq(
        'flow',
        field('name', $.identifier),
        '{',
        repeat(choice($.params_field, $.expect_block, $.save_statement, $.flow_binding, $._expression, ',')),
        '}',
      ),
    flow_binding: ($) => seq(field('name', $.identifier), '=', field('value', $._expression)),

    let_declaration: ($) => seq('let', field('name', $.identifier), '=', field('value', $._expression)),

    shape_declaration: ($) =>
      seq('shape', field('name', $.identifier), choice($.shape_object, seq('=', $._shape))),

    _shape: ($) => choice($.shape_union, $._shape_term),
    shape_union: ($) => prec.left(seq($._shape, '|', $._shape_term)),
    _shape_term: ($) =>
      choice(
        alias($.identifier, $.type),
        $.shape_object,
        $.shape_array,
        $.string,
        $.number,
        $.true,
        $.false,
        $.null,
        $.schema,
      ),
    shape_object: ($) => seq('{', list($.shape_field), '}'),
    shape_field: ($) =>
      seq(field('name', choice(alias($.identifier, $.property), $.string)), optional('?'), ':', field('type', $._shape)),
    shape_array: ($) => seq('[', $._shape, ']'),
    schema: ($) => seq('schema', '(', $.string, ')'),

    // ---------- expressions ----------

    _expression: ($) =>
      choice($._primary, $.unary_expression, $.binary_expression, $.matches_expression, $.lambda),

    _primary: ($) =>
      choice(
        $.identifier,
        $.number,
        $.string,
        $.true,
        $.false,
        $.null,
        $.array,
        $.object,
        $.member_expression,
        $.index_expression,
        $.call_expression,
        $.parenthesized_expression,
      ),

    parenthesized_expression: ($) => seq('(', $._expression, ')'),

    member_expression: ($) =>
      prec(PREC.postfix, seq(field('object', $._primary), '.', field('property', alias($.header_name, $.property)))),

    index_expression: ($) =>
      prec(PREC.postfix, seq(field('object', $._primary), '[', field('index', $._expression), ']')),

    // `Login(email: "a")`, `users.Create()`, `uuid()`, `body.items.map(i => i.id)`.
    call_expression: ($) =>
      prec(
        PREC.postfix,
        seq(field('function', choice($.identifier, $.member_expression)), $.arguments),
      ),
    arguments: ($) => seq(token.immediate('('), list(choice($.argument, $._expression)), ')'),
    argument: ($) => seq(field('name', $.identifier), ':', field('value', $._expression)),

    lambda: ($) => prec.right(PREC.lambda, seq(field('parameter', $.identifier), '=>', field('body', $._expression))),

    unary_expression: ($) =>
      prec(PREC.unary, seq(field('operator', choice('!', '-', 'typeof', 'fresh')), field('argument', $._expression))),

    binary_expression: ($) => {
      const table = [
        [PREC.or, '||'],
        [PREC.and, '&&'],
        [PREC.test, 'in'],
        [PREC.equal, choice('==', '!=')],
        [PREC.compare, choice('<', '<=', '>', '>=')],
        [PREC.add, choice('+', '-')],
        [PREC.multiply, choice('*', '/', '%')],
      ];
      return choice(
        ...table.map(([p, op]) =>
          prec.left(p, seq(field('left', $._expression), field('operator', op), field('right', $._expression))),
        ),
      );
    },

    matches_expression: ($) =>
      prec.left(PREC.test, seq(field('left', $._expression), 'matches', field('pattern', choice($.regex, $._shape)))),

    regex: (_) => token(seq('/', /([^/\\\n]|\\.)+/, '/', /[a-z]*/)),

    array: ($) => seq('[', list($._expression), ']'),
    object: ($) => seq('{', list(choice($.pair, $.identifier)), '}'),
    pair: ($) =>
      seq(field('key', choice(alias($.identifier, $.property), $.string)), ':', field('value', $._expression)),

    string: ($) =>
      choice(
        seq('"""', repeat(choice($._triple_content, $.escape_sequence, $.interpolation)), '"""'),
        seq('"', repeat(choice($._double_content, $.escape_sequence, $.interpolation)), token.immediate('"')),
        seq("'", repeat(choice($._single_content, $.escape_sequence, $.interpolation)), token.immediate("'")),
      ),
    _double_content: (_) => alias(token.immediate(prec(1, /([^"\\$\n]|\$[^{"\\\n])+|\$/)), 'string_content'),
    _single_content: (_) => alias(token.immediate(prec(1, /([^'\\$\n]|\$[^{'\\\n])+|\$/)), 'string_content'),
    _triple_content: (_) => token.immediate(prec(1, /([^"\\$]|"[^"]|""[^"]|\$[^{])+|\$|"/)),
    escape_sequence: (_) => token.immediate(seq('\\', choice(/u\{[0-9a-fA-F]+\}/, /[^u]/))),
    interpolation: ($) => seq(token.immediate(prec(2, '${')), $._expression, '}'),

    number: (_) => /\d+(\.\d+)?([eE][+-]?\d+)?/,
    duration: (_) => /\d+(\.\d+)?(ms|s|m|h)/,
    true: (_) => 'true',
    false: (_) => 'false',
    null: (_) => 'null',

    identifier: (_) => IDENT,
  },
});
