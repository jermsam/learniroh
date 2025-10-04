use clap::{Parser, Subcommand};

#[derive(Subcommand)]
pub enum Cmd {
    Caller { 
        #[arg(default_value = "lost_woods")]
        ringtone: String 
    },
    Peer { 
        token: String,
    },
}

#[derive(Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Cmd,
}
