use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use socket2::{Domain, Protocol, Socket, Type};
use solana_transaction::versioned::VersionedTransaction;

use crate::accumulator::{AccumulatorConfig, SlotAccumulator};
use crate::fec::ReedSolomonCache;
use crate::parser::{self, ParseError, ShredKind};
use crate::pool::{BufferHandle, ShredPool};

const DEFAULT_RECV_BUF: usize = 64 * 1024 * 1024;
const DEFAULT_MAX_AGE: u64 = 3;
const DEFAULT_POOL_SIZE: usize = 4096;
const MAX_PLAUSIBLE_SLOT: u64 = 1 << 40;
const MAX_SLOT_JUMP_FORWARD: u64 = 1_000_000;
const MAX_BOOTSTRAP_SLOT: u64 = 10_000_000_000;
const BLACKLIST_EXTRA_SLOTS: u64 = 50;
const COMPLETED_RETENTION_SLOTS: u64 = 1;

#[derive(Clone, Debug)]
pub struct ListenerOptions {
    pub recv_buf: usize,
    pub max_age: u64,
    pub busy_poll_us: Option<u32>,
    pub pool_size: usize,
    pub enable_fec: bool,
    pub disable_salvage_delivery: bool,
    pub accumulator: AccumulatorConfig,
}

impl Default for ListenerOptions {
    fn default() -> Self {
        Self {
            recv_buf: DEFAULT_RECV_BUF,
            max_age: DEFAULT_MAX_AGE,
            busy_poll_us: Some(200),
            pool_size: DEFAULT_POOL_SIZE,
            enable_fec: true,
            disable_salvage_delivery: false,
            accumulator: AccumulatorConfig::default(),
        }
    }
}

pub struct ShredListener {
    socket: std::net::UdpSocket,
    slots: FxHashMap<u64, SlotAccumulator>,
    recently_seen_slots: FxHashSet<u64>,
    pending_batches: VecDeque<(u64, Vec<VersionedTransaction>)>,
    max_age: u64,
    last_slot: u64,
    pool: Arc<ShredPool>,
    pool_exhausted: u64,
    rs_cache: ReedSolomonCache,
    enable_fec: bool,
    disable_salvage_delivery: bool,
    accumulator_cfg: AccumulatorConfig,
    data_shred_count: u64,
    code_shred_count: u64,
    bytes_received: u64,
    unparseable_too_short: u64,
    unparseable_variant: u64,
    unparseable_payload: u64,
    unparseable_slot_range: u64,
    dropped_known_slots: u64,
    decode_errors_total: u64,
    fec_recoveries_total: u64,
    fec_recovery_failures_total: u64,
    batches_skipped_total: u64,
    batches_decoded_streaming_total: u64,
    batches_decoded_fallback_total: u64,
    slots_completed_total: u64,
    slots_evicted_by_age: u64,
    harvested_batches_total: u64,
    salvaged_tail_tx_total: u64,
    fec_sets_discarded_unused_total: u64,
    fec_sets_evicted_early_total: u64,
    batches_force_finalized_corrupted_total: u64,
    batches_force_finalized_timeout_total: u64,
    last_io_error_kind: Option<io::ErrorKind>,
    busy_poll_active: bool,
}

impl ShredListener {
    pub fn bind(port: u16) -> io::Result<Self> {
        Self::bind_with_options(port, ListenerOptions::default())
    }

    pub fn bind_with_options(port: u16, opts: ListenerOptions) -> io::Result<Self> {
        let addr = SocketAddr::from(([0, 0, 0, 0], port));

        let raw = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        raw.set_reuse_address(true)?;
        raw.set_recv_buffer_size(opts.recv_buf)?;
        raw.set_nonblocking(false)?;
        raw.bind(&addr.into())?;

        let busy_poll_active = match opts.busy_poll_us {
            Some(us) => set_busy_poll(&raw, us).is_ok(),
            None => false,
        };

        let std_socket: std::net::UdpSocket = raw.into();
        let mut listener = Self::from_socket_inner(std_socket, opts)?;
        listener.busy_poll_active = busy_poll_active;
        Ok(listener)
    }

    pub fn from_socket(socket: std::net::UdpSocket, opts: ListenerOptions) -> io::Result<Self> {
        Self::from_socket_inner(socket, opts)
    }

    fn from_socket_inner(socket: std::net::UdpSocket, opts: ListenerOptions) -> io::Result<Self> {
        eprintln!(
            "\x1b[2m⚡ShredStream.com SDK v{} initiated\x1b[0m",
            env!("CARGO_PKG_VERSION")
        );
        let slots_cap = 32;
        let recently_seen_cap = 128;
        let pending_cap = 32;
        Ok(Self {
            socket,
            slots: FxHashMap::with_capacity_and_hasher(slots_cap, Default::default()),
            recently_seen_slots: FxHashSet::with_capacity_and_hasher(
                recently_seen_cap,
                Default::default(),
            ),
            pending_batches: VecDeque::with_capacity(pending_cap),
            max_age: opts.max_age,
            last_slot: 0,
            pool: ShredPool::new(opts.pool_size),
            pool_exhausted: 0,
            rs_cache: ReedSolomonCache::new(),
            enable_fec: opts.enable_fec,
            disable_salvage_delivery: opts.disable_salvage_delivery,
            accumulator_cfg: opts.accumulator,
            data_shred_count: 0,
            code_shred_count: 0,
            bytes_received: 0,
            unparseable_too_short: 0,
            unparseable_variant: 0,
            unparseable_payload: 0,
            unparseable_slot_range: 0,
            dropped_known_slots: 0,
            decode_errors_total: 0,
            fec_recoveries_total: 0,
            fec_recovery_failures_total: 0,
            batches_skipped_total: 0,
            batches_decoded_streaming_total: 0,
            batches_decoded_fallback_total: 0,
            slots_completed_total: 0,
            slots_evicted_by_age: 0,
            harvested_batches_total: 0,
            salvaged_tail_tx_total: 0,
            fec_sets_discarded_unused_total: 0,
            fec_sets_evicted_early_total: 0,
            batches_force_finalized_corrupted_total: 0,
            batches_force_finalized_timeout_total: 0,
            last_io_error_kind: None,
            busy_poll_active: false,
        })
    }

    pub fn transactions(&mut self) -> TransactionIter<'_> {
        TransactionIter { listener: self }
    }

    pub fn shreds(&mut self) -> ShredIter<'_> {
        ShredIter {
            listener: self,
            buf: [0u8; 2048],
        }
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    pub fn pool_exhausted_count(&self) -> u64 {
        self.pool_exhausted
    }

    pub fn last_io_error_kind(&self) -> Option<io::ErrorKind> {
        self.last_io_error_kind
    }

    pub fn busy_poll_active(&self) -> bool {
        self.busy_poll_active
    }

    pub fn data_shred_count_total(&self) -> u64 {
        self.data_shred_count
    }

    pub fn code_shred_count_total(&self) -> u64 {
        self.code_shred_count
    }

    pub fn bytes_received(&self) -> u64 {
        self.bytes_received
    }

    pub fn unparseable_packets(&self) -> u64 {
        self.unparseable_too_short
            .wrapping_add(self.unparseable_variant)
            .wrapping_add(self.unparseable_payload)
            .wrapping_add(self.unparseable_slot_range)
    }

    pub fn unparseable_too_short(&self) -> u64 {
        self.unparseable_too_short
    }

    pub fn unparseable_variant(&self) -> u64 {
        self.unparseable_variant
    }

    pub fn unparseable_payload(&self) -> u64 {
        self.unparseable_payload
    }

    pub fn unparseable_slot_range(&self) -> u64 {
        self.unparseable_slot_range
    }

    pub fn dropped_known_slots(&self) -> u64 {
        self.dropped_known_slots
    }

    pub fn harvested_batches_total(&self) -> u64 {
        self.harvested_batches_total
    }

    pub fn decode_errors_total(&self) -> u64 {
        self.decode_errors_total + self.sum_live(|a| a.decode_errors() as u64)
    }

    pub fn fec_recoveries_total(&self) -> u64 {
        self.fec_recoveries_total + self.sum_live(|a| a.recoveries_done())
    }

    pub fn fec_recovery_failures_total(&self) -> u64 {
        self.fec_recovery_failures_total + self.sum_live(|a| a.recovery_failures())
    }

    pub fn batches_skipped_total(&self) -> u64 {
        self.batches_skipped_total + self.sum_live(|a| a.batches_skipped())
    }

    pub fn batches_decoded_streaming_total(&self) -> u64 {
        self.batches_decoded_streaming_total
            + self.sum_live(|a| a.batches_decoded_streaming())
    }

    pub fn batches_decoded_fallback_total(&self) -> u64 {
        self.batches_decoded_fallback_total
            + self.sum_live(|a| a.batches_decoded_fallback())
    }

    pub fn slots_completed_total(&self) -> u64 {
        self.slots_completed_total
    }

    pub fn slots_evicted_by_age(&self) -> u64 {
        self.slots_evicted_by_age
    }

    pub fn salvaged_tail_tx_total(&self) -> u64 {
        self.salvaged_tail_tx_total + self.sum_live(|a| a.salvaged_tail_tx())
    }

    pub fn fec_sets_discarded_unused_total(&self) -> u64 {
        self.fec_sets_discarded_unused_total
            + self.sum_live(|a| a.fec_sets_discarded_unused())
    }

    pub fn fec_sets_evicted_early_total(&self) -> u64 {
        self.fec_sets_evicted_early_total
            + self.sum_live(|a| a.fec_sets_evicted_early())
    }

    pub fn batches_force_finalized_corrupted_total(&self) -> u64 {
        self.batches_force_finalized_corrupted_total
            + self.sum_live(|a| a.batches_force_finalized_corrupted())
    }

    pub fn batches_force_finalized_timeout_total(&self) -> u64 {
        self.batches_force_finalized_timeout_total
            + self.sum_live(|a| a.batches_force_finalized_timeout())
    }

    fn sum_live<F: Fn(&SlotAccumulator) -> u64>(&self, f: F) -> u64 {
        self.slots.values().map(f).sum()
    }

    fn rollup_accumulator(&mut self, acc: &SlotAccumulator) {
        self.decode_errors_total = self
            .decode_errors_total
            .wrapping_add(acc.decode_errors() as u64);
        self.fec_recoveries_total = self
            .fec_recoveries_total
            .wrapping_add(acc.recoveries_done());
        self.fec_recovery_failures_total = self
            .fec_recovery_failures_total
            .wrapping_add(acc.recovery_failures());
        self.batches_skipped_total = self
            .batches_skipped_total
            .wrapping_add(acc.batches_skipped());
        self.batches_decoded_streaming_total = self
            .batches_decoded_streaming_total
            .wrapping_add(acc.batches_decoded_streaming());
        self.batches_decoded_fallback_total = self
            .batches_decoded_fallback_total
            .wrapping_add(acc.batches_decoded_fallback());
        self.salvaged_tail_tx_total = self
            .salvaged_tail_tx_total
            .wrapping_add(acc.salvaged_tail_tx());
        self.fec_sets_discarded_unused_total = self
            .fec_sets_discarded_unused_total
            .wrapping_add(acc.fec_sets_discarded_unused());
        self.fec_sets_evicted_early_total = self
            .fec_sets_evicted_early_total
            .wrapping_add(acc.fec_sets_evicted_early());
        self.batches_force_finalized_corrupted_total = self
            .batches_force_finalized_corrupted_total
            .wrapping_add(acc.batches_force_finalized_corrupted());
        self.batches_force_finalized_timeout_total = self
            .batches_force_finalized_timeout_total
            .wrapping_add(acc.batches_force_finalized_timeout());
    }

    pub fn handle_packet(&mut self, raw: &[u8]) -> Option<(u64, Vec<VersionedTransaction>)> {
        let mut handle = match self.acquire_buffer_with_fallback() {
            Some(h) => h,
            None => {
                self.pool_exhausted = self.pool_exhausted.wrapping_add(1);
                return None;
            }
        };
        let n = raw.len().min(handle.as_mut_slice().len());
        handle.as_mut_slice()[..n].copy_from_slice(&raw[..n]);
        handle.set_len(n);
        self.handle_packet_owned(handle)
    }

    fn handle_packet_owned(
        &mut self,
        handle: BufferHandle,
    ) -> Option<(u64, Vec<VersionedTransaction>)> {
        self.bytes_received = self.bytes_received.wrapping_add(handle.len() as u64);

        let kind = match parser::parse_kind(&handle) {
            Ok(k) => k,
            Err(e) => {
                match e {
                    ParseError::TooShort => {
                        self.unparseable_too_short =
                            self.unparseable_too_short.wrapping_add(1);
                    }
                    ParseError::UnknownVariant => {
                        self.unparseable_variant = self.unparseable_variant.wrapping_add(1);
                    }
                    ParseError::PayloadInvalid => {
                        self.unparseable_payload = self.unparseable_payload.wrapping_add(1);
                    }
                }
                return None;
            }
        };

        if let ShredKind::Data(d) = &kind {
            let slot = d.slot;
            let index = d.index;
            let fec_set_index = d.fec_set_index;
            let payload_end = (parser::DATA_HEADER_SIZE + d.payload.len()) as u16;
            let batch_complete = d.batch_complete;
            let last_in_slot = d.last_in_slot;
            drop(kind);

            if slot > MAX_PLAUSIBLE_SLOT
                || (self.last_slot == 0 && slot > MAX_BOOTSTRAP_SLOT)
                || (self.last_slot > 0
                    && slot > self.last_slot.saturating_add(MAX_SLOT_JUMP_FORWARD))
            {
                self.unparseable_slot_range = self.unparseable_slot_range.wrapping_add(1);
                return None;
            }
            if self.recently_seen_slots.contains(&slot) {
                self.dropped_known_slots = self.dropped_known_slots.wrapping_add(1);
                return None;
            }

            self.data_shred_count = self.data_shred_count.wrapping_add(1);
            if slot > self.last_slot {
                self.last_slot = slot;
                self.evict_old_slots();
            }
            debug_assert!(self.last_slot <= MAX_PLAUSIBLE_SLOT);

            let cfg = self.accumulator_cfg;
            let acc = self
                .slots
                .entry(slot)
                .or_insert_with(|| SlotAccumulator::with_config(cfg));
            let txs = acc.push_data(
                index,
                fec_set_index,
                handle,
                payload_end,
                batch_complete,
                last_in_slot,
                &mut self.rs_cache,
            );

            return if txs.is_empty() {
                None
            } else {
                Some((slot, txs))
            };
        }

        let ShredKind::Code(c) = kind else {
            return None;
        };

        let slot = c.slot;
        if slot > MAX_PLAUSIBLE_SLOT
            || (self.last_slot == 0 && slot > MAX_BOOTSTRAP_SLOT)
            || (self.last_slot > 0 && slot > self.last_slot.saturating_add(MAX_SLOT_JUMP_FORWARD))
        {
            self.unparseable_slot_range = self.unparseable_slot_range.wrapping_add(1);
            return None;
        }
        if self.recently_seen_slots.contains(&slot) {
            self.dropped_known_slots = self.dropped_known_slots.wrapping_add(1);
            return None;
        }

        self.code_shred_count = self.code_shred_count.wrapping_add(1);
        if !self.enable_fec {
            return None;
        }
        if slot > self.last_slot {
            self.last_slot = slot;
            self.evict_old_slots();
        }
        debug_assert!(self.last_slot <= MAX_PLAUSIBLE_SLOT);

        let cfg = self.accumulator_cfg;
        let acc = self
            .slots
            .entry(slot)
            .or_insert_with(|| SlotAccumulator::with_config(cfg));
        let txs = acc.push_code(
            c.fec_set_index,
            c.num_data_shreds,
            c.num_coding_shreds,
            c.position,
            c.coded,
            c.variant,
            &mut self.rs_cache,
        );

        if txs.is_empty() {
            None
        } else {
            Some((slot, txs))
        }
    }

    fn evict_old_slots(&mut self) {
        let last = self.last_slot;
        let max_age = self.max_age;
        let to_evict: Vec<u64> = self
            .slots
            .iter()
            .filter_map(|(&s, acc)| {
                let threshold = if acc.slot_complete {
                    COMPLETED_RETENTION_SLOTS
                } else {
                    max_age
                };
                if last > s.saturating_add(threshold) {
                    Some(s)
                } else {
                    None
                }
            })
            .collect();
        if !to_evict.is_empty() {
            for s in to_evict {
                if let Some(mut acc) = self.slots.remove(&s) {
                    let tail_txs = acc.try_harvest_tail(&mut self.rs_cache);
                    if !tail_txs.is_empty() {
                        self.harvested_batches_total =
                            self.harvested_batches_total.wrapping_add(1);
                        if !self.disable_salvage_delivery {
                            self.pending_batches.push_back((s, tail_txs));
                        }
                    }
                    if acc.slot_complete {
                        self.slots_completed_total =
                            self.slots_completed_total.wrapping_add(1);
                    } else {
                        self.slots_evicted_by_age =
                            self.slots_evicted_by_age.wrapping_add(1);
                    }
                    self.rollup_accumulator(&acc);
                    self.recently_seen_slots.insert(s);
                }
            }
        }

        let blacklist_floor = self
            .last_slot
            .saturating_sub(self.max_age + BLACKLIST_EXTRA_SLOTS);
        if !self.recently_seen_slots.is_empty() {
            self.recently_seen_slots.retain(|&s| s >= blacklist_floor);
        }
    }

    fn recv_one(&mut self) -> io::Result<Option<(u64, Vec<VersionedTransaction>)>> {
        if let Some(batch) = self.pending_batches.pop_front() {
            return Ok(Some(batch));
        }
        let mut handle = match self.acquire_buffer_with_fallback() {
            Some(h) => h,
            None => {
                self.pool_exhausted = self.pool_exhausted.wrapping_add(1);
                let mut dummy = [0u8; 2048];
                let _ = self.socket.recv(&mut dummy)?;
                return Ok(None);
            }
        };
        let len = self.socket.recv(handle.as_mut_slice())?;
        handle.set_len(len);
        Ok(self.handle_packet_owned(handle))
    }

    fn acquire_buffer_with_fallback(&mut self) -> Option<BufferHandle> {
        if let Some(h) = self.pool.acquire() {
            return Some(h);
        }
        self.evict_old_slots();
        if let Some(h) = self.pool.acquire() {
            return Some(h);
        }
        self.force_evict_oldest_slot();
        self.pool.acquire()
    }

    fn force_evict_oldest_slot(&mut self) {
        let oldest = self.slots.keys().copied().min();
        if let Some(s) = oldest {
            if let Some(mut acc) = self.slots.remove(&s) {
                let tail_txs = acc.try_harvest_tail(&mut self.rs_cache);
                if !tail_txs.is_empty() {
                    self.harvested_batches_total =
                        self.harvested_batches_total.wrapping_add(1);
                    if !self.disable_salvage_delivery {
                        self.pending_batches.push_back((s, tail_txs));
                    }
                }
                if acc.slot_complete {
                    self.slots_completed_total =
                        self.slots_completed_total.wrapping_add(1);
                } else {
                    self.slots_evicted_by_age =
                        self.slots_evicted_by_age.wrapping_add(1);
                }
                self.rollup_accumulator(&acc);
                self.recently_seen_slots.insert(s);
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn set_busy_poll(socket: &Socket, us: u32) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let fd = socket.as_raw_fd();
    let val = us as libc::c_int;
    let ret = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_BUSY_POLL,
            &val as *const _ as *const libc::c_void,
            std::mem::size_of_val(&val) as libc::socklen_t,
        )
    };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
fn set_busy_poll(_socket: &Socket, _us: u32) -> io::Result<()> {
    Ok(())
}

pub struct RawShred {
    pub slot: u64,
    pub index: u32,
    pub payload_len: usize,
}

pub struct TransactionIter<'a> {
    listener: &'a mut ShredListener,
}

impl TransactionIter<'_> {
    pub fn listener(&self) -> &ShredListener {
        self.listener
    }
}

impl Iterator for TransactionIter<'_> {
    type Item = (u64, Vec<VersionedTransaction>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.listener.recv_one() {
                Ok(Some(batch)) => return Some(batch),
                Ok(None) => continue,
                Err(e) => {
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                            | io::ErrorKind::TimedOut
                    ) {
                        continue;
                    }
                    self.listener.last_io_error_kind = Some(e.kind());
                    return None;
                }
            }
        }
    }
}

pub struct ShredIter<'a> {
    listener: &'a mut ShredListener,
    buf: [u8; 2048],
}

impl Iterator for ShredIter<'_> {
    type Item = RawShred;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let len = match self.listener.socket.recv(&mut self.buf) {
                Ok(n) => n,
                Err(e) => {
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                            | io::ErrorKind::TimedOut
                    ) {
                        continue;
                    }
                    self.listener.last_io_error_kind = Some(e.kind());
                    return None;
                }
            };
            let raw = &self.buf[..len];
            if let Some(shred) = parser::parse_shred(raw) {
                return Some(RawShred {
                    slot: shred.slot,
                    index: shred.index,
                    payload_len: shred.payload.len(),
                });
            }
        }
    }
}
