use sshbind::executor;
use sshbind::async_ssh::{AsyncSession, SessionConfiguration};

fn main() {
    println!("Starting simple SSH test");
    
    let result = executor::block_on(async {
        println!("In async block");
        
        // Try to connect to localhost on port 22 (which likely doesn't exist)
        let addr = "127.0.0.1:22".parse().unwrap();
        println!("Trying to connect to {:?}", addr);
        
        match AsyncSession::connect(addr, None).await {
            Ok(_) => {
                println!("Connected successfully (unexpected)");
                Ok(())
            }
            Err(e) => {
                println!("Connection failed as expected: {}", e);
                Ok(())
            }
        }
    });
    
    println!("Test completed with result: {:?}", result);
}