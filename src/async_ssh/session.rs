use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use ssh2::{Session, KeyboardInteractivePrompt};

use crate::async_ssh::{AsyncTcpStream, AsyncChannel, Result};

// Configuration for the SSH session
#[derive(Clone, Debug)]
pub struct SessionConfiguration {
    pub compress: bool,
    pub timeout: Option<u32>,
}

impl Default for SessionConfiguration {
    fn default() -> Self {
        Self {
            compress: false,
            timeout: None,
        }
    }
}

// Async wrapper around ssh2::Session
pub struct AsyncSession {
    inner: Arc<Mutex<Session>>,
    stream: Arc<AsyncTcpStream>,
    config: SessionConfiguration,
}

impl AsyncSession {
    // Connect to an SSH server
    pub async fn connect(
        addr: SocketAddr, 
        config: Option<SessionConfiguration>
    ) -> Result<Self> {
        let stream = AsyncTcpStream::connect(addr).await?;
        let config = config.unwrap_or_default();
        
        let mut session = Session::new()?;
        
        // Apply configuration
        if config.compress {
            session.set_compress(true);
        }
        if let Some(timeout) = config.timeout {
            session.set_timeout(timeout);
        }
        
        // Set TCP stream
        session.set_tcp_stream(stream.inner().try_clone()?);
        
        Ok(Self {
            inner: Arc::new(Mutex::new(session)),
            stream: Arc::new(stream),
            config,
        })
    }
}

impl AsyncSession {
    // Create from existing session and stream
    pub fn from_parts(mut session: Session, stream: AsyncTcpStream, config: SessionConfiguration) -> Result<Self> {
        // Set TCP stream - this is required before handshake
        session.set_tcp_stream(stream.inner().try_clone()?);
        
        Ok(Self {
            inner: Arc::new(Mutex::new(session)),
            stream: Arc::new(stream),
            config,
        })
    }

    // Perform SSH handshake
    pub async fn handshake(&self) -> Result<()> {
        HandshakeFuture {
            session: self.inner.clone(),
            stream: self.stream.clone(),
        }.await
    }

    // Check if authenticated
    pub fn authenticated(&self) -> bool {
        self.inner.lock().unwrap().authenticated()
    }

    // Get authentication methods for a user
    pub async fn auth_methods(&self, username: &str) -> Result<String> {
        let session = self.inner.lock().unwrap();
        match session.auth_methods(username) {
            Ok(methods) => Ok(methods.to_string()),
            Err(e) => Err(e.into()),
        }
    }

    // Authenticate with password
    pub async fn userauth_password(&self, username: &str, password: &str) -> Result<()> {
        let username = username.to_string();
        let password = password.to_string();
        let session = self.inner.clone();
        
        UserAuthPasswordFuture {
            session,
            stream: self.stream.clone(),
            username,
            password,
        }.await
    }

    // Authenticate with keyboard-interactive
    pub async fn userauth_keyboard_interactive<P>(
        &self,
        username: &str,
        prompter: &mut P,
    ) -> Result<()>
    where
        P: KeyboardInteractivePrompt,
    {
        // SSH2 keyboard-interactive is synchronous, so we wrap it
        let session = self.inner.lock().unwrap();
        session.userauth_keyboard_interactive(username, prompter)?;
        Ok(())
    }

    // Authenticate with public key file
    pub async fn userauth_pubkey_file(
        &self,
        username: &str,
        pubkey: Option<&Path>,
        privatekey: &Path,
        passphrase: Option<&str>,
    ) -> Result<()> {
        let username = username.to_string();
        let pubkey = pubkey.map(|p| p.to_path_buf());
        let privatekey = privatekey.to_path_buf();
        let passphrase = passphrase.map(|s| s.to_string());
        let session = self.inner.clone();
        
        UserAuthPubkeyFileFuture {
            session,
            stream: self.stream.clone(),
            username,
            pubkey,
            privatekey,
            passphrase,
        }.await
    }

    // Create a session channel
    pub async fn channel_session(&self) -> Result<AsyncChannel> {
        let session = self.inner.clone();
        let stream = self.stream.clone();
        
        ChannelSessionFuture {
            session: session.clone(),
            stream: stream.clone(),
        }.await.map(|channel| {
            AsyncChannel::new(channel, session, stream)
        })
    }

    // Create a direct TCP/IP channel
    pub async fn channel_direct_tcpip(
        &self,
        host: &str,
        port: u16,
        src_host: Option<&str>,
        src_port: u16,
    ) -> Result<AsyncChannel> {
        let host = host.to_string();
        let src_host = src_host.map(|s| s.to_string()).unwrap_or_else(|| "127.0.0.1".to_string());
        let session = self.inner.clone();
        let stream = self.stream.clone();
        
        ChannelDirectTcpipFuture {
            session: session.clone(),
            stream: stream.clone(),
            host,
            port,
            src_host,
            src_port,
        }.await.map(|channel| {
            AsyncChannel::new(channel, session, stream)
        })
    }

    // Disconnect the session
    pub async fn disconnect(
        &self,
        reason: Option<ssh2::DisconnectCode>,
        description: &str,
        lang: Option<&str>,
    ) -> Result<()> {
        let session = self.inner.lock().unwrap();
        session.disconnect(reason, description, lang)?;
        Ok(())
    }
}

// Future for async handshake
struct HandshakeFuture {
    session: Arc<Mutex<Session>>,
    stream: Arc<AsyncTcpStream>,
}

impl Future for HandshakeFuture {
    type Output = Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut session = self.session.lock().unwrap();
        
        match session.handshake() {
            Ok(()) => Poll::Ready(Ok(())),
            Err(e) if would_block(&e) => {
                drop(session);
                // Register with I/O reactor for socket readiness
                let fd = self.stream.as_raw_fd();
                crate::executor::io::REACTOR.lock().unwrap()
                    .set_readable(fd, cx.waker().clone());
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e.into())),
        }
    }
}


// Future for password authentication
struct UserAuthPasswordFuture {
    session: Arc<Mutex<Session>>,
    stream: Arc<AsyncTcpStream>,
    username: String,
    password: String,
}

impl Future for UserAuthPasswordFuture {
    type Output = Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let session = self.session.lock().unwrap();
        
        match session.userauth_password(&self.username, &self.password) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(e) if would_block(&e) => {
                drop(session);
                // Register with I/O reactor for socket readiness
                let fd = self.stream.as_raw_fd();
                crate::executor::io::REACTOR.lock().unwrap()
                    .set_readable(fd, cx.waker().clone());
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e.into())),
        }
    }
}

// Future for public key file authentication
struct UserAuthPubkeyFileFuture {
    session: Arc<Mutex<Session>>,
    stream: Arc<AsyncTcpStream>,
    username: String,
    pubkey: Option<std::path::PathBuf>,
    privatekey: std::path::PathBuf,
    passphrase: Option<String>,
}

impl Future for UserAuthPubkeyFileFuture {
    type Output = Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let session = self.session.lock().unwrap();
        
        match session.userauth_pubkey_file(
            &self.username,
            self.pubkey.as_deref(),
            &self.privatekey,
            self.passphrase.as_deref(),
        ) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(e) if would_block(&e) => {
                drop(session);
                // Register with I/O reactor for socket readiness
                let fd = self.stream.as_raw_fd();
                crate::executor::io::REACTOR.lock().unwrap()
                    .set_readable(fd, cx.waker().clone());
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e.into())),
        }
    }
}

// Future for creating session channel
struct ChannelSessionFuture {
    session: Arc<Mutex<Session>>,
    stream: Arc<AsyncTcpStream>,
}

impl Future for ChannelSessionFuture {
    type Output = Result<ssh2::Channel>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let session = self.session.lock().unwrap();
        
        match session.channel_session() {
            Ok(channel) => Poll::Ready(Ok(channel)),
            Err(e) if would_block(&e) => {
                drop(session);
                // Register with I/O reactor for socket readiness
                let fd = self.stream.as_raw_fd();
                crate::executor::io::REACTOR.lock().unwrap()
                    .set_readable(fd, cx.waker().clone());
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e.into())),
        }
    }
}

// Future for creating direct TCP/IP channel
struct ChannelDirectTcpipFuture {
    session: Arc<Mutex<Session>>,
    stream: Arc<AsyncTcpStream>,
    host: String,
    port: u16,
    src_host: String,
    src_port: u16,
}

impl Future for ChannelDirectTcpipFuture {
    type Output = Result<ssh2::Channel>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let session = self.session.lock().unwrap();
        
        match session.channel_direct_tcpip(&self.host, self.port, Some((&self.src_host, self.src_port))) {
            Ok(channel) => Poll::Ready(Ok(channel)),
            Err(e) if would_block(&e) => {
                drop(session);
                // Register with I/O reactor for socket readiness
                let fd = self.stream.as_raw_fd();
                crate::executor::io::REACTOR.lock().unwrap()
                    .set_readable(fd, cx.waker().clone());
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e.into())),
        }
    }
}

// Helper to check if SSH2 error indicates would block
fn would_block(e: &ssh2::Error) -> bool {
    e.code() == ssh2::ErrorCode::Session(-37) // LIBSSH2_ERROR_EAGAIN
}