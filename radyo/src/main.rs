use anyhow::Result;
use clap::{Parser, Subcommand};
// use cpal::traits::{DeviceTrait, HostTrait};
use iroh::endpoint::{Connection, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, NodeAddr};
use iroh_base::ticket::NodeTicket;
// use ringbuf::{HeapCons, HeapProd, HeapRb};
// use ringbuf::traits::{Consumer, Producer, Split};
use std::future::Future;
use std::path::Path;
// use tokio::task::JoinHandle;

#[derive(Subcommand)]
enum Cmd {
    Caller,
    Receiver { 
        token: String, 
        #[arg(default_value = "lost_woods")]
        ringtone: String 
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
            audio_stream(conn);
            Ok(())
        }
    }
}

async fn call() -> Result<()> {
    let endpoint = Endpoint::builder().discovery_n0().bind().await?;
    let router = Router::builder(endpoint)
        .accept(ALPN, RadyoProtocol)
        .spawn();
    let node_id = router.endpoint().node_id();
    let node_addr = NodeAddr::from(node_id);
    let ticket = NodeTicket::new(node_addr);
    println!("Caller Node ID: {}", ticket);

    tokio::signal::ctrl_c().await?;
    println!("Shutting down server...");
    router.shutdown().await?;
    Ok(())
}

async fn receive(ticket: String, ringtone: String) -> Result<()> {
    let node_id: NodeTicket = ticket
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid node Ticket format"))?;
    let node_addr = NodeAddr::from(node_id);
    println!("Dialing {ticket:?} ...");
    // Create a client endpoint and connect to the peer using the same ALPN
    let endpoint = Endpoint::builder().discovery_n0().bind().await?;
    let conn = endpoint.connect(node_addr, ALPN).await?;
    println!("Connected. Opening bi-directional stream...");
    let (send, _recv) = conn.open_bi().await?;
    // Stream ringtone to peer
    stream_ringtone(send, ringtone).await?;
    Ok(())
}

async fn stream_ringtone(mut send: SendStream, ringtone: String) -> Result<()> {
    println!("Sending ringtone '{}' to caller for playback...", ringtone);
    
    // Load the ringtone file to send to the caller
    let file_path = format!("ringtons/{}.mp3", ringtone);
    let file_data = if Path::new(&file_path).exists() {
        std::fs::read(&file_path)?
    } else {
        println!("Ringtone '{}' not found, using lost_woods.mp3", ringtone);
        std::fs::read("ringtons/lost_woods.mp3")?
    };
    
    println!("Sending {} bytes of MP3 data to caller...", file_data.len());
    
    // Send file size first (4 bytes)
    send.write_all(&(file_data.len() as u32).to_le_bytes()).await?;
    
    // Send the MP3 file data in chunks
    const CHUNK_SIZE: usize = 8192; // 8KB chunks
    for chunk in file_data.chunks(CHUNK_SIZE) {
        send.write_all(chunk).await?;
    }
    
    println!("MP3 file sent to caller successfully");
    println!("Caller should now be playing the ringtone...");
    
    // Keep connection alive and wait for Ctrl+C
    println!("Press Ctrl+C to hang up and stop the ringtone");
    tokio::signal::ctrl_c().await?;
    println!("Hanging up - sending stop signal to caller");
    
    // Send hangup signal
    send.write_all(b"HANGUP").await?;
    
    Ok(())
}



#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Cmd::Caller => call().await?,
        Cmd::Receiver { token, ringtone } => receive(token, ringtone).await?,
    }
    Ok(())
}
/**
Stream differences:
 - [async vs realtime] QUIC recv is async; audio output is realtime callback.
 - [pull vs push] Audio pulls; network pushes.
 - [bridge] We need a buffer between them.
Next steps
 - [Choose buffer]
   - Simple: std::sync::mpsc (easy but can drop samples).
   - Better: lock-free SPSC ring buffer (e.g., ringbuf crate) for fewer glitches.
 - [Confirm audio format]
   - What does the sender produce? Sample type (f32/i16), channels (mono/stereo), sample rate (e.g., 48k).
   - If it doesn’t match a device format, we’ll add conversion/resampling.
 - [Deps]
   - To play audio with cpal, you’ll need cpal in radyo/Cargo.toml
*/
fn audio_stream(conn: Connection) {
    // Use spawn_blocking to avoid Send issues with cpal::Stream
    tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async move {
            if let Err(err) = process_audio_stream(conn).await {
                eprintln!("audio_stream error: {err}");
            }
        });
    });
}

async fn process_audio_stream(conn: Connection) -> Result<()> {
    
    // Accept a bi-stream:
    let (mut _send, mut rcv) = conn.accept_bi().await?;
    
    // Read file size first (4 bytes)
    let mut size_buf = [0u8; 4];
    rcv.read_exact(&mut size_buf).await?;
    let file_size = u32::from_le_bytes(size_buf) as usize;
    
    println!("📞 Incoming call! Receiving ringtone: {} bytes", file_size);
    
    // Read the entire MP3 file
    let mut mp3_data = vec![0u8; file_size];
    rcv.read_exact(&mut mp3_data).await?;
    
    println!("🎵 Playing incoming call ringtone...");
    
    // Play the MP3 using rodio properly on the caller side
    let (_stream, stream_handle) = rodio::OutputStream::try_default()?;
    let sink = std::sync::Arc::new(rodio::Sink::try_new(&stream_handle)?);
    
    // Create a cursor from the MP3 data and decode it
    let cursor = std::io::Cursor::new(mp3_data);
    let source = rodio::Decoder::new(cursor)?;
    
    sink.append(source);
    sink.set_volume(0.5); // 50% volume for incoming call
    
    println!("🔊 Ringtone playing on caller's device... (Press Ctrl+C to stop)");
    
    // Monitor for hangup signal from receiver
    let sink_clone = sink.clone();
    let connection_monitor = async move {
        let mut hangup_buf = [0u8; 6];
        match rcv.read_exact(&mut hangup_buf).await {
            Ok(_) if &hangup_buf == b"HANGUP" => {
                println!("📞 Received hangup signal - stopping ringtone");
                sink_clone.stop();
            }
            _ => {
                println!("📞 Connection lost - stopping ringtone");
                sink_clone.stop();
            }
        }
    };
    
    let sink_for_playback = sink.clone();
    let playback_monitor = async move {
        sink_for_playback.sleep_until_end();
        println!("📞 Call ringtone finished");
    };
    
    // Race between connection monitoring and playback completion
    tokio::select! {
        _ = connection_monitor => {
            println!("🔇 Ringtone stopped due to connection loss");
        }
        _ = playback_monitor => {
            println!("🎵 Ringtone completed normally");
        }
    }
    
    Ok(())
}

// LEGACY AUDIO STREAMING FUNCTIONS - COMMENTED OUT
// These functions were used for raw audio streaming but are not currently used
// The current implementation uses MP3 file transfer + rodio for playback

/*
// create a minimum, lock free ring buffer from an async QUIC stream to a realtime consumer

/// Spawn a task that reads little-endian f32 samples from an AsyncRead and pushes them
/// into a lock-free SPSC ring buffer. Returns the consumer end and the JoinHandle for the
/// producer task. The consumer can be used by a realtime audio callback to pop samples.
fn spawn_f32_recv_ring_from_quic(
    recv: RecvStream,
    capacity_samples: usize,
) -> (HeapCons<f32>, JoinHandle<anyhow::Result<()>>) {
    let rb = HeapRb::<f32>::new(capacity_samples);
    let (mut prod, cons): (HeapProd<f32>, HeapCons<f32>) = rb.split();

    let handle = tokio::spawn(async move {
        let mut recv = recv;
        let mut buf = [0u8; 4];
        loop {
            match recv.read_exact(&mut buf).await {
                Ok(_) => {
                    let s = f32::from_le_bytes(buf);
                    // Drop the newest on overflow
                    let _ = prod.try_push(s);
                }
                Err(e) => {
                    // For now, treat any error as EOF to gracefully handle stream end
                    eprintln!("Read error (treating as EOF): {e}");
                    break;
                }
            }
        }
        Ok(())
    });

    (cons, handle)
}

fn playback_stream(mut cons: HeapCons<f32>) -> Result<cpal::Stream> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("No default output audio device found"))?;
    let supported = device.default_output_config()?; // SupportedStreamConfig
    let channels = supported.channels() as usize;
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => {
            let cfg: cpal::StreamConfig = supported.clone().into();
            device.build_output_stream(
                &cfg,
                move |data: &mut [f32], _| {
                    for frame in data.chunks_mut(channels) {
                        let s = cons.try_pop().unwrap_or(0.0);
                        for sample in frame.iter_mut() { *sample = s; }
                    }
                },
                move |err| eprintln!("audio error: {err}"),
                None,
            )?
        }
        cpal::SampleFormat::I16 => {
            let cfg: cpal::StreamConfig = supported.clone().into();
            device.build_output_stream(
                &cfg,
                move |data: &mut [i16], _| {
                    for frame in data.chunks_mut(channels) {
                        let s = cons.try_pop().unwrap_or(0.0);
                        let s = (s * i16::MAX as f32) as i16;
                        for sample in frame.iter_mut() { *sample = s; }
                    }
                },
                move |err| eprintln!("audio error: {err}"),
                None,
            )?
        }
        cpal::SampleFormat::U16 => {
            let cfg: cpal::StreamConfig = supported.clone().into();
            device.build_output_stream(
                &cfg,
                move |data: &mut [u16], _| {
                    for frame in data.chunks_mut(channels) {
                        let s = cons.try_pop().unwrap_or(0.0);
                        let s = (((s + 1.0) * 0.5).clamp(0.0, 1.0) * u16::MAX as f32) as u16;
                        for sample in frame.iter_mut() { *sample = s; }
                    }
                },
                move |err| eprintln!("audio error: {err}"),
                None,
            )?
        }
        _ => unreachable!(),
    };
    Ok(stream)
}
*/