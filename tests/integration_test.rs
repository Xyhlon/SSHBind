use env_logger::Builder;
use log::LevelFilter;
use std::io::Write;
use std::sync::LazyLock;

mod helpers;

static LOGGER: LazyLock<()> = LazyLock::new(|| {
    let _ = Builder::new()
        .filter(None, LevelFilter::Info)
        .format(|buffer, record| writeln!(buffer, "[{}] - {}", record.level(), record.args()))
        .try_init();
});

fn init_logger() {
    #[allow(clippy::let_unit_value)]
    let _ = *LOGGER;
}

#[test]
#[serial_test::serial]
fn missing_sops_file_does_not_create_binding() {
    init_logger();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let bind_addr = listener.local_addr().unwrap().to_string();
    drop(listener);
    let temp_dir = tempfile::tempdir().unwrap();
    let missing = temp_dir.path().join("missing.yaml");

    sshbind::bind(
        &bind_addr,
        vec!["127.0.0.1:22".into()],
        Some("127.0.0.1:80".into()),
        missing.to_str().unwrap(),
        None,
    );
    let rebound = std::net::TcpListener::bind(&bind_addr)
        .expect("missing SOPS file must not create a listener");
    drop(rebound);
    sshbind::unbind(&bind_addr);
}

#[tokio::test]
#[serial_test::serial]
async fn forwards_through_password_authenticated_chain() -> helpers::TestResult<()> {
    init_logger();
    let mut rig = helpers::SshTestRig::start(helpers::RigOptions::password_chain(2)).await?;
    rig.start_binding().await?;
    assert_eq!(rig.read_payload().await?, b"hello world!");
    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn retries_when_initial_bind_address_is_occupied() -> helpers::TestResult<()> {
    init_logger();
    let mut rig = helpers::SshTestRig::start(helpers::RigOptions::password_chain(1)).await?;
    let occupied = rig.occupy_initial_bind_address()?;

    rig.start_binding().await?;
    occupied.abort();

    assert_eq!(rig.read_payload().await?, b"hello world!");
    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn serves_repeated_connections() -> helpers::TestResult<()> {
    init_logger();
    let mut rig = helpers::SshTestRig::start(helpers::RigOptions::password_chain(2)).await?;
    rig.start_binding().await?;
    assert_eq!(rig.read_payload().await?, b"hello world!");
    assert_eq!(rig.read_payload().await?, b"hello world!");
    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn rejects_invalid_credentials_on_second_host() -> helpers::TestResult<()> {
    init_logger();
    let mut rig = helpers::SshTestRig::start(helpers::RigOptions::rejecting_second_host()).await?;
    rig.start_binding().await?;
    let payload = rig.read_until_closed().await?;
    assert!(
        payload.is_empty(),
        "unexpected forwarded payload: {payload:?}"
    );
    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn retries_rejected_auth_when_initial_bind_address_is_occupied() -> helpers::TestResult<()> {
    init_logger();
    let mut rig = helpers::SshTestRig::start(helpers::RigOptions::rejecting_second_host()).await?;
    let occupied = rig.occupy_initial_bind_address()?;

    rig.start_binding().await?;
    occupied.abort();

    assert!(rig.read_until_closed().await?.is_empty());
    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn forwards_with_totp_authentication() -> helpers::TestResult<()> {
    init_logger();
    let mut rig = helpers::SshTestRig::start(helpers::RigOptions::totp()).await?;
    rig.start_binding().await?;
    assert_eq!(rig.read_payload().await?, b"hello world!");
    Ok(())
}
