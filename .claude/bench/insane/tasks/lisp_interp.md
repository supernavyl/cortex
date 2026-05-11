Build a minimal Lisp interpreter in Rust: lex → parse → eval.

Implement in `src/lib.rs`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Sym(String),
    Cons(std::rc::Rc<Value>, std::rc::Rc<Value>),  // cons cell
    Lambda(Vec<String>, std::rc::Rc<Value>, Env),  // params, body, captured env
    Builtin(String),  // tag for primitive functions
}

#[derive(Debug, Clone, Default)]
pub struct Env { /* private; supports nested scopes via Rc<RefCell<Frame>> */ }

#[derive(Debug)]
pub enum LispError {
    Lex(String),
    Parse(String),
    Undefined(String),
    TypeMismatch(String),
    ArityMismatch { expected: usize, got: usize },
    Custom(String),
}

impl std::fmt::Display for LispError;
impl std::error::Error for LispError;

/// Evaluate a source string; returns the final Value.
/// Env is fresh with all builtins installed.
pub fn eval(src: &str) -> Result<Value, LispError>;

/// Evaluate with a caller-provided Env (for REPL-like usage).
pub fn eval_with_env(src: &str, env: &mut Env) -> Result<Value, LispError>;
```

Must support:

- Special forms: `define`, `lambda`, `let`, `if`, `cond`, `quote`, `set!`
- Builtins: `+ - * / = < > <= >=`, `cons car cdr list null? pair? eq?`, `display`
- Integer arithmetic only (no floats)
- Lexical scoping — `lambda` captures the env at definition time
- Proper recursion (factorial 10 should work; no need to optimize tail-calls)

Tests:

- `test_eval_integer` — eval("42") → Int(42)
- `test_arithmetic` — eval("(+ 1 2 3)") → Int(6)
- `test_define_then_use` — eval("(define x 5)(* x x)") → Int(25)
- `test_lambda_basic` — eval("((lambda (x) (* x x)) 7)") → Int(49)
- `test_closures_capture_lexical_env` — eval defines y=10, then a lambda that
  uses y, then re-binds y=99, then calls the lambda → must still see y=10
- `test_factorial` — eval defines fact recursively, calls (fact 10) → Int(3628800)
- `test_cons_car_cdr` — eval("(car (cons 1 2))") → Int(1); ("(cdr (cons 1 2))") → Int(2)
- `test_undefined_returns_error` — eval("(+ x 1)") → Err(Undefined("x"))
- `test_arity_mismatch_error` — eval("((lambda (x y) x) 1)") → Err(ArityMismatch)
- `test_quote_returns_form_unevaluated` — eval("(quote (1 2 3))") → cons list

`cargo check` clean, `cargo test` all pass.
