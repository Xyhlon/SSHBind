use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::io::AsRawFd;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::io::{AsyncRead, AsyncWrite};

// Async wrapper around std::net::TcpStream
pub struct AsyncTcpStream {
    inner: Arc<TcpStream>,
}

impl AsyncTcpStream {
    pub async fn connect(addr: SocketAddr) -> io::Result<Self> {
        let stream = TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(10))?;
        stream.set_nonblocking(true)?;
        
        // Register with global reactor
        let fd = stream.as_raw_fd();
        crate::executor::io::REACTOR.lock().unwrap().register(fd)?;
        
        Ok(Self {
            inner: Arc::new(stream),
        })
    }

    pub fn from_std(stream: TcpStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        
        // Register with global reactor
        let fd = stream.as_raw_fd();
        crate::executor::io::REACTOR.lock().unwrap().register(fd)?;
        
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
        let fd = self.inner.as_raw_fd();
        
        // Check if readable
        let is_readable = crate::executor::io::REACTOR.lock().unwrap().is_readable(fd);
        
        if is_readable {
            // Try to read
            match (&*self.inner).read(buf) {
                Ok(n) => Poll::Ready(Ok(n)),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    // Register waker for when it becomes readable
                    crate::executor::io::REACTOR.lock().unwrap()
                        .set_readable(fd, cx.waker().clone());
                    Poll::Pending
                }
                Err(e) => Poll::Ready(Err(e)),
            }
        } else {
            // Try reading anyway (might be ready)
            match (&*self.inner).read(buf) {
                Ok(n) => Poll::Ready(Ok(n)),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    // Register waker for when it becomes readable
                    crate::executor::io::REACTOR.lock().unwrap()
                        .set_readable(fd, cx.waker().clone());
                    Poll::Pending
                }
                Err(e) => Poll::Ready(Err(e)),
            }
        }
    }
}

impl AsyncWrite for AsyncTcpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let fd = self.inner.as_raw_fd();
        
        // Try to write
        match (&*self.inner).write(buf) {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // Register waker for when it becomes writable
                crate::executor::io::REACTOR.lock().unwrap()
                    .set_writable(fd, cx.waker().clone());
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        match (&*self.inner).flush() {
            Ok(()) => Poll::Ready(Ok(())),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_close(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        let fd = self.inner.as_raw_fd();
        crate::executor::io::REACTOR.lock().unwrap().unregister(fd).ok();
        self.inner.shutdown(std::net::Shutdown::Write)?;
        Poll::Ready(Ok(()))
    }
}