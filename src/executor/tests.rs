// Tests for the executor module
#[cfg(test)]
mod tests {
    use crate::executor::{Executor, Reactor, TaskId, SshBindWaker, block_on};
    use std::time::{Duration, Instant};
    use std::future::Future;

    #[test]
    fn test_executor_creation() {
        let executor = Executor::new();
        assert!(executor.is_ok());
    }

    #[test]
    fn test_task_spawn_and_execution() {
        let mut executor = Executor::new().unwrap();
        
        let mut executed = false;
        let executed_ptr = &mut executed as *mut bool;

        // Spawn a simple task
        executor.spawn_task(async move {
            unsafe {
                *executed_ptr = true;
            }
        });

        // Run the executor briefly
        let result = executor.run(Some(Duration::from_millis(100)));
        assert!(result.is_ok());
        assert!(executed);
    }

    #[test]
    fn test_multiple_tasks() {
        let mut executor = Executor::new().unwrap();
        
        let mut counter = 0;
        let counter_ptr = &mut counter as *mut i32;

        // Spawn multiple tasks
        for i in 0..5 {
            executor.spawn_task(async move {
                unsafe {
                    *counter_ptr += i;
                }
            });
        }

        // Run the executor
        let result = executor.run(Some(Duration::from_millis(100)));
        assert!(result.is_ok());
        assert_eq!(counter, 0 + 1 + 2 + 3 + 4);
    }

    #[test]
    fn test_executor_with_timeout() {
        let mut executor = Executor::new().unwrap();
        let mut completed = false;
        let completed_ptr = &mut completed as *mut bool;
        
        // Spawn a task that completes quickly
        executor.spawn_task(async move {
            unsafe {
                *completed_ptr = true;
            }
        });

        let start = Instant::now();
        let result = executor.run(Some(Duration::from_millis(50)));
        let elapsed = start.elapsed();
        
        assert!(result.is_ok());
        assert!(completed);
        assert!(elapsed < Duration::from_millis(50)); // Should complete before timeout
    }

    #[test]
    fn test_empty_executor() {
        let mut executor = Executor::new().unwrap();
        
        // Run executor with no tasks - should return immediately
        let start = Instant::now();
        let result = executor.run(Some(Duration::from_millis(100)));
        let elapsed = start.elapsed();
        
        assert!(result.is_ok());
        assert!(elapsed < Duration::from_millis(10)); // Should return very quickly
    }

    #[test]
    fn test_reactor_creation() {
        let reactor = Reactor::new();
        assert!(reactor.is_ok());
        
        let reactor = reactor.unwrap();
        assert_eq!(reactor.source_count(), 0);
    }

    #[test]
    fn test_task_states() {
        use crate::executor::task::{Task, TaskState};
        use std::pin::Pin;
        
        let task_id = TaskId(42);
        let future = Box::pin(async {});
        let task = Task::new(task_id, future as Pin<Box<dyn Future<Output = ()>>>);
        
        assert_eq!(task.id(), task_id);
        assert_eq!(task.state(), TaskState::Ready);
        assert!(task.is_ready());
        assert!(!task.is_waiting());
        assert!(!task.is_completed());
    }

    #[tokio::test]
    async fn test_block_on_success() {
        let result = block_on(async {
            Ok::<i32, crate::async_ssh::Error>(42)
        });
        
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test] 
    async fn test_block_on_error() {
        let result = block_on(async {
            Err::<i32, crate::async_ssh::Error>(crate::async_ssh::Error::Other("test error".to_string()))
        });
        
        assert!(result.is_err());
    }

    #[test]
    fn test_waker_functionality() {
        let task_id = TaskId(123);
        let waker = SshBindWaker::new(task_id);
        
        assert!(!waker.is_woken());
        waker.wake();
        assert!(waker.is_woken());
        assert!(!waker.is_woken()); // Should be cleared after check
    }
}