use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::io::{AsyncRead, AsyncWrite};

// Async wrapper around std::net::TcpStream
pub struct AsyncTcpStream {
    inner: Arc<TcpStream>,
}

impl AsyncTcpStream {
    pub async fn connect(addr: SocketAddr) -> io::Result<Self> {
        // Use a timeout-based approach that's better than blocking
        // This connects with a small timeout and retries if needed
        let stream = loop {
            match TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(10)) {
                Ok(stream) => break stream,
                Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                    // Yield to executor and try again
                    futures::future::ready(()).await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        };
        
        // Set to non-blocking for future operations
        stream.set_nonblocking(true)?;
        
        Ok(Self {
            inner: Arc::new(stream),
        })
    }

    pub fn from_std(stream: TcpStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        
        Ok(Self {
            inner: Arc::new(stream),
        })
    }

    pub fn as_raw_fd(&self) -> std::os::unix::io::RawFd {
        use std::os::unix::io::AsRawFd;
        self.inner.as_raw_fd()
    }

    pub fn inner(&self) -> &TcpStream {
        &self.inner
    }
}


impl AsyncRead for AsyncTcpStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        // Try to read directly
        match (&*self.inner).read(buf) {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // Schedule to wake up later
                let waker = cx.waker().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(1));
                    waker.wake();
                });
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

impl AsyncWrite for AsyncTcpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        // Try to write directly
        match (&*self.inner).write(buf) {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // Schedule to wake up later
                let waker = cx.waker().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(1));
                    waker.wake();
                });
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        // Try to flush directly
        match (&*self.inner).flush() {
            Ok(()) => Poll::Ready(Ok(())),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // Schedule to wake up later
                let waker = cx.waker().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(1));
                    waker.wake();
                });
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_close(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        self.inner.shutdown(std::net::Shutdown::Write)?;
        Poll::Ready(Ok(()))
    }
}