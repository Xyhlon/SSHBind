use env_logger::Builder;
use log::info;
use log::LevelFilter;
use russh::server::Server;
use sshbind::{bind, unbind};
use sshbind::{Creds, YamlCreds};
use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, LazyLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task;

mod helpers;

#[cfg(windows)]
const FAST_RSA_KEY_SIZE: usize = 1024;

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
    let jump_hosts = vec!["127.0.0.1:20".to_string()];
    let service_addr = Some("127.0.0.1:8080".to_string());

    bind(bind_addr, jump_hosts, service_addr, "aq^fasdfs*$%", None);
    unbind(bind_addr);
}

#[tokio::test]
async fn test_correct_configuration() -> Result<(), Box<dyn std::error::Error>> {
    #[allow(clippy::let_unit_value)]
    let _ = *LOGGER; // Ensure logger is initialized

    // Test ID 1: 2 jump hosts, uses ports 10000, 10001-10002 (jump hosts), 10003 (service)
    let ports = helpers::TestPorts::new(1, 2);

    let mut testcreds = YamlCreds::new();
    testcreds.insert(
        ports.jump_host(0),
        Creds {
            username: "pi".to_string(),
            password: "max".to_string(),
            totp_key: None,
        },
    );
    testcreds.insert(
        ports.jump_host(1),
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

    #[cfg(unix)]
    {
        use russh::keys::Algorithm;
        let pk = russh::keys::PrivateKey::random(&mut rng, Algorithm::Ed25519).unwrap();
        config.keys.push(pk);
    }
    #[cfg(windows)]
    {
        use russh::keys::ssh_key::private::{KeypairData, RsaKeypair};
        let keypair = KeypairData::from(RsaKeypair::random(&mut rng, FAST_RSA_KEY_SIZE).unwrap());
        let pk = russh::keys::PrivateKey::new(keypair, "").unwrap();
        config.keys.push(pk);
    }
    let config = Arc::new(config);
    use helpers::Credentials;
    let mut hosts_users: HashMap<String, HashMap<String, Credentials>> = HashMap::new();
    testcreds.iter().for_each(|(k, v)| {
        let mut map: HashMap<String, Credentials> = HashMap::new();
        map.insert(v.username.clone(), Credentials::from(v.clone()));
        hosts_users.insert(k.to_string(), map);
    });
    info!("Prepared SSH server configuration");

    let bind_addr = ports.bind_addr();
    let jump_hosts = ports.jump_hosts();
    let service_addr_consume = ports.service_addr();
    let service_addr = Some(service_addr_consume.clone());

    // Set up cleanup handler to ensure cleanup happens even on test failure
    let mut cleanup = helpers::TestCleanup::new(bind_addr.clone());

    let ssh_tasks: Vec<_> = jump_hosts
        .clone()
        .into_iter()
        .map(|ssh_addr| {
            let cloned_config = config.clone();
            let users = hosts_users.get(&ssh_addr).cloned().unwrap_or_default();
            let mut server = helpers::SSHServer::new(Some(users));

            task::spawn(async move {
                let _ = server.run_on_address(cloned_config, &ssh_addr).await;
            })
        })
        .collect();

    // Register SSH tasks with cleanup handler
    for task in &ssh_tasks {
        cleanup.add_ssh_task(task);
    }

    // Give SSH servers more time to start up on Windows
    #[cfg(windows)]
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    #[cfg(not(windows))]
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    info!("SSH servers started");
    let service_handle = task::spawn(async move {
        let serv = TcpListener::bind(service_addr_consume).await.unwrap();
        loop {
            let (mut socket, _) = serv.accept().await.unwrap();
            socket.write_all(b"hello world!").await.unwrap();
        }
    });

    // Register service handle with cleanup handler
    cleanup.set_service_handle(&service_handle);
    info!("Service started");

    bind(
        &bind_addr,
        jump_hosts,
        service_addr,
        sopsfile_path.to_str().unwrap(),
        None,
    );

    info!("Bind started");
    let mut conn = TcpStream::connect(&bind_addr).await.unwrap();
    let mut buf = vec![0; 1024];
    let n = conn.read(&mut buf).await?;
    let response = String::from_utf8_lossy(&buf[..n]).to_string();
    println!("Received: {}", response);
    assert_eq!(response, "hello world!");

    // Cleanup will be handled automatically by the Drop implementation
    // but we can also call it explicitly if desired
    cleanup.cleanup();
    Ok(())
}

#[tokio::test]
async fn test_correct_configuration_multiple() -> Result<(), Box<dyn std::error::Error>> {
    #[allow(clippy::let_unit_value)]
    let _ = *LOGGER; // Ensure logger is initialized

    // Test ID 2: 2 jump hosts, uses ports 10010, 10011-10012 (jump hosts), 10013 (service)
    let ports = helpers::TestPorts::new(2, 2);

    let mut testcreds = YamlCreds::new();
    testcreds.insert(
        ports.jump_host(0),
        Creds {
            username: "pi".to_string(),
            password: "max".to_string(),
            totp_key: None,
        },
    );
    testcreds.insert(
        ports.jump_host(1),
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

    #[cfg(unix)]
    {
        use russh::keys::Algorithm;
        let pk = russh::keys::PrivateKey::random(&mut rng, Algorithm::Ed25519).unwrap();
        config.keys.push(pk);
    }
    #[cfg(windows)]
    {
        use russh::keys::ssh_key::private::{KeypairData, RsaKeypair};
        let keypair = KeypairData::from(RsaKeypair::random(&mut rng, FAST_RSA_KEY_SIZE).unwrap());
        let pk = russh::keys::PrivateKey::new(keypair, "").unwrap();
        config.keys.push(pk);
    }
    let config = Arc::new(config);
    use helpers::Credentials;
    let mut hosts_users: HashMap<String, HashMap<String, Credentials>> = HashMap::new();
    testcreds.iter().for_each(|(k, v)| {
        let mut map: HashMap<String, Credentials> = HashMap::new();
        map.insert(v.username.clone(), Credentials::from(v.clone()));
        hosts_users.insert(k.to_string(), map);
    });

    let bind_addr = ports.bind_addr();
    let jump_hosts = ports.jump_hosts();
    let service_addr_consume = ports.service_addr();
    let service_addr = Some(service_addr_consume.clone());

    // Set up cleanup handler to ensure cleanup happens even on test failure
    let mut cleanup = helpers::TestCleanup::new(bind_addr.clone());

    let ssh_tasks: Vec<_> = jump_hosts
        .clone()
        .into_iter()
        .map(|ssh_addr| {
            let cloned_config = config.clone();
            let users = hosts_users.get(&ssh_addr).cloned().unwrap_or_default();
            let mut server = helpers::SSHServer::new(Some(users));

            task::spawn(async move {
                let _ = server.run_on_address(cloned_config, &ssh_addr).await;
            })
        })
        .collect();

    // Register SSH tasks with cleanup handler
    for task in &ssh_tasks {
        cleanup.add_ssh_task(task);
    }

    // Give SSH servers more time to start up on Windows
    #[cfg(windows)]
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

    #[cfg(not(windows))]
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let service_handle = task::spawn(async move {
        let serv = TcpListener::bind(service_addr_consume).await.unwrap();
        loop {
            let (mut socket, _) = serv.accept().await.unwrap();
            socket.write_all(b"hello world!").await.unwrap();
        }
    });

    // Register service handle with cleanup handler
    cleanup.set_service_handle(&service_handle);

    bind(
        &bind_addr,
        jump_hosts,
        service_addr,
        sopsfile_path.to_str().unwrap(),
        None,
    );

    let mut conn = TcpStream::connect(&bind_addr).await.unwrap();
    let mut buf = vec![0; 1024];
    let n = conn.read(&mut buf).await?;
    let response = String::from_utf8_lossy(&buf[..n]).to_string();
    println!("Received: {}", response);
    assert_eq!(response, "hello world!");

    let mut conn = TcpStream::connect(&bind_addr).await.unwrap();
    let mut buf = vec![0; 1024];
    let n = conn.read(&mut buf).await?;
    let response = String::from_utf8_lossy(&buf[..n]).to_string();
    println!("Received: {}", response);
    assert_eq!(response, "hello world!");

    // Cleanup will be handled automatically by the Drop implementation
    // but we can also call it explicitly if desired
    cleanup.cleanup();
    Ok(())
}

#[tokio::test]
async fn test_second_server_wrong_credentials() -> Result<(), Box<dyn std::error::Error>> {
    #[allow(clippy::let_unit_value)]
    let _ = *LOGGER; // Ensure logger is initialized

    // Test ID 3: 2 jump hosts, uses ports 10020, 10021-10022 (jump hosts), 10023 (service)
    let ports = helpers::TestPorts::new(3, 2);

    let mut testcreds = YamlCreds::new();
    testcreds.insert(
        ports.jump_host(0),
        Creds {
            username: "pi".to_string(),
            password: "max".to_string(),
            totp_key: None,
        },
    );
    testcreds.insert(
        ports.jump_host(1),
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

    #[cfg(unix)]
    {
        use russh::keys::Algorithm;
        let pk = russh::keys::PrivateKey::random(&mut rng, Algorithm::Ed25519).unwrap();
        config.keys.push(pk);
    }
    #[cfg(windows)]
    {
        use russh::keys::ssh_key::private::{KeypairData, RsaKeypair};
        let keypair = KeypairData::from(RsaKeypair::random(&mut rng, FAST_RSA_KEY_SIZE).unwrap());
        let pk = russh::keys::PrivateKey::new(keypair, "").unwrap();
        config.keys.push(pk);
    }
    let config = Arc::new(config);
    use helpers::Credentials;
    let mut hosts_users: HashMap<String, HashMap<String, Credentials>> = HashMap::new();
    testcreds.iter().for_each(|(k, v)| {
        let mut map: HashMap<String, Credentials> = HashMap::new();
        map.insert(v.username.clone(), Credentials::from(v.clone()));
        hosts_users.insert(k.to_string(), map);
    });

    let bind_addr = ports.bind_addr();
    let jump_hosts = ports.jump_hosts();
    let service_addr_consume = ports.service_addr();
    let service_addr = Some(service_addr_consume.clone());

    // Set up cleanup handler to ensure cleanup happens even on test failure
    let mut cleanup = helpers::TestCleanup::new(bind_addr.clone());

    let ssh_tasks: Vec<_> = jump_hosts
        .clone()
        .into_iter()
        .enumerate()
        .map(|(idx, ssh_addr)| {
            let cloned_config = config.clone();
            // Second server should have no valid users (to test wrong credentials)
            let users = if idx == 1 {
                HashMap::new() // No valid users for second server
            } else {
                hosts_users.get(&ssh_addr).cloned().unwrap_or_default()
            };
            let mut server = helpers::SSHServer::new(Some(users));

            task::spawn(async move {
                let _ = server.run_on_address(cloned_config, &ssh_addr).await;
            })
        })
        .collect();

    // Register SSH tasks with cleanup handler
    for task in &ssh_tasks {
        cleanup.add_ssh_task(task);
    }

    // Give SSH servers more time to start up on Windows
    #[cfg(windows)]
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    #[cfg(not(windows))]
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let service_handle = task::spawn(async move {
        let serv = TcpListener::bind(service_addr_consume).await.unwrap();
        loop {
            let (mut socket, _) = serv.accept().await.unwrap();
            socket.write_all(b"hello world!").await.unwrap();
        }
    });

    cleanup.set_service_handle(&service_handle);

    bind(
        &bind_addr,
        jump_hosts,
        service_addr,
        sopsfile_path.to_str().unwrap(),
        None,
    );

    // Give time for the bind to attempt connection through SSH chain
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

    // Try to connect - this should succeed but the connection through SSH will have failed
    let result = TcpStream::connect(&bind_addr).await;
    if let Ok(mut conn) = result {
        let mut buf = vec![0; 1024];
        // The read should either fail or return empty since SSH auth failed
        match conn.read(&mut buf).await {
            Ok(0) => {
                // Connection closed immediately - auth failed as expected
            }
            Ok(n) => {
                let response = String::from_utf8_lossy(&buf[..n]).to_string();
                println!("Received: {}", response);
                assert_eq!(response, "");
            }
            Err(_) => {
                // Connection error - auth failed as expected
            }
        }
    }

    // Cleanup will be handled automatically by the Drop implementation
    cleanup.cleanup();
    Ok(())
}

#[tokio::test]
async fn test_correct_configuration_2fa() -> Result<(), Box<dyn std::error::Error>> {
    #[allow(clippy::let_unit_value)]
    let _ = *LOGGER; // Ensure logger is initialized

    // Test ID 4: 1 jump host, uses ports 10030, 10031 (jump host), 10032 (service)
    let ports = helpers::TestPorts::new(4, 1);

    let mut testcreds = YamlCreds::new();
    testcreds.insert(
        ports.jump_host(0),
        Creds {
            username: "pi".to_string(),
            password: "max".to_string(),
            totp_key: "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ".to_string().into(),
        },
    );
    let tmp_dir = helpers::setup_sopsfile(testcreds.clone());
    let sopsfile_path = tmp_dir.path().join("secrets.yaml");
    let mut config = russh::server::Config::default();
    // Use OsRng from rand for randomness.
    use russh::keys::ssh_key::rand_core::OsRng;
    let mut rng = OsRng;

    #[cfg(unix)]
    {
        use russh::keys::Algorithm;
        let pk = russh::keys::PrivateKey::random(&mut rng, Algorithm::Ed25519).unwrap();
        config.keys.push(pk);
    }
    #[cfg(windows)]
    {
        use russh::keys::ssh_key::private::{KeypairData, RsaKeypair};
        let keypair = KeypairData::from(RsaKeypair::random(&mut rng, FAST_RSA_KEY_SIZE).unwrap());
        let pk = russh::keys::PrivateKey::new(keypair, "").unwrap();
        config.keys.push(pk);
    }
    let config = Arc::new(config);
    use helpers::Credentials;
    let mut hosts_users: HashMap<String, HashMap<String, Credentials>> = HashMap::new();
    testcreds.iter().for_each(|(k, v)| {
        let mut map: HashMap<String, Credentials> = HashMap::new();
        map.insert(v.username.clone(), v.clone().into());
        hosts_users.insert(k.to_string(), map);
    });

    println!("hosts_users: {:?}", hosts_users);

    let bind_addr = ports.bind_addr();
    let jump_hosts = ports.jump_hosts();
    let service_addr_consume = ports.service_addr();
    let service_addr = Some(service_addr_consume.clone());

    // Set up cleanup handler to ensure cleanup happens even on test failure
    let mut cleanup = helpers::TestCleanup::new(bind_addr.clone());

    let ssh_tasks: Vec<_> = jump_hosts
        .clone()
        .into_iter()
        .map(|ssh_addr| {
            let cloned_config = config.clone();
            let users = hosts_users.get(&ssh_addr).cloned().unwrap_or_default();
            let mut server = helpers::SSHServer::new(Some(users));

            task::spawn(async move {
                let _ = server.run_on_address(cloned_config, &ssh_addr).await;
            })
        })
        .collect();

    // Register SSH tasks with cleanup handler
    for task in &ssh_tasks {
        cleanup.add_ssh_task(task);
    }

    // Give SSH servers more time to start up on Windows
    #[cfg(windows)]
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    #[cfg(not(windows))]
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let service_handle = task::spawn(async move {
        let serv = TcpListener::bind(service_addr_consume).await.unwrap();
        loop {
            let (mut socket, _) = serv.accept().await.unwrap();
            socket.write_all(b"hello world!").await.unwrap();
        }
    });

    // Register service handle with cleanup handler
    cleanup.set_service_handle(&service_handle);

    bind(
        &bind_addr,
        jump_hosts,
        service_addr,
        sopsfile_path.to_str().unwrap(),
        None,
    );

    let mut conn = TcpStream::connect(&bind_addr).await.unwrap();
    let mut buf = vec![0; 1024];
    let n = conn.read(&mut buf).await?;
    let response = String::from_utf8_lossy(&buf[..n]).to_string();
    println!("Received: {}", response);
    assert_eq!(response, "hello world!");

    // Cleanup will be handled automatically by the Drop implementation
    cleanup.cleanup();
    Ok(())
}

// #[tokio::test]
// async fn serial_integration_test_correct_configuration_2fa_aug(
// ) -> Result<(), Box<dyn std::error::Error>> {
//     #[allow(clippy::let_unit_value)]
//     let _ = *LOGGER; // Ensure logger is initialized
//     let bind_addr = "127.0.0.1:7000";
//     let jump_hosts = vec![
//         "gate.mpcdf.mpg.de:22".to_string(),
//         "toki01.bc.rzg.mpg.de:22".to_string(),
//     ];
//     let service_addr = "httpforever.com:80";
//
//     bind(
//         &bind_addr,
//         jump_hosts,
//         service_addr,
//         "../tokadata-python-template/tests/creds/aug.yaml",
//     );
//
//     let mut conn = TcpStream::connect(&bind_addr).await.unwrap();
//     let mut buf = vec![0; 1024];
//     let n = conn.read(&mut buf).await?;
//     let response = String::from_utf8_lossy(&buf[..n]).to_string();
//     println!("Received: {}", response);
//     assert_eq!(response, "hello world!");
//
//     unbind(&bind_addr);
//     Ok(())
// }
