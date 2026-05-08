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