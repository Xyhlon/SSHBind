//! Tokio-based backend for async SSH when tokio feature is enabled
//! This allows integration tests to work properly

#[cfg(feature = "tokio")]
pub use tokio_implementation::*;

#[cfg(feature = "tokio")]
mod tokio_implementation {
    use std::net::SocketAddr;
    use tokio::net::TcpStream;
    use crate::async_ssh::{Result, Error, SessionConfiguration};
    use std::sync::{Arc, Mutex};
    use ssh2::Session;
    
    pub struct TokioAsyncSession {
        inner: Arc<Mutex<Session>>,
        stream: TcpStream,
    }
    
    impl TokioAsyncSession {
        pub async fn connect(addr: SocketAddr) -> Result<Self> {
            let stream = TcpStream::connect(addr).await
                .map_err(|e| Error::Io(e))?;
            
            // Convert to std stream for ssh2
            let std_stream = stream.into_std()
                .map_err(|e| Error::Io(e.into()))?;
            
            let mut session = Session::new()
                .map_err(|e| Error::Ssh2(e))?;
            
            session.set_tcp_stream(std_stream);
            
            // Convert back for async operations  
            let stream = TcpStream::from_std(session.tcp_stream().unwrap().try_clone().unwrap())
                .map_err(|e| Error::Io(e))?;
            
            Ok(Self {
                inner: Arc::new(Mutex::new(session)),
                stream,
            })
        }
        
        pub async fn handshake(&mut self) -> Result<()> {
            let session = self.inner.clone();
            tokio::task::spawn_blocking(move || {
                let mut session = session.lock().unwrap();
                session.handshake()
            }).await
            .map_err(|e| Error::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?
            .map_err(|e| Error::Ssh2(e))
        }
        
        pub async fn userauth_password(&self, user: &str, password: &str) -> Result<()> {
            let session = self.inner.clone();
            let user = user.to_string();
            let password = password.to_string();
            
            tokio::task::spawn_blocking(move || {
                let session = session.lock().unwrap();
                session.userauth_password(&user, &password)
            }).await
            .map_err(|e| Error::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?
            .map_err(|e| Error::Ssh2(e))
        }
    }
}