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

/// Task queue for managing ready and waiting tasks
pub struct TaskQueue {
    ready_tasks: std::collections::VecDeque<Task>,
    waiting_tasks: std::collections::HashMap<TaskId, Task>,
}

impl TaskQueue {
    /// Create a new empty task queue
    pub fn new() -> Self {
        Self {
            ready_tasks: std::collections::VecDeque::new(),
            waiting_tasks: std::collections::HashMap::new(),
        }
    }

    /// Add a task to the ready queue
    pub fn push_ready(&mut self, task: Task) {
        self.ready_tasks.push_back(task);
    }

    /// Pop a ready task from the front of the queue
    pub fn pop_ready(&mut self) -> Option<Task> {
        self.ready_tasks.pop_front()
    }

    /// Move a task to the waiting state
    pub fn mark_waiting(&mut self, task: Task) {
        self.waiting_tasks.insert(task.id(), task);
    }

    /// Wake a waiting task by ID, moving it to ready
    pub fn wake_task(&mut self, task_id: TaskId) -> bool {
        if let Some(mut task) = self.waiting_tasks.remove(&task_id) {
            task.wake();
            self.ready_tasks.push_back(task);
            true
        } else {
            false
        }
    }

    /// Get the number of ready tasks
    pub fn ready_count(&self) -> usize {
        self.ready_tasks.len()
    }

    /// Get the number of waiting tasks
    pub fn waiting_count(&self) -> usize {
        self.waiting_tasks.len()
    }

    /// Check if the queue is empty (no ready or waiting tasks)
    pub fn is_empty(&self) -> bool {
        self.ready_tasks.is_empty() && self.waiting_tasks.is_empty()
    }

    /// Get total task count
    pub fn total_count(&self) -> usize {
        self.ready_tasks.len() + self.waiting_tasks.len()
    }

    /// Remove and return all completed tasks
    pub fn drain_completed(&mut self) -> Vec<Task> {
        let mut completed = Vec::new();
        
        // Check ready tasks
        let mut remaining_ready = std::collections::VecDeque::new();
        while let Some(task) = self.ready_tasks.pop_front() {
            if task.is_completed() {
                completed.push(task);
            } else {
                remaining_ready.push_back(task);
            }
        }
        self.ready_tasks = remaining_ready;

        // Check waiting tasks
        let waiting_ids: Vec<TaskId> = self.waiting_tasks.keys().copied().collect();
        for task_id in waiting_ids {
            if let Some(task) = self.waiting_tasks.get(&task_id) {
                if task.is_completed() {
                    completed.push(self.waiting_tasks.remove(&task_id).unwrap());
                }
            }
        }

        completed
    }
}

impl Default for TaskQueue {
    fn default() -> Self {
        Self::new()
    }
}