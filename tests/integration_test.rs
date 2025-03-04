use log::info;
use russh::server::Server;
use sshbind::{bind, unbind};
use sshbind::{Creds, YamlCreds};
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};

mod helpers;

#[test]
fn fail_not_path() {
    let bind_addr = "127.0.0.1:8000";
    let jump_hosts = vec!["127.0.0.0:20".to_string()];
    let service_addr = "127.0.0.0:80";

    bind(bind_addr, jump_hosts, &service_addr, "aq^fasdfs*$%");
    unbind(bind_addr);
}

// #[test]
// async fn correct_configuration() -> Result<(), Box<dyn std::error::Error>> {
//     let mut testcreds = YamlCreds::new();
//     testcreds.insert(
//         "example.com".to_string(),
//         Creds {
//             username: "pi".to_string(),
//             password: "max".to_string(),
//             totp_key: None,
//         },
//     );
//     testcreds.insert(
//         "httpforever.com".to_string(),
//         Creds {
//             username: "pi".to_string(),
//             password: "max".to_string(),
//             totp_key: "ABCAD37A".to_string().into(),
//         },
//     );
//
//     let sopsfile_path = helpers::setup_sopsfile(testcreds);
//
//     let bind_addr = "127.0.0.1:8000";
//     let jump_hosts = vec!["127.0.0.0:2222".to_string()];
//     let service_addr = "127.0.0.0:80";
//
//     let server = helpers::SSHServer();
//     russh::server::run(config, "127.0.0.1:2222", server).await?;
//
//     russh::Server::<helpers::SSHServer>::new(config, server)
//         .listen(addr)
//         .await?;
//
//     let handle = thread::spawn(move || {
//         let rt = Runtime::new().unwrap();
//         rt.block_on(async {
//             if let Err(e) =
//                 run_server(&bind_addr, jump_hosts, &remote_addr, creds, token_clone).await
//             {
//                 eprintln!("Server encountered an error: {}", e);
//             }
//         });
//     });
//
//     let mock_service = TcpListener::bind("127.0.0.0:80");
//
//     bind(bind_addr, jump_hosts, &service_addr, &sopsfile_path);
//     let stream = TcpStream::connect(bind_addr);
//     unbind(bind_addr);
//     Ok(())
// }

#[tokio::test]
async fn test_correct_configuration() -> Result<(), Box<dyn std::error::Error>> {
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

    for ssh_addr in jump_hosts.clone() {
        info!("Starting SSH server on {}", ssh_addr);
        let users: HashMap<String, Credentials>;
        if let Some(value) = hosts_users.get(&ssh_addr) {
            users = value.clone();
        } else {
            users = HashMap::new();
        }
        let mut server = helpers::SSHServer::new(Some(users));
        let cloned_config = config.clone();
        let _handle = tokio::spawn(async move {
            let _ = server
                .run_on_address(cloned_config, ssh_addr)
                .await
                .unwrap();
        });
        // users.clear();
    }

    let _service_handle = tokio::spawn(async move {
        let serv = TcpListener::bind(service_addr).await.unwrap();
        loop {
            let (mut socket, _) = serv.accept().await.unwrap();
            socket.write_all(b"hello world!").await.unwrap();
        }
    });

    bind(
        bind_addr,
        jump_hosts,
        &service_addr,
        &sopsfile_path.to_str().unwrap(),
    );

    std::thread::sleep(std::time::Duration::from_secs(1));

    let mut conn = TcpStream::connect(bind_addr).await.unwrap();
    let mut buf = vec![0; 1024];
    let n = conn.read(&mut buf).await?;
    let response = String::from_utf8_lossy(&buf[..n]).to_string();
    println!("Received: {}", response);
    assert_eq!(response, "hello world!");

    unbind(bind_addr);
    Ok(())
}

// #[tokio::test]
// async fn test_second_server_wrong_credentials() -> Result<(), Box<dyn std::error::Error>> {
//     let mut testcreds = YamlCreds::new();
//     testcreds.insert(
//         "127.0.0.1:2222".to_string(),
//         Creds {
//             username: "pi".to_string(),
//             password: "max".to_string(),
//             totp_key: None,
//         },
//     );
//     testcreds.insert(
//         "127.0.0.1:2323".to_string(),
//         Creds {
//             username: "pi".to_string(),
//             password: "max".to_string(),
//             totp_key: None,
//         },
//     );
//     testcreds.insert(
//         "httpforever.com".to_string(),
//         Creds {
//             username: "pi".to_string(),
//             password: "max".to_string(),
//             totp_key: "ABCAD37A".to_string().into(),
//         },
//     );
//     let tmp_dir = helpers::setup_sopsfile(testcreds.clone());
//     let sopsfile_path = tmp_dir.path().join("secrets.yaml");
//     let mut config = russh::server::Config::default();
//
//     // Use OsRng from rand for randomness.
//     use russh::keys::ssh_key::rand_core::OsRng;
//     let mut rng = OsRng;
//     use russh::keys::Algorithm;
//     let pk = russh::keys::PrivateKey::random(&mut rng, Algorithm::Ed25519).unwrap();
//     config.keys.push(pk);
//     let config = Arc::new(config);
//     use helpers::Credentials;
//     let mut hosts_users: HashMap<String, HashMap<String, Credentials>> = HashMap::new();
//     testcreds.iter().for_each(|(k, v)| {
//         let mut map: HashMap<String, Credentials> = HashMap::new();
//         map.insert(v.username.clone(), Credentials::from(v.clone()));
//         hosts_users.insert(k.to_string(), map);
//     });
//
//     let bind_addr = "127.0.0.1:7000";
//     let jump_hosts = vec!["127.0.0.1:2222".to_string(), "127.0.0.1:2323".to_string()];
//     let service_addr = "127.0.0.1:8080";
//
//     for ssh_addr in jump_hosts.clone() {
//         info!("Starting SSH server on {}", ssh_addr);
//         let users: HashMap<String, Credentials>;
//         if let Some(value) = hosts_users.get(&ssh_addr) {
//             users = value.clone();
//         } else {
//             users = HashMap::new();
//         }
//         let mut server = helpers::SSHServer::new(Some(users));
//         let cloned_config = config.clone();
//         let _handle = tokio::spawn(async move {
//             let _ = server
//                 .run_on_address(cloned_config, ssh_addr)
//                 .await
//                 .unwrap();
//         });
//         hosts_users.clear();
//     }
//
//     let service_handle = tokio::spawn(async move {
//         let serv = TcpListener::bind(service_addr).await.unwrap();
//         loop {
//             let (mut socket, _) = serv.accept().await.unwrap();
//             socket.write_all(b"hello world!").await.unwrap();
//         }
//     });
//
//     bind(
//         bind_addr,
//         jump_hosts,
//         &service_addr,
//         &sopsfile_path.to_str().unwrap(),
//     );
//
//     std::thread::sleep(std::time::Duration::from_secs(1));
//
//     let mut conn = TcpStream::connect(bind_addr).await.unwrap();
//     let mut buf = vec![0; 1024];
//     let n = conn.read(&mut buf).await?;
//     let response = String::from_utf8_lossy(&buf[..n]).to_string();
//     println!("Received: {}", response);
//     assert_eq!(response, "hello world!");
//
//     unbind(bind_addr);
//     Ok(())
// }
