use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct AudioManager {
    stop_flag: Arc<AtomicBool>,
}

impl AudioManager {
    pub fn new() -> Self {
        Self {
            stop_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }

    pub fn is_stopped(&self) -> bool {
        self.stop_flag.load(Ordering::Relaxed)
    }

    pub fn play_ringtone_async(&self, ringtone_name: &str, call_id: u128) -> Result<tokio::sync::oneshot::Receiver<()>> {
        let (audio_ready_tx, audio_ready_rx) = tokio::sync::oneshot::channel();
        let stop_flag = self.stop_flag.clone();
        
        // Load the ringtone file
        let file_path = format!("ringtons/{}.mp3", ringtone_name);
        let file_data = if Path::new(&file_path).exists() {
            std::fs::read(&file_path)?
        } else {
            println!("Ringtone '{}' not found, using lost_woods.mp3", ringtone_name);
            std::fs::read("ringtons/lost_woods.mp3")?
        };

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
                
                let cursor = std::io::Cursor::new(file_data);
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
                    if stop_flag.load(Ordering::Relaxed) {
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

        Ok(audio_ready_rx)
    }
}

impl Default for AudioManager {
    fn default() -> Self {
        Self::new()
    }
}
