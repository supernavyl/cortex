Build an Erlang-style actor framework project in Rust on top of tokio.

Layout:

```
src/
  lib.rs            - public API + re-exports
  actor.rs          - Actor trait + ActorRef + Mailbox
  system.rs         - ActorSystem (root supervisor + spawn API)
  supervision.rs    - SupervisionStrategy + restart logic
  message.rs        - Message envelope, ask vs tell semantics
tests/
  ping_pong.rs      - two-actor ping-pong
  supervision.rs    - supervisor restarts crashed children
```

`Cargo.toml` deps:
- `tokio = { version = "1", features = ["full"] }`
- `async-trait = "0.1"`
- `thiserror = "1"`
- `tracing = "0.1"`

Public API:

```rust
#[async_trait::async_trait]
pub trait Actor: Send + 'static {
    type Msg: Send + 'static;

    async fn handle(&mut self, msg: Self::Msg, ctx: &mut Ctx<Self::Msg>);

    async fn on_start(&mut self, _ctx: &mut Ctx<Self::Msg>) {}
    async fn on_stop(&mut self) {}
}

pub struct ActorRef<M>(/* private */);
pub struct Ctx<M>(/* private — exposes self_ref, system handle, spawn */);

pub struct ActorSystem(/* private */);
impl ActorSystem {
    pub fn new() -> Self;
    pub fn spawn<A: Actor>(&self, actor: A) -> ActorRef<A::Msg>;
    pub async fn shutdown(self);
}

impl<M: Send + 'static> ActorRef<M> {
    pub fn tell(&self, msg: M);                        // fire-and-forget
    pub async fn ask<R: Send + 'static>(&self, build: impl FnOnce(oneshot::Sender<R>) -> M)
        -> Result<R, AskError>;                        // request-reply via oneshot
}

#[derive(Clone, Copy)]
pub enum SupervisionStrategy { Stop, Restart, RestartWithBackoff(Duration) }
```

Required behaviors:
- Each actor runs in its own tokio task with a bounded mpsc mailbox (capacity 128)
- `tell` returns immediately even if the mailbox is full (drops + logs warn)
- `ask` waits on a oneshot reply channel with a 5-second timeout
- A panicking `Actor::handle` triggers the supervisor's strategy (default: Restart)
- `on_start` is called after spawn before the first message; `on_stop` on shutdown
- `ActorSystem::shutdown` sends a stop signal to all actors and awaits drain

Tests:
- `test_spawn_and_tell` — spawn an actor that counts messages; tell 10 times; ask
  current count
- `test_ask_returns_reply`
- `test_ask_timeout_on_unresponsive_actor`
- `test_supervision_restart_after_panic` — actor panics on first message; supervisor
  restarts; second message handled normally
- `test_supervision_stop_strategy_does_not_restart`
- `test_actor_lifecycle_hooks_called` — on_start before first handle, on_stop after
  shutdown

`cargo check` clean, `cargo test` all pass.
