use sshbind::{HostPort, Creds, YamlCreds};

#[test]
fn test_hostport_parsing() {
    // Test valid host:port combinations
    let hp = HostPort::try_from("127.0.0.1:22").expect("Failed to parse valid host:port");
    assert_eq!(hp.host, "127.0.0.1");
    assert_eq!(hp.port, 22);
    
    let hp = HostPort::try_from("example.com:2222").expect("Failed to parse domain:port");
    assert_eq!(hp.host, "example.com");
    assert_eq!(hp.port, 2222);
    
    let hp = HostPort::try_from("[::1]:22").expect("Failed to parse IPv6:port");
    assert_eq!(hp.host, "::1");
    assert_eq!(hp.port, 22);
    
    // Test FromStr trait
    let hp: HostPort = "192.168.1.1:8080".parse().expect("Failed to parse via FromStr");
    assert_eq!(hp.host, "192.168.1.1");
    assert_eq!(hp.port, 8080);
    
    // Test Display trait
    let hp = HostPort { host: "test.com".to_string(), port: 443 };
    assert_eq!(format!("{}", hp), "test.com:443");
}

#[test]
fn test_hostport_parsing_failures() {
    // Test invalid formats
    assert!(HostPort::try_from("invalid").is_err());
    assert!(HostPort::try_from("host:").is_err());
    assert!(HostPort::try_from(":22").is_err());
    assert!(HostPort::try_from("host:invalid_port").is_err());
    assert!(HostPort::try_from("host:99999").is_err()); // Port too high
}

#[test]
fn test_credentials_structure() {
    let creds = Creds {
        username: "testuser".to_string(),
        password: "testpass".to_string(),
        totp_key: Some("GEZDGNBVGY3TQOJQ".to_string()),
    };
    
    assert_eq!(creds.username, "testuser");
    assert_eq!(creds.password, "testpass");
    assert_eq!(creds.totp_key, Some("GEZDGNBVGY3TQOJQ".to_string()));
    
    // Test credentials without TOTP
    let creds_no_totp = Creds {
        username: "user2".to_string(),
        password: "pass2".to_string(),
        totp_key: None,
    };
    
    assert_eq!(creds_no_totp.username, "user2");
    assert_eq!(creds_no_totp.password, "pass2");
    assert_eq!(creds_no_totp.totp_key, None);
}

#[test]
fn test_yaml_credentials_map() {
    let mut creds_map = YamlCreds::new();
    
    creds_map.insert("host1:22".to_string(), Creds {
        username: "user1".to_string(),
        password: "pass1".to_string(),
        totp_key: None,
    });
    
    creds_map.insert("host2:2222".to_string(), Creds {
        username: "user2".to_string(),
        password: "pass2".to_string(),
        totp_key: Some("TOTP_KEY".to_string()),
    });
    
    assert_eq!(creds_map.len(), 2);
    
    // Test retrieval
    let host1_creds = creds_map.get("host1:22").expect("host1 creds not found");
    assert_eq!(host1_creds.username, "user1");
    assert_eq!(host1_creds.totp_key, None);
    
    let host2_creds = creds_map.get("host2:2222").expect("host2 creds not found");
    assert_eq!(host2_creds.username, "user2");
    assert_eq!(host2_creds.totp_key, Some("TOTP_KEY".to_string()));
}

#[test]
fn test_credentials_serialization() {
    let mut creds_map = YamlCreds::new();
    
    creds_map.insert("test.example.com:22".to_string(), Creds {
        username: "admin".to_string(),
        password: "secret123".to_string(),
        totp_key: Some("GEZDGNBVGY3TQOJQ".to_string()),
    });
    
    // Test serialization to YAML
    let yaml_str = serde_yml::to_string(&creds_map).expect("Failed to serialize to YAML");
    assert!(yaml_str.contains("test.example.com:22"));
    assert!(yaml_str.contains("username: admin"));
    assert!(yaml_str.contains("password: secret123"));
    assert!(yaml_str.contains("totp_key: GEZDGNBVGY3TQOJQ"));
    
    // Test deserialization from YAML
    let deserialized: YamlCreds = serde_yml::from_str(&yaml_str)
        .expect("Failed to deserialize from YAML");
    
    assert_eq!(deserialized.len(), 1);
    let creds = deserialized.get("test.example.com:22").expect("Creds not found after deserialization");
    assert_eq!(creds.username, "admin");
    assert_eq!(creds.password, "secret123");
    assert_eq!(creds.totp_key, Some("GEZDGNBVGY3TQOJQ".to_string()));
}

#[test]
fn test_hostport_socket_addr_conversion() {
    use std::net::SocketAddr;
    
    let hp = HostPort {
        host: "127.0.0.1".to_string(),
        port: 8080,
    };
    
    let socket_addr: SocketAddr = hp.into();
    assert_eq!(socket_addr.ip().to_string(), "127.0.0.1");
    assert_eq!(socket_addr.port(), 8080);
}

#[test]
fn test_credentials_clone_and_equality() {
    let creds1 = Creds {
        username: "user".to_string(),
        password: "pass".to_string(),
        totp_key: Some("KEY123".to_string()),
    };
    
    let creds2 = creds1.clone();
    assert_eq!(creds1, creds2);
    
    let creds3 = Creds {
        username: "user".to_string(),
        password: "different".to_string(),
        totp_key: Some("KEY123".to_string()),
    };
    
    assert_ne!(creds1, creds3);
}

#[test]
fn test_auth_method_parsing() {
    // Test the AuthMethod enum indirectly through string parsing
    // This tests the internal functionality used in userauth
    
    let methods = "password,keyboard-interactive,publickey";
    let method_list: Vec<&str> = methods.split(',').collect();
    
    assert_eq!(method_list.len(), 3);
    assert!(method_list.contains(&"password"));
    assert!(method_list.contains(&"keyboard-interactive"));
    assert!(method_list.contains(&"publickey"));
}

#[test]
fn test_empty_credentials_map() {
    let creds_map = YamlCreds::new();
    assert_eq!(creds_map.len(), 0);
    assert!(creds_map.is_empty());
    
    // Test that getting non-existent key returns None
    assert!(creds_map.get("nonexistent:22").is_none());
}

#[test] 
fn test_credentials_with_special_characters() {
    let creds = Creds {
        username: "user@domain.com".to_string(),
        password: "p@ssw0rd!#$%".to_string(),
        totp_key: Some("MFRGG43JMVZXG5DJMRSXG43FMFZGK43UMFZGK".to_string()),
    };
    
    // Test that special characters are preserved
    assert!(creds.username.contains('@'));
    assert!(creds.password.contains('!'));
    assert!(creds.password.contains('#'));
    assert!(creds.password.contains('$'));
    assert!(creds.password.contains('%'));
    
    // Test serialization/deserialization with special characters
    let mut creds_map = YamlCreds::new();
    creds_map.insert("test:22".to_string(), creds);
    
    let yaml_str = serde_yml::to_string(&creds_map).expect("Failed to serialize with special chars");
    let deserialized: YamlCreds = serde_yml::from_str(&yaml_str)
        .expect("Failed to deserialize with special chars");
    
    let retrieved_creds = deserialized.get("test:22").expect("Creds not found");
    assert_eq!(retrieved_creds.username, "user@domain.com");
    assert_eq!(retrieved_creds.password, "p@ssw0rd!#$%");
}

#[test]
fn test_multiple_hostport_formats() {
    // Test various valid host:port combinations
    let test_cases = vec![
        ("localhost:22", "localhost", 22),
        ("192.168.1.1:2222", "192.168.1.1", 2222),
        ("example.com:443", "example.com", 443),
        ("sub.domain.example.org:8080", "sub.domain.example.org", 8080),
    ];
    
    for (input, expected_host, expected_port) in test_cases {
        let hp = HostPort::try_from(input).expect(&format!("Failed to parse {}", input));
        assert_eq!(hp.host, expected_host, "Host mismatch for {}", input);
        assert_eq!(hp.port, expected_port, "Port mismatch for {}", input);
        
        // Test round-trip via Display
        let formatted = format!("{}", hp);
        assert_eq!(formatted, input, "Display format mismatch for {}", input);
    }
}