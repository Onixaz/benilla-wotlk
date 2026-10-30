//! Player-to-player trade wire: the trade window's request verbs and the two status packets the
//! server pushes back, read as 12340's handlers `0x7044a0` and `0x704680` read them.

use std::io;

use crate::wire::{read_i32_le, read_u32_le, read_u64_le, read_u8};

/// Slots per side (`TradeData.h`): six traded plus the non-traded enchant slot.
pub const TRADE_SLOT_COUNT: usize = 7;
/// Slots 0..6, whose items change hands.
pub const TRADE_SLOT_TRADED_COUNT: usize = 6;
/// UI slot 7: its item stays with its owner as the target of an enchant or lockpick spell.
pub const TRADE_SLOT_NONTRADED: usize = 6;

/// `SMSG_TRADE_STATUS` codes (`SharedDefines.h` `enum TradeStatus`), each with its tail inline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeStatus {
    /// The target is busy, or the initiator is already trading.
    Busy,
    /// To the target of `CMSG_INITIATE_TRADE`; its `CMSG_BEGIN_TRADE` opens both windows.
    BeginTrade { partner: u64 },
    /// Opens the window on both sides; `trade_id` is the id every snapshot must carry (`0x70453e`).
    OpenWindow { trade_id: u32 },
    /// The trade was cancelled; both windows close.
    Canceled,
    /// The partner pressed Trade (the accept highlight on their column).
    Accept,
    /// A second busy code, handled as `Busy`.
    Busy2,
    /// No such target for `CMSG_INITIATE_TRADE`.
    NoTarget,
    /// An offer changed after an accept, or the 200 ms scam delay bounced one: the accept drops.
    BackToTrade,
    /// Both sides accepted and the swap completed; both windows close.
    Complete,
    /// The trade was rejected.
    Rejected,
    /// Out of `TRADE_DISTANCE`; also the refusal to initiate while flying or off-map.
    TargetTooFar,
    /// Cross-faction, unless the server allows two-side interaction.
    WrongFaction,
    /// The tail is usually zero; the `u8` vmangos writes between the two fields is dropped.
    CloseWindow {
        result: u32,
        item_limit_category: u32,
    },
    /// Per a vmangos note, handled as `Canceled`; kept distinct so the code round-trips.
    Unknown13,
    /// The target has you on ignore.
    IgnoreYou,
    /// You are stunned.
    YouStunned,
    /// The target is stunned.
    TargetStunned,
    /// You are dead.
    YouDead,
    /// The target is dead.
    TargetDead,
    /// You are logging out.
    YouLogout,
    /// The target is logging out.
    TargetLogout,
    /// A trial account restriction.
    TrialAccount,
    /// "You can only trade conjured items"; `slot` is the offending trade slot.
    OnlyConjured { slot: u8 },
    /// The item is soulbound to a tap list the partner is not on; `slot` is its trade slot.
    NotEligible { slot: u8 },
    /// A code outside 0..=23; read with no tail.
    Unknown(u32),
}

impl TradeStatus {
    /// The `u32` status code on the wire.
    pub fn code(self) -> u32 {
        match self {
            TradeStatus::Busy => 0,
            TradeStatus::BeginTrade { .. } => 1,
            TradeStatus::OpenWindow { .. } => 2,
            TradeStatus::Canceled => 3,
            TradeStatus::Accept => 4,
            TradeStatus::Busy2 => 5,
            TradeStatus::NoTarget => 6,
            TradeStatus::BackToTrade => 7,
            TradeStatus::Complete => 8,
            TradeStatus::Rejected => 9,
            TradeStatus::TargetTooFar => 10,
            TradeStatus::WrongFaction => 11,
            TradeStatus::CloseWindow { .. } => 12,
            TradeStatus::Unknown13 => 13,
            TradeStatus::IgnoreYou => 14,
            TradeStatus::YouStunned => 15,
            TradeStatus::TargetStunned => 16,
            TradeStatus::YouDead => 17,
            TradeStatus::TargetDead => 18,
            TradeStatus::YouLogout => 19,
            TradeStatus::TargetLogout => 20,
            TradeStatus::TrialAccount => 21,
            TradeStatus::OnlyConjured { .. } => 22,
            TradeStatus::NotEligible { .. } => 23,
            TradeStatus::Unknown(code) => code,
        }
    }
}

/// One trade slot's 72-byte item block; an all-zero block is an empty slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TradeItem {
    /// `Item.dbc` entry (`OBJECT_FIELD_ENTRY`), the key for name and icon via `ITEM_QUERY_SINGLE`.
    pub entry: u32,
    pub display_id: u32,
    pub count: u32,
    /// A wrapped gift: the client hides stats and shows the gift-creator name.
    pub wrapped: bool,
    pub gift_creator: u64,
    pub perm_enchant: u32,
    /// The three socket enchantments, the gems.
    pub socket_enchants: [u32; 3],
    pub creator: u64,
    /// Signed: a negative count means N uses left on some items.
    pub charges: i32,
    pub suffix_factor: u32,
    pub random_prop_id: u32,
    pub lock_id: u32,
    pub max_durability: u32,
    pub durability: u32,
}

/// `SMSG_TRADE_STATUS_EXTENDED`: one side's snapshot, sent whenever that side's offer changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TradeStatusExtended {
    /// The partner's (right) column when the wire byte is 1, ours (left) when 0.
    pub their_window: bool,
    /// The trade's id; one other than `OPEN_WINDOW`'s cancels the trade (`0x7046ae`).
    pub trade_id: u32,
    /// The sender's count of our own changes; an own snapshot behind ours is stale (`0x7046cf`).
    pub state_index: u32,
    /// This side's offer state, which our `CMSG_ACCEPT_TRADE` echoes for the partner's (`0x7046eb`).
    pub side_state: u32,
    /// In copper.
    pub gold: u32,
    /// The spell cast on this side's non-traded slot item, 0 for none.
    pub enchant_spell_id: u32,
    /// Every slot the snapshot lists; one it omits is empty (`0x704852`).
    pub slots: [Option<TradeItem>; TRADE_SLOT_COUNT],
}

// ── Client → server bodies ──

/// `CMSG_INITIATE_TRADE`: a refusal is a status back to us; success sends the target `BeginTrade`.
pub fn initiate_trade(target: u64) -> Vec<u8> {
    target.to_le_bytes().to_vec()
}

/// `CMSG_ACCEPT_TRADE`: the partner's offer state being accepted.
pub fn accept_trade(their_state: u32) -> Vec<u8> {
    their_state.to_le_bytes().to_vec()
}

/// `CMSG_SET_TRADE_ITEM`: puts the item at (`bag`, `slot`) into `trade_slot`.
pub fn set_trade_item(trade_slot: u8, bag: u8, slot: u8) -> Vec<u8> {
    vec![trade_slot, bag, slot]
}

/// `CMSG_CLEAR_TRADE_ITEM` body.
pub fn clear_trade_item(trade_slot: u8) -> Vec<u8> {
    vec![trade_slot]
}

/// `CMSG_SET_TRADE_GOLD` body, in copper.
pub fn set_trade_gold(copper: u32) -> Vec<u8> {
    copper.to_le_bytes().to_vec()
}

// `CMSG_BEGIN_TRADE`, `CMSG_BUSY_TRADE`, `CMSG_IGNORE_TRADE`, `CMSG_UNACCEPT_TRADE` and
// `CMSG_CANCEL_TRADE` have empty bodies (vmangos `NullClientPacket`), so they need no builder.

// ── Server → client parses ──

/// `SMSG_TRADE_STATUS` as `0x7044a0` reads it: a `u32` code, then its tail.
pub(super) fn read_trade_status(r: &mut &[u8]) -> io::Result<TradeStatus> {
    let code = read_u32_le(r)?;
    Ok(match code {
        0 => TradeStatus::Busy,
        1 => TradeStatus::BeginTrade {
            partner: read_u64_le(r)?,
        },
        2 => TradeStatus::OpenWindow {
            trade_id: read_u32_le(r)?,
        },
        3 => TradeStatus::Canceled,
        4 => TradeStatus::Accept,
        5 => TradeStatus::Busy2,
        6 => TradeStatus::NoTarget,
        7 => TradeStatus::BackToTrade,
        8 => TradeStatus::Complete,
        9 => TradeStatus::Rejected,
        10 => TradeStatus::TargetTooFar,
        11 => TradeStatus::WrongFaction,
        12 => {
            let result = read_u32_le(r)?;
            let _unk = read_u8(r)?;
            let item_limit_category = read_u32_le(r)?;
            TradeStatus::CloseWindow {
                result,
                item_limit_category,
            }
        }
        13 => TradeStatus::Unknown13,
        14 => TradeStatus::IgnoreYou,
        15 => TradeStatus::YouStunned,
        16 => TradeStatus::TargetStunned,
        17 => TradeStatus::YouDead,
        18 => TradeStatus::TargetDead,
        19 => TradeStatus::YouLogout,
        20 => TradeStatus::TargetLogout,
        21 => TradeStatus::TrialAccount,
        22 => TradeStatus::OnlyConjured { slot: read_u8(r)? },
        23 => TradeStatus::NotEligible { slot: read_u8(r)? },
        other => TradeStatus::Unknown(other),
    })
}

/// `SMSG_TRADE_STATUS_EXTENDED` as `0x704680` reads it: `u8` side, trade id, state index, a
/// per-side `u32`, gold, enchant spell, then `u8` slot and a 72-byte item block until the body ends.
pub(super) fn read_trade_status_extended(r: &mut &[u8]) -> io::Result<TradeStatusExtended> {
    let which = read_u8(r)?;
    let trade_id = read_u32_le(r)?;
    let state_index = read_u32_le(r)?;
    let side_state = read_u32_le(r)?;
    let gold = read_u32_le(r)?;
    let enchant_spell_id = read_u32_le(r)?;

    let mut slots: [Option<TradeItem>; TRADE_SLOT_COUNT] = [None; TRADE_SLOT_COUNT];
    while !r.is_empty() {
        let index = read_u8(r)? as usize;
        let entry = read_u32_le(r)?;
        let display_id = read_u32_le(r)?;
        let count = read_u32_le(r)?;
        let wrapped = read_u32_le(r)? != 0;
        let gift_creator = read_u64_le(r)?;
        let perm_enchant = read_u32_le(r)?;
        let socket_enchants = [read_u32_le(r)?, read_u32_le(r)?, read_u32_le(r)?];
        let creator = read_u64_le(r)?;
        let charges = read_i32_le(r)?;
        let suffix_factor = read_u32_le(r)?;
        let random_prop_id = read_u32_le(r)?;
        let lock_id = read_u32_le(r)?;
        let max_durability = read_u32_le(r)?;
        let durability = read_u32_le(r)?;
        // Deviation: 12340 indexes its 7-slot mirror with the wire byte unchecked; past it, dropped.
        if entry != 0 && index < TRADE_SLOT_COUNT {
            slots[index] = Some(TradeItem {
                entry,
                display_id,
                count,
                wrapped,
                gift_creator,
                perm_enchant,
                socket_enchants,
                creator,
                charges,
                suffix_factor,
                random_prop_id,
                lock_id,
                max_durability,
                durability,
            });
        }
    }
    Ok(TradeStatusExtended {
        their_window: which == 1,
        trade_id,
        state_index,
        side_state,
        gold,
        enchant_spell_id,
        slots,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── CMSG body goldens ──

    #[test]
    fn initiate_trade_is_the_target_guid_le() {
        assert_eq!(
            initiate_trade(0x1122_3344_5566_7788),
            vec![0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11]
        );
    }

    #[test]
    fn accept_trade_is_the_partners_state() {
        assert_eq!(accept_trade(3), vec![3, 0, 0, 0]);
    }

    #[test]
    fn set_trade_item_is_three_u8s() {
        assert_eq!(set_trade_item(2, 0, 23), vec![2, 0, 23]);
    }

    #[test]
    fn clear_trade_item_is_one_u8() {
        assert_eq!(clear_trade_item(5), vec![5]);
    }

    #[test]
    fn set_trade_gold_is_u32_le() {
        assert_eq!(set_trade_gold(0x0001_E240), vec![0x40, 0xE2, 0x01, 0x00]);
    }

    // ── SMSG_TRADE_STATUS parse goldens ──

    /// Every code without a tail reads as its variant and leaves the byte after the code unread.
    #[test]
    fn trade_status_bare_code_has_no_tail() {
        for (code, want) in [
            (0, TradeStatus::Busy),
            (3, TradeStatus::Canceled),
            (4, TradeStatus::Accept),
            (5, TradeStatus::Busy2),
            (6, TradeStatus::NoTarget),
            (7, TradeStatus::BackToTrade),
            (8, TradeStatus::Complete),
            (9, TradeStatus::Rejected),
            (10, TradeStatus::TargetTooFar),
            (11, TradeStatus::WrongFaction),
            (13, TradeStatus::Unknown13),
            (14, TradeStatus::IgnoreYou),
            (15, TradeStatus::YouStunned),
            (16, TradeStatus::TargetStunned),
            (17, TradeStatus::YouDead),
            (18, TradeStatus::TargetDead),
            (19, TradeStatus::YouLogout),
            (20, TradeStatus::TargetLogout),
            (21, TradeStatus::TrialAccount),
        ] {
            let mut buf = u32::to_le_bytes(code).to_vec();
            buf.push(0xEE); // the next byte, which a bare status leaves unread
            let mut r = &buf[..];
            assert_eq!(read_trade_status(&mut r).unwrap(), want, "code {code}");
            assert_eq!(
                r,
                [0xEE],
                "no tail should be consumed for bare status {code}"
            );
            assert_eq!(want.code(), code);
        }
    }

    #[test]
    fn trade_status_begin_trade_reads_the_partner_guid() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u32.to_le_bytes()); // BEGIN_TRADE
        buf.extend_from_slice(&0xDEAD_BEEF_0000_0001u64.to_le_bytes());
        let mut r = &buf[..];
        assert_eq!(
            read_trade_status(&mut r).unwrap(),
            TradeStatus::BeginTrade {
                partner: 0xDEAD_BEEF_0000_0001
            }
        );
        assert!(r.is_empty());
    }

    #[test]
    fn trade_status_open_window_reads_the_trade_id() {
        let mut buf = 2u32.to_le_bytes().to_vec();
        buf.extend_from_slice(&0x55u32.to_le_bytes());
        let mut r = &buf[..];
        assert_eq!(
            read_trade_status(&mut r).unwrap(),
            TradeStatus::OpenWindow { trade_id: 0x55 }
        );
        assert!(r.is_empty());
    }

    #[test]
    fn trade_status_close_window_consumes_u32_u8_u32() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&12u32.to_le_bytes()); // CLOSE_WINDOW
        buf.extend_from_slice(&7u32.to_le_bytes()); // result
        buf.push(0); // unk (dropped)
        buf.extend_from_slice(&3u32.to_le_bytes()); // itemLimitCategory
        let mut r = &buf[..];
        assert_eq!(
            read_trade_status(&mut r).unwrap(),
            TradeStatus::CloseWindow {
                result: 7,
                item_limit_category: 3
            }
        );
        assert!(r.is_empty(), "the full u32+u8+u32 tail must be consumed");
    }

    #[test]
    fn trade_status_only_conjured_reads_the_slot_byte() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&22u32.to_le_bytes()); // ONLY_CONJURED
        buf.push(4); // slot
        let mut r = &buf[..];
        assert_eq!(
            read_trade_status(&mut r).unwrap(),
            TradeStatus::OnlyConjured { slot: 4 }
        );
        assert!(r.is_empty());
    }

    #[test]
    fn trade_status_not_eligible_reads_the_slot_byte() {
        let mut buf = 23u32.to_le_bytes().to_vec();
        buf.push(3);
        let mut r = &buf[..];
        assert_eq!(
            read_trade_status(&mut r).unwrap(),
            TradeStatus::NotEligible { slot: 3 }
        );
        assert!(r.is_empty());
    }

    #[test]
    fn trade_status_out_of_range_is_unknown_no_tail() {
        let buf = 99u32.to_le_bytes();
        let mut r = &buf[..];
        assert_eq!(read_trade_status(&mut r).unwrap(), TradeStatus::Unknown(99));
        assert!(r.is_empty());
    }

    // ── SMSG_TRADE_STATUS_EXTENDED parse goldens ──

    /// One 73-byte slot record: the index byte and a 72-byte block, `None` all zeroes.
    fn slot_record(index: u8, entry: Option<u32>) -> Vec<u8> {
        let mut b = vec![index];
        match entry {
            Some(e) => {
                b.extend_from_slice(&e.to_le_bytes()); // entry
                b.extend_from_slice(&11u32.to_le_bytes()); // display_id
                b.extend_from_slice(&5u32.to_le_bytes()); // count
                b.extend_from_slice(&0u32.to_le_bytes()); // wrapped
                b.extend_from_slice(&0u64.to_le_bytes()); // gift_creator
                b.extend_from_slice(&0u32.to_le_bytes()); // perm_enchant
                for gem in [21u32, 22, 23] {
                    b.extend_from_slice(&gem.to_le_bytes()); // socket enchants
                }
                b.extend_from_slice(&0u64.to_le_bytes()); // creator
                b.extend_from_slice(&(-3i32).to_le_bytes()); // charges (signed)
                b.extend_from_slice(&0u32.to_le_bytes()); // suffix_factor
                b.extend_from_slice(&0u32.to_le_bytes()); // random_prop_id
                b.extend_from_slice(&0u32.to_le_bytes()); // lock_id
                b.extend_from_slice(&100u32.to_le_bytes()); // max_durability
                b.extend_from_slice(&80u32.to_le_bytes()); // durability
            }
            None => b.extend_from_slice(&[0u8; 72]),
        }
        b
    }

    /// A 3.3.5a snapshot: the 21-byte header, then one record per listed `(slot, entry)`.
    fn extended_wire(which: u8, gold: u32, spell: u32, records: &[(u8, Option<u32>)]) -> Vec<u8> {
        let mut b = vec![which];
        b.extend_from_slice(&9u32.to_le_bytes()); // trade id
        b.extend_from_slice(&4u32.to_le_bytes()); // state index
        b.extend_from_slice(&5u32.to_le_bytes()); // per-side state
        b.extend_from_slice(&gold.to_le_bytes());
        b.extend_from_slice(&spell.to_le_bytes());
        for &(index, entry) in records {
            b.extend(slot_record(index, entry));
        }
        b
    }

    /// The full seven-slot snapshot a server sends is 21 + 7 × 73 = 532 bytes, all read.
    #[test]
    fn a_seven_slot_snapshot_is_532_bytes_and_read_whole() {
        let records: Vec<_> = (0..TRADE_SLOT_COUNT as u8)
            .map(|i| (i, (i == 0).then_some(0xABCD)))
            .collect();
        let wire = extended_wire(1, 0, 0, &records);
        assert_eq!(wire.len(), 532);
        let mut r = &wire[..];
        let ext = read_trade_status_extended(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(ext.slots[0].map(|s| s.entry), Some(0xABCD));
        assert!(ext.slots[1..].iter().all(Option::is_none));
        assert!(
            read_trade_status_extended(&mut &wire[..531]).is_err(),
            "a cut record is a short read"
        );
    }

    #[test]
    fn extended_parses_header_slots_gems_and_signed_charges() {
        let wire = extended_wire(
            1,
            12_345,
            777,
            &[
                (0, Some(0xABCD)),
                (TRADE_SLOT_NONTRADED as u8, Some(0x1111)),
            ],
        );
        let ext = read_trade_status_extended(&mut &wire[..]).unwrap();
        assert!(ext.their_window);
        assert_eq!((ext.trade_id, ext.state_index, ext.side_state), (9, 4, 5));
        assert_eq!(ext.gold, 12_345);
        assert_eq!(ext.enchant_spell_id, 777);

        let slot0 = ext.slots[0].expect("slot 0 filled");
        assert_eq!(slot0.entry, 0xABCD);
        assert_eq!(slot0.display_id, 11);
        assert_eq!(slot0.count, 5);
        assert_eq!(slot0.socket_enchants, [21, 22, 23]);
        assert_eq!(slot0.charges, -3);
        assert_eq!(slot0.max_durability, 100);
        assert_eq!(slot0.durability, 80);
        assert!(
            ext.slots[TRADE_SLOT_NONTRADED].is_some(),
            "enchant slot parsed"
        );
    }

    /// A slot the snapshot omits, or lists with a zero block, is empty.
    #[test]
    fn omitted_and_zeroed_slots_are_empty() {
        let wire = extended_wire(0, 0, 0, &[(2, Some(0x22)), (4, None)]);
        let ext = read_trade_status_extended(&mut &wire[..]).unwrap();
        assert!(!ext.their_window, "which == 0 is our own column");
        let filled: Vec<bool> = ext.slots.iter().map(Option::is_some).collect();
        assert_eq!(filled, [false, false, true, false, false, false, false]);
    }

    /// A slot byte past the seven slots is dropped rather than written out of bounds.
    #[test]
    fn an_out_of_range_slot_is_dropped() {
        let wire = extended_wire(1, 0, 0, &[(7, Some(0x77)), (200, Some(0x78))]);
        let ext = read_trade_status_extended(&mut &wire[..]).unwrap();
        assert!(ext.slots.iter().all(Option::is_none));
    }
}
