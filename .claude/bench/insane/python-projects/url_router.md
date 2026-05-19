Build a URL pattern router in Python (stdlib only).

## Requirements

Implement `router.py` at the project root:

```python
from typing import Callable, Any

Handler = Callable[..., Any]
Middleware = Callable[[dict, Callable], Any]

class Router:
    def add_route(
        self,
        method: str,
        pattern: str,
        handler: Handler,
        *,
        middlewares: list[Middleware] | None = None,
    ) -> None:
        """Register a route. Pattern supports:
        - Static: "/users"
        - Named params: "/users/{id}" — captured as str
        - Typed params: "/users/{id:int}" — captured as int
        - Wildcards: "/files/{path:*}" — captures rest of path
        method is case-insensitive. Use "*" to match any method.
        """
    
    def match(self, method: str, path: str) -> tuple[Handler, dict[str, Any], list[Middleware]] | None:
        """Return (handler, params, middlewares) or None if no match.
        Routes are matched in registration order. More specific routes
        registered first take priority.
        """
    
    def dispatch(self, method: str, path: str, **kwargs: Any) -> Any:
        """Match route and call handler through middleware chain.
        Raise ValueError if no route matches.
        Middleware signature: middleware(context, next) where context is a dict
        and next is a callable that continues the chain.
        """
    
    def route(self, pattern: str, methods: list[str] | None = None):
        """Decorator factory: @router.route("/users/{id:int}", methods=["GET"])"""
```

## Tests

Write `tests/test_router.py` with pytest tests covering:

1. Static route match: exact path matches, wrong path returns None
2. Named param: `/users/{id}` captures `id` as string
3. Typed param int: `/users/{id:int}` captures `id` as int, rejects non-numeric
4. Wildcard param: `/files/{path:*}` captures rest of path including slashes
5. Method match: GET and POST registered separately, correct one matched
6. Method `*`: wildcard method matches any HTTP verb
7. Method case-insensitive: "get" matches "GET" route
8. No match: returns None for unregistered path
9. Registration order: first-registered wins on ambiguous patterns
10. dispatch calls handler with captured params
11. Middleware chain: middleware wraps handler, can modify context before/after
12. Multiple middlewares: applied in order (outer-to-inner)
13. dispatch raises ValueError on no match
14. `@router.route` decorator registers and calls handler correctly

Write no other files. All imports must be stdlib only.
