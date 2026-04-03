use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;

use socket2::{Domain, Protocol, Socket, Type};
use solana_transaction::versioned::VersionedTransaction;

use crate::accumulator::SlotAccumulator;
use crate::parser;

const DEFAULT_RECV_BUF: usize = 25 * 1024 * 1024;
const DEFAULT_MAX_AGE: u64 = 10;

pub struct ListenerOptions {
    pub recv_buf: usize,
    pub max_age: u64,
}

impl Default for ListenerOptions {
    fn default() -> Self {
        Self {
            recv_buf: DEFAULT_RECV_BUF,
            max_age: DEFAULT_MAX_AGE,
        }
    }
}

pub struct ShredListener {
    socket: std::net::UdpSocket,
    slots: HashMap<u64, SlotAccumulator>,
    max_age: u64,
    last_slot: u64,
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

        let std_socket: std::net::UdpSocket = raw.into();

        Ok(Self {
            socket: std_socket,
            slots: HashMap::new(),
            max_age: opts.max_age,
            last_slot: 0,
        })
    }

    pub fn transactions(&mut self) -> TransactionIter<'_> {
        TransactionIter {
            listener: self,
            buf: [0u8; 2048],
        }
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

    pub fn handle_packet(&mut self, raw: &[u8]) -> Option<(u64, Vec<VersionedTransaction>)> {
        let shred = parser::parse_shred(raw)?;

        let slot = shred.slot;

        if slot > self.last_slot {
            self.last_slot = slot;
        }

        let acc = self.slots.entry(slot).or_insert_with(SlotAccumulator::new);
        let prev_errors = acc.decode_errors();
        let txs = acc.push(
            shred.index,
            shred.payload.to_vec(),
            shred.batch_complete,
            shred.last_in_slot,
        );
        let new_errors = acc.decode_errors() - prev_errors;
        let slot_done = acc.slot_complete;

        if new_errors > 0 {
            self.slots.remove(&slot);
        } else if slot_done {
            self.slots.remove(&slot);
        }

        if self.last_slot > self.max_age {
            let min = self.last_slot - self.max_age;
            self.slots.retain(|&s, _| s >= min);
        }

        if txs.is_empty() {
            None
        } else {
            Some((slot, txs))
        }
    }

    fn recv_one(&mut self, buf: &mut [u8]) -> io::Result<Option<(u64, Vec<VersionedTransaction>)>> {
        let len = self.socket.recv(buf)?;
        Ok(self.handle_packet(&buf[..len]))
    }
}

pub struct RawShred {
    pub slot: u64,
    pub index: u32,
    pub payload_len: usize,
}

pub struct TransactionIter<'a> {
    listener: &'a mut ShredListener,
    buf: [u8; 2048],
}

impl Iterator for TransactionIter<'_> {
    type Item = (u64, Vec<VersionedTransaction>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.listener.recv_one(&mut self.buf) {
                Ok(Some(batch)) => return Some(batch),
                Ok(None) | Err(_) => continue,
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
            let len = self.listener.socket.recv(&mut self.buf).ok()?;
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
