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
        // Handle IPv6 addresses in brackets [::1]:22
        if s.starts_with('[') {
            if let Some(bracket_end) = s.find("]:") {
                let host = s[1..bracket_end].to_string();
                let port_str = &s[bracket_end + 2..];
                let port = port_str
                    .parse::<u16>()
                    .map_err(|_| format!("Invalid port number: {}", port_str))?;
                return Ok(HostPort { host, port });
            } else {
                return Err(format!("Invalid IPv6 host:port format: {}", s));
            }
        }

        // Handle regular host:port
        let parts: Vec<&str> = s.rsplitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid host:port format: {}", s));
        }

        let port_str = parts[0];
        let host = parts[1].to_string();
        
        // Validate that we have both host and port
        if host.is_empty() || port_str.is_empty() {
            return Err(format!("Invalid host:port format: {}", s));
        }

        let port = port_str
            .parse::<u16>()
            .map_err(|_| format!("Invalid port number: {}", port_str))?;

        Ok(HostPort { host, port })
    }
}

impl TryFrom<&str> for HostPort {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl Into<SocketAddr> for HostPort {
    fn into(self) -> SocketAddr {
        self.to_socket_addr().expect("Failed to convert HostPort to SocketAddr")
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
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Creds {
    pub username: String,
    pub password: String,
    pub totp_key: Option<String>,
}

/// YAML credentials mapping type
pub type YamlCreds = BTreeMap<String, Creds>;

/// Cancellation token for async operations
pub struct CancellationToken {
    inner: Arc<CancellationTokenInner>,
}

struct CancellationTokenInner {
    cancelled: Mutex<bool>,
    notify: std::sync::Condvar,
}

impl CancellationToken {
    pub fn new() -> Self {
        CancellationToken {
            inner: Arc::new(CancellationTokenInner {
                cancelled: Mutex::new(false),
                notify: std::sync::Condvar::new(),
            }),
        }
    }

    pub fn clone(&self) -> Self {
        CancellationToken {
            inner: self.inner.clone(),
        }
    }

    pub fn cancel(&self) {
        match self.inner.cancelled.lock() {
            Ok(mut cancelled) => {
                *cancelled = true;
                self.inner.notify.notify_all();
            }
            Err(_) => {
                // Poisoned lock, but we still want to notify
                self.inner.notify.notify_all();
            }
        }
    }

    pub async fn cancelled(&self) -> bool {
        // For async compatibility, we check the state and return immediately
        // In a real async runtime, this would be implemented differently
        match self.inner.cancelled.lock() {
            Ok(cancelled) => *cancelled,
            Err(_) => false, // On poison, assume not cancelled
        }
    }

    /// Blocking wait for cancellation (for synchronous contexts)
    pub fn wait_cancelled(&self) {
        if let Ok(guard) = self.inner.cancelled.lock() {
            let _result = self.inner.notify.wait_while(guard, |cancelled| !*cancelled);
        }
    }

    /// Check if cancelled without blocking
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.lock().map_or(false, |cancelled| *cancelled)
    }
}