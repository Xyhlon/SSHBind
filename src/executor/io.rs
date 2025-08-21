// Global I/O reactor for async operations
use std::collections::HashMap;
use std::io;
use std::os::unix::io::RawFd;
use std::sync::{Arc, Mutex};
use std::task::Waker;
use polling::{Event, Events, Poller};

lazy_static::lazy_static! {
    pub static ref REACTOR: Arc<Mutex<Reactor>> = Arc::new(Mutex::new(Reactor::new()));
}

pub struct Reactor {
    poller: Poller,
    sources: HashMap<RawFd, Source>,
}

struct Source {
    waker: Option<Waker>,
    is_readable: bool,
    is_writable: bool,
}

impl Reactor {
    fn new() -> Self {
        Self {
            poller: Poller::new().expect("Failed to create poller"),
            sources: HashMap::new(),
        }
    }
    
    pub fn register(&mut self, fd: RawFd) -> io::Result<()> {
        if !self.sources.contains_key(&fd) {
            unsafe {
                self.poller.add(fd, Event::all(fd as usize))?;
            }
            self.sources.insert(fd, Source {
                waker: None,
                is_readable: false,
                is_writable: false,
            });
        }
        Ok(())
    }
    
    pub fn unregister(&mut self, fd: RawFd) -> io::Result<()> {
        if self.sources.remove(&fd).is_some() {
            // Create a BorrowedFd for the delete operation
            unsafe {
                use std::os::unix::io::BorrowedFd;
                let borrowed_fd = BorrowedFd::borrow_raw(fd);
                self.poller.delete(borrowed_fd)?;
            }
        }
        Ok(())
    }
    
    pub fn set_readable(&mut self, fd: RawFd, waker: Waker) {
        if let Some(source) = self.sources.get_mut(&fd) {
            source.waker = Some(waker);
            source.is_readable = false; // Reset state
        }
    }
    
    pub fn set_writable(&mut self, fd: RawFd, waker: Waker) {
        if let Some(source) = self.sources.get_mut(&fd) {
            source.waker = Some(waker);
            source.is_writable = false; // Reset state
        }
    }
    
    pub fn is_readable(&self, fd: RawFd) -> bool {
        self.sources.get(&fd).map_or(false, |s| s.is_readable)
    }
    
    pub fn is_writable(&self, fd: RawFd) -> bool {
        self.sources.get(&fd).map_or(false, |s| s.is_writable)
    }
    
    pub fn poll(&mut self) -> io::Result<()> {
        let mut events = Events::new();
        
        // Poll with zero timeout to check for ready I/O
        self.poller.wait(&mut events, Some(std::time::Duration::ZERO))?;
        
        for event in events.iter() {
            let fd = event.key as RawFd;
            if let Some(source) = self.sources.get_mut(&fd) {
                if event.readable {
                    source.is_readable = true;
                }
                if event.writable {
                    source.is_writable = true;
                }
                
                // Wake the task if it's ready
                if (source.is_readable || source.is_writable) && source.waker.is_some() {
                    if let Some(waker) = source.waker.take() {
                        waker.wake();
                    }
                }
            }
        }
        
        Ok(())
    }
    
    pub fn wait(&mut self, timeout: Option<std::time::Duration>) -> io::Result<bool> {
        let mut events = Events::new();
        
        // Wait for I/O events
        let n = self.poller.wait(&mut events, timeout)?;
        
        for event in events.iter() {
            let fd = event.key as RawFd;
            if let Some(source) = self.sources.get_mut(&fd) {
                if event.readable {
                    source.is_readable = true;
                }
                if event.writable {
                    source.is_writable = true;
                }
                
                // Wake the task
                if let Some(waker) = source.waker.take() {
                    waker.wake();
                }
            }
        }
        
        Ok(n > 0)
    }
}