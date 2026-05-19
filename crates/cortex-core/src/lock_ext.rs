//! Extension trait for `std::sync::Mutex` that names the unwrap-on-poison contract.
//!
//! Mutex poisoning means another thread panicked while holding the lock. For
//! cortex's mutex-protected data (`SymbolStore`, `KairosState`, semaphore
//! state), the recovery answer is identical to what `.lock().unwrap()` does:
//! propagate the panic. This trait makes that contract grep-able and lets us
//! turn on `clippy::unwrap_used` workspace-wide without per-site allows.

use std::sync::{Mutex, MutexGuard};

pub trait LockExt<T> {
    /// Lock the mutex, panicking with a contextual message if poisoned.
    ///
    /// Poisoning is unrecoverable: another thread panicked while holding the
    /// lock, so the protected data may be in an inconsistent state.
    fn lock_panic_on_poison(&self) -> MutexGuard<'_, T>;
}

impl<T> LockExt<T> for Mutex<T> {
    #[inline]
    #[allow(clippy::expect_used)]
    fn lock_panic_on_poison(&self) -> MutexGuard<'_, T> {
        self.lock()
            .expect("mutex poisoned: another thread panicked while holding the lock")
    }
}
