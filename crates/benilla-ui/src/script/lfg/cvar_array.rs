//! A bit array kept in a CVar string, which `lfdSelectedDungeons` and `lfdCollapsedHeaders` use.
//! Byte 0 is the format version, 2; each later byte holds six bits over the base `'@'`, bit `i` in
//! byte `i / 6 + 1` at `1 << (i % 6)`.

/// The format byte the array's set writes (`0x766ca0`).
const VERSION: u8 = 2;
/// A byte with no bits set.
const EMPTY: u8 = b'@';
/// The set refuses a string at 255 bytes or more.
const MAX_LEN: usize = 255;

/// The array's get (`0x766860`): the bit, false for an empty string or past its end.
pub(super) fn get(value: &str, index: u32) -> bool {
    let bytes = value.as_bytes();
    let index = index as usize;
    !bytes.is_empty()
        && index < 6 * (bytes.len() - 1)
        && bytes[index / 6 + 1] & (1 << (index % 6)) != 0
}

/// The array's set (`0x766ca0`): the string with the bit set or cleared, `None` when the array
/// cannot hold it. A set past the end pads with `'@'`; a clear trims the trailing `'@'`s.
/// Deviation: byte 0 is always the version, where the reference's pad overwrites it with `'@'`
/// when it grows an empty string, which every later read ignores.
pub(super) fn set(value: &str, index: u32, on: bool) -> Option<String> {
    let mut bytes = value.as_bytes().to_vec();
    if bytes.len() >= MAX_LEN {
        return None;
    }
    let index = index as usize;
    let pos = index / 6 + 1;
    if pos + 1 >= 256 {
        return None;
    }
    if bytes.is_empty() {
        bytes.push(VERSION);
    }
    bytes[0] = VERSION;
    if on {
        if bytes.len() <= pos {
            bytes.resize(pos + 1, EMPTY);
        }
        bytes[pos] |= 1 << (index % 6);
    } else {
        if pos >= bytes.len() {
            return Some(String::from_utf8_lossy(&bytes).into_owned());
        }
        bytes[pos] &= !(1 << (index % 6));
        while bytes.len() > 1 && bytes[bytes.len() - 1] == EMPTY {
            bytes.pop();
        }
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_round_trip_and_clear_trims_the_tail() {
        let s = set("", 261, true).unwrap();
        assert_eq!(s.len(), 261 / 6 + 2);
        assert!(get(&s, 261));
        assert!(!get(&s, 260) && !get(&s, 262));
        let s = set(&s, 3, true).unwrap();
        assert!(get(&s, 3) && get(&s, 261));
        let s = set(&s, 261, false).unwrap();
        assert!(!get(&s, 261) && get(&s, 3));
        assert_eq!(s.len(), 2, "the cleared tail is trimmed");
        let s = set(&s, 3, false).unwrap();
        assert_eq!(s.as_bytes(), [VERSION]);
        assert!(!get("", 0));
    }
}
