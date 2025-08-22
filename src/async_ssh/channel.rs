use std::io::{self, Read, Write};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use futures::io::{AsyncRead, AsyncWrite};
use ssh2::Channel;

use crate::async_ssh::reactor::{Reactor, Token};

/// Async wrapper around ssh2::Channel
pub struct AsyncChannel {
    inner: Arc<Mutex<Channel>>,
    token: Token,
    reactor: Arc<Reactor>,
}

impl AsyncChannel {
    pub fn new(channel: Channel, reactor: Arc<Reactor>) -> Self {
        let token = Token::next();
        
        AsyncChannel {
            inner: Arc::new(Mutex::new(channel)),
            token,
            reactor,
        }
    }

    /// Execute a command on the channel
    pub async fn exec(&mut self, command: &str) -> Result<(), ssh2::Error> {
        let command = command.to_string();
        
        futures::future::poll_fn(|cx| {
            let mut channel = self.inner.lock().unwrap();
            match channel.exec(&command) {
                Ok(()) => Poll::Ready(Ok(())),
                Err(e) => {
                    if e.code() == ssh2::ErrorCode::Session(-37) { // EAGAIN
                        self.reactor.register_waker(self.token, cx.waker().clone());
                        Poll::Pending
                    } else {
                        Poll::Ready(Err(e))
                    }
                }
            }
        }).await
    }

    /// Send EOF on the channel
    pub async fn send_eof(&mut self) -> Result<(), ssh2::Error> {
        futures::future::poll_fn(|cx| {
            let mut channel = self.inner.lock().unwrap();
            match channel.send_eof() {
                Ok(()) => Poll::Ready(Ok(())),
                Err(e) => {
                    if e.code() == ssh2::ErrorCode::Session(-37) { // EAGAIN
                        self.reactor.register_waker(self.token, cx.waker().clone());
                        Poll::Pending
                    } else {
                        Poll::Ready(Err(e))
                    }
                }
            }
        }).await
    }

    /// Wait for EOF on the channel
    pub async fn wait_eof(&mut self) -> Result<(), ssh2::Error> {
        futures::future::poll_fn(|cx| {
            let mut channel = self.inner.lock().unwrap();
            match channel.wait_eof() {
                Ok(()) => Poll::Ready(Ok(())),
                Err(e) => {
                    if e.code() == ssh2::ErrorCode::Session(-37) { // EAGAIN
                        self.reactor.register_waker(self.token, cx.waker().clone());
                        Poll::Pending
                    } else {
                        Poll::Ready(Err(e))
                    }
                }
            }
        }).await
    }

    /// Close the channel
    pub async fn close(&mut self) -> Result<(), ssh2::Error> {
        futures::future::poll_fn(|cx| {
            let mut channel = self.inner.lock().unwrap();
            match channel.close() {
                Ok(()) => Poll::Ready(Ok(())),
                Err(e) => {
                    if e.code() == ssh2::ErrorCode::Session(-37) { // EAGAIN
                        self.reactor.register_waker(self.token, cx.waker().clone());
                        Poll::Pending
                    } else {
                        Poll::Ready(Err(e))
                    }
                }
            }
        }).await
    }

    /// Wait for channel close
    pub async fn wait_close(&mut self) -> Result<(), ssh2::Error> {
        futures::future::poll_fn(|cx| {
            let mut channel = self.inner.lock().unwrap();
            match channel.wait_close() {
                Ok(()) => Poll::Ready(Ok(())),
                Err(e) => {
                    if e.code() == ssh2::ErrorCode::Session(-37) { // EAGAIN
                        self.reactor.register_waker(self.token, cx.waker().clone());
                        Poll::Pending
                    } else {
                        Poll::Ready(Err(e))
                    }
                }
            }
        }).await
    }

}

impl Drop for AsyncChannel {
    fn drop(&mut self) {
        self.reactor.remove_waker(self.token);
    }
}

impl AsyncRead for AsyncChannel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let mut channel = self.inner.lock().unwrap();
        match channel.read(buf) {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.reactor.register_waker(self.token, cx.waker().clone());
                Poll::Pending
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
                self.reactor.register_waker(self.token, cx.waker().clone());
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut channel = self.inner.lock().unwrap();
        match channel.flush() {
            Ok(()) => Poll::Ready(Ok(())),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.reactor.register_waker(self.token, cx.waker().clone());
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // SSH channels are closed via close() method
        Poll::Ready(Ok(()))
    }
}

