use clap::Parser;
use radyo::{Cli, Cmd, caller_mode, peer_mode, Result};

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Cmd::Caller { ringtone } => caller_mode(ringtone).await?,
        Cmd::Peer { token } => peer_mode(token).await?,
    }
    Ok(())
}