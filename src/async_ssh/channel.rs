use std::io::{self, Read, Write};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::async_ssh::Result;

// Async wrapper around ssh2::Channel
pub struct AsyncChannel<S> {
    inner: Arc<Mutex<ssh2::Channel>>,
    session: Arc<Mutex<ssh2::Session>>,
    stream: Arc<S>,
    eof_sent: bool,
}

impl<S> AsyncChannel<S> {
    pub(crate) fn new(
        mut channel: ssh2::Channel,
        session: Arc<Mutex<ssh2::Session>>,
        stream: Arc<S>,
    ) -> Self {
        // SSH2 channels inherit non-blocking from session
        // No need to set explicitly
        
        Self {
            inner: Arc::new(Mutex::new(channel)),
            session,
            stream,
            eof_sent: false,
        }
    }

    // Execute a command
    pub async fn exec(&self, command: &str) -> Result<()> {
        let mut channel = self.inner.lock().unwrap();
        
        loop {
            match channel.exec(command) {
                Ok(()) => return Ok(()),
                Err(e) if would_block(&e) => {
                    // Need to wait for I/O
                    drop(channel);
                    tokio::task::yield_now().await;
                    channel = self.inner.lock().unwrap();
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    // Request a PTY
    pub async fn request_pty(
        &self,
        term: &str,
        mode: Option<ssh2::PtyModes>,
        dim: Option<(u32, u32, u32, u32)>,
    ) -> Result<()> {
        let mut channel = self.inner.lock().unwrap();
        
        loop {
            match channel.request_pty(term, mode.clone(), dim) {
                Ok(()) => return Ok(()),
                Err(e) if would_block(&e) => {
                    drop(channel);
                    tokio::task::yield_now().await;
                    channel = self.inner.lock().unwrap();
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    // Start a shell
    pub async fn shell(&self) -> Result<()> {
        let mut channel = self.inner.lock().unwrap();
        
        loop {
            match channel.shell() {
                Ok(()) => return Ok(()),
                Err(e) if would_block(&e) => {
                    drop(channel);
                    tokio::task::yield_now().await;
                    channel = self.inner.lock().unwrap();
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    // Send EOF
    pub async fn send_eof(&mut self) -> Result<()> {
        if self.eof_sent {
            return Ok(());
        }
        
        let mut channel = self.inner.lock().unwrap();
        
        loop {
            match channel.send_eof() {
                Ok(()) => {
                    self.eof_sent = true;
                    return Ok(());
                }
                Err(e) if would_block(&e) => {
                    drop(channel);
                    tokio::task::yield_now().await;
                    channel = self.inner.lock().unwrap();
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    // Wait for EOF
    pub async fn wait_eof(&self) -> Result<()> {
        loop {
            let mut channel = self.inner.lock().unwrap();
            
            match channel.wait_eof() {
                Ok(()) => return Ok(()),
                Err(e) if would_block(&e) => {
                    drop(channel);
                    tokio::task::yield_now().await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    // Close the channel
    pub async fn close(&self) -> Result<()> {
        let mut channel = self.inner.lock().unwrap();
        
        loop {
            match channel.close() {
                Ok(()) => return Ok(()),
                Err(e) if would_block(&e) => {
                    drop(channel);
                    tokio::task::yield_now().await;
                    channel = self.inner.lock().unwrap();
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    // Wait for the channel to close
    pub async fn wait_close(&self) -> Result<()> {
        loop {
            let mut channel = self.inner.lock().unwrap();
            
            match channel.wait_close() {
                Ok(()) => return Ok(()),
                Err(e) if would_block(&e) => {
                    drop(channel);
                    tokio::task::yield_now().await;
                }
                Err(e) => return Err(e.into()),
            }
        }
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

impl<S> AsyncRead for AsyncChannel<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut channel = self.inner.lock().unwrap();
        
        // Try to read from the channel using std::io::Read
        let mut temp = vec![0u8; buf.remaining()];
        match channel.read(&mut temp) {
            Ok(0) => {
                // EOF reached
                Poll::Ready(Ok(()))
            }
            Ok(n) => {
                buf.put_slice(&temp[..n]);
                Poll::Ready(Ok(()))
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // Need to wait for data
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

impl<S> AsyncWrite for AsyncChannel<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut channel = self.inner.lock().unwrap();
        
        match channel.write(buf) {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                cx.waker().wake_by_ref();
                Poll::Pending
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
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_shutdown(
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
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
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
                cx.waker().wake_by_ref();
                Poll::Pending
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