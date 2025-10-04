use anyhow::Result;
use iroh::endpoint::Connection;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
#[allow(unused_imports)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use crate::audio::AudioManager;

// Global storage for caller's ringtone preference
static CALLER_RINGTONE: OnceLock<String> = OnceLock::new();

// Global hangup signal - can be triggered by either side
static HANGUP_SIGNAL: OnceLock<tokio::sync::broadcast::Sender<()>> = OnceLock::new();

// Global call state - ensure only one call at a time
static CALL_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

pub struct CallManager;

impl CallManager {
    pub fn set_ringtone(ringtone: String) -> Result<()> {
        CALLER_RINGTONE.set(ringtone).map_err(|_| anyhow::anyhow!("Failed to set ringtone"))
    }

    pub fn get_ringtone() -> String {
        CALLER_RINGTONE.get().cloned().unwrap_or_else(|| "lost_woods".to_string())
    }

    pub fn is_call_in_progress() -> bool {
        CALL_IN_PROGRESS.load(Ordering::Relaxed)
    }

    pub fn try_acquire_call() -> bool {
        CALL_IN_PROGRESS.compare_exchange(
            false, 
            true, 
            Ordering::Acquire,
            Ordering::Relaxed
        ).is_ok()
    }

    pub fn release_call() {
        CALL_IN_PROGRESS.store(false, Ordering::Release);
    }
}

pub struct CallState {
    pub call_id: u128,
}

impl CallState {
    pub fn new() -> Self {
        let call_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() % 10000; // Short ID for this call
        
        Self { call_id }
    }
}

// Initialize the hangup signal system
pub fn init_hangup_system() -> tokio::sync::broadcast::Receiver<()> {
    let (sender, receiver) = tokio::sync::broadcast::channel(1);
    
    // Store the sender globally so hangup() can access it (only if not already set)
    let _ = HANGUP_SIGNAL.set(sender); // Ignore error if already set
    
    receiver
}

// Hangup function that can be called by either side
pub async fn hangup() -> Result<()> {
    if let Some(sender) = HANGUP_SIGNAL.get() {
        println!("📞 Initiating hangup...");
        let _ = sender.send(()); // Notify all listeners
        println!("✅ Hangup signal sent");
    }
    Ok(())
}

pub async fn incoming_call_handler(conn: Connection) {
    let call_state = CallState::new();
    
    println!("📞 [CALL-{}] New incoming call session started", call_state.call_id);
    if let Err(e) = handle_incoming_call(conn, call_state.call_id).await {
        eprintln!("❌ [CALL-{}] Call handling error: {}", call_state.call_id, e);
    }
    println!("📞 [CALL-{}] Call session ended - ready for next call", call_state.call_id);
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
        if !CallManager::try_acquire_call() {
            // Another call is in progress - send busy signal and close
            println!("📞 [CALL-{}] Phone is busy - rejecting call", call_id);
            send.write_all(b"BUSY").await?;
            send.finish()?; // Close the send stream
            return Ok(());
        }
        
        println!("📞 [CALL-{}] Confirmed incoming call - phone is now busy", call_id);
        
        // Get the caller's preferred ringtone
        let ringtone_name = CallManager::get_ringtone();
        
        // Play the caller's ringtone and listen for hangup signal with acknowledgment
        let result = play_caller_ringtone_with_hangup_ack(&ringtone_name, recv, send, call_id).await;
        
        // Always free the call lock when done
        CallManager::release_call();
        println!("📞 [CALL-{}] Phone is now available for new calls", call_id);
        
        result?;
    }
    
    Ok(())
}

// Function that listens for HANGUP message and sends acknowledgment
async fn play_caller_ringtone_with_hangup_ack(
    ringtone_name: &str, 
    mut recv: iroh::endpoint::RecvStream, 
    mut send: iroh::endpoint::SendStream, 
    call_id: u128
) -> Result<()> {
    println!("🎵 [CALL-{}] Playing caller's ringtone: {}", call_id, ringtone_name);
    
    // Create per-call hangup channel - NO GLOBAL STATE!
    let (call_hangup_tx, mut call_hangup_rx) = tokio::sync::broadcast::channel::<()>(1);
    println!("📡 [CALL-{}] Created independent hangup channel for this call", call_id);
    
    // Create audio manager and start playing
    let audio_manager = AudioManager::new();
    let audio_ready_rx = audio_manager.play_ringtone_async(ringtone_name, call_id)?;
    
    println!("🔊 [CALL-{}] Ringtone playing on caller's device...", call_id);
    println!("💡 [CALL-{}] Press Ctrl+C or call hangup() to stop", call_id);
    
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
                audio_manager.stop(); // Stop the audio immediately
                
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
            audio_manager.stop(); // Stop the audio immediately
        }
        _ = tokio::signal::ctrl_c() => {
            println!("🔇 [CALL-{}] Ctrl+C pressed - hanging up call!", call_id);
            audio_manager.stop(); // Stop the audio immediately
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

pub async fn send_hangup_to_caller(send: &mut iroh::endpoint::SendStream) -> Result<()> {
    println!("📞 Sending hangup signal to caller...");
    if let Err(e) = send.write_all(b"HANGUP").await {
        println!("❌ Failed to send hangup signal: {}", e);
        return Err(e.into());
    }
    println!("✅ Hangup signal sent successfully");
    Ok(())
}
