// Waker implementation for task notification
// Provides the bridge between I/O events and task scheduling

use std::sync::{Arc, Mutex};
use std::task::{RawWaker, RawWakerVTable, Waker};
use std::collections::HashMap;
use lazy_static::lazy_static;

use super::TaskId;

// Global waker registry shared across all executor instances
lazy_static! {
    static ref GLOBAL_WAKER_STATE: Arc<Mutex<WakerState>> = Arc::new(Mutex::new(WakerState {
        woken_tasks: HashMap::new(),
    }));
}

/// Waker implementation for SSHBind executor
pub struct SshBindWaker {
    task_id: TaskId,
}

/// Shared state for waker management
struct WakerState {
    woken_tasks: HashMap<TaskId, bool>,
}

impl SshBindWaker {
    /// Create a new waker for the given task
    pub fn new(task_id: TaskId) -> Self {
        Self { task_id }
    }

    /// Wake the associated task
    pub fn wake(&self) {
        if let Ok(mut state) = GLOBAL_WAKER_STATE.lock() {
            state.woken_tasks.insert(self.task_id, true);
        }
    }

    /// Check if this task has been woken and clear the flag
    pub fn is_woken(&self) -> bool {
        if let Ok(mut state) = GLOBAL_WAKER_STATE.lock() {
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

}