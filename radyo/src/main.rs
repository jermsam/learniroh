use anyhow::Result;
use clap::{Parser, Subcommand};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, NodeAddr};
use iroh_base::ticket::NodeTicket;
use ringbuf::{HeapCons, HeapProd, HeapRb};
use ringbuf::traits::{Consumer, Producer, Split};
use std::future::Future;
use std::path::Path;
use tokio::task::JoinHandle;

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
    println!("Loading ringtone '{}'...", ringtone);
    
    // Load audio with fallback logic
    let audio_samples = load_ringtone_with_fallback(&ringtone)?;
    
    const CHUNK_LEN: usize = 480; // 10ms chunks at 48kHz
    let mut buf = vec![0u8; CHUNK_LEN * 4];
    let mut sample_index = 0;
    let samples_len = audio_samples.len();
    
    println!("Streaming ringtone to peer (48kHz)...");
    loop {
        // Pre-calculate modulo outside the loop for better performance
        let start_idx = sample_index % samples_len;
        
        for i in 0..CHUNK_LEN {
            let sample = audio_samples[(start_idx + i) % samples_len];
            buf[i * 4..i * 4 + 4].copy_from_slice(&sample.to_le_bytes());
        }
        
        send.write_all(&buf).await?;
        sample_index = (sample_index + CHUNK_LEN) % samples_len;
    }
}

fn load_ringtone_with_fallback(ringtone: &str) -> Result<Vec<f32>> {
    let ringtone_path = format!("ringtons/{}.mp3", ringtone);
    
    // Try requested ringtone first
    if Path::new(&ringtone_path).exists() {
        match load_audio_file(&ringtone_path) {
            Ok(samples) => {
                println!("Successfully loaded ringtone '{}': {} samples", ringtone, samples.len());
                return Ok(samples);
            }
            Err(e) => {
                println!("Failed to load {}: {}", ringtone_path, e);
            }
        }
    } else {
        println!("Ringtone '{}' not found", ringtone);
    }
    
    // Fallback to lost_woods.mp3
    println!("Using default lost_woods.mp3");
    load_audio_file("ringtons/lost_woods.mp3")
        .map_err(|e| anyhow::anyhow!("Failed to load fallback lost_woods.mp3: {}", e))
}

fn load_audio_file(path: &str) -> Result<Vec<f32>> {
    use rodio::Source;
    use std::fs::File;
    use std::io::BufReader;
    
    // Open the file
    let file = File::open(path)?;
    let source = rodio::Decoder::new(BufReader::new(file))?;
    
    // Collect all samples and convert to f32
    let samples: Vec<f32> = source.convert_samples().collect();
    
    if samples.is_empty() {
        return Err(anyhow::anyhow!("No audio data found in file"));
    }
    
    println!("Loaded audio file: {} samples", samples.len());
    Ok(samples)
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
    let (mut _send, rcv) = conn.accept_bi().await?;
    // Larger buffer: ~200ms @ 48kHz mono for better network jitter handling
    let (cons, handle) = spawn_f32_recv_ring_from_quic(rcv, 9600);
    
    // Create and start the audio stream
    let stream = playback_stream(cons)?;
    stream.play()?;
    
    // Wait for the producer to finish
    let result = handle.await?;
    
    // Keep the stream alive until we're done
    std::mem::drop(stream);
    
    result
}

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