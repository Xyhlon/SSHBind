use std::collections::HashMap;
use std::convert::TryFrom;
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::thread;
use log::{error, info, warn};
use serde_yml;

mod async_ssh;
mod auth;
mod executor;
mod select;
mod server;
mod types;
mod utils;

pub use types::*;
use executor::Runtime;
use server::run_server;
use utils::decrypt_sops_file;











#[allow(clippy::needless_doctest_main)]
/// Binds a local address to a server that forwards incoming TCP connections through a chain
/// of SSH jump hosts to a specified remote address. The server runs in a separate thread.
///
/// # Arguments
///
/// * `addr` - The local address to bind the server to (e.g., "127.0.0.1:8000").
/// * `jump_hosts` - A vector of SSH jump host addresses (e.g., vec!["jump1.example.com:22", "jump2.example.com:22"]).
/// * `remote_addr` - Optional final remote address to forward connections to (e.g., Some("remote.example.com:80".to_string())).
/// * `sopsfile` - The path to a SOPS-encrypted YAML file containing SSH credentials.
/// * `cmd` - Optional command to execute on the remote host before connecting (e.g., Some("docker run -p 3000:3000 app".to_string())).
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
///     let remote_addr = Some("remote.example.com:80".to_string());
///     let sopsfile = "/path/to/creds.sops.yaml";
///     let cmd = None; // No command to execute, just forward
///
///     // Start the server in a separate thread
///     let server_thread = thread::spawn(move || {
///         bind(addr, jump_hosts, remote_addr, sopsfile, cmd);
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
    let mut binds = match BINDINGS.lock() {
        Ok(binds) => binds,
        Err(e) => {
            error!("Failed to acquire lock for bindings: {}", e);
            return;
        }
    };

    if !std::path::Path::new(sopsfile).exists() {
        error!("SOPS file not found: {sopsfile}");
        return;
    }

    // Try to read the file first to check if it's encrypted
    let file_content = match std::fs::read_to_string(sopsfile) {
        Ok(content) => content,
        Err(err) => {
            error!("Failed to read file {}: {}", sopsfile, err);
            return;
        }
    };
    
    // Check if the file appears to be SOPS encrypted (contains "sops" key)
    let decrypted_content = if file_content.contains("\"sops\"") || file_content.contains("sops:") {
        match decrypt_sops_file(sopsfile) {
            Ok(content) => content,
            Err(err) => {
                error!("SOPS decryption failed: {}", err);
                return;
            }
        }
    } else {
        // File is not encrypted, use as-is
        info!("Using unencrypted credentials file");
        file_content
    };

    let jump_hosts: Vec<HostPort> = match jump_hosts
        .iter()
        .map(|host| HostPort::try_from(host.as_str()))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(hosts) => hosts,
        Err(e) => {
            error!("Failed to parse jump hosts: {}", e);
            return;
        }
    };

    let remote_addr = match remote_addr
        .map(|addr| HostPort::try_from(addr.as_str()))
        .transpose()
    {
        Ok(addr) => addr,
        Err(e) => {
            error!("Failed to parse remote address: {}", e);
            return;
        }
    };

    let creds: YamlCreds = match serde_yml::from_str(&decrypted_content) {
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
        let bind_addr_owned = bind_addr.clone();
        let cmd_owned = cmd.clone();
        match Runtime::new() {
            Ok(rt) => {
                match rt.block_on(async move {
                    run_server(
                        &bind_addr_owned,
                        jump_hosts,
                        remote_addr,
                        cmd_owned.as_deref(),
                        creds,
                        token_clone,
                        pair_clone,
                    ).await
                }) {
                    Ok(_) => {},
                    Err(e) => error!("Server error: {e}"),
                }
            }
            Err(e) => {
                error!("Failed to create custom runtime: {e}");
                return;
            }
        }
    });

    binds.insert(addr.to_string(), (cancel_token, handle));

    // Wait for the thread to start up, bind and listen.
    let pair_clone = Arc::clone(&pair);
    let (lock, cvar) = &*pair_clone;
    if let Ok(guard) = lock.lock() {
        if let Err(e) = cvar.wait_while(guard, |pending| *pending) {
            error!("Error waiting for server startup: {e}");
        }
    } else {
        error!("Failed to acquire lock for server startup notification");
    };
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
    match BINDINGS.lock() {
        Ok(mut binds) => {
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
        Err(e) => {
            error!("Failed to acquire lock for bindings: {}", e);
        }
    }
}