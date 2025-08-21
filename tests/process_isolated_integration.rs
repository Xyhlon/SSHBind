use std::process::Command;
use std::time::Duration;

/// Run a test in the isolated test runner binary
fn run_isolated_test(test_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new("cargo")
        .args(&["run", "--bin", "integration_test_runner", "--features", "tokio", "--", test_name])
        .output()?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("TEST_PASSED") {
            println!("Test {} passed", test_name);
            Ok(())
        } else {
            println!("Test {} output: {}", test_name, stdout);
            Err(format!("Test {} did not report success", test_name).into())
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        println!("Test {} failed with exit code: {}", test_name, output.status);
        println!("STDOUT: {}", stdout);
        println!("STDERR: {}", stderr);
        Err(format!("Test {} failed", test_name).into())
    }
}

#[test]
fn test_bind_with_password_isolated() {
    match run_isolated_test("test_bind_with_password") {
        Ok(_) => println!("Password auth test passed"),
        Err(e) => {
            // Accept connection refused as a valid result (no SSH server running)
            if e.to_string().contains("Connection refused") || 
               e.to_string().contains("connection was refused") {
                println!("Connection refused (expected if no SSH server)");
            } else {
                panic!("Test failed: {}", e);
            }
        }
    }
}

#[test]
fn test_bind_with_key_isolated() {
    match run_isolated_test("test_bind_with_key") {
        Ok(_) => println!("Key auth test passed"),
        Err(e) => {
            // Accept connection refused or key not found as valid results
            if e.to_string().contains("Connection refused") || 
               e.to_string().contains("connection was refused") ||
               e.to_string().contains("No such file") {
                println!("Expected error: {}", e);
            } else {
                panic!("Test failed: {}", e);
            }
        }
    }
}

#[test]
fn test_bind_timeout_isolated() {
    match run_isolated_test("test_bind_timeout") {
        Ok(_) => println!("Timeout test passed"),
        Err(e) => {
            // Timeout or connection refused are both acceptable
            if e.to_string().contains("timeout") || 
               e.to_string().contains("Connection refused") ||
               e.to_string().contains("connection was refused") {
                println!("Expected timeout or connection error: {}", e);
            } else {
                panic!("Test failed: {}", e);
            }
        }
    }
}

#[test]
fn test_bind_multiple_connections_isolated() {
    match run_isolated_test("test_bind_multiple_connections") {
        Ok(_) => println!("Multiple connections test passed"),
        Err(e) => {
            // Connection refused is acceptable for this test too
            if e.to_string().contains("Connection refused") || 
               e.to_string().contains("connection was refused") {
                println!("Connection refused (expected if no SSH server)");
            } else {
                panic!("Test failed: {}", e);
            }
        }
    }
}