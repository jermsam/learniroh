use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use iroh::protocol::Router;
use iroh::*;
use iroh_blobs::api::blobs::{ExportMode, ExportOptions};
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::ticket::BlobTicket;
use iroh_blobs::BlobsProtocol;
use std::path::{absolute, PathBuf};
use std::str::FromStr;

#[derive(Subcommand)]
enum Cmd {
    Listen {
        pathname: String,
    },
    Connect {
        token: String,
        destination_path: String,
    },
}

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[tokio::main]
async fn main() -> Result<()> {
    match Endpoint::builder().discovery_n0().bind().await {
        Ok(endpoint) => {
            // let memory = MemStore::new();
            let home = dirs_next::home_dir().ok_or_else(|| anyhow!("no home directory"))?;
            let tmp = home.join(".lean".to_string()).join(".data".to_string());
            let temp_store = FsStore::load(tmp).await?;
            // This avoids buffering entire blobs in RAM and avoids an extra copy when the filesystem supports hardlinks/reflinks;
            let blobs_protocol = BlobsProtocol::new(&temp_store, endpoint.clone(), None);

            match Cli::parse().cmd {
                Cmd::Listen { pathname } => {
                    listen(&blobs_protocol, &endpoint, pathname.as_str()).await?;
                    // listen to incoming peers
                    route(endpoint, blobs_protocol).await?;
                }
                Cmd::Connect {
                    token,
                    destination_path,
                } => connect(&blobs_protocol, &endpoint, &token, destination_path.as_str()).await?,
            }
            drop(temp_store);
        }
        Err(e) => {
            println!("Endpoint error: {:?}", e);
        }
    }

    Ok(())
}

async fn listen(protocol: &BlobsProtocol, endpoint: &Endpoint, pathname: &str) -> Result<()> {
    let path = PathBuf::from(pathname);
    let abs_path = absolute(path)?;
    let tag = protocol.add_path(abs_path).await?;
    let node_addr = endpoint.node_id().into();
    let ticket = BlobTicket::new(node_addr, tag.hash, tag.format);
    println!("TICKET: {}", format!("{}", ticket).blue());
    Ok(())
}

async fn route(endpoint: Endpoint, protocol: BlobsProtocol) -> Result<()> {
    let router = Router::builder(endpoint)
        .accept(iroh_blobs::ALPN, protocol)
        .spawn();
    tokio::signal::ctrl_c().await?;
    // Gracefully shut down the node
    println!("Shutting down.");
    router.shutdown().await?;
    Ok(())
}

async fn connect(
    protocol: &BlobsProtocol,
    endpoint: &Endpoint,
    token: &str,
    destination_path: &str,
) -> Result<()> {
    let ticket = BlobTicket::from_str(token)?;
    let hash = ticket.hash();
    let provider = Some(ticket.node_addr().node_id);

    let store = protocol.store();
    let downloader = store.downloader(endpoint);

    println!("Starting download...");
    downloader.download(hash, provider).await?;
    println!("Finished download.");
    let path = PathBuf::from(destination_path);
    let abs_path = absolute(path)?;

    // If the destination is a directory, create a filename based on the hash
    let export_path = if abs_path.is_dir() {
        abs_path.join(format!("downloaded_{}", hash))
    } else {
        abs_path
    };

    println!("Exporting to: {}", export_path.display());

    // Export using TryReference to avoid an extra copy when possible
    let options = ExportOptions {
        hash: ticket.hash(),
        mode: ExportMode::TryReference,
        target: export_path.clone(),
    };

    protocol.export_with_opts(options).await?;

    println!("Export complete: {}", export_path.display());
    // Clean shutdown
    println!("Shutting down receiver.");
    endpoint.close().await;
    Ok(())
}
