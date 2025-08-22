use crate::{
    auth::connect_chain,
    types::*,
    utils::connect_duplex,
    async_ssh::AsyncSession,
    select::select,
};
use futures::FutureExt;
use log::{error, info, warn};
use std::error::Error;
use std::io::{ErrorKind, Result as IoResult};
use std::net::TcpListener;
use std::sync::{Arc, Condvar, Mutex};

/// Accepts incoming TCP connection from listener
async fn accept_connection(
    listener: &TcpListener,
) -> IoResult<crate::async_ssh::AsyncTcpStream> {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(true)?;
                let reactor = std::sync::Arc::new(crate::async_ssh::reactor::Reactor::new()?);
                return Ok(crate::async_ssh::AsyncTcpStream::new(stream, reactor)?);
            }
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                crate::executor::yield_now().await;
                continue;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Handles port forwarding for incoming connection
async fn handle_port_forwarding(
    session: &AsyncSession,
    stream: crate::async_ssh::AsyncTcpStream,
    remote_addr: Option<&HostPort>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if let Some(remote) = remote_addr {
        info!("Forwarding connection to {}:{}", remote.host, remote.port);
        let channel = session
            .channel_direct_tcpip(&remote.host, remote.port, None)
            .await?;
        connect_duplex(stream, channel).await;
    } else {
        info!("No remote address specified, closing connection");
    }
    Ok(())
}

/// Handles command execution for incoming connection
async fn handle_command_execution(
    session: &AsyncSession,
    _stream: crate::async_ssh::AsyncTcpStream,
    cmd: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    info!("Executing command: {}", cmd);
    let mut channel = session.channel_session().await?;
    channel.exec(cmd).await?;
    
    let mut output = Vec::new();
    use futures::io::AsyncReadExt;
    channel.read_to_end(&mut output).await?;
    info!("Command output: {}", String::from_utf8_lossy(&output));
    
    channel.wait_close().await?;
    Ok(())
}

/// Sets up server notification for startup completion
fn setup_server_notification(
    pair: &Arc<(Mutex<bool>, Condvar)>,
) {
    let (lock, cvar) = &**pair;
    if let Ok(mut pending) = lock.lock() {
        *pending = false;
        cvar.notify_one();
    }
}

/// Main server loop that handles incoming connections
async fn server_main_loop(
    listener: &TcpListener,
    session: &AsyncSession,
    remote_addr: Option<&HostPort>,
    cmd: Option<&str>,
    cancel_token: &CancellationToken,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    loop {
        select! {
            _ = cancel_token.cancelled().fuse() => {
                warn!("Shutdown signal received. Stopping server.");
                break;
            }
            result = accept_connection(listener).fuse() => {
                match result {
                    Ok(stream) => {
                        info!("Accepted connection from client");
                        
                        if let Some(cmd) = cmd {
                            if let Err(e) = handle_command_execution(session, stream, cmd).await {
                                error!("Error executing command: {}", e);
                            }
                        } else {
                            if let Err(e) = handle_port_forwarding(session, stream, remote_addr).await {
                                error!("Error handling port forwarding: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        error!("Error accepting connection: {}", e);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Runs the SSH tunnel server
///
/// Establishes SSH connection chain and runs server loop to handle incoming connections
pub async fn run_server(
    addr: &str,
    jump_hosts: Vec<HostPort>,
    remote_addr: Option<HostPort>,
    cmd: Option<&str>,
    creds: YamlCreds,
    cancel_token: CancellationToken,
    pair: Arc<(Mutex<bool>, Condvar)>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let listener = TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    info!("Listening on {addr}");
    
    setup_server_notification(&pair);
    
    let session = connect_chain(&jump_hosts, &creds).await?;
    info!("SSH session established");
    
    server_main_loop(
        &listener,
        &session,
        remote_addr.as_ref(),
        cmd,
        &cancel_token,
    ).await
}