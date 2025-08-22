use serde::{Deserialize, Serialize};
use ssh2::{KeyboardInteractivePrompt, Prompt};
use std::collections::BTreeMap;
use std::convert::TryFrom;
use std::error::Error;
use std::fmt;
use std::net::{SocketAddr, ToSocketAddrs};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use libreauth::oath::TOTPBuilder;

/// Host and port combination
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HostPort {
    pub host: String,
    pub port: u16,
}

impl HostPort {
    pub fn new(host: String, port: u16) -> Self {
        Self { host, port }
    }

    pub fn to_socket_addr(&self) -> Result<SocketAddr, Box<dyn Error + Send + Sync>> {
        format!("{}:{}", self.host, self.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| format!("Failed to resolve {}:{}", self.host, self.port).into())
    }
}

impl fmt::Display for HostPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

impl FromStr for HostPort {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid host:port format: {}", s));
        }

        let host = parts[0].to_string();
        let port = parts[1]
            .parse::<u16>()
            .map_err(|_| format!("Invalid port number: {}", parts[1]))?;

        Ok(HostPort { host, port })
    }
}

impl TryFrom<&str> for HostPort {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

/// TOTP prompt handler for keyboard interactive authentication
pub struct TotpPromptHandler {
    pub password: String,
    pub totp_key: Option<String>,
}

impl KeyboardInteractivePrompt for TotpPromptHandler {
    fn prompt<'a>(
        &mut self,
        _username: &str,
        _instructions: &str,
        prompts: &[Prompt<'a>],
    ) -> Vec<String> {
        prompts
            .iter()
            .map(|prompt| {
                let prompt_text = prompt.text.to_lowercase();
                if prompt_text.contains("password") {
                    self.password.clone()
                } else if prompt_text.contains("verification")
                    || prompt_text.contains("code")
                    || prompt_text.contains("token")
                    || prompt_text.contains("otp")
                {
                    if let Some(ref key) = self.totp_key {
                        generate_totp_code(key).unwrap_or_else(|_| String::new())
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                }
            })
            .collect()
    }
}

/// Generates TOTP code from base32 key
fn generate_totp_code(base32_key: &str) -> Result<String, Box<dyn Error + Send + Sync>> {
    let totp = TOTPBuilder::new()
        .base32_key(base32_key)
        .finalize()?;
    Ok(totp.generate())
}

/// Credentials structure for YAML deserialization
#[derive(Serialize, Deserialize, Debug)]
pub struct Creds {
    pub username: String,
    pub password: String,
    pub totp_key: Option<String>,
}

/// YAML credentials mapping type
pub type YamlCreds = BTreeMap<String, Creds>;

/// Cancellation token for async operations
pub struct CancellationToken {
    cancelled: Arc<Mutex<bool>>,
}

impl CancellationToken {
    pub fn new() -> Self {
        CancellationToken {
            cancelled: Arc::new(Mutex::new(false)),
        }
    }

    pub fn clone(&self) -> Self {
        CancellationToken {
            cancelled: self.cancelled.clone(),
        }
    }

    pub fn cancel(&self) {
        let mut cancelled = self.cancelled.lock().unwrap();
        *cancelled = true;
    }

    pub async fn cancelled(&self) -> bool {
        loop {
            {
                let cancelled = self.cancelled.lock().unwrap();
                if *cancelled {
                    return true;
                }
            }
            crate::executor::yield_now().await;
        }
    }
}