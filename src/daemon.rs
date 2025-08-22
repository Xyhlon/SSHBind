use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions, Name, Stream, ToFsName, ToNsName,
};
use log::{error, LevelFilter};
use named_sem::{Error as SemError, NamedSemaphore};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use env_logger::{Builder, Target};

use crate::LOG_PATH;

#[derive(Serialize, Deserialize, Debug)]
pub enum DaemonCommand {
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
pub enum DaemonResponse {
    Success(String),
    List(Vec<BindingDetails>),
    Error(String),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BindingDetails {
    pub addr: String,
    pub jump_hosts: Vec<String>,
    pub cmd: Option<String>,
    pub remote: Option<String>,
    pub timestamp: u64,
}

static BINDINGS: LazyLock<Mutex<HashMap<String, BindingDetails>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static TIME_TO_DIE: LazyLock<Mutex<bool>> = LazyLock::new(|| Mutex::new(false));

// Constants for semaphores and IPC
const SERVER_NAME_PATH: &str = "/tmp/sshbind_daemon.ipc";
const SEM_SERVER_READY: &str = "/sshbind_server_ready";
const SEM_SENDING_STICK: &str = "/sshbind_sending_stick";
const SEM_CHECK_LOCK: &str = "/sshbind_check_lock";
const SEM_FLOOD_GATE: &str = "/sshbind_flood_gate";

pub fn server_name() -> Result<Name<'static>, Box<dyn Error>> {
    if let Ok(n) = "sshbind.daemon".to_ns_name::<GenericNamespaced>() {
        Ok(n.into_owned())
    } else {
        match SERVER_NAME_PATH.to_fs_name::<GenericFilePath>() {
            Ok(n) => Ok(n.into_owned()),
            Err(e) => Err(Box::new(e)),
        }
    }
}

pub fn cleanup_server_resources() -> Result<(), Box<dyn Error>> {
    let mut sending_stick = NamedSemaphore::create(SEM_SENDING_STICK, 1)?;
    let mut server_ready = NamedSemaphore::create(SEM_SERVER_READY, 0)?;
    let mut check_lock = NamedSemaphore::create(SEM_CHECK_LOCK, 1)?;
    let mut flood_gate = NamedSemaphore::create(SEM_FLOOD_GATE, 0)?;
    
    sending_stick.wait()?;
    check_lock.wait()?;
    server_ready.wait()?;

    check_lock.post()?;
    flood_gate.wait()?;
    sending_stick.post()?;
    Ok(())
}

pub fn start_ipc_daemon() -> Result<(), Box<dyn Error>> {
    let mut server_ready = NamedSemaphore::create(SEM_SERVER_READY, 0)?;
    let mut check_lock = NamedSemaphore::create(SEM_CHECK_LOCK, 1)?;
    let mut flood_gate = NamedSemaphore::create(SEM_FLOOD_GATE, 0)?;

    check_lock.wait()?;

    if server_ready.try_wait().is_ok() {
        check_lock.post()?;
        return Ok(());
    }

    let socket_name = match server_name() {
        Ok(name) => name,
        Err(e) => {
            check_lock.post()?;
            return Err(e);
        }
    };
    
    let listener = ListenerOptions::new()
        .name(socket_name)
        .create_sync()
        .map_err(|e| {
            check_lock.post().ok();
            Box::<dyn Error>::from(e)
        })?;

    server_ready.post()?;
    check_lock.post()?;
    flood_gate.post()?;

    setup_logging()?;

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                thread::spawn(move || {
                    if let Err(e) = handle_client_stream(&mut stream) {
                        error!("Error handling client: {}", e);
                    }
                    if TIME_TO_DIE.lock().map_or(false, |ttd| *ttd) {
                        if let Err(e) = cleanup_server_resources() {
                            error!("Error cleaning up server resources: {}", e);
                        }
                        std::process::exit(0);
                    }
                });
            }
            Err(e) => {
                error!("Failed to accept client: {}", e);
            }
        }
    }
    Ok(())
}

fn setup_logging() -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(LOG_PATH.parent().ok_or("Parent must exist")?)?;
    let log_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .append(false)
        .open(LOG_PATH.as_path())?;
    Builder::new()
        .filter(None, LevelFilter::Debug)
        .target(Target::Pipe(Box::new(log_file)))
        .init();
    Ok(())
}

fn handle_client_stream(stream: &mut Stream) -> Result<(), Box<dyn Error>> {
    let mut buf = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        let n = stream.read(&mut byte)?;
        if n == 0 {
            return Err(Box::from("client closed connection"));
        }
        if byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0]);
    }

    let cmd: DaemonCommand = serde_json::from_slice(&buf)?;
    let response = handle_command(cmd);
    let resp_bytes = serde_json::to_vec(&response)?;
    stream.write_all(&resp_bytes)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

fn handle_command(cmd: DaemonCommand) -> DaemonResponse {
    match cmd {
        DaemonCommand::Ping => DaemonResponse::Success("PONG".into()),
        DaemonCommand::Bind {
            addr,
            jump_hosts,
            remote,
            sopsfile,
            cmd,
        } => {
            match BINDINGS.lock() {
                Ok(mut binds) => {
                    if binds.contains_key(&addr) {
                        DaemonResponse::Error(format!("Address {} is already bound.", addr))
                    } else {
                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                    
                binds.insert(
                    addr.clone(),
                    BindingDetails {
                        addr: addr.clone(),
                        jump_hosts: jump_hosts.clone(),
                        cmd: cmd.clone(),
                        remote: remote.clone(),
                        timestamp,
                    },
                );
                sshbind::bind(&addr, jump_hosts, remote, &sopsfile, cmd);
                DaemonResponse::Success(format!("Bound at {}", addr))
                    }
                }
                Err(e) => DaemonResponse::Error(format!("Failed to acquire bindings lock: {}", e)),
            }
        }
        DaemonCommand::Unbind { addr } => {
            sshbind::unbind(&addr);
            match BINDINGS.lock() {
                Ok(mut binds) => {
                    binds.remove(&addr);
                    DaemonResponse::Success(format!("Unbound {}", addr))
                }
                Err(e) => DaemonResponse::Error(format!("Failed to acquire bindings lock: {}", e)),
            }
        }
        DaemonCommand::Shutdown => {
            match BINDINGS.lock() {
                Ok(mut binds) => {
                    for a in binds.keys().cloned().collect::<Vec<_>>() {
                        sshbind::unbind(&a);
                    }
                    binds.clear();
                    match TIME_TO_DIE.lock() {
                        Ok(mut ttd) => {
                            *ttd = true;
                            DaemonResponse::Success("All bindings cleared. Shutting down.".into())
                        }
                        Err(e) => DaemonResponse::Error(format!("Failed to set shutdown flag: {}", e)),
                    }
                }
                Err(e) => DaemonResponse::Error(format!("Failed to acquire bindings lock: {}", e)),
            }
        }
        DaemonCommand::List => {
            match BINDINGS.lock() {
                Ok(binds) => {
                    let list = binds.values().cloned().collect();
                    DaemonResponse::List(list)
                }
                Err(e) => DaemonResponse::Error(format!("Failed to acquire bindings lock: {}", e)),
            }
        }
    }
}

pub fn spawn_daemon_if_needed() -> Result<(), Box<dyn Error>> {
    let mut server_ready = NamedSemaphore::create(SEM_SERVER_READY, 0)?;
    let mut check_lock = NamedSemaphore::create(SEM_CHECK_LOCK, 1)?;
    let mut flood_gate = NamedSemaphore::create(SEM_FLOOD_GATE, 0)?;
    let mut sending_stick = NamedSemaphore::create(SEM_SENDING_STICK, 1)?;
    
    sending_stick.wait()?;
    check_lock.wait()?;
    
    match server_ready.try_wait() {
        Ok(_) => {
            server_ready.post()?;
            check_lock.post()?;
            sending_stick.post()?;
            return Ok(());
        }
        Err(SemError::WouldBlock) => {
            std::fs::create_dir_all(LOG_PATH.parent().ok_or("Parent must exist")?)?;

            let stderr = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(
                    LOG_PATH
                        .parent()
                        .ok_or("Parent must exist")?
                        .join("sshbind_stderr.log"),
                )?;
            let stdout = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(
                    LOG_PATH
                        .parent()
                        .ok_or("Parent must exist")?
                        .join("sshbind_stdout.log"),
                )?;
            let exe = std::env::current_exe()?;

            let _ = Command::new(exe)
                .arg("daemon")
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn();
        }
        Err(e) => return Err(Box::new(e)),
    }
    check_lock.post()?;
    flood_gate.wait()?;
    flood_gate.post()?;
    sending_stick.post()?;
    Ok(())
}