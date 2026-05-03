use std::io::Cursor;

use bincode::Options;
use solana_entry::entry::Entry;
use solana_transaction::versioned::VersionedTransaction;

use crate::error::DecodeError;

const MAX_ENTRY_COUNT: u64 = 50_000;
const INITIAL_BUFFER_CAPACITY: usize = 64 * 1024;

fn bincode_opts(max_bytes: u64) -> impl Options + Copy {
    bincode::options()
        .with_fixint_encoding()
        .with_limit(max_bytes)
        .allow_trailing_bytes()
}

fn streaming_opts() -> impl Options + Copy {
    bincode::options()
        .with_fixint_encoding()
        .allow_trailing_bytes()
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

        let opts = streaming_opts();

        let mut txs = Vec::new();

        while self.entries_yielded < expected {
            let remaining = &self.buffer[self.cursor..];
            if remaining.is_empty() {
                break;
            }
            let mut cur = Cursor::new(remaining);
            match opts.deserialize_from::<_, Entry>(&mut cur) {
                Ok(entry) => {
                    self.cursor += cur.position() as usize;
                    self.entries_yielded += 1;
                    txs.extend(entry.transactions);
                }
                Err(ref e) if is_eof(e) => break,
                Err(e) => return Err(DecodeError::Bincode(e)),
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

fn is_eof(err: &bincode::Error) -> bool {
    matches!(
        err.as_ref(),
        bincode::ErrorKind::Io(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof
    )
}

pub fn decode_batch(bytes: &[u8]) -> Option<Vec<VersionedTransaction>> {
    if !validate_vec_prefix(bytes) {
        return None;
    }
    let entries: Vec<Entry> = bincode_opts(bytes.len() as u64).deserialize(bytes).ok()?;
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
        let entries: Vec<Entry> =
            match bincode_opts(remaining.len() as u64).deserialize(remaining) {
                Ok(v) => v,
                Err(_) => break,
            };
        let consumed = match bincode_opts(remaining.len() as u64).serialized_size(&entries) {
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
