use clap::{Parser, Subcommand};
use env_logger::{Builder, Target};
use log::{info, LevelFilter};
use named_sem::{Error as SemError, NamedSemaphore};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::result::Result;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use ipc_channel::ipc::{channel, IpcOneShotServer, IpcReceiver, IpcSender};

static LOG_PATH: LazyLock<PathBuf> = LazyLock::new(|| {
    dirs::data_local_dir()
        .expect("Failed to get local data dir")
        .join("sshbind/sshbind.log")
});

static BINDINGS: LazyLock<Mutex<HashMap<String, BindingDetails>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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

const SERVER_NAME_PATH: &str = "/tmp/sshbind_daemon.ipc";
const SEM_SERVER_READY: &str = "/sshbind_server_ready";
// Serializes client creation and server readiness checking
const SEM_SENDING_STICK: &str = "/sshbind_sending_stick";
// I believe we need this because forking isn't allowed in rust
// Thus multiple daemon instances could be started at the same
// time by invoking the daemon command
const SEM_CHECK_LOCK: &str = "/sshbind_check_lock";
const SEM_FLOOD_GATE: &str = "/sshbind_flood_gate";

/// Starts the one-shot IPC server that loops to handle clients.
fn start_ipc_daemon() -> Result<(), Box<dyn Error>> {
    // Bootstrap first server
    let mut server_ready = NamedSemaphore::create(SEM_SERVER_READY, 0)
        .expect("failed to create server_ready semaphore");

    let mut check_lock =
        NamedSemaphore::create(SEM_CHECK_LOCK, 1).expect("failed to create check_lock semaphore");

    let mut flood_gate =
        NamedSemaphore::create(SEM_FLOOD_GATE, 0).expect("failed to create server_ready semaphore");

    check_lock
        .wait()
        .expect("Failed to wait for startup semaphore");
    // Catch dispatch race condition
    // could be better solved via extra semaphore which the clients provide
    // and the server instances waits for and the first getting it is the
    // real server.
    // I should definitly fix this as there is a race-condition
    // with the waiting procedure induced by the client
    let (mut server, mut server_name) = match server_ready.try_wait() {
        Ok(_) => return Ok(()), // Server,
        Err(SemError::WouldBlock) => {
            let (mut server, mut server_name) =
                IpcOneShotServer::<(DaemonCommand, IpcSender<DaemonResponse>)>::new()
                    .expect("failed to create one-shot server");
            std::fs::write(SERVER_NAME_PATH, &server_name)?;

            info!("Wrote server name to {}", SERVER_NAME_PATH);
            server_ready.post();
            (server, server_name)
        }
        Err(e) => return Err(Box::new(e)),
    };
    check_lock.post();

    flood_gate.post();

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

    loop {
        // Accept a single client: returns (receiver,_msg)
        let (rx, (cmd, resp_tx)): (
            IpcReceiver<(DaemonCommand, IpcSender<DaemonResponse>)>,
            (DaemonCommand, IpcSender<DaemonResponse>),
        ) = server.accept()?;

        // Spawn handler thread
        thread::spawn(move || {
            let response = handle_command(cmd);
            let _ = resp_tx.send(response);
        });
        // Prepare next one-shot server
        let (new_server, new_server_name) =
            IpcOneShotServer::<(DaemonCommand, IpcSender<DaemonResponse>)>::new()
                .expect("failed to create one-shot server");
        server = new_server;
        server_name = new_server_name;
        std::fs::write(SERVER_NAME_PATH, &server_name)?;
    }
}

/// Client-side: send a command and wait for a response.
fn send_command(cmd: DaemonCommand) -> Result<DaemonResponse, String> {
    let server_name = std::fs::read_to_string(SERVER_NAME_PATH)
        .map_err(|e| format!("Failed to read server name: {}", e))?;
    // Connect to server one-shot name
    let tx: IpcSender<(DaemonCommand, IpcSender<DaemonResponse>)> =
        IpcSender::connect(server_name).expect("ipc connect failed");

    // Create reply channel
    let (resp_tx, resp_rx): (IpcSender<DaemonResponse>, IpcReceiver<DaemonResponse>) =
        channel().map_err(|e| format!("IPC channel error: {}", e))?;
    // Send (command, reply-sender)
    tx.send((cmd, resp_tx))
        .map_err(|e| format!("IPC send error: {}", e))?;
    // Wait for response
    resp_rx.recv().map_err(|e| format!("IPC recv error: {}", e))
}

/// Process a DaemonCommand and mutate state, returning a DaemonResponse.
fn handle_command(cmd: DaemonCommand) -> DaemonResponse {
    let mut time_to_die = false;
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
            time_to_die = true;
            DaemonResponse::Success("All bindings cleared. Shutting down.".into())
        }
        DaemonCommand::List => {
            let list = BINDINGS.lock().unwrap().values().cloned().collect();
            DaemonResponse::List(list)
        }
    };
    if time_to_die {
        let mut sending_stick = NamedSemaphore::create(SEM_SENDING_STICK, 1)
            .expect("failed to create sending_stick semaphore");
        let mut server_ready = NamedSemaphore::create(SEM_SERVER_READY, 0)
            .expect("failed to create server_ready semaphore");
        let mut check_lock = NamedSemaphore::create(SEM_CHECK_LOCK, 1)
            .expect("failed to create check_lock semaphore");
        let mut flood_gate = NamedSemaphore::create(SEM_FLOOD_GATE, 0)
            .expect("failed to create flood_gate semaphore");
        sending_stick
            .wait()
            .expect("Failed to wait for sending_stick");
        check_lock.wait().expect("Failed to wait for check_lock");
        server_ready
            .wait()
            .expect("Failed to wait for server_ready");
        check_lock.post();
        flood_gate.wait().expect("Failed to wait for flood_gate");
        sending_stick.post();

        std::process::exit(0);
    }
    response
}

fn spawn_daemon_if_needed() {
    println!("Checking if daemon is running...");
    let mut server_ready = NamedSemaphore::create(SEM_SERVER_READY, 0)
        .expect("failed to create server_ready semaphore");
    let mut check_lock =
        NamedSemaphore::create(SEM_CHECK_LOCK, 1).expect("failed to create server_ready semaphore");
    let mut flood_gate =
        NamedSemaphore::create(SEM_FLOOD_GATE, 0).expect("failed to create flood_gate semaphore");
    let mut sending_stick = NamedSemaphore::create(SEM_SENDING_STICK, 1)
        .expect("failed to create sending_stick semaphore");
    sending_stick
        .wait()
        .expect("Failed to wait for sending_stick");
    check_lock
        .wait()
        .expect("Failed to wait for startup semaphore");
    match server_ready.try_wait() {
        Ok(_) => {
            check_lock.post();
            return;
        }
        Err(SemError::WouldBlock) => {
            std::fs::create_dir_all(LOG_PATH.parent().expect("Parent must exist"))
                .expect("Failed to create log directory");

            let mut stderr = OpenOptions::new()
                .create(true)
                .write(true)
                .open(
                    LOG_PATH
                        .parent()
                        .expect("Parent must exist")
                        .join("sshbind_stderr.log"),
                )
                .expect("Cannot open log file");
            let mut stdout = OpenOptions::new()
                .create(true)
                .write(true)
                .open(
                    LOG_PATH
                        .parent()
                        .expect("Parent must exist")
                        .join("sshbind_stdout.log"),
                )
                .expect("Cannot open log file");
            let exe = std::env::current_exe().expect("Failed to get current exe");

            let _ = Command::new(exe)
                .arg("daemon")
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn();
            println!("Daemon started.");
        }
        Err(e) => {
            println!("Error waiting for server_ready: {}", e);
            ()
        }
    }
    check_lock.post();
    println!("Waiting for daemon to start...");
    flood_gate.wait().expect("Failed to wait for flood_gate");
    flood_gate.post();
    sending_stick.post();
    println!("Daemon is running.");
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
