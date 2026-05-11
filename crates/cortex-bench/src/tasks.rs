//! Benchmark task registry.

/// A single coding task for the benchmark.
#[derive(Debug, Clone)]
pub struct BenchTask {
    pub name: &'static str,
    pub prompt: &'static str,
    // Reserved for per-language filtering — not yet consumed by the runner.
    #[allow(dead_code)]
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
    // ── Hard tier ────────────────────────────────────────────────────────────
    BenchTask {
        name: "expr_eval",
        prompt: "Create src/expr_eval.py: a complete expression evaluator. Implement: (1) tokenize(expr: str) -> list[Token] where Token is a dataclass with kind: str ('NUM','OP','LPAREN','RPAREN') and value: str|float; (2) parse(tokens: list[Token]) -> ASTNode where ASTNode is a dataclass representing a binary tree (left/right/op/value fields, with op=None for leaf numbers); (3) evaluate(node: ASTNode) -> float. Support +, -, *, / with correct precedence and parentheses. Raise ExprError(Exception) on invalid input. No use of eval(). Full type annotations.",
        language: "python",
        expected_min_lines: 120,
    },
    BenchTask {
        name: "trie",
        prompt: "Create src/trie.py: class Trie with insert(word: str) -> None, search(word: str) -> bool, starts_with(prefix: str) -> bool, delete(word: str) -> bool (returns False if word not found), words_with_prefix(prefix: str) -> list[str] (all words starting with prefix, sorted), count_words() -> int. Internal TrieNode class. Full type annotations.",
        language: "python",
        expected_min_lines: 90,
    },
    BenchTask {
        name: "graph_algos",
        prompt: "Create src/graph.py: class Graph with add_edge(u: int, v: int, weight: float = 1.0) -> None, bfs(start: int) -> list[int], dfs(start: int) -> list[int], has_cycle() -> bool (works for directed graphs), dijkstra(start: int) -> dict[int, float] (shortest distances from start, math.inf for unreachable). Internal adjacency list. Full type annotations.",
        language: "python",
        expected_min_lines: 120,
    },
    BenchTask {
        name: "mini_http_parser",
        prompt: "Create src/http_parser.py: (1) dataclass HttpRequest with method:str, path:str, version:str, headers:dict[str,str], body:bytes; (2) parse_request(raw: bytes) -> HttpRequest that parses a raw HTTP/1.1 request (handle CRLF line endings, header folding not required); (3) class Router with route(method:str, path:str) -> Callable decorator, dispatch(request:HttpRequest) -> tuple[int,str] (status_code, body_str) calling the matching handler or returning 404; (4) class HttpError(Exception) with status_code:int. Full type annotations.",
        language: "python",
        expected_min_lines: 130,
    },
    BenchTask {
        name: "consistent_hash",
        prompt: "Create src/consistent_hash.py: class ConsistentHashRing with add_node(node: str, virtual_nodes: int = 150) -> None, remove_node(node: str) -> None, get_node(key: str) -> str | None (returns None if ring is empty), get_nodes(key: str, n: int) -> list[str] (n distinct nodes in preference order for replication). Uses hashlib.md5 for ring positions. Full type annotations. Raise ValueError if n > number of real nodes in get_nodes.",
        language: "python",
        expected_min_lines: 80,
    },
    BenchTask {
        name: "lis_scale",
        prompt: "Build a production-ready local AI assistant daemon. Create these 8 files: (1) src/server.py: FastAPI app with POST /chat endpoint returning streaming SSE response, uses ConversationMemory and LLMClient; (2) src/memory.py: class ConversationMemory(max_history:int=50) with add_turn(role:str,content:str), get_history(n:int|None=None)->list[dict], clear(), __len__; (3) src/llm.py: class LLMClient(base_url:str,model:str) with async chat(messages:list[dict])->str calling Ollama /api/chat via httpx, async stream_chat(messages)->AsyncGenerator[str,None]; (4) src/plugins/base.py: abstract class Plugin with name:str property, description:str property, async execute(inp:str)->str; (5) src/plugins/calculator.py: Calculator(Plugin) evaluating math expressions safely using ast; (6) src/config.py: Config dataclass (model:str, host:str='0.0.0.0', port:int=8000, max_history:int=50) with classmethod from_toml(path:str)->Config; (7) src/main.py: argparse CLI --host --port --model --config, starts uvicorn; (8) tests/test_memory.py: pytest tests for ConversationMemory covering add/get/clear/overflow/len.",
        language: "python",
        expected_min_lines: 300,
    },
    // ── XL tier — full project (25 files) ────────────────────────────────────
    BenchTask {
        name: "full_project",
        prompt: "\
Build a complete production-grade task management REST API. \
Create EXACTLY these 25 files with full Python type annotations throughout:\n\
(1) src/__init__.py — empty;\n\
(2) src/main.py — FastAPI app factory create_app()->FastAPI, mounts all routers, CORS, exception handlers, lifespan context manager (creates tables on startup);\n\
(3) src/config.py — Settings(BaseSettings) with DATABASE_URL:str='sqlite:///./tasks.db', SECRET_KEY:str='change-me', ACCESS_TOKEN_EXPIRE_MINUTES:int=30, CORS_ORIGINS:list[str]=['*']; get_settings()->Settings cached with lru_cache;\n\
(4) src/database.py — SQLAlchemy async engine, AsyncSessionLocal, Base, get_db() async generator;\n\
(5) src/models/__init__.py — re-exports User, Task, Tag;\n\
(6) src/models/user.py — User(Base): id(UUID pk), email(str unique), hashed_password(str), is_active(bool=True), created_at(datetime), tasks relationship;\n\
(7) src/models/task.py — Task(Base): id(UUID pk), title(str), description(str|None), status(Enum: TODO/IN_PROGRESS/DONE/CANCELLED), priority(Enum: LOW/MEDIUM/HIGH/URGENT), due_date(datetime|None), created_at, updated_at, owner_id(UUID FK→users), tags many-to-many via task_tags;\n\
(8) src/models/tag.py — Tag(Base): id(UUID pk), name(str unique), color(str='#808080'), tasks relationship;\n\
(9) src/schemas/__init__.py — re-exports all;\n\
(10) src/schemas/user.py — UserCreate(email,password), UserRead(id,email,is_active,created_at), Token(access_token,token_type), TokenData(user_id|None);\n\
(11) src/schemas/task.py — TaskCreate(title,description,status,priority,due_date,tag_ids), TaskUpdate(all optional), TaskRead(all fields+tags), TaskFilter(status,priority,due_before,due_after,tag_ids,search);\n\
(12) src/schemas/tag.py — TagCreate(name,color), TagRead(id,name,color,task_count);\n\
(13) src/crud/__init__.py — re-exports all;\n\
(14) src/crud/user.py — async get_user_by_email, create_user, get_user_by_id;\n\
(15) src/crud/task.py — async create_task, get_task, get_tasks(owner_id,filter:TaskFilter,skip,limit), update_task, delete_task, get_task_stats(owner_id)->dict with counts by status and priority;\n\
(16) src/crud/tag.py — async create_tag, get_tag, get_tags, delete_tag, get_or_create_tags(names);\n\
(17) src/auth.py — hash_password, verify_password(using passlib bcrypt), create_access_token(data,expires_delta), decode_access_token->TokenData, get_current_user(db,token)->User dependency;\n\
(18) src/routers/__init__.py — empty;\n\
(19) src/routers/auth.py — POST /auth/register->UserRead, POST /auth/token->Token (OAuth2PasswordRequestForm);\n\
(20) src/routers/tasks.py — GET /tasks (paginated+filtered), POST /tasks, GET /tasks/{id}, PATCH /tasks/{id}, DELETE /tasks/{id}, GET /tasks/stats;\n\
(21) src/routers/tags.py — GET /tags, POST /tags, DELETE /tags/{id};\n\
(22) src/middleware.py — RequestTimingMiddleware(BaseHTTPMiddleware): adds X-Process-Time header, logs slow requests >500ms;\n\
(23) tests/__init__.py — empty;\n\
(24) tests/test_auth.py — pytest-asyncio tests for register (success, duplicate email), login (valid creds, wrong password, unknown user); uses httpx AsyncClient;\n\
(25) tests/test_tasks.py — pytest-asyncio tests for create task, list with filters, update status, delete, get stats; authenticated via fixture.\
",
        language: "python",
        expected_min_lines: 800,
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
