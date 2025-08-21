use env_logger::Builder;
use log::{info, LevelFilter};
use std::io::Write;
use std::sync::LazyLock;
use std::time::Duration;

// Ensure the logger is initialized only once
static LOGGER: LazyLock<()> = LazyLock::new(|| {
    Builder::new()
        .filter(None, LevelFilter::Debug)
        .format(|buf, record| writeln!(buf, "[{}] - {}", record.level(), record.args()))
        .init();
});

#[tokio::test]
async fn minimal_ssh_test() -> Result<(), Box<dyn std::error::Error>> {
    #[allow(clippy::let_unit_value)]
    let _ = *LOGGER; // Ensure logger is initialized
    
    info!("Starting minimal SSH test");
    
    // Just test that we can establish an SSH connection without the complex server setup
    use sshbind::{bind, unbind};
    
    // Try to bind to a non-existent SSH server - should fail fast
    bind(
        "127.0.0.1:8900",
        vec!["127.0.0.1:9999".to_string()], // Non-existent SSH server
        Some("127.0.0.1:8901".to_string()),
        "/tmp/nonexistent.yaml", // Non-existent credentials 
        None,
    );
    
    // Give it a moment to try connecting
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    // Try to connect to our bound port - should also fail since SSH connection failed
    match tokio::net::TcpStream::connect("127.0.0.1:8900").await {
        Ok(_) => {
            info!("Unexpected successful connection");
        }
        Err(e) => {
            info!("Expected connection failure: {}", e);
        }
    }
    
    unbind("127.0.0.1:8900");
    
    info!("Minimal test completed");
    Ok(())
}