use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::time::Duration;

use crate::async_ssh::reactor::Reactor;

mod runtime;
pub use runtime::Runtime;

/// Minimal executor for SSHBind
pub struct Executor {
    reactor: Arc<Reactor>,
    tasks: VecDeque<Pin<Box<dyn Future<Output = ()> + Send>>>,
    ready_tasks: VecDeque<Pin<Box<dyn Future<Output = ()> + Send>>>,
}

impl Executor {
    pub fn new() -> std::io::Result<Self> {
        Ok(Executor {
            reactor: Arc::new(Reactor::new()?),
            tasks: VecDeque::new(),
            ready_tasks: VecDeque::new(),
        })
    }

    /// Spawn a future to be executed
    pub fn spawn<F>(&mut self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.tasks.push_back(Box::pin(future));
    }

    /// Run a future to completion
    pub fn block_on<F>(&mut self, future: F) -> F::Output
    where
        F: Future,
    {
        let mut future = Box::pin(future);
        let waker = dummy_waker();
        let mut context = Context::from_waker(&waker);

        loop {
            // Try to poll the main future
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }

            // Process all ready tasks
            while let Some(mut task) = self.ready_tasks.pop_front() {
                let task_waker = dummy_waker();
                let mut task_context = Context::from_waker(&task_waker);
                
                if task.as_mut().poll(&mut task_context).is_pending() {
                    self.tasks.push_back(task);
                }
            }

            // Process pending tasks
            let task_count = self.tasks.len();
            for _ in 0..task_count {
                if let Some(mut task) = self.tasks.pop_front() {
                    let task_waker = dummy_waker();
                    let mut task_context = Context::from_waker(&task_waker);
                    
                    if task.as_mut().poll(&mut task_context).is_pending() {
                        self.tasks.push_back(task);
                    }
                }
            }

            // Wait for I/O events
            if let Err(_) = self.reactor.poll_once(Some(Duration::from_millis(10))) {
                // Continue on poll errors
            }
        }
    }

    /// Get the reactor for async operations
    pub fn reactor(&self) -> Arc<Reactor> {
        self.reactor.clone()
    }
}

/// Create a dummy waker that does nothing
fn dummy_waker() -> Waker {
    unsafe fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    unsafe fn wake(_: *const ()) {}
    unsafe fn wake_by_ref(_: *const ()) {}
    unsafe fn drop(_: *const ()) {}

    const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);

    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
}

/// Global executor for convenience functions
thread_local! {
    static EXECUTOR: std::cell::RefCell<Option<Executor>> = std::cell::RefCell::new(None);
}

/// Spawn a future on the current executor
pub fn spawn<F>(future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    EXECUTOR.with(|executor| {
        if let Some(ref mut exec) = *executor.borrow_mut() {
            exec.spawn(future);
        }
    });
}

/// Run a future to completion using a new executor
pub fn block_on<F>(future: F) -> F::Output
where
    F: Future,
{
    let mut executor = Executor::new().expect("Failed to create executor");
    executor.block_on(future)
}

/// Run a future to completion using the current executor if available
pub fn run_with_executor<F>(future: F) -> F::Output
where
    F: Future,
{
    EXECUTOR.with(|executor| {
        if let Some(ref mut exec) = *executor.borrow_mut() {
            exec.block_on(future)
        } else {
            let mut new_executor = Executor::new().expect("Failed to create executor");
            *executor.borrow_mut() = Some(new_executor);
            let result = executor.borrow_mut().as_mut().unwrap().block_on(future);
            *executor.borrow_mut() = None;
            result
        }
    })
}