Build an RFC 1951 DEFLATE decompressor in Rust.

Implement in `src/lib.rs`:

```rust
#[derive(Debug)]
pub enum DeflateError {
    UnexpectedEof,
    InvalidBlock,
    InvalidCode,
    OutputTooLarge,
}

impl std::fmt::Display for DeflateError;
impl std::error::Error for DeflateError;

/// Inflate a DEFLATE stream. Output is bounded by `max_output_bytes` to prevent
/// resource exhaustion. Return Err(OutputTooLarge) if exceeded.
pub fn inflate(input: &[u8], max_output_bytes: usize) -> Result<Vec<u8>, DeflateError>;
```

Implementation must support all three block types:

- **BTYPE=00 stored** — uncompressed, with length + nlength header
- **BTYPE=01 fixed Huffman** — fixed RFC 1951 §3.2.6 tables
- **BTYPE=10 dynamic Huffman** — code-length-code, then literal/distance code-length
  sequences with RLE codes 16/17/18

Backreference distance handling: maintain a sliding window of the last 32 KB of output.

No external compression deps — write the bit reader, Huffman decoder, and
LZ77 backref logic from scratch. Stdlib + `flate2` for testing ONLY (as dev-dependency
to generate test inputs).

Tests:

- `test_stored_block_roundtrip` — feed a 4-byte stored block, get the same bytes
- `test_fixed_huffman_short` — compress "hello world" with flate2 deflate(),
  pass output to your inflate(), assert == "hello world".as_bytes()
- `test_dynamic_huffman_medium` — same flow with a 2 KB repetitive payload
- `test_lz77_backreference_simple` — input contains repeating patterns; verify
  decoded output matches flate2's reference implementation
- `test_max_output_bytes_enforced` — give it a small max + a large compressed
  input; expect Err(OutputTooLarge)
- `test_truncated_input_returns_unexpected_eof`
- `test_corrupt_input_returns_invalid_block`

Add `flate2 = "1"` as a `[dev-dependencies]` entry — used only to generate
compressed inputs in tests, never in the public API.

`cargo check` clean, `cargo test` all pass.
