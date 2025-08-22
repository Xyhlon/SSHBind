use env_logger::Builder;
use log::info;
use log::LevelFilter;
use sshbind::{bind, unbind, Creds, YamlCreds};
use std::io::Write;
use std::sync::LazyLock;
use tempfile::NamedTempFile;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// Ensure the logger is initialized only once
static LOGGER: LazyLock<()> = LazyLock::new(|| {
    Builder::new()
        .filter(None, LevelFilter::Info) 
        .format(|buf, record| writeln!(buf, "[{}] - {}", record.level(), record.args()))
        .init();
});

#[tokio::test]
async fn test_simple_ssh_tunnel() -> Result<(), Box<dyn std::error::Error>> {
    #[allow(clippy::let_unit_value)]
    let _ = *LOGGER;
    
    // Create a simple unencrypted credentials file
    let mut testcreds = YamlCreds::new();
    testcreds.insert(
        "127.0.0.1:2222".to_string(),
        Creds {
            username: "test".to_string(),
            password: "password".to_string(),
            totp_key: None,
        },
    );
    
    // Write unencrypted YAML to temp file
    let mut temp_file = NamedTempFile::new()?;
    let yaml_content = serde_yml::to_string(&testcreds)?;
    temp_file.write_all(yaml_content.as_bytes())?;
    let creds_path = temp_file.path().to_str().unwrap();
    
    info!("Created unencrypted credentials file at: {}", creds_path);
    
    // Start a simple echo service
    let service_handle = tokio::spawn(async {
        let listener = TcpListener::bind("127.0.0.1:8080").await.unwrap();
        info!("Echo service listening on 127.0.0.1:8080");
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            info!("Echo service: accepted connection");
            socket.write_all(b"hello world!").await.unwrap();
        }
    });
    
    // Give service time to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    
    // Start SSH bind tunnel (no SSH server, will fail but should handle gracefully)
    let bind_addr = "127.0.0.1:7000";
    let jump_hosts = vec!["127.0.0.1:2222".to_string()];
    let service_addr = Some("127.0.0.1:8080".to_string());
    
    // Run bind in a blocking task
    let bind_addr_clone = bind_addr.to_string();
    let jump_hosts_clone = jump_hosts.clone();
    let service_addr_clone = service_addr.clone();
    let creds_path_str = creds_path.to_string();
    
    tokio::task::spawn_blocking(move || {
        bind(
            &bind_addr_clone,
            jump_hosts_clone,
            service_addr_clone,
            &creds_path_str,
            None,
        );
    });
    
    // Give bind time to start and attempt connection
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    // The bind should fail to connect to SSH server but handle it gracefully
    info!("Test completed - bind should have logged connection failure");
    
    unbind(bind_addr);
    service_handle.abort();
    
    Ok(())
}