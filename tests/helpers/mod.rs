use age::x25519;
use libreauth::oath::TOTPBuilder;
use secrecy::ExposeSecret;
use sshbind::YamlCreds;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

pub fn setup_sopsfile(testcreds: YamlCreds) -> TempDir {
    let binding = std::env::current_dir().unwrap();
    let wd = binding.as_path();
    let tmp_dir = TempDir::new_in(wd).expect("Failed to create temp dir");
    info!("Temp dir: {:?}", tmp_dir.path());
    let file_path = tmp_dir.path().join("secrets.yaml");

    // Generate a new age key pair.
    let identity = x25519::Identity::generate();
    let public_key = identity.to_public();

    // Define file paths within the temporary directory.
    let key_path: PathBuf = tmp_dir.path().join("age_key.txt");
    let config_path: PathBuf = tmp_dir.path().join(".sops.yaml");

    // Write the private key to our temporary file.
    fs::write(&key_path, identity.to_string().expose_secret()).expect("Failed to write key");

    // Create a minimal SOPS configuration file.
    // Here we specify that for any YAML file, sops should use the given age public key.
    let config_content = format!("keys:\n  - &master {}\ncreation_rules:\n  - path_regex: secrets.yaml$\n    key_groups:\n    - age:\n      - *master", public_key);

    fs::write(&config_path, config_content).expect("Failed to write config");

    // Optionally, you can assert that the files exist in the temp dir.
    assert!(key_path.exists());
    assert!(config_path.exists());

    let stringified = serde_yml::to_string(&testcreds).expect("Failed to serialize");
    //
    // Write test configuration to the file
    fs::write(&file_path, stringified).expect("Failed to write to file");
    let path = file_path.to_str().unwrap();

    std::env::set_var("SOPS_AGE_KEY_FILE", key_path.to_str().unwrap());
    let _ = std::env::set_current_dir(tmp_dir.path());

    let output = Command::new("sops")
        .arg("encrypt")
        .arg(path) // user input as a separate argument
        .output()
        .expect("failed to execute process");

    let enc_content = String::from_utf8_lossy(&output.stdout).to_string();
    fs::write(&file_path, enc_content).expect("Failed to write to file");
    tmp_dir
}

use async_trait::async_trait;
use log::{error, info};
use russh::keys::PublicKey;
use russh::server::{Auth, Handler, Response, Server, Session};
use russh::Channel;
use std::borrow::Cow;
use std::collections::HashMap;
use tokio::net::TcpStream;

///
/// Credentials and SSHServer state.
///
#[derive(Clone, Debug)]
pub struct Credentials {
    pub password: String,
    pub require_2fa: bool,
    pub two_factor_code: Option<String>,
    pub allowed_pubkey_base64: Option<String>,
}

impl From<sshbind::Creds> for Credentials {
    fn from(creds: sshbind::Creds) -> Self {
        if let Some(ref totp_key) = creds.totp_key {
            #[allow(clippy::needless_return)]
            return Credentials {
                password: creds.password.clone(),
                require_2fa: true,
                two_factor_code: Some(totp_key.clone()),
                allowed_pubkey_base64: None,
            };
        } else {
            #[allow(clippy::needless_return)]
            return Credentials {
                password: creds.password.clone(),
                require_2fa: false,
                two_factor_code: None,
                allowed_pubkey_base64: None,
            };
        };
    }
}

#[derive(Clone)]
pub struct SSHServer {
    pub users: HashMap<String, Credentials>,
}

impl SSHServer {
    pub fn new(users: Option<HashMap<String, Credentials>>) -> Self {
        if let Some(users) = users {
            SSHServer { users }
        } else {
            let mut users = HashMap::new();
            users.insert(
                "test".to_string(),
                Credentials {
                    password: "password".to_string(),
                    require_2fa: true,
                    two_factor_code: Some("123456".to_string()),
                    allowed_pubkey_base64: None,
                },
            );
            SSHServer { users }
        }
    }
}

impl Server for SSHServer {
    type Handler = SSHServer;

    // For each new client, we simply return a clone of our handler.
    fn new_client(&mut self, _peer_addr: Option<SocketAddr>) -> Self::Handler {
        self.clone()
    }

    // Log any session errors.
    fn handle_session_error(&mut self, error: <Self::Handler as Handler>::Error) {
        eprintln!("Session error: {}", error);
    }
}

///
/// Handler implementation for SSHServer.
///
#[async_trait]
impl Handler for SSHServer {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    #[allow(clippy::manual_async_fn)]
    fn auth_password(
        &mut self,
        user: &str,
        password: &str,
    ) -> impl std::future::Future<Output = Result<Auth, Self::Error>> + Send {
        async move {
            info!("auth_password: user={} password={}", user, password);
            if let Some(cred) = self.users.get(user) {
                if cred.password == password {
                    if cred.require_2fa {
                        info!("Password valid but 2FA required for user {}", user);
                        return Ok(Auth::Partial {
                            name: "".into(),
                            instructions: "2FA required".into(),
                            prompts: vec![(Cow::from("Enter 2FA code: "), false)].into(),
                        });
                    }
                    info!("Password authentication accepted for user {}", user);
                    return Ok(Auth::Accept);
                }
            }
            error!("Password authentication rejected for user: {}", user);
            Ok(Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            })
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn auth_publickey(
        &mut self,
        user: &str,
        public_key: &PublicKey,
    ) -> impl std::future::Future<Output = Result<Auth, Self::Error>> + Send {
        async move {
            info!("auth_publickey: user={}", user);
            if let Some(cred) = self.users.get(user) {
                if let Some(ref allowed) = cred.allowed_pubkey_base64 {
                    if allowed == &public_key.to_string() {
                        if cred.require_2fa {
                            info!("Public key valid but 2FA required for user {}", user);
                            return Ok(Auth::Partial {
                                name: "".into(),
                                instructions: "2FA required".into(),
                                prompts: vec![(Cow::from("Enter 2FA code: "), false)].into(),
                            });
                        }
                        info!("Public key authentication accepted for user {}", user);
                        return Ok(Auth::Accept);
                    }
                }
            }
            error!("Public key authentication rejected for user: {}", user);
            Ok(Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            })
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        submethods: &str,
        response: Option<Response<'a>>,
    ) -> impl std::future::Future<Output = Result<Auth, Self::Error>> + Send {
        async move {
            info!(
                "auth_keyboard_interactive: user={} submethods={}",
                user, submethods
            );
            if let Some(cred) = self.users.get(user) {
                if !cred.require_2fa {
                    return Ok(Auth::Accept);
                }
                if response.is_none() {
                    info!("2FA required for user {}", user);
                    return Ok(Auth::Partial {
                        name: "".into(),
                        instructions: "2FA required".into(),
                        prompts: vec![
                            (Cow::from("Password: "), false),
                            (Cow::from("Enter 2FA code: "), false),
                        ]
                        .into(),
                    });
                } else {
                    info!("Else");
                    let responses: Vec<String> = response
                        .unwrap()
                        .filter_map(|b| String::from_utf8(b.to_vec()).ok())
                        .collect();
                    info!("2FA response: {:?}", responses);

                    let password = responses
                        .first()
                        .map(|s| s.trim().to_string())
                        .unwrap_or_default();
                    let otp_code = responses
                        .get(1)
                        .map(|s| s.trim().to_string())
                        .unwrap_or_default();

                    let ref_code = match cred.two_factor_code.as_ref() {
                        Some(key) => TOTPBuilder::new().base32_key(key).finalize()?.generate(),
                        None => "".to_string(),
                    };
                    if password != cred.password {
                        error!("Invalid password provided by user {}", user);
                        return Ok(Auth::Reject {
                            proceed_with_methods: None,
                            partial_success: false,
                        });
                    }
                    if otp_code != ref_code {
                        error!("Invalid 2FA code provided by user {}", user);
                        return Ok(Auth::Reject {
                            proceed_with_methods: None,
                            partial_success: false,
                        });
                    }

                    info!("2FA accepted for user {}", user);
                    return Ok(Auth::Accept);
                }
            } else if response.is_none() {
                info!("2FA required for user {}", user);
                return Ok(Auth::Partial {
                    name: "".into(),
                    instructions: "2FA required".into(),
                    prompts: vec![
                        (Cow::from("Username: "), true),
                        (Cow::from("Password: "), false),
                        (Cow::from("Enter 2FA code: "), false),
                    ]
                    .into(),
                });
            } else {
                let responses: Vec<String> = response
                    .unwrap()
                    .filter_map(|b| String::from_utf8(b.to_vec()).ok())
                    .collect();
                info!("2FA response: {:?}", responses);

                let username = responses
                    .first()
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                let password = responses
                    .get(1)
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                let otp_code = responses
                    .get(2)
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                if let Some(creds) = self.users.get(&username) {
                    let ref_code = match creds.two_factor_code.as_ref() {
                        Some(key) => TOTPBuilder::new().base32_key(key).finalize()?.generate(),
                        None => "".to_string(),
                    };
                    if password != creds.password {
                        error!("Invalid password provided by user {}", user);
                        return Ok(Auth::Reject {
                            proceed_with_methods: None,
                            partial_success: false,
                        });
                    }
                    if otp_code != ref_code {
                        error!("Invalid 2FA code provided by user {}", user);
                        return Ok(Auth::Reject {
                            proceed_with_methods: None,
                            partial_success: false,
                        });
                    }
                    info!("2FA accepted for user {}", user);
                    return Ok(Auth::Accept);
                }
            }
            error!("User {} not found in keyboard interactive auth", user);
            Ok(Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            })
        }
    }

    #[allow(unused_variables, clippy::manual_async_fn)]
    fn channel_open_session(
        &mut self,
        channel: Channel<russh::server::Msg>,
        session: &mut Session,
    ) -> impl std::future::Future<Output = Result<bool, Self::Error>> + Send {
        async move {
            info!("Session channel opened: {:?}", channel);
            Ok(true)
        }
    }

    #[allow(unused_mut, clippy::manual_async_fn)]
    fn channel_open_direct_tcpip(
        &mut self,
        mut channel: Channel<russh::server::Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        session: &mut Session,
    ) -> impl std::future::Future<Output = Result<bool, Self::Error>> + Send {
        async move {
            info!(
                "Direct TCP/IP channel request to {}:{}",
                host_to_connect, port_to_connect
            );
            if host_to_connect != "127.0.0.1" && host_to_connect != "localhost" {
                error!(
                    "Rejected direct TCP/IP channel: target {} not allowed",
                    host_to_connect
                );
                return Ok(false);
            }
            let port: u16 = port_to_connect as u16;
            match TcpStream::connect((host_to_connect, port)).await {
                Ok(mut target_stream) => {
                    // Signal that the channel connection was successful.
                    session.channel_success(channel.id())?;
                    info!("Channel confirmed");
                    // use russh::channels::channel_stream::ChannelStream;
                    // let mut chan_stream = ChannelStream::new(channel);
                    let mut chan_stream = channel.into_stream();

                    // Spawn a task to relay data between the channel and target stream.
                    tokio::spawn(async move {
                        if let Err(e) =
                            tokio::io::copy_bidirectional(&mut target_stream, &mut chan_stream)
                                .await
                        {
                            error!("Forwarding error: {}", e);
                        }
                    });
                    Ok(true)
                }
                Err(e) => {
                    error!(
                        "Failed to connect to target {}:{} - {}",
                        host_to_connect, port, e
                    );
                    Ok(false)
                }
            }
        }
    }
}
