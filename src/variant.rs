pub const PROOF_NODE_SIZE: usize = 20;
pub const RESIGNED_EXTRA: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariantKind {
    DataLegacy,
    CodeLegacy,
    DataMerkle { proof_size: u8, resigned: bool },
    CodeMerkle { proof_size: u8, resigned: bool },
}

impl VariantKind {
    pub fn is_data(&self) -> bool {
        matches!(self, Self::DataLegacy | Self::DataMerkle { .. })
    }

    pub fn is_code(&self) -> bool {
        matches!(self, Self::CodeLegacy | Self::CodeMerkle { .. })
    }

    pub fn merkle_suffix(&self) -> usize {
        match self {
            Self::DataLegacy | Self::CodeLegacy => 0,
            Self::DataMerkle {
                proof_size,
                resigned,
            }
            | Self::CodeMerkle {
                proof_size,
                resigned,
            } => {
                let proof = (*proof_size as usize) * PROOF_NODE_SIZE;
                let merkle_root = 32;
                let resigned_sig = if *resigned { RESIGNED_EXTRA } else { 0 };
                proof + merkle_root + resigned_sig
            }
        }
    }

    pub fn proof_size(&self) -> u8 {
        match self {
            Self::DataMerkle { proof_size, .. } | Self::CodeMerkle { proof_size, .. } => {
                *proof_size
            }
            Self::DataLegacy | Self::CodeLegacy => 0,
        }
    }

    pub fn resigned(&self) -> bool {
        match self {
            Self::DataMerkle { resigned, .. } | Self::CodeMerkle { resigned, .. } => *resigned,
            Self::DataLegacy | Self::CodeLegacy => false,
        }
    }
}

#[inline]
pub fn classify_variant(b: u8) -> Option<VariantKind> {
    match b {
        0xA5 => Some(VariantKind::DataLegacy),
        0x5A => Some(VariantKind::CodeLegacy),
        0x60..=0x6F => Some(VariantKind::CodeMerkle {
            proof_size: b & 0x0F,
            resigned: false,
        }),
        0x70..=0x7F => Some(VariantKind::CodeMerkle {
            proof_size: b & 0x0F,
            resigned: true,
        }),
        0x90..=0x9F => Some(VariantKind::DataMerkle {
            proof_size: b & 0x0F,
            resigned: false,
        }),
        0xB0..=0xBF => Some(VariantKind::DataMerkle {
            proof_size: b & 0x0F,
            resigned: true,
        }),
        _ => None,
    }
}
