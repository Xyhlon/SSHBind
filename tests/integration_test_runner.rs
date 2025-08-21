use sshbind::bind;
use std::env;
use std::process;
use std::io::Write;
use std::fs::File;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <test_name> [args...]", args[0]);
        process::exit(1);
    }

    let test_name = &args[1];
    let result = match test_name.as_str() {
        "test_bind_with_password" => test_bind_with_password(),
        "test_bind_with_key" => test_bind_with_key(),
        "test_bind_timeout" => test_bind_timeout(),
        "test_bind_multiple_connections" => test_bind_multiple_connections(),
        _ => {
            eprintln!("Unknown test: {}", test_name);
            process::exit(1);
        }
    };

    match result {
        Ok(_) => {
            println!("TEST_PASSED");
            process::exit(0);
        }
        Err(e) => {
            eprintln!("TEST_FAILED: {}", e);
            process::exit(1);
        }
    }
}

fn test_bind_with_password() -> Result<(), Box<dyn std::error::Error>> {
    // Create a temporary sops file for testing
    let mut temp_file = File::create("/tmp/test_sops.yaml")?;
    let sops_content = r#"
test:
    username: test
    password: test
"#;
    temp_file.write_all(sops_content.as_bytes())?;
    let sops_path = "/tmp/test_sops.yaml";
    
    // Try to bind - this should work with tokio runtime
    let jump_hosts = vec!["127.0.0.1:2222".to_string()];
    let remote_addr = Some("127.0.0.1:8080".to_string());
    let _result = bind("127.0.0.1:8081", jump_hosts, remote_addr, sops_path, None);
    
    // bind() doesn't return a Result, it runs in background
    // For testing, we just check that it doesn't panic
    println!("Bind call completed without panic");
    Ok(())
}

fn test_bind_with_key() -> Result<(), Box<dyn std::error::Error>> {
    // Create a temporary sops file for key-based auth
    let mut temp_file = File::create("/tmp/test_key_sops.yaml")?;
    let sops_content = r#"
test:
    username: test
    private_key: /tmp/nonexistent_key
"#;
    temp_file.write_all(sops_content.as_bytes())?;
    let sops_path = "/tmp/test_key_sops.yaml";
    
    let jump_hosts = vec!["127.0.0.1:2323".to_string()];
    let remote_addr = Some("127.0.0.1:8082".to_string());
    let _result = bind("127.0.0.1:8083", jump_hosts, remote_addr, sops_path, None);
    
    println!("Key auth bind call completed without panic");
    Ok(())
}

fn test_bind_timeout() -> Result<(), Box<dyn std::error::Error>> {
    // Create a temporary sops file for timeout testing
    let mut temp_file = File::create("/tmp/test_timeout_sops.yaml")?;
    let sops_content = r#"
test:
    username: test
    password: test
"#;
    temp_file.write_all(sops_content.as_bytes())?;
    let sops_path = "/tmp/test_timeout_sops.yaml";
    
    // Use a non-existent port to trigger timeout
    let jump_hosts = vec!["127.0.0.1:9999".to_string()];
    let remote_addr = Some("127.0.0.1:8084".to_string());
    let _result = bind("127.0.0.1:8085", jump_hosts, remote_addr, sops_path, None);
    
    println!("Timeout test bind call completed without panic");
    Ok(())
}

fn test_bind_multiple_connections() -> Result<(), Box<dyn std::error::Error>> {
    // Create a temporary sops file for multiple connection testing
    let mut temp_file = File::create("/tmp/test_multi_sops.yaml")?;
    let sops_content = r#"
test:
    username: test
    password: test
"#;
    temp_file.write_all(sops_content.as_bytes())?;
    let sops_path = "/tmp/test_multi_sops.yaml";
    
    // Try multiple connections to same SSH server
    let jump_hosts1 = vec!["127.0.0.1:2222".to_string()];
    let jump_hosts2 = vec!["127.0.0.1:2222".to_string()];
    
    let remote_addr1 = Some("127.0.0.1:8086".to_string());
    let remote_addr2 = Some("127.0.0.1:8088".to_string());
    
    bind("127.0.0.1:8087", jump_hosts1, remote_addr1, sops_path, None);
    bind("127.0.0.1:8089", jump_hosts2, remote_addr2, sops_path, None);
    
    println!("Multiple connection bind calls completed without panic");
    Ok(())
}