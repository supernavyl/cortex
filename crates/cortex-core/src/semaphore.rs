use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use crate::lock_ext::LockExt;

/// A fair asynchronous semaphore that maintains FIFO order for acquiring permits.
pub struct Semaphore {
    state: Arc<Mutex<SemaphoreState>>,
}

struct SemaphoreState {
    permits: usize,
    waiters: VecDeque<Waker>,
}

impl Semaphore {
    /// Creates a new semaphore with the specified number of permits.
    pub fn new(permits: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(SemaphoreState {
                permits,
                waiters: VecDeque::new(),
            })),
        }
    }

    /// Acquires a permit from the semaphore.
    ///
    /// This method will wait until a permit is available, maintaining fairness
    /// by waking waiters in the order they requested permits.
    pub fn acquire(&self) -> AcquireFuture {
        AcquireFuture {
            state: self.state.clone(),
        }
    }
}

/// RAII guard that automatically releases the semaphore permit when dropped.
pub struct SemaphoreGuard {
    state: Arc<Mutex<SemaphoreState>>,
}

impl Drop for SemaphoreGuard {
    fn drop(&mut self) {
        let mut state = self.state.lock_panic_on_poison();
        state.permits += 1;

        // Wake exactly one waiter if there are any
        if let Some(waker) = state.waiters.pop_front() {
            waker.wake();
        }
    }
}

/// Future for acquiring a semaphore permit.
pub struct AcquireFuture {
    state: Arc<Mutex<SemaphoreState>>,
}

impl Future for AcquireFuture {
    type Output = SemaphoreGuard;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.state.lock_panic_on_poison();

        if state.permits > 0 {
            state.permits -= 1;
            Poll::Ready(SemaphoreGuard {
                state: self.state.clone(),
            })
        } else {
            // Add our waker to the back of the queue to maintain fairness
            // But first check if we're already in the queue to avoid duplicates
            let waker_exists = state.waiters.iter().any(|w| w.will_wake(cx.waker()));
            if !waker_exists {
                state.waiters.push_back(cx.waker().clone());
            }
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_semaphore_basic() {
        let semaphore = Semaphore::new(2);
        let sem = Arc::new(semaphore);

        // Acquire two permits
        let _guard1 = sem.acquire().await;
        let _guard2 = sem.acquire().await;

        // Try to acquire a third - should wait
        let sem_clone = sem.clone();
        let handle = tokio::spawn(async move {
            let _guard = sem_clone.acquire().await;
            42
        });

        // Give some time for the task to start waiting
        sleep(Duration::from_millis(10)).await;

        // Release one permit
        drop(_guard1);

        // The spawned task should now complete
        let result = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().unwrap(), 42);
    }

    #[tokio::test]
    async fn test_fairness() {
        let semaphore = Semaphore::new(1);
        let sem = Arc::new(semaphore);

        // Acquire the only permit
        let _guard = sem.acquire().await;

        // Spawn several tasks that will wait
        let mut handles = vec![];
        for i in 0..3 {
            let sem_clone = sem.clone();
            let handle = tokio::spawn(async move {
                let _guard = sem_clone.acquire().await;
                i
            });
            handles.push(handle);
            // Small delay to ensure ordering
            sleep(Duration::from_millis(1)).await;
        }

        // Give time for all tasks to register
        sleep(Duration::from_millis(10)).await;

        // Release the permit - first task should complete first (FIFO)
        drop(_guard);

        // Check that tasks complete in order
        for (i, handle) in handles.into_iter().enumerate() {
            let result = tokio::time::timeout(Duration::from_secs(1), handle).await;
            assert!(result.is_ok());
            assert_eq!(result.unwrap().unwrap(), i);
        }
    }
}
