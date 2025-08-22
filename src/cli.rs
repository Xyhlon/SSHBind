use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(author, version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
    #[arg(short = 'v', long)]
    pub verbose: bool,
    #[arg(short = 'l', long)]
    pub log_level: Option<u64>,
}

#[derive(Subcommand)]
pub enum Commands {
    Bind {
        #[arg(short, long)]
        addr: String,
        #[arg(short = 'j', long)]
        jump_host: Vec<String>,
        #[arg(short, long)]
        remote: Option<String>,
        #[arg(short, long)]
        sopsfile: String,
        #[arg(short, long)]
        cmd: Option<String>,
    },
    Unbind {
        #[arg(short, long)]
        addr: Option<String>,
        #[arg(long)]
        all: bool,
    },
    Shutdown,
    List,
    #[command(hide = true)]
    Daemon,
}