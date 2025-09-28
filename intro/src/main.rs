use std::path::{absolute, PathBuf};
use std::str::FromStr;
use anyhow::Result;
use iroh::*;
use iroh_blobs::{ticket, BlobsProtocol};
use iroh_blobs::store::mem::MemStore;
use clap::{Parser, Subcommand};
use iroh::protocol::Router;
use iroh_blobs::api::tags::TagInfo;
use iroh_blobs::ticket::BlobTicket;

#[derive(Subcommand)]
enum Cmd {
    Listen { pathname: String },
    Connect { token: String , destination_path: String },
}

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[tokio::main]
async fn main() -> Result<()> {
    
    match  Endpoint::builder()
        .discovery_n0()
        .bind()
        .await{
        Ok(endpoint) => {
            let memory = MemStore::new();
            let blobs_protocol = BlobsProtocol::new(&memory, endpoint.clone(), None);

            match Cli::parse().cmd {
                Cmd::Listen { pathname }=> {
                    let tag = listen(&blobs_protocol, pathname.as_str()).await?;
                    let node_addr = endpoint.node_id().into();
                    let ticket = BlobTicket::new(node_addr, tag.hash, tag.format);
                    println!("Connect using ticket: {}", ticket);
                },
                Cmd::Connect { token, destination_path } => connect(&blobs_protocol, &endpoint, &token, &destination_path).await?,
            }

            let router =  Router::builder(endpoint)
                .accept(iroh_blobs::ALPN, blobs_protocol)
                .spawn();
            tokio::signal::ctrl_c().await?;
            // Gracefully shut down the node
            println!("Shutting down.");
            router.shutdown().await?;

        },
        Err(e) => {
            println!("Endpoint error: {:?}", e);
            // Ok(())
        }
    }

    Ok(())
}

async fn listen(protocol: &BlobsProtocol, pathname: &str) -> Result<TagInfo> {
    let path = PathBuf::from(pathname);
    let abs_path = absolute(path)?;
    let tag = protocol.add_path(abs_path).await?;
    Ok(tag)
}

async fn connect( protocol: &BlobsProtocol, endpoint: &Endpoint, token: &str, destination_path: &str) -> Result<()> {
    let ticket = BlobTicket::from_str(token)?;
    let downloader = protocol.downloader(endpoint);
    downloader.download(ticket.hash(),  Some(ticket.node_addr().node_id)).await?;
    println!("Finished download.");
    let path = PathBuf::from(destination_path);
    let abs_path = absolute(path)?;
    protocol.export(ticket.hash(), abs_path).await?;
    println!("Finished copying.");
    Ok(())
}