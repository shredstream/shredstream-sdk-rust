use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use rustc_hash::FxHashSet;
use solana_transaction::versioned::VersionedTransaction;

use crate::decoder::{decode_batch, decode_concatenated, StreamingDecoder};
use crate::fec::{FecSetBuffer, ReedSolomonCache};
use crate::parser::{self, DATA_HEADER_SIZE};
use crate::pool::BufferHandle;
use crate::variant::VariantKind;

const DEFAULT_MAX_FEC_SETS_PER_SLOT: usize = 32;
const DEFAULT_STUCK_BATCH_TIMEOUT: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug)]
pub struct AccumulatorConfig {
    pub max_fec_sets_per_slot: usize,
    pub stuck_batch_timeout: Duration,
}

impl Default for AccumulatorConfig {
    fn default() -> Self {
        Self {
            max_fec_sets_per_slot: DEFAULT_MAX_FEC_SETS_PER_SLOT,
            stuck_batch_timeout: DEFAULT_STUCK_BATCH_TIMEOUT,
        }
    }
}

enum PayloadSource {
    Pool(BufferHandle),
    Recovered(Vec<u8>),
}

impl PayloadSource {
    fn slice(&self) -> &[u8] {
        match self {
            Self::Pool(h) => &h[..],
            Self::Recovered(v) => v,
        }
    }
}

struct PendingShred {
    source: PayloadSource,
    payload_end: u16,
    batch_complete: bool,
    fec_set_index: u32,
}

impl PendingShred {
    fn payload(&self) -> Option<&[u8]> {
        let buf = self.source.slice();
        let end = self.payload_end as usize;
        if end < DATA_HEADER_SIZE || end > buf.len() {
            return None;
        }
        Some(&buf[DATA_HEADER_SIZE..end])
    }
}

pub struct SlotAccumulator {
    cfg: AccumulatorConfig,
    pending: BTreeMap<u32, PendingShred>,
    fec_sets: BTreeMap<u32, FecSetBuffer>,

    batch_complete_indices: BTreeMap<u32, Instant>,

    streaming_cursor: u32,
    streaming_decoder: StreamingDecoder,
    yielded_sigs: FxHashSet<[u8; 64]>,

    batch_start: u32,

    pub slot_complete: bool,

    decode_errors: u32,
    recoveries_done: u64,
    recovery_failures: u64,
    batches_decoded_streaming: u64,
    batches_decoded_fallback: u64,
    batches_skipped: u64,
    fec_sets_discarded_unused: u64,
    fec_sets_evicted_early: u64,
    salvaged_tail_tx: u64,
    batches_force_finalized_corrupted: u64,
    batches_force_finalized_timeout: u64,
    combined_scratch: Vec<u8>,
}

impl SlotAccumulator {
    pub fn with_config(cfg: AccumulatorConfig) -> Self {
        Self {
            cfg,
            pending: BTreeMap::new(),
            fec_sets: BTreeMap::new(),
            batch_complete_indices: BTreeMap::new(),
            streaming_cursor: 0,
            streaming_decoder: StreamingDecoder::new(),
            yielded_sigs: FxHashSet::with_capacity_and_hasher(256, Default::default()),
            batch_start: 0,
            slot_complete: false,
            decode_errors: 0,
            recoveries_done: 0,
            recovery_failures: 0,
            batches_decoded_streaming: 0,
            batches_decoded_fallback: 0,
            batches_skipped: 0,
            fec_sets_discarded_unused: 0,
            fec_sets_evicted_early: 0,
            salvaged_tail_tx: 0,
            batches_force_finalized_corrupted: 0,
            batches_force_finalized_timeout: 0,
            combined_scratch: Vec::with_capacity(24 * 1024),
        }
    }

    pub fn new() -> Self {
        Self::with_config(AccumulatorConfig::default())
    }

    pub fn decode_errors(&self) -> u32 {
        self.decode_errors
    }
    pub fn recoveries_done(&self) -> u64 {
        self.recoveries_done
    }
    pub fn recovery_failures(&self) -> u64 {
        self.recovery_failures
    }
    pub fn batches_decoded_streaming(&self) -> u64 {
        self.batches_decoded_streaming
    }
    pub fn batches_decoded_fallback(&self) -> u64 {
        self.batches_decoded_fallback
    }
    pub fn batches_skipped(&self) -> u64 {
        self.batches_skipped
    }
    pub fn fec_sets_discarded_unused(&self) -> u64 {
        self.fec_sets_discarded_unused
    }
    pub fn fec_sets_evicted_early(&self) -> u64 {
        self.fec_sets_evicted_early
    }
    pub fn salvaged_tail_tx(&self) -> u64 {
        self.salvaged_tail_tx
    }
    pub fn batches_force_finalized_corrupted(&self) -> u64 {
        self.batches_force_finalized_corrupted
    }
    pub fn batches_force_finalized_timeout(&self) -> u64 {
        self.batches_force_finalized_timeout
    }

    pub fn push_data(
        &mut self,
        index: u32,
        fec_set_index: u32,
        handle: BufferHandle,
        payload_end: u16,
        batch_complete: bool,
        last_in_slot: bool,
        rs_cache: &mut ReedSolomonCache,
    ) -> Vec<VersionedTransaction> {
        if last_in_slot {
            self.slot_complete = true;
            let mut out = Vec::new();
            self.try_flush_batches(rs_cache, &mut out);
            return out;
        }
        if index < self.batch_start {
            return Vec::new();
        }
        use std::collections::btree_map::Entry as BTreeEntry;
        match self.pending.entry(index) {
            BTreeEntry::Occupied(_) => return Vec::new(),
            BTreeEntry::Vacant(slot) => {
                slot.insert(PendingShred {
                    source: PayloadSource::Pool(handle),
                    payload_end,
                    batch_complete,
                    fec_set_index,
                });
            }
        }
        if batch_complete {
            self.batch_complete_indices
                .entry(index)
                .or_insert_with(Instant::now);
        }

        let mut out = Vec::new();
        self.advance_streaming(&mut out);

        if self.streaming_cursor <= index && self.try_fec_fill(fec_set_index, rs_cache) {
            self.advance_streaming(&mut out);
        }

        self.try_flush_batches(rs_cache, &mut out);

        out
    }

    pub fn push_code(
        &mut self,
        fec_set_index: u32,
        num_data: u16,
        num_coding: u16,
        position: u16,
        coded: &[u8],
        variant: VariantKind,
        rs_cache: &mut ReedSolomonCache,
    ) -> Vec<VersionedTransaction> {
        if self.fec_sets.len() >= self.cfg.max_fec_sets_per_slot
            && !self.fec_sets.contains_key(&fec_set_index)
        {
            if let Some((&oldest, _)) = self.fec_sets.iter().next() {
                if oldest < fec_set_index {
                    self.fec_sets.remove(&oldest);
                    self.fec_sets_evicted_early =
                        self.fec_sets_evicted_early.saturating_add(1);
                }
            }
        }
        let entry = self
            .fec_sets
            .entry(fec_set_index)
            .or_insert_with(|| FecSetBuffer::new(fec_set_index));
        entry.record_code(position, num_data, num_coding, coded, variant);

        let mut out = Vec::new();
        if self.try_fec_fill(fec_set_index, rs_cache) {
            self.advance_streaming(&mut out);
        }
        self.try_flush_batches(rs_cache, &mut out);
        out
    }

    fn advance_streaming(&mut self, out: &mut Vec<VersionedTransaction>) {
        debug_assert!(
            self.streaming_cursor >= self.batch_start,
            "streaming cursor cannot rewind behind batch_start"
        );
        loop {
            let Some(ps) = self.pending.get(&self.streaming_cursor) else {
                break;
            };
            if matches!(ps.source, PayloadSource::Recovered(_)) {
                break;
            }
            let batch_complete_flag = ps.batch_complete;
            let payload = match ps.payload() {
                Some(p) => p,
                None => {
                    let dropped_idx = self.streaming_cursor;
                    self.pending.remove(&dropped_idx);
                    if batch_complete_flag {
                        self.batch_complete_indices.remove(&dropped_idx);
                    }
                    self.streaming_cursor = self.streaming_cursor.saturating_add(1);
                    self.decode_errors = self.decode_errors.saturating_add(1);

                    if batch_complete_flag && dropped_idx > self.batch_start {
                        let salvaged = self.salvage_contiguous_runs_in(
                            self.batch_start,
                            dropped_idx.saturating_sub(1),
                            out,
                        );
                        if salvaged > 0 {
                            self.batches_decoded_fallback =
                                self.batches_decoded_fallback.saturating_add(1);
                        } else {
                            self.batches_skipped =
                                self.batches_skipped.saturating_add(1);
                        }
                        self.batches_force_finalized_corrupted =
                            self.batches_force_finalized_corrupted.saturating_add(1);
                        self.complete_batch(dropped_idx);
                    }
                    continue;
                }
            };
            let push_result = self.streaming_decoder.push(payload);
            self.streaming_cursor = self.streaming_cursor.saturating_add(1);
            match push_result {
                Ok(txs) => {
                    for tx in txs {
                        if self.record_sig(&tx) {
                            out.push(tx);
                        }
                    }
                }
                Err(_) => {
                    self.decode_errors = self.decode_errors.saturating_add(1);
                    self.streaming_decoder.reset();
                    return;
                }
            }
            let batch_complete = batch_complete_flag;
            if batch_complete {
                return;
            }
        }
    }

    fn try_fec_fill(&mut self, hint: u32, rs_cache: &mut ReedSolomonCache) -> bool {
        let target = self.select_recoverable_set(hint);
        let set_idx = match target {
            Some(i) => i,
            None => return false,
        };

        let recovered = {
            let fec = match self.fec_sets.get(&set_idx) {
                Some(f) => f,
                None => return false,
            };
            let nd = fec.num_data as usize;
            if nd == 0 {
                return false;
            }
            let data_refs: Vec<(u16, &[u8])> = self
                .pending
                .iter()
                .filter(|(&idx, ps)| {
                    idx >= set_idx
                        && (idx - set_idx) < nd as u32
                        && ps.fec_set_index == set_idx
                })
                .map(|(&idx, ps)| ((idx - set_idx) as u16, ps.source.slice()))
                .collect();
            match fec.try_reconstruct(&data_refs, rs_cache) {
                Ok(v) => v,
                Err(_) => {
                    self.recovery_failures = self.recovery_failures.saturating_add(1);
                    return false;
                }
            }
        };

        if recovered.is_empty() {
            return false;
        }

        let mut any = false;
        for (position_in_set, bytes) in recovered {
            let global_index = set_idx + position_in_set as u32;
            if global_index < self.batch_start || self.pending.contains_key(&global_index) {
                continue;
            }
            let (payload_end, batch_complete, last_in_slot) = match parser::parse_kind(&bytes) {
                Ok(parser::ShredKind::Data(d)) => (
                    (DATA_HEADER_SIZE + d.payload.len()) as u16,
                    d.batch_complete,
                    d.last_in_slot,
                ),
                _ => continue,
            };
            if last_in_slot {
                self.slot_complete = true;
                continue;
            }
            self.pending.insert(
                global_index,
                PendingShred {
                    source: PayloadSource::Recovered(bytes),
                    payload_end,
                    batch_complete,
                    fec_set_index: set_idx,
                },
            );
            if batch_complete {
                self.batch_complete_indices
                    .entry(global_index)
                    .or_insert_with(Instant::now);
            }
            any = true;
        }

        if any {
            self.recoveries_done = self.recoveries_done.saturating_add(1);
            self.fec_sets.remove(&set_idx);
        }
        any
    }

    fn select_recoverable_set(&self, hint: u32) -> Option<u32> {
        let cursor = self.streaming_cursor;
        let data_count_for = |set_idx: u32, fec: &FecSetBuffer| -> usize {
            let nd = fec.num_data as u32;
            if nd == 0 {
                return 0;
            }
            self.pending
                .range(set_idx..set_idx.saturating_add(nd))
                .filter(|(_, ps)| ps.fec_set_index == set_idx)
                .count()
        };
        if let Some(fec) = self.fec_sets.get(&hint) {
            if fec.can_recover_with(data_count_for(hint, fec)) {
                return Some(hint);
            }
        }
        for (&set_idx, fec) in self.fec_sets.iter() {
            let nd = fec.num_data as u32;
            if nd == 0 {
                continue;
            }
            if set_idx <= cursor
                && cursor < set_idx + nd
                && fec.can_recover_with(data_count_for(set_idx, fec))
            {
                return Some(set_idx);
            }
        }
        None
    }

    fn try_flush_batches(
        &mut self,
        rs_cache: &mut ReedSolomonCache,
        out: &mut Vec<VersionedTransaction>,
    ) {
        loop {
            let (bc_idx, first_seen) = match self
                .batch_complete_indices
                .range(self.batch_start..)
                .next()
            {
                Some((&k, &t)) => (k, t),
                None => return,
            };
            let start = self.batch_start;

            let expected = (bc_idx - start + 1) as usize;
            let mut present = self.pending.range(start..=bc_idx).count();
            if present < expected {
                let hints: Vec<u32> = self
                    .fec_sets
                    .iter()
                    .filter(|(&f, fec)| {
                        let nd = fec.num_data as u32;
                        nd != 0 && f + nd > start && f <= bc_idx
                    })
                    .map(|(&f, _)| f)
                    .collect();
                for h in hints {
                    self.try_fec_fill(h, rs_cache);
                }
                present = self.pending.range(start..=bc_idx).count();
            }

            if present < expected {
                if first_seen.elapsed() >= self.cfg.stuck_batch_timeout {
                    let salvaged =
                        self.salvage_contiguous_runs_in(start, bc_idx, out);
                    if salvaged > 0 {
                        self.batches_decoded_fallback =
                            self.batches_decoded_fallback.saturating_add(1);
                    } else {
                        self.batches_skipped =
                            self.batches_skipped.saturating_add(1);
                    }
                    self.batches_force_finalized_timeout =
                        self.batches_force_finalized_timeout.saturating_add(1);
                    self.complete_batch(bc_idx);
                    continue;
                }
                return;
            }

            self.combined_scratch.clear();
            for i in start..=bc_idx {
                if let Some(ps) = self.pending.get(&i) {
                    if let Some(p) = ps.payload() {
                        self.combined_scratch.extend_from_slice(p);
                    }
                }
            }
            match decode_batch(&self.combined_scratch) {
                Some(txs) => {
                    let mut any_new = false;
                    for tx in txs {
                        if self.record_sig(&tx) {
                            out.push(tx);
                            any_new = true;
                        }
                    }
                    if any_new {
                        self.batches_decoded_fallback =
                            self.batches_decoded_fallback.saturating_add(1);
                    } else {
                        self.batches_decoded_streaming =
                            self.batches_decoded_streaming.saturating_add(1);
                    }
                }
                None => {
                    self.decode_errors = self.decode_errors.saturating_add(1);
                    self.batches_skipped = self.batches_skipped.saturating_add(1);
                }
            }
            self.complete_batch(bc_idx);
        }
    }

    fn complete_batch(&mut self, bc_idx: u32) {
        let next_start = bc_idx.saturating_add(1);
        self.pending.retain(|&k, _| k >= next_start);
        self.batch_complete_indices.retain(|&k, _| k >= next_start);
        self.fec_sets.retain(|&set_idx, fec| {
            let nd = fec.num_data as u32;
            !(nd > 0 && set_idx.saturating_add(nd) <= next_start)
        });
        self.batch_start = next_start;
        self.streaming_cursor = self.streaming_cursor.max(next_start);
        self.streaming_decoder.reset();
        self.yielded_sigs.clear();
    }

    pub fn try_harvest_tail(
        &mut self,
        rs_cache: &mut ReedSolomonCache,
    ) -> Vec<VersionedTransaction> {
        let mut out = Vec::new();

        let fec_hints: Vec<u32> = self.fec_sets.keys().copied().collect();
        for h in fec_hints {
            let _ = self.try_fec_fill(h, rs_cache);
        }

        let boundaries: Vec<u32> = self
            .pending
            .iter()
            .filter(|(_, ps)| ps.batch_complete)
            .map(|(&k, _)| k)
            .filter(|&k| k >= self.batch_start)
            .collect();

        for bc_idx in boundaries {
            let start = self.batch_start;
            if bc_idx < start {
                continue;
            }
            if (start..=bc_idx).all(|i| self.pending.contains_key(&i)) {
                self.combined_scratch.clear();
                for i in start..=bc_idx {
                    if let Some(ps) = self.pending.get(&i) {
                        if let Some(p) = ps.payload() {
                            self.combined_scratch.extend_from_slice(p);
                        }
                    }
                }
                if let Some(txs) = decode_batch(&self.combined_scratch) {
                    let mut any_new = false;
                    for tx in txs {
                        if self.record_sig(&tx) {
                            out.push(tx);
                            any_new = true;
                        }
                    }
                    if any_new {
                        self.batches_decoded_fallback =
                            self.batches_decoded_fallback.saturating_add(1);
                    } else {
                        self.batches_decoded_streaming =
                            self.batches_decoded_streaming.saturating_add(1);
                    }
                } else {
                    self.decode_errors = self.decode_errors.saturating_add(1);
                    self.batches_skipped = self.batches_skipped.saturating_add(1);
                }
            } else {
                let salvaged =
                    self.salvage_contiguous_runs_in(start, bc_idx, &mut out);
                if salvaged > 0 {
                    self.batches_decoded_fallback =
                        self.batches_decoded_fallback.saturating_add(1);
                } else {
                    self.batches_skipped = self.batches_skipped.saturating_add(1);
                }
            }
            self.complete_batch(bc_idx);
        }

        if !self.pending.is_empty() {
            let indices: Vec<u32> = self.pending.keys().copied().collect();
            let mut i = 0;
            while i < indices.len() {
                let run_start = indices[i];
                self.combined_scratch.clear();
                let mut expected = run_start;
                while i < indices.len() && indices[i] == expected {
                    if let Some(ps) = self.pending.get(&indices[i]) {
                        if let Some(p) = ps.payload() {
                            self.combined_scratch.extend_from_slice(p);
                        }
                    }
                    expected = indices[i].saturating_add(1);
                    i += 1;
                }
                if !self.combined_scratch.is_empty() {
                    for tx in decode_concatenated(&self.combined_scratch) {
                        if self.record_sig(&tx) {
                            out.push(tx);
                        }
                    }
                }
            }
        }

        self.fec_sets_discarded_unused = self
            .fec_sets_discarded_unused
            .saturating_add(self.fec_sets.len() as u64);
        self.salvaged_tail_tx = self.salvaged_tail_tx.saturating_add(out.len() as u64);

        out
    }

    fn salvage_contiguous_runs_in(
        &mut self,
        start: u32,
        end: u32,
        out: &mut Vec<VersionedTransaction>,
    ) -> usize {
        let mut salvaged = 0usize;
        let mut cursor = start;
        while cursor <= end {
            while cursor <= end && !self.pending.contains_key(&cursor) {
                cursor = cursor.saturating_add(1);
            }
            if cursor > end {
                break;
            }
            self.combined_scratch.clear();
            while cursor <= end && self.pending.contains_key(&cursor) {
                if let Some(ps) = self.pending.get(&cursor) {
                    if let Some(p) = ps.payload() {
                        self.combined_scratch.extend_from_slice(p);
                    }
                }
                cursor = cursor.saturating_add(1);
            }
            if !self.combined_scratch.is_empty() {
                for tx in decode_concatenated(&self.combined_scratch) {
                    if self.record_sig(&tx) {
                        out.push(tx);
                        salvaged = salvaged.saturating_add(1);
                    }
                }
            }
        }
        salvaged
    }

    fn record_sig(&mut self, tx: &VersionedTransaction) -> bool {
        let sig = match tx.signatures.first() {
            Some(s) => *s,
            None => return false,
        };
        let bytes: [u8; 64] = sig.into();
        self.yielded_sigs.insert(bytes)
    }
}

impl Default for SlotAccumulator {
    fn default() -> Self {
        Self::new()
    }
}
