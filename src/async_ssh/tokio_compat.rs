//! Tokio compatibility layer for integration tests
//! This allows our async SSH implementation to work with tokio in tests

#[cfg(feature = "tokio")]
use tokio::io::{AsyncRead, AsyncWrite};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::sync::{Arc, Mutex};

use crate::async_ssh::{AsyncSession, AsyncChannel, Result};

/// Tokio-compatible wrapper for AsyncSession
#[cfg(feature = "tokio")]
pub struct TokioAsyncSession {
    inner: AsyncSession,
}

#[cfg(feature = "tokio")]
impl TokioAsyncSession {
    pub async fn connect(addr: std::net::SocketAddr) -> Result<Self> {
        // Use tokio's TcpStream instead of our custom one
        let tokio_stream = tokio::net::TcpStream::connect(addr).await
            .map_err(|e| crate::async_ssh::Error::Io(e))?;
        
        // Convert to std::net::TcpStream 
        let std_stream = tokio_stream.into_std()
            .map_err(|e| crate::async_ssh::Error::Io(e.into()))?;
        
        // Create our AsyncTcpStream from the std stream
        let async_stream = crate::async_ssh::AsyncTcpStream::from_std(std_stream)
            .map_err(|e| crate::async_ssh::Error::Io(e))?;
        
        // Create session using our implementation  
        let session = AsyncSession::connect(addr, None).await?;
        
        Ok(Self { inner: session })
    }
    
    pub async fn handshake(&self) -> Result<()> {
        self.inner.handshake().await
    }
    
    pub async fn userauth_password(&self, user: &str, pass: &str) -> Result<()> {
        self.inner.userauth_password(user, pass).await
    }
    
    pub async fn channel_session(&self) -> Result<AsyncChannel> {
        self.inner.channel_session().await
    }
    
    pub async fn channel_direct_tcpip(
        &self,
        host: &str,
        port: u16,
        src_host: Option<&str>,
        src_port: u16,
    ) -> Result<AsyncChannel> {
        self.inner.channel_direct_tcpip(host, port, src_host, src_port).await
    }
}

/// Enable using our AsyncSession with tokio in tests
#[cfg(feature = "tokio")]
pub async fn create_tokio_compatible_session(addr: std::net::SocketAddr) -> Result<AsyncSession> {
    // Create the session in a spawn_blocking to isolate the runtimes
    let session = tokio::task::spawn_blocking(move || {
        // Use our custom executor in the blocking context
        crate::executor::block_on(async move {
            AsyncSession::connect(addr, None).await
        })
    }).await
    .map_err(|e| crate::async_ssh::Error::Io(io::Error::new(io::ErrorKind::Other, e)))?;
    
    session
}