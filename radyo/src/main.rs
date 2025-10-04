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
use tokio::task::JoinHandle;

#[derive(Subcommand)]
enum Cmd {
    Caller,
    Receiver { token: String },
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

async fn receive(ticket: String) -> Result<()> {
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
    stream_ringtone(send).await?;
    Ok(())
}

async fn stream_ringtone(mut send: SendStream) -> Result<()> {
    let sample_rate = 48_000.0f32;
    let two_pi = core::f32::consts::TAU; // 2π
    
    // Ringtone melody: C5, E5, G5, C6 (523, 659, 784, 1047 Hz)
    let melody_freqs = [523.25, 659.25, 783.99, 1046.50];
    let note_duration_samples = (sample_rate * 0.5) as usize; // 500ms per note (longer)
    let pause_duration_samples = (sample_rate * 0.15) as usize; // 150ms pause
    let fade_samples = (sample_rate * 0.02) as usize; // 20ms fade in/out
    let chunk_len: usize = 480; // 10ms chunks
    let mut buf = vec![0u8; chunk_len * 4];
    
    let mut phase: f32 = 0.0;
    let mut chunk_counter = 0;

    println!("Streaming high-quality ringtone to peer (48kHz)...");
    loop {
        // Process entire chunk at once for better timing
        let chunk_start_sample = chunk_counter * chunk_len;
        
        for i in 0..chunk_len {
            let current_sample = chunk_start_sample + i;
            let samples_in_current_segment = current_sample % (note_duration_samples + pause_duration_samples);
            
            // Determine if we're in a note or pause
            let is_note_active = samples_in_current_segment < note_duration_samples;
            
            // Update current note based on total progress
            let current_note = (current_sample / (note_duration_samples + pause_duration_samples)) % melody_freqs.len();
            
            let s = if is_note_active {
                let freq = melody_freqs[current_note];
                
                // Calculate envelope for smooth transitions
                let envelope = if samples_in_current_segment < fade_samples {
                    // Fade in
                    samples_in_current_segment as f32 / fade_samples as f32
                } else if samples_in_current_segment > note_duration_samples - fade_samples {
                    // Fade out
                    (note_duration_samples - samples_in_current_segment) as f32 / fade_samples as f32
                } else {
                    // Full volume
                    1.0
                };
                
                // Generate sample with smooth phase continuation
                let sample = phase.sin() * 0.12 * envelope; // Slightly quieter with envelope
                phase += two_pi * freq / sample_rate;
                if phase >= two_pi { phase -= two_pi; }
                sample
            } else {
                // Pause - continue phase evolution to avoid clicks when resuming
                if current_sample % (note_duration_samples + pause_duration_samples) == note_duration_samples {
                    // Just entered pause, continue with next note's frequency for smooth transition
                    let next_note = (current_note + 1) % melody_freqs.len();
                    let freq = melody_freqs[next_note];
                    phase += two_pi * freq / sample_rate;
                    if phase >= two_pi { phase -= two_pi; }
                }
                0.0 // Silence during pause
            };
            
            buf[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
        }
        
        send.write_all(&buf).await?;
        chunk_counter += 1;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Cmd::Caller => call().await?,
        Cmd::Receiver { token } =>receive(token).await?,
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