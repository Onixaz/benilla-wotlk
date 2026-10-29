//! The barber shop's wire: the chair's `SMSG_ENABLE_BARBER_SHOP` (no body), the paid change
//! `CMSG_ALTER_APPEARANCE` and its `SMSG_BARBER_SHOP_RESULT`.

use std::io::{self, Read};

use crate::wire::read_u32_le;

/// Body of `CMSG_ALTER_APPEARANCE` (0x426, client `0x52eda0`): the hair style's
/// `BarberShopStyle` id, the hair colour, the facial hair's style id, and the skin's style id or
/// 0 where the race has no skin styles.
pub fn alter_appearance(hair_style: u32, hair_color: u32, facial_hair: u32, skin: u32) -> Vec<u8> {
    [hair_style, hair_color, facial_hair, skin]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect()
}

/// `SMSG_BARBER_SHOP_RESULT` (0x428, client `0x52e5b0`): one `u32`, 0 success, 1 and 3 not enough
/// money, 2 not sitting.
pub(super) fn read_barber_shop_result(r: &mut impl Read) -> io::Result<u32> {
    read_u32_le(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_change_is_four_words_in_the_reference_order() {
        let body = alter_appearance(12, 3, 40, 0);
        assert_eq!(body.len(), 16);
        assert_eq!(&body[..4], &12u32.to_le_bytes());
        assert_eq!(&body[4..8], &3u32.to_le_bytes());
        assert_eq!(&body[8..12], &40u32.to_le_bytes());
        assert_eq!(&body[12..], &0u32.to_le_bytes());
    }

    #[test]
    fn the_result_reads_its_one_word() {
        assert_eq!(
            read_barber_shop_result(&mut &2u32.to_le_bytes()[..]).unwrap(),
            2
        );
    }
}
