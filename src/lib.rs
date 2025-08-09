use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use std::error::Error;
use std::fmt;
use std::net::{SocketAddr, ToSocketAddrs};
use std::process::Command;
use std::str::FromStr;
use std::thread;
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;
use url::{Host, Url};

use async_ssh2_lite::ssh2::{KeyboardInteractivePrompt, Prompt};
use async_ssh2_lite::{AsyncSession, SessionConfiguration, TokioTcpStream};
use libreauth::oath::TOTPBuilder;
use ssh2_config::{ParseRule, SshConfig};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::BufReader;
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};

use log::{error, info, warn};
use std::sync::{Arc, Condvar, LazyLock, Mutex};

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
        let _ = self.creds.clone();

        let mut responses = Vec::with_capacity(prompts.len());
        for prompt in prompts {
            // Print the prompt text and flush to ensure it appears before input.
            let code = TOTPBuilder::new()
                .base32_key(&self.creds.totp_key.clone().unwrap_or("".to_string()))
                .finalize()
                .expect("Failed to build totp builder")
                .generate();
            let response = match prompt.text.to_lowercase() {
                s if s.contains("user") => self.creds.username.clone(),
                s if (s.contains("otp")
                    || s.contains("2fa")
                    || s.contains("one")
                    || s.contains("time")
                    || s.contains("code")
                    || s.contains("token")) =>
                {
                    code.clone()
                }
                s if s.contains("pass") => self.creds.password.clone(),
                _ => "".into(),
            };
            let response_type = match response {
                ref s if *s == self.creds.username => "Username",
                ref s if *s == code => "TOTP Code",
                ref s if *s == self.creds.password => "Password",
                _ => "Nothing",
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
    session: &AsyncSession<TokioTcpStream>,
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

                    let config = if config.is_ok() {
                        config.unwrap()
                    } else {
                        info!(
                            "Failed to parse SSH config: {:?}. Trying other Authentication Methods",
                            config_path
                        );
                        continue;
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
async fn connect_chain(
    jump_hosts: &[HostPort],
    creds_map: &YamlCreds,
) -> Result<AsyncSession<TokioTcpStream>, Box<dyn Error>> {
    assert!(!jump_hosts.is_empty(), "No jump hosts provided");
    info!("Starting SSH chain connection through {:?}", jump_hosts);

    let mut session = AsyncSession::<TokioTcpStream>::connect(jump_hosts[0].clone(), None).await?;
    session.handshake().await?;
    userauth(&session, creds_map, &jump_hosts[0]).await?;

    for (i, jump) in jump_hosts.iter().enumerate().skip(1) {
        info!("Connecting through jump {}: {}", i, jump);
        let mut channel = session
            .channel_direct_tcpip(&jump.host, jump.port, None)
            .await?;

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let local_addr = listener.local_addr()?;

        let accept_task =
            tokio::spawn(async move { listener.accept().await.map(|(local_conn, _)| local_conn) });

        let client_conn = TcpStream::connect(local_addr).await?;
        let mut server_conn = accept_task.await?.map_err(|e| e.to_string())?;

        tokio::spawn(async move {
            let _ = copy_bidirectional(&mut channel, &mut server_conn).await;
        });

        session = AsyncSession::new(client_conn, SessionConfiguration::default())?;
        session.handshake().await?;
        userauth(&session, creds_map, &jump_hosts[i]).await?;
    }

    Ok(session)
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
    addr: &str,
    jump_hosts: Vec<HostPort>,
    remote_addr: Option<HostPort>,
    cmd: Option<&str>,
    creds: YamlCreds,
    cancel_token: CancellationToken,
    pair: Arc<(Mutex<bool>, Condvar)>,
) -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind(addr).await?;
    info!("Listening on {addr}");
    {
        let (lock, cvar) = &*pair;
        let mut pending = lock.lock().unwrap();
        *pending = false;
        // We notify the condvar that the value has changed.
        cvar.notify_one();
    }
    let remote_addr = remote_addr.as_ref();
    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                warn!("Shutdown signal received. Stopping server.");
                break;
            }
            result = listener.accept() => {
                match result {
                    Ok((mut inbound, _)) => {
                        if jump_hosts.is_empty() {
                            let socket = remote_addr
                                .expect("Remote address is required when no jump hosts are provided")
                                .to_string()
                                .to_socket_addrs()?
                                .next()
                                .expect("Failed to resolve remote address");
                            let mut outbound = TcpStream::connect(socket).await?;
                            tokio::spawn(async move {
                                if let Err(e) = copy_bidirectional(&mut inbound, &mut outbound).await {
                                    error!("Connection error: {e}");
                                }
                            });
                        }
                        else{
                            let session = connect_chain(&jump_hosts, &creds).await?;
                            info!("SSH session established");
                            info!("Command executed: {}", cmd.unwrap_or("No command provided"));
                            match (cmd, remote_addr) {
                                (Some(cmd), Some(remote_addr)) => {
                                    // Execute the command on the remote server and
                                    // then assume a local socket is opened to which
                                    // we can connect to on the remote_addr.
                                    let mut channel = session.channel_session().await?; // dies here
                                    info!("SSH channel established ");
                                    channel.exec(cmd).await?;
                                    let exit_code = channel.exit_status().expect("Failed to get exit code");
                                    if exit_code != 0 {
                                        error!("Command execution failed with exit code: {}", exit_code);
                                    }
                                    let mut channel = session.channel_direct_tcpip(&remote_addr.host, remote_addr.port, None).await?;
                                    info!("Connected to remote address: {}", remote_addr);
                                    tokio::spawn(async move {
                                        if let Err(e) = copy_bidirectional(&mut inbound, &mut channel).await {
                                            error!("Connection error: {e}");
                                        }
                                    });
                                }
                                (Some(cmd), None) => {
                                    // Execute the command on the remote server and
                                    // then assume since no remote_addr is provided
                                    // that communication is done over via stdio over
                                    // the channel.
                                    let mut channel = session.channel_session().await?; // dies here
                                    info!("SSH channel established ");
                                    channel.exec(cmd).await?;
                                    let exit_code = channel.exit_status().expect("Failed to get exit code");
                                    if exit_code != 0 {
                                        error!("Command execution failed with exit code: {}", exit_code);
                                    }
                                    tokio::spawn(async move {
                                        if let Err(e) = copy_bidirectional(&mut inbound, &mut channel).await {
                                            error!("Connection error: {e}");
                                        }
                                    });
                                    continue;
                                }
                                (None, Some(remote_addr)) => {
                                    // It is assumed that after the jumping through the
                                    // jump list one is on a host from which the service
                                    // is reachable and already running.
                                    let mut channel = session.channel_direct_tcpip(&remote_addr.host, remote_addr.port, None).await?;
                                    info!("SSH channel established ");
                                    info!("Connected to remote address: {}", remote_addr);
                                    tokio::spawn(async move {
                                        if let Err(e) = copy_bidirectional(&mut inbound, &mut channel).await {
                                            error!("Connection error: {e}");
                                        }
                                    });
                                    continue
                                }
                                (None, None) => {
                                    error!("Either a command or a remote address must be provided.");
                                    return Err("Either a command or a remote address must be provided.".into());
                                }
                            }
                        }
                    }
                    Err(e) => error!("Failed to accept connection: {e}")
                }
            }
        }
    }
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

    let cancel_token = CancellationToken::new();
    let token_clone = cancel_token.clone();
    let bind_addr = addr.to_string();

    let pair = Arc::new((Mutex::new(true), Condvar::new()));
    let pair_clone = Arc::clone(&pair);

    let handle = thread::spawn(move || {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            if let Err(e) = run_server(
                &bind_addr,
                jump_hosts,
                remote_addr,
                cmd.as_deref(),
                creds,
                token_clone,
                pair_clone,
            )
            .await
            {
                error!("Server error: {e}");
            }
        });
    });

    binds.insert(addr.to_string(), (cancel_token, handle));

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
static BINDINGS: LazyLock<
    Mutex<HashMap<String, (CancellationToken, std::thread::JoinHandle<()>)>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

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
    if let Some((cancel_token, handle)) = binds.remove(addr) {
        info!("Destructing binding on {}", addr);
        info!("Signaling cancellation...");
        cancel_token.cancel();
        if let Err(e) = handle.join() {
            error!("Failed to join thread for {}: {:?}", addr, e);
        } else {
            info!("Successfully unbound {}", addr);
        }
    } else {
        warn!("No binding found for {}", addr);
    }
}
