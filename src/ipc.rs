use interprocess::local_socket::{Stream, traits::Stream as StreamTrait};
use std::io::{Read, Write};

use crate::daemon::{DaemonCommand, DaemonResponse, server_name};

/// Client-side: send a command and wait for a response.
pub fn send_command(cmd: DaemonCommand) -> Result<DaemonResponse, String> {
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