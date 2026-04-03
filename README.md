# Solana ShredStream SDK for Rust

Solana ShredStream SDK/Decoder for Rust, enabling ultra-low latency Solana transaction streaming via UDP shreds from ShredStream.com

> Part of the [ShredStream.com](https://shredstream.com) ecosystem — ultra-low latency [Solana shred streaming](https://shredstream.com) via UDP.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2021-DEA584?logo=rust&logoColor=white)](#)

## 📋 Prerequisites

1. **Create an account** on [ShredStream.com](https://shredstream.com)
2. **Launch a Shred Stream** and pick your region (Frankfurt, Amsterdam, Singapore, Chicago, and more)
3. **Enter your server's IP address** and the UDP port where you want to receive shreds
4. **Open your firewall** for inbound UDP traffic on that port (e.g. configure your cloud provider's security group)
5. Install [Rust](https://rustup.rs) and Cargo:
   ```bash
   # Linux / macOS
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   source $HOME/.cargo/env
   ```

> 🎁 Want to try before you buy? Open a ticket on our [Discord](https://discord.gg/4w2DNbTaWD) to request a free trial.

## 📦 Installation

```bash
# Initialize your project (skip if you already have a Cargo.toml)
cargo init myproject
cd myproject
```

Add `shredstream` to your `Cargo.toml`:

```toml
[dependencies]
shredstream = "1.0"
```

## ⚡ Quick Start

Edit `src/main.rs`:

```rust
use shredstream::ShredListener;

fn main() {
    let port: u16 = std::env::var("SHREDSTREAM_PORT")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(8001);
    let mut listener = ShredListener::bind(port).expect("bind");

    // Decoded transactions — ready-to-use Solana transactions
    for (slot, transactions) in listener.transactions() {
        for tx in &transactions {
            println!("slot {}: {}", slot, tx.signatures[0]);
        }
    }

    // OR raw shreds — lowest latency, arrives before block assembly
    // for shred in listener.shreds() {
    //     println!("slot {} index {} len {}", shred.slot, shred.index, shred.payload_len);
    // }
}
```

Run it:

```bash
cargo run
```

## 📖 API Reference

### `ShredListener`

- `ShredListener::bind(port: u16) -> io::Result<Self>` -- Bind with defaults (25 MB recv buf, 10 slot max age)
- `ShredListener::bind_with_options(port, opts) -> io::Result<Self>` -- Custom configuration
- `listener.transactions() -> TransactionIter` -- Blocking iterator yielding decoded transactions as they arrive
- `listener.shreds() -> ShredIter` -- Blocking iterator over `RawShred { slot, index, payload_len }`
- `listener.active_slots() -> usize` -- Number of slots currently being accumulated

### `ListenerOptions`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `recv_buf` | `usize` | 25 MB | UDP receive buffer size |
| `max_age` | `u64` | 10 | Maximum slot age before eviction |

## 🎯 Use Cases

ShredStream.com shred data powers a wide range of latency-sensitive strategies — HFT, MEV extraction, token sniping, copy trading, liquidation bots, on-chain analytics, and more.

### 💎 PumpFun Token Sniping

ShredStream.com SDK detects PumpFun token creations **~499ms before they appear on PumpFun's live feed** — tested across 25 consecutive detections:

<img src="https://raw.githubusercontent.com/shredstream/shredstream-sdk-rust/main/assets/shredstream.com_sdk_vs_pumpfun_live_feed.gif" alt="ShredStream.com SDK vs PumpFun live feed — ~499ms advantage" width="600">

> [ShredStream.com](https://shredstream.com) provides a complete, optimized PumpFun token creation detection code exclusively to Pro plan subscribers and above. Battle-tested, high-performance, ready to plug into your sniping pipeline. To get access, open a ticket on [Discord](https://discord.gg/4w2DNbTaWD) or reach out on Telegram [@shredstream](https://t.me/shredstream).

## ⚙️ Configuration

### OS Tuning

For high-throughput environments, increase the kernel receive buffer:

```bash
# Linux
sudo sysctl -w net.core.rmem_max=33554432
sudo sysctl -w net.core.rmem_default=33554432

# macOS
sudo sysctl -w kern.ipc.maxsockbuf=33554432
```

## 🚀 Launch a Shred Stream

Need a feed? **[Launch a Solana Shred Stream on ShredStream.com](https://shredstream.com)** — sub-millisecond delivery, multiple global regions, 5-minute setup.

## 🔗 Links

- 🌐 Website: https://www.shredstream.com/
- 📖 Documentation: https://docs.shredstream.com/
- 🐦 X (Twitter): https://x.com/ShredStream
- 🎮 Discord: https://discord.gg/4w2DNbTaWD
- 💬 Telegram: https://t.me/ShredStream
- 💻 GitHub: https://github.com/ShredStream
- 🎫 Support: [Discord](https://discord.gg/4w2DNbTaWD)
- 📊 Benchmarks: [Discord](https://discord.gg/4w2DNbTaWD)

## 📄 License

MIT — [ShredStream.com](https://shredstream.com)
