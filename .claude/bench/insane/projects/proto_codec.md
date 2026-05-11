Build a Protobuf-like binary codec project in Rust (schema parser + runtime codec).

Layout:

```
src/
  lib.rs            - public API
  schema.rs         - .proto-lite schema parser
  wire.rs           - wire format encoder/decoder (varint + length-delimited)
  value.rs          - dynamic Value type
  codec.rs          - encode/decode via schema
tests/
  roundtrip.rs      - schema → encode → decode → check equality
```

`Cargo.toml` deps:
- `thiserror = "1"`
- `bytes = "1"`

(No prost, no protobuf crate. Pure Rust.)

Schema grammar (simplified — ASCII `.proto`-like):

```
message Point {
  int32 x = 1;
  int32 y = 2;
  string label = 3;
  repeated int32 tags = 4;
}

message Line {
  Point start = 1;
  Point end = 2;
}
```

Supported types: `int32`, `int64`, `uint32`, `uint64`, `bool`, `string`, `bytes`,
plus nested message types and `repeated` modifier.

Wire format follows real Protobuf:
- Tag = (field_num << 3) | wire_type, encoded as varint
- Wire types: 0=varint, 2=length-delimited
- int32/int64/uint32/uint64/bool → varint
- string/bytes/nested message → length-delimited

Public API:

```rust
pub use schema::{Schema, MessageDef, FieldDef, FieldType};
pub use value::Value;

pub fn parse_schema(src: &str) -> Result<Schema, ParseError>;
pub fn encode(schema: &Schema, msg_name: &str, value: &Value) -> Result<Vec<u8>, CodecError>;
pub fn decode(schema: &Schema, msg_name: &str, bytes: &[u8]) -> Result<Value, CodecError>;
```

`Value::Message(HashMap<String, Value>)` — dynamic, no codegen.

Tests:
- `test_parse_schema_single_message`
- `test_parse_schema_with_nested`
- `test_parse_error_missing_semicolon`
- `test_varint_roundtrip` — values 0, 127, 128, 16384, u64::MAX
- `test_encode_simple_message` — Point{x:1, y:2} → known byte sequence
- `test_decode_simple_message` — same bytes → Point{x:1, y:2}
- `test_encode_decode_roundtrip` — Point with all fields populated
- `test_encode_decode_nested_message` — Line{start, end} roundtrip
- `test_encode_decode_repeated_field` — Point with tags=[1,2,3,4,5]
- `test_unknown_field_decoded_skipped` — message with extra unknown field decodes
  by skipping the unknown bytes (forward compat)

`cargo check` clean, `cargo test` all pass.
