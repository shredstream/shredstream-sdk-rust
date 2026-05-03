use crate::variant::{classify_variant, VariantKind};

pub const VARIANT_OFFSET: usize = 0x40;
pub const SLOT_OFFSET: usize = 0x41;
pub const INDEX_OFFSET: usize = 0x49;
pub const VERSION_OFFSET: usize = 0x4D;
pub const FEC_SET_INDEX_OFFSET: usize = 0x4F;
pub const DATA_PARENT_OFFSET: usize = 0x53;
pub const DATA_FLAGS_OFFSET: usize = 0x55;
pub const DATA_SIZE_OFFSET: usize = 0x56;
pub const DATA_HEADER_SIZE: usize = 0x58;

pub const CODE_NUM_DATA_OFFSET: usize = 0x53;
pub const CODE_NUM_CODING_OFFSET: usize = 0x55;
pub const CODE_POSITION_OFFSET: usize = 0x57;
pub const CODE_HEADER_SIZE: usize = 0x59;

const DATA_COMPLETE: u8 = 0b0100_0000;
const LAST_IN_SLOT: u8 = 0b1100_0000;

const MAX_DATA_SHRED_SIZE: usize = 1203;

#[derive(Debug)]
pub enum ShredKind<'a> {
    Data(DataShred<'a>),
    Code(CodeShred<'a>),
}

#[derive(Debug)]
pub struct DataShred<'a> {
    pub slot: u64,
    pub index: u32,
    pub version: u16,
    pub fec_set_index: u32,
    pub parent_offset: u16,
    pub payload: &'a [u8],
    pub batch_complete: bool,
    pub last_in_slot: bool,
    pub variant: VariantKind,
    pub raw_len: usize,
}

#[derive(Debug)]
pub struct CodeShred<'a> {
    pub slot: u64,
    pub index: u32,
    pub version: u16,
    pub fec_set_index: u32,
    pub num_data_shreds: u16,
    pub num_coding_shreds: u16,
    pub position: u16,
    pub coded: &'a [u8],
    pub variant: VariantKind,
}

#[derive(Debug)]
pub struct ParsedShred<'a> {
    pub slot: u64,
    pub index: u32,
    pub payload: &'a [u8],
    pub batch_complete: bool,
    pub last_in_slot: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    TooShort,
    UnknownVariant,
    PayloadInvalid,
}

#[inline]
fn read_u16(raw: &[u8], offset: usize) -> Option<u16> {
    let end = offset.checked_add(2)?;
    let bytes: [u8; 2] = raw.get(offset..end)?.try_into().ok()?;
    Some(u16::from_le_bytes(bytes))
}

#[inline]
fn read_u32(raw: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let bytes: [u8; 4] = raw.get(offset..end)?.try_into().ok()?;
    Some(u32::from_le_bytes(bytes))
}

#[inline]
fn read_u64(raw: &[u8], offset: usize) -> Option<u64> {
    let end = offset.checked_add(8)?;
    let bytes: [u8; 8] = raw.get(offset..end)?.try_into().ok()?;
    Some(u64::from_le_bytes(bytes))
}

#[inline]
pub fn parse_kind(raw: &[u8]) -> Result<ShredKind<'_>, ParseError> {
    if raw.len() < DATA_HEADER_SIZE {
        return Err(ParseError::TooShort);
    }
    let variant_byte = raw[VARIANT_OFFSET];
    let slot = read_u64(raw, SLOT_OFFSET).ok_or(ParseError::PayloadInvalid)?;
    let index = read_u32(raw, INDEX_OFFSET).ok_or(ParseError::PayloadInvalid)?;
    let version = read_u16(raw, VERSION_OFFSET).ok_or(ParseError::PayloadInvalid)?;
    let fec_set_index = read_u32(raw, FEC_SET_INDEX_OFFSET).ok_or(ParseError::PayloadInvalid)?;

    if let Some(kind) = classify_variant(variant_byte) {
        if kind.is_data() {
            return parse_data_with_kind(raw, slot, index, version, fec_set_index, kind);
        }
        if kind.is_code() {
            return parse_code_with_kind(raw, slot, index, version, fec_set_index, kind);
        }
    }

    parse_data_legacy_fallback(raw, slot, index).map_err(|_| ParseError::UnknownVariant)
}

fn parse_data_with_kind(
    raw: &[u8],
    slot: u64,
    index: u32,
    version: u16,
    fec_set_index: u32,
    variant: VariantKind,
) -> Result<ShredKind<'_>, ParseError> {
    if fec_set_index > index {
        return parse_data_legacy_fallback(raw, slot, index);
    }
    let parent_offset = read_u16(raw, DATA_PARENT_OFFSET).ok_or(ParseError::PayloadInvalid)?;
    let flags = *raw.get(DATA_FLAGS_OFFSET).ok_or(ParseError::PayloadInvalid)?;
    let size = read_u16(raw, DATA_SIZE_OFFSET).ok_or(ParseError::PayloadInvalid)? as usize;
    if size > MAX_DATA_SHRED_SIZE {
        return parse_data_legacy_fallback(raw, slot, index);
    }
    if size > raw.len() {
        return Err(ParseError::PayloadInvalid);
    }
    let payload = if size > DATA_HEADER_SIZE {
        raw.get(DATA_HEADER_SIZE..size).ok_or(ParseError::PayloadInvalid)?
    } else {
        &[]
    };
    Ok(ShredKind::Data(DataShred {
        slot,
        index,
        version,
        fec_set_index,
        parent_offset,
        payload,
        batch_complete: flags & DATA_COMPLETE != 0,
        last_in_slot: flags & LAST_IN_SLOT == LAST_IN_SLOT,
        variant,
        raw_len: raw.len(),
    }))
}

fn parse_code_with_kind(
    raw: &[u8],
    slot: u64,
    index: u32,
    version: u16,
    fec_set_index: u32,
    variant: VariantKind,
) -> Result<ShredKind<'_>, ParseError> {
    if raw.len() < CODE_HEADER_SIZE {
        return Err(ParseError::TooShort);
    }
    let num_data_shreds = read_u16(raw, CODE_NUM_DATA_OFFSET).ok_or(ParseError::PayloadInvalid)?;
    let num_coding_shreds =
        read_u16(raw, CODE_NUM_CODING_OFFSET).ok_or(ParseError::PayloadInvalid)?;
    let position = read_u16(raw, CODE_POSITION_OFFSET).ok_or(ParseError::PayloadInvalid)?;
    if num_data_shreds == 0 || num_coding_shreds == 0 || position >= num_coding_shreds {
        return Err(ParseError::PayloadInvalid);
    }
    let suffix = variant.merkle_suffix();
    if suffix >= raw.len() {
        return Err(ParseError::PayloadInvalid);
    }
    let coded_end = raw.len() - suffix;
    let coded = raw.get(..coded_end).ok_or(ParseError::PayloadInvalid)?;
    Ok(ShredKind::Code(CodeShred {
        slot,
        index,
        version,
        fec_set_index,
        num_data_shreds,
        num_coding_shreds,
        position,
        coded,
        variant,
    }))
}

fn parse_data_legacy_fallback(
    raw: &[u8],
    slot: u64,
    index: u32,
) -> Result<ShredKind<'_>, ParseError> {
    let flags = *raw.get(DATA_FLAGS_OFFSET).ok_or(ParseError::PayloadInvalid)?;
    let size = read_u16(raw, DATA_SIZE_OFFSET).ok_or(ParseError::PayloadInvalid)? as usize;
    if size > raw.len() {
        return Err(ParseError::PayloadInvalid);
    }
    let payload = if size > DATA_HEADER_SIZE {
        raw.get(DATA_HEADER_SIZE..size).ok_or(ParseError::PayloadInvalid)?
    } else {
        &[]
    };
    Ok(ShredKind::Data(DataShred {
        slot,
        index,
        version: 0,
        fec_set_index: index,
        parent_offset: 0,
        payload,
        batch_complete: flags & DATA_COMPLETE != 0,
        last_in_slot: flags & LAST_IN_SLOT == LAST_IN_SLOT,
        variant: VariantKind::DataLegacy,
        raw_len: raw.len(),
    }))
}

pub fn parse_shred(raw: &[u8]) -> Option<ParsedShred<'_>> {
    match parse_kind(raw).ok()? {
        ShredKind::Data(d) => Some(ParsedShred {
            slot: d.slot,
            index: d.index,
            payload: d.payload,
            batch_complete: d.batch_complete,
            last_in_slot: d.last_in_slot,
        }),
        ShredKind::Code(_) => None,
    }
}
