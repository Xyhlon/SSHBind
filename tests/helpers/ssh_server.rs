use async_trait::async_trait;
use libreauth::oath::TOTPBuilder;
use log::{error, info};
use russh::keys::PublicKey;
use russh::server::{Auth, ChannelOpenHandle, Handler, Response, Server, Session};
use russh::{Channel, ChannelOpenFailure};
use std::borrow::Cow;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpStream;

#[derive(Clone, Debug)]
pub struct Credentials {
    pub password: String,
    pub require_2fa: bool,
    pub two_factor_code: Option<String>,
    pub allowed_pubkey_base64: Option<String>,
}

impl From<sshbind::Creds> for Credentials {
    fn from(creds: sshbind::Creds) -> Self {
        Credentials {
            password: creds.password,
            require_2fa: creds.totp_key.is_some(),
            two_factor_code: creds.totp_key,
            allowed_pubkey_base64: None,
        }
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

pub fn server_config() -> Arc<russh::server::Config> {
    let mut config = russh::server::Config::default();
    let mut rng = rand::rng();

    #[cfg(unix)]
    config.keys.push(
        russh::keys::PrivateKey::random(&mut rng, russh::keys::Algorithm::Ed25519)
            .expect("generate test host key"),
    );

    #[cfg(windows)]
    {
        use russh::keys::ssh_key::private::{KeypairData, RsaKeypair};
        let keypair = KeypairData::from(RsaKeypair::random(&mut rng, 1024).unwrap());
        config
            .keys
            .push(russh::keys::PrivateKey::new(keypair, "").unwrap());
    }

    Arc::new(config)
}

impl Server for SSHServer {
    type Handler = SSHServer;

    fn new_client(&mut self, _peer_addr: Option<SocketAddr>) -> Self::Handler {
        self.clone()
    }

    fn handle_session_error(&mut self, error: <Self::Handler as Handler>::Error) {
        eprintln!("Session error: {error}");
    }
}

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
            info!("auth_password: user={user} password={password}");
            if let Some(cred) = self.users.get(user) {
                if cred.password == password {
                    if cred.require_2fa {
                        info!("Password valid but 2FA required for user {user}");
                        return Ok(Auth::Partial {
                            name: "".into(),
                            instructions: "2FA required".into(),
                            prompts: vec![(Cow::from("Enter 2FA code: "), false)].into(),
                        });
                    }
                    info!("Password authentication accepted for user {user}");
                    return Ok(Auth::Accept);
                }
            }
            error!("Password authentication rejected for user: {user}");
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
            info!("auth_publickey: user={user}");
            if let Some(cred) = self.users.get(user) {
                if let Some(ref allowed) = cred.allowed_pubkey_base64 {
                    if allowed == &public_key.to_string() {
                        if cred.require_2fa {
                            info!("Public key valid but 2FA required for user {user}");
                            return Ok(Auth::Partial {
                                name: "".into(),
                                instructions: "2FA required".into(),
                                prompts: vec![(Cow::from("Enter 2FA code: "), false)].into(),
                            });
                        }
                        info!("Public key authentication accepted for user {user}");
                        return Ok(Auth::Accept);
                    }
                }
            }
            error!("Public key authentication rejected for user: {user}");
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
            info!("auth_keyboard_interactive: user={user} submethods={submethods}");
            if let Some(cred) = self.users.get(user) {
                if !cred.require_2fa {
                    return Ok(Auth::Accept);
                }
                if response.is_none() {
                    info!("2FA required for user {user}");
                    return Ok(Auth::Partial {
                        name: "".into(),
                        instructions: "2FA required".into(),
                        prompts: vec![
                            (Cow::from("Password: "), false),
                            (Cow::from("Enter 2FA code: "), false),
                        ]
                        .into(),
                    });
                }

                let responses: Vec<String> = response
                    .into_iter()
                    .flatten()
                    .filter_map(|bytes| String::from_utf8(bytes.to_vec()).ok())
                    .collect();
                let password = responses
                    .first()
                    .map(|value| value.trim().to_string())
                    .unwrap_or_default();
                let otp_code = responses
                    .get(1)
                    .map(|value| value.trim().to_string())
                    .unwrap_or_default();
                let reference_code = match cred.two_factor_code.as_ref() {
                    Some(key) => TOTPBuilder::new().base32_key(key).finalize()?.generate(),
                    None => String::new(),
                };
                if password == cred.password && otp_code == reference_code {
                    info!("2FA accepted for user {user}");
                    return Ok(Auth::Accept);
                }
                return Ok(Auth::Reject {
                    proceed_with_methods: None,
                    partial_success: false,
                });
            }

            if response.is_none() {
                info!("2FA required for user {user}");
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
            }

            let responses: Vec<String> = response
                .into_iter()
                .flatten()
                .filter_map(|bytes| String::from_utf8(bytes.to_vec()).ok())
                .collect();
            let username = responses
                .first()
                .map(|value| value.trim().to_string())
                .unwrap_or_default();
            let password = responses
                .get(1)
                .map(|value| value.trim().to_string())
                .unwrap_or_default();
            let otp_code = responses
                .get(2)
                .map(|value| value.trim().to_string())
                .unwrap_or_default();
            if let Some(creds) = self.users.get(&username) {
                let reference_code = match creds.two_factor_code.as_ref() {
                    Some(key) => TOTPBuilder::new().base32_key(key).finalize()?.generate(),
                    None => String::new(),
                };
                if password == creds.password && otp_code == reference_code {
                    info!("2FA accepted for user {user}");
                    return Ok(Auth::Accept);
                }
            }
            error!("User {user} not found in keyboard interactive auth");
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
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        async move {
            info!("Session channel opened: {channel:?}");
            reply.accept().await;
            Ok(())
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
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        async move {
            info!("Direct TCP/IP channel request to {host_to_connect}:{port_to_connect}");
            if host_to_connect != "127.0.0.1" && host_to_connect != "localhost" {
                error!("Rejected direct TCP/IP channel: target {host_to_connect} not allowed");
                reply
                    .reject(ChannelOpenFailure::AdministrativelyProhibited)
                    .await;
                return Ok(());
            }
            let port = port_to_connect as u16;
            match TcpStream::connect((host_to_connect, port)).await {
                Ok(mut target_stream) => {
                    reply.accept().await;
                    info!("Channel confirmed");
                    let mut channel_stream = channel.into_stream();
                    tokio::spawn(async move {
                        if let Err(error) =
                            tokio::io::copy_bidirectional(&mut target_stream, &mut channel_stream)
                                .await
                        {
                            error!("Forwarding error: {error}");
                        }
                    });
                    Ok(())
                }
                Err(error) => {
                    error!("Failed to connect to target {host_to_connect}:{port} - {error}");
                    reply.reject(ChannelOpenFailure::ConnectFailed).await;
                    Ok(())
                }
            }
        }
    }
}
