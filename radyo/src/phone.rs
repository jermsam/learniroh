use anyhow::Result;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use std::path::Path;
use tokio::io::AsyncReadExt;

// Phone call protocol messages
#[derive(Debug, Clone)]
pub enum CallMessage {
    IncomingCall,           // Peer → Caller: "Someone is calling you"
    CallAnswered,           // Caller → Peer: "I picked up the call"
    CallDeclined,           // Caller → Peer: "I declined the call"
    CallNotAnswered,        // Caller → Peer: "Call timed out, not answered"
    Hangup,                 // Either → Other: "I'm hanging up"
    VoiceData { size: u32 }, // Either → Other: Voice data packet (future use)
}

impl CallMessage {
    pub async fn send(&self, stream: &mut SendStream) -> Result<()> {
        match self {
            CallMessage::IncomingCall => stream.write_all(&[0u8]).await?,
            CallMessage::CallAnswered => stream.write_all(&[1u8]).await?,
            CallMessage::CallDeclined => stream.write_all(&[2u8]).await?,
            CallMessage::CallNotAnswered => stream.write_all(&[3u8]).await?,
            CallMessage::Hangup => stream.write_all(&[4u8]).await?,
            CallMessage::VoiceData { size } => {
                stream.write_all(&[5u8]).await?;
                stream.write_all(&size.to_le_bytes()).await?;
            }
        }
        Ok(())
    }

    pub async fn recv(stream: &mut RecvStream) -> Result<Self> {
        let mut msg_type = [0u8; 1];
        stream.read_exact(&mut msg_type).await?;
        
        match msg_type[0] {
            0 => Ok(CallMessage::IncomingCall),
            1 => Ok(CallMessage::CallAnswered),
            2 => Ok(CallMessage::CallDeclined),
            3 => Ok(CallMessage::CallNotAnswered),
            4 => Ok(CallMessage::Hangup),
            5 => {
                let mut size_bytes = [0u8; 4];
                stream.read_exact(&mut size_bytes).await?;
                let size = u32::from_le_bytes(size_bytes);
                Ok(CallMessage::VoiceData { size })
            }
            _ => Err(anyhow::anyhow!("Unknown message type: {}", msg_type[0])),
        }
    }
}

// Call states for managing the phone call workflow
#[derive(Debug, Clone, PartialEq)]
pub enum CallState {
    Idle,                   // No active call
    Ringing,               // Incoming call, ringtone playing
    InCall,                // Active call, two-way communication
    CallEnded,             // Call finished
}

// Ringtone management
pub struct Ringtone {
    pub name: String,
    pub data: Vec<u8>,
}

impl Ringtone {
    pub fn load(name: &str) -> Result<Self> {
        let file_path = format!("ringtons/{}.mp3", name);
        let data = if Path::new(&file_path).exists() {
            std::fs::read(&file_path)?
        } else {
            println!("Ringtone '{}' not found, using lost_woods.mp3", name);
            std::fs::read("ringtons/lost_woods.mp3")?
        };
        
        Ok(Ringtone {
            name: name.to_string(),
            data,
        })
    }

    pub fn play(&self) -> Result<rodio::Sink> {
        let (_stream, stream_handle) = rodio::OutputStream::try_default()?;
        let sink = rodio::Sink::try_new(&stream_handle)?;
        
        let cursor = std::io::Cursor::new(self.data.clone());
        let source = rodio::Decoder::new(cursor)?;
        
        sink.append(source);
        sink.set_volume(0.5);
        
        println!("🎵 Playing ringtone: {}", self.name);
        Ok(sink)
    }
}

// Phone call session manager
pub struct CallSession {
    pub state: CallState,
    pub connection: Connection,
    pub send_stream: SendStream,
    pub recv_stream: RecvStream,
}

impl CallSession {
    pub fn new(connection: Connection, send_stream: SendStream, recv_stream: RecvStream) -> Self {
        Self {
            state: CallState::Idle,
            connection,
            send_stream,
            recv_stream,
        }
    }

    pub async fn send_message(&mut self, message: CallMessage) -> Result<()> {
        println!("📤 Sending: {:?}", message);
        message.send(&mut self.send_stream).await
    }

    pub async fn recv_message(&mut self) -> Result<CallMessage> {
        let message = CallMessage::recv(&mut self.recv_stream).await?;
        println!("📥 Received: {:?}", message);
        Ok(message)
    }

    pub fn set_state(&mut self, state: CallState) {
        println!("📞 Call state: {:?} → {:?}", self.state, state);
        self.state = state;
    }
}
