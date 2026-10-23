//! A fast hasher for the model's handle-keyed maps. A [`super::FrameHandle`] or
//! [`super::RegionHandle`] is two `u32`s the arena hands out, never attacker-chosen, so the
//! default SipHash's flood resistance buys nothing on lookups the UI makes thousands of times a
//! frame. This is the Fx multiply-rotate mix.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

/// A `HashMap` keyed by arena handles, hashed with [`HandleHasher`].
pub type HandleMap<K, V> = HashMap<K, V, BuildHasherDefault<HandleHasher>>;

/// A `HashSet` of arena handles, hashed with [`HandleHasher`].
pub type HandleSet<K> = HashSet<K, BuildHasherDefault<HandleHasher>>;

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

/// The Fx word mix: `hash = (hash.rotl(5) ^ word) * SEED` per word.
#[derive(Default, Clone, Copy)]
pub struct HandleHasher {
    hash: u64,
}

impl HandleHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for HandleHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        for w in words {
            self.add(u64::from_le_bytes(*w));
        }
        for &b in rest {
            self.add(u64::from(b));
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A handle hashes as its two `u32` halves (the derived `Hash`); swapping or bumping either
    /// half must move the hash.
    #[test]
    fn handles_that_differ_in_either_half_hash_apart() {
        let h = |index: u32, generation: u32| {
            let mut s = HandleHasher::default();
            s.write_u32(index);
            s.write_u32(generation);
            s.finish()
        };
        assert_ne!(h(1, 0), h(0, 1));
        assert_ne!(h(1, 1), h(2, 1));
        assert_ne!(h(1, 1), h(1, 2));
    }
}
