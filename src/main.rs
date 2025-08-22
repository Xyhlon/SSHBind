use std::path::PathBuf;
use std::sync::LazyLock;

mod cli;
mod daemon;
mod ipc;
mod logging;

use cli::{Cli, Commands};
use clap::Parser;

static LOG_PATH: LazyLock<PathBuf> = LazyLock::new(|| {
    dirs::data_local_dir()
        .unwrap_or_else(|| std::env::temp_dir())
        .join("sshbind/sshbind.log")
});

fn main() {
    let cli = Cli::parse();
    
    let start_offset = match logging::handle_verbose_logging(cli.verbose, cli.log_level) {
        Ok(offset) => offset,
        Err(e) => {
            eprintln!("Error handling verbose logging: {}", e);
            return;
        }
    };

    let result = match cli.command {
        Some(Commands::Daemon) => daemon::start_ipc_daemon(),
        Some(Commands::Bind {
            addr,
            jump_host,
            remote,
            sopsfile,
            cmd,
        }) => handle_bind_command(addr, jump_host, remote, sopsfile, cmd),
        Some(Commands::Unbind { addr, all }) => handle_unbind_command(addr, all),
        Some(Commands::Shutdown) => handle_shutdown_command(),
        Some(Commands::List) => handle_list_command(),
        None => {
            println!("Use --help");
            Ok(())
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
    }

    if cli.verbose {
        if let Err(e) = logging::print_logs_from_offset(start_offset) {
            eprintln!("Error printing logs: {}", e);
        }
    }
}

fn handle_bind_command(
    addr: String,
    jump_host: Vec<String>,
    remote: Option<String>,
    sopsfile: String,
    cmd: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    daemon::spawn_daemon_if_needed()?;
    let cmd = daemon::DaemonCommand::Bind {
        addr,
        jump_hosts: jump_host,
        remote,
        sopsfile,
        cmd,
    };
    match ipc::send_command(cmd) {
        Ok(daemon::DaemonResponse::Success(msg)) => println!("{}", msg),
        Ok(resp) => println!("Unexpected: {:?}", resp),
        Err(e) => eprintln!("{}", e),
    }
    Ok(())
}

fn handle_unbind_command(
    addr: Option<String>,
    all: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    daemon::spawn_daemon_if_needed()?;
    let cmd = if all {
        daemon::DaemonCommand::Shutdown
    } else if let Some(a) = addr {
        daemon::DaemonCommand::Unbind { addr: a }
    } else {
        eprintln!("--addr or --all required");
        return Ok(());
    };
    match ipc::send_command(cmd) {
        Ok(daemon::DaemonResponse::Success(msg)) => println!("{}", msg),
        Ok(resp) => println!("Unexpected: {:?}", resp),
        Err(e) => eprintln!("{}", e),
    }
    Ok(())
}

fn handle_shutdown_command() -> Result<(), Box<dyn std::error::Error>> {
    daemon::spawn_daemon_if_needed()?;
    match ipc::send_command(daemon::DaemonCommand::Shutdown) {
        Ok(daemon::DaemonResponse::Success(msg)) => println!("{}", msg),
        Ok(resp) => println!("Unexpected: {:?}", resp),
        Err(e) => eprintln!("{}", e),
    }
    Ok(())
}

fn handle_list_command() -> Result<(), Box<dyn std::error::Error>> {
    daemon::spawn_daemon_if_needed()?;
    match ipc::send_command(daemon::DaemonCommand::List) {
        Ok(daemon::DaemonResponse::List(list)) if list.is_empty() => {
            println!("No active bindings.")
        }
        Ok(daemon::DaemonResponse::List(list)) => {
            println!("{:<20} {:<30} {:<10} Hosts", "Address", "Remote", "Time");
            for b in list {
                println!(
                    "{:<20} {:<30} {:<10} {:?}",
                    b.addr,
                    b.remote.unwrap_or_default(),
                    b.timestamp,
                    b.jump_hosts
                );
                match b.cmd {
                    None => println!("  Command: None"),
                    Some(ref cmd) => println!("  Command: {:?}", cmd),
                }
            }
        }
        Ok(resp) => println!("Unexpected: {:?}", resp),
        Err(e) => eprintln!("{}", e),
    }
    Ok(())
}
