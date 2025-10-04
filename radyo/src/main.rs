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
            // Spawn each call handler concurrently to allow multiple calls
            tokio::spawn(async move {
                incoming_call_handler(conn).await;
            });
            Ok(())
        }
    }
}

// Global storage for caller's ringtone preference
static CALLER_RINGTONE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

// Global hangup signal - can be triggered by either side
static HANGUP_SIGNAL: std::sync::OnceLock<tokio::sync::broadcast::Sender<()>> = std::sync::OnceLock::new();

// Global call state - ensure only one call at a time
static CALL_IN_PROGRESS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);


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
    let call_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() % 10000; // Short ID for this call
    
    println!("📞 [CALL-{}] New incoming call session started", call_id);
    if let Err(e) = handle_incoming_call(conn, call_id).await {
        eprintln!("❌ [CALL-{}] Call handling error: {}", call_id, e);
    }
    println!("📞 [CALL-{}] Call session ended - ready for next call", call_id);
}

async fn handle_incoming_call(conn: Connection, call_id: u128) -> Result<()> {
    println!("📞 [CALL-{}] Incoming call detected!", call_id);
    
    // Accept the bidirectional stream
    let (mut send, mut recv) = conn.accept_bi().await?;
    
    // Read the incoming call signal
    let mut buffer = [0u8; 13]; // "INCOMING_CALL" length
    recv.read_exact(&mut buffer).await?;
    
    if &buffer == b"INCOMING_CALL" {
        // Try to acquire call lock - only one call at a time
        if CALL_IN_PROGRESS.compare_exchange(
            false, 
            true, 
            std::sync::atomic::Ordering::Acquire,
            std::sync::atomic::Ordering::Relaxed
        ).is_err() {
            // Another call is in progress - send busy signal and close
            println!("📞 [CALL-{}] Phone is busy - rejecting call", call_id);
            send.write_all(b"BUSY").await?;
            send.finish()?; // Close the send stream
            return Ok(());
        }
        
        println!("📞 [CALL-{}] Confirmed incoming call - phone is now busy", call_id);
        
        // Get the caller's preferred ringtone
        let default_ringtone = "lost_woods".to_string();
        let ringtone_name = CALLER_RINGTONE.get().unwrap_or(&default_ringtone);
        
        // Play the caller's ringtone and listen for hangup signal with acknowledgment
        let result = play_caller_ringtone_with_hangup_ack(ringtone_name, recv, send, call_id).await;
        
        // Always free the call lock when done
        CALL_IN_PROGRESS.store(false, std::sync::atomic::Ordering::Release);
        println!("📞 [CALL-{}] Phone is now available for new calls", call_id);
        
        result?;
    }
    
    Ok(())
}


// Function that listens for HANGUP message and sends acknowledgment
async fn play_caller_ringtone_with_hangup_ack(ringtone_name: &str, mut recv: iroh::endpoint::RecvStream, mut send: iroh::endpoint::SendStream, call_id: u128) -> Result<()> {
    println!("🎵 [CALL-{}] Playing caller's ringtone: {}", call_id, ringtone_name);
    
    // Create per-call hangup channel - NO GLOBAL STATE!
    let (call_hangup_tx, mut call_hangup_rx) = tokio::sync::broadcast::channel::<()>(1);
    println!("📡 [CALL-{}] Created independent hangup channel for this call", call_id);
    
    // Load the ringtone file
    let file_path = format!("ringtons/{}.mp3", ringtone_name);
    let file_data = if Path::new(&file_path).exists() {
        std::fs::read(&file_path)?
    } else {
        println!("Ringtone '{}' not found, using lost_woods.mp3", ringtone_name);
        std::fs::read("ringtons/lost_woods.mp3")?
    };
    
    println!("🔊 [CALL-{}] Ringtone playing on caller's device...", call_id);
    println!("💡 [CALL-{}] Press Ctrl+C or call hangup() to stop", call_id);
    
    // Create a shared atomic flag to control audio playback
    let stop_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_flag_clone = stop_flag.clone();
    
    // Create a channel to wait for audio readiness
    let (audio_ready_tx, audio_ready_rx) = tokio::sync::oneshot::channel();
    
    // Spawn dedicated audio thread with readiness notification
    let file_data_clone = file_data.clone();
    let start_time = std::time::Instant::now();
    println!("⏰ [CALL-{}] Audio thread spawn starting at {:?}", call_id, start_time);
    
    std::thread::spawn(move || {
        let spawn_delay = start_time.elapsed();
        println!("🎵 [CALL-{}] Audio thread started (delay: {:?})", call_id, spawn_delay);
        
        let audio_result = (|| -> Result<()> {
            let audio_start = std::time::Instant::now();
            println!("🎵 [CALL-{}] Creating audio output stream...", call_id);
            let (_stream, stream_handle) = rodio::OutputStream::try_default()?;
            
            println!("🎵 [CALL-{}] Creating audio sink...", call_id);
            let sink = rodio::Sink::try_new(&stream_handle)?;
            
            let cursor = std::io::Cursor::new(file_data_clone);
            let source = rodio::Decoder::new(cursor)?;
            
            sink.append(source);
            sink.set_volume(0.5);
            
            let setup_time = audio_start.elapsed();
            println!("🎵 [CALL-{}] Audio ready! Setup time: {:?} - RINGTONE SHOULD BE PLAYING NOW", call_id, setup_time);
            
            // Signal that audio is ready
            let _ = audio_ready_tx.send(());
            
            // Check for stop signal periodically while playing
            let mut check_count = 0;
            loop {
                if sink.empty() {
                    println!("📞 [CALL-{}] Ringtone finished naturally (after {} checks)", call_id, check_count);
                    break;
                }
                
                // Check if we should stop
                if stop_flag_clone.load(std::sync::atomic::Ordering::Relaxed) {
                    println!("📞 [CALL-{}] Ringtone stopped by hangup signal (after {} checks)", call_id, check_count);
                    sink.stop();
                    break;
                }
                
                check_count += 1;
                if check_count % 10 == 0 {
                    println!("🔄 [CALL-{}] Audio thread alive - check #{}", call_id, check_count);
                }
                
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Ok(())
        })();
        
        if let Err(e) = audio_result {
            println!("❌ [CALL-{}] Audio thread error: {}", call_id, e);
        }
        println!("🎵 [CALL-{}] Audio thread completed", call_id);
    });
    
    println!("⚡ [CALL-{}] Audio thread spawned, waiting for audio to be ready...", call_id);
    
    // Wait for audio to be ready before starting hangup monitoring
    match tokio::time::timeout(tokio::time::Duration::from_secs(5), audio_ready_rx).await {
        Ok(Ok(())) => {
            println!("✅ [CALL-{}] Audio confirmed ready - starting call monitoring", call_id);
        }
        Ok(Err(_)) => {
            println!("⚠️ [CALL-{}] Audio ready channel closed - continuing anyway", call_id);
        }
        Err(_) => {
            println!("⚠️ [CALL-{}] Audio ready timeout - continuing anyway", call_id);
        }
    }
    
    // Create a dummy audio task for the select! to work with
    let audio_task = tokio::task::spawn(async {
        // Just wait indefinitely - the real audio runs in the dedicated thread above
        tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
        Ok::<(), anyhow::Error>(())
    });
    
    // Listen for hangup signal from peer
    let peer_hangup_monitor = async move {
        println!("👂 [CALL-{}] Starting peer hangup monitor...", call_id);
        let mut hangup_buf = [0u8; 6]; // "HANGUP" length
        match recv.read_exact(&mut hangup_buf).await {
            Ok(_) if &hangup_buf == b"HANGUP" => {
                println!("📞 [CALL-{}] Received HANGUP signal from peer!", call_id);
                true
            }
            Ok(_) => {
                println!("📞 [CALL-{}] Received unexpected data from peer", call_id);
                false
            }
            Err(e) => {
                println!("📞 [CALL-{}] Connection lost: {}", call_id, e);
                false
            }
        }
    };
    
    // Race between audio completion, peer hangup, local hangup, and Ctrl+C
    println!("🔄 [CALL-{}] Starting select! loop - monitoring for events...", call_id);
    tokio::select! {
        result = audio_task => {
            match result {
                Ok(Ok(())) => println!("🎵 [CALL-{}] Audio task completed normally", call_id),
                Ok(Err(e)) => println!("❌ [CALL-{}] Audio task error: {}", call_id, e),
                Err(e) => println!("❌ [CALL-{}] Audio task panic: {}", call_id, e),
            }
        }
        hangup_received = peer_hangup_monitor => {
            if hangup_received {
                println!("🔇 [CALL-{}] Peer hung up - stopping ringtone!", call_id);
                stop_flag.store(true, std::sync::atomic::Ordering::Relaxed); // Stop the audio immediately
                
                // Send acknowledgment to peer
                println!("📤 [CALL-{}] Sending hangup acknowledgment to peer...", call_id);
                if let Err(e) = send.write_all(b"HANGUP_ACK").await {
                    println!("⚠️ [CALL-{}] Failed to send hangup acknowledgment: {}", call_id, e);
                } else {
                    println!("✅ [CALL-{}] Hangup acknowledgment sent", call_id);
                }
                
                let _ = call_hangup_tx.send(()); // Signal this call to stop
            } else {
                println!("🔇 [CALL-{}] Peer hangup monitor returned false", call_id);
            }
        }
        _ = call_hangup_rx.recv() => {
            println!("🔇 [CALL-{}] Call-specific hangup signal received - stopping ringtone!", call_id);
            stop_flag.store(true, std::sync::atomic::Ordering::Relaxed); // Stop the audio immediately
        }
        _ = tokio::signal::ctrl_c() => {
            println!("🔇 [CALL-{}] Ctrl+C pressed - hanging up call!", call_id);
            stop_flag.store(true, std::sync::atomic::Ordering::Relaxed); // Stop the audio immediately
            let _ = call_hangup_tx.send(()); // Signal this call to stop
        }
    }
    
    // Properly close streams to clean up connection
    println!("🧹 [CALL-{}] Cleaning up call session...", call_id);
    drop(send);
    // recv is already consumed by peer_hangup_monitor
    
    // Wait a moment for cleanup to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    println!("✅ [CALL-{}] Call cleanup completed", call_id);
    
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