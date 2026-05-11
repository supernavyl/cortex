Build a DPLL SAT solver in Rust with unit propagation, pure-literal elimination,
and 2-watched-literal data structure.

Implement in `src/lib.rs`:

```rust
/// Variables are u32 (1-indexed; 0 is reserved/unused).
pub type Var = u32;

/// A literal: positive = var, negative = !var. Use i32 (never 0).
pub type Lit = i32;

/// A clause is a Vec of literals (disjunction).
pub type Clause = Vec<Lit>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Sat(Vec<i8>),  // length = num_vars+1; index 0 is unused; values -1, 0, or 1
    Unsat,
    Timeout,
}

pub struct Solver { /* private */ }

impl Solver {
    pub fn new(num_vars: u32, clauses: Vec<Clause>) -> Self;

    /// Parse standard DIMACS CNF format from a string.
    /// Returns Err for malformed input.
    pub fn from_dimacs(s: &str) -> Result<Self, String>;

    /// Solve, with optional decision/conflict budget. 0 = unbounded.
    pub fn solve(&mut self, budget: u64) -> Verdict;
}
```

Implementation rules:

- 2-watched-literals — each clause maintains 2 watched literals; propagation
  only inspects clauses where a watched literal becomes false
- Unit propagation runs to fixed point after each decision
- Pure literal elimination is applied once at the start (before any decision)
- Branching heuristic can be VSIDS, DLIS, or just "first unassigned" — keep it simple
- Recursive DPLL with explicit undo on backtrack

Tests:

- `test_trivial_sat` — single clause [1] → Sat with var 1 = true
- `test_trivial_unsat` — clauses [[1], [-1]] → Unsat
- `test_unit_propagation` — clauses [[1, 2], [-1]] → Sat with var 1 = false, var 2 = true
- `test_pure_literal_assigns_immediately` — variable that appears only positively
  must be assigned true in the result
- `test_3sat_known_sat` — a small 3SAT formula known to be SAT; verify model
  satisfies every clause (write a model-checker helper)
- `test_3sat_known_unsat` — pigeonhole 3-into-2 encoded as CNF → Unsat
- `test_dimacs_parser_round_trip` — parse minimal DIMACS, solve, sanity-check
- `test_budget_timeout` — solve a hard pigeonhole 7-into-6 with budget=10 → Timeout

`cargo check` clean, `cargo test` all pass.
