//! A big-endian bit reader over one packet, reading zeros past its end so a damaged packet ends
//! in a decode error instead of a panic.

pub(crate) struct Bits<'a> {
    data: &'a [u8],
    /// Position in bits.
    pos: usize,
}

impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn left(&self) -> isize {
        (self.data.len() * 8) as isize - self.pos as isize
    }

    /// The next 32 bits without consuming them.
    #[inline]
    pub fn peek32(&self) -> u32 {
        let byte = self.pos >> 3;
        let mut w = [0u8; 8];
        let n = self.data.len().saturating_sub(byte).min(8);
        if n > 0 {
            w[..n].copy_from_slice(&self.data[byte..byte + n]);
        }
        ((u64::from_be_bytes(w) << (self.pos & 7)) >> 32) as u32
    }

    #[inline]
    pub fn peek(&self, n: u32) -> u32 {
        debug_assert!((1..=32).contains(&n));
        self.peek32() >> (32 - n)
    }

    #[inline]
    pub fn skip(&mut self, n: u32) {
        self.pos += n as usize;
    }

    #[inline]
    pub fn read(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let v = self.peek(n);
        self.skip(n);
        v
    }

    #[inline]
    pub fn bit(&mut self) -> bool {
        self.read(1) == 1
    }

    /// An `n`-bit field in two's complement.
    #[inline]
    pub fn read_signed(&mut self, n: u32) -> i32 {
        let v = self.read(n) as i32;
        (v << (32 - n)) >> (32 - n)
    }

    /// The byte holding the reader's position.
    pub fn byte_pos(&self) -> usize {
        self.pos / 8
    }
}

/// A prefix-code table: every code up to `max` bits, looked up by peeking `max` bits.
pub(crate) struct Vlc {
    max: u32,
    /// Indexed by the next `max` bits: `(symbol, length)`; length 0 is no code.
    table: Vec<(u16, u8)>,
}

impl Vlc {
    /// `codes[i]` is `(code, length)` for symbol `i`; length 0 skips the symbol.
    pub fn new(codes: &[(u32, u8)]) -> Self {
        let max = u32::from(codes.iter().map(|c| c.1).max().unwrap_or(1));
        let mut table = vec![(0u16, 0u8); 1 << max];
        for (sym, &(code, len)) in codes.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let shift = max - u32::from(len);
            let base = (code as usize) << shift;
            for slot in &mut table[base..base + (1 << shift)] {
                assert_eq!(slot.1, 0, "codes are not a prefix code");
                *slot = (sym as u16, len);
            }
        }
        Self { max, table }
    }

    /// The next symbol, or `None` on a bit pattern that is no code.
    #[inline]
    pub fn read(&self, b: &mut Bits) -> Option<usize> {
        let (sym, len) = self.table[b.peek(self.max) as usize];
        if len == 0 {
            return None;
        }
        b.skip(u32::from(len));
        Some(sym as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_across_bytes_and_past_the_end() {
        let mut b = Bits::new(&[0b1010_1100, 0b0101_0011]);
        assert_eq!(b.read(3), 0b101);
        assert_eq!(b.read(7), 0b011_0001);
        assert_eq!(b.read_signed(3), 2);
        assert_eq!(b.read(3), 0b011);
        assert_eq!(b.left(), 0);
        assert_eq!(b.read(8), 0);
        assert_eq!(Bits::new(&[0b1110_0000]).read_signed(3), -1);
    }

    #[test]
    fn decodes_a_prefix_code() {
        let v = Vlc::new(&[(0b1, 1), (0b01, 2), (0b001, 3)]);
        let mut b = Bits::new(&[0b1010_0100]);
        assert_eq!(v.read(&mut b), Some(0));
        assert_eq!(v.read(&mut b), Some(1));
        assert_eq!(v.read(&mut b), Some(2));
        assert_eq!(v.read(&mut b), None);
    }
}
