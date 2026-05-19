//! Benchmark task registry.
//!
//! All tasks are stdlib-only Rust (no external dependencies) so the bench
//! workspace can be a minimal `Cargo.toml` + `src/lib.rs` with no `cargo
//! fetch` round-trip. The runner bootstraps the workspace and pre-registers
//! `pub mod <task_name>;` in `lib.rs` so each task's output participates in
//! the `cargo check` verification.

/// A single coding task for the benchmark.
#[derive(Debug, Clone)]
pub struct BenchTask {
    pub name: &'static str,
    pub prompt: &'static str,
    /// Output language. ADR-005 binds this to "rust" — kept as a field for
    /// future filtering / reporting, not consumed by the runner today.
    #[allow(dead_code)]
    pub language: &'static str,
    /// Rough lower bound on lines expected in the output file.
    pub expected_min_lines: u32,
}

/// All benchmark tasks. Stdlib-only Rust by design (ADR-005).
pub static ALL_TASKS: &[BenchTask] = &[
    // ── Easy tier ────────────────────────────────────────────────────────────
    BenchTask {
        name: "hello_fn",
        prompt: "Create src/hello_fn.rs: a public function `greet(name: &str) -> String` that returns the string \"Hello, <name>!\" using format!. Stdlib only. No external crates.",
        language: "rust",
        expected_min_lines: 3,
    },
    BenchTask {
        name: "fizzbuzz",
        prompt: "Create src/fizzbuzz.rs: a public function `fizzbuzz(n: u32) -> Vec<String>` that returns the classic FizzBuzz list for 1..=n (\"Fizz\" for multiples of 3, \"Buzz\" for multiples of 5, \"FizzBuzz\" for both, else the number as string). Stdlib only.",
        language: "rust",
        expected_min_lines: 10,
    },
    // ── Medium tier ──────────────────────────────────────────────────────────
    BenchTask {
        name: "string_utils",
        prompt: "Create src/string_utils.rs: five public functions, all stdlib-only — `reverse_words(s: &str) -> String` (reverses word order, single-space separated), `is_palindrome(s: &str) -> bool` (case-insensitive, ignores ASCII whitespace), `count_vowels(s: &str) -> usize` (a/e/i/o/u, case-insensitive), `title_case(s: &str) -> String` (first letter of each word uppercased, rest lowercased), `truncate(s: &str, n: usize) -> String` (returns s unchanged if shorter than n, else cuts to n-3 chars and appends \"...\").",
        language: "rust",
        expected_min_lines: 30,
    },
    BenchTask {
        name: "stack",
        prompt: "Create src/stack.rs: a generic `pub struct Stack<T>` with methods `new() -> Self`, `push(&mut self, item: T)`, `pop(&mut self) -> Option<T>`, `peek(&self) -> Option<&T>`, `is_empty(&self) -> bool`, `len(&self) -> usize`, `clear(&mut self)`. Implement `Default` for Stack<T>. Add `#[cfg(test)] mod tests` with at least 3 assertions covering push/pop ordering, peek-after-push, and is_empty transitions.",
        language: "rust",
        expected_min_lines: 50,
    },
    BenchTask {
        name: "lru_cache",
        prompt: "Create src/lru_cache.rs: `pub struct LruCache<K: Eq + std::hash::Hash + Clone, V>` with `new(capacity: usize) -> Self`, `get(&mut self, key: &K) -> Option<&V>` (also marks key as most-recently-used), `put(&mut self, key: K, value: V) -> Option<V>` (returns evicted value if any), `len(&self) -> usize`, `contains(&self, key: &K) -> bool`. Use `std::collections::HashMap` + a doubly-linked-list-by-indices OR `std::collections::VecDeque<K>` for ordering. Capacity 0 is allowed and rejects all puts. Stdlib only.",
        language: "rust",
        expected_min_lines: 70,
    },
    BenchTask {
        name: "trie",
        prompt: "Create src/trie.rs: `pub struct Trie` with `new() -> Self`, `insert(&mut self, word: &str)`, `contains(&self, word: &str) -> bool`, `starts_with(&self, prefix: &str) -> bool`, `delete(&mut self, word: &str) -> bool` (returns false if word wasn't present), `words_with_prefix(&self, prefix: &str) -> Vec<String>` (sorted), `len(&self) -> usize` (number of distinct words). Use a node struct with `std::collections::HashMap<char, TrieNode>` children. Stdlib only.",
        language: "rust",
        expected_min_lines: 80,
    },
    // ── Hard tier ────────────────────────────────────────────────────────────
    BenchTask {
        name: "expr_evaluator",
        prompt: "Create src/expr_evaluator.rs: a stdlib-only arithmetic expression interpreter. Define `pub enum Token { Num(f64), Plus, Minus, Star, Slash, LParen, RParen }`, `pub enum Expr { Lit(f64), Bin { op: Op, left: Box<Expr>, right: Box<Expr> } }` and `pub enum Op { Add, Sub, Mul, Div }`. Implement `pub fn tokenize(s: &str) -> Result<Vec<Token>, ExprError>`, `pub fn parse(tokens: &[Token]) -> Result<Expr, ExprError>` (recursive-descent with correct +,-,*,/ precedence and parentheses), and `pub fn evaluate(expr: &Expr) -> Result<f64, ExprError>`. `pub struct ExprError(pub String);` with std::fmt::Display + std::error::Error. Reject division by zero with an error variant. Stdlib only.",
        language: "rust",
        expected_min_lines: 130,
    },
    BenchTask {
        name: "resp_protocol",
        prompt: "Create src/resp_protocol.rs: Redis RESP wire format. Define `pub enum RespValue { SimpleString(String), Error(String), Integer(i64), BulkString(Option<Vec<u8>>), Array(Option<Vec<RespValue>>) }`. Implement `pub fn parse(data: &[u8]) -> Result<(RespValue, usize), RespError>` (returns value + bytes consumed) and `pub fn serialize(value: &RespValue) -> Vec<u8>`. Handle null bulk string (`$-1\\r\\n`) and null array (`*-1\\r\\n`). `pub struct RespError(pub String);` with Display + Error. Stdlib only — no external crates.",
        language: "rust",
        expected_min_lines: 120,
    },
    BenchTask {
        name: "consistent_hash",
        prompt: "Create src/consistent_hash.rs: `pub struct ConsistentHashRing` with `new() -> Self`, `add_node(&mut self, node: &str, virtual_nodes: usize)`, `remove_node(&mut self, node: &str)`, `get_node(&self, key: &str) -> Option<String>` (returns None if ring is empty), `get_nodes(&self, key: &str, n: usize) -> Vec<String>` (n distinct real nodes in preference order; returns fewer if ring has fewer real nodes). Use std::collections::BTreeMap<u64, String> for the ring and std::hash::{Hasher, DefaultHasher} for positions. Stdlib only — no external hashlib crate.",
        language: "rust",
        expected_min_lines: 90,
    },
    BenchTask {
        name: "graph_algos",
        prompt: "Create src/graph_algos.rs: `pub struct Graph { directed: bool, ... }` with `new(directed: bool) -> Self`, `add_edge(&mut self, u: u32, v: u32, weight: f64)`, `bfs(&self, start: u32) -> Vec<u32>` (visit order), `dfs(&self, start: u32) -> Vec<u32>` (visit order), `has_cycle(&self) -> bool` (works for both directed and undirected), `dijkstra(&self, start: u32) -> std::collections::HashMap<u32, f64>` (shortest distances, f64::INFINITY for unreachable; reject negative weights with a panic). Adjacency list using `HashMap<u32, Vec<(u32, f64)>>`. Stdlib only.",
        language: "rust",
        expected_min_lines: 130,
    },
];

/// Quick task set (first 3 tasks — for fast smoke testing).
pub fn quick_tasks() -> &'static [BenchTask] {
    &ALL_TASKS[..3]
}

/// Look up a task by name.
pub fn find_task(name: &str) -> Option<&'static BenchTask> {
    ALL_TASKS.iter().find(|t| t.name == name)
}
