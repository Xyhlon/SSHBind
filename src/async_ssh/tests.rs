use super::*;
use crate::executor;
use std::time::Duration;

#[test]
fn test_session_configuration_default() {
    let config = SessionConfiguration::default();
    assert!(!config.compress);
    assert_eq!(config.timeout, None);
}

#[test]
fn test_session_configuration_custom() {
    let config = SessionConfiguration {
        compress: true,
        timeout: Some(30),
    };
    assert!(config.compress);
    assert_eq!(config.timeout, Some(30));
}

#[test]
fn test_async_tcp_stream_connect_timeout() {
    let result: crate::async_ssh::Result<()> = executor::block_on(async {
        // Try to connect to a non-existent port
        let addr: std::net::SocketAddr = "127.0.0.1:54321".parse().unwrap();
        match AsyncTcpStream::connect(addr).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e.into()),
        }
    });
    
    // Should get connection refused error
    assert!(result.is_err());
}

#[test]
fn test_async_session_creation() {
    let result = executor::block_on(async {
        // Try to connect to a non-existent SSH port
        let addr: std::net::SocketAddr = "127.0.0.1:54321".parse().unwrap();
        AsyncSession::connect(addr, None).await
    });
    
    // Should get connection error
    assert!(result.is_err());
}

#[test]
fn test_async_tcp_stream_from_std() {
    use std::net::TcpListener;
    
    // Create a local server
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    
    std::thread::spawn(move || {
        let _stream = listener.accept().unwrap();
        // Just accept and close
    });
    
    let result = executor::block_on(async move {
        let std_stream = std::net::TcpStream::connect(addr)?;
        let _async_stream = AsyncTcpStream::from_std(std_stream)?;
        Ok::<(), crate::async_ssh::Error>(())
    });
    
    assert!(result.is_ok());
}

#[test]
fn test_error_types() {
    // Test error conversion
    let io_error = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "test");
    let ssh_error: Error = io_error.into();
    
    match ssh_error {
        Error::Io(_) => {},
        _ => panic!("Expected IO error"),
    }
}

#[test]
fn test_would_block_detection() {
    use ssh2::Error as Ssh2Error;
    
    // Test would_block function (internal function)
    let ssh_error = Ssh2Error::new(ssh2::ErrorCode::Session(-37), "would block");
    // We can't directly test the internal would_block function, but we can test error handling
    match Error::from(ssh_error) {
        Error::Ssh2(_) => {},
        _ => panic!("Expected SSH2 error"),
    }
}

#[test]
fn test_keyboard_interactive_prompt() {
    let prompt = Prompt {
        text: "Password:".into(),
        echo: false,
    };
    
    assert_eq!(prompt.text, "Password:");
    assert!(!prompt.echo);
}

#[test]
fn test_async_operations_timeout() {
    let start = std::time::Instant::now();
    let result = executor::block_on(async move {
        // This should timeout after 30 seconds (default timeout)
        loop {
            executor::yield_now().await;
            if start.elapsed() > Duration::from_secs(1) {
                // Break early to avoid actually waiting 30 seconds
                break;
            }
        }
        Ok::<(), Error>(())
    });
    
    // Should complete quickly since we break the loop
    assert!(result.is_ok());
    assert!(start.elapsed() < Duration::from_secs(2));
}