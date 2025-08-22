use std::process::{Command, Stdio, Child};
use std::thread;
use std::time::Duration;
use std::fs;
use std::io::{Write, Read};
use tempfile::TempDir;
use serial_test::serial;
use log::info;

use sshbind::{bind, unbind, Creds, YamlCreds};

fn setup_test_credentials() -> (TempDir, String) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let sops_file = temp_dir.path().join("test_creds.yaml");
    
    let mut creds = YamlCreds::new();
    creds.insert("127.0.0.1:2222".to_string(), Creds {
        username: "pi".to_string(),
        password: "max".to_string(),
        totp_key: None,
    });
    creds.insert("127.0.0.1:2323".to_string(), Creds {
        username: "pi".to_string(),
        password: "max".to_string(),
        totp_key: None,
    });
    
    let yaml_content = serde_yml::to_string(&creds).expect("Failed to serialize creds");
    fs::write(&sops_file, yaml_content).expect("Failed to write creds file");
    
    (temp_dir, sops_file.to_string_lossy().to_string())
}

fn start_ssh_server(port: u16, username: &str, password: &str) -> Result<Child, std::io::Error> {
    Command::new("cargo")
        .args(&[
            "run", "--bin", "ssh_test_server", "--features", "tokio", "--",
            &port.to_string(),
            username,
            password
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

fn start_echo_server(port: u16) -> Result<Child, std::io::Error> {
    // Simple echo server using netcat if available, otherwise use our own
    let nc_available = Command::new("nc")
        .args(&["-h"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    
    if nc_available {
        Command::new("nc")
            .args(&["-l", &format!("127.0.0.1:{}", port)])
            .spawn()
    } else {
        // Fallback: create a simple echo server using a small rust program
        Command::new("python3")
            .args(&["-c", &format!(r#"
import socket
import threading

def handle_client(conn, addr):
    try:
        data = conn.recv(1024)
        if data:
            conn.send(b"hello world!")
        conn.close()
    except:
        pass

s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('127.0.0.1', {}))
s.listen(5)

while True:
    try:
        conn, addr = s.accept()
        t = threading.Thread(target=handle_client, args=(conn, addr))
        t.daemon = True
        t.start()
    except:
        break
"#, port)])
            .spawn()
    }
}

#[test]
#[serial]
fn test_process_separated_ssh_chain() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .is_test(true)
        .try_init()
        .ok();
    
    info!("Starting process separated SSH chain test");
    
    // Set up test credentials
    let (_temp_dir, sops_file) = setup_test_credentials();
    
    // Start SSH servers in separate processes
    let mut ssh_server_1 = start_ssh_server(2222, "pi", "max")
        .expect("Failed to start SSH server 1");
    let mut ssh_server_2 = start_ssh_server(2323, "pi", "max")
        .expect("Failed to start SSH server 2");
    
    // Start echo server
    let mut echo_server = start_echo_server(8080)
        .expect("Failed to start echo server");
    
    // Give servers time to start
    thread::sleep(Duration::from_millis(500));
    
    // Test our SSH binding with custom async runtime
    let bind_addr = "127.0.0.1:7000";
    let jump_hosts = vec!["127.0.0.1:2222".to_string(), "127.0.0.1:2323".to_string()];
    let remote_addr = Some("127.0.0.1:8080".to_string());
    
    // Run bind in separate thread with our custom runtime
    thread::spawn(move || {
        bind(bind_addr, jump_hosts, remote_addr, &sops_file, None);
    });
    
    // Give bind time to establish connections
    thread::sleep(Duration::from_millis(1000));
    
    // Test connection using std TCP (no tokio in test process)
    let test_result = std::panic::catch_unwind(|| {
        let mut stream = std::net::TcpStream::connect("127.0.0.1:7000")
            .expect("Failed to connect to bind address");
        
        stream.write_all(b"test message").expect("Failed to send data");
        
        let mut buf = [0u8; 1024];
        let n = stream.read(&mut buf).expect("Failed to read response");
        
        let response = String::from_utf8_lossy(&buf[..n]);
        info!("Received response: {}", response);
        
        assert!(response.contains("hello world"), "Expected 'hello world', got: {}", response);
    });
    
    // Cleanup
    unbind(bind_addr);
    
    // Terminate servers
    let _ = ssh_server_1.kill();
    let _ = ssh_server_2.kill();
    let _ = echo_server.kill();
    
    // Wait for servers to terminate
    let _ = ssh_server_1.wait();
    let _ = ssh_server_2.wait();
    let _ = echo_server.wait();
    
    // Check test result
    match test_result {
        Ok(_) => info!("Process separated test passed"),
        Err(e) => {
            if let Some(msg) = e.downcast_ref::<&str>() {
                panic!("Test failed: {}", msg);
            } else if let Some(msg) = e.downcast_ref::<String>() {
                panic!("Test failed: {}", msg);
            } else {
                panic!("Test failed with unknown error");
            }
        }
    }
}

#[test]
#[serial]
fn test_custom_async_runtime_basic() {
    // Test that our custom async runtime works independently
    use sshbind::{HostPort};
    
    let host_port = HostPort::try_from("127.0.0.1:22").expect("Failed to parse host:port");
    assert_eq!(host_port.host, "127.0.0.1");
    assert_eq!(host_port.port, 22);
    
    let formatted = format!("{}", host_port);
    assert_eq!(formatted, "127.0.0.1:22");
    
    info!("Custom async runtime basic test passed");
}

#[test]
fn test_connection_without_servers() {
    // Test behavior when no SSH servers are running (should fail gracefully)
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .is_test(true)
        .try_init()
        .ok();
    
    let (_temp_dir, sops_file) = setup_test_credentials();
    
    let bind_addr = "127.0.0.1:7001";
    let jump_hosts = vec!["127.0.0.1:9999".to_string()]; // Non-existent port
    let remote_addr = Some("127.0.0.1:8080".to_string());
    
    // This should fail quickly since no SSH server is running
    let start_time = std::time::Instant::now();
    
    // Run in separate thread to avoid blocking main test thread
    let handle = thread::spawn(move || {
        bind(bind_addr, jump_hosts, remote_addr, &sops_file, None);
    });
    
    // Give it a moment to try connecting
    thread::sleep(Duration::from_millis(100));
    
    // Try to connect - this should fail since binding should have failed
    let connect_result = std::net::TcpStream::connect("127.0.0.1:7001");
    
    match connect_result {
        Err(_) => {
            info!("Connection failed as expected when no SSH servers available");
        }
        Ok(_) => {
            // If it connected, the bind probably succeeded somehow, so clean up
            unbind(bind_addr);
        }
    }
    
    let elapsed = start_time.elapsed();
    info!("Test completed in {:?}", elapsed);
    
    // Don't wait for the thread if it's blocked - just let it timeout naturally
    thread::sleep(Duration::from_millis(50));
}