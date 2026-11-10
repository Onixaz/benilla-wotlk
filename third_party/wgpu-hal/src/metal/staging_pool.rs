//! benilla patch: transient staging buffers are recycled instead of allocated per write.
//!
//! `wgpu-core` stages every `Queue::write_buffer` and `write_texture` through a fresh
//! `MAP_WRITE | COPY_SRC` buffer flagged [`crate::MemoryFlags::TRANSIENT`], and destroys it once
//! the submission that read it has completed. On Metal each one is a whole `MTLBuffer`: created
//! under the device mutex, so parallel render systems queue on it, and released through IOKit on
//! the thread that triages submissions. Here a destroyed staging buffer returns to a free list
//! by power-of-two size class, and the next staging request of that class takes it back.

use alloc::vec::Vec;

/// The smallest class, 4 KiB: smaller requests share it.
const MIN_CLASS: u32 = 12;
/// The largest class kept, 16 MiB; a bigger staging buffer is allocated and freed as upstream.
const MAX_CLASS: u32 = 24;
const CLASSES: usize = (MAX_CLASS - MIN_CLASS + 1) as usize;
/// The most the free lists hold, in bytes; a buffer returned past it is released.
const MAX_POOLED_BYTES: u64 = 64 << 20;

/// The size class a staging request of `size` bytes falls in, or `None` past [`MAX_CLASS`].
pub(super) fn class_for(size: u64) -> Option<u8> {
    let bits = size.max(1).checked_next_power_of_two()?.trailing_zeros();
    (bits <= MAX_CLASS).then_some(bits.max(MIN_CLASS) as u8)
}

/// The bytes a buffer of `class` is allocated with.
pub(super) fn class_bytes(class: u8) -> u64 {
    1 << class
}

/// Free staging buffers by size class.
#[derive(Default)]
pub(super) struct StagingPool {
    free: [Vec<metal::Buffer>; CLASSES],
    bytes: u64,
}

impl StagingPool {
    /// A free buffer of `class`, if one is pooled.
    pub(super) fn take(&mut self, class: u8) -> Option<metal::Buffer> {
        let raw = self.free[(class as u32 - MIN_CLASS) as usize].pop()?;
        self.bytes -= class_bytes(class);
        Some(raw)
    }

    /// Pool `raw`, a buffer of `class` the GPU is done with; past the cap it is handed back for
    /// the caller to release outside the lock.
    pub(super) fn give(&mut self, class: u8, raw: metal::Buffer) -> Option<metal::Buffer> {
        if self.bytes + class_bytes(class) > MAX_POOLED_BYTES {
            return Some(raw);
        }
        self.bytes += class_bytes(class);
        self.free[(class as u32 - MIN_CLASS) as usize].push(raw);
        None
    }
}
