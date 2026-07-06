// use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    // /// Optional name to operate on
    // name: Option<String>,

    // /// Sets a custom config file
    // #[arg(short, long, value_name = "FILE")]
    // config: Option<PathBuf>,

    // /// Turn debugging information on
    // #[arg(short, long, action = clap::ArgAction::Count)]
    // debug: u8,
    /// Sub Commands
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    Launch(LaunchCmd),
    FsWorker(FsWorkerCmd),
}

impl Default for Commands {
    fn default() -> Self {
        Commands::Launch(LaunchCmd::default())
    }
}

/// normal startup
#[derive(Parser)]
pub struct LaunchCmd {
    /// launch a new instance instead of reusing an existing one
    #[arg(long, default_value_t = false)]
    new_instance: bool,

    /// start as an instance ID, or join an existing instance ID
    #[arg(long)]
    instance_id: Option<u64>,
}

impl Default for LaunchCmd {
    fn default() -> Self {
        Self {
            new_instance: false,
            instance_id: None,
        }
    }
}

/// start as a file system worker process
#[derive(Parser)]
pub struct FsWorkerCmd {
    /// Worker ID
    #[arg(long)]
    worker_id: u64,
}

pub fn parse() {
    let p = Cli::parse();

    let cmd = p.command.unwrap_or_default();
    match cmd {
        Commands::Launch(launch_cmd) => {
            if launch_cmd.new_instance {
                
            }
        }
        Commands::FsWorker(fs_worker_cmd) => {}
    }
}
