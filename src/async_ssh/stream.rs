use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use polling::{Event, Events, Poller};
use futures::io::{AsyncRead, AsyncWrite};

// Global reactor for I/O polling
lazy_static::lazy_static! {
    pub(crate) static ref REACTOR: Reactor = Reactor::new();
}

pub(crate) struct Reactor {
    poller: Poller,
    sources: Mutex<slab::Slab<Source>>,
}

struct Source {
    key: usize,
    waker: Option<Waker>,
    readable: bool,
    writable: bool,
}

impl Reactor {
    fn new() -> Self {
        Self {
            poller: Poller::new().expect("Failed to create poller"),
            sources: Mutex::new(slab::Slab::new()),
        }
    }

    fn register(&self, stream: &TcpStream) -> io::Result<usize> {
        let mut sources = self.sources.lock().unwrap();
        let entry = sources.vacant_entry();
        let key = entry.key();
        
        unsafe {
            self.poller.add(stream, Event::all(key))?;
        }
        
        entry.insert(Source {
            key,
            waker: None,
            readable: false,
            writable: false,
        });
        
        Ok(key)
    }

    fn deregister(&self, stream: &TcpStream, key: usize) -> io::Result<()> {
        self.poller.delete(stream)?;
        let mut sources = self.sources.lock().unwrap();
        sources.remove(key);
        Ok(())
    }

    pub(crate) fn poll_ready(&self, key: usize, cx: &mut Context<'_>, readable: bool) 
        -> Poll<io::Result<()>> 
    {
        let mut sources = self.sources.lock().unwrap();
        
        if let Some(source) = sources.get_mut(key) {
            // Check if already ready
            if readable && source.readable {
                return Poll::Ready(Ok(()));
            }
            if !readable && source.writable {
                return Poll::Ready(Ok(()));
            }
            
            // Register waker
            source.waker = Some(cx.waker().clone());
        }
        
        drop(sources);
        
        // Poll for events
        let mut events = Events::new();
        match self.poller.wait(&mut events, Some(Duration::ZERO)) {
            Ok(_) => {
                let mut sources = self.sources.lock().unwrap();
                for ev in events.iter() {
                    if let Some(source) = sources.get_mut(ev.key) {
                        if ev.readable {
                            source.readable = true;
                        }
                        if ev.writable {
                            source.writable = true;
                        }
                        
                        // Wake if this is our event
                        if ev.key == key {
                            if let Some(waker) = source.waker.take() {
                                waker.wake();
                            }
                        }
                    }
                }
                
                // Check again after processing events
                if let Some(source) = sources.get(key) {
                    if readable && source.readable {
                        return Poll::Ready(Ok(()));
                    }
                    if !readable && source.writable {
                        return Poll::Ready(Ok(()));
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Poll::Ready(Err(e)),
        }
        
        Poll::Pending
    }

    fn clear_readiness(&self, key: usize, readable: bool) {
        let mut sources = self.sources.lock().unwrap();
        if let Some(source) = sources.get_mut(key) {
            if readable {
                source.readable = false;
            } else {
                source.writable = false;
            }
        }
    }
}

// Async wrapper around std::net::TcpStream
pub struct AsyncTcpStream {
    inner: Arc<TcpStream>,
    pub(crate) key: usize,
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
        let key = REACTOR.register(&stream)?;
        
        Ok(Self {
            inner: Arc::new(stream),
            key,
        })
    }

    pub fn from_std(stream: TcpStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        let key = REACTOR.register(&stream)?;
        
        Ok(Self {
            inner: Arc::new(stream),
            key,
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

impl Drop for AsyncTcpStream {
    fn drop(&mut self) {
        let _ = REACTOR.deregister(&self.inner, self.key);
    }
}

impl AsyncRead for AsyncTcpStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        // Wait for readability
        match REACTOR.poll_ready(self.key, cx, true) {
            Poll::Ready(Ok(())) => {
                // Try to read
                match (&*self.inner).read(buf) {
                    Ok(n) => {
                        Poll::Ready(Ok(n))
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        REACTOR.clear_readiness(self.key, true);
                        Poll::Pending
                    }
                    Err(e) => Poll::Ready(Err(e)),
                }
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for AsyncTcpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        // Wait for writability
        match REACTOR.poll_ready(self.key, cx, false) {
            Poll::Ready(Ok(())) => {
                // Try to write
                match (&*self.inner).write(buf) {
                    Ok(n) => Poll::Ready(Ok(n)),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        REACTOR.clear_readiness(self.key, false);
                        Poll::Pending
                    }
                    Err(e) => Poll::Ready(Err(e)),
                }
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        // Wait for writability
        match REACTOR.poll_ready(self.key, cx, false) {
            Poll::Ready(Ok(())) => {
                match (&*self.inner).flush() {
                    Ok(()) => Poll::Ready(Ok(())),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        REACTOR.clear_readiness(self.key, false);
                        Poll::Pending
                    }
                    Err(e) => Poll::Ready(Err(e)),
                }
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
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