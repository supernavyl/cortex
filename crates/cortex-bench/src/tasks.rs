//! Benchmark task registry.

/// A single coding task for the benchmark.
#[derive(Debug, Clone)]
pub struct BenchTask {
    pub name: &'static str,
    pub prompt: &'static str,
    pub language: &'static str,
    /// Rough lower bound on lines expected in the output file.
    pub expected_min_lines: u32,
}

/// All benchmark tasks.
pub static ALL_TASKS: &[BenchTask] = &[
    BenchTask {
        name: "hello_fn",
        prompt: "Create src/hello.py: a function greet(name: str) -> str that returns f'Hello, {name}!'",
        language: "python",
        expected_min_lines: 2,
    },
    BenchTask {
        name: "fizzbuzz",
        prompt: "Create src/fizzbuzz.py: a function fizzbuzz(n: int) -> list[str] that returns the classic FizzBuzz list for 1..=n (Fizz for multiples of 3, Buzz for multiples of 5, FizzBuzz for both, else the number as string)",
        language: "python",
        expected_min_lines: 8,
    },
    BenchTask {
        name: "string_utils",
        prompt: "Create src/string_utils.py: five functions with full type annotations — reverse_words(s: str) -> str (reverses word order), is_palindrome(s: str) -> bool (case-insensitive, strips spaces), count_vowels(s: str) -> int, title_case(s: str) -> str, truncate(s: str, n: int) -> str (truncates with '...' if longer than n)",
        language: "python",
        expected_min_lines: 20,
    },
    BenchTask {
        name: "stack",
        prompt: "Create src/stack.py: a generic class Stack[T] with methods push(item: T) -> None, pop() -> T | None, peek() -> T | None, is_empty() -> bool, size() -> int, clear() -> None. Use Python generics (from __future__ import annotations or TypeVar). Full type annotations on all methods.",
        language: "python",
        expected_min_lines: 30,
    },
    BenchTask {
        name: "lru_cache",
        prompt: "Create src/lru_cache.py: class LRUCache with __init__(self, capacity: int), get(self, key: int) -> int (returns -1 if key is missing or evicted), put(self, key: int, value: int) -> None. Implement using collections.OrderedDict for O(1) operations. Full type annotations.",
        language: "python",
        expected_min_lines: 25,
    },
    BenchTask {
        name: "resp_protocol",
        prompt: "Create src/protocol.py: implement the Redis RESP (REdis Serialization Protocol) wire format. Include: parse_resp(data: bytes) -> tuple[Any, int] (returns value and bytes consumed), serialize_resp(value: Any) -> bytes, class RespError(Exception). Handle all five RESP types: +simple strings, -errors, :integers, $bulk strings (including $-1 null bulk), *arrays (including *-1 null array). Use from __future__ import annotations and full type hints.",
        language: "python",
        expected_min_lines: 80,
    },
    BenchTask {
        name: "kv_store",
        prompt: "Create src/store.py: class KVStore with methods set(self, k: str, v: bytes, ex: int | None = None) -> None (ex = TTL in seconds), get(self, k: str) -> bytes | None (returns None if missing or expired), delete(self, k: str) -> int (returns count deleted), exists(self, k: str) -> bool, keys(self, pattern: str = '*') -> list[str] (fnmatch pattern matching, excludes expired), flush(self) -> None. Thread-safe using threading.Lock, TTL via time.monotonic(). Full type annotations.",
        language: "python",
        expected_min_lines: 70,
    },
    BenchTask {
        name: "async_worker",
        prompt: "Create src/worker.py: class AsyncWorker with __init__(self, max_workers: int = 4), async def start(self) -> None, async def stop(self) -> None, async def submit(self, coro: Coroutine[Any, Any, Any]) -> asyncio.Task[Any], property running: bool, property pending: int. Uses asyncio.Queue internally to process coroutines concurrently up to max_workers. Full type annotations including from __future__ import annotations and from collections.abc import Coroutine.",
        language: "python",
        expected_min_lines: 60,
    },
];

/// Quick task set (first 3 tasks only — for fast smoke testing).
pub fn quick_tasks() -> &'static [BenchTask] {
    &ALL_TASKS[..3]
}

/// Look up a task by name.
pub fn find_task(name: &str) -> Option<&'static BenchTask> {
    ALL_TASKS.iter().find(|t| t.name == name)
}
