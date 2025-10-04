use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;
use iroh::{
    Endpoint,
    NodeAddr,
    NodeId,
};
use iroh::endpoint::Connection;
use iroh::protocol::{
    AcceptError,
    ProtocolHandler,
    Router
};
use n0_future::boxed::BoxFuture;
use std::fmt::Debug;
use tokio::io::{copy, AsyncWriteExt};

#[derive(Subcommand)]
enum Cmd {
    /// Start the echo server and listen for connections
    Listen,
    /// Connect to an echo server and send a message
    Connect {
        /// Node ID to connect to
        node_id: String,
    },
}

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Clone)]
struct Echo;

impl ProtocolHandler for Echo {
    fn accept(&self, connection: Connection) -> BoxFuture<Result<(), AcceptError>> {
        Box::pin(async move {
                    let node_id = connection.remote_node_id()?;
                    println!("accepted connection from {node_id}");
                    // Our protocol is a simple request-response protocol, so we expect the
                    // connecting peer to open a single bidirectional stream.
                    let (mut send, mut recv) = connection.accept_bi().await?;

                    // Echo any bytes received back directly.
                    // This will keep copying until the sender signals the end of data on the stream.
                    let bytes_sent = copy(&mut recv, &mut send).await?;
                    println!("Copied over {bytes_sent} byte(s)");

                    // By calling `finish` on the send stream we signal that we will not send anything
                    // further, which makes the reception stream on the other end terminate.
                    send.finish()?;

                    // Wait until the remote closes the connection, which it does once it
                    // received the response.
                    connection.closed().await;
            Ok(())
        })
    }
}

const ALPN: &[u8] = b"lean/tour/echo/0";

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.cmd {
        Cmd::Listen => {
            let router = start_accept_side().await?;
            let node_id = router.endpoint().node_id();
            println!("Echo server started!");
            println!("Node ID: {}", format!("{}", node_id).blue());
            println!("Waiting for connections... Press Ctrl+C to stop.");

            // Keep the server running
            tokio::signal::ctrl_c().await?;
            println!("Shutting down server...");
            router.shutdown().await?;
        }
        Cmd::Connect { node_id } => {
            let node_id: NodeId = node_id.parse().map_err(|_| anyhow::anyhow!("Invalid node ID format"))?;
            let node_addr = NodeAddr::from(node_id);
            connect_side(node_addr).await?;
        }
    }
    Ok(())
}

async fn start_accept_side() -> Result<Router> {
    let endpoint = Endpoint::builder().discovery_n0().bind().await?;
    // Build our protocol handler and add our protocol, identified by its ALPN, and spawn the node.
    let router = Router::builder(endpoint).accept(ALPN, Echo).spawn();
    Ok(router)
}


async fn connect_side(node_addr: NodeAddr) -> Result<()> {
    println!("Connecting to: {}", format!("{:?}", node_addr).blue());
    let endpoint = Endpoint::builder().discovery_n0().bind().await?;
    let conn = endpoint.connect(node_addr, ALPN).await?;
    println!("Connected! Sending message...");

    let (mut sender, mut receiver) = conn.open_bi().await?;
    sender.write_all(b"Hello, world!").await?;
    sender.finish()?;

    let response = receiver.read_to_end(1024).await?;
    println!("Received echo: {}", String::from_utf8_lossy(&response).green());
    assert_eq!(response, b"Hello, world!");

    conn.close(0u32.into(), b"bye");
    endpoint.close().await;
    println!("Connection closed successfully!");
    Ok(())
}
