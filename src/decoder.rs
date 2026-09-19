use solana_entry::entry::Entry;
use solana_transaction::versioned::VersionedTransaction;

use crate::error::DecodeError;

const MAX_ENTRY_COUNT: u64 = 50_000;
const INITIAL_BUFFER_CAPACITY: usize = 64 * 1024;

fn is_truncated(err: &wincode::ReadError) -> bool {
    matches!(
        err,
        wincode::ReadError::Io(wincode::io::ReadError::ReadSizeLimit(_))
    )
}

pub struct StreamingDecoder {
    buffer: Vec<u8>,
    cursor: usize,
    expected_count: Option<u64>,
    entries_yielded: u64,
}

impl StreamingDecoder {
    pub fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(INITIAL_BUFFER_CAPACITY),
            cursor: 0,
            expected_count: None,
            entries_yielded: 0,
        }
    }

    #[inline]
    pub fn push(
        &mut self,
        payload: &[u8],
    ) -> Result<Vec<VersionedTransaction>, DecodeError> {
        self.buffer.extend_from_slice(payload);
        self.try_deserialize()
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.expected_count = None;
        self.entries_yielded = 0;
    }

    fn try_deserialize(&mut self) -> Result<Vec<VersionedTransaction>, DecodeError> {
        if self.expected_count.is_none() && self.buffer.len() >= self.cursor + 8 {
            let bytes: [u8; 8] = self.buffer[self.cursor..self.cursor + 8]
                .try_into()
                .map_err(|_| DecodeError::Corruption("slice len".into()))?;
            let count = u64::from_le_bytes(bytes);
            if count > MAX_ENTRY_COUNT {
                return Err(DecodeError::Corruption("invalid entry count".into()));
            }
            self.cursor += 8;
            self.expected_count = Some(count);
        }

        let expected = match self.expected_count {
            Some(c) => c,
            None => return Ok(Vec::new()),
        };

        let mut txs = Vec::new();

        while self.entries_yielded < expected {
            let remaining = &self.buffer[self.cursor..];
            if remaining.is_empty() {
                break;
            }
            match wincode::deserialize::<Entry>(remaining) {
                Ok(entry) => {
                    let consumed = wincode::serialized_size(&entry)
                        .map_err(|e| DecodeError::Corruption(e.to_string()))?;
                    self.cursor += consumed as usize;
                    self.entries_yielded += 1;
                    txs.extend(entry.transactions);
                }
                Err(ref e) if is_truncated(e) => break,
                Err(e) => return Err(DecodeError::Corruption(e.to_string())),
            }
        }

        Ok(txs)
    }
}

impl Default for StreamingDecoder {
    fn default() -> Self {
        Self::new()
    }
}

pub fn decode_batch(bytes: &[u8]) -> Option<Vec<VersionedTransaction>> {
    if !validate_vec_prefix(bytes) {
        return None;
    }
    let entries: Vec<Entry> = wincode::deserialize(bytes).ok()?;
    Some(extract_txs(&entries))
}

pub fn decode_concatenated(bytes: &[u8]) -> Vec<VersionedTransaction> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset + 8 <= bytes.len() {
        let remaining = &bytes[offset..];
        if !validate_vec_prefix(remaining) {
            break;
        }
        let entries: Vec<Entry> = match wincode::deserialize(remaining) {
            Ok(v) => v,
            Err(_) => break,
        };
        let consumed = match wincode::serialized_size(&entries) {
            Ok(n) => n as usize,
            Err(_) => break,
        };
        if consumed == 0 {
            break;
        }
        out.extend(extract_txs(&entries));
        offset += consumed;
    }
    out
}

fn validate_vec_prefix(bytes: &[u8]) -> bool {
    if bytes.len() < 8 {
        return false;
    }
    let arr: [u8; 8] = match bytes[..8].try_into() {
        Ok(a) => a,
        Err(_) => return false,
    };
    let count = u64::from_le_bytes(arr);
    if count > MAX_ENTRY_COUNT {
        return false;
    }
    if count.saturating_mul(48) > bytes.len() as u64 {
        return false;
    }
    true
}

fn extract_txs(entries: &[Entry]) -> Vec<VersionedTransaction> {
    let mut txs = Vec::new();
    for e in entries {
        txs.extend(e.transactions.iter().cloned());
    }
    txs
}
