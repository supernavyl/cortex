Build a jq-clone JSON query language interpreter project in Rust.

Layout:

```
src/
  main.rs           - CLI: reads stdin or file, applies filter, writes stdout
  lib.rs            - public API
  lexer.rs          - tokenizer
  parser.rs         - recursive descent → AST
  ast.rs            - filter AST types
  eval.rs           - apply Filter to serde_json::Value
  error.rs          - error types
tests/
  filters.rs        - many small filter→input→expected_output cases
```

`Cargo.toml` deps:
- `clap = { version = "4", features = ["derive"] }`
- `serde_json = "1"`
- `thiserror = "1"`
- `anyhow = "1"`

CLI:
- `jq '<filter>'`              — read JSON from stdin, apply filter, write to stdout
- `jq '<filter>' file.json`    — read from file
- `--raw-output / -r`          — strings emitted without quotes
- `--compact-output / -c`      — single-line output

Filters supported (subset of real jq):

- `.`                  — identity
- `.foo`               — object index
- `.foo.bar`           — chained index
- `.[0]`               — array index
- `.[]`                — iterate array (emits one stream per element)
- `.foo // "default"`  — alternative (use right side if left is null/missing)
- `length`             — array/object/string length
- `keys`               — sorted keys of an object
- `select(.x > 5)`     — filter elements; supports `==`, `!=`, `<`, `>`, `<=`, `>=`, `and`, `or`, `not`
- `map(F)`             — apply F to each element of an array
- `, (comma)`          — concatenate streams from two filters
- `|  (pipe)`          — pass output of left as input to right
- `to_entries / from_entries`
- Numbers, strings, booleans, null literals

Public API (`lib.rs`):

```rust
pub fn run(filter: &str, input: &serde_json::Value) -> Result<Vec<serde_json::Value>, JqError>;
pub fn parse(filter: &str) -> Result<ast::Filter, JqError>;
pub fn eval(f: &ast::Filter, v: &serde_json::Value) -> Result<Vec<serde_json::Value>, JqError>;
```

Tests (at least these):
- `test_identity`
- `test_field_access`
- `test_chained_field_access`
- `test_array_index_and_iter`
- `test_pipe_composes_filters`
- `test_select_with_comparison`
- `test_map_doubles_numbers`
- `test_length_on_array_string_object`
- `test_alternative_default_when_missing`
- `test_parse_error_unclosed_paren`
- `test_to_from_entries_roundtrip`

`cargo check` clean, `cargo test` all pass.
