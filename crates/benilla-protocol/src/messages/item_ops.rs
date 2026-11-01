//! The item round trips beyond moving and using: socketing gems, the purchase refund record and its
//! refund, a book's read check, a used GameObject's page and the item-name cache.

use std::io;

use crate::wire::{read_cstring, read_u32_le, read_u64_le, read_u8};

/// Body of `CMSG_SOCKET_GEMS` (`0x5c4ff0`): the socketed item, then each socket's gem, 0 for none.
pub fn socket_gems(item: u64, gems: [u64; 3]) -> Vec<u8> {
    let mut body = Vec::with_capacity(32);
    body.extend_from_slice(&item.to_le_bytes());
    for gem in gems {
        body.extend_from_slice(&gem.to_le_bytes());
    }
    body
}

/// Read `SMSG_SOCKET_GEMS_RESULT` (`0x6e6330` → `0x7081b0`): the item, then the enchants of its
/// three sockets and its socket bonus, enchant slots 2 to 5; a 0 leaves its slot as it was.
pub(super) fn read_socket_gems_result(r: &mut &[u8]) -> io::Result<(u64, [u32; 4])> {
    let item = read_u64_le(r)?;
    let mut enchants = [0u32; 4];
    for e in &mut enchants {
        *e = read_u32_le(r)?;
    }
    Ok((item, enchants))
}

/// Body of `CMSG_ITEM_REFUND_INFO` (`0x7089e0`) and `CMSG_ITEM_REFUND` (`0x5d91b0`): the item.
pub fn item_guid_body(item: u64) -> Vec<u8> {
    item.to_le_bytes().to_vec()
}

/// What a purchase cost, as the refund record (`item+0x3d4`) and the refund result hold it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PurchaseCost {
    pub money: u32,
    pub honor: u32,
    pub arena: u32,
    /// `(item id, count)` per cost item, in wire order; a 0 id is no item.
    pub items: [(u32, u32); 5],
}

fn read_purchase_cost(r: &mut &[u8]) -> io::Result<PurchaseCost> {
    let money = read_u32_le(r)?;
    let honor = read_u32_le(r)?;
    let arena = read_u32_le(r)?;
    let mut items = [(0, 0); 5];
    for it in &mut items {
        *it = (read_u32_le(r)?, read_u32_le(r)?);
    }
    Ok(PurchaseCost {
        money,
        honor,
        arena,
        items,
    })
}

/// One item's refund record, `SMSG_ITEM_REFUND_INFO_RESPONSE` (`0x6d1650`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RefundRecord {
    pub cost: PurchaseCost,
    /// The player's total played seconds at the purchase (`record[0xe]`); the window is open while
    /// it is more than the played seconds now less 7200.
    pub played_at_purchase: u32,
}

/// Read `SMSG_ITEM_REFUND_INFO_RESPONSE`: the item, the cost, a `u32` neither verb reads, then the
/// played-time stamp.
pub(super) fn read_item_refund_info(r: &mut &[u8]) -> io::Result<(u64, RefundRecord)> {
    let item = read_u64_le(r)?;
    let cost = read_purchase_cost(r)?;
    let _unread = read_u32_le(r)?;
    let played_at_purchase = read_u32_le(r)?;
    Ok((
        item,
        RefundRecord {
            cost,
            played_at_purchase,
        },
    ))
}

/// Read `SMSG_ITEM_REFUND_RESULT` (`0x6d9b40`): the item and the result; result 0 carries what was
/// refunded.
pub(super) fn read_item_refund_result(
    r: &mut &[u8],
) -> io::Result<(u64, u32, Option<PurchaseCost>)> {
    let item = read_u64_le(r)?;
    let result = read_u32_le(r)?;
    let cost = match result {
        0 => Some(read_purchase_cost(r)?),
        _ => None,
    };
    Ok((item, result, cost))
}

/// Body of `CMSG_READ_ITEM` (`0x6d5c90`): the bag (`0xff` for the backpack, else the bag's
/// inventory slot) and the 0-based slot.
pub fn read_item(bag: u8, slot: u8) -> Vec<u8> {
    vec![bag, slot]
}

/// How `SMSG_READ_ITEM_FAILED`'s reason routes the reader (`0x6d5d90`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadItemFailure {
    /// 0: the text shows translated.
    Show,
    /// 1: the text shows untranslated, and the translation bar runs for `ms`.
    Untranslated { ms: u32 },
    /// 2 and above, or no reason at all: the reader closes. AzerothCore sends the guid alone, where
    /// 12340 reads past the body into a stale byte.
    Close,
}

/// Read `SMSG_READ_ITEM_FAILED`: the item, then the reason byte and for reason 1 a `u32` ms.
pub(super) fn read_read_item_failed(r: &mut &[u8]) -> io::Result<(u64, ReadItemFailure)> {
    let item = read_u64_le(r)?;
    let failure = match read_u8(r).ok() {
        Some(0) => ReadItemFailure::Show,
        Some(1) => ReadItemFailure::Untranslated {
            ms: read_u32_le(r)?,
        },
        _ => ReadItemFailure::Close,
    };
    Ok((item, failure))
}

/// Body of `CMSG_ITEM_NAME_QUERY` (`0x668a20`): the entry, then a `u64` guid the cache passes as 0.
pub fn item_name_query(entry: u32) -> Vec<u8> {
    let mut body = entry.to_le_bytes().to_vec();
    body.extend_from_slice(&0u64.to_le_bytes());
    body
}

/// Read `SMSG_ITEM_NAME_QUERY_RESPONSE` (`0x6351d0` → `0x98d8c0`): the entry, then the name and
/// `InventoryType`; an entry with the high bit set is the server's "unknown" and carries neither.
pub(super) fn read_item_name_query_response(
    r: &mut &[u8],
) -> io::Result<(u32, Option<(String, u32)>)> {
    let entry = read_u32_le(r)?;
    if entry & 0x8000_0000 != 0 {
        return Ok((entry & 0x7fff_ffff, None));
    }
    let name = read_cstring(r)?;
    let inventory_type = read_u32_le(r)?;
    Ok((entry, Some((name, inventory_type))))
}

/// One worn item of an inspected player (`0x6cdf30`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InspectItem {
    /// The inventory slot, 0-based (0 head .. 18 tabard).
    pub slot: u8,
    pub entry: u32,
    /// A negative wire entry: the item counts toward its wearer's gem conditions (`0x6de4d0`).
    pub counts_gems: bool,
    /// The twelve enchant slots, 0 for an empty one.
    pub enchants: [u32; 12],
    pub random_property: i16,
    pub creator: u64,
    pub seed: u32,
}

/// Read `SMSG_INSPECT_TALENT` (`0x6ce0c0`): the packed guid, the player talent body, then the worn
/// items: a `u32` slot mask and per set bit the entry, a `u16` enchant mask with a `u16` per set bit,
/// a `u16` random property, the creator's packed guid and a `u32` seed. Only the items are kept.
pub(super) fn read_inspect_talent(r: &mut &[u8]) -> io::Result<(u64, Vec<InspectItem>)> {
    let guid = crate::wire::read_packed_guid(r)?;
    super::talents::read_player_talents(r)?;
    let mask = read_u32_le(r)?;
    let mut items = Vec::new();
    for slot in 0..19u8 {
        if mask & (1 << slot) == 0 {
            continue;
        }
        let raw = read_u32_le(r)? as i32;
        let enchant_mask = crate::wire::read_u16_le(r)?;
        let mut enchants = [0u32; 12];
        for (i, e) in enchants.iter_mut().enumerate() {
            if enchant_mask & (1 << i) != 0 {
                *e = u32::from(crate::wire::read_u16_le(r)?);
            }
        }
        let random_property = crate::wire::read_u16_le(r)? as i16;
        let creator = crate::wire::read_packed_guid(r)?;
        let seed = read_u32_le(r)?;
        items.push(InspectItem {
            slot,
            entry: raw.unsigned_abs(),
            counts_gems: raw < 0,
            enchants,
            random_property,
            creator,
            seed,
        });
    }
    Ok((guid, items))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{opcode, parse_server, ServerPacket};

    #[test]
    fn socket_gems_is_the_item_then_three_gems() {
        let body = socket_gems(0x40_0000_0001, [0x40_0000_0002, 0, 0x40_0000_0003]);
        assert_eq!(body.len(), 32);
        assert_eq!(&body[..8], &0x40_0000_0001u64.to_le_bytes());
        assert_eq!(&body[16..24], &[0; 8], "an empty socket is a 0 guid");
    }

    #[test]
    fn the_refund_record_reads_cost_pairs_then_the_stamp() {
        let mut b = 7u64.to_le_bytes().to_vec();
        for v in [100u32, 2000, 0, 29434, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3600] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        match parse_server(opcode::SMSG_ITEM_REFUND_INFO_RESPONSE, &b).unwrap() {
            ServerPacket::ItemRefundInfo { item, record } => {
                assert_eq!(item, 7);
                assert_eq!(record.cost.money, 100);
                assert_eq!(record.cost.honor, 2000);
                assert_eq!(record.cost.items[0], (29434, 2));
                assert_eq!(record.played_at_purchase, 3600);
            }
            other => panic!("got {}", other.name()),
        }
    }

    #[test]
    fn a_failed_refund_carries_no_cost() {
        let mut b = 7u64.to_le_bytes().to_vec();
        b.extend_from_slice(&10u32.to_le_bytes());
        assert!(matches!(
            parse_server(opcode::SMSG_ITEM_REFUND_RESULT, &b).unwrap(),
            ServerPacket::ItemRefundResult {
                result: 10,
                cost: None,
                ..
            }
        ));
    }

    #[test]
    fn a_read_failure_without_its_reason_closes() {
        let b = 7u64.to_le_bytes().to_vec();
        assert!(matches!(
            parse_server(opcode::SMSG_READ_ITEM_FAILED, &b).unwrap(),
            ServerPacket::ReadItemFailed {
                failure: ReadItemFailure::Close,
                ..
            }
        ));
        let mut b = 7u64.to_le_bytes().to_vec();
        b.push(1);
        b.extend_from_slice(&1500u32.to_le_bytes());
        assert!(matches!(
            parse_server(opcode::SMSG_READ_ITEM_FAILED, &b).unwrap(),
            ServerPacket::ReadItemFailed {
                failure: ReadItemFailure::Untranslated { ms: 1500 },
                ..
            }
        ));
    }

    #[test]
    fn an_item_name_answer_and_its_unknown() {
        let mut b = 6303u32.to_le_bytes().to_vec();
        b.extend_from_slice(b"Defias Mark\0");
        b.extend_from_slice(&5u32.to_le_bytes());
        assert!(matches!(
            parse_server(opcode::SMSG_ITEM_NAME_QUERY_RESPONSE, &b).unwrap(),
            ServerPacket::ItemNameQueryResponse { entry: 6303, name: Some((ref n, 5)) } if n == "Defias Mark"
        ));
        let b = (6303u32 | 0x8000_0000).to_le_bytes().to_vec();
        assert!(matches!(
            parse_server(opcode::SMSG_ITEM_NAME_QUERY_RESPONSE, &b).unwrap(),
            ServerPacket::ItemNameQueryResponse {
                entry: 6303,
                name: None
            }
        ));
    }

    /// The talent body is read past, then each worn item with its enchants by mask.
    #[test]
    fn an_inspect_talent_reads_the_worn_items() {
        let mut b = vec![0x01, 0x07]; // packed guid 7
        b.extend_from_slice(&0u32.to_le_bytes()); // unspent
        b.extend_from_slice(&[1, 0]); // one group, active 0
        b.extend_from_slice(&[1]); // one talent
        b.extend_from_slice(&1234u32.to_le_bytes());
        b.push(2);
        b.push(0); // no glyphs
        b.extend_from_slice(&0b10u32.to_le_bytes()); // slot 1, the neck
        b.extend_from_slice(&(-40_000i32).to_le_bytes());
        b.extend_from_slice(&0b100u16.to_le_bytes()); // enchant slot 2, a gem
        b.extend_from_slice(&3628u16.to_le_bytes());
        b.extend_from_slice(&(-5i16).to_le_bytes());
        b.push(0); // creator, empty packed guid
        b.extend_from_slice(&9u32.to_le_bytes());
        match parse_server(opcode::SMSG_INSPECT_TALENT, &b).unwrap() {
            ServerPacket::InspectItems { guid, items } => {
                assert_eq!(guid, 7);
                assert_eq!(items.len(), 1);
                let it = &items[0];
                assert_eq!((it.slot, it.entry, it.counts_gems), (1, 40_000, true));
                assert_eq!(it.enchants[2], 3628);
                assert_eq!((it.random_property, it.seed), (-5, 9));
            }
            other => panic!("got {}", other.name()),
        }
    }

    #[test]
    fn the_socket_result_reads_four_enchants() {
        let mut b = 7u64.to_le_bytes().to_vec();
        for v in [3879u32, 0, 0, 2872] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        assert!(matches!(
            parse_server(opcode::SMSG_SOCKET_GEMS_RESULT, &b).unwrap(),
            ServerPacket::SocketGemsResult {
                item: 7,
                enchants: [3879, 0, 0, 2872]
            }
        ));
    }
}
