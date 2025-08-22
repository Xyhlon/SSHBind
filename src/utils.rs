use futures::io::{AsyncRead, AsyncWrite};
use log::warn;
use std::process::Command;

/// Bidirectionally copies data between two async streams
pub async fn connect_duplex<A, B>(a: A, b: B)
where
    A: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    B: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    crate::executor::spawn(async move {
        use futures::io::{AsyncReadExt, AsyncWriteExt};
        use futures::future::select;
        use futures::pin_mut;
        
        let (mut a_read, mut a_write) = futures::AsyncReadExt::split(a);
        let (mut b_read, mut b_write) = futures::AsyncReadExt::split(b);

        let a_to_b = async move {
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                match a_read.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Err(e) = b_write.write_all(&buf[..n]).await {
                            warn!("Error writing A to B: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("Error reading from A: {}", e);
                        break;
                    }
                }
            }
        };

        let b_to_a = async move {
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                match b_read.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Err(e) = a_write.write_all(&buf[..n]).await {
                            warn!("Error writing B to A: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("Error reading from B: {}", e);
                        break;
                    }
                }
            }
        };

        pin_mut!(a_to_b, b_to_a);
        
        match select(a_to_b, b_to_a).await {
            futures::future::Either::Left(_) => {},
            futures::future::Either::Right(_) => {},
        }
    });
}

/// Finds the sops binary in system PATH
pub fn find_sops_binary() -> Result<String, String> {
    let output = Command::new("which")
        .arg("sops")
        .output()
        .map_err(|e| format!("Failed to execute 'which' command: {}", e))?;

    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout);
        let path = path.trim();
        if path.is_empty() {
            return Err("sops not found in PATH".to_string());
        }
        Ok(path.to_string())
    } else {
        Err("sops not found in PATH".to_string())
    }
}

/// Decrypts SOPS file and returns decrypted content
pub fn decrypt_sops_file(file_path: &str) -> Result<String, String> {
    let sops_path = find_sops_binary()?;
    
    let output = Command::new(sops_path)
        .arg("-d")
        .arg(file_path)
        .output()
        .map_err(|e| format!("Failed to execute sops command: {}", e))?;

    if output.status.success() {
        String::from_utf8(output.stdout)
            .map_err(|e| format!("Failed to parse sops output as UTF-8: {}", e))
    } else {
        let error_msg = String::from_utf8_lossy(&output.stderr);
        Err(format!("sops decryption failed: {}", error_msg))
    }
}