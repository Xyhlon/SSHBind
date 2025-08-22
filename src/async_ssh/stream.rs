use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::io::{AsyncRead, AsyncWrite};

use crate::async_ssh::reactor::{Reactor, Token};

/// Async wrapper around std::net::TcpStream
pub struct AsyncTcpStream {
    inner: TcpStream,
    token: Token,
    pub reactor: Arc<Reactor>,
}

impl AsyncTcpStream {
    pub fn new(stream: TcpStream, reactor: Arc<Reactor>) -> io::Result<Self> {
        let token = Token::next();
        reactor.register_socket(&stream, token)?;
        
        Ok(AsyncTcpStream {
            inner: stream,
            token,
            reactor,
        })
    }

    pub fn token(&self) -> Token {
        self.token
    }

    pub fn as_raw_stream(&self) -> &TcpStream {
        &self.inner
    }
}

impl Drop for AsyncTcpStream {
    fn drop(&mut self) {
        let _ = self.reactor.remove_socket(&self.inner);
        self.reactor.remove_waker(self.token);
    }
}

impl AsyncRead for AsyncTcpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match self.inner.read(buf) {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.reactor.register_waker(self.token, cx.waker().clone());
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

impl AsyncWrite for AsyncTcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.inner.write(buf) {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.reactor.register_waker(self.token, cx.waker().clone());
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.inner.flush() {
            Ok(()) => Poll::Ready(Ok(())),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.reactor.register_waker(self.token, cx.waker().clone());
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // TCP streams don't need explicit close
        Poll::Ready(Ok(()))
    }
}

