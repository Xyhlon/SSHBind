// Task management for the executor
// Wraps futures with task metadata and polling interface

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Unique identifier for a task
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub usize);

/// A spawned task that wraps a future
pub struct Task {
    id: TaskId,
    future: Pin<Box<dyn Future<Output = ()> + 'static>>,
    state: TaskState,
}

/// Current state of a task
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// Task is ready to run
    Ready,
    /// Task is waiting for I/O or other event
    Waiting,
    /// Task has completed
    Completed,
}

impl Task {
    /// Create a new task with the given ID and future
    pub fn new(id: TaskId, future: Pin<Box<dyn Future<Output = ()> + 'static>>) -> Self {
        Self {
            id,
            future,
            state: TaskState::Ready,
        }
    }

    /// Get the task ID
    pub fn id(&self) -> TaskId {
        self.id
    }

    /// Get the current task state
    pub fn state(&self) -> TaskState {
        self.state
    }

    /// Poll the underlying future
    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        match self.state {
            TaskState::Completed => {
                // Task is already done
                Poll::Ready(())
            }
            TaskState::Ready | TaskState::Waiting => {
                match self.future.as_mut().poll(cx) {
                    Poll::Ready(()) => {
                        self.state = TaskState::Completed;
                        Poll::Ready(())
                    }
                    Poll::Pending => {
                        self.state = TaskState::Waiting;
                        Poll::Pending
                    }
                }
            }
        }
    }

    /// Wake the task, marking it as ready
    pub fn wake(&mut self) {
        if self.state == TaskState::Waiting {
            self.state = TaskState::Ready;
        }
    }

    /// Check if the task is completed
    pub fn is_completed(&self) -> bool {
        self.state == TaskState::Completed
    }

    /// Check if the task is waiting for events
    pub fn is_waiting(&self) -> bool {
        self.state == TaskState::Waiting
    }

    /// Check if the task is ready to run
    pub fn is_ready(&self) -> bool {
        self.state == TaskState::Ready
    }
}

impl std::fmt::Debug for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Task")
            .field("id", &self.id)
            .field("state", &self.state)
            .finish()
    }
}

