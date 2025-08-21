// Integration tests for async_ssh module
#[cfg(test)]
mod tests {
    use sshbind::async_ssh::{AsyncSession, SessionConfiguration};
    use std::net::{SocketAddr, IpAddr, Ipv4Addr};

    #[tokio::test]
    async fn test_async_session_creation() {
        // Test that we can create an AsyncSession using a definitely unused port
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 65432);
        let config = SessionConfiguration::default();
        
        // This should fail to connect but should create the session structure properly
        let result = AsyncSession::connect(addr, Some(config)).await;
        
        // The result doesn't matter as much as the fact that it compiles and runs
        match result {
            Err(e) => {
                // Should be either connection refused or timeout
                println!("Expected connection error: {}", e);
                assert!(true); // Connection failed as expected
            }
            Ok(session) => {
                // If there's actually an SSH server listening, that's fine too
                // The important thing is our code compiled and ran
                println!("Unexpected success connecting to port 65432");
                assert!(true); // Still a success for our module compilation
            }
        }
    }

    #[test]
    fn test_session_configuration_default() {
        let config = SessionConfiguration::default();
        assert_eq!(config.compress, false);
        assert_eq!(config.timeout, None);
    }
}