Build an NFA-based regex engine in Rust.

Implement in `src/lib.rs`:

```rust
#[derive(Debug)]
pub enum RegexError {
    Parse(String),
    UnsupportedFeature(String),
}

impl std::fmt::Display for RegexError;
impl std::error::Error for RegexError;

pub struct Regex { /* private — owns compiled NFA */ }

impl Regex {
    /// Compile a pattern.
    pub fn new(pattern: &str) -> Result<Self, RegexError>;

    /// True if the regex matches the input anywhere (substring).
    pub fn is_match(&self, input: &str) -> bool;

    /// Return the first matching substring (start, end) byte offsets, or None.
    pub fn find(&self, input: &str) -> Option<(usize, usize)>;

    /// Return all non-overlapping matches.
    pub fn find_all(&self, input: &str) -> Vec<(usize, usize)>;
}
```

Must support:

- Literals: `a`, `b`, ...
- Concatenation: `ab`
- Alternation: `a|b`
- Kleene star: `a*`
- Plus: `a+` (= `aa*`)
- Optional: `a?`
- Grouping: `(ab)+`
- Character classes: `[a-z]`, `[^0-9]`, negation works inside ranges
- Anchors: `^` and `$`
- Escapes: `\.`, `\*`, `\\`, `\d` (= `[0-9]`), `\w` (= `[A-Za-z0-9_]`), `\s`
- ASCII-only input is fine; full Unicode support not required

Approach: build an NFA via Thompson's construction, then do simulation with
ε-closures. No backtracking. No need for an explicit DFA conversion.

No `regex` crate. Stdlib only. (`regex` may be a `[dev-dependency]` for cross-check.)

Tests:

- `test_literal` — Regex::new("abc").is_match("xxabcyy") → true
- `test_alternation` — "a|b" matches "a" and "b" but not "c"
- `test_star` — "ab*c" matches "ac", "abc", "abbbbc"
- `test_plus_requires_one` — "ab+c" matches "abc" but not "ac"
- `test_optional` — "ab?c" matches "ac" and "abc" but not "abbc"
- `test_char_class` — "[a-z]+" matches "hello" but not "ABC"
- `test_negated_class` — "[^0-9]+" matches "abc" but not "123"
- `test_anchors` — "^abc$" matches "abc" exactly but not "xabc" or "abcx"
- `test_grouping_with_plus` — "(ab)+" matches "ababab"
- `test_escape_dot_literal` — "a\\.b" matches "a.b" but not "axb"
- `test_find_returns_offsets` — find("abc", "xxabcyy") → Some((2, 5))
- `test_find_all_non_overlapping` — find_all("aa", "aaaaa") → [(0,2), (2,4)]
- `test_parse_error_on_unclosed_group` — Regex::new("(abc") → Err(Parse(_))

`cargo check` clean, `cargo test` all pass.
