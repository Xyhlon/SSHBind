use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::task::Poll;

use ssh2::Session;

pub mod reactor;
mod stream;
mod channel;

use reactor::Reactor;

pub use channel::AsyncChannel;
pub use stream::AsyncTcpStream;

/// Custom async wrapper around ssh2::Session
pub struct AsyncSession {
    inner: Arc<Mutex<Session>>,
    socket: Arc<AsyncTcpStream>,
    reactor: Arc<Reactor>,
}

impl AsyncSession {
    /// Connect to SSH server at given address
    pub async fn connect<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        let socket_addr = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::AddrNotAvailable, "No address found"))?;

        let tcp_stream = TcpStream::connect(socket_addr)?;
        tcp_stream.set_nonblocking(true)?;

        let reactor = Arc::new(Reactor::new()?);
        let socket = Arc::new(AsyncTcpStream::new(tcp_stream, reactor.clone())?);

        let mut session = Session::new()?;
        session.set_tcp_stream(socket.as_raw_stream().try_clone()?);
        
        Ok(AsyncSession {
            inner: Arc::new(Mutex::new(session)),
            socket,
            reactor,
        })
    }

    /// Create session from existing AsyncTcpStream
    pub fn from_stream(stream: AsyncTcpStream) -> io::Result<Self> {
        let reactor = stream.reactor.clone();
        let socket = Arc::new(stream);
        
        let mut session = Session::new()?;
        session.set_tcp_stream(socket.as_raw_stream().try_clone()?);

        Ok(AsyncSession {
            inner: Arc::new(Mutex::new(session)),
            socket,
            reactor,
        })
    }

    /// Get the reactor for spawning tasks
    pub fn reactor(&self) -> Arc<Reactor> {
        self.reactor.clone()
    }

    /// Perform SSH handshake
    pub async fn handshake(&mut self) -> Result<(), ssh2::Error> {
        futures::future::poll_fn(|cx| {
            let mut session = self.inner.lock().unwrap();
            match session.handshake() {
                Ok(()) => Poll::Ready(Ok(())),
                Err(e) => {
                    if e.code() == ssh2::ErrorCode::Session(-37) { // EAGAIN
                        self.reactor.register_waker(self.socket.token(), cx.waker().clone());
                        Poll::Pending
                    } else {
                        Poll::Ready(Err(e))
                    }
                }
            }
        }).await
    }

    /// Authenticate with password
    pub async fn userauth_password(&self, username: &str, password: &str) -> Result<(), ssh2::Error> {
        let username = username.to_string();
        let password = password.to_string();
        
        futures::future::poll_fn(|cx| {
            let session = self.inner.lock().unwrap();
            match session.userauth_password(&username, &password) {
                Ok(()) => Poll::Ready(Ok(())),
                Err(e) => {
                    if e.code() == ssh2::ErrorCode::Session(-37) { // EAGAIN
                        self.reactor.register_waker(self.socket.token(), cx.waker().clone());
                        Poll::Pending
                    } else {
                        Poll::Ready(Err(e))
                    }
                }
            }
        }).await
    }

    /// Authenticate with keyboard interactive
    pub async fn userauth_keyboard_interactive<P>(
        &self,
        username: &str,
        prompter: &mut P,
    ) -> Result<(), ssh2::Error>
    where
        P: ssh2::KeyboardInteractivePrompt,
    {
        let username = username.to_string();
        
        futures::future::poll_fn(|cx| {
            let session = self.inner.lock().unwrap();
            match session.userauth_keyboard_interactive(&username, prompter) {
                Ok(()) => Poll::Ready(Ok(())),
                Err(e) => {
                    if e.code() == ssh2::ErrorCode::Session(-37) { // EAGAIN
                        self.reactor.register_waker(self.socket.token(), cx.waker().clone());
                        Poll::Pending
                    } else {
                        Poll::Ready(Err(e))
                    }
                }
            }
        }).await
    }

    /// Authenticate with public key file
    pub async fn userauth_pubkey_file(
        &self,
        username: &str,
        pubkey: Option<&std::path::Path>,
        privatekey: &std::path::Path,
        passphrase: Option<&str>,
    ) -> Result<(), ssh2::Error> {
        let username = username.to_string();
        let pubkey = pubkey.map(|p| p.to_path_buf());
        let privatekey = privatekey.to_path_buf();
        let passphrase = passphrase.map(|s| s.to_string());
        
        futures::future::poll_fn(|cx| {
            let session = self.inner.lock().unwrap();
            match session.userauth_pubkey_file(
                &username,
                pubkey.as_deref(),
                &privatekey,
                passphrase.as_deref(),
            ) {
                Ok(()) => Poll::Ready(Ok(())),
                Err(e) => {
                    if e.code() == ssh2::ErrorCode::Session(-37) { // EAGAIN
                        self.reactor.register_waker(self.socket.token(), cx.waker().clone());
                        Poll::Pending
                    } else {
                        Poll::Ready(Err(e))
                    }
                }
            }
        }).await
    }

    /// Get available authentication methods
    pub async fn auth_methods(&self, username: &str) -> Result<String, ssh2::Error> {
        let username = username.to_string();
        
        futures::future::poll_fn(|cx| {
            let session = self.inner.lock().unwrap();
            match session.auth_methods(&username) {
                Ok(methods) => Poll::Ready(Ok(methods.to_string())),
                Err(e) => {
                    if e.code() == ssh2::ErrorCode::Session(-37) { // EAGAIN
                        self.reactor.register_waker(self.socket.token(), cx.waker().clone());
                        Poll::Pending
                    } else {
                        Poll::Ready(Err(e))
                    }
                }
            }
        }).await
    }

    /// Check if authenticated
    pub fn authenticated(&self) -> bool {
        self.inner.lock().unwrap().authenticated()
    }

    /// Create a session channel
    pub async fn channel_session(&self) -> Result<AsyncChannel, ssh2::Error> {
        futures::future::poll_fn(|cx| {
            let session = self.inner.lock().unwrap();
            match session.channel_session() {
                Ok(channel) => {
                    Poll::Ready(Ok(AsyncChannel::new(channel, self.reactor.clone())))
                }
                Err(e) => {
                    if e.code() == ssh2::ErrorCode::Session(-37) { // EAGAIN
                        self.reactor.register_waker(self.socket.token(), cx.waker().clone());
                        Poll::Pending
                    } else {
                        Poll::Ready(Err(e))
                    }
                }
            }
        }).await
    }

    /// Create a direct TCP/IP channel
    pub async fn channel_direct_tcpip(
        &self,
        host: &str,
        port: u16,
        src: Option<(&str, u16)>,
    ) -> Result<AsyncChannel, ssh2::Error> {
        let host = host.to_string();
        let src = src.map(|(h, p)| (h.to_string(), p));
        
        futures::future::poll_fn(|cx| {
            let session = self.inner.lock().unwrap();
            let src_ref = src.as_ref().map(|(h, p)| (h.as_str(), *p));
            match session.channel_direct_tcpip(&host, port, src_ref) {
                Ok(channel) => {
                    Poll::Ready(Ok(AsyncChannel::new(channel, self.reactor.clone())))
                }
                Err(e) => {
                    if e.code() == ssh2::ErrorCode::Session(-37) { // EAGAIN
                        self.reactor.register_waker(self.socket.token(), cx.waker().clone());
                        Poll::Pending
                    } else {
                        Poll::Ready(Err(e))
                    }
                }
            }
        }).await
    }
}