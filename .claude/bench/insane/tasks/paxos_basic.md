Build a single-decree Paxos implementation in Rust with proposer, acceptor,
and learner, plus an in-memory simulator that tests crash-recovery.

Implement in `src/lib.rs`:

```rust
pub type NodeId = u32;
pub type ProposalNum = u64;  // monotonic; ties broken by NodeId in upper bits

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Prepare { from: NodeId, n: ProposalNum },
    Promise { from: NodeId, n: ProposalNum, accepted: Option<(ProposalNum, Vec<u8>)> },
    Accept { from: NodeId, n: ProposalNum, value: Vec<u8> },
    Accepted { from: NodeId, n: ProposalNum, value: Vec<u8> },
    Nack { from: NodeId, n: ProposalNum },
}

pub struct Proposer {
    pub id: NodeId,
    /* state */
}

pub struct Acceptor {
    pub id: NodeId,
    /* state: promised_n, accepted_n, accepted_value — must survive a crash */
}

impl Proposer {
    pub fn new(id: NodeId, quorum_size: usize) -> Self;
    /// Begin a proposal with the given value; emits a Prepare to send to all acceptors.
    pub fn propose(&mut self, value: Vec<u8>) -> Message;
    /// Receive a message; may return zero or one outbound messages.
    pub fn recv(&mut self, msg: Message) -> Vec<Message>;
    /// True once a quorum of Accepted messages for our latest proposal has arrived.
    pub fn decided(&self) -> Option<Vec<u8>>;
}

impl Acceptor {
    pub fn new(id: NodeId) -> Self;
    /// Receive a message and return zero or one outbound responses.
    /// Must persist state changes (in-memory: keep them in `self`).
    pub fn recv(&mut self, msg: Message) -> Vec<Message>;

    /// Snapshot for "crash + restart with same persisted state" testing.
    pub fn snapshot(&self) -> AcceptorSnapshot;
    pub fn restore(snap: AcceptorSnapshot) -> Self;
}

#[derive(Debug, Clone)]
pub struct AcceptorSnapshot {
    pub id: NodeId,
    pub promised_n: Option<ProposalNum>,
    pub accepted_n: Option<ProposalNum>,
    pub accepted_value: Option<Vec<u8>>,
}
```

Invariants (must hold across all tests):

- An acceptor never accepts a value with n < its `promised_n`
- An acceptor never promises a smaller n than it already promised
- A learner never observes two different values being chosen

Tests:

- `test_happy_path_single_proposer` — 1 proposer, 3 acceptors; propose "hello";
  drive the message exchange via a small in-memory bus; assert `decided()==Some("hello")`
- `test_two_proposers_one_wins` — two proposers race with values "A" and "B";
  exactly one of them must reach decided(); the other will see Nack
- `test_acceptor_crash_recovery` — proposer reaches phase 2 with value V; one
  acceptor crashes (snapshot/drop/restore); after restore that acceptor still
  remembers its promised_n + accepted_value; the proposer still decides V
- `test_quorum_not_met_no_decision` — propose with quorum_size=3 but only 2
  acceptors respond; decided()==None
- `test_higher_n_supersedes_lower` — A1 proposes n=1; A2 proposes n=2; A2 wins
- `test_proposal_num_includes_node_id_to_break_ties` — encoding scheme must
  guarantee two different nodes never generate the same ProposalNum

`cargo check` clean, `cargo test` all pass.

No async, no real networking. Pure state-machine simulation via direct
function calls on a Vec<Acceptor>.
