use std::io::{self, Read, Write};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use futures::io::{AsyncRead, AsyncWrite};

use crate::async_ssh::Result;
use crate::async_ssh::stream::{AsyncTcpStream, REACTOR};

// Async wrapper around ssh2::Channel
pub struct AsyncChannel {
    inner: Arc<Mutex<ssh2::Channel>>,
    stream: Arc<AsyncTcpStream>,
    eof_sent: bool,
}

impl AsyncChannel {
    pub(crate) fn new(
        channel: ssh2::Channel,
        _session: Arc<Mutex<ssh2::Session>>,
        stream: Arc<AsyncTcpStream>,
    ) -> Self {
        // SSH2 channels inherit non-blocking from session
        // No need to set explicitly
        
        Self {
            inner: Arc::new(Mutex::new(channel)),
            stream,
            eof_sent: false,
        }
    }

    // Execute a command
    pub async fn exec(&self, command: &str) -> Result<()> {
        futures::future::poll_fn(|cx| {
            let mut channel = self.inner.lock().unwrap();
            
            match channel.exec(command) {
                Ok(()) => Poll::Ready(Ok(())),
                Err(e) if would_block(&e) => {
                    // Wait for socket to be ready for write
                    match REACTOR.poll_ready(self.stream.key, cx, false) {
                        Poll::Ready(_) => Poll::Pending, // Try again next poll
                        Poll::Pending => Poll::Pending,
                    }
                }
                Err(e) => Poll::Ready(Err(e.into())),
            }
        }).await
    }

    // Request a PTY
    pub async fn request_pty(
        &self,
        term: &str,
        mode: Option<ssh2::PtyModes>,
        dim: Option<(u32, u32, u32, u32)>,
    ) -> Result<()> {
        let mut channel = self.inner.lock().unwrap();
        
        futures::future::poll_fn(|cx| {
            let mut channel = self.inner.lock().unwrap();
            
            match channel.request_pty(term, mode.clone(), dim) {
                Ok(()) => Poll::Ready(Ok(())),
                Err(e) if would_block(&e) => {
                    // Wait for socket to be ready for write
                    match REACTOR.poll_ready(self.stream.key, cx, false) {
                        Poll::Ready(_) => Poll::Pending, // Try again next poll
                        Poll::Pending => Poll::Pending,
                    }
                }
                Err(e) => Poll::Ready(Err(e.into())),
            }
        }).await
    }

    // Start a shell
    pub async fn shell(&self) -> Result<()> {
        let mut channel = self.inner.lock().unwrap();
        
        futures::future::poll_fn(|cx| {
            let mut channel = self.inner.lock().unwrap();
            
            match channel.shell() {
                Ok(()) => Poll::Ready(Ok(())),
                Err(e) if would_block(&e) => {
                    // Wait for socket to be ready for write
                    match REACTOR.poll_ready(self.stream.key, cx, false) {
                        Poll::Ready(_) => Poll::Pending, // Try again next poll
                        Poll::Pending => Poll::Pending,
                    }
                }
                Err(e) => Poll::Ready(Err(e.into())),
            }
        }).await
    }

    // Send EOF
    pub async fn send_eof(&mut self) -> Result<()> {
        if self.eof_sent {
            return Ok(());
        }
        
        futures::future::poll_fn(|cx| {
            let mut channel = self.inner.lock().unwrap();
            
            match channel.send_eof() {
                Ok(()) => {
                    self.eof_sent = true;
                    Poll::Ready(Ok(()))
                }
                Err(e) if would_block(&e) => {
                    // Wait for socket to be ready for write
                    match REACTOR.poll_ready(self.stream.key, cx, false) {
                        Poll::Ready(_) => Poll::Pending, // Try again next poll
                        Poll::Pending => Poll::Pending,
                    }
                }
                Err(e) => Poll::Ready(Err(e.into())),
            }
        }).await
    }

    // Wait for EOF
    pub async fn wait_eof(&self) -> Result<()> {
        futures::future::poll_fn(|cx| {
            let mut channel = self.inner.lock().unwrap();
            
            match channel.wait_eof() {
                Ok(()) => Poll::Ready(Ok(())),
                Err(e) if would_block(&e) => {
                    drop(channel);
                    // Wait for socket to be ready for read
                    match REACTOR.poll_ready(self.stream.key, cx, true) {
                        Poll::Ready(_) => Poll::Pending, // Try again next poll
                        Poll::Pending => Poll::Pending,
                    }
                }
                Err(e) => Poll::Ready(Err(e.into())),
            }
        }).await
    }

    // Close the channel
    pub async fn close(&self) -> Result<()> {
        futures::future::poll_fn(|cx| {
            let mut channel = self.inner.lock().unwrap();
            
            match channel.close() {
                Ok(()) => Poll::Ready(Ok(())),
                Err(e) if would_block(&e) => {
                    // Wait for socket to be ready for write
                    match REACTOR.poll_ready(self.stream.key, cx, false) {
                        Poll::Ready(_) => Poll::Pending, // Try again next poll
                        Poll::Pending => Poll::Pending,
                    }
                }
                Err(e) => Poll::Ready(Err(e.into())),
            }
        }).await
    }

    // Wait for the channel to close
    pub async fn wait_close(&self) -> Result<()> {
        futures::future::poll_fn(|cx| {
            let mut channel = self.inner.lock().unwrap();
            
            match channel.wait_close() {
                Ok(()) => Poll::Ready(Ok(())),
                Err(e) if would_block(&e) => {
                    drop(channel);
                    // Wait for socket to be ready for read
                    match REACTOR.poll_ready(self.stream.key, cx, true) {
                        Poll::Ready(_) => Poll::Pending, // Try again next poll
                        Poll::Pending => Poll::Pending,
                    }
                }
                Err(e) => Poll::Ready(Err(e.into())),
            }
        }).await
    }

    // Get exit status
    pub fn exit_status(&self) -> Result<i32> {
        let channel = self.inner.lock().unwrap();
        Ok(channel.exit_status()?)
    }

    // Check if EOF has been received
    pub fn eof(&self) -> bool {
        let channel = self.inner.lock().unwrap();
        channel.eof()
    }

}

impl AsyncRead for AsyncChannel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let mut channel = self.inner.lock().unwrap();
        
        // Try to read from the channel using std::io::Read
        match channel.read(buf) {
            Ok(0) => {
                // EOF reached
                Poll::Ready(Ok(0))
            }
            Ok(n) => {
                Poll::Ready(Ok(n))
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // Need to wait for socket to be ready for reading
                match REACTOR.poll_ready(self.stream.key, cx, true) {
                    Poll::Ready(_) => {
                        // Socket is ready, but we already tried reading and got WouldBlock
                        // Return Pending to try again on next poll
                        Poll::Pending
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

impl AsyncWrite for AsyncChannel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut channel = self.inner.lock().unwrap();
        
        match channel.write(buf) {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // Need to wait for socket to be ready for writing
                match REACTOR.poll_ready(self.stream.key, cx, false) {
                    Poll::Ready(_) => {
                        // Socket is ready, but we already tried writing and got WouldBlock
                        // Return Pending to try again on next poll
                        Poll::Pending
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        let mut channel = self.inner.lock().unwrap();
        
        match channel.flush() {
            Ok(()) => Poll::Ready(Ok(())),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // Need to wait for socket to be ready for writing
                match REACTOR.poll_ready(self.stream.key, cx, false) {
                    Poll::Ready(_) => {
                        // Socket is ready, but we already tried flushing and got WouldBlock
                        // Return Pending to try again on next poll
                        Poll::Pending
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_close(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        // Send EOF if not already sent
        if !self.eof_sent {
            let mut channel = self.inner.lock().unwrap();
            match channel.send_eof() {
                Ok(()) => {
                    drop(channel);
                    self.eof_sent = true;
                }
                Err(e) if would_block(&e) => {
                    // Need to wait for socket to be ready for writing
                    return match REACTOR.poll_ready(self.stream.key, cx, false) {
                        Poll::Ready(_) => Poll::Pending, // Try again on next poll
                        Poll::Pending => Poll::Pending,
                    };
                }
                Err(e) => {
                    // Convert ssh2::Error to io::Error
                    return Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e)));
                }
            }
        }
        
        // Close the channel
        let mut channel = self.inner.lock().unwrap();
        match channel.close() {
            Ok(()) => Poll::Ready(Ok(())),
            Err(e) if would_block(&e) => {
                // Need to wait for socket to be ready for writing
                match REACTOR.poll_ready(self.stream.key, cx, false) {
                    Poll::Ready(_) => Poll::Pending, // Try again on next poll
                    Poll::Pending => Poll::Pending,
                }
            }
            Err(e) => {
                // Convert ssh2::Error to io::Error
                Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e)))
            }
        }
    }
}

// Helper to check if SSH2 error indicates would block
fn would_block(e: &ssh2::Error) -> bool {
    e.code() == ssh2::ErrorCode::Session(-37) // LIBSSH2_ERROR_EAGAIN
}