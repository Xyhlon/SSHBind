// Minimal single-threaded executor for SSHBind
// Replaces Tokio with lightweight I/O polling

mod reactor;
mod task;
mod waker;

#[cfg(test)]
mod tests;

use std::future::Future;
use std::task::{Context, Poll};
use std::time::Duration;
use std::pin::Pin;

pub use reactor::Reactor;
pub use task::{Task, TaskId};
pub use waker::SshBindWaker;

use crate::async_ssh::Result;

thread_local! {
    static EXECUTOR: std::cell::RefCell<Option<Executor>> = 
        std::cell::RefCell::new(None);
}

/// Single-threaded executor that runs futures with I/O polling
pub struct Executor {
    reactor: Reactor,
    task_queue: std::collections::VecDeque<Task>,
    next_task_id: TaskId,
}

impl Executor {
    /// Create a new executor instance
    pub fn new() -> Result<Self> {
        Ok(Self {
            reactor: Reactor::new()?,
            task_queue: std::collections::VecDeque::new(),
            next_task_id: TaskId(0),
        })
    }

    /// Spawn a new task on this executor
    pub fn spawn_task<F>(&mut self, future: F) -> TaskId
    where
        F: Future<Output = ()> + 'static,
    {
        let task_id = self.next_task_id;
        self.next_task_id.0 += 1;

        let task = Task::new(task_id, Box::pin(future));
        self.task_queue.push_back(task);
        task_id
    }

    /// Run the executor until all tasks complete or timeout
    pub fn run(&mut self, timeout: Option<Duration>) -> Result<()> {
        let start_time = std::time::Instant::now();

        loop {
            // Check timeout
            if let Some(timeout) = timeout {
                if start_time.elapsed() >= timeout {
                    break;
                }
            }

            // If no tasks, we're done
            if self.task_queue.is_empty() {
                break;
            }

            // Poll all ready tasks
            self.poll_tasks()?;

            // Poll I/O events with very short timeout to keep executor responsive
            if !self.task_queue.is_empty() {
                // Use very short timeout to keep executor responsive for SSH handshakes
                self.reactor.poll_events(Duration::from_micros(100))?;
            }
        }

        Ok(())
    }

    fn poll_tasks(&mut self) -> Result<()> {
        let mut tasks_to_repoll = Vec::new();
        
        // For debugging: always poll all tasks for now
        while let Some(mut task) = self.task_queue.pop_front() {
            let ssh_waker = SshBindWaker::new(task.id());
            let waker = ssh_waker.into_std_waker();
            let mut context = Context::from_waker(&waker);

            match task.poll(&mut context) {
                std::task::Poll::Ready(_) => {
                    // Task completed, don't re-queue
                }
                std::task::Poll::Pending => {
                    // Task is waiting for I/O, re-queue for later
                    tasks_to_repoll.push(task);
                }
            }
        }

        // Re-queue pending tasks
        for task in tasks_to_repoll {
            self.task_queue.push_back(task);
        }

        Ok(())
    }

    /// Get reactor for I/O registration
    pub fn reactor(&self) -> &Reactor {
        &self.reactor
    }
}

/// Block on a future until it completes  
pub fn block_on<F, T: 'static>(future: F) -> Result<T>
where
    F: Future<Output = Result<T>> + 'static,
{
    use std::sync::{Arc, Mutex};
    
    let mut executor = Executor::new()?;
    let result = Arc::new(Mutex::new(None));
    let result_clone = Arc::clone(&result);

    // Add any pending tasks first
    let pending_tasks = PENDING_TASKS.with(|pending| {
        pending.borrow_mut().drain(..).collect::<Vec<_>>()
    });
    
    for task in pending_tasks {
        let task_id = executor.next_task_id;
        executor.next_task_id.0 += 1;
        let task = Task::new(task_id, task);
        executor.task_queue.push_back(task);
    }

    // Spawn the main future
    executor.spawn_task(async move {
        match future.await {
            Ok(value) => {
                *result_clone.lock().unwrap() = Some(Ok(value));
            }
            Err(err) => {
                *result_clone.lock().unwrap() = Some(Err(err));
            }
        }
    });

    // Run until completion or timeout (30 seconds for SSH operations)
    executor.run(Some(Duration::from_secs(30)))?;

    // Extract result
    let mut result_guard = result.lock().unwrap();
    match result_guard.take() {
        Some(result) => result,
        None => Err(crate::async_ssh::Error::Timeout("Operation timed out".to_string())),
    }
}

/// Spawn a future on the global executor instance
pub fn spawn<F>(future: F)
where
    F: Future<Output = ()> + 'static,
{
    // Always store in pending tasks - block_on will pick them up
    PENDING_TASKS.with(|tasks| {
        tasks.borrow_mut().push(Box::pin(future));
    });
}

// Thread-local storage for tasks spawned before executor starts
thread_local! {
    static PENDING_TASKS: std::cell::RefCell<Vec<std::pin::Pin<Box<dyn Future<Output = ()>>>>> = 
        std::cell::RefCell::new(Vec::new());
}

/// Future that yields execution to other tasks
pub struct YieldNow {
    yielded: bool,
}

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.yielded = true;
            // Schedule to be woken on the next iteration without immediate wake
            let waker = cx.waker().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(1));
                waker.wake();
            });
            Poll::Pending
        }
    }
}

/// Yield execution to allow other tasks to run
pub fn yield_now() -> YieldNow {
    YieldNow { yielded: false }
}