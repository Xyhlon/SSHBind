use sshbind::executor;
use sshbind::async_ssh::{AsyncSession, SessionConfiguration};

fn main() {
    println!("Starting handshake test");
    
    let result = executor::block_on(async {
        println!("Connecting...");
        let addr = "127.0.0.1:22".parse().unwrap();
        let session = AsyncSession::connect(addr, None).await?;
        
        println!("Connected, starting handshake...");
        session.handshake().await?;
        println!("Handshake completed!");
        
        Ok::<(), sshbind::async_ssh::Error>(())
    });
    
    println!("Test result: {:?}", result);
}