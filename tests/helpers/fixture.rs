use super::ssh_server::{server_config, Credentials, SSHServer};
use russh::server::Server;
use sshbind::{Creds, YamlCreds};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub type TestResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const IO_TIMEOUT: Duration = Duration::from_secs(10);
const BIND_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const BIND_ATTEMPTS: usize = 3;
const PAYLOAD: &[u8] = b"hello world!";
const AGE_IDENTITY: &str =
    "AGE-SECRET-KEY-1D8JAD8SXNFVQEFHAUNNAX4QCE3K5CUKMT7YYHNGTUSSP97YGWL4STV89UH";
const AGE_RECIPIENT: &str = "age1uc8zlurjyjpenrslc2thyl28u7ylz6x8c2g9yphvjha6xm8ppf3slq0l25";

pub struct RigOptions {
    pub jump_host_count: usize,
    pub totp_key: Option<String>,
    pub rejecting_host: Option<usize>,
}

impl RigOptions {
    pub fn password_chain(jump_host_count: usize) -> Self {
        Self {
            jump_host_count,
            totp_key: None,
            rejecting_host: None,
        }
    }

    pub fn totp() -> Self {
        Self {
            jump_host_count: 1,
            totp_key: Some("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ".into()),
            rejecting_host: None,
        }
    }

    pub fn rejecting_second_host() -> Self {
        Self {
            jump_host_count: 2,
            totp_key: None,
            rejecting_host: Some(1),
        }
    }
}

struct SopsFixture {
    _temp_dir: TempDir,
    secrets_path: PathBuf,
    previous_age_key_file: Option<OsString>,
}

impl SopsFixture {
    fn new(creds: &YamlCreds) -> TestResult<Self> {
        let temp_dir = tempfile::tempdir()?;
        let secrets_path = temp_dir.path().join("secrets.yaml");
        let key_path = temp_dir.path().join("age_key.txt");
        let config_path = temp_dir.path().join(".sops.yaml");

        fs::write(&key_path, AGE_IDENTITY)?;
        fs::write(
            &config_path,
            format!(
                "keys:\n  - &master {AGE_RECIPIENT}\ncreation_rules:\n  - path_regex: secrets.yaml$\n    key_groups:\n    - age:\n      - *master"
            ),
        )?;
        fs::write(&secrets_path, serde_yaml_ng::to_string(creds)?)?;

        let output = Command::new("sops")
            .current_dir(temp_dir.path())
            .env("SOPS_AGE_KEY_FILE", &key_path)
            .args(["encrypt", "--in-place"])
            .arg(&secrets_path)
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "sops encryption failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        let previous_age_key_file = std::env::var_os("SOPS_AGE_KEY_FILE");
        std::env::set_var("SOPS_AGE_KEY_FILE", &key_path);

        Ok(Self {
            _temp_dir: temp_dir,
            secrets_path,
            previous_age_key_file,
        })
    }

    fn path(&self) -> &Path {
        &self.secrets_path
    }
}

impl Drop for SopsFixture {
    fn drop(&mut self) {
        if let Some(value) = &self.previous_age_key_file {
            std::env::set_var("SOPS_AGE_KEY_FILE", value);
        } else {
            std::env::remove_var("SOPS_AGE_KEY_FILE");
        }
    }
}

pub struct SshTestRig {
    bind_addr: String,
    jump_hosts: Vec<String>,
    service_addr: String,
    sops: SopsFixture,
    tasks: Vec<tokio::task::AbortHandle>,
    binding_started: bool,
    expect_payload: bool,
    closed_payload: Option<Vec<u8>>,
}

impl SshTestRig {
    pub async fn start(options: RigOptions) -> TestResult<Self> {
        assert!(options.jump_host_count > 0, "a test rig needs a jump host");

        let bind_addr = unused_loopback_address()?;
        let expect_payload = options.rejecting_host.is_none();
        let mut ssh_listeners = Vec::with_capacity(options.jump_host_count);
        for _ in 0..options.jump_host_count {
            ssh_listeners.push(TcpListener::bind("127.0.0.1:0").await?);
        }
        let jump_hosts = ssh_listeners
            .iter()
            .map(|listener| listener.local_addr().map(|address| address.to_string()))
            .collect::<Result<Vec<_>, _>>()?;

        let service_listener = TcpListener::bind("127.0.0.1:0").await?;
        let service_addr = service_listener.local_addr()?.to_string();
        let service_task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = service_listener.accept().await {
                if socket.write_all(PAYLOAD).await.is_err() {
                    break;
                }
            }
        });
        let mut tasks = vec![service_task.abort_handle()];

        let mut creds = YamlCreds::new();
        for jump_host in &jump_hosts {
            creds.insert(
                jump_host.clone(),
                Creds {
                    username: "pi".into(),
                    password: "max".into(),
                    totp_key: options.totp_key.clone(),
                },
            );
        }
        let sops = SopsFixture::new(&creds)?;

        let config = server_config();
        for (index, (jump_host, listener)) in jump_hosts.iter().zip(ssh_listeners).enumerate() {
            let users = if options.rejecting_host == Some(index) {
                HashMap::new()
            } else {
                let cred = creds[jump_host].clone();
                HashMap::from([(cred.username.clone(), Credentials::from(cred))])
            };
            let mut server = SSHServer::new(Some(users));
            let server_config = config.clone();
            let address = jump_host.clone();
            let task = tokio::spawn(async move {
                if let Err(error) = server.run_on_socket(server_config, &listener).await {
                    eprintln!("mock SSH server at {address} stopped: {error}");
                }
            });
            tasks.push(task.abort_handle());
        }

        Ok(Self {
            bind_addr,
            jump_hosts,
            service_addr,
            sops,
            tasks,
            binding_started: false,
            expect_payload,
            closed_payload: None,
        })
    }

    pub fn occupy_initial_bind_address(&self) -> std::io::Result<tokio::task::JoinHandle<()>> {
        let listener = std::net::TcpListener::bind(&self.bind_addr)?;
        listener.set_nonblocking(true)?;
        let listener = TcpListener::from_std(listener)?;
        Ok(tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                if stream
                    .write_all(b"occupied by another service")
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }))
    }

    pub async fn start_binding(&mut self) -> TestResult<()> {
        for attempt in 1..=BIND_ATTEMPTS {
            sshbind::bind(
                &self.bind_addr,
                self.jump_hosts.clone(),
                Some(self.service_addr.clone()),
                self.sops.path().to_str().expect("UTF-8 temporary path"),
                None,
            );
            self.binding_started = true;

            let ready = if self.expect_payload {
                matches!(
                    read_payload_from(&self.bind_addr, BIND_PROBE_TIMEOUT).await,
                    Ok(payload) if payload == PAYLOAD
                )
            } else {
                match read_until_closed_from(&self.bind_addr, BIND_PROBE_TIMEOUT).await {
                    Ok(payload) if payload.is_empty() => {
                        self.closed_payload = Some(payload);
                        true
                    }
                    _ => false,
                }
            };
            if ready {
                return Ok(());
            }

            sshbind::unbind(&self.bind_addr);
            self.binding_started = false;
            if attempt < BIND_ATTEMPTS {
                self.bind_addr = unused_loopback_address()?;
            }
        }

        Err(format!("failed to bind a test listener after {BIND_ATTEMPTS} attempts").into())
    }

    pub async fn read_payload(&self) -> TestResult<Vec<u8>> {
        read_payload_from(&self.bind_addr, IO_TIMEOUT).await
    }

    pub async fn read_until_closed(&mut self) -> TestResult<Vec<u8>> {
        if let Some(payload) = self.closed_payload.take() {
            return Ok(payload);
        }
        read_until_closed_from(&self.bind_addr, IO_TIMEOUT).await
    }
}

impl Drop for SshTestRig {
    fn drop(&mut self) {
        if self.binding_started {
            sshbind::unbind(&self.bind_addr);
            self.binding_started = false;
        }
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

fn unused_loopback_address() -> std::io::Result<String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.local_addr().map(|address| address.to_string())
}

async fn read_payload_from(address: &str, timeout: Duration) -> TestResult<Vec<u8>> {
    let address = address.to_string();
    let connect_address = address.clone();
    tokio::time::timeout(timeout, async move {
        let mut stream = TcpStream::connect(&connect_address).await?;
        let mut payload = Vec::new();
        stream.read_to_end(&mut payload).await?;
        Ok::<_, std::io::Error>(payload)
    })
    .await
    .map_err(|_| format!("timed out reading payload from {address}"))?
    .map_err(Into::into)
}

async fn read_until_closed_from(address: &str, timeout: Duration) -> TestResult<Vec<u8>> {
    let address = address.to_string();
    let connect_address = address.clone();
    tokio::time::timeout(timeout, async move {
        let mut stream = TcpStream::connect(&connect_address).await?;
        let mut payload = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            match stream.read(&mut buffer).await {
                Ok(0) => return Ok(payload),
                Ok(count) => payload.extend_from_slice(&buffer[..count]),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::BrokenPipe
                            | std::io::ErrorKind::UnexpectedEof
                    ) =>
                {
                    return Ok(payload);
                }
                Err(error) => return Err(error),
            }
        }
    })
    .await
    .map_err(|_| format!("timed out waiting for {address} to close"))?
    .map_err(Into::into)
}
