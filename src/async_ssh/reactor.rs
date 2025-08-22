use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::time::Duration;

use polling::{Event, Events, Poller};

/// Unique token for tracking I/O sources
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Token(pub usize);

static TOKEN_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);

impl Token {
    pub fn next() -> Self {
        Token(TOKEN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// Single-threaded reactor using polling
pub struct Reactor {
    poller: Arc<Poller>,
    wakers: Arc<Mutex<HashMap<Token, Waker>>>,
}

impl Reactor {
    pub fn new() -> io::Result<Self> {
        Ok(Reactor {
            poller: Arc::new(Poller::new()?),
            wakers: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Register a waker for the given token
    pub fn register_waker(&self, token: Token, waker: Waker) {
        let mut wakers = self.wakers.lock().unwrap();
        wakers.insert(token, waker);
    }

    /// Remove waker for the given token
    pub fn remove_waker(&self, token: Token) {
        let mut wakers = self.wakers.lock().unwrap();
        wakers.remove(&token);
    }

    /// Register interest in a socket for the given token
    pub fn register_socket(&self, socket: &std::net::TcpStream, token: Token) -> io::Result<()> {
        // Register for both read and write events
        unsafe {
            self.poller.add(socket, Event::all(token.0))?;
        }
        Ok(())
    }

    /// Modify interest in a socket
    pub fn modify_socket(&self, socket: &std::net::TcpStream, token: Token) -> io::Result<()> {
        unsafe {
            self.poller.modify(socket, Event::all(token.0))?;
        }
        Ok(())
    }

    /// Remove socket from poller
    pub fn remove_socket(&self, socket: &std::net::TcpStream) -> io::Result<()> {
        unsafe {
            self.poller.delete(socket)?;
        }
        Ok(())
    }

    /// Wait for events and wake corresponding tasks
    pub fn poll_once(&self, timeout: Option<Duration>) -> io::Result<()> {
        let mut events = Events::new();
        self.poller.wait(&mut events, timeout)?;

        let wakers = self.wakers.lock().unwrap();
        for event in events.iter() {
            if let Some(waker) = wakers.get(&Token(event.key)) {
                waker.wake_by_ref();
            }
        }

        Ok(())
    }

    /// Get the underlying poller
    pub fn poller(&self) -> &Arc<Poller> {
        &self.poller
    }
}