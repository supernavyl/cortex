Build a JSON schema validator in Python (stdlib only, no jsonschema package).

## Requirements

Implement `validator.py` at the project root. Support a subset of JSON Schema Draft-7:

```python
class ValidationError(Exception):
    def __init__(self, message: str, path: str = "") -> None: ...

def validate(instance: object, schema: dict) -> None:
    """Validate `instance` against `schema`. Raise ValidationError on failure.
    
    Supported keywords:
    - type: "string" | "number" | "integer" | "boolean" | "array" | "object" | "null"
    - properties: {key: schema} — validate object properties
    - required: [str] — required object keys
    - additionalProperties: bool | schema — control extra keys
    - items: schema — validate array elements
    - minItems / maxItems: int
    - minLength / maxLength: int (strings)
    - minimum / maximum: number
    - exclusiveMinimum / exclusiveMaximum: number
    - enum: [values] — value must be in list
    - const: value — value must equal exactly
    - anyOf: [schema] — must match at least one
    - allOf: [schema] — must match all
    - oneOf: [schema] — must match exactly one
    - not: schema — must NOT match
    - $ref: "#/definitions/Name" — reference to schema["definitions"]["Name"]
    """

def is_valid(instance: object, schema: dict) -> bool:
    """Return True if valid, False otherwise."""
```

## Tests

Write `tests/test_validator.py` with pytest tests covering:

1. type validation: correct types pass, wrong types raise ValidationError
2. required: missing required key raises ValidationError
3. properties: nested object validated recursively
4. additionalProperties: false rejects extra keys
5. items: array elements validated against schema
6. minItems/maxItems: array length constraints enforced
7. minLength/maxLength: string length constraints enforced
8. minimum/maximum: numeric range enforced
9. exclusiveMinimum/exclusiveMaximum: exclusive bounds enforced
10. enum: value not in list raises ValidationError
11. const: value not equal raises ValidationError
12. anyOf: at least one schema must match
13. oneOf: exactly one schema must match (not zero, not two)
14. allOf: all schemas must match
15. not: passes when schema does not match
16. $ref: resolves definition and validates against it
17. is_valid: returns bool without raising

Write no other files. All imports must be stdlib only.
