Build a SQL-subset parser → AST → simple query planner project in Rust.

Layout:

```
src/
  lib.rs            - public API
  lexer.rs          - tokenizer
  parser.rs         - recursive descent → AST
  ast.rs            - AST types
  optimizer.rs      - simple constant-folding + predicate pushdown
  plan.rs           - logical plan tree + display
tests/
  parse.rs          - SQL → AST tests
  optimize.rs       - SQL → optimized plan tests
```

`Cargo.toml` deps:
- `thiserror = "1"`

(No sqlparser crate. Hand-roll.)

SQL subset:

```sql
-- Supported:
SELECT col1, col2, ... FROM table [WHERE pred] [ORDER BY col [ASC|DESC]] [LIMIT n]
SELECT col1, COUNT(*), SUM(col2) FROM table [WHERE pred] GROUP BY col1
SELECT * FROM t1 JOIN t2 ON t1.id = t2.fk [WHERE ...]
-- Predicates:
--   col = val, col != val, col < val, col > val, col <= val, col >= val
--   pred AND pred, pred OR pred, NOT pred
--   col IN (val1, val2, ...)
--   col IS NULL, col IS NOT NULL
-- Literals:
--   integer, string ('...'), boolean (TRUE/FALSE), NULL
```

AST types:

```rust
pub enum Stmt { Select(Select) }
pub struct Select { projections: Vec<Projection>, from: From, predicate: Option<Expr>,
                    group_by: Vec<Expr>, order_by: Vec<OrderBy>, limit: Option<u64> }
pub enum Projection { Star, Expr(Expr, Option<String>) }  // expr [AS alias]
pub enum From { Table(String), Join { left: Box<From>, right: String, on: Expr } }
pub enum Expr { Column(String), Literal(Value), Binary { op: BinOp, l: Box<Expr>, r: Box<Expr> },
                FunctionCall { name: String, args: Vec<Expr> }, Not(Box<Expr>),
                IsNull(Box<Expr>), IsNotNull(Box<Expr>), In(Box<Expr>, Vec<Expr>) }
pub enum BinOp { Eq, Ne, Lt, Gt, Le, Ge, And, Or, Add, Sub, Mul, Div }
pub enum Value { Int(i64), Str(String), Bool(bool), Null }
```

Public API:

```rust
pub fn parse(sql: &str) -> Result<Stmt, ParseError>;
pub fn plan(stmt: Stmt) -> LogicalPlan;
pub fn optimize(plan: LogicalPlan) -> LogicalPlan;  // constant fold + predicate pushdown
```

LogicalPlan ops: `Scan`, `Filter`, `Project`, `Sort`, `Limit`, `Join`, `Aggregate`.

Tests:
- `test_parse_simple_select`
- `test_parse_select_with_where_compound_pred`
- `test_parse_select_with_in_list`
- `test_parse_select_with_join_on`
- `test_parse_select_with_group_by_count`
- `test_parse_error_missing_from`
- `test_plan_simple_select_yields_scan_project`
- `test_optimize_constant_fold` — `WHERE 1 = 1 AND col > 5` reduces to `WHERE col > 5`
- `test_optimize_predicate_pushdown_into_join` — predicate referencing only one side
  pushed down to that side's Filter before the Join

`cargo check` clean, `cargo test` all pass.
