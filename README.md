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
shredstream = "3.0"
```

> 3.0.0 delivers version 1 transactions and moves to `solana-transaction` 4.x.
> Update your own `solana-transaction` dependency to 4.x and handle
> `VersionedMessage::V1` where you match on message versions.

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
}
```

Run it:

```bash
cargo run
```

## 📖 API Reference

### `ShredListener`

- `ShredListener::bind(port: u16) -> io::Result<Self>` — Bind with defaults (64 MB recv buf, 3 slot window, FEC enabled)
- `ShredListener::bind_with_options(port, opts) -> io::Result<Self>` — Custom configuration
- `ShredListener::from_socket(socket, opts) -> io::Result<Self>` — Adopt an existing `UdpSocket`
- `listener.transactions() -> TransactionIter` — Blocking iterator yielding `(slot, Vec<VersionedTransaction>)`
- `listener.shreds() -> ShredIter` — Blocking iterator yielding `RawShred` headers (no decode)
- `listener.handle_packet(&[u8]) -> Option<(u64, Vec<VersionedTransaction>)>` — Inject an externally-received UDP datagram
- `listener.local_addr() -> io::Result<SocketAddr>` — Bound socket address
- `listener.slot_count() -> usize` — Number of slots currently active in the window

### `ListenerOptions`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `recv_buf` | `usize` | `64 * 1024 * 1024` | `SO_RCVBUF` size |
| `max_age` | `u64` | `3` | Slot retention window |
| `busy_poll_us` | `Option<u32>` | `Some(200)` | Linux `SO_BUSY_POLL` µs (`None` disables) |
| `pool_size` | `usize` | `4096` | Number of 2 KiB buffers in the zero-copy pool |
| `enable_fec` | `bool` | `true` | Reed-Solomon recovery on dropped data shreds |
| `disable_salvage_delivery` | `bool` | `false` | Drop salvaged tail txs for lowest p99 |
| `accumulator` | `AccumulatorConfig` | *defaults* | FEC and stuck-batch tuning |

### `AccumulatorConfig`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_fec_sets_per_slot` | `usize` | `32` | Per-slot FEC buffer cap |
| `stuck_batch_timeout` | `Duration` | `50ms` | Force-finalize a stuck batch after this delay |

### Metrics

Read-only counters on `&ShredListener`:

| Group | Methods |
|-------|---------|
| **Throughput** | `data_shred_count_total`, `code_shred_count_total`, `bytes_received`, `slot_count` |
| **Decoder** | `batches_decoded_streaming_total`, `batches_decoded_fallback_total`, `batches_skipped_total`, `decode_errors_total` |
| **FEC** | `fec_recoveries_total`, `fec_recovery_failures_total`, `fec_sets_discarded_unused_total`, `fec_sets_evicted_early_total` |
| **Unparseable** | `unparseable_packets`, `unparseable_too_short`, `unparseable_variant`, `unparseable_payload`, `unparseable_slot_range` |
| **Slot lifecycle** | `slots_completed_total`, `slots_evicted_by_age`, `dropped_known_slots`, `harvested_batches_total`, `salvaged_tail_tx_total` |
| **Tail control** | `batches_force_finalized_corrupted_total`, `batches_force_finalized_timeout_total` |
| **Pool / I-O** | `pool_exhausted_count`, `last_io_error_kind`, `busy_poll_active` |

### Helpers

- `shredstream::classify_variant(byte) -> Option<VariantKind>` — Classify a shred variant byte. `VariantKind` exposes `.is_data()`, `.is_code()`, `.proof_size()`, `.resigned()`, `.merkle_suffix()`.
- `shredstream::pin_current_thread_to_cpu(cpu_id: usize) -> io::Result<()>` — Best-effort thread pinning (Linux: `sched_setaffinity`; macOS: hint; other: no-op)

## 🎯 Use Cases

ShredStream.com shred data powers a wide range of latency-sensitive strategies — HFT, MEV extraction, token sniping, copy trading, liquidation bots, on-chain analytics, and more.

### 💎 PumpFun Token Sniping

ShredStream.com SDK detects PumpFun token creations **~499ms before they appear on PumpFun's live feed** — tested across 25 consecutive detections:

<img src="https://raw.githubusercontent.com/shredstream/shredstream-sdk-rust/main/assets/shredstream.com_sdk_vs_pumpfun_live_feed.gif" alt="ShredStream.com SDK vs PumpFun live feed — ~499ms advantage" width="600">

> Ready-to-run example included: see [`examples/pumpfun_creates.rs`](examples/pumpfun_creates.rs). Run with `cargo run --release --example pumpfun_creates [port]`.

## ⚙️ Configuration

### OS Tuning

For high-throughput environments, increase the kernel receive buffer:

```bash
# Linux
sudo sysctl -w net.core.rmem_max=67108864
sudo sysctl -w net.core.busy_read=200

# macOS
sudo sysctl -w kern.ipc.maxsockbuf=67108864
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
