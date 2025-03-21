use env_logger::Builder;
use log::LevelFilter;
use russh::server::Server;
use serial_test::serial;
use sshbind::{bind, unbind};
use sshbind::{Creds, YamlCreds};
use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, LazyLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task;

mod helpers;

// Ensure the logger is initialized only once
static LOGGER: LazyLock<()> = LazyLock::new(|| {
    Builder::new()
        .filter(None, LevelFilter::Info) // Adjust log level as needed
        .format(|buf, record| writeln!(buf, "[{}] - {}", record.level(), record.args()))
        .init();
});

#[test]
fn fail_not_path() {
    #[allow(clippy::let_unit_value)]
    let _ = *LOGGER; // Ensure logger is initialized
    let bind_addr = "127.0.0.1:8000";
    let jump_hosts = vec!["127.0.0.0:20".to_string()];
    let service_addr = "127.0.0.0:80";

    bind(bind_addr, jump_hosts, service_addr, "aq^fasdfs*$%");
    unbind(bind_addr);
}

#[tokio::test]
#[serial]
async fn serial_integration_test_correct_configuration() -> Result<(), Box<dyn std::error::Error>> {
    #[allow(clippy::let_unit_value)]
    let _ = *LOGGER; // Ensure logger is initialized
    let mut testcreds = YamlCreds::new();
    testcreds.insert(
        "127.0.0.1:2222".to_string(),
        Creds {
            username: "pi".to_string(),
            password: "max".to_string(),
            totp_key: None,
        },
    );
    testcreds.insert(
        "127.0.0.1:2323".to_string(),
        Creds {
            username: "pi".to_string(),
            password: "max".to_string(),
            totp_key: None,
        },
    );
    testcreds.insert(
        "httpforever.com".to_string(),
        Creds {
            username: "pi".to_string(),
            password: "max".to_string(),
            totp_key: "ABCAD37A".to_string().into(),
        },
    );
    let tmp_dir = helpers::setup_sopsfile(testcreds.clone());
    let sopsfile_path = tmp_dir.path().join("secrets.yaml");
    let mut config = russh::server::Config::default();

    // Use OsRng from rand for randomness.
    use russh::keys::ssh_key::rand_core::OsRng;
    let mut rng = OsRng;
    use russh::keys::Algorithm;
    let pk = russh::keys::PrivateKey::random(&mut rng, Algorithm::Ed25519).unwrap();
    config.keys.push(pk);
    let config = Arc::new(config);
    use helpers::Credentials;
    let mut hosts_users: HashMap<String, HashMap<String, Credentials>> = HashMap::new();
    testcreds.iter().for_each(|(k, v)| {
        let mut map: HashMap<String, Credentials> = HashMap::new();
        map.insert(v.username.clone(), Credentials::from(v.clone()));
        hosts_users.insert(k.to_string(), map);
    });

    let bind_addr = "127.0.0.1:7000";
    let jump_hosts = vec!["127.0.0.1:2222".to_string(), "127.0.0.1:2323".to_string()];
    let service_addr = "127.0.0.1:8080";

    let ssh_tasks: Vec<_> = jump_hosts
        .clone()
        .into_iter()
        .map(|ssh_addr| {
            let cloned_config = config.clone();
            let users = hosts_users.get(&ssh_addr).cloned().unwrap_or_default();
            let mut server = helpers::SSHServer::new(Some(users));

            task::spawn(async move {
                let _ = server.run_on_address(cloned_config, ssh_addr.clone()).await;
            })
        })
        .collect();
    let service_handle = task::spawn(async move {
        let serv = TcpListener::bind(service_addr).await.unwrap();
        loop {
            let (mut socket, _) = serv.accept().await.unwrap();
            socket.write_all(b"hello world!").await.unwrap();
        }
    });

    bind(
        bind_addr,
        jump_hosts,
        service_addr,
        sopsfile_path.to_str().unwrap(),
    );

    let mut conn = TcpStream::connect(bind_addr).await.unwrap();
    let mut buf = vec![0; 1024];
    let n = conn.read(&mut buf).await?;
    let response = String::from_utf8_lossy(&buf[..n]).to_string();
    println!("Received: {}", response);
    assert_eq!(response, "hello world!");

    unbind(bind_addr);
    for task in ssh_tasks {
        task.abort();
    }
    service_handle.abort();
    Ok(())
}

#[tokio::test]
#[serial]
async fn serial_integration_test_correct_configuration_multiple(
) -> Result<(), Box<dyn std::error::Error>> {
    #[allow(clippy::let_unit_value)]
    let _ = *LOGGER; // Ensure logger is initialized
    let mut testcreds = YamlCreds::new();
    testcreds.insert(
        "127.0.0.1:2222".to_string(),
        Creds {
            username: "pi".to_string(),
            password: "max".to_string(),
            totp_key: None,
        },
    );
    testcreds.insert(
        "127.0.0.1:2323".to_string(),
        Creds {
            username: "pi".to_string(),
            password: "max".to_string(),
            totp_key: None,
        },
    );
    testcreds.insert(
        "httpforever.com".to_string(),
        Creds {
            username: "pi".to_string(),
            password: "max".to_string(),
            totp_key: "ABCAD37A".to_string().into(),
        },
    );
    let tmp_dir = helpers::setup_sopsfile(testcreds.clone());
    let sopsfile_path = tmp_dir.path().join("secrets.yaml");
    let mut config = russh::server::Config::default();

    // Use OsRng from rand for randomness.
    use russh::keys::ssh_key::rand_core::OsRng;
    let mut rng = OsRng;
    use russh::keys::Algorithm;
    let pk = russh::keys::PrivateKey::random(&mut rng, Algorithm::Ed25519).unwrap();
    config.keys.push(pk);
    let config = Arc::new(config);
    use helpers::Credentials;
    let mut hosts_users: HashMap<String, HashMap<String, Credentials>> = HashMap::new();
    testcreds.iter().for_each(|(k, v)| {
        let mut map: HashMap<String, Credentials> = HashMap::new();
        map.insert(v.username.clone(), Credentials::from(v.clone()));
        hosts_users.insert(k.to_string(), map);
    });

    let bind_addr = "127.0.0.1:7000";
    let jump_hosts = vec!["127.0.0.1:2222".to_string(), "127.0.0.1:2323".to_string()];
    let service_addr = "127.0.0.1:8080";

    let ssh_tasks: Vec<_> = jump_hosts
        .clone()
        .into_iter()
        .map(|ssh_addr| {
            let cloned_config = config.clone();
            let users = hosts_users.get(&ssh_addr).cloned().unwrap_or_default();
            let mut server = helpers::SSHServer::new(Some(users));

            task::spawn(async move {
                let _ = server.run_on_address(cloned_config, ssh_addr.clone()).await;
            })
        })
        .collect();
    let service_handle = task::spawn(async move {
        let serv = TcpListener::bind(service_addr).await.unwrap();
        loop {
            let (mut socket, _) = serv.accept().await.unwrap();
            socket.write_all(b"hello world!").await.unwrap();
        }
    });

    bind(
        bind_addr,
        jump_hosts,
        service_addr,
        sopsfile_path.to_str().unwrap(),
    );

    let mut conn = TcpStream::connect(bind_addr).await.unwrap();
    let mut buf = vec![0; 1024];
    let n = conn.read(&mut buf).await?;
    let response = String::from_utf8_lossy(&buf[..n]).to_string();
    println!("Received: {}", response);
    assert_eq!(response, "hello world!");

    let mut conn = TcpStream::connect(bind_addr).await.unwrap();
    let mut buf = vec![0; 1024];
    let n = conn.read(&mut buf).await?;
    let response = String::from_utf8_lossy(&buf[..n]).to_string();
    println!("Received: {}", response);
    assert_eq!(response, "hello world!");

    unbind(bind_addr);
    for task in ssh_tasks {
        task.abort();
    }
    service_handle.abort();
    Ok(())
}

#[tokio::test]
#[serial]
async fn serial_integration_test_second_server_wrong_credentials(
) -> Result<(), Box<dyn std::error::Error>> {
    #[allow(clippy::let_unit_value)]
    let _ = *LOGGER; // Ensure logger is initialized
    let mut testcreds = YamlCreds::new();
    testcreds.insert(
        "127.0.0.1:2222".to_string(),
        Creds {
            username: "pi".to_string(),
            password: "max".to_string(),
            totp_key: None,
        },
    );
    testcreds.insert(
        "127.0.0.1:2323".to_string(),
        Creds {
            username: "pi".to_string(),
            password: "max".to_string(),
            totp_key: None,
        },
    );
    testcreds.insert(
        "httpforever.com".to_string(),
        Creds {
            username: "pi".to_string(),
            password: "max".to_string(),
            totp_key: "ABCAD37A".to_string().into(),
        },
    );
    let tmp_dir = helpers::setup_sopsfile(testcreds.clone());
    let sopsfile_path = tmp_dir.path().join("secrets.yaml");
    let mut config = russh::server::Config::default();

    // Use OsRng from rand for randomness.
    use russh::keys::ssh_key::rand_core::OsRng;
    let mut rng = OsRng;
    use russh::keys::Algorithm;
    let pk = russh::keys::PrivateKey::random(&mut rng, Algorithm::Ed25519).unwrap();
    config.keys.push(pk);
    let config = Arc::new(config);
    use helpers::Credentials;
    let mut hosts_users: HashMap<String, HashMap<String, Credentials>> = HashMap::new();
    testcreds.iter().for_each(|(k, v)| {
        let mut map: HashMap<String, Credentials> = HashMap::new();
        map.insert(v.username.clone(), Credentials::from(v.clone()));
        hosts_users.insert(k.to_string(), map);
    });

    let bind_addr = "127.0.0.1:7000";
    let jump_hosts = vec!["127.0.0.1:2222".to_string(), "127.0.0.1:2323".to_string()];
    let service_addr = "127.0.0.1:8080";

    let ssh_tasks: Vec<_> = jump_hosts
        .clone()
        .into_iter()
        .map(|ssh_addr| {
            let cloned_config = config.clone();
            let users = hosts_users.get(&ssh_addr).cloned().unwrap_or_default();
            hosts_users.clear();
            let mut server = helpers::SSHServer::new(Some(users));

            task::spawn(async move {
                let _ = server.run_on_address(cloned_config, ssh_addr.clone()).await;
            })
        })
        .collect();
    let service_handle = task::spawn(async move {
        let serv = TcpListener::bind(service_addr).await.unwrap();
        loop {
            let (mut socket, _) = serv.accept().await.unwrap();
            socket.write_all(b"hello world!").await.unwrap();
        }
    });

    bind(
        bind_addr,
        jump_hosts,
        service_addr,
        sopsfile_path.to_str().unwrap(),
    );

    let mut conn = TcpStream::connect(bind_addr).await.unwrap();
    let mut buf = vec![0; 1024];
    let n = conn.read(&mut buf).await?;
    let response = String::from_utf8_lossy(&buf[..n]).to_string();
    println!("Received: {}", response);
    assert_eq!(response, "");

    unbind(bind_addr);
    for task in ssh_tasks {
        task.abort();
    }
    service_handle.abort();
    Ok(())
}
