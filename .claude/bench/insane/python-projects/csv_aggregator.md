Build a streaming CSV aggregator in Python (stdlib only, no pandas/polars).

## Requirements

Implement `csv_aggregator.py` at the project root:

```python
from typing import Literal, Iterator
import pathlib

AggFunc = Literal["sum", "count", "mean", "min", "max", "first", "last"]

class Aggregation:
    def __init__(self, column: str, func: AggFunc, alias: str | None = None) -> None: ...

class GroupBy:
    def __init__(self, keys: list[str], aggregations: list[Aggregation]) -> None: ...

def aggregate_csv(
    source: str | pathlib.Path | Iterator[dict],
    group_by: GroupBy,
    *,
    delimiter: str = ",",
    skip_errors: bool = False,
) -> list[dict]:
    """Stream-aggregate CSV file (or iterator of row dicts).
    
    - source: file path OR an iterator of dicts (for testing without files)
    - group_by: grouping keys and aggregation definitions
    - skip_errors: if True, skip rows with missing/invalid values instead of raising
    - Returns list of result dicts, sorted by grouping keys ascending
    
    Aggregate functions:
    - sum: numeric sum of column values
    - count: count of non-None values
    - mean: arithmetic mean
    - min / max: minimum / maximum value (numeric)
    - first / last: first or last value seen in group
    """

def write_csv(rows: list[dict], path: str | pathlib.Path, *, delimiter: str = ",") -> None:
    """Write rows to CSV file with header from keys of first row."""
```

Processing must be streaming (constant memory regardless of file size — do NOT load entire file into a list first).

## Tests

Write `tests/test_csv_aggregator.py` with pytest tests covering:

1. count: count rows per group key
2. sum: sum numeric column per group
3. mean: compute mean per group (correct floating point)
4. min/max: correct extremes per group
5. first/last: correct first/last value per group
6. multiple aggregations in one pass
7. multiple group keys (composite key)
8. alias: result dict uses alias name instead of column name
9. skip_errors=True: rows with missing numeric values skipped, others processed
10. skip_errors=False: missing numeric value raises ValueError
11. Iterator input: works with dict iterator (no file I/O)
12. Result sorted by group keys ascending
13. write_csv: written file can be read back with csv.DictReader and matches input

Use in-memory dict iterators in tests (no temp files needed except test 13).
Write no other files. All imports must be stdlib only.
