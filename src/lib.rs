use serde::{Deserialize, Serialize};
use serde_yml;
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

use std::sync::{LazyLock, Mutex};

async fn userauth(
    session: &AsyncSession<TokioTcpStream>,
    creds_map: &YamlCreds,
    host: &str,
) -> Result<(), Box<dyn Error>> {
    let creds = creds_map
        .get(host)
        .expect(&format!("Couldn't find credentials for {}", host));
    let username = creds.username.clone();
    let password = creds.password.clone();
    let totp_key = creds.totp_key.clone();
    if totp_key.is_some() {
        #[allow(unused_variables)]
        let code = TOTPBuilder::new()
            .base32_key(&totp_key.unwrap())
            .finalize()
            .unwrap()
            .generate();
    }

    println!("Authenticating with: {}", username);
    session.userauth_password(&username, &password).await?;

    // Verify that authentication succeeded.
    if session.authenticated() {
        println!("SSH session established and authenticated!");
    } else {
        eprintln!(
            "Authentication failed. With user: {} into host: {}",
            username, host,
        );
        return Err(format!(
            "Authentication failed. With user: {} into host: {}",
            username, host,
        )
        .into());
    }
    Ok(())
}

/// Establishes an SSH session chain through the given jump hosts.
/// Returns the final AsyncSession connected to the last host in the chain.
async fn connect_chain(
    jump_hosts: &[String],
    creds_map: &YamlCreds,
) -> Result<AsyncSession<TokioTcpStream>, Box<dyn Error>> {
    assert!(!jump_hosts.is_empty(), "No jump hosts provided");

    // Connect to the first jump host over TCP
    let jump_socket_addr = jump_hosts[0]
        .to_socket_addrs()?
        .next()
        .expect("Failed to resolve address");
    let mut session = AsyncSession::<TokioTcpStream>::connect(jump_socket_addr, None).await?;
    session.handshake().await?;
    let host = &jump_socket_addr.ip().to_string();
    let port = jump_socket_addr.port();
    userauth(&session, creds_map, &format!("{host}:{port}")).await?;
    println!("Jump {} connected: {}", 0, jump_hosts[0]);

    // Chain through subsequent jump hosts (if any)
    let mut current_session = session;
    for (i, jump) in jump_hosts.iter().enumerate().skip(1) {
        let jump_socket_addr = jump
            .to_socket_addrs()?
            .next()
            .expect("Failed to resolve address");
        let host = &jump_socket_addr.ip().to_string();
        let port = jump_socket_addr.port();
        // Open a channel from the current session to the next host's SSH port
        let mut channel = current_session
            .channel_direct_tcpip(&host, port, None)
            .await?;

        // Create a loopback TCP socket pair (listener + client socket)
        let listener = TcpListener::bind("127.0.0.1:0").await?; // Bind to any available port
        let local_addr = listener.local_addr()?; // Get the assigned port

        // task for accepting the forwading connnection we establish below
        let accept_task = tokio::spawn(async move {
            match listener.accept().await {
                Ok((local_conn, _)) => Ok(local_conn),
                Err(e) => Err(e),
            }
        });

        // Connect to the listener (this forms the TCP pair)
        let client_conn = TcpStream::connect(local_addr).await?;
        let mut server_conn = match accept_task.await {
            Ok(Ok(conn)) => conn, // Successfully received the accepted connection
            Ok(Err(e)) => return Err(Box::new(e)), // The accept task failed with an IO error
            Err(_) => {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Accept task failed",
                )))
            } // Task panicked or was dropped
        };

        // Relay data between SSH channel and the loopback TCP socket
        tokio::spawn(async move {
            let _ = copy_bidirectional(&mut channel, &mut server_conn).await;
        });
        // Use the other side of the duplex as the transport for the next SSH session
        // (Implement AsyncSessionStream for DuplexStream so that AsyncSession can use it)
        // Create a new session with the duplex as underlying stream
        let mut next_session = AsyncSession::new(client_conn, SessionConfiguration::default())?;
        next_session.handshake().await?;

        userauth(&next_session, creds_map, &format!("{host}:{port}")).await?;
        println!("Jump {} connected: {}", i, jump);
        current_session = next_session;
    }
    Ok(current_session)
}

async fn run_server(
    addr: &str,
    jump_hosts: Vec<String>,
    remote_addr: &str,
    creds: YamlCreds,
    cancel_token: CancellationToken,
) -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind(addr).await?;
    println!("Listening on {}", addr);

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                println!("Cancellation signal received. Shutting down server.");
                break;
            }
            result = listener.accept() => {
                let (mut inbound, _) = result?;
                let session = connect_chain(&jump_hosts,  &creds).await?;
                let remote_socket_addr = remote_addr
                        .to_socket_addrs()?
                        .next()
                        .expect("Failed to resolve address");

                let mut channel  = session.channel_direct_tcpip(&remote_socket_addr.ip().to_string(), remote_socket_addr.port(), None).await?;

                // TODO: make it work if the jump list is empty
                // Spawn a task to handle the bidirectional copy.
                // for no jumps
                // let mut chan = TcpStream::connect(remote_addr).await?;
                tokio::spawn(async move {
                    match copy_bidirectional(&mut inbound, &mut channel).await {
                        Ok((bytes_in, bytes_out)) => {
                            println!("Forwarded {} bytes in, {} bytes out", bytes_in, bytes_out);
                        }
                        Err(e) => eprintln!("Connection error: {}", e),
                    }
                });
            }
        }
    }
    Ok(())
}

pub fn bind(addr: &str, jump_hosts: Vec<String>, remote_addr: &str, sopsfile: &str) {
    let mut binds = BINDINGS.lock().unwrap();

    let output = Command::new("which")
        .arg("sops")
        .output()
        .expect("failed to execute process");
    println!("status: {}", output.status);
    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stdout: {}", String::from_utf8_lossy(&output.stderr));

    let output = Command::new("sops")
        .arg("decrypt")
        .arg(sopsfile) // user input as a separate argument
        .output()
        .expect("failed to execute process");
    // TODO: file not found error
    // TODO: not allowed / permissions error
    // TODO: sops not installed error
    // TODO: parse path is valid and throw error if not correct
    // TODO: symlinking to other not allowed sopsfile is a sops security issue?
    println!("status: {}", output.status);
    // println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stdout: {}", String::from_utf8_lossy(&output.stderr));
    let creds: YamlCreds = serde_yml::from_str(&String::from_utf8_lossy(&output.stdout))
        .expect(&format!("Failed to deserialize {}", sopsfile)); // throw error that yaml has incorrect format

    // Create a cancellation token.
    let cancel_token = CancellationToken::new();
    let token_clone = cancel_token.clone();
    let bind_addr = addr.to_string();
    let remote_addr = remote_addr.to_string();

    // Spawn a thread that runs our async server.
    let handle = thread::spawn(move || {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            if let Err(e) =
                run_server(&bind_addr, jump_hosts, &remote_addr, creds, token_clone).await
            {
                eprintln!("Server encountered an error: {}", e);
            }
        });
    });
    binds.insert(addr.to_string(), (cancel_token.clone(), handle));
}

#[derive(Debug, PartialEq, Serialize, Deserialize, Clone)]
pub struct Creds {
    pub username: String,
    pub password: String,
    pub totp_key: Option<String>,
}

pub type YamlCreds = BTreeMap<String, Creds>;

static BINDINGS: LazyLock<
    Mutex<HashMap<String, (CancellationToken, std::thread::JoinHandle<()>)>>,
> = LazyLock::new(|| {
    let map = HashMap::new();
    Mutex::new(map)
});

pub fn unbind(addr: &str) {
    let mut binds = BINDINGS.lock().unwrap();
    if let Some((addr, (cancel_token, handle))) = binds.remove_entry(addr) {
        println!("Destructing binding on {}", addr);
        println!("Signaling cancellation...");
        cancel_token.cancel();
        handle.join().unwrap();
    }
}
