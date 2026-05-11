Build a WebSocket echo server project in Rust implementing RFC 6455 from scratch.

Layout:

```
src/
  main.rs           - listens on a port, accepts WS upgrades
  lib.rs            - public API
  handshake.rs      - HTTP/1.1 Upgrade + Sec-WebSocket-Accept handshake
  frame.rs          - WebSocket frame encoder/decoder
  conn.rs           - per-connection state machine
tests/
  frames.rs         - frame encode/decode unit tests
  echo.rs           - integration: client connects, sends text + binary, receives echo
```

`Cargo.toml` deps:
- `tokio = { version = "1", features = ["full"] }`
- `base64 = "0.22"`
- `sha1 = "0.10"`
- `thiserror = "1"`
- `tracing = "0.1"`
- `clap = { version = "4", features = ["derive"] }`

[dev-dependencies]:
- `tokio-tungstenite = "0.24"` — used **only** in tests to verify against a reference client

(No `tungstenite`, no `axum-ws` in the library code.)

RFC 6455 handshake:
- Client sends HTTP/1.1 GET with `Upgrade: websocket`, `Connection: Upgrade`,
  `Sec-WebSocket-Key: <base64>`, `Sec-WebSocket-Version: 13`
- Server responds 101 Switching Protocols with `Sec-WebSocket-Accept:
  base64(sha1(key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"))`

Frame format:
- bit 0: FIN
- bits 1-3: RSV1-3 (reject if any set in v0)
- bits 4-7: opcode (0x0=Continuation, 0x1=Text, 0x2=Binary, 0x8=Close, 0x9=Ping, 0xA=Pong)
- bit 8: MASK
- bits 9-15: payload-length (7-bit / 16-bit / 64-bit extended)
- masking key (4 bytes if MASK)
- payload (XOR-unmasked if MASK was set)

Client→Server frames are masked; Server→Client are not.

Behaviors:
- Echo text frames as-is (text → text)
- Echo binary frames as-is
- Respond to Ping with Pong containing the same payload
- Respond to Close with Close (same code) then drop the connection
- Reject masked Server-frames + unmasked Client-frames per RFC

Tests:
- `test_handshake_computes_accept_key` — known input from RFC §1.3, expected output
- `test_frame_decode_unmasked_short` — 4-byte text frame
- `test_frame_decode_masked_short` — same payload but with mask
- `test_frame_decode_extended_16bit_length`
- `test_frame_decode_extended_64bit_length`
- `test_frame_encode_decode_roundtrip` — encode a text frame, decode, get same payload
- `test_echo_text_via_tokio_tungstenite_client` — integration: real WS client connects,
  sends "hello", receives "hello"
- `test_echo_binary_via_client`
- `test_ping_responds_with_pong`

`cargo check` clean, `cargo test` all pass.
