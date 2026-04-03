const SLOT_OFFSET: usize = 0x41;
const INDEX_OFFSET: usize = 0x49;
const FLAGS_OFFSET: usize = 0x55;
const SIZE_OFFSET: usize = 0x56;
pub const DATA_HEADER_SIZE: usize = 0x58;

const DATA_COMPLETE: u8 = 0b0100_0000;
const LAST_IN_SLOT: u8 = 0b1100_0000;

#[derive(Debug)]
pub struct ParsedShred<'a> {
    pub slot: u64,
    pub index: u32,
    pub payload: &'a [u8],
    pub batch_complete: bool,
    pub last_in_slot: bool,
}

pub fn parse_shred(raw: &[u8]) -> Option<ParsedShred<'_>> {
    if raw.len() < DATA_HEADER_SIZE {
        return None;
    }

    let slot = u64::from_le_bytes(raw[SLOT_OFFSET..SLOT_OFFSET + 8].try_into().unwrap());
    let index = u32::from_le_bytes(raw[INDEX_OFFSET..INDEX_OFFSET + 4].try_into().unwrap());
    let flags = raw[FLAGS_OFFSET];
    let size = u16::from_le_bytes(raw[SIZE_OFFSET..SIZE_OFFSET + 2].try_into().unwrap()) as usize;

    if size > raw.len() {
        return None;
    }

    let payload = if size > DATA_HEADER_SIZE {
        &raw[DATA_HEADER_SIZE..size]
    } else {
        &[]
    };

    Some(ParsedShred {
        slot,
        index,
        payload,
        batch_complete: flags & DATA_COMPLETE != 0,
        last_in_slot: flags & LAST_IN_SLOT == LAST_IN_SLOT,
    })
}
