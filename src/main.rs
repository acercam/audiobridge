mod audio;
mod codec;
mod protocol;
mod session;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "audiobridge", about = "Bidirectional low-latency audio bridge over UDP")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Wait for a Mac client to connect (run on Linux server)
    Listen {
        /// Bind address (default: 0.0.0.0)
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,
        /// Audio device name substring (Linux default: USB Audio Device)
        #[arg(long)]
        device: Option<String>,
    },
    /// Connect to a remote listener (run on Mac)
    Connect {
        /// Remote Tailscale IP or hostname
        host: String,
        /// Audio device name substring
        #[arg(long)]
        device: Option<String>,
    },
    /// List available audio input/output devices
    Devices,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Listen { bind, device } => {
            session::run_listen(&bind, device)?;
        }
        Commands::Connect { host, device } => {
            session::run_connect(&host, device)?;
        }
        Commands::Devices => {
            session::run_devices()?;
        }
    }
    Ok(())
}
