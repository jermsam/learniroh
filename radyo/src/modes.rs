use anyhow::Result;
use iroh::protocol::Router;
use iroh::{Endpoint, NodeAddr, Watcher};
use iroh_base::ticket::NodeTicket;
#[allow(unused_imports)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::call::{init_hangup_system, hangup, send_hangup_to_caller, CallManager};
use crate::protocol::{RadyoProtocol, ALPN};

pub async fn caller_mode(ringtone: String) -> Result<()> {
    println!("📞 Starting persistent phone service with ringtone: {}", ringtone);
    
    // Store the ringtone preference globally
    CallManager::set_ringtone(ringtone.clone())?;
    let endpoint = Endpoint::builder().discovery_n0().bind().await?;
    let router = Router::builder(endpoint)
        .accept(ALPN, RadyoProtocol)
        .spawn();
    let node_addr = router.endpoint().node_addr().initialized().await;
    let ticket = NodeTicket::new(node_addr);
    
    println!("📱 Your Contact Card (Node Ticket): {}", ticket);
    println!("📞 Phone service is now online - waiting for calls...");
    println!("💡 Share your contact card with others so they can call you");
    println!("🔄 This service will handle multiple calls - each call is a separate session");
    println!("⏹️  Press Ctrl+C to shut down your phone service");

    tokio::signal::ctrl_c().await?;
    println!("📞 Shutting down phone service...");
    router.shutdown().await?;
    println!("✅ Phone service stopped");
    Ok(())
}

pub async fn peer_mode(ticket: String) -> Result<()> {
    println!("📞 Starting peer mode - calling: {}", ticket);
    // Initialize hangup system
    let mut hangup_rx = init_hangup_system();
    
    let node_id: NodeTicket = ticket
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid node Ticket format"))?;
    let node_addr = NodeAddr::from(node_id);
    println!("Dialing {ticket:?} ...");
    // Create a client endpoint and connect to the peer using the same ALPN
    let endpoint = Endpoint::builder().discovery_n0().bind().await?;
    let conn = endpoint.connect(node_addr, ALPN).await?;
    println!("Connected. Opening bi-directional stream...");
    let (mut send, mut recv) = conn.open_bi().await?;
    
    // Send incoming call signal to trigger caller's ringtone
    println!("📞 Sending incoming call signal...");
    send.write_all(b"INCOMING_CALL").await?;
    println!("✅ Call initiated - caller should be ringing now");
    
    // Set up hangup monitoring
    println!("⏳ Press Ctrl+C to hang up the call...");
    println!("💡 You can also call hangup() programmatically");
    
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("📞 Ctrl+C detected - initiating hangup...");
            hangup().await?;
            
            // Send hangup and wait for acknowledgment
            send_hangup_to_caller(&mut send).await?;
            println!("⏳ Waiting for caller to acknowledge hangup...");
            
            // Wait for HANGUP_ACK from caller
            let mut ack_buf = [0u8; 10]; // "HANGUP_ACK" length
            match recv.read_exact(&mut ack_buf).await {
                Ok(_) if &ack_buf == b"HANGUP_ACK" => {
                    println!("✅ Caller acknowledged hangup - terminating cleanly");
                }
                _ => {
                    println!("⚠️ No acknowledgment received - terminating anyway");
                }
            }
        }
        _ = hangup_rx.recv() => {
            println!("📞 Hangup signal received - terminating call...");
            send_hangup_to_caller(&mut send).await?;
        }
        _ = tokio::time::sleep(tokio::time::Duration::from_secs(30)) => {
            println!("📞 Call timed out");
        }
    }
    
    Ok(())
}
