// Waker implementation for task notification
// Provides the bridge between I/O events and task scheduling

use std::sync::{Arc, Mutex};
use std::task::{RawWaker, RawWakerVTable, Waker};
use std::collections::HashMap;

use super::TaskId;

/// Waker implementation for SSHBind executor
pub struct SshBindWaker {
    task_id: TaskId,
    shared: Arc<Mutex<WakerState>>,
}

/// Shared state for waker management
struct WakerState {
    woken_tasks: HashMap<TaskId, bool>,
}

impl SshBindWaker {
    /// Create a new waker for the given task
    pub fn new(task_id: TaskId) -> Self {
        let shared = Arc::new(Mutex::new(WakerState {
            woken_tasks: HashMap::new(),
        }));

        Self { task_id, shared }
    }

    /// Wake the associated task
    pub fn wake(&self) {
        if let Ok(mut state) = self.shared.lock() {
            state.woken_tasks.insert(self.task_id, true);
        }
    }

    /// Check if this task has been woken and clear the flag
    pub fn is_woken(&self) -> bool {
        if let Ok(mut state) = self.shared.lock() {
            state.woken_tasks.remove(&self.task_id).unwrap_or(false)
        } else {
            false
        }
    }

    /// Convert to a standard Waker
    pub fn into_std_waker(self) -> Waker {
        let raw_waker = self.into_raw_waker();
        unsafe { Waker::from_raw(raw_waker) }
    }

    fn into_raw_waker(self) -> RawWaker {
        let data = Box::into_raw(Box::new(self)) as *const ();
        RawWaker::new(data, &WAKER_VTABLE)
    }
}

impl Clone for SshBindWaker {
    fn clone(&self) -> Self {
        Self {
            task_id: self.task_id,
            shared: Arc::clone(&self.shared),
        }
    }
}

impl From<&SshBindWaker> for Waker {
    fn from(waker: &SshBindWaker) -> Self {
        let cloned = waker.clone();
        cloned.into_std_waker()
    }
}

/// Virtual function table for RawWaker
static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    waker_clone,
    waker_wake,
    waker_wake_by_ref,
    waker_drop,
);

/// Clone function for RawWaker
unsafe fn waker_clone(data: *const ()) -> RawWaker {
    let waker = &*(data as *const SshBindWaker);
    let cloned = waker.clone();
    cloned.into_raw_waker()
}

/// Wake function for RawWaker (takes ownership)
unsafe fn waker_wake(data: *const ()) {
    let waker = Box::from_raw(data as *mut SshBindWaker);
    waker.wake();
}

/// Wake by reference function for RawWaker
unsafe fn waker_wake_by_ref(data: *const ()) {
    let waker = &*(data as *const SshBindWaker);
    waker.wake();
}

/// Drop function for RawWaker
unsafe fn waker_drop(data: *const ()) {
    drop(Box::from_raw(data as *mut SshBindWaker));
}

/// Waker registry for managing task wakers globally
pub struct WakerRegistry {
    wakers: Mutex<HashMap<TaskId, SshBindWaker>>,
}

impl WakerRegistry {
    /// Create a new waker registry
    pub fn new() -> Self {
        Self {
            wakers: Mutex::new(HashMap::new()),
        }
    }

    /// Register a waker for a task
    pub fn register(&self, task_id: TaskId, waker: SshBindWaker) {
        if let Ok(mut wakers) = self.wakers.lock() {
            wakers.insert(task_id, waker);
        }
    }

    /// Unregister a waker for a task
    pub fn unregister(&self, task_id: TaskId) {
        if let Ok(mut wakers) = self.wakers.lock() {
            wakers.remove(&task_id);
        }
    }

    /// Wake a task by ID
    pub fn wake_task(&self, task_id: TaskId) -> bool {
        if let Ok(wakers) = self.wakers.lock() {
            if let Some(waker) = wakers.get(&task_id) {
                waker.wake();
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    /// Get all woken task IDs and clear their flags
    pub fn drain_woken(&self) -> Vec<TaskId> {
        let mut woken_tasks = Vec::new();
        
        if let Ok(wakers) = self.wakers.lock() {
            for (&task_id, waker) in wakers.iter() {
                if waker.is_woken() {
                    woken_tasks.push(task_id);
                }
            }
        }

        woken_tasks
    }

    /// Get the number of registered wakers
    pub fn count(&self) -> usize {
        if let Ok(wakers) = self.wakers.lock() {
            wakers.len()
        } else {
            0
        }
    }
}

impl Default for WakerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_waker_creation() {
        let task_id = TaskId(42);
        let waker = SshBindWaker::new(task_id);
        assert_eq!(waker.task_id, task_id);
        assert!(!waker.is_woken());
    }

    #[test]
    fn test_waker_wake() {
        let task_id = TaskId(42);
        let waker = SshBindWaker::new(task_id);
        
        waker.wake();
        assert!(waker.is_woken());
        
        // Second check should return false (flag cleared)
        assert!(!waker.is_woken());
    }

    #[test]
    fn test_waker_registry() {
        let registry = WakerRegistry::new();
        let task_id = TaskId(42);
        let waker = SshBindWaker::new(task_id);
        
        registry.register(task_id, waker);
        assert_eq!(registry.count(), 1);
        
        assert!(registry.wake_task(task_id));
        assert!(!registry.wake_task(TaskId(999))); // Non-existent task
        
        registry.unregister(task_id);
        assert_eq!(registry.count(), 0);
    }
}