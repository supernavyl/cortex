Build a Byzantine fault-tolerant consensus simulator project in Rust.

Layout:

```
src/
  lib.rs            - public API
  node.rs           - Node state machine (Replica role)
  message.rs        - PrePrepare / Prepare / Commit / ViewChange message types
  bus.rs            - in-memory message bus with controllable drop / delay / corrupt
  fault.rs          - FaultInjector strategies
  client.rs         - Client that issues requests + collects responses
tests/
  consensus.rs      - integration tests: N nodes, F faulty, request commits
```

`Cargo.toml` deps:
- `serde = { version = "1", features = ["derive"] }`
- `serde_json = "1"`
- `thiserror = "1"`
- `tracing = "0.1"`
- `sha2 = "0.10"`

Algorithm: simplified PBFT (Castro & Liskov 1999). 3-phase protocol:
1. Client → Primary: Request
2. Primary → All: PrePrepare(view, seq, request, digest)
3. Each Replica → All: Prepare(view, seq, digest, from)
4. Once 2F Prepares for the same (view, seq, digest): broadcast Commit
5. Once 2F+1 Commits: execute request and reply to client
6. Client accepts a response when F+1 matching responses arrive

System parameters:
- N replicas, F = (N-1) / 3 faulty
- Primary index = view mod N
- Sequence numbers monotonic

Public API:

```rust
pub use node::{Replica, ReplicaConfig};
pub use bus::{Bus, BusConfig};
pub use client::Client;
pub use fault::FaultInjector;

pub fn run_simulation(num_nodes: usize, num_faulty: usize, num_requests: usize,
                     fault_injector: FaultInjector) -> SimulationResult;

pub struct SimulationResult {
    pub committed: Vec<(u64, Vec<u8>)>,  // (seq, payload)
    pub messages_sent: usize,
    pub messages_dropped: usize,
    pub view_changes: usize,
}
```

`FaultInjector` modes:
- `Honest` — no faults
- `Crash(set: Vec<NodeId>)` — listed nodes never send
- `DropRate(p: f64)` — bus drops fraction p of messages
- `Delay(min, max)` — random delay per message

Tests:
- `test_4_nodes_0_faulty_commits` — 4 replicas, 0 faulty, 3 requests; all 3 commit
- `test_4_nodes_1_crashed_still_commits` — 4 replicas, 1 crashed (= F); progress continues
- `test_4_nodes_2_crashed_no_commit` — 4 replicas, 2 crashed (> F); requests don't commit
- `test_consistency_no_two_different_values_same_seq` — across all replicas and
  all completed runs, never observe two different committed payloads for the
  same seq number
- `test_view_change_replaces_dead_primary` — start sim with primary in the crash
  set; after timeout, view-change triggers; new primary makes progress
- `test_drop_rate_recovers_via_retry` — DropRate(0.2) — sim still eventually
  commits all N requests

`cargo check` clean, `cargo test` all pass. Deterministic when given a fixed RNG seed.
