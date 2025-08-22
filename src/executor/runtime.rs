use std::future::Future;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use super::Executor;

/// Simple runtime for running futures
pub struct Runtime {
    executor: Arc<Mutex<Executor>>,
    shutdown: Arc<(Mutex<bool>, Condvar)>,
}

impl Runtime {
    pub fn new() -> std::io::Result<Self> {
        let executor = Arc::new(Mutex::new(Executor::new()?));
        let shutdown = Arc::new((Mutex::new(false), Condvar::new()));

        Ok(Runtime {
            executor,
            shutdown,
        })
    }

    /// Spawn a future on this runtime
    pub fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut executor = self.executor.lock().unwrap();
        executor.spawn(future);
    }

    /// Run a future to completion on this runtime
    pub fn block_on<F>(&self, future: F) -> F::Output
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let executor = self.executor.clone();
        let shutdown = self.shutdown.clone();
        
        let handle = thread::spawn(move || {
            let mut exec = executor.lock().unwrap();
            let result = exec.block_on(future);
            
            // Signal shutdown
            let (lock, cvar) = &*shutdown;
            let mut shutdown_flag = lock.lock().unwrap();
            *shutdown_flag = true;
            cvar.notify_all();
            
            result
        });

        // Wait for completion
        let (lock, cvar) = &*self.shutdown;
        let _guard = cvar
            .wait_while(lock.lock().unwrap(), |shutdown| !*shutdown)
            .unwrap();

        handle.join().expect("Runtime thread panicked")
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new().expect("Failed to create runtime")
    }
}