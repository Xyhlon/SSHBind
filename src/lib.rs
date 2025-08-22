use serde::{Deserialize, Serialize};
use std::error::Error;
use std::net::ToSocketAddrs;
use std::process::Command;
use std::thread;
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

use async_ssh2_lite::{AsyncSession, SessionConfiguration, TokioTcpStream};
use libreauth::oath::TOTPBuilder;
use std::collections::{BTreeMap, HashMap};
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};

use log::{error, info, warn};
use std::sync::{Arc, Condvar, LazyLock, Mutex};

#[cfg(test)]
mod tests {

    /// **Given**: A bind call with debug flag set to Some(true)
    /// **When**: The bind function is called
    /// **Then**: Debug simulation should be triggered before normal server operation
    #[test]
    fn test_debug_flag_enabled_triggers_simulation() {
        // This test will fail initially - we need to modify run_server to be testable
        // Currently run_server is not easily testable due to blocking operations
        assert!(false, "Debug simulation behavior needs verification");
    }

    /// **Given**: A bind call with debug flag set to Some(false)  
    /// **When**: The bind function is called
    /// **Then**: No debug simulation should occur, only normal server behavior
    #[test]
    fn test_debug_flag_disabled_no_simulation() {
        // This test will fail initially - we need to extract debug logic for testing
        assert!(false, "Non-debug behavior needs verification");
    }

    /// **Given**: A bind call with debug flag set to None
    /// **When**: The bind function is called  
    /// **Then**: No debug simulation should occur, only normal server behavior
    #[test]
    fn test_debug_flag_none_no_simulation() {
        // This test will fail initially - we need to extract debug logic for testing
        assert!(false, "None debug behavior needs verification");
    }

    /// **Given**: Debug credentials and jump hosts configuration
    /// **When**: Debug simulation runs with the SSH connection chain
    /// **Then**: The connect_chain function should be called and complete successfully
    #[test]
    fn test_debug_simulation_calls_connect_chain() {
        // This test will fail initially - we need to make connect_chain testable
        assert!(false, "Debug simulation connect_chain call needs verification");
    }

    /// **Given**: A debug-enabled server configuration
    /// **When**: Debug simulation is triggered
    /// **Then**: Debug output messages should be printed to stdout
    #[test] 
    fn test_debug_simulation_outputs_debug_messages() {
        // This test will fail initially - we need to capture stdout in tests
        assert!(false, "Debug message output needs verification");
    }

    /// **Given**: Debug simulation with invalid jump host configuration
    /// **When**: Debug simulation attempts to connect
    /// **Then**: The simulation should handle errors gracefully
    #[test]
    fn test_debug_simulation_error_handling() {
        // This test will fail initially - need to test error scenarios
        assert!(false, "Debug simulation error handling needs verification");
    }
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
    host: &str,
) -> Result<(), Box<dyn Error>> {
    let creds = creds_map
        .get(host)
        .ok_or_else(|| format!("Couldn't find credentials for {}", host))?;
    let username = creds.username.clone();
    let password = creds.password.clone();
    let totp_key = creds.totp_key.clone();

    if let Some(key) = totp_key {
        let _code = TOTPBuilder::new().base32_key(&key).finalize()?.generate();
        // Use the generated TOTP code as needed
    }

    info!("Authenticating with: {}", username);
    session.userauth_password(&username, &password).await?;

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

async fn process_connection(
    mut inbound: TcpStream,
    jump_hosts: &[String],
    remote_addr: &str,
    creds: &YamlCreds,
) -> Result<(), Box<dyn Error>> {
    let session = connect_chain(jump_hosts, creds).await?;
    let remote_socket_addr = remote_addr
        .to_socket_addrs()?
        .next()
        .ok_or("Failed to resolve remote address")?;
    let mut channel = session
        .channel_direct_tcpip(
            &remote_socket_addr.ip().to_string(),
            remote_socket_addr.port(),
            None,
        )
        .await?;
    copy_bidirectional(&mut inbound, &mut channel).await?;
    Ok(())
}

async fn run_server(
    addr: &str,
    jump_hosts: Vec<String>,
    remote_addr: &str,
    creds: YamlCreds,
    cancel_token: CancellationToken,
    pair: Arc<(Mutex<bool>, Condvar)>,
    debug: Option<bool>,
) -> Result<(), Box<dyn Error>> {
    // Debug branch: simulate a connect by establishing an outbound connection and processing it
    if let Some(true) = debug {
        let listener = TcpListener::bind(addr).await?;
        info!("Listening on {addr}");
        println!("Debugging enabled");
        std::thread::sleep(std::time::Duration::from_secs(2));
        // Simulate an inbound connection by connecting to the first jump host.
        let accept_task =
            tokio::spawn(async move { listener.accept().await.map(|(local_conn, _)| local_conn) });

        let _client_conn = TcpStream::connect(addr).await?;
        let server_conn = accept_task.await?.map_err(|e| e.to_string())?;
        process_connection(server_conn, &jump_hosts, remote_addr, &creds).await?;
        println!("Simulated connection complete");
    }
    let listener = TcpListener::bind(addr).await?;
    info!("Listening on {addr}");

    // Notify that initialization is complete.
    {
        let (lock, cvar) = &*pair;
        let mut pending = lock.lock().unwrap();
        *pending = false;
        println!("Done");
        cvar.notify_one();
    }

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                warn!("Shutdown signal received. Stopping server.");
                break;
            }
            result = listener.accept() => {
                match result {
                    Ok((inbound, _)) => {
                        // Clone data as needed for the spawned task.
                        let jump_hosts_clone = jump_hosts.clone();
                        let remote_addr = remote_addr.to_string();
                        let creds_clone = creds.clone(); // Ensure YamlCreds implements Clone
                        tokio::spawn(async move {
                            if let Err(e) = process_connection(inbound, &jump_hosts_clone, &remote_addr, &creds_clone).await {
                                error!("Connection error: {e}");
                            }
                        });
                    }
                    Err(e) => error!("Failed to accept connection: {e}")
                }
            }
        }
    }
    Ok(())
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
    jump_hosts: &[String],
    creds_map: &YamlCreds,
) -> Result<AsyncSession<TokioTcpStream>, Box<dyn Error>> {
    assert!(!jump_hosts.is_empty(), "No jump hosts provided");
    info!("Starting SSH chain connection through {:?}", jump_hosts);

    let jump_socket_addr = jump_hosts[0]
        .to_socket_addrs()?
        .next()
        .ok_or("Failed to resolve address")?;

    let mut session = AsyncSession::<TokioTcpStream>::connect(jump_socket_addr, None).await?;
    session.handshake().await?;
    userauth(&session, creds_map, &jump_hosts[0]).await?;

    for (i, jump) in jump_hosts.iter().enumerate().skip(1) {
        info!("Connecting through jump {}: {}", i, jump);
        let jump_socket_addr = jump
            .to_socket_addrs()?
            .next()
            .ok_or("Failed to resolve address")?;
        let mut channel = session
            .channel_direct_tcpip(
                &jump_socket_addr.ip().to_string(),
                jump_socket_addr.port(),
                None,
            )
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
    remote_addr: &str,
    sopsfile: &str,
    debug: Option<bool>,
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
    let remote_addr = remote_addr.to_string();

    let pair = Arc::new((Mutex::new(true), Condvar::new()));
    let pair_clone = Arc::clone(&pair);

    let handle = thread::spawn(move || {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            if let Err(e) = run_server(
                &bind_addr,
                jump_hosts,
                &remote_addr,
                creds,
                token_clone,
                pair_clone,
                debug,
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
