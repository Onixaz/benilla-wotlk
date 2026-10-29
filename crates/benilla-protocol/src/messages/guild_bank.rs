//! The guild bank wire (opcodes `0x3E6`-`0x3EE`, `0x40A`, `0x40B`): the banker hello, the tab
//! contents, the item and money moves, the tab log and the tab text. `MSG_GUILD_BANK_MONEY_WITHDRAWN`
//! lives with the guild messages, which ask it on a money event.

use std::io::{self, Read};

use crate::wire::{capacity_hint, read_cstring, read_u32_le, read_u64_le, read_u8};

/// Slots per tab: a virtual slot is `tab * 98 + slot` (`0x5a42e0`).
pub const GUILD_BANK_TAB_SLOTS: u8 = 98;
/// The money log's tab index, past the six item tabs (`GetNumGuildBankMoneyTransactions` reads
/// row 6, `0x5a45b0`).
pub const GUILD_BANK_MONEY_LOG_TAB: u8 = 6;

/// Body of `CMSG_GUILD_BANKER_ACTIVATE` (`TalkToGuildBanker`, `0x6d23c0`): the banker's guid, then
/// whether tab 0's contents are stale, which asks the server for the tab list too.
pub fn guild_bank_activate(banker: u64, full_update: bool) -> Vec<u8> {
    let mut body = banker.to_le_bytes().to_vec();
    body.push(u8::from(full_update));
    body
}

/// Body of `CMSG_GUILD_BANK_QUERY_TAB` (`0x5a5800`): banker, tab, and the tab's stale flag.
pub fn guild_bank_query_tab(banker: u64, tab: u8, full_update: bool) -> Vec<u8> {
    let mut body = banker.to_le_bytes().to_vec();
    body.extend_from_slice(&[tab, u8::from(full_update)]);
    body
}

/// One `CMSG_GUILD_BANK_SWAP_ITEMS`, by its four 12340 senders. Every arm names the bank slot
/// and the item id the client last saw there, 0 for an empty slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuildBankSwap {
    /// A bag item dropped on a bank slot (`0x5a50c0`): `count` 0 moves the stack.
    Deposit {
        tab: u8,
        slot: u8,
        bank_item: u32,
        bag: u8,
        bag_slot: u8,
        count: u32,
    },
    /// A held bank item dropped on a bag slot (`0x6db880`).
    Withdraw {
        tab: u8,
        slot: u8,
        item: u32,
        bag: u8,
        bag_slot: u8,
        count: u32,
    },
    /// A right-clicked bank item, stored wherever it fits (`AutoStoreGuildBankItem`, `0x5a63f0`).
    AutoStore {
        tab: u8,
        slot: u8,
        item: u32,
        count: u32,
    },
    /// A held bank item dropped on another bank slot (`0x5a5410`).
    BankToBank {
        tab: u8,
        slot: u8,
        bank_item: u32,
        src_tab: u8,
        src_slot: u8,
        item: u32,
        count: u32,
    },
}

/// Body of `CMSG_GUILD_BANK_SWAP_ITEMS`: banker, the bank-only flag, the destination bank slot,
/// then each sender's tail.
pub fn guild_bank_swap_items(banker: u64, swap: GuildBankSwap) -> Vec<u8> {
    let mut body = banker.to_le_bytes().to_vec();
    let head = |b: &mut Vec<u8>, bank_only: u8, tab: u8, slot: u8, item: u32| {
        b.extend_from_slice(&[bank_only, tab, slot]);
        b.extend_from_slice(&item.to_le_bytes());
    };
    match swap {
        GuildBankSwap::Deposit {
            tab,
            slot,
            bank_item,
            bag,
            bag_slot,
            count,
        } => {
            head(&mut body, 0, tab, slot, bank_item);
            // Not an auto-store, the bag position, then 0: to the bank.
            body.extend_from_slice(&[0, bag, bag_slot, 0]);
            body.extend_from_slice(&count.to_le_bytes());
        }
        GuildBankSwap::Withdraw {
            tab,
            slot,
            item,
            bag,
            bag_slot,
            count,
        } => {
            head(&mut body, 0, tab, slot, item);
            // Not an auto-store, the bag position, then 1: to the character.
            body.extend_from_slice(&[0, bag, bag_slot, 1]);
            body.extend_from_slice(&count.to_le_bytes());
        }
        GuildBankSwap::AutoStore {
            tab,
            slot,
            item,
            count,
        } => {
            head(&mut body, 0, tab, slot, item);
            body.push(1);
            body.extend_from_slice(&count.to_le_bytes());
            body.push(1);
            body.extend_from_slice(&0u32.to_le_bytes());
        }
        GuildBankSwap::BankToBank {
            tab,
            slot,
            bank_item,
            src_tab,
            src_slot,
            item,
            count,
        } => {
            head(&mut body, 1, tab, slot, bank_item);
            body.extend_from_slice(&[src_tab, src_slot]);
            body.extend_from_slice(&item.to_le_bytes());
            body.push(0);
            body.extend_from_slice(&count.to_le_bytes());
        }
    }
    body
}

/// Body of `CMSG_GUILD_BANK_BUY_TAB` (`0x5a5010`): banker and the tab bought, the current count.
pub fn guild_bank_buy_tab(banker: u64, tab: u8) -> Vec<u8> {
    let mut body = banker.to_le_bytes().to_vec();
    body.push(tab);
    body
}

/// Body of `CMSG_GUILD_BANK_UPDATE_TAB` (`0x5a5a00`): banker, tab, name, icon file name.
pub fn guild_bank_update_tab(banker: u64, tab: u8, name: &str, icon: &str) -> Vec<u8> {
    let mut body = banker.to_le_bytes().to_vec();
    body.push(tab);
    for s in [name, icon] {
        body.extend_from_slice(s.as_bytes());
        body.push(0);
    }
    body
}

/// Body of `CMSG_GUILD_BANK_DEPOSIT_MONEY` (`0x5a5b00`) and `CMSG_GUILD_BANK_WITHDRAW_MONEY`
/// (`0x5a5ba0`): banker and copper.
pub fn guild_bank_money(banker: u64, copper: u32) -> Vec<u8> {
    let mut body = banker.to_le_bytes().to_vec();
    body.extend_from_slice(&copper.to_le_bytes());
    body
}

/// Body of `MSG_GUILD_BANK_LOG_QUERY` (`0x5a58c0`) and `MSG_QUERY_GUILD_BANK_TEXT` (`0x5a5940`):
/// the tab alone, 6 for the money log.
pub fn guild_bank_tab_query(tab: u8) -> Vec<u8> {
    vec![tab]
}

/// Body of `CMSG_SET_GUILD_BANK_TEXT` (`0x5a5c40`): tab and text, no banker.
pub fn guild_bank_set_text(tab: u8, text: &str) -> Vec<u8> {
    let mut body = vec![tab];
    body.extend_from_slice(text.as_bytes());
    body.push(0);
    body
}

/// Every guild bank send, by opcode: the writer's one entry point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuildBankSend {
    Activate {
        banker: u64,
        full_update: bool,
    },
    QueryTab {
        banker: u64,
        tab: u8,
        full_update: bool,
    },
    Swap {
        banker: u64,
        swap: GuildBankSwap,
    },
    BuyTab {
        banker: u64,
        tab: u8,
    },
    UpdateTab {
        banker: u64,
        tab: u8,
        name: String,
        icon: String,
    },
    DepositMoney {
        banker: u64,
        copper: u32,
    },
    WithdrawMoney {
        banker: u64,
        copper: u32,
    },
    QueryLog {
        tab: u8,
    },
    QueryText {
        tab: u8,
    },
    SetText {
        tab: u8,
        text: String,
    },
}

impl GuildBankSend {
    /// The opcode and body.
    pub fn encode(&self) -> (u16, Vec<u8>) {
        use super::opcode as op;
        match self {
            Self::Activate {
                banker,
                full_update,
            } => (
                op::CMSG_GUILD_BANKER_ACTIVATE,
                guild_bank_activate(*banker, *full_update),
            ),
            Self::QueryTab {
                banker,
                tab,
                full_update,
            } => (
                op::CMSG_GUILD_BANK_QUERY_TAB,
                guild_bank_query_tab(*banker, *tab, *full_update),
            ),
            Self::Swap { banker, swap } => (
                op::CMSG_GUILD_BANK_SWAP_ITEMS,
                guild_bank_swap_items(*banker, *swap),
            ),
            Self::BuyTab { banker, tab } => (
                op::CMSG_GUILD_BANK_BUY_TAB,
                guild_bank_buy_tab(*banker, *tab),
            ),
            Self::UpdateTab {
                banker,
                tab,
                name,
                icon,
            } => (
                op::CMSG_GUILD_BANK_UPDATE_TAB,
                guild_bank_update_tab(*banker, *tab, name, icon),
            ),
            Self::DepositMoney { banker, copper } => (
                op::CMSG_GUILD_BANK_DEPOSIT_MONEY,
                guild_bank_money(*banker, *copper),
            ),
            Self::WithdrawMoney { banker, copper } => (
                op::CMSG_GUILD_BANK_WITHDRAW_MONEY,
                guild_bank_money(*banker, *copper),
            ),
            Self::QueryLog { tab } => (op::MSG_GUILD_BANK_LOG_QUERY, guild_bank_tab_query(*tab)),
            Self::QueryText { tab } => (op::MSG_QUERY_GUILD_BANK_TEXT, guild_bank_tab_query(*tab)),
            Self::SetText { tab, text } => (
                op::CMSG_SET_GUILD_BANK_TEXT,
                guild_bank_set_text(*tab, text),
            ),
        }
    }
}

/// One tab's name and icon file name, as `SMSG_GUILD_BANK_LIST` lists them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GuildBankTab {
    pub name: String,
    pub icon: String,
}

/// One bank slot in `SMSG_GUILD_BANK_LIST`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GuildBankSlot {
    /// The 0-based slot, below [`GUILD_BANK_TAB_SLOTS`].
    pub slot: u8,
    /// 0 empties the slot.
    pub item_id: u32,
    pub flags: u32,
    pub random_property_id: i32,
    /// Read only for a random property (`0x5a754f`).
    pub suffix_factor: u32,
    pub count: u32,
    /// The permanent enchant, which the link carries.
    pub enchant: u32,
    pub charges: u8,
    /// The three socket gems' enchant ids, by socket.
    pub gems: [u32; 3],
}

/// `SMSG_GUILD_BANK_LIST`, one tab's changed slots.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GuildBankList {
    pub money: u64,
    pub tab: u8,
    /// Today's remaining item withdrawals from this tab, -1 unlimited.
    pub withdrawals_remaining: i32,
    /// The tab list, sent with tab 0 when the query asked for a full update.
    pub tabs: Option<Vec<GuildBankTab>>,
    pub slots: Vec<GuildBankSlot>,
}

/// Read `SMSG_GUILD_BANK_LIST` (`0x5a7250`): `u64` money, `u8` tab, `i32` withdrawals, `u8` full
/// update; on tab 0 with that flag a `u8` count of name and icon cstrings; then a `u8` count of
/// slots, each `u8` slot and `u32` item, and for an item `u32` flags, `i32` random property (then
/// `u32` suffix factor), `u32` count, `u32` enchant, `u8` charges, and a `u8` count of `(u8 socket,
/// u32 enchant)` gems (`0x5a748e`-`0x5a7624`).
pub(super) fn read_guild_bank_list(r: &mut impl Read) -> io::Result<GuildBankList> {
    let money = read_u64_le(r)?;
    let tab = read_u8(r)?;
    let withdrawals_remaining = read_u32_le(r)? as i32;
    let full_update = read_u8(r)? != 0;
    let tabs = if full_update && tab == 0 {
        let n = read_u8(r)?;
        let mut tabs = Vec::with_capacity(capacity_hint(n, 6));
        for _ in 0..n {
            tabs.push(GuildBankTab {
                name: read_cstring(r)?,
                icon: read_cstring(r)?,
            });
        }
        Some(tabs)
    } else {
        None
    };
    let n = read_u8(r)?;
    let mut slots = Vec::with_capacity(capacity_hint(n, usize::from(GUILD_BANK_TAB_SLOTS)));
    for _ in 0..n {
        let mut s = GuildBankSlot {
            slot: read_u8(r)?,
            item_id: read_u32_le(r)?,
            ..Default::default()
        };
        if s.item_id != 0 {
            s.flags = read_u32_le(r)?;
            s.random_property_id = read_u32_le(r)? as i32;
            if s.random_property_id != 0 {
                s.suffix_factor = read_u32_le(r)?;
            }
            s.count = read_u32_le(r)?;
            s.enchant = read_u32_le(r)?;
            s.charges = read_u8(r)?;
            for _ in 0..read_u8(r)? {
                let socket = read_u8(r)?;
                let gem = read_u32_le(r)?;
                // The reference indexes its three-gem array unchecked; past it is dropped.
                if let Some(g) = s.gems.get_mut(usize::from(socket)) {
                    *g = gem;
                }
            }
        }
        slots.push(s);
    }
    Ok(GuildBankList {
        money,
        tab,
        withdrawals_remaining,
        tabs,
        slots,
    })
}

/// A guild bank log row's kind (`GetGuildBankTransaction` `0x5a6dc0` and
/// `GetGuildBankMoneyTransaction` `0x5a4e50` name them).
pub mod guild_bank_log {
    pub const DEPOSIT_ITEM: u8 = 1;
    pub const WITHDRAW_ITEM: u8 = 2;
    pub const MOVE_ITEM: u8 = 3;
    pub const DEPOSIT_MONEY: u8 = 4;
    pub const WITHDRAW_MONEY: u8 = 5;
    pub const REPAIR_MONEY: u8 = 6;
    pub const MOVE_ITEM2: u8 = 7;
    pub const WITHDRAW_FOR_TAB: u8 = 8;
    pub const BUY_TAB: u8 = 9;

    /// The kinds that carry money rather than an item (`0x5a4800`).
    pub fn is_money(kind: u8) -> bool {
        matches!(
            kind,
            DEPOSIT_MONEY | WITHDRAW_MONEY | REPAIR_MONEY | WITHDRAW_FOR_TAB | BUY_TAB
        )
    }
}

/// One guild bank log row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GuildBankLogEntry {
    /// A [`guild_bank_log`] kind.
    pub kind: u8,
    pub player: u64,
    pub item_id: u32,
    pub count: u32,
    pub money: u32,
    /// The other tab of a move.
    pub other_tab: u8,
    /// Seconds since the row was written.
    pub age: u32,
}

/// `MSG_GUILD_BANK_LOG_QUERY`'s answer: one tab's log, newest last.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GuildBankLog {
    /// The item tab 0-5, or [`GUILD_BANK_MONEY_LOG_TAB`].
    pub tab: u8,
    pub entries: Vec<GuildBankLogEntry>,
}

/// Read `MSG_GUILD_BANK_LOG_QUERY` (`0x5a4800`): `u8` tab, `u8` count, then per row `u8` kind,
/// `u64` player, `u32` money for a money kind or `u32` item and `u32` count (plus `u8` other tab
/// for a move), then `u32` age.
pub(super) fn read_guild_bank_log(r: &mut impl Read) -> io::Result<GuildBankLog> {
    let tab = read_u8(r)?;
    let n = read_u8(r)?;
    let mut entries = Vec::with_capacity(capacity_hint(n, 25));
    for _ in 0..n {
        let mut e = GuildBankLogEntry {
            kind: read_u8(r)?,
            player: read_u64_le(r)?,
            ..Default::default()
        };
        if guild_bank_log::is_money(e.kind) {
            e.money = read_u32_le(r)?;
        } else {
            e.item_id = read_u32_le(r)?;
            e.count = read_u32_le(r)?;
            if matches!(
                e.kind,
                guild_bank_log::MOVE_ITEM | guild_bank_log::MOVE_ITEM2
            ) {
                e.other_tab = read_u8(r)?;
            }
        }
        e.age = read_u32_le(r)?;
        entries.push(e);
    }
    Ok(GuildBankLog { tab, entries })
}

/// Read `MSG_QUERY_GUILD_BANK_TEXT`'s answer (`0x5a4ae0`): `u8` tab and the text.
pub(super) fn read_guild_bank_text(r: &mut impl Read) -> io::Result<(u8, String)> {
    Ok((read_u8(r)?, read_cstring(r)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_swap_senders_lay_out_as_12340_sends_them() {
        let banker = 0x0102_0304_0506_0708u64;
        let g = banker.to_le_bytes();
        let deposit = guild_bank_swap_items(
            banker,
            GuildBankSwap::Deposit {
                tab: 1,
                slot: 7,
                bank_item: 0,
                bag: 255,
                bag_slot: 23,
                count: 0,
            },
        );
        assert_eq!(&deposit[..8], &g);
        assert_eq!(
            &deposit[8..],
            &[0, 1, 7, 0, 0, 0, 0, 0, 255, 23, 0, 0, 0, 0, 0]
        );

        let withdraw = guild_bank_swap_items(
            banker,
            GuildBankSwap::Withdraw {
                tab: 0,
                slot: 3,
                item: 2589,
                bag: 19,
                bag_slot: 4,
                count: 5,
            },
        );
        assert_eq!(
            &withdraw[8..],
            &[0, 0, 3, 0x1d, 0x0a, 0, 0, 0, 19, 4, 1, 5, 0, 0, 0]
        );

        let auto = guild_bank_swap_items(
            banker,
            GuildBankSwap::AutoStore {
                tab: 2,
                slot: 97,
                item: 2589,
                count: 20,
            },
        );
        assert_eq!(
            &auto[8..],
            &[0, 2, 97, 0x1d, 0x0a, 0, 0, 1, 20, 0, 0, 0, 1, 0, 0, 0, 0]
        );

        let move_ = guild_bank_swap_items(
            banker,
            GuildBankSwap::BankToBank {
                tab: 0,
                slot: 1,
                bank_item: 0,
                src_tab: 3,
                src_slot: 9,
                item: 2589,
                count: 0,
            },
        );
        assert_eq!(
            &move_[8..],
            &[1, 0, 1, 0, 0, 0, 0, 3, 9, 0x1d, 0x0a, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn a_full_tab_0_list_reads_the_tabs_then_the_slots() {
        let mut b = Vec::new();
        b.extend_from_slice(&1_234_567u64.to_le_bytes());
        b.push(0); // tab
        b.extend_from_slice(&(-1i32).to_le_bytes());
        b.push(1); // full update
        b.push(2);
        b.extend_from_slice(b"Mats\0INV_Misc_Bag_10\0\0\0");
        b.push(3); // slots
                   // An empty slot.
        b.push(5);
        b.extend_from_slice(&0u32.to_le_bytes());
        // A plain stack.
        b.push(6);
        b.extend_from_slice(&2589u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes()); // flags
        b.extend_from_slice(&0u32.to_le_bytes()); // random property
        b.extend_from_slice(&20u32.to_le_bytes()); // count
        b.extend_from_slice(&0u32.to_le_bytes()); // enchant
        b.push(0); // charges
        b.push(0); // gems
                   // A suffixed, enchanted, socketed item.
        b.push(97);
        b.extend_from_slice(&15_000u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&(-42i32).to_le_bytes());
        b.extend_from_slice(&77u32.to_le_bytes()); // suffix factor
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&1900u32.to_le_bytes());
        b.push(3);
        b.push(1);
        b.push(2); // socket 2
        b.extend_from_slice(&3300u32.to_le_bytes());
        let list = read_guild_bank_list(&mut &b[..]).unwrap();
        assert_eq!(list.money, 1_234_567);
        assert_eq!(list.withdrawals_remaining, -1);
        let tabs = list.tabs.unwrap();
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0].name, "Mats");
        assert_eq!(tabs[0].icon, "INV_Misc_Bag_10");
        assert_eq!(tabs[1], GuildBankTab::default());
        assert_eq!(list.slots.len(), 3);
        assert_eq!(list.slots[0].item_id, 0);
        assert_eq!((list.slots[1].item_id, list.slots[1].count), (2589, 20));
        let s = &list.slots[2];
        assert_eq!((s.slot, s.flags, s.random_property_id), (97, 1, -42));
        assert_eq!((s.suffix_factor, s.enchant, s.charges), (77, 1900, 3));
        assert_eq!(s.gems, [0, 0, 3300]);
    }

    #[test]
    fn a_tab_1_list_carries_no_tab_list_even_flagged_full() {
        let mut b = Vec::new();
        b.extend_from_slice(&5u64.to_le_bytes());
        b.push(1);
        b.extend_from_slice(&3i32.to_le_bytes());
        b.push(1);
        b.push(0);
        let list = read_guild_bank_list(&mut &b[..]).unwrap();
        assert_eq!((list.tab, list.withdrawals_remaining), (1, 3));
        assert!(list.tabs.is_none());
        assert!(list.slots.is_empty());
    }

    #[test]
    fn the_log_reads_money_rows_and_moves() {
        let mut b = vec![0u8, 3];
        b.push(guild_bank_log::DEPOSIT_ITEM);
        b.extend_from_slice(&0x11u64.to_le_bytes());
        b.extend_from_slice(&2589u32.to_le_bytes());
        b.extend_from_slice(&20u32.to_le_bytes());
        b.extend_from_slice(&60u32.to_le_bytes());
        b.push(guild_bank_log::MOVE_ITEM);
        b.extend_from_slice(&0x12u64.to_le_bytes());
        b.extend_from_slice(&2589u32.to_le_bytes());
        b.extend_from_slice(&5u32.to_le_bytes());
        b.push(2);
        b.extend_from_slice(&120u32.to_le_bytes());
        b.push(guild_bank_log::REPAIR_MONEY);
        b.extend_from_slice(&0x13u64.to_le_bytes());
        b.extend_from_slice(&900u32.to_le_bytes());
        b.extend_from_slice(&5u32.to_le_bytes());
        let log = read_guild_bank_log(&mut &b[..]).unwrap();
        assert_eq!(log.entries.len(), 3);
        assert_eq!(
            (
                log.entries[0].item_id,
                log.entries[0].count,
                log.entries[0].age
            ),
            (2589, 20, 60)
        );
        assert_eq!(log.entries[1].other_tab, 2);
        assert_eq!((log.entries[2].money, log.entries[2].age), (900, 5));
    }

    #[test]
    fn the_small_sends() {
        let g = 7u64.to_le_bytes();
        assert_eq!(guild_bank_activate(7, true), [&g[..], &[1]].concat());
        assert_eq!(
            guild_bank_query_tab(7, 3, false),
            [&g[..], &[3, 0]].concat()
        );
        assert_eq!(guild_bank_buy_tab(7, 2), [&g[..], &[2]].concat());
        assert_eq!(
            guild_bank_update_tab(7, 1, "A", "B"),
            [&g[..], &[1, b'A', 0, b'B', 0]].concat()
        );
        assert_eq!(
            guild_bank_money(7, 300),
            [&g[..], &300u32.to_le_bytes()].concat()
        );
        assert_eq!(guild_bank_tab_query(6), vec![6]);
        assert_eq!(guild_bank_set_text(0, "hi"), vec![0, b'h', b'i', 0]);
        assert_eq!(
            read_guild_bank_text(&mut &[2u8, b'x', 0][..]).unwrap(),
            (2, "x".to_string())
        );
    }
}
