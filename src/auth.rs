use crate::{async_ssh::AsyncSession, types::*};
use log::{info, warn};
use ssh2_config::{ParseRule, SshConfig};
use std::error::Error;
use std::fs::File;
use std::io::BufReader;

/// Authentication methods enumeration
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum AuthMethod {
    Password,
    PublicKey,
    KeyboardInteractive,
    HostBased,
    GssApiWithMic,
    GssApiKeyex,
    None,
}

impl std::fmt::Display for AuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthMethod::Password => write!(f, "password"),
            AuthMethod::PublicKey => write!(f, "publickey"),
            AuthMethod::KeyboardInteractive => write!(f, "keyboard-interactive"),
            AuthMethod::HostBased => write!(f, "hostbased"),
            AuthMethod::GssApiWithMic => write!(f, "gssapi-with-mic"),
            AuthMethod::GssApiKeyex => write!(f, "gssapi-keyex"),
            AuthMethod::None => write!(f, "none"),
        }
    }
}

impl std::str::FromStr for AuthMethod {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "password" => Ok(AuthMethod::Password),
            "publickey" => Ok(AuthMethod::PublicKey),
            "keyboard-interactive" => Ok(AuthMethod::KeyboardInteractive),
            "hostbased" => Ok(AuthMethod::HostBased),
            "gssapi-with-mic" => Ok(AuthMethod::GssApiWithMic),
            "gssapi-keyex" => Ok(AuthMethod::GssApiKeyex),
            "none" => Ok(AuthMethod::None),
            _ => Err(format!("Unknown authentication method: {}", s)),
        }
    }
}

/// Ordered authentication methods container
#[derive(Debug)]
pub struct OrderedAuthMethods {
    pub methods: Vec<AuthMethod>,
}

impl OrderedAuthMethods {
    pub fn parse(methods_str: &str) -> Self {
        let methods = methods_str
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        Self { methods }
    }
}

/// Handles password authentication
async fn handle_password_auth(
    session: &AsyncSession,
    username: &str,
    password: &str,
) -> Result<bool, Box<dyn Error + Send + Sync>> {
    match session.userauth_password(username, password).await {
        Ok(_) => Ok(true),
        Err(e) => {
            warn!("Password authentication failed: {}", e);
            Ok(false)
        }
    }
}

/// Handles keyboard interactive authentication
async fn handle_keyboard_interactive_auth(
    session: &AsyncSession,
    username: &str,
    password: &str,
    totp_key: &Option<String>,
) -> Result<bool, Box<dyn Error + Send + Sync>> {
    let mut handler = TotpPromptHandler {
        password: password.to_string(),
        totp_key: totp_key.clone(),
    };

    match session
        .userauth_keyboard_interactive(username, &mut handler)
        .await
    {
        Ok(_) => Ok(true),
        Err(e) => {
            warn!("Keyboard interactive authentication failed: {}", e);
            Ok(false)
        }
    }
}

/// Handles public key authentication
async fn handle_public_key_auth(
    session: &AsyncSession,
    username: &str,
) -> Result<bool, Box<dyn Error + Send + Sync>> {
    // Try to use SSH config to find identity files
    let home = dirs::home_dir().ok_or("No home directory found")?;
    let config_path = home.join(".ssh/config");
    
    if !config_path.exists() {
        warn!("SSH config file not found at {:?}", config_path);
        return Ok(false);
    }
    
    let file = File::open(&config_path)?;
    let mut reader = BufReader::new(file);
    let config = SshConfig::default().parse(&mut reader, ParseRule::STRICT)
        .map_err(|_| "Failed to parse SSH config")?;
    
    let host_params = config.query("*"); // Use wildcard for general config
    
    if let Some(identity_files) = host_params.identity_file {
        for identity_file in &identity_files {
            match session
                .userauth_pubkey_file(username, Some(identity_file), identity_file, None)
                .await
            {
                Ok(_) => return Ok(true),
                Err(e) => {
                    warn!("Public key authentication failed with {}: {}", identity_file.display(), e);
                    continue;
                }
            }
        }
    }
    
    warn!("No working identity files found");
    Ok(false)
}

/// Performs user authentication against SSH session
///
/// # Arguments
///
/// * `session` - The SSH session to authenticate
/// * `creds_map` - Credentials mapping for hosts
/// * `host` - The target host information
///
/// # Returns
///
/// A `Result` indicating success or failure of the authentication process.
pub async fn userauth(
    session: &AsyncSession,
    creds_map: &YamlCreds,
    host: &HostPort,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let creds = creds_map
        .get(&host.to_string())
        .ok_or_else(|| format!("Couldn't find credentials for {}", host))?;
    let username = creds.username.clone();
    let password = creds.password.clone();
    let totp_key = creds.totp_key.clone();

    let auth_methods_str = session.auth_methods(&username).await?;
    let mut ordered_auth = OrderedAuthMethods::parse(&auth_methods_str);
    info!(
        "Available authentication methods in order: {:?}",
        ordered_auth
    );

    while !ordered_auth.methods.is_empty() && !session.authenticated() {
        let method = ordered_auth.methods.remove(0);
        info!("Trying authentication method: {}", method);
        
        let success = match method {
            AuthMethod::Password => {
                handle_password_auth(session, &username, &password).await?
            }
            AuthMethod::KeyboardInteractive => {
                handle_keyboard_interactive_auth(
                    session,
                    &username,
                    &password,
                    &totp_key,
                )
                .await?
            }
            AuthMethod::PublicKey => {
                handle_public_key_auth(session, &username).await?
            }
            _ => {
                warn!("Authentication method {} not implemented", method);
                false
            }
        };

        if success {
            info!("Authentication successful with method: {}", method);
            break;
        } else {
            warn!("Authentication failed with method: {}", method);
        }
    }

    if !session.authenticated() {
        return Err("All authentication methods failed".into());
    }

    Ok(())
}

/// Establishes SSH connection chain through jump hosts
pub async fn connect_chain(
    jump_hosts: &[HostPort],
    creds: &YamlCreds,
) -> Result<AsyncSession, Box<dyn Error + Send + Sync>> {
    if jump_hosts.is_empty() {
        return Err("No jump hosts provided".into());
    }

    let first_host = &jump_hosts[0];
    info!("Connecting to first host: {}", first_host);
    
    // Create TCP connection to first host
    let stream = std::net::TcpStream::connect(first_host.to_socket_addr()?)?;
    stream.set_nonblocking(true)?;
    
    // Create async stream and session
    let reactor = std::sync::Arc::new(crate::async_ssh::reactor::Reactor::new()?);
    let async_stream = crate::async_ssh::AsyncTcpStream::new(stream, reactor)?;
    let mut session = AsyncSession::from_stream(async_stream)?;
    session.handshake().await?;
    userauth(&session, creds, first_host).await?;

    // Currently only supports single hop connections
    // Multi-hop requires wrapping SSH channels as streams, which is complex
    if jump_hosts.len() > 1 {
        return Err(format!(
            "Multi-hop SSH connections are not supported. Only single hop connections are implemented. \
            Received {} hosts, but only the first will be used: {}",
            jump_hosts.len(),
            first_host
        ).into());
    }

    Ok(session)
}