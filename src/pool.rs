use std::cell::UnsafeCell;
use std::sync::{Arc, Mutex};

pub const BUF_SIZE: usize = 2048;

pub struct ShredPool {
    buffers: Box<[UnsafeCell<[u8; BUF_SIZE]>]>,
    free: Mutex<Vec<u16>>,
}

unsafe impl Send for ShredPool {}
unsafe impl Sync for ShredPool {}

pub struct BufferHandle {
    pool: Arc<ShredPool>,
    idx: u16,
    len: u16,
}

impl ShredPool {
    pub fn new(capacity: usize) -> Arc<Self> {
        assert!(capacity > 0 && capacity <= u16::MAX as usize);
        let buffers: Box<[_]> = (0..capacity)
            .map(|_| UnsafeCell::new([0u8; BUF_SIZE]))
            .collect();
        let free: Vec<u16> = (0..capacity as u16).rev().collect();
        Arc::new(Self {
            buffers,
            free: Mutex::new(free),
        })
    }

    pub fn acquire(self: &Arc<Self>) -> Option<BufferHandle> {
        let idx = self.free.lock().unwrap().pop()?;
        Some(BufferHandle {
            pool: Arc::clone(self),
            idx,
            len: 0,
        })
    }

    fn release(&self, idx: u16) {
        self.free.lock().unwrap().push(idx);
    }
}

impl BufferHandle {
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe {
            let buf = &mut *self.pool.buffers[self.idx as usize].get();
            &mut buf[..]
        }
    }

    #[inline]
    pub fn set_len(&mut self, len: usize) {
        debug_assert!(len <= BUF_SIZE);
        self.len = len as u16;
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl std::ops::Deref for BufferHandle {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        unsafe {
            let buf = &*self.pool.buffers[self.idx as usize].get();
            &buf[..self.len as usize]
        }
    }
}

impl Drop for BufferHandle {
    fn drop(&mut self) {
        self.pool.release(self.idx);
    }
}
