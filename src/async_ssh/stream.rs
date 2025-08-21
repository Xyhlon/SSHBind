use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::io::AsRawFd;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::io::{AsyncRead, AsyncWrite};
use polling::{Event, Events, Poller};

// Async wrapper around std::net::TcpStream with proper polling
pub struct AsyncTcpStream {
    inner: Arc<TcpStream>,
    poller: Arc<Poller>,
}

impl AsyncTcpStream {
    pub async fn connect(addr: SocketAddr) -> io::Result<Self> {
        // Connect with non-blocking I/O
        let stream = TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(10))?;
        stream.set_nonblocking(true)?;
        
        let poller = Poller::new()?;
        unsafe {
            poller.add(&stream, Event::all(0))?;
        }
        
        Ok(Self {
            inner: Arc::new(stream),
            poller: Arc::new(poller),
        })
    }

    pub fn from_std(stream: TcpStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        
        let poller = Poller::new()?;
        unsafe {
            poller.add(&stream, Event::all(0))?;
        }
        
        Ok(Self {
            inner: Arc::new(stream),
            poller: Arc::new(poller),
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
        // Try to read
        match (&*self.inner).read(buf) {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // Poll for readability
                let mut events = Events::new();
                match self.poller.wait(&mut events, Some(std::time::Duration::ZERO)) {
                    Ok(_) => {
                        // Check if our socket is readable
                        for ev in events.iter() {
                            if ev.readable {
                                // Try reading again
                                match (&*self.inner).read(buf) {
                                    Ok(n) => return Poll::Ready(Ok(n)),
                                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                                    Err(e) => return Poll::Ready(Err(e)),
                                }
                            }
                        }
                    }
                    Err(_) => {}
                }
                
                // Not ready, register waker for next poll
                cx.waker().wake_by_ref();
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
        // Try to write
        match (&*self.inner).write(buf) {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // Poll for writability
                let mut events = Events::new();
                match self.poller.wait(&mut events, Some(std::time::Duration::ZERO)) {
                    Ok(_) => {
                        // Check if our socket is writable
                        for ev in events.iter() {
                            if ev.writable {
                                // Try writing again
                                match (&*self.inner).write(buf) {
                                    Ok(n) => return Poll::Ready(Ok(n)),
                                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                                    Err(e) => return Poll::Ready(Err(e)),
                                }
                            }
                        }
                    }
                    Err(_) => {}
                }
                
                // Not ready, register waker for next poll
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
        match (&*self.inner).flush() {
            Ok(()) => Poll::Ready(Ok(())),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                cx.waker().wake_by_ref();
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