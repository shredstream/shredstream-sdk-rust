use std::io::Cursor;

use solana_entry::entry::Entry;
use solana_transaction::versioned::VersionedTransaction;

use crate::error::DecodeError;

pub struct BatchDecoder {
    buffer: Vec<u8>,
    expected_count: Option<u64>,
    entries_yielded: u64,
    cursor: usize,
}

impl BatchDecoder {
    pub fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(64 * 1024),
            expected_count: None,
            entries_yielded: 0,
            cursor: 0,
        }
    }

    pub fn push(&mut self, payload: &[u8]) -> Result<Vec<VersionedTransaction>, DecodeError> {
        self.buffer.extend_from_slice(payload);
        self.try_deserialize()
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
        self.expected_count = None;
        self.entries_yielded = 0;
        self.cursor = 0;
    }

    fn try_deserialize(&mut self) -> Result<Vec<VersionedTransaction>, DecodeError> {
        if self.expected_count.is_none() && self.buffer.len() >= self.cursor + 8 {
            let count = u64::from_le_bytes(
                self.buffer[self.cursor..self.cursor + 8].try_into().unwrap(),
            );
            if count > 100_000 {
                return Err(DecodeError::Corruption("invalid entry count".into()));
            }
            self.cursor += 8;
            self.expected_count = Some(count);
        }

        let expected = match self.expected_count {
            Some(c) => c,
            None => return Ok(vec![]),
        };

        let mut txs = Vec::new();

        while self.entries_yielded < expected {
            let remaining = &self.buffer[self.cursor..];
            if remaining.is_empty() {
                break;
            }

            let mut cursor = Cursor::new(remaining);
            match bincode::deserialize_from::<_, Entry>(&mut cursor) {
                Ok(entry) => {
                    self.cursor += cursor.position() as usize;
                    self.entries_yielded += 1;
                    txs.extend(entry.transactions);
                }
                Err(ref e) if is_eof(e) => break,
                Err(e) => return Err(DecodeError::Bincode(e)),
            }
        }

        if self.cursor > 128 * 1024 {
            self.buffer.drain(..self.cursor);
            self.cursor = 0;
        }

        Ok(txs)
    }
}

fn is_eof(err: &bincode::Error) -> bool {
    matches!(
        err.as_ref(),
        bincode::ErrorKind::Io(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof
    )
}
