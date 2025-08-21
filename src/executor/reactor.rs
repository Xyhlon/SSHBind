// I/O event loop using polling crate
// Manages I/O readiness notifications for async operations

use std::collections::HashMap;
use std::io;
use std::net::TcpStream;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::time::Duration;
use polling::{Event, Events, Poller};
use slab::Slab;

use crate::async_ssh::Result;
use super::SshBindWaker;

/// Token to identify I/O sources in the poller
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Token(pub usize);

/// I/O interest flags
#[derive(Debug, Clone, Copy)]
pub struct Interest {
    pub readable: bool,
    pub writable: bool,
}

impl Interest {
    pub const READABLE: Self = Self { readable: true, writable: false };
    pub const WRITABLE: Self = Self { readable: false, writable: true };
    pub const BOTH: Self = Self { readable: true, writable: true };

}

/// Registered I/O source  
pub struct IoSource {
    pub token: Token,
    pub interest: Interest,
    pub waker: Option<SshBindWaker>,
}

/// Reactor manages I/O polling and waker dispatch
pub struct Reactor {
    poller: Poller,
    sources: Slab<IoSource>,
    fd_to_token: HashMap<i32, Token>,
}

impl Reactor {
    /// Create a new reactor
    pub fn new() -> Result<Self> {
        let poller = Poller::new().map_err(|e| {
            crate::async_ssh::Error::Io(io::Error::new(io::ErrorKind::Other, format!("Failed to create poller: {}", e)))
        })?;

        Ok(Self {
            poller,
            sources: Slab::new(),
            fd_to_token: HashMap::new(),
        })
    }

    /// Register a TCP stream for I/O notifications
    pub fn register_tcp_stream(
        &mut self,
        stream: &TcpStream,
        interest: Interest,
    ) -> Result<Token> {
        let fd = stream.as_raw_fd();
        
        // Check if already registered
        if let Some(&existing_token) = self.fd_to_token.get(&fd) {
            // Update interest
            self.modify_interest(existing_token, interest)?;
            return Ok(existing_token);
        }

        let entry = self.sources.vacant_entry();
        let token = Token(entry.key());

        // Store source first to avoid borrow checker issues
        let io_source = IoSource {
            token,
            interest,
            waker: None,
        };
        entry.insert(io_source);

        // Register with poller
        let borrowed_fd = unsafe { BorrowedFd::borrow_raw(fd) };
        unsafe {
            self.poller.add(&borrowed_fd, Event::none(token.0)).map_err(|e| {
                crate::async_ssh::Error::Io(io::Error::new(io::ErrorKind::Other, format!("Failed to register stream: {}", e)))
            })?;
        }

        // Update interest
        self.modify_interest_internal(fd, token, interest)?;

        self.fd_to_token.insert(fd, token);
        Ok(token)
    }

    /// Modify interest flags for a registered source
    pub fn modify_interest(&mut self, token: Token, interest: Interest) -> Result<()> {
        if let Some(source) = self.sources.get_mut(token.0) {
            source.interest = interest;

            // Find the file descriptor
            let fd = self.fd_to_token.iter()
                .find(|(_, &t)| t == token)
                .map(|(&fd, _)| fd)
                .ok_or_else(|| {
                    crate::async_ssh::Error::Io(io::Error::new(io::ErrorKind::NotFound, "Token not found"))
                })?;

            self.modify_interest_internal(fd, token, interest)?;
            Ok(())
        } else {
            Err(crate::async_ssh::Error::Io(io::Error::new(io::ErrorKind::InvalidInput, "Invalid token")))
        }
    }

    fn modify_interest_internal(
        &mut self,
        fd: i32,
        token: Token,
        interest: Interest,
    ) -> Result<()> {
        let event = if interest.readable && interest.writable {
            Event::all(token.0)
        } else if interest.readable {
            Event::readable(token.0)
        } else if interest.writable {
            Event::writable(token.0)
        } else {
            Event::none(token.0)
        };

        let borrowed_fd = unsafe { BorrowedFd::borrow_raw(fd) };
        self.poller.modify(&borrowed_fd, event).map_err(|e| {
            crate::async_ssh::Error::Io(io::Error::new(io::ErrorKind::Other, format!("Failed to modify interest: {}", e)))
        })?;

        Ok(())
    }

    /// Register a waker for the given token
    pub fn register_waker(&mut self, token: Token, waker: SshBindWaker) -> Result<()> {
        if let Some(source) = self.sources.get_mut(token.0) {
            source.waker = Some(waker);
            Ok(())
        } else {
            Err(crate::async_ssh::Error::Io(io::Error::new(io::ErrorKind::InvalidInput, "Invalid token")))
        }
    }

    /// Poll for I/O events and wake corresponding tasks
    pub fn poll_events(&mut self, timeout: Duration) -> Result<usize> {
        let mut events = Events::new();
        
        let ready_count = self.poller.wait(&mut events, Some(timeout)).map_err(|e| {
            crate::async_ssh::Error::Io(io::Error::new(io::ErrorKind::Other, format!("Polling failed: {}", e)))
        })?;

        // Wake tasks for ready events
        for event in events.iter() {
            let token = Token(event.key);
            
            if let Some(source) = self.sources.get(token.0) {
                if let Some(waker) = &source.waker {
                    waker.wake();
                }
            }
        }

        Ok(ready_count)
    }

    /// Unregister an I/O source
    pub fn unregister(&mut self, token: Token) -> Result<()> {
        if let Some(_source) = self.sources.try_remove(token.0) {
            // Find and remove file descriptor mapping
            let fd_to_remove = self.fd_to_token.iter()
                .find(|(_, &t)| t == token)
                .map(|(&fd, _)| fd);

            if let Some(fd) = fd_to_remove {
                self.fd_to_token.remove(&fd);
                
                // Remove from poller
                let borrowed_fd = unsafe { BorrowedFd::borrow_raw(fd) };
                if let Err(e) = self.poller.delete(&borrowed_fd) {
                    log::warn!("Failed to unregister fd {}: {}", fd, e);
                }
            }

            Ok(())
        } else {
            Err(crate::async_ssh::Error::Io(io::Error::new(io::ErrorKind::InvalidInput, "Invalid token")))
        }
    }

    /// Check if a token is registered
    pub fn is_registered(&self, token: Token) -> bool {
        self.sources.contains(token.0)
    }

    /// Get the number of registered sources
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }
}