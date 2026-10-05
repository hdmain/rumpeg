//! Reference-counted media buffers with optional pooling.
//!
//! Modeled after FFmpeg's `AVBuffer` / `AVBufferRef`: cheap clones share
//! ownership, and a [`BufferPool`] recycles allocations to cut allocator pressure
//! in hot decode/encode loops.

use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex};

/// Immutable, reference-counted byte buffer.
///
/// Cloning is `O(1)` and shares the underlying allocation. Use
/// [`Buffer::make_mut`] when exclusive write access is required.
#[derive(Clone, Debug, Default)]
pub struct Buffer {
    data: Arc<[u8]>,
}

impl Buffer {
    /// Create a buffer by copying `data`.
    pub fn from_slice(data: &[u8]) -> Self {
        Self {
            data: Arc::from(data.to_vec().into_boxed_slice()),
        }
    }

    /// Create a buffer from an owned `Vec<u8>` without an extra copy when unique.
    pub fn from_vec(data: Vec<u8>) -> Self {
        Self {
            data: Arc::from(data.into_boxed_slice()),
        }
    }

    /// Allocate a zeroed buffer of `len` bytes.
    pub fn zeroed(len: usize) -> Self {
        Self::from_vec(vec![0u8; len])
    }

    /// Allocate an uninitialized buffer of `len` bytes (filled with zeros for safety).
    pub fn with_capacity_len(len: usize) -> Self {
        Self::from_vec(vec![0u8; len])
    }

    /// Number of bytes in the buffer.
    #[inline]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Returns `true` if the buffer is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Borrow the bytes.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    /// Clone-on-write: return a uniquely owned mutable copy.
    pub fn make_mut(&mut self) -> &mut [u8] {
        if Arc::get_mut(&mut self.data).is_none() {
            let copy = self.data.to_vec();
            self.data = Arc::from(copy.into_boxed_slice());
        }
        // SAFETY: we just ensured uniqueness above.
        Arc::get_mut(&mut self.data).expect("buffer must be unique after make_mut")
    }

    /// Strong reference count (for diagnostics / pooling heuristics).
    #[inline]
    pub fn strong_count(&self) -> usize {
        Arc::strong_count(&self.data)
    }

    /// Consume into the inner `Arc`.
    pub fn into_arc(self) -> Arc<[u8]> {
        self.data
    }
}

impl Deref for Buffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl AsRef<[u8]> for Buffer {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl From<Vec<u8>> for Buffer {
    fn from(value: Vec<u8>) -> Self {
        Self::from_vec(value)
    }
}

impl From<&[u8]> for Buffer {
    fn from(value: &[u8]) -> Self {
        Self::from_slice(value)
    }
}

/// Thread-safe pool of reusable buffers sized for a fixed capacity.
///
/// Reduces allocation churn when packet/frame sizes are relatively stable
/// (common in streaming decode pipelines).
#[derive(Debug)]
pub struct BufferPool {
    capacity: usize,
    free: Mutex<Vec<Buffer>>,
    max_pooled: usize,
}

impl BufferPool {
    /// Create a pool that yields buffers of at least `capacity` bytes.
    pub fn new(capacity: usize, max_pooled: usize) -> Self {
        Self {
            capacity,
            free: Mutex::new(Vec::with_capacity(max_pooled.min(64))),
            max_pooled,
        }
    }

    /// Acquire a buffer, recycling a pooled one when possible.
    pub fn acquire(&self) -> Buffer {
        if let Ok(mut free) = self.free.lock() {
            if let Some(buf) = free.pop() {
                return buf;
            }
        }
        Buffer::zeroed(self.capacity)
    }

    /// Acquire a buffer large enough for `needed` bytes.
    pub fn acquire_at_least(&self, needed: usize) -> Buffer {
        if needed <= self.capacity {
            self.acquire()
        } else {
            Buffer::zeroed(needed)
        }
    }

    /// Return a uniquely-owned buffer to the pool.
    pub fn release(&self, buf: Buffer) {
        if buf.len() < self.capacity || buf.strong_count() != 1 {
            return;
        }
        if let Ok(mut free) = self.free.lock() {
            if free.len() < self.max_pooled {
                free.push(buf);
            }
        }
    }

    /// Target buffer capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

/// Mutable view helper used when filling a freshly acquired buffer.
#[derive(Debug)]
pub struct BufferMut {
    inner: Vec<u8>,
}

impl BufferMut {
    /// Create a mutable buffer of `len` zeroed bytes.
    pub fn zeroed(len: usize) -> Self {
        Self {
            inner: vec![0u8; len],
        }
    }

    /// Freeze into an immutable [`Buffer`].
    pub fn freeze(self) -> Buffer {
        Buffer::from_vec(self.inner)
    }
}

impl Deref for BufferMut {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for BufferMut {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cow_make_mut() {
        let mut a = Buffer::from_slice(b"hello");
        let b = a.clone();
        assert_eq!(a.strong_count(), 2);
        a.make_mut()[0] = b'H';
        assert_eq!(&a[..], b"Hello");
        assert_eq!(&b[..], b"hello");
    }

    #[test]
    fn pool_reuses() {
        let pool = BufferPool::new(64, 4);
        let buf = pool.acquire();
        assert_eq!(buf.len(), 64);
        pool.release(buf);
        let again = pool.acquire();
        assert_eq!(again.len(), 64);
    }
}
