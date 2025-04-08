use clap::{Parser, Subcommand};
use env_logger::Builder;
use log::LevelFilter;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::Command;
use std::sync::{LazyLock, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[allow(clippy::type_complexity)]
static BINDINGS: LazyLock<Mutex<HashMap<String, BindingDetails>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Serialize, Deserialize, Debug)]
enum DaemonCommand {
    Ping,
    Bind {
        addr: String,
        jump_hosts: Vec<String>,
        remote: String,
        sopsfile: String,
        cmd: Option<String>,
    },
    Unbind {
        addr: String,
    },
    List,
    Shutdown,
}

/// Responses from the daemon.
#[derive(Serialize, Deserialize, Debug)]
enum DaemonResponse {
    Success(String),
    List(Vec<BindingDetails>),
    Error(String),
}

/// Details for an active binding.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct BindingDetails {
    addr: String,
    jump_hosts: Vec<String>,
    remote: String,
    timestamp: u64, // seconds since UNIX_EPOCH
}

/// CLI definition.
#[derive(Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Bind a local address to a remote server through SSH jump hosts.
    Bind {
        /// The local address to bind (e.g., "127.0.0.1:8000")
        #[arg(short, long)]
        addr: String,

        /// One or more SSH jump host addresses (e.g., -j "jump1.example.com:22" -j "jump2.example.com:22"). Supply multiple times.
        #[arg(short = 'j', long)]
        jump_host: Vec<String>,

        /// The final remote address (e.g., "remote.example.com:80")
        #[arg(short, long)]
        remote: String,

        /// Path to the SOPS-encrypted YAML file containing SSH credentials.
        #[arg(short, long)]
        sopsfile: String,

        /// Command to run on the remote server in order to start a service (optional).
        #[arg(short, long)]
        cmd: Option<String>,
    },
    /// Unbind the binding for the given address.
    Unbind {
        /// The address to unbind (e.g., "127.0.0.1:8000")
        #[arg(short, long)]
        addr: Option<String>,

        /// Unbind all addresses and shut down the daemon.
        #[arg(long)]
        all: bool,
    },

    /// Gracefully unbind all addresses and shut down the daemon.
    Shutdown,
    /// List all currently open bindings.
    List,
    /// [Internal] Run as daemon.
    #[command(hide = true)]
    Daemon,
}

//
// Cross-platform IPC implementation
//

// Unix implementation using a Unix domain socket.
#[cfg(unix)]
mod ipc {
    use super::*;
    use std::fs;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;

    pub const IPC_PATH: &str = "/tmp/sshbind_daemon.sock";

    pub fn start_server<F>(handler: F) -> std::io::Result<()>
    where
        F: Fn(UnixStream) + Send + 'static + Clone,
    {
        // Remove existing socket if present.
        if Path::new(IPC_PATH).exists() {
            fs::remove_file(IPC_PATH)?;
        }
        let listener = UnixListener::bind(IPC_PATH)?;
        println!("Daemon started (Unix). Listening on {}", IPC_PATH);
        // for stream in listener.incoming() {
        //     if let Ok(stream) = stream {
        //         println!("Client connected.");
        //         let h = handler.clone();
        //         thread::spawn(move || {
        //             println!("spawing.");
        //             h(stream);
        //         });
        //     }
        // }
        listener.incoming().flatten().for_each(|stream| {
            let h = handler.clone();
            thread::spawn(move || {
                h(stream);
            });
        });
        Ok(())
    }

    pub fn connect() -> std::io::Result<UnixStream> {
        UnixStream::connect(IPC_PATH)
    }
}

// Windows implementation using named pipes.
#[cfg(windows)]
mod ipc {
    use super::*;
    use named_pipe::{PipeClient, PipeOptions, PipeServer};
    pub const PIPE_NAME: &str = r"\\.\pipe\sshbind_daemon";

    pub fn start_server<F>(handler: F) -> std::io::Result<()>
    where
        F: Fn(PipeServer) + Send + 'static + Clone,
    {
        println!("Daemon started (Windows). Listening on {}", PIPE_NAME);
        loop {
            // Create a new pipe server instance.
            let server = PipeOptions::new(PIPE_NAME).single()?;
            // Wait for a client to connect.
            server.connect()?;
            let h = handler.clone();
            // Clone the server handle so we can move it into the thread.
            let server_clone = server.try_clone()?;
            thread::spawn(move || {
                h(server_clone);
            });
        }
    }

    pub fn connect() -> std::io::Result<PipeClient> {
        PipeClient::connect(PIPE_NAME)
    }
}

/// The handle_client function accepts any stream that implements Read and Write.
/// It sends an immediate "READY" message (with a null byte delimiter), reads a command
/// (delimited by a null byte), processes it, and sends a JSON response terminated by a null byte.
fn handle_client<T: Read + Write>(mut stream: T) {
    // Send "READY" followed by a zero byte as a delimiter.
    if let Err(e) = stream.write_all(b"READY\0") {
        eprintln!("Error sending READY: {}", e);
        return;
    }
    if let Err(e) = stream.flush() {
        eprintln!("Error flushing READY: {}", e);
        return;
    }

    // Read the incoming command until the zero byte delimiter.
    let cmd_bytes = match read_until_delim(&mut stream, 0) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("Error reading command: {}", e);
            return;
        }
    };

    let cmd_str = match String::from_utf8(cmd_bytes) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Invalid UTF-8 in command: {}", e);
            return;
        }
    };

    let cmd: DaemonCommand = match serde_json::from_str(&cmd_str) {
        Ok(c) => c,
        Err(e) => {
            let error_resp = DaemonResponse::Error(format!("Command parse error: {}", e));
            let resp_json = serde_json::to_string(&error_resp)
                .unwrap_or_else(|_| "{\"error\":\"serialization failed\"}".to_string());
            let _ = stream.write_all(resp_json.as_bytes());
            let _ = stream.write_all(b"\0");
            let _ = stream.flush();
            return;
        }
    };

    let mut time_to_die = false;

    // Process the command.
    let response = match cmd {
        DaemonCommand::Ping => DaemonResponse::Success("PONG".to_string()),
        DaemonCommand::Bind {
            addr,
            jump_hosts,
            remote,
            sopsfile,
            cmd,
        } => {
            // Here you would add the binding details to your daemon's state.
            // For now, we simply return a success message.
            let mut binds = BINDINGS.lock().unwrap();
            binds.insert(
                addr.clone(),
                BindingDetails {
                    addr: addr.clone(),
                    jump_hosts: jump_hosts.clone(),
                    remote: remote.clone(),
                    timestamp: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                },
            );

            sshbind::bind(&addr, jump_hosts, &remote, &sopsfile, cmd);
            DaemonResponse::Success(format!("Bound at {}", addr))
        }
        DaemonCommand::Unbind { addr } => {
            // Remove the binding from your state.
            sshbind::unbind(&addr);
            let mut binds = BINDINGS.lock().unwrap();
            binds.remove(&addr);
            DaemonResponse::Success(format!("Unbound {}", addr))
        }
        DaemonCommand::Shutdown => {
            let mut binds = BINDINGS.lock().unwrap();
            for addr in binds.keys().cloned().collect::<Vec<_>>() {
                sshbind::unbind(&addr);
            }
            binds.clear();
            time_to_die = true;
            DaemonResponse::Success("All bindings cleared. Shutting down.".to_string())
        }
        DaemonCommand::List => {
            // For demonstration, we return an empty list.
            let binds = BINDINGS.lock().unwrap();
            let list: Vec<BindingDetails> = binds.values().cloned().collect();
            DaemonResponse::List(list)
        }
    };

    // Serialize the response to JSON.
    let resp_json = match serde_json::to_string(&response) {
        Ok(json) => json,
        Err(e) => {
            eprintln!("Response serialization error: {}", e);
            return;
        }
    };

    // Write the JSON response followed by a zero byte delimiter.
    if let Err(e) = stream.write_all(resp_json.as_bytes()) {
        eprintln!("Error sending response: {}", e);
        return;
    }
    if let Err(e) = stream.write_all(b"\0") {
        eprintln!("Error sending delimiter: {}", e);
        return;
    }
    if let Err(e) = stream.flush() {
        eprintln!("Error flushing response: {}", e);
    }

    if time_to_die {
        std::process::exit(0);
    }
}

use std::io::{self};

/// Reads from `stream` until the delimiter byte `delim` is encountered.
/// The returned vector does not include the delimiter.
fn read_until_delim<R: Read>(stream: &mut R, delim: u8) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        stream.read_exact(&mut byte)?;
        if byte[0] == delim {
            break;
        }
        buf.push(byte[0]);
    }
    Ok(buf)
}

fn send_command(cmd: DaemonCommand) -> Result<DaemonResponse, String> {
    #[cfg(unix)]
    let mut stream = ipc::connect().map_err(|e| format!("Failed to connect: {}", e))?;
    #[cfg(windows)]
    let mut stream = ipc::connect().map_err(|e| format!("Failed to connect: {}", e))?;

    // Wait for the "READY" ack.
    let ack_buf =
        read_until_delim(&mut stream, 0).map_err(|e| format!("Failed to read ack: {}", e))?;
    let ack = String::from_utf8(ack_buf).map_err(|e| format!("Ack not valid UTF-8: {}", e))?;
    if ack != "READY" {
        return Err(format!("Daemon did not send proper ack: {}", ack));
    }

    // Serialize and send the command, terminated with a zero byte.
    let cmd_json =
        serde_json::to_string(&cmd).map_err(|e| format!("Serialization error: {}", e))?;
    stream
        .write_all(cmd_json.as_bytes())
        .map_err(|e| format!("Failed to send command: {}", e))?;
    stream
        .write_all(&[0])
        .map_err(|e| format!("Failed to send delimiter: {}", e))?;
    stream.flush().map_err(|e| format!("Flush error: {}", e))?;

    // Remove the shutdown call here so we don't signal EOF prematurely.
    // #[cfg(unix)]
    // {
    //     stream.shutdown(Shutdown::Write)
    //         .map_err(|e| format!("Shutdown error: {}", e))?;
    // }

    // Read the response until the zero byte delimiter.
    let resp_buf =
        read_until_delim(&mut stream, 0).map_err(|e| format!("Failed to read response: {}", e))?;
    let resp_str =
        String::from_utf8(resp_buf).map_err(|e| format!("Response not valid UTF-8: {}", e))?;
    serde_json::from_str(&resp_str).map_err(|e| format!("Response parse error: {}", e))
}

// fn spawn_daemon_if_needed() {
//     #[cfg(unix)]
//     let already_running = ipc::connect().is_ok();
//     #[cfg(windows)]
//     let already_running = ipc::connect().is_ok();
//
//     if !already_running {
//         let exe = std::env::current_exe().expect("Failed to get current exe");
//         // Spawn self with the "daemon" subcommand.
//         let _child = Command::new(exe)
//             .arg("daemon")
//             .spawn()
//             .expect("Failed to spawn daemon");
//         // Give the daemon time to start.
//         thread::sleep(Duration::from_millis(500));
//     }
// }

/// Spawns the daemon if not already running.
/// (This simple version tries to connect; if it fails, it spawns self with "daemon".)
fn spawn_daemon_if_needed() {
    // Try to connect and send a Ping command.
    let daemon_running = send_command(DaemonCommand::Ping).is_ok();

    if !daemon_running {
        let exe = std::env::current_exe().expect("Failed to get current exe");
        // Spawn self with the "daemon" subcommand.
        #[allow(clippy::zombie_processes)]
        let _child = Command::new(exe)
            .arg("daemon")
            .spawn()
            .expect("Failed to spawn daemon");
        // Wait until the daemon is ready.
        thread::sleep(Duration::from_millis(500));
    }
}

//
// Main entry point
//
fn main() {
    Builder::new()
        .filter(None, LevelFilter::Info) // Adjust log level as needed
        .format(|buf, record| writeln!(buf, "[{}] - {}", record.level(), record.args()))
        .init();
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Daemon) => {
            // Run in daemon mode.
            #[cfg(unix)]
            {
                let handler = |stream: std::os::unix::net::UnixStream| {
                    handle_client(stream);
                };
                ipc::start_server(handler).unwrap();
            }
            #[cfg(windows)]
            {
                let handler = |server: named_pipe::PipeServer| {
                    handle_client(server);
                };
                ipc::start_server(handler).unwrap();
            }
        }
        Some(Commands::Bind {
            addr,
            jump_host,
            remote,
            sopsfile,
            cmd,
        }) => {
            spawn_daemon_if_needed();
            let command = DaemonCommand::Bind {
                addr,
                jump_hosts: jump_host,
                remote,
                sopsfile,
                cmd,
            };
            println!("Sending");
            match send_command(command) {
                Ok(DaemonResponse::Success(msg)) => println!("{}", msg),
                Ok(resp) => println!("Unexpected response: {:?}", resp),
                Err(e) => eprintln!("{}", e),
            }
            println!("Sent");
        }
        Some(Commands::Unbind { addr, all }) => {
            spawn_daemon_if_needed();
            let cmd = if all {
                DaemonCommand::Shutdown
            } else if let Some(a) = addr {
                DaemonCommand::Unbind { addr: a }
            } else {
                eprintln!("Error: either --addr must be specified or use --all");
                return;
            };

            match send_command(cmd) {
                Ok(DaemonResponse::Success(msg)) => println!("{}", msg),
                Ok(resp) => println!("Unexpected response: {:?}", resp),
                Err(e) => eprintln!("{}", e),
            }
        }
        Some(Commands::Shutdown) => {
            spawn_daemon_if_needed();
            match send_command(DaemonCommand::Shutdown) {
                Ok(DaemonResponse::Success(msg)) => println!("{}", msg),
                Ok(resp) => println!("Unexpected response: {:?}", resp),
                Err(e) => eprintln!("{}", e),
            }
        }
        Some(Commands::List) => {
            spawn_daemon_if_needed();
            let cmd = DaemonCommand::List;
            match send_command(cmd) {
                Ok(DaemonResponse::List(list)) => {
                    if list.is_empty() {
                        println!("No active bindings.");
                    } else {
                        println!(
                            "{:<20} {:<30} {:<10} Jump Hosts",
                            "Address", "Remote", "Time"
                        );
                        for b in list {
                            println!(
                                "{:<20} {:<30} {:<10} {:?}",
                                b.addr, b.remote, b.timestamp, b.jump_hosts
                            );
                        }
                    }
                }
                Ok(resp) => println!("Unexpected response: {:?}", resp),
                Err(e) => eprintln!("{}", e),
            }
        }
        None => {
            println!("No command provided. Use --help for usage.");
        }
    }
}
