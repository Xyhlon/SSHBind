use clap::{Parser, Subcommand};
use env_logger::{Builder, Target};
use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions, Name, Stream, ToFsName, ToNsName,
};
use log::{error, info, LevelFilter};
use named_sem::{Error as SemError, NamedSemaphore};
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::HashMap;
use std::error::Error;
use std::fs::OpenOptions;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::result::Result;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

static LOG_PATH: LazyLock<PathBuf> = LazyLock::new(|| {
    dirs::data_local_dir()
        .expect("Failed to get local data dir")
        .join("sshbind/sshbind.log")
});

static BINDINGS: LazyLock<Mutex<HashMap<String, BindingDetails>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static TIME_TO_DIE: LazyLock<Mutex<bool>> = LazyLock::new(|| Mutex::new(false));

#[derive(Serialize, Deserialize, Debug)]
enum DaemonCommand {
    Ping,
    Bind {
        addr: String,
        jump_hosts: Vec<String>,
        remote: Option<String>,
        sopsfile: String,
        cmd: Option<String>,
    },
    Unbind {
        addr: String,
    },
    List,
    Shutdown,
}

#[derive(Serialize, Deserialize, Debug)]
enum DaemonResponse {
    Success(String),
    List(Vec<BindingDetails>),
    Error(String),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct BindingDetails {
    addr: String,
    jump_hosts: Vec<String>,
    cmd: Option<String>,
    remote: Option<String>,
    timestamp: u64,
}

#[derive(Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    #[arg(short = 'v', long)]
    verbose: bool,
    #[arg(short = 'l', long)]
    log_level: Option<u64>,
}

#[derive(Subcommand)]
enum Commands {
    Bind {
        #[arg(short, long)]
        addr: String,
        #[arg(short = 'j', long)]
        jump_host: Vec<String>,
        #[arg(short, long)]
        remote: Option<String>,
        #[arg(short, long)]
        sopsfile: String,
        #[arg(short, long)]
        cmd: Option<String>,
    },
    Unbind {
        #[arg(short, long)]
        addr: Option<String>,
        #[arg(long)]
        all: bool,
    },
    Shutdown,
    List,
    #[command(hide = true)]
    Daemon,
}

// Path where we will both listen and also advertise our socket name to clients.
const SERVER_NAME_PATH: &str = "/tmp/sshbind_daemon.ipc";
const SEM_SERVER_READY: &str = "/sshbind_server_ready";
// Serializes client creation and server readiness checking
const SEM_SENDING_STICK: &str = "/sshbind_sending_stick";
// I believe we need this because forking isn't allowed in rust
// Thus multiple daemon instances could be started at the same
// time by invoking the daemon command
const SEM_CHECK_LOCK: &str = "/sshbind_check_lock";
const SEM_FLOOD_GATE: &str = "/sshbind_flood_gate";

fn server_name() -> Result<Name<'static>, Box<dyn Error>> {
    // Prefer non-persistent, namespaced name
    if let Ok(n) = "sshbind.daemon".to_ns_name::<GenericNamespaced>() {
        Ok(n.into_owned())
    } else {
        // Fallback for macOS/BSD to a filesystem path
        match SERVER_NAME_PATH.to_fs_name::<GenericFilePath>() {
            Ok(n) => return Ok(n.into_owned()),
            Err(e) => {
                return Err(e.into());
            }
        }
    }
}

fn cleanup_server_resources() {
    let mut sending_stick = NamedSemaphore::create(SEM_SENDING_STICK, 1)
        .expect("Failed to create sending_stick semaphore");
    let mut server_ready = NamedSemaphore::create(SEM_SERVER_READY, 0)
        .expect("Failed to create server_ready semaphore");
    let mut check_lock =
        NamedSemaphore::create(SEM_CHECK_LOCK, 1).expect("Failed to create check_lock semaphore");
    let mut flood_gate =
        NamedSemaphore::create(SEM_FLOOD_GATE, 0).expect("Failed to create flood_gate semaphore");
    sending_stick
        .wait()
        .expect("Failed to wait for sending_stick");
    check_lock.wait().expect("Failed to wait for check_lock");
    server_ready
        .wait()
        .expect("Failed to wait for server_ready");

    check_lock.post().expect("Failed to post check_lock");
    flood_gate.wait().expect("Failed to wait for flood_gate");
    sending_stick.post().expect("Failed to post sending_stick");
}

fn start_ipc_daemon() -> Result<(), Box<dyn Error>> {
    // Bootstrap first server
    let mut server_ready = NamedSemaphore::create(SEM_SERVER_READY, 0)
        .expect("Failed to create server_ready semaphore");
    let mut check_lock =
        NamedSemaphore::create(SEM_CHECK_LOCK, 1).expect("Failed to create check_lock semaphore");
    let mut flood_gate =
        NamedSemaphore::create(SEM_FLOOD_GATE, 0).expect("Failed to create server_ready semaphore");

    // Serialize client creation and server readiness checking
    check_lock
        .wait()
        .expect("Failed to wait for startup semaphore");

    // Catch dispatch race condition. If the semaphore was already posted, another
    // daemon instance has already bound the socket and is running, so we exit.
    if let Ok(_) = server_ready.try_wait() {
        check_lock.post().expect("Failed to post check_lock");
        return Ok(());
    }

    // We are the first server. Bind a local socket on the predetermined path.
    let socket_name = match server_name() {
        Ok(name) => name,
        Err(e) => {
            // Failed to convert the path into a Name, post the lock and propagate error.
            check_lock.post().expect("Failed to post check_lock");
            return Err(e.into());
        }
    };
    let listener = ListenerOptions::new()
        .name(socket_name)
        .create_sync()
        .map_err(|e| {
            // Failed to bind the socket; post the lock and propagate error.
            check_lock.post().expect("Failed to post check_lock");
            Box::<dyn Error>::from(e)
        })?;

    // Signal readiness so clients can continue.
    server_ready.post().expect("Failed to post server_ready");
    check_lock.post().expect("Failed to post check_lock");
    flood_gate.post().expect("Failed to post flood_gate");

    // Initialise logging to our log file
    std::fs::create_dir_all(LOG_PATH.parent().expect("Parent must exist"))
        .expect("Failed to create log directory");
    let log_file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(false)
        .open(LOG_PATH.as_path())
        .expect("Cannot open log file");
    Builder::new()
        .filter(None, LevelFilter::Debug)
        .target(Target::Pipe(Box::new(log_file)))
        .init();

    // Accept incoming connections. Each stream is handled in its own thread.
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                thread::spawn(move || {
                    // Read a single command from the stream, handle it, and write
                    // back the response. Newlines delimit messages.
                    if let Err(e) = handle_client_stream(&mut stream) {
                        // Log or ignore errors on individual client connections.
                        error!("Error handling client: {}", e);
                    }
                    if TIME_TO_DIE.lock().unwrap().clone() {
                        cleanup_server_resources();
                        std::process::exit(0);
                    }
                });
            }
            Err(e) => {
                // If accepting a connection fails, log and continue listening.
                error!("Failed to accept client: {}", e);
            }
        }
    }
    Ok(())
}

/// Handle a single client connection on the server side.
///
/// The protocol is newline-delimited JSON.  We read bytes until a newline
/// character, deserialize the command, invoke the handler, then write the
/// serialized response followed by a newline.  Using JSON via serde allows
/// forward compatibility and human readability.
fn handle_client_stream(stream: &mut Stream) -> Result<(), Box<dyn Error>> {
    // Read the incoming command up to the newline delimiter
    let mut buf = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        let n = stream.read(&mut byte)?;
        if n == 0 {
            // Connection closed prematurely
            return Err(Box::from("client closed connection"));
        }
        if byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0]);
    }

    // Deserialize command using serde_json
    let cmd: DaemonCommand = serde_json::from_slice(&buf)?;
    let response = handle_command(cmd);
    // Serialize the response and write it back with a newline terminator
    let resp_bytes = serde_json::to_vec(&response)?;
    stream.write_all(&resp_bytes)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

/// Client-side: send a command and wait for a response.
fn send_command(cmd: DaemonCommand) -> Result<DaemonResponse, String> {
    let socket_name = match server_name() {
        Ok(name) => name,
        Err(e) => {
            return Err(format!("Failed to get server name: {}", e));
        }
    };
    let mut stream =
        Stream::connect(socket_name).map_err(|e| format!("ipc connect failed: {}", e))?;
    // Serialize and send the command with a newline terminator
    let cmd_bytes = serde_json::to_vec(&cmd).map_err(|e| format!("serialize error: {}", e))?;
    stream
        .write_all(&cmd_bytes)
        .map_err(|e| format!("IPC send error: {}", e))?;
    stream
        .write_all(b"\n")
        .map_err(|e| format!("IPC send error: {}", e))?;
    stream
        .flush()
        .map_err(|e| format!("IPC flush error: {}", e))?;
    // Read the response up to the newline delimiter
    let mut buf = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        let n = stream
            .read(&mut byte)
            .map_err(|e| format!("IPC recv error: {}", e))?;
        if n == 0 {
            return Err("IPC recv error: server closed connection".into());
        }
        if byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0]);
    }
    let resp: DaemonResponse =
        serde_json::from_slice(&buf).map_err(|e| format!("deserialize error: {}", e))?;
    Ok(resp)
}

/// Process a DaemonCommand and mutate state, returning a DaemonResponse.
fn handle_command(cmd: DaemonCommand) -> DaemonResponse {
    let response = match cmd {
        DaemonCommand::Ping => DaemonResponse::Success("PONG".into()),
        DaemonCommand::Bind {
            addr,
            jump_hosts,
            remote,
            sopsfile,
            cmd,
        } => {
            let mut binds = BINDINGS.lock().unwrap();
            if binds.contains_key(&addr) {
                DaemonResponse::Error(format!("Address {} is already bound.", addr))
            } else {
                binds.insert(
                    addr.clone(),
                    BindingDetails {
                        addr: addr.clone(),
                        jump_hosts: jump_hosts.clone(),
                        cmd: cmd.clone(),
                        remote: remote.clone(),
                        timestamp: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs(),
                    },
                );
                sshbind::bind(&addr, jump_hosts, remote, &sopsfile, cmd);
                DaemonResponse::Success(format!("Bound at {}", addr))
            }
        }
        DaemonCommand::Unbind { addr } => {
            sshbind::unbind(&addr);
            BINDINGS.lock().unwrap().remove(&addr);
            DaemonResponse::Success(format!("Unbound {}", addr))
        }
        DaemonCommand::Shutdown => {
            let mut binds = BINDINGS.lock().unwrap();
            for a in binds.keys().cloned().collect::<Vec<_>>() {
                sshbind::unbind(&a);
            }
            binds.clear();
            let mut ttd = TIME_TO_DIE.lock().unwrap();
            *ttd = true;
            DaemonResponse::Success("All bindings cleared. Shutting down.".into())
        }
        DaemonCommand::List => {
            let list = BINDINGS.lock().unwrap().values().cloned().collect();
            DaemonResponse::List(list)
        }
    };
    response
}

fn spawn_daemon_if_needed() {
    let mut server_ready = NamedSemaphore::create(SEM_SERVER_READY, 0)
        .expect("Failed to create server_ready semaphore");
    let mut check_lock =
        NamedSemaphore::create(SEM_CHECK_LOCK, 1).expect("Failed to create server_ready semaphore");
    let mut flood_gate =
        NamedSemaphore::create(SEM_FLOOD_GATE, 0).expect("Failed to create flood_gate semaphore");
    let mut sending_stick = NamedSemaphore::create(SEM_SENDING_STICK, 1)
        .expect("Failed to create sending_stick semaphore");
    sending_stick
        .wait()
        .expect("Failed to wait for sending_stick");
    check_lock
        .wait()
        .expect("Failed to wait for startup semaphore");
    match server_ready.try_wait() {
        Ok(_) => {
            server_ready.post().expect("Failed to post server_ready");
            check_lock.post().expect("Failed to post check_lock");
            sending_stick.post().expect("Failed to post sending_stick");
            return;
        }
        Err(SemError::WouldBlock) => {
            std::fs::create_dir_all(LOG_PATH.parent().expect("Parent must exist"))
                .expect("Failed to create log directory");

            let stderr = OpenOptions::new()
                .create(true)
                .write(true)
                .open(
                    LOG_PATH
                        .parent()
                        .expect("Parent must exist")
                        .join("sshbind_stderr.log"),
                )
                .expect("Cannot open stderr log file");
            let stdout = OpenOptions::new()
                .create(true)
                .write(true)
                .open(
                    LOG_PATH
                        .parent()
                        .expect("Parent must exist")
                        .join("sshbind_stdout.log"),
                )
                .expect("Cannot open stdout log file");
            let exe = std::env::current_exe().expect("Failed to get current exe");

            let _ = Command::new(exe)
                .arg("daemon")
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn();
        }
        Err(e) => (),
    }
    check_lock.post().expect("Failed to post check_lock");
    flood_gate.wait().expect("Failed to wait for flood_gate");
    flood_gate.post().expect("Failed to post flood_gate");
    sending_stick.post().expect("Failed to post sending_stick");
}

fn main() {
    let cli = Cli::parse();
    let start_offset = if cli.verbose {
        let start = match cli.log_level {
            Some(0) => SeekFrom::End(0),
            Some(1) => SeekFrom::Start(0),
            None => SeekFrom::End(0),
            Some(_) => {
                println!("Unsupported Log-Level");
                return;
            }
        };
        std::fs::create_dir_all(LOG_PATH.parent().expect("Parent must exist"))
            .expect("Failed to create log directory");

        let mut log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(LOG_PATH.as_path())
            .expect("Cannot open log file");
        let off = log_file.seek(start).expect("Cannot seek in log file");
        off
    } else {
        0
    };

    match cli.command {
        Some(Commands::Daemon) => {
            start_ipc_daemon().unwrap();
        }
        Some(Commands::Bind {
            addr,
            jump_host,
            remote,
            sopsfile,
            cmd,
        }) => {
            spawn_daemon_if_needed();
            let cmd = DaemonCommand::Bind {
                addr,
                jump_hosts: jump_host,
                remote,
                sopsfile,
                cmd,
            };
            match send_command(cmd) {
                Ok(DaemonResponse::Success(msg)) => println!("{}", msg),
                Ok(resp) => println!("Unexpected: {:?}", resp),
                Err(e) => eprintln!("{}", e),
            }
        }
        Some(Commands::Unbind { addr, all }) => {
            spawn_daemon_if_needed();
            let cmd = if all {
                DaemonCommand::Shutdown
            } else if let Some(a) = addr {
                DaemonCommand::Unbind { addr: a }
            } else {
                eprintln!("--addr or --all required");
                return;
            };
            match send_command(cmd) {
                Ok(DaemonResponse::Success(msg)) => println!("{}", msg),
                Ok(resp) => println!("Unexpected: {:?}", resp),
                Err(e) => eprintln!("{}", e),
            }
        }
        Some(Commands::Shutdown) => {
            spawn_daemon_if_needed();
            if let Ok(DaemonResponse::Success(msg)) = send_command(DaemonCommand::Shutdown) {
                println!("{}", msg);
            }
        }
        Some(Commands::List) => {
            spawn_daemon_if_needed();
            match send_command(DaemonCommand::List) {
                Ok(DaemonResponse::List(list)) if list.is_empty() => {
                    println!("No active bindings.")
                }
                Ok(DaemonResponse::List(list)) => {
                    println!("{:<20} {:<30} {:<10} Hosts", "Address", "Remote", "Time");
                    for b in list {
                        println!(
                            "{:<20} {:<30} {:<10} {:?}",
                            b.addr,
                            b.remote.unwrap_or_default(),
                            b.timestamp,
                            b.jump_hosts
                        );
                        match b.cmd {
                            None => println!("  Command: None"),
                            Some(ref cmd) => println!("  Command: {:?}", cmd),
                        }
                    }
                }
                Ok(resp) => println!("Unexpected: {:?}", resp),
                Err(e) => eprintln!("{}", e),
            }
        }
        None => println!("Use --help"),
    }

    if cli.verbose {
        std::fs::create_dir_all(LOG_PATH.parent().expect("Parent must exist"))
            .expect("Failed to create log directory");
        let mut log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(LOG_PATH.as_path())
            .expect("Cannot open log file");
        log_file
            .seek(SeekFrom::Start(start_offset))
            .expect("Cannot seek in log file");

        let mut reader = BufReader::new(log_file);
        let mut line = String::new();

        // Read until EOF
        while reader.read_line(&mut line).expect("") > 0 {
            print!("{}", line);
            line.clear();
        }
    }
}
