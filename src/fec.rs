use std::collections::BTreeMap;

use reed_solomon_erasure::galois_8::ReedSolomon;

use crate::variant::VariantKind;

const CACHE_CAPACITY: usize = 32;

pub const SIZE_OF_SIGNATURE: usize = 64;
pub const SIZE_OF_CODING_SHRED_HEADERS: usize = 89;
pub const SIZE_OF_MERKLE_ROOT: usize = 32;
pub const SIZE_OF_MERKLE_PROOF_ENTRY: usize = 20;
pub const DATA_SHRED_PAYLOAD_LEN: usize = 1203;
pub const CODE_SHRED_PAYLOAD_LEN: usize = 1228;

#[inline]
pub fn shard_size(proof_size: u8, resigned: bool) -> usize {
    CODE_SHRED_PAYLOAD_LEN
        - SIZE_OF_CODING_SHRED_HEADERS
        - SIZE_OF_MERKLE_ROOT
        - (proof_size as usize) * SIZE_OF_MERKLE_PROOF_ENTRY
        - if resigned { SIZE_OF_SIGNATURE } else { 0 }
}

pub fn data_shard_slice(raw: &[u8], variant: VariantKind) -> Option<&[u8]> {
    match variant {
        VariantKind::DataMerkle {
            proof_size,
            resigned,
        } => {
            let len = shard_size(proof_size, resigned);
            let end = SIZE_OF_SIGNATURE.checked_add(len)?;
            raw.get(SIZE_OF_SIGNATURE..end)
        }
        _ => None,
    }
}

pub fn code_shard_slice(raw: &[u8], variant: VariantKind) -> Option<&[u8]> {
    match variant {
        VariantKind::CodeMerkle {
            proof_size,
            resigned,
        } => {
            let len = shard_size(proof_size, resigned);
            let end = SIZE_OF_CODING_SHRED_HEADERS.checked_add(len)?;
            raw.get(SIZE_OF_CODING_SHRED_HEADERS..end)
        }
        _ => None,
    }
}

pub struct ReedSolomonCache {
    entries: Vec<((u16, u16), ReedSolomon)>,
}

impl Default for ReedSolomonCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ReedSolomonCache {
    pub fn new() -> Self {
        Self {
            entries: Vec::with_capacity(CACHE_CAPACITY),
        }
    }

    pub fn get_or_build(
        &mut self,
        data: u16,
        coding: u16,
    ) -> Result<&ReedSolomon, reed_solomon_erasure::Error> {
        if let Some(pos) = self.entries.iter().position(|(k, _)| *k == (data, coding)) {
            let entry = self.entries.swap_remove(pos);
            self.entries.push(entry);
            return Ok(&self.entries.last().unwrap().1);
        }
        let rs = ReedSolomon::new(data as usize, coding as usize)?;
        if self.entries.len() >= CACHE_CAPACITY {
            self.entries.remove(0);
        }
        self.entries.push(((data, coding), rs));
        Ok(&self.entries.last().unwrap().1)
    }
}

pub struct FecSetBuffer {
    #[allow(dead_code)]
    pub(crate) fec_set_index: u32,
    pub(crate) num_data: u16,
    pub(crate) num_coding: u16,
    pub(crate) shard_size: usize,
    pub(crate) variant: Option<VariantKind>,
    pub(crate) code: BTreeMap<u16, Vec<u8>>,
}

impl FecSetBuffer {
    pub fn new(fec_set_index: u32) -> Self {
        Self {
            fec_set_index,
            num_data: 0,
            num_coding: 0,
            shard_size: 0,
            variant: None,
            code: BTreeMap::new(),
        }
    }

    pub fn record_code(
        &mut self,
        position: u16,
        num_data: u16,
        num_coding: u16,
        raw: &[u8],
        variant: VariantKind,
    ) {
        if self.num_data == 0 {
            self.num_data = num_data;
        }
        if self.num_coding == 0 {
            self.num_coding = num_coding;
        }
        if self.variant.is_none() {
            self.variant = Some(variant);
        }
        if self.shard_size == 0 {
            if let VariantKind::CodeMerkle {
                proof_size,
                resigned,
            } = variant
            {
                self.shard_size = shard_size(proof_size, resigned);
            }
        }
        self.code.entry(position).or_insert_with(|| raw.to_vec());
    }

    pub fn can_recover_with(&self, data_received: usize) -> bool {
        self.num_data > 0 && data_received + self.code.len() >= self.num_data as usize
    }

    pub fn try_reconstruct(
        &self,
        data_shards: &[(u16, &[u8])],
        cache: &mut ReedSolomonCache,
    ) -> Result<Vec<(u16, Vec<u8>)>, reed_solomon_erasure::Error> {
        if self.num_data == 0 || self.shard_size == 0 {
            return Ok(Vec::new());
        }
        let variant = match self.variant {
            Some(v) => v,
            None => return Ok(Vec::new()),
        };
        let (proof_size, resigned) = match variant {
            VariantKind::CodeMerkle {
                proof_size,
                resigned,
            } => (proof_size, resigned),
            _ => return Ok(Vec::new()),
        };

        let nd = self.num_data as usize;
        let nc = self.num_coding as usize;
        let total = nd + nc;
        let shard_len = self.shard_size;
        let mut shards: Vec<Option<Vec<u8>>> = vec![None; total];

        for (&pos, raw) in self.code.iter() {
            let idx = nd + pos as usize;
            if idx >= total {
                continue;
            }
            let shard = match code_shard_slice(raw, variant) {
                Some(s) if s.len() == shard_len => s.to_vec(),
                _ => continue,
            };
            shards[idx] = Some(shard);
        }

        for (pos, raw) in data_shards.iter() {
            let p = *pos as usize;
            if p >= nd || shards[p].is_some() {
                continue;
            }
            let shard = match data_shard_slice(raw, variant_as_data(variant)) {
                Some(s) if s.len() == shard_len => s.to_vec(),
                _ => continue,
            };
            shards[p] = Some(shard);
        }

        let have = shards.iter().flatten().count();
        if have < nd {
            return Ok(Vec::new());
        }

        let rs = cache.get_or_build(self.num_data, self.num_coding)?;
        rs.reconstruct_data(&mut shards)?;

        let data_positions_present: std::collections::BTreeSet<u16> =
            data_shards.iter().map(|(p, _)| *p).collect();

        let mut out = Vec::new();
        for pos in 0..nd as u16 {
            if data_positions_present.contains(&pos) {
                continue;
            }
            let shard = match shards[pos as usize].take() {
                Some(s) if s.len() == shard_len => s,
                _ => continue,
            };
            let _ = (proof_size, resigned);
            let mut raw = vec![0u8; DATA_SHRED_PAYLOAD_LEN];
            raw[SIZE_OF_SIGNATURE..SIZE_OF_SIGNATURE + shard_len].copy_from_slice(&shard);
            out.push((pos, raw));
        }
        Ok(out)
    }
}

fn variant_as_data(v: VariantKind) -> VariantKind {
    match v {
        VariantKind::CodeMerkle {
            proof_size,
            resigned,
        } => VariantKind::DataMerkle {
            proof_size,
            resigned,
        },
        other => other,
    }
}
