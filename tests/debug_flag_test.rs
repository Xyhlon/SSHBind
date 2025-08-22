use env_logger::Builder;
use log::LevelFilter;
use serial_test::serial;
use sshbind::{bind, unbind};
use sshbind::{Creds, YamlCreds};
use std::io::Write;
use std::sync::LazyLock;
use std::time::Duration;

mod helpers;

// Ensure the logger is initialized only once
static LOGGER: LazyLock<()> = LazyLock::new(|| {
    Builder::new()
        .filter(None, LevelFilter::Info)
        .format(|buf, record| writeln!(buf, "[{}] - {}", record.level(), record.args()))
        .init();
});

/// **Given**: A bind call with debug flag explicitly set to Some(true)
/// **When**: The bind function is called with valid sops file
/// **Then**: Debug simulation should execute and complete successfully
#[tokio::test]
#[serial]
async fn test_debug_flag_enabled_integration() {
    #[allow(clippy::let_unit_value)]
    let _ = *LOGGER;

    let mut testcreds = YamlCreds::new();
    testcreds.insert(
        "127.0.0.1:2222".to_string(),
        Creds {
            username: "testuser".to_string(),
            password: "testpass".to_string(),
            totp_key: None,
        },
    );

    let tmp_dir = helpers::setup_sopsfile(testcreds.clone());
    let sopsfile_path = tmp_dir.path().join("secrets.yaml");

    let bind_addr = "127.0.0.1:9001";
    let jump_hosts = vec!["127.0.0.1:2222".to_string()];
    let remote_addr = "127.0.0.1:8080";

    // This test will fail initially because debug simulation requires real SSH servers
    // but we're testing the debug flag behavior specifically
    bind(
        bind_addr,
        jump_hosts,
        remote_addr,
        sopsfile_path.to_str().unwrap(),
        Some(true),
    );

    // Wait a moment for debug simulation to potentially complete
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    unbind(bind_addr);
    
    // This test will fail - we need to verify debug simulation actually ran
    assert!(false, "Need to verify debug simulation executed");
}

/// **Given**: A bind call with debug flag explicitly set to Some(false)
/// **When**: The bind function is called
/// **Then**: No debug simulation should occur, only normal server startup
#[tokio::test]
#[serial]
async fn test_debug_flag_disabled_integration() {
    #[allow(clippy::let_unit_value)]
    let _ = *LOGGER;

    let mut testcreds = YamlCreds::new();
    testcreds.insert(
        "127.0.0.1:2223".to_string(),
        Creds {
            username: "testuser".to_string(),
            password: "testpass".to_string(),
            totp_key: None,
        },
    );

    let tmp_dir = helpers::setup_sopsfile(testcreds.clone());
    let sopsfile_path = tmp_dir.path().join("secrets.yaml");

    let bind_addr = "127.0.0.1:9002";
    let jump_hosts = vec!["127.0.0.1:2223".to_string()];
    let remote_addr = "127.0.0.1:8080";

    bind(
        bind_addr,
        jump_hosts,
        remote_addr,
        sopsfile_path.to_str().unwrap(),
        Some(false),
    );

    // Wait a moment then unbind
    tokio::time::sleep(Duration::from_millis(100)).await;
    unbind(bind_addr);
    
    // This test will fail - we need to verify debug simulation DID NOT run
    assert!(false, "Need to verify debug simulation was disabled");
}

/// **Given**: A bind call with debug flag set to None
/// **When**: The bind function is called
/// **Then**: No debug simulation should occur, default behavior
#[tokio::test]
#[serial]
async fn test_debug_flag_none_integration() {
    #[allow(clippy::let_unit_value)]
    let _ = *LOGGER;

    let mut testcreds = YamlCreds::new();
    testcreds.insert(
        "127.0.0.1:2224".to_string(),
        Creds {
            username: "testuser".to_string(),
            password: "testpass".to_string(),
            totp_key: None,
        },
    );

    let tmp_dir = helpers::setup_sopsfile(testcreds.clone());
    let sopsfile_path = tmp_dir.path().join("secrets.yaml");

    let bind_addr = "127.0.0.1:9003";
    let jump_hosts = vec!["127.0.0.1:2224".to_string()];
    let remote_addr = "127.0.0.1:8080";

    bind(
        bind_addr,
        jump_hosts,
        remote_addr,
        sopsfile_path.to_str().unwrap(),
        None,
    );

    // Wait a moment then unbind
    tokio::time::sleep(Duration::from_millis(100)).await;
    unbind(bind_addr);
    
    // This test will fail - we need to verify debug simulation DID NOT run
    assert!(false, "Need to verify None debug flag uses default behavior");
}

/// **Given**: Debug flag enabled with invalid SSH configuration
/// **When**: Debug simulation attempts to run
/// **Then**: Error should be handled gracefully without panicking
#[tokio::test]
#[serial]
async fn test_debug_flag_error_handling_integration() {
    #[allow(clippy::let_unit_value)]
    let _ = *LOGGER;

    let mut testcreds = YamlCreds::new();
    testcreds.insert(
        "nonexistent.host:22".to_string(),
        Creds {
            username: "testuser".to_string(),
            password: "testpass".to_string(),
            totp_key: None,
        },
    );

    let tmp_dir = helpers::setup_sopsfile(testcreds.clone());
    let sopsfile_path = tmp_dir.path().join("secrets.yaml");

    let bind_addr = "127.0.0.1:9004";
    let jump_hosts = vec!["nonexistent.host:22".to_string()];
    let remote_addr = "127.0.0.1:8080";

    // This should not panic even with invalid configuration in debug mode
    bind(
        bind_addr,
        jump_hosts,
        remote_addr,
        sopsfile_path.to_str().unwrap(),
        Some(true),
    );

    // Wait for potential error handling
    tokio::time::sleep(Duration::from_millis(500)).await;
    unbind(bind_addr);
    
    // This test will fail - we need to verify error was handled gracefully
    assert!(false, "Need to verify debug simulation error handling");
}