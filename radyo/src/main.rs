use anyhow::Result;
use clap::{Parser, Subcommand};
// use cpal::traits::{DeviceTrait, HostTrait};
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, NodeAddr, Watcher};
use iroh_base::ticket::NodeTicket;
// use ringbuf::{HeapCons, HeapProd, HeapRb};
// use ringbuf::traits::{Consumer, Producer, Split};
use std::future::Future;
use std::path::Path;
// use tokio::task::JoinHandle;

#[derive(Subcommand)]
enum Cmd {
    Caller { 
        #[arg(default_value = "lost_woods")]
        ringtone: String 
    },
    Peer { 
        token: String,
    },
}

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

const ALPN: &[u8] = b"radyo/1.0";

#[derive(Debug, Clone)]
struct RadyoProtocol;

impl ProtocolHandler for RadyoProtocol {
    fn accept(&self, conn: Connection) -> impl Future<Output = Result<(), AcceptError>> + Send {
        async move {
            incoming_call_handler(conn).await;
            Ok(())
        }
    }
}

// Global storage for caller's ringtone preference
static CALLER_RINGTONE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

// Global hangup signal - can be triggered by either side
static HANGUP_SIGNAL: std::sync::OnceLock<tokio::sync::broadcast::Sender<()>> = std::sync::OnceLock::new();

// Initialize the hangup signal system
fn init_hangup_system() -> tokio::sync::broadcast::Receiver<()> {
    let (sender, receiver) = tokio::sync::broadcast::channel(1);
    
    // Store the sender globally so hangup() can access it (only if not already set)
    let _ = HANGUP_SIGNAL.set(sender); // Ignore error if already set
    
    receiver
}

// Hangup function that can be called by either side
async fn hangup() -> Result<()> {
    if let Some(sender) = HANGUP_SIGNAL.get() {
        println!("📞 Initiating hangup...");
        let _ = sender.send(()); // Notify all listeners
        println!("✅ Hangup signal sent");
    }
    Ok(())
}

async fn incoming_call_handler(conn: Connection) {
    println!("📞 New incoming call session started");
    if let Err(e) = handle_incoming_call(conn).await {
        eprintln!("❌ Call handling error: {}", e);
    }
    println!("📞 Call session ended - ready for next call");
}

async fn handle_incoming_call(conn: Connection) -> Result<()> {
    println!("📞 Incoming call detected!");
    
    // Accept the bidirectional stream
    let (send, mut recv) = conn.accept_bi().await?;
    
    // Read the incoming call signal
    let mut buffer = [0u8; 13]; // "INCOMING_CALL" length
    recv.read_exact(&mut buffer).await?;
    
    if &buffer == b"INCOMING_CALL" {
        println!("📞 Confirmed incoming call - playing ringtone");
        
        // Get the caller's preferred ringtone
        let default_ringtone = "lost_woods".to_string();
        let ringtone_name = CALLER_RINGTONE.get().unwrap_or(&default_ringtone);
        
        // Play the caller's ringtone and listen for hangup signal with acknowledgment
        play_caller_ringtone_with_hangup_ack(ringtone_name, recv, send).await?;
    }
    
    Ok(())
}


// Function that listens for HANGUP message and sends acknowledgment
async fn play_caller_ringtone_with_hangup_ack(ringtone_name: &str, mut recv: iroh::endpoint::RecvStream, mut send: iroh::endpoint::SendStream) -> Result<()> {
    println!("🎵 Playing caller's ringtone: {}", ringtone_name);
    
    // Initialize hangup system for caller side
    let mut local_hangup_rx = init_hangup_system();
    
    // Load the ringtone file
    let file_path = format!("ringtons/{}.mp3", ringtone_name);
    let file_data = if Path::new(&file_path).exists() {
        std::fs::read(&file_path)?
    } else {
        println!("Ringtone '{}' not found, using lost_woods.mp3", ringtone_name);
        std::fs::read("ringtons/lost_woods.mp3")?
    };
    
    println!("🔊 Ringtone playing on caller's device...");
    println!("💡 Press Ctrl+C or call hangup() to stop");
    
    // Create a shared atomic flag to control audio playback
    let stop_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_flag_clone = stop_flag.clone();
    
    // Spawn the audio task with stop control
    let file_data_clone = file_data.clone();
    let audio_task = tokio::task::spawn_blocking(move || -> Result<()> {
        let (_stream, stream_handle) = rodio::OutputStream::try_default()?;
        let sink = rodio::Sink::try_new(&stream_handle)?;
        
        let cursor = std::io::Cursor::new(file_data_clone);
        let source = rodio::Decoder::new(cursor)?;
        
        sink.append(source);
        sink.set_volume(0.5);
        
        // Check for stop signal periodically while playing
        loop {
            if sink.empty() {
                println!("📞 Ringtone finished naturally");
                break;
            }
            
            // Check if we should stop
            if stop_flag_clone.load(std::sync::atomic::Ordering::Relaxed) {
                println!("📞 Ringtone stopped by hangup signal");
                sink.stop();
                break;
            }
            
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        Ok(())
    });
    
    // Listen for hangup signal from peer
    let peer_hangup_monitor = async move {
        let mut hangup_buf = [0u8; 6]; // "HANGUP" length
        match recv.read_exact(&mut hangup_buf).await {
            Ok(_) if &hangup_buf == b"HANGUP" => {
                println!("📞 Received HANGUP signal from peer!");
                true
            }
            Ok(_) => {
                println!("📞 Received unexpected data from peer");
                false
            }
            Err(_) => {
                println!("📞 Connection lost");
                false
            }
        }
    };
    
    // Race between audio completion, peer hangup, local hangup, and Ctrl+C
    tokio::select! {
        result = audio_task => {
            match result {
                Ok(Ok(())) => println!("🎵 Ringtone completed normally"),
                Ok(Err(e)) => println!("❌ Ringtone error: {}", e),
                Err(e) => println!("❌ Task error: {}", e),
            }
        }
        hangup_received = peer_hangup_monitor => {
            if hangup_received {
                println!("🔇 Peer hung up - stopping ringtone!");
                stop_flag.store(true, std::sync::atomic::Ordering::Relaxed); // Stop the audio immediately
                
                // Send acknowledgment to peer
                println!("📤 Sending hangup acknowledgment to peer...");
                if let Err(e) = send.write_all(b"HANGUP_ACK").await {
                    println!("⚠️ Failed to send hangup acknowledgment: {}", e);
                } else {
                    println!("✅ Hangup acknowledgment sent");
                }
                
                hangup().await?; // Trigger local hangup too
            }
        }
        _ = local_hangup_rx.recv() => {
            println!("🔇 Local hangup signal received - stopping ringtone!");
            stop_flag.store(true, std::sync::atomic::Ordering::Relaxed); // Stop the audio immediately
        }
        _ = tokio::signal::ctrl_c() => {
            println!("🔇 Ctrl+C pressed - hanging up call!");
            stop_flag.store(true, std::sync::atomic::Ordering::Relaxed); // Stop the audio immediately
            hangup().await?;
        }
    }
    
    Ok(())
}

async fn caller_mode(ringtone: String) -> Result<()> {
    println!("📞 Starting persistent phone service with ringtone: {}", ringtone);
    
    // Store the ringtone preference globally
    CALLER_RINGTONE.set(ringtone.clone()).map_err(|_| anyhow::anyhow!("Failed to set ringtone"))?;
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

async fn peer_mode(ticket: String) -> Result<()> {
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

async fn send_hangup_to_caller(send: &mut iroh::endpoint::SendStream) -> Result<()> {
    println!("📞 Sending hangup signal to caller...");
    if let Err(e) = send.write_all(b"HANGUP").await {
        println!("❌ Failed to send hangup signal: {}", e);
        return Err(e.into());
    }
    println!("✅ Hangup signal sent successfully");
    Ok(())
}




#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Cmd::Caller { ringtone } => caller_mode(ringtone).await?,
        Cmd::Peer { token } => peer_mode(token).await?,
    }
    Ok(())
}