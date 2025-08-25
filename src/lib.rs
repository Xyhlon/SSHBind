use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use std::error::Error;
use std::fmt;
use std::net::{SocketAddr, ToSocketAddrs};
use std::process::Command;
use std::str::FromStr;
use std::thread;
use url::{Host, Url};

use async_ssh2_lite::ssh2::{KeyboardInteractivePrompt, Prompt};
use async_ssh2_lite::{AsyncSession, AsyncSessionStream, SessionConfiguration};
use libreauth::oath::TOTPBuilder;
use ssh2_config::{ParseRule, SshConfig};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::BufReader;
use std::time::Duration;

use async_channel::{Receiver, Sender};
use async_executor::{Executor, Task};
use async_io::Timer;
use async_ssh2_lite::async_io::Async;
use futures::future::FutureExt;
use futures::{select, AsyncReadExt, AsyncWriteExt};
// use futures_lite::future;
use std::net::{TcpListener as StdTcpListener, TcpStream};

#[cfg(not(unix))]
use async_io::Async as AsyncTcpListener;

use log::{error, info, warn};
use std::sync::{Arc, Condvar, LazyLock, Mutex};

type AsyncTcpStream = Async<TcpStream>;
type AsyncTcpListener = Async<StdTcpListener>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPort {
    pub host: String,
    pub port: u16,
}

impl TryFrom<&str> for HostPort {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        let url = Url::parse(&format!("ssh://{}", s))
            .map_err(|e| format!("invalid host:port syntax: {}", e))?;

        let host = url
            .host()
            .ok_or_else(|| "missing host".to_string())
            .map(|h| match h {
                Host::Domain(d) => d.to_string(),
                Host::Ipv4(d) => d.to_string(),
                Host::Ipv6(d) => d.to_string(),
            })?;

        let port = url.port().ok_or_else(|| "missing port".to_string())?;

        Ok(HostPort { host, port })
    }
}

impl FromStr for HostPort {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        HostPort::try_from(s)
    }
}

impl fmt::Display for HostPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

impl From<HostPort> for SocketAddr {
    fn from(hp: HostPort) -> SocketAddr {
        let s = format!("{}:{}", hp.host, hp.port);
        s.to_socket_addrs()
            .expect("Failed to convert HostPort to SocketAddr")
            .next()
            .expect("HostPort verified earlier")
    }
}

pub struct TotpPromptHandler {
    creds: Creds,
}

impl KeyboardInteractivePrompt for TotpPromptHandler {
    fn prompt(
        &mut self,
        username: &str,
        instructions: &str,
        prompts: &[Prompt<'_>],
    ) -> Vec<String> {
        info!("Keyboard Authenticating user: {}", username);
        if !instructions.is_empty() {
            info!("Instructions: {}", instructions);
        }

        let mut responses = Vec::with_capacity(prompts.len());
        for prompt in prompts {
            // Print the prompt text and flush to ensure it appears before input.
            let response = match prompt.text.to_lowercase() {
                s if s.contains("user") => self.creds.username.clone(),
                s if (s.contains("otp")
                    || s.contains("2fa")
                    || s.contains("one")
                    || s.contains("time")
                    || s.contains("code")
                    || s.contains("token")) =>
                {
                    TOTPBuilder::new()
                        .base32_key(&self.creds.totp_key.clone().expect("TOTP key is required"))
                        .finalize()
                        .expect("Failed to build totp builder")
                        .generate()
                }
                s if s.contains("pass") => self.creds.password.clone(),
                _ => "".into(),
            };
            let response_type = match response {
                ref s if *s == self.creds.username => "Username",
                ref s if *s == self.creds.password => "Password",
                ref s if s.is_empty() => "Nothing",
                _ => "TOTP Code",
            };
            info!("Prompt: {} | Responsed with {}", prompt.text, response_type);
            responses.push(response);
        }
        responses
    }
}

/// Represents the standard SSH authentication methods.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum AuthMethod {
    Password,
    KeyboardInteractive,
    PublicKey,
    HostBased,
    GssapiWithMic,
}

impl FromStr for AuthMethod {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "password" => Ok(AuthMethod::Password),
            "keyboard-interactive" => Ok(AuthMethod::KeyboardInteractive),
            "publickey" => Ok(AuthMethod::PublicKey),
            "hostbased" => Ok(AuthMethod::HostBased),
            "gssapi-with-mic" => Ok(AuthMethod::GssapiWithMic),
            _ => Err(()),
        }
    }
}

impl fmt::Display for AuthMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let method = match self {
            AuthMethod::Password => "password",
            AuthMethod::KeyboardInteractive => "keyboard-interactive",
            AuthMethod::PublicKey => "publickey",
            AuthMethod::HostBased => "hostbased",
            AuthMethod::GssapiWithMic => "gssapi-with-mic",
        };
        write!(f, "{}", method)
    }
}

/// A parser that preserves the ordering of the allowed authentication methods.
#[derive(Debug)]
struct OrderedAuthMethods {
    methods: Vec<AuthMethod>,
}

impl OrderedAuthMethods {
    /// Parse a comma-separated list of methods (e.g. "password,publickey,hostbased,keyboard-interactive")
    /// into an OrderedAuthMethods instance that preserves ordering.
    fn parse(methods_str: &str) -> Self {
        // Split the input string and convert each method into an AuthMethod.
        // Filtering out unsupported ones.
        let methods: Vec<AuthMethod> = methods_str
            .split(',')
            .filter_map(|m| AuthMethod::from_str(m).ok())
            .collect();
        OrderedAuthMethods { methods }
    }
}

/// Authenticates a user for the given SSH session.
///
/// # Arguments
///
/// * `session` - A reference to the `AsyncSession` object.
/// * `creds_map` - A reference to a map containing credentials for each host.
/// * `host` - The hostname or IP address of the SSH server.
///
/// # Returns
///
/// A `Result` indicating success or failure of the authentication process.
///
async fn userauth(
    session: &AsyncSession<AsyncTcpStream>,
    creds_map: &YamlCreds,
    host: &HostPort,
) -> Result<(), Box<dyn Error>> {
    let creds = creds_map
        .get(&host.to_string())
        .ok_or_else(|| format!("Couldn't find credentials for {}", host))?;
    let username = creds.username.clone();
    let password = creds.password.clone();
    let totp_key = creds.totp_key.clone();

    // Query the allowed methods as a comma-separated string.
    let auth_methods_str = session.auth_methods(&username).await?;
    let mut ordered_auth = OrderedAuthMethods::parse(auth_methods_str);
    info!(
        "Available authentication methods in order: {:?}",
        ordered_auth
    );

    // Continue while there are methods to try and the session isn't authenticated.
    while !ordered_auth.methods.is_empty() && !session.authenticated() {
        // Remove the first (preferred) method.
        let method = ordered_auth.methods.remove(0);
        info!("Trying authentication method: {}", method);
        let result = match method {
            AuthMethod::Password => session.userauth_password(&username, &password).await,
            AuthMethod::KeyboardInteractive => {
                let mut prompter = TotpPromptHandler {
                    creds: creds.clone(),
                };
                session
                    .userauth_keyboard_interactive(&username, &mut prompter)
                    .await
            }
            AuthMethod::PublicKey => {
                info!("Attempting public key authentication via ssh config.");

                let host_params = {
                    // Read and parse the SSH configuration from ~/.ssh/config.
                    let home = dirs::home_dir().expect("No home directory found");
                    let config_path = home.join(".ssh/config");
                    // open file if exists else continue
                    let config_path = if config_path.exists() {
                        config_path
                    } else {
                        info!("SSH config file not found at {:?}. Trying other Authentication Methods", config_path);
                        continue;
                    };
                    let file = File::open(&config_path)
                        .map_err(|e| format!("Failed to open SSH config: {}", e))?;
                    let mut reader = BufReader::new(file);
                    // Parse the SSH config file. if fails continue
                    let config = SshConfig::default().parse(&mut reader, ParseRule::STRICT);

                    let config = match config {
                        Ok(config) => config,
                        Err(_) => {
                            info!(
                            "Failed to parse SSH config: {:?}. Trying other Authentication Methods",
                            config_path
                        );
                            continue;
                        }
                    };

                    // Query the config for this host.
                    config.query(&host.host)
                };

                if let Some(identity_files) = host_params.identity_file {
                    info!("Found IdentityFile in config: {:?}", identity_files);
                    // Use the identity file for public key authentication.
                    // (Adjust the API call if async_ssh2_lite has a different signature.)
                    let identity_file = identity_files.first().expect("No identity files found");
                    session
                        .userauth_pubkey_file(
                            &username,
                            Some(identity_file),
                            identity_file,
                            None, // Optionally supply a passphrase here
                        )
                        .await
                } else {
                    warn!("No IdentityFile found in SSH config for host {}", host);
                    continue;
                }
            }
            _ => {
                info!("Skipping unsupported method: {}", method);
                continue;
            }
        };

        match result {
            Ok(_) if session.authenticated() => {
                info!("Authenticated via {}.", method);
                break;
            }
            Ok(_) => {
                info!("{} authentication succeeded partially.", method);
                // Reparse to get the updated list of allowed methods.
                let new_auth_methods = session.auth_methods(&username).await?;
                ordered_auth = OrderedAuthMethods::parse(new_auth_methods);
                info!("Updated authentication methods: {:?}", ordered_auth);
            }
            Err(e) => match totp_key {
                // this seems hacky but currently the only way to handle partial auth
                Some(_) => {
                    info!("Probably partial auth: {:?}", e);
                    ordered_auth =
                        OrderedAuthMethods::parse(session.auth_methods(&username).await?);
                    ordered_auth.methods.retain(|m| *m != method);
                    ordered_auth.methods.retain(|m| *m != AuthMethod::PublicKey);
                    info!(
                        "Available authentication methods in order: {:?}",
                        ordered_auth
                    );
                }
                None => {
                    error!("Password authentication failed: {:?}", e);
                }
            },
        }
    }

    if session.authenticated() {
        info!("SSH session established and authenticated!");
        Ok(())
    } else {
        let error_msg = format!(
            "Authentication failed for user: {} on host: {}",
            username, host
        );
        error!("{}", error_msg);
        Err(error_msg.into())
    }
}

fn connect_chain_tunnel<A, B>(
    ex: Arc<Executor<'_>>,
    mut a: A,
    mut b: B,
    _cancel_rx: Receiver<()>,
) -> Task<std::io::Result<()>>
where
    A: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
    B: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
{
    ex.spawn(async move {
        info!("Bridge task starting");

        let mut buf_a = vec![0; 16 * 1024];
        let mut buf_b = vec![0; 16 * 1024];

        loop {
            select! {
                ret_a = a.read(&mut buf_a).fuse() => match ret_a {
                    Ok(0) => {
                        info!("Bridge connection A read 0");
                        break;
                    },
                    Ok(n) => {
                        if let Err(err) = b.write_all(&buf_a[..n]).await {
                            if err.kind() != std::io::ErrorKind::BrokenPipe {
                                error!("Bridge write to B failed: {:?}", err);
                            }
                            break;
                        }
                    },
                    Err(err) => {
                        if err.kind() != std::io::ErrorKind::UnexpectedEof {
                            error!("Bridge read from A failed: {:?}", err);
                        }
                        break;
                    }
                },
                ret_b = b.read(&mut buf_b).fuse() => match ret_b {
                    Ok(0) => {
                        info!("Bridge connection B read 0");
                        break;
                    },
                    Ok(n) => {
                        if let Err(err) = a.write_all(&buf_b[..n]).await {
                            if err.kind() != std::io::ErrorKind::BrokenPipe {
                                error!("Bridge write to A failed: {:?}", err);
                            }
                            break;
                        }
                    },
                    Err(err) => {
                        if err.kind() != std::io::ErrorKind::UnexpectedEof {
                            error!("Bridge read from B failed: {:?}", err);
                        }
                        break;
                    }
                },
            }
        }

        Ok(())
    })
}

/// Establishes an SSH session chain through the given jump hosts.
///
/// # Arguments
///
/// * `jump_hosts` - A slice of strings representing the jump host addresses.
/// * `creds_map` - A reference to a map containing credentials for each host.
///
/// # Returns
///
/// A `Result` containing the final `AsyncSession` connected to the last host in the chain,
/// or an error if the connection fails.
///
type SharedSession = Arc<Mutex<Option<AsyncSession<AsyncTcpStream>>>>;
type FailureCounter = Arc<Mutex<u32>>;

async fn cleanup_session(shared_session: &SharedSession) -> Result<(), Box<dyn Error>> {
    info!("Starting session cleanup");
    let session_opt = {
        let mut session_guard = shared_session.lock().unwrap();
        session_guard.take()
    };

    if let Some(session) = session_opt {
        info!("Disconnecting existing session");
        let _ = session
            .disconnect(None, "Session healing - cleaning up old session", None)
            .await;
    }
    info!("Session cleanup completed");
    Ok(())
}

async fn rebuild_session(
    shared_session: &SharedSession,
    ex: Arc<Executor<'_>>,
    jump_hosts: &[HostPort],
    creds_map: &YamlCreds,
    cancel_tx: Sender<()>,
) -> Result<(), Box<dyn Error>> {
    info!("Starting session rebuild");

    cleanup_session(shared_session).await?;

    let new_session = connect_chain(ex, jump_hosts, creds_map, cancel_tx).await?;

    {
        let mut session_guard = shared_session.lock().unwrap();
        *session_guard = Some(new_session);
    }

    info!("Session rebuild completed successfully");
    Ok(())
}

async fn heal_session_with_retry(
    shared_session: &SharedSession,
    ex: Arc<Executor<'_>>,
    jump_hosts: &[HostPort],
    creds_map: &YamlCreds,
    cancel_tx: Sender<()>,
    max_attempts: u32,
) -> Result<(), Box<dyn Error>> {
    let mut attempt = 1;

    while attempt <= max_attempts {
        info!("Session healing attempt {} of {}", attempt, max_attempts);

        let error_msg = match rebuild_session(
            shared_session,
            ex.clone(),
            jump_hosts,
            creds_map,
            cancel_tx.clone(),
        )
        .await
        {
            Ok(_) => {
                info!("Session healing successful after {} attempt(s)", attempt);
                return Ok(());
            }
            Err(e) => e.to_string(),
        };

        error!("Session healing attempt {} failed: {}", attempt, error_msg);

        if attempt == max_attempts {
            error!("All session healing attempts exhausted");
            return Err(format!(
                "Session healing failed after {} attempts: {}",
                max_attempts, error_msg
            )
            .into());
        }

        let delay_ms = std::cmp::min(1000 * (1 << (attempt - 1)), 30000);
        warn!("Retrying session healing in {}ms", delay_ms);
        Timer::after(Duration::from_millis(delay_ms)).await;

        attempt += 1;
    }

    Err("Unexpected end of session healing retry loop".into())
}

async fn connect_chain(
    ex: Arc<Executor<'_>>,
    jump_hosts: &[HostPort],
    creds_map: &YamlCreds,
    _cancel_tx: Sender<()>,
) -> Result<AsyncSession<AsyncTcpStream>, Box<dyn Error>> {
    assert!(!jump_hosts.is_empty(), "No jump hosts provided");
    info!("Starting SSH chain connection through {:?}", jump_hosts);

    let stream = AsyncTcpStream::connect(jump_hosts[0].clone()).await?;
    let mut session = AsyncSession::new(stream, SessionConfiguration::default())?;
    session.handshake().await?;
    userauth(&session, creds_map, &jump_hosts[0]).await?;

    for (i, jump) in jump_hosts.iter().enumerate().skip(1) {
        info!("Connecting through jump {}: {}", i, jump);
        info!("Creating channel to {}:{}", jump.host, jump.port);
        let channel = session
            .channel_direct_tcpip(&jump.host, jump.port, None)
            .await?;
        info!("Channel created successfully!");

        let listener = AsyncTcpListener::bind("127.0.0.1:0".parse::<SocketAddr>()?)?;
        let local_addr = listener.get_ref().local_addr()?;
        info!("Local bridge listening on {}", local_addr);

        let accept_task =
            ex.spawn(async move { listener.accept().await.map(|(local_conn, _)| local_conn) });

        let client_conn = AsyncTcpStream::connect(local_addr).await?;
        let server_conn = accept_task.await.map_err(|e| e.to_string())?;

        // Use the chain tunnel version to keep the connection alive
        let (_bridge_tx, bridge_rx) = async_channel::unbounded();
        connect_chain_tunnel(ex.clone(), server_conn, channel, bridge_rx).detach();

        // Give the bridge task a moment to start
        Timer::after(Duration::from_millis(100)).await;

        session = AsyncSession::new(client_conn, SessionConfiguration::default())?;
        session.handshake().await?;
        userauth(&session, creds_map, &jump_hosts[i]).await?;
    }

    Ok(session)
}

fn should_trigger_session_healing(error: &dyn Error) -> bool {
    let error_str = error.to_string().to_lowercase();
    error_str.contains("channel session")
        || error_str.contains("connection reset")
        || error_str.contains("broken pipe")
        || error_str.contains("not authenticated")
        || error_str.contains("session closed")
        || error_str.contains("ssh disconnect")
        || error_str.contains("transport read")
        || error_str.contains("unable to send channel-open request")
        || error_str.contains("channel open fail")
        || error_str.contains("session(-7)")
        || error_str.contains("would block")
}

async fn is_session_healthy(session: &AsyncSession<AsyncTcpStream>) -> bool {
    // Try to create a simple channel to test if the session is still working
    match session.channel_session().await {
        Ok(mut channel) => {
            // Close the channel properly
            let _ = channel.close().await;
            let _ = channel.wait_close().await;
            true
        }
        Err(e) => {
            warn!("Session health check failed: {}", e);
            false
        }
    }
}

async fn connect_to_tunnel(
    ex: Arc<Executor<'_>>,
    inbound: AsyncTcpStream,
    shared_session: SharedSession,
    jump_hosts: &[HostPort],
    remote_addr: Option<&HostPort>,
    cmd: Option<&str>,
    cancel_rx: Receiver<()>,
    failure_counter: FailureCounter,
    creds_map: &YamlCreds,
    tunnel_cancel_tx: Sender<()>,
) -> Result<(), Box<dyn Error>> {
    let session = {
        let session_guard = shared_session.lock().unwrap();
        match &*session_guard {
            Some(session) => session.clone(),
            None => {
                error!("No active session available");
                return Err("No active session available".into());
            }
        }
    };

    let (connection_success, should_heal, current_failures, error_msg_opt) =
        match connect_to_tunnel_inner(
            ex.clone(),
            inbound,
            session,
            jump_hosts,
            remote_addr,
            cmd,
            cancel_rx,
        )
        .await
        {
            Ok(_) => {
                // Reset failure counter on successful connection
                let mut counter = failure_counter.lock().unwrap();
                *counter = 0;
                (true, false, 0, None)
            }
            Err(e) => {
                let error_str = e.to_string();
                let is_healable = should_trigger_session_healing(&*e);

                if is_healable {
                    warn!("Connection failed with healable error: {}", error_str);

                    // Increment failure counter
                    let current_failures = {
                        let mut counter = failure_counter.lock().unwrap();
                        *counter += 1;
                        *counter
                    };
                    (false, true, current_failures, Some(error_str))
                } else {
                    // Reset failure counter for non-healable errors
                    let mut counter = failure_counter.lock().unwrap();
                    *counter = 0;
                    (false, false, 0, Some(error_str))
                }
            }
        };

    // Handle healing outside the match to avoid Send issues
    if should_heal && current_failures >= 2 {
        warn!(
            "Multiple connection failures detected ({}), triggering immediate session healing",
            current_failures
        );

        if let Err(heal_error) = heal_session_with_retry(
            &shared_session,
            ex.clone(),
            jump_hosts,
            creds_map,
            tunnel_cancel_tx.clone(),
            3,
        )
        .await
        {
            let heal_error_str = heal_error.to_string();
            error!("Immediate session healing failed: {}", heal_error_str);
        } else {
            // Reset failure counter on successful healing
            let mut counter = failure_counter.lock().unwrap();
            *counter = 0;
            info!("Session healing completed, failure counter reset");
        }
    }

    if connection_success {
        Ok(())
    } else {
        Err(error_msg_opt
            .unwrap_or_else(|| "Unknown connection error".to_string())
            .into())
    }
}

async fn connect_to_tunnel_inner<S>(
    ex: Arc<Executor<'_>>,
    inbound: AsyncTcpStream,
    session: AsyncSession<S>,
    jump_hosts: &[HostPort],
    remote_addr: Option<&HostPort>,
    cmd: Option<&str>,
    cancel_rx: Receiver<()>,
) -> Result<(), Box<dyn Error>>
where
    S: AsyncSessionStream + Send + Sync + 'static,
{
    if jump_hosts.is_empty() {
        let socket = remote_addr
            .expect("Remote address is required when no jump hosts are provided")
            .to_string()
            .to_socket_addrs()?
            .next()
            .expect("Failed to resolve remote address");
        let outbound = AsyncTcpStream::connect(socket).await?;

        connect_chain_tunnel(ex, inbound, outbound, cancel_rx).detach();
        // tokio::spawn(async move {
        //     let _ = copy_bidirectional(&mut inbound, &mut outbound).await;
        // });
    } else {
        info!("Command executed: {}", cmd.unwrap_or("No command provided"));
        match (cmd, remote_addr) {
            (Some(cmd), Some(remote_addr)) => {
                // Execute the command on the remote server and
                // then assume a local socket is opened to which
                // we can connect to on the remote_addr.
                let mut channel = session.channel_session().await?; // dies here
                info!("SSH channel established ");
                channel.exec(cmd).await?;

                // Wait for started service to be ready
                // I know this isn't the cleanest solution
                // open for suggestions
                let mut counter = 0;
                // Use async sleep instead of blocking thread::sleep - increase for iperf3
                loop {
                    match session
                        .channel_direct_tcpip(&remote_addr.host, remote_addr.port, None)
                        .await
                    {
                        Ok(channel) => {
                            info!("SSH channel established ");
                            info!("Connected to remote address: {}", remote_addr);
                            connect_chain_tunnel(ex.clone(), inbound, channel, cancel_rx).detach();
                            break;
                        }
                        Err(err) => {
                            if counter >= 10 {
                                error!("Failed to connect to remote address {} after 10 attempts: {err}", remote_addr);
                                return Err(err.into());
                            }
                            counter += 1;

                            // Exponential backoff for high concurrency
                            let delay = 5 * counter;
                            error!("remote not ready yet: {err}, retrying in {}ms…", delay);
                            Timer::after(Duration::from_millis(delay)).await;
                        }
                    }
                }
            }
            (Some(cmd), None) => {
                // Execute the command on the remote server and
                // then assume since no remote_addr is provided
                // that communication is done over via stdio over
                // the channel.
                let mut channel = session.channel_session().await?;
                info!("SSH channel established ");
                channel.exec(cmd).await?;
                // let exit_code = channel.exit_status().expect("Failed to get exit code");
                // if exit_code != 0 {
                //     error!("Command execution failed with exit code: {}", exit_code);
                // }
                connect_chain_tunnel(ex.clone(), inbound, channel, cancel_rx).detach();
            }
            (None, Some(remote_addr)) => {
                // It is assumed that after the jumping through the
                // jump list one is on a host from which the service
                // is reachable and already running.
                match session
                    .channel_direct_tcpip(&remote_addr.host, remote_addr.port, None)
                    .await
                {
                    Ok(channel) => {
                        info!("SSH channel established ");
                        info!("Connected to remote address: {}", remote_addr);
                        connect_chain_tunnel(ex.clone(), inbound, channel, cancel_rx).detach();
                    }
                    Err(err) => return Err(err.into()),
                }
            }
            (None, None) => {
                error!("Either a command or a remote address must be provided.");
                return Err("Either a command or a remote address must be provided.".into());
            }
        }
    }
    Ok(())
}

/// Runs an asynchronous TCP server that listens for incoming connections and forwards them
/// through a chain of SSH jump hosts to a specified remote address.
///
/// # Arguments
///
/// * `addr` - The local address to bind the server to (e.g., "127.0.0.1:8000").
/// * `jump_hosts` - A vector of SSH jump host addresses (e.g., ["jump1.example.com:22", "jump2.example.com:22"]).
/// * `remote_addr` - The final remote address to forward connections to (e.g., "remote.example.com:80").
/// * `creds` - A map containing SSH credentials for each host.
/// * `cancel_token` - A cancellation token used to gracefully shut down the server.
///
/// # Returns
///
/// This function returns a `Result` indicating success or failure. On success, it runs the server
/// indefinitely until the cancellation token is triggered.
///
/// # Errors
///
/// This function will return an error if:
///
/// * Binding to the specified local address fails.
/// * Accepting incoming connections fails.
/// * Establishing an SSH session to any of the jump hosts fails.
/// * Forwarding data between the local connection and the SSH channel fails.
///
async fn run_server(
    ex: Arc<Executor<'_>>,
    addr: &str,
    jump_hosts: Vec<HostPort>,
    remote_addr: Option<HostPort>,
    cmd: Option<&str>,
    creds: YamlCreds,
    cancel_rx: Receiver<()>,
    pair: Arc<(Mutex<bool>, Condvar)>,
) -> Result<(), Box<dyn Error>> {
    let listener = AsyncTcpListener::bind(addr.parse::<SocketAddr>()?)?;
    info!("Listening on {addr}");
    {
        let (lock, cvar) = &*pair;
        let mut pending = lock.lock().unwrap();
        *pending = false;
        // We notify the condvar that the value has changed.
        cvar.notify_one();
    }
    let remote_addr = remote_addr.as_ref();
    let (tunnel_cancel_tx, tunnel_cancel_rx) = async_channel::unbounded();
    let initial_session =
        connect_chain(ex.clone(), &jump_hosts, &creds, tunnel_cancel_tx.clone()).await?;
    let shared_session: SharedSession = Arc::new(Mutex::new(Some(initial_session)));
    let failure_counter: FailureCounter = Arc::new(Mutex::new(0));
    info!("SSH session established");

    // Start session health monitor
    let session_monitor = {
        let shared_session_clone = shared_session.clone();
        let failure_counter_clone = failure_counter.clone();
        let ex_clone = ex.clone();
        let jump_hosts_clone = jump_hosts.clone();
        let creds_clone = creds.clone();
        let tunnel_cancel_tx_clone = tunnel_cancel_tx.clone();

        ex.spawn(async move {
            let mut healing_in_progress = false;
            let mut consecutive_failures = 0u32;

            loop {
                Timer::after(Duration::from_secs(30)).await;

                if healing_in_progress {
                    continue;
                }

                let session_healthy = {
                    let session_opt = {
                        let session_guard = shared_session_clone.lock().unwrap();
                        session_guard.clone()
                    };

                    match session_opt {
                        Some(session) => is_session_healthy(&session).await,
                        None => false,
                    }
                };

                if !session_healthy && !healing_in_progress {
                    healing_in_progress = true;
                    consecutive_failures += 1;

                    warn!(
                        "Session health check failed, attempting healing (failure #{})",
                        consecutive_failures
                    );

                    match heal_session_with_retry(
                        &shared_session_clone,
                        ex_clone.clone(),
                        &jump_hosts_clone,
                        &creds_clone,
                        tunnel_cancel_tx_clone.clone(),
                        3,
                    )
                    .await
                    {
                        Ok(_) => {
                            info!("Session health monitor: healing successful");
                            consecutive_failures = 0;
                        }
                        Err(e) => {
                            error!("Session health monitor: healing failed: {}", e);
                        }
                    }

                    healing_in_progress = false;
                } else {
                    consecutive_failures = 0;
                    // Decay the failure counter over time if session is healthy
                    let mut counter = failure_counter_clone.lock().unwrap();
                    if *counter > 0 {
                        *counter = (*counter).saturating_sub(1);
                        if *counter == 0 {
                            info!(
                                "Failure counter decayed to 0, connection issues may be resolved"
                            );
                        }
                    }
                }
            }
        })
    };
    loop {
        select! {
            _ = cancel_rx.recv().fuse() => {
                warn!("Shutdown signal received. Stopping server.");
                break;
            }
            sock_result = listener.accept().fuse() => {
                let sock = match sock_result {
                    Ok((sock, _)) => sock,
                    Err(e) => {
                        error!("Failed to accept connection: {e}");
                        return Err(e.into());
                    }
                };

                // Handle each connection in a separate task
                let shared_session_clone = shared_session.clone();
                let failure_counter_clone = failure_counter.clone();
                let jump_hosts_clone = jump_hosts.clone();
                let remote_addr_clone = remote_addr.cloned();
                let cmd_clone = cmd.map(|s| s.to_string());
                let tunnel_rx = tunnel_cancel_rx.clone();
                let ex_clone = ex.clone();
                let creds_clone = creds.clone();
                let tunnel_cancel_tx_clone = tunnel_cancel_tx.clone();

                ex.spawn(async move {
                    match connect_to_tunnel(
                        ex_clone,
                        sock,
                        shared_session_clone,
                        &jump_hosts_clone,
                        remote_addr_clone.as_ref(),
                        cmd_clone.as_deref(),
                        tunnel_rx,
                        failure_counter_clone,
                        &creds_clone,
                        tunnel_cancel_tx_clone,
                    ).await {
                        Ok(_) => {
                            info!("Connection handled successfully");
                        }
                        Err(e) => {
                            error!("Failed to handle connection: {e}");
                        }
                    }
                }).detach();
            }
        }
    }

    session_monitor.cancel().await;
    cleanup_session(&shared_session).await?;
    Ok(())
}

/// Checks if the `sops` binary is available in the system's PATH and returns its path if found.
///
/// # Returns
///
/// * `Ok(String)` - The absolute path to the `sops` binary.
/// * `Err(String)` - An error message indicating that `sops` was not found.
fn find_sops_binary() -> Result<String, String> {
    // Retrieve the system's PATH environment variable
    if let Ok(paths) = std::env::var("PATH") {
        // Iterate over each path in the PATH variable
        for path in std::env::split_paths(&paths) {
            // Construct the full path to the `sops` executable
            let sops_path = path.join("sops");
            // On Windows, executables typically have a `.exe` extension
            let sops_path_exe = path.join("sops.exe");

            // Check if the `sops` executable exists and is a file
            if sops_path.is_file() {
                return Ok(sops_path.to_string_lossy().to_string());
            } else if sops_path_exe.is_file() {
                return Ok(sops_path_exe.to_string_lossy().to_string());
            }
        }
    }
    Err("`sops` binary not found in system's PATH.".to_string())
}

#[allow(clippy::needless_doctest_main)]
/// Binds a local address to a server that forwards incoming TCP connections through a chain
/// of SSH jump hosts to a specified remote address. The server runs in a separate thread.
///
/// # Arguments
///
/// * `addr` - The local address to bind the server to (e.g., "127.0.0.1:8000").
/// * `jump_hosts` - A vector of SSH jump host addresses (e.g., vec!["jump1.example.com:22", "jump2.example.com:22"]).
/// * `remote_addr` - The final remote address to forward connections to (e.g., "remote.example.com:80").
/// * `sopsfile` - The path to a SOPS-encrypted YAML file containing SSH credentials.
///
/// # Panics
///
/// This function will panic if:
///
/// * The `sops` command is not found in the system's PATH.
/// * Decrypting the SOPS file fails.
/// * Deserializing the decrypted YAML content into credentials fails.
/// * Binding to the specified local address fails.
///
/// # Example
///
/// ```rust,no_run
/// use std::thread;
/// use sshbind::bind;
///
/// fn main() {
///     let addr = "127.0.0.1:8000";
///     let jump_hosts = vec!["jump1.example.com:22".to_string(), "jump2.example.com:22".to_string()];
///     let remote_addr = "remote.example.com:80";
///     let sopsfile = "/path/to/creds.sops.yaml";
///
///     // Start the server in a separate thread
///     let server_thread = thread::spawn(move || {
///         bind(addr, jump_hosts, remote_addr, sopsfile);
///     });
///
///     // Perform other tasks or wait for user input
///
///     // Optionally, join the server thread if you want to wait for its completion
///     // server_thread.join().unwrap();
/// }
/// ```
///
/// Note: Ensure that the `sops` command-line tool is installed and accessible in the system's PATH.
pub fn bind(
    addr: &str,
    jump_hosts: Vec<String>,
    remote_addr: Option<String>,
    sopsfile: &str,
    cmd: Option<String>,
) {
    // Check for the `sops` binary
    let sops_path = match find_sops_binary() {
        Ok(path) => {
            info!("Using `sops` binary at: {}", path);
            path
        }
        Err(err) => {
            error!("{}", err);
            panic!("{}", err);
        }
    };

    let mut binds = BINDINGS.lock().unwrap();

    if !std::path::Path::new(sopsfile).exists() {
        error!("SOPS file not found: {sopsfile}");
        return;
    }

    let output = Command::new(&sops_path)
        .arg("decrypt")
        .arg(sopsfile)
        .output()
        .expect("Failed to execute sops command");

    if !output.status.success() {
        error!(
            "SOPS decryption failed: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let jump_hosts: Vec<HostPort> = jump_hosts
        .iter()
        .map(|host| HostPort::try_from(host.as_str()))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|e| {
            error!("Failed to parse jump hosts: {}", e);
            panic!("Invalid jump host, doesn't conform to URI format of RFC 3986 / RFC 7230 / RFC 9110");
        });

    let remote_addr = remote_addr
        .map(|addr| HostPort::try_from(addr.as_str()))
        .transpose()
        .unwrap_or_else(|e| {
            error!("Failed to parse remote address: {}", e);
            panic!("Invalid remote address, doesn't conform to URI format of RFC 3986 / RFC 7230 / RFC 9110");
        });

    let creds: YamlCreds = match serde_yml::from_str(&String::from_utf8_lossy(&output.stdout)) {
        Ok(creds) => creds,
        Err(e) => {
            error!("Failed to deserialize credentials: {e}");
            return;
        }
    };

    let (cancel_tx, cancel_rx) = async_channel::unbounded();
    let bind_addr = addr.to_string();

    let pair = Arc::new((Mutex::new(true), Condvar::new()));
    let pair_clone = Arc::clone(&pair);

    let handle = thread::spawn(move || {
        use async_executor::{Executor, LocalExecutor};
        use easy_parallel::Parallel;
        use futures::executor::block_on;

        let ex = Arc::new(Executor::new());
        let local_ex = LocalExecutor::new();
        let (trigger, shutdown) = async_channel::unbounded::<()>();

        let _result = Parallel::new()
            .each(0..4, |_| block_on(ex.run(async { shutdown.recv().await })))
            .finish(|| {
                block_on(local_ex.run(async {
                    if let Err(e) = run_server(
                        ex.clone(),
                        &bind_addr,
                        jump_hosts,
                        remote_addr,
                        cmd.as_deref(),
                        creds,
                        cancel_rx,
                        pair_clone,
                    )
                    .await
                    {
                        error!("Server error: {e}");
                    }

                    drop(trigger);
                }))
            });
    });

    binds.insert(addr.to_string(), (cancel_tx, handle));

    // Wait for the thread to start up, bind and listen.
    let (lock, cvar) = &*pair;
    let _guard = cvar
        .wait_while(lock.lock().unwrap(), |pending| *pending)
        .unwrap();
}

/// A map of credentials loaded from a YAML file.
pub type YamlCreds = BTreeMap<String, Creds>;

/// Credentials required for SSH authentication.
#[derive(Debug, PartialEq, Serialize, Deserialize, Clone)]
pub struct Creds {
    /// SSH username.
    pub username: String,
    /// SSH password.
    pub password: String,
    /// Optional base32 TOTP key for two-factor authentication.
    pub totp_key: Option<String>,
}

#[allow(clippy::type_complexity)]
static BINDINGS: LazyLock<Mutex<HashMap<String, (Sender<()>, std::thread::JoinHandle<()>)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Unbinds a previously established binding for the given address.
///
/// This function cancels the running server associated with the provided address
/// and waits for its thread to finish. If the address is not found in the bindings,
/// a warning is logged.
///
/// # Arguments
///
/// * `addr` - The address of the binding to unbind.
///
/// # Example
///
/// ```
/// use sshbind::unbind;
///
/// unbind("127.0.0.1:8000");
/// ```
pub fn unbind(addr: &str) {
    let mut binds = BINDINGS.lock().unwrap();
    if let Some((cancel_tx, handle)) = binds.remove(addr) {
        info!("Destructing binding on {}", addr);
        info!("Signaling cancellation...");
        cancel_tx.close();
        if let Err(e) = handle.join() {
            error!("Failed to join thread for {}: {:?}", addr, e);
        } else {
            info!("Successfully unbound {}", addr);
        }
    } else {
        warn!("No binding found for {}", addr);
    }
}
