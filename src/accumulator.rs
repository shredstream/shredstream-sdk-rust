use std::collections::BTreeMap;

use solana_transaction::versioned::VersionedTransaction;

use crate::decoder::BatchDecoder;

const GAP_SKIP_THRESHOLD: u32 = 5;

pub struct SlotAccumulator {
    pending: BTreeMap<u32, (Vec<u8>, bool, bool)>,
    decoder: BatchDecoder,
    next_drain: u32,
    pub slot_complete: bool,
    stall_count: u32,
    decode_errors: u32,
}

impl SlotAccumulator {
    pub fn new() -> Self {
        Self {
            pending: BTreeMap::new(),
            decoder: BatchDecoder::new(),
            next_drain: 0,
            slot_complete: false,
            stall_count: 0,
            decode_errors: 0,
        }
    }

    pub fn decode_errors(&self) -> u32 {
        self.decode_errors
    }

    pub fn push(
        &mut self,
        index: u32,
        payload: Vec<u8>,
        batch_complete: bool,
        last_in_slot: bool,
    ) -> Vec<VersionedTransaction> {
        if index < self.next_drain || self.pending.contains_key(&index) {
            return vec![];
        }

        self.pending.insert(index, (payload, batch_complete, last_in_slot));

        let mut all_txs = Vec::new();
        let drain_start = self.next_drain;
        let errors_before = self.decode_errors;

        self.drain_contiguous(&mut all_txs);

        if self.decode_errors > errors_before {
            return all_txs;
        }

        if self.next_drain == drain_start {
            self.stall_count += 1;
        } else {
            self.stall_count = 0;
        }

        if self.stall_count >= GAP_SKIP_THRESHOLD && !self.pending.is_empty() {
            if let Some(&lowest) = self.pending.keys().next() {
                self.next_drain = lowest;
                self.stall_count = 0;
                self.decoder.reset();
                self.drain_contiguous(&mut all_txs);
            }
        }

        all_txs
    }

    fn drain_contiguous(&mut self, all_txs: &mut Vec<VersionedTransaction>) {
        while let Some((payload, bc, lis)) = self.pending.remove(&self.next_drain) {
            self.next_drain += 1;

            match self.decoder.push(&payload) {
                Ok(txs) => all_txs.extend(txs),
                Err(_) => {
                    self.decode_errors += 1;
                    return;
                }
            }

            if lis {
                self.slot_complete = true;
            }
            if bc {
                self.decoder.reset();
            }
        }
    }
}
