use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use russh::server::Server;
use log::{info, error};

#[path = "../../tests/helpers/mod.rs"]
mod helpers;

use helpers::{SSHServer, Credentials};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <port> <username> [password] [totp_key]", args[0]);
        eprintln!("Example: {} 2222 pi max", args[0]);
        eprintln!("Example: {} 2222 pi max GEZDGNBVGY3TQOJQ", args[0]);
        std::process::exit(1);
    }
    
    let port: u16 = args[1].parse().map_err(|_| "Invalid port number")?;
    let username = args[2].clone();
    let password = args.get(3).unwrap_or(&"".to_string()).clone();
    let totp_key = args.get(4).map(|s| s.clone());
    
    let addr = format!("127.0.0.1:{}", port);
    
    let mut config = russh::server::Config::default();
    use russh::keys::ssh_key::rand_core::OsRng;
    let mut rng = OsRng;
    
    #[cfg(unix)]
    {
        use russh::keys::Algorithm;
        let pk = russh::keys::PrivateKey::random(&mut rng, Algorithm::Ed25519)?;
        config.keys.push(pk);
    }
    #[cfg(windows)]
    {
        use russh::keys::ssh_key::private::{KeypairData, RsaKeypair};
        let keypair = KeypairData::from(RsaKeypair::random(&mut rng, 1024)?);
        let pk = russh::keys::PrivateKey::new(keypair, "")?;
        config.keys.push(pk);
    }
    
    let config = Arc::new(config);
    
    let mut users = HashMap::new();
    users.insert(username, Credentials {
        password,
        require_2fa: totp_key.is_some(),
        two_factor_code: totp_key,
        allowed_pubkey_base64: None,
    });
    
    let mut server = SSHServer::new(Some(users));
    
    info!("Starting SSH test server on {}", addr);
    
    match server.run_on_address(config, addr).await {
        Ok(_) => info!("SSH server stopped"),
        Err(e) => error!("SSH server error: {}", e),
    }
    
    Ok(())
}