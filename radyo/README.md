# 📞 Radyo - P2P Voice Call System

A peer-to-peer voice calling application built with Rust and the Iroh networking library. Radyo enables direct voice calls between nodes without requiring central servers.

## 🚀 Features

- **📱 P2P Voice Calls**: Direct peer-to-peer communication using Iroh
- **🎵 Custom Ringtones**: Configurable MP3 ringtones for incoming calls
- **🔄 Concurrent Calls**: Handle multiple call sessions simultaneously
- **🛡️ Call Management**: Busy signals, hangup acknowledgments, and proper cleanup
- **⚡ Async Architecture**: Built with Tokio for high-performance async I/O
- **🧩 Modular Design**: Clean, testable, and maintainable code structure

## 📊 Project Statistics

```
Language: Rust
Modules: 6
Lines of Code: ~400 (down from 418 in single file)
Dependencies: iroh, tokio, rodio, clap, anyhow
Architecture: Modular, async-first
```

## 🏗️ Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                        Radyo Application                     │
├─────────────────────────────────────────────────────────────┤
│  main.rs (11 lines)                                        │
│  ├── CLI parsing & mode dispatch                           │
│  └── Application entry point                               │
├─────────────────────────────────────────────────────────────┤
│  lib.rs                                                    │
│  ├── Module declarations                                   │
│  ├── Public API exports                                    │
│  └── Common type aliases                                   │
├─────────────────────────────────────────────────────────────┤
│  cli.rs          │  protocol.rs     │  modes.rs           │
│  ├── Cmd enum   │  ├── RadyoProtocol│  ├── caller_mode() │
│  └── Cli struct │  └── ALPN const   │  └── peer_mode()   │
├─────────────────────────────────────────────────────────────┤
│  call.rs                          │  audio.rs            │
│  ├── CallManager (state)          │  ├── AudioManager    │
│  ├── CallState (sessions)         │  ├── Ringtone loading│
│  ├── Global state management      │  ├── Playback control│
│  └── Call handling logic          │  └── Stop signaling  │
└─────────────────────────────────────────────────────────────┘
```

## 🚀 Quick Start

### Prerequisites
- Rust 1.70+ with Cargo
- Audio system (for ringtone playback)
- Network connectivity

### Installation & Usage

1. **Clone and build**:
   ```bash
   git clone <repository>
   cd radyo
   cargo build --release
   ```

2. **Start as caller (receive calls)**:
   ```bash
   cargo run -- caller [ringtone_name]
   # Example: cargo run -- caller lost_woods
   ```

3. **Call someone (peer mode)**:
   ```bash
   cargo run -- peer <node_ticket>
   ```

### Example Workflow

```bash
# Terminal 1: Start phone service
$ cargo run -- caller
📞 Starting persistent phone service with ringtone: lost_woods
📱 Your Contact Card (Node Ticket): <long_ticket_string>
📞 Phone service is now online - waiting for calls...

# Terminal 2: Call the first terminal
$ cargo run -- peer <ticket_from_terminal_1>
📞 Starting peer mode - calling: <ticket>
✅ Call initiated - caller should be ringing now
⏳ Press Ctrl+C to hang up the call...
```

## 📁 Project Structure

```
radyo/
├── src/
│   ├── main.rs           # 📍 Entry point (11 lines)
│   ├── lib.rs            # 📚 Library exports
│   ├── cli.rs            # 🖥️  CLI definitions
│   ├── protocol.rs       # 🌐 Network protocol
│   ├── call.rs           # 📞 Call management
│   ├── audio.rs          # 🎵 Audio playback
│   └── modes.rs          # 🔄 App modes
├── ringtons/             # 🎶 Ringtone files (.mp3)
├── Cargo.toml           # 📦 Dependencies
├── README.md            # 📖 This file
└── MODULAR_STRUCTURE.md # 🏗️  Architecture docs
```

## 🔧 Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `iroh` | Latest | P2P networking and connections |
| `tokio` | Latest | Async runtime and I/O |
| `rodio` | Latest | Audio playback for ringtones |
| `clap` | Latest | Command-line argument parsing |
| `anyhow` | Latest | Error handling |

## 🎵 Ringtone Setup

1. Create a `ringtons/` directory in the project root
2. Add MP3 files (e.g., `lost_woods.mp3`, `nokia.mp3`)
3. Use the filename (without extension) as the ringtone parameter

```bash
mkdir ringtons
# Add your .mp3 files here
cargo run -- caller my_ringtone
```

## 🧪 Testing

```bash
# Check compilation
cargo check

# Run with different ringtones
cargo run -- caller nokia
cargo run -- caller lost_woods

# Test CLI help
cargo run -- --help
cargo run -- caller --help
```

## 🔍 Module Breakdown

| Module | Lines | Responsibility | Key Types |
|--------|-------|----------------|-----------|
| `main.rs` | 11 | Entry point | `main()` |
| `cli.rs` | 15 | CLI parsing | `Cli`, `Cmd` |
| `protocol.rs` | 20 | Network protocol | `RadyoProtocol` |
| `call.rs` | 250 | Call management | `CallManager`, `CallState` |
| `audio.rs` | 80 | Audio playback | `AudioManager` |
| `modes.rs` | 90 | App modes | `caller_mode()`, `peer_mode()` |

## 🚧 Development

### Adding New Features

1. **New CLI commands**: Modify `src/cli.rs`
2. **Audio formats**: Extend `src/audio.rs`
3. **Protocol features**: Update `src/protocol.rs`
4. **Call features**: Enhance `src/call.rs`

### Code Quality

- ✅ Modular architecture
- ✅ Async-first design
- ✅ Error handling with `anyhow`
- ✅ Thread-safe state management
- ✅ Clean separation of concerns

## 📚 Documentation

- **Architecture**: See [MODULAR_STRUCTURE.md](MODULAR_STRUCTURE.md)
- **API Docs**: Run `cargo doc --open`
- **Examples**: Check the usage section above

## 🤝 Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes following the modular structure
4. Test with `cargo test` and `cargo check`
5. Submit a pull request

## 📄 License

[Add your license here]
