//! The guild bank bindings, 12340's guild bank script table (registrar `0xacf8d0`). The verbs read
//! the bank the app keeps at the call ([`super::LiveState`]): the banker, the tabs, each slot and
//! its lock, the money and the logs. The engine keeps the cursor modes 11 (a bank item) and 12
//! (bank money); each send the app gates is a [`GuildBankRequest`] it drains
//! ([`UiScript::take_guild_bank_requests`]).

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{flag, number_arg};
use super::cursor::{self, CursorGuildBankItem, CursorMoney, CursorPayload};
use super::{read_live, write_live, Model};

/// The bank's tabs (six tab records, `0xc11458`).
pub const GUILD_BANK_TABS: usize = 6;
/// Slots per tab; a virtual slot is `tab * 98 + slot` (`0x5a42e0`).
pub const GUILD_BANK_TAB_SLOTS: usize = 98;
/// The money log, after the six item tabs (`0x5a45b0` reads row 6).
pub const GUILD_BANK_MONEY_LOG: usize = 6;
/// The log rows a tab keeps (`m_log[7][25]`).
const GUILD_BANK_LOG_ROWS: usize = 25;

/// `GR_RIGHT_WITHDRAW_GOLD`, the right `CanWithdrawGuildBankMoney` tests (`0x5a5f60`).
const WITHDRAW_GOLD: u32 = 0x0008_0000;
/// A tab's rights (`GetGuildBankTabInfo` `0x5a67f0`, `CanEditGuildTabInfo` `0x5cc290`).
mod tab_right {
    pub(super) const VIEW: u32 = 0x1;
    pub(super) const DEPOSIT: u32 = 0x2;
    pub(super) const UPDATE_TEXT: u32 = 0x4;
}

/// The price of each tab, by tab bought (`GetGuildBankTabCost` `0x5a6b20` reads its six-entry
/// table; AzerothCore's `GuildMgr` charges the same 100 to 5000 gold).
const TAB_COSTS: [u32; GUILD_BANK_TABS] = [
    1_000_000, 2_500_000, 5_000_000, 10_000_000, 25_000_000, 50_000_000,
];

/// One bank slot, resolved for display.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GuildBankSlotView {
    pub item_id: u32,
    /// `Interface\Icons\…`; `None` while the template is in flight.
    pub texture: Option<String>,
    pub count: u32,
    pub link: Option<String>,
    /// The template's stack size, which `SplitGuildBankItem` bounds an amount by.
    pub max_stack: u32,
    /// The name with its random suffix joined, for the tooltip.
    pub name: Option<String>,
    /// The permanent enchant, the gems and the suffix roll, named.
    pub enchants: Vec<super::EnchantView>,
    /// The wire flags; `0x4` is the tooltip's unlocked bit (`0x6304a0`).
    pub flags: u32,
}

/// One tab's name, icon and text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildBankTabView {
    /// Empty shows `GUILDBANK_TAB_NUMBER`.
    pub name: String,
    /// The icon's file name under `Interface\Icons\`; empty shows the question mark.
    pub icon: String,
    /// The tab's info text, as `MSG_QUERY_GUILD_BANK_TEXT` last answered.
    pub text: String,
}

/// One log row, resolved for display.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildBankLogView {
    /// The wire kind: 1 deposit, 2 withdraw, 3 and 7 move, 4-9 money.
    pub kind: u8,
    /// `None` until the name cache answers.
    pub player: Option<String>,
    /// The item's quality link, `""` until the template answers.
    pub link: String,
    pub count: u32,
    pub money: u32,
    /// The other tab of a move, 0-based.
    pub other_tab: u8,
    /// Unix seconds when the row was written: arrival time less its age (`0x5a4800`).
    pub written_at: u32,
}

/// A whole guild bank, as a test's live view holds it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GuildBankView {
    /// A guild banker is open (`0xc1dc10`).
    pub open: bool,
    pub num_tabs: u8,
    /// [`GUILD_BANK_TABS`] rows.
    pub tabs: Vec<GuildBankTabView>,
    /// Per tab, [`GUILD_BANK_TAB_SLOTS`] slots once its first list arrived, else empty.
    pub slots: Vec<Vec<Option<GuildBankSlotView>>>,
    /// The last list's withdrawals left, `GetGuildBankTabInfo`'s sixth return.
    pub withdrawals_remaining: i32,
    /// The six tab logs and the money log.
    pub logs: Vec<Vec<GuildBankLogView>>,
}

impl GuildBankView {
    /// Slot `(tab, slot)`, `None` when empty or unlisted.
    pub fn slot(&self, tab: usize, slot: usize) -> Option<&GuildBankSlotView> {
        self.slots.get(tab)?.get(slot)?.as_ref()
    }
}

/// The guild bank's sends, drained by the app, which gates them on the open banker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuildBankRequest {
    /// `QueryGuildBankTab(tab)`, 0-based.
    QueryTab(u8),
    /// A held bag item dropped on bank slot `(tab, slot)`: the bag-to-bank swap (`0x5a50c0`), whose
    /// item gates and stack arithmetic the app runs.
    DepositItem {
        tab: u8,
        slot: u8,
        bag: i64,
        bag_slot: u32,
        split: Option<u32>,
    },
    /// A held bank item dropped on another bank slot: the bank-to-bank swap (`0x5a5410`).
    MoveItem {
        tab: u8,
        slot: u8,
        src_tab: u8,
        src_slot: u8,
        item_id: u32,
        split: Option<u32>,
    },
    /// A held bank item dropped on a bag slot: `PickupContainerItem`'s mode-11 arm (`0x5d7ff0`).
    WithdrawItem {
        src_tab: u8,
        src_slot: u8,
        item_id: u32,
        bag: i64,
        bag_slot: u32,
        split: Option<u32>,
    },
    /// `AutoStoreGuildBankItem(tab, slot)` (`0x5a63f0`).
    AutoStore {
        tab: u8,
        slot: u8,
        item_id: u32,
        count: u32,
    },
    /// `BuyGuildBankTab()`, the tab after the last.
    BuyTab(u8),
    /// `SetGuildBankTabInfo(tab, name, iconIndex)`: a checked name and the icon's file name.
    SetTabInfo {
        tab: u8,
        name: String,
        icon: String,
    },
    DepositMoney(u32),
    WithdrawMoney(u32),
    /// `QueryGuildBankLog(tab)`, 0-based, 6 the money log.
    QueryLog(u8),
    /// `SetGuildBankText(tab, text)`.
    SetText {
        tab: u8,
        text: String,
    },
}

impl super::UiScript {
    /// The bank item the cursor holds as `(tab, slot, item)`: `GetCursorVirtualItem` (`0x5136e0`).
    pub fn guild_bank_cursor_item(&self) -> Option<(u8, u8, u32)> {
        match &self.model_ref().cursor {
            Some(CursorPayload::GuildBankItem(h)) => Some((h.tab, h.slot, h.item_id)),
            _ => None,
        }
    }

    /// The app sent a swap: the reference clears the cursor, unlocking a held bank item, and
    /// fires `GUILDBANKBAGSLOTS_CHANGED` (`0x5a50c0`, `0x5a5410`).
    pub fn guild_bank_swap_sent(&mut self) {
        let model = &mut *self.model_mut();
        cursor::clear_cursor(model);
        queue(model, "GUILDBANKBAGSLOTS_CHANGED");
    }

    /// A held bank item dropped back where nothing is sent: the cursor clears and the slot
    /// unlocks (`0x5a5410`, `0x513800`).
    pub fn guild_bank_drop_held(&mut self) {
        cursor::clear_cursor(&mut self.model_mut());
    }

    /// The app refused a swap the reference refuses before its send: the cursor stays.
    pub fn guild_bank_swap_refused(&mut self, error: &'static str) {
        self.model_mut().ui_errors.push(error);
    }

    /// Drain the queued sends.
    pub fn take_guild_bank_requests(&mut self) -> Vec<GuildBankRequest> {
        drain_field!(self, guild_bank_requests)
    }

    /// Give the VM the tab-icon chooser's builder, run on the first `GetNumMacroItemIcons`.
    pub fn set_macro_item_icon_source(&mut self, source: crate::script::MacroIconSource) {
        self.model_mut().macro_item_icon_source = Some(source);
    }
}

fn queue(model: &mut Model, event: &str) {
    model.pending_events.push((event.to_string(), Vec::new()));
}

/// Fire the events this call queued since `mark`, in order and inside the call, as the cursor
/// setter and the lock fire theirs (`0x520da7`, `0x5137f1`).
fn fire_queued_since(lua: &Lua, mark: usize) {
    let events = {
        let mut model = crate::script::model_mut_of(lua);
        if model.pending_events.len() > mark {
            model.pending_events.split_off(mark)
        } else {
            Vec::new()
        }
    };
    for (event, args) in events {
        super::tick::fire_event_into(lua, &event, args);
    }
}

/// Whether a guild banker is open (`0xc1dc10`), through the lent view.
fn bank_open() -> bool {
    read_live(|l| l.guild_bank_open()).unwrap_or(false)
}

/// The bought tabs (`0xc1dc0c`).
fn num_tabs() -> u8 {
    read_live(|l| l.guild_bank_num_tabs()).unwrap_or(0)
}

/// The current tab (`0xc1dc0d`).
pub(super) fn current_tab() -> u8 {
    read_live(|l| l.guild_bank_current_tab()).unwrap_or(0)
}

/// The bank's money (`0xc1dc20`).
pub(super) fn bank_money() -> u64 {
    read_live(|l| l.guild_bank_money()).unwrap_or(0)
}

/// The lock test (`0x5a4c40`): the slot is locked for this item.
fn is_locked(tab: u8, slot: u8, item_id: u32) -> bool {
    read_live(|l| l.guild_bank_locked(tab, slot, item_id)).unwrap_or(false)
}

/// `ClearCursor`'s mode-11 arm (`0x519406`): the held slot unlocks, with its event.
pub(super) fn unlock_held(model: &mut Model, held: &CursorGuildBankItem) {
    write_live(|l| l.set_guild_bank_lock(held.tab, held.slot, held.item_id, false));
    queue(model, "GUILDBANK_ITEM_LOCK_CHANGED");
}

/// Put a bank item on the cursor (`0x520d30`, kind 11, with the split), then lock its slot
/// (`0x5a6372` -> `0x5137d0`): every event fires inside the call, the lock's last.
fn hold(lua: &Lua, tab: u8, slot: u8, split: Option<u32>) {
    let Some(s) = read_live(|l| l.guild_bank_slot(tab, slot)).flatten() else {
        return;
    };
    if !crate::script::live::has_control() {
        return;
    }
    let mark = {
        let mut model = crate::script::model_mut_of(lua);
        let mark = model.pending_events.len();
        cursor::clear_cursor(&mut model);
        model.cursor = Some(CursorPayload::GuildBankItem(CursorGuildBankItem {
            tab,
            slot,
            item_id: s.item_id,
            texture: s.texture.clone(),
            link: s.link.clone(),
            split,
        }));
        cursor::queue_cursor_update(&mut model);
        mark
    };
    fire_queued_since(lua, mark);
    write_live(|l| l.set_guild_bank_lock(tab, slot, s.item_id, true));
    super::tick::fire_event_into(lua, "GUILDBANK_ITEM_LOCK_CHANGED", Vec::new());
}

/// `PickupContainerItem`'s mode-11 arm: a held bank item dropped on a bag slot. The reference
/// clears the cursor whatever the app then sends (`0x5d7ff0`).
pub(super) fn drop_on_bag_slot(model: &mut Model, held: CursorGuildBankItem, bag: i64, slot: u32) {
    if bank_open() {
        model
            .guild_bank_requests
            .push(GuildBankRequest::WithdrawItem {
                src_tab: held.tab,
                src_slot: held.slot,
                item_id: held.item_id,
                bag,
                bag_slot: slot,
                split: held.split,
            });
    }
    model.cursor = Some(CursorPayload::GuildBankItem(held));
    cursor::clear_cursor(model);
}

/// `UseContainerItem`'s guild bank arm (`0x5d8650` calls the bank's auto-store `0x5a56b0`): with a
/// banker open, the item goes to the current tab, slot `0xFF` for wherever it fits.
pub(super) fn use_container_item(model: &mut Model, bag: i64, slot: u32) -> bool {
    if !bank_open() {
        return false;
    }
    let occupied = super::container::index_slot(model, bag, slot).is_some_and(|s| s.item_id != 0);
    if occupied {
        model
            .guild_bank_requests
            .push(GuildBankRequest::DepositItem {
                tab: current_tab(),
                slot: 0xFF,
                bag,
                bag_slot: slot,
                split: None,
            });
    }
    true
}

/// A 1-based Lua tab or slot as the reference's byte: `trunc(arg) - 1`, wrapping.
fn index_arg(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<u8> {
    Ok(number_arg(lua, v, usage)?.wrapping_sub(1) as u8)
}

/// `Interface\Icons\<icon>` (the client's string lookup, table 3), the question mark for an empty
/// icon.
fn icon_path(icon: &str) -> String {
    let icon = if icon.is_empty() {
        "INV_Misc_QuestionMark"
    } else {
        icon
    };
    format!("Interface\\Icons\\{icon}")
}

/// A log row's age as `(years, months, days, hours)`, the reference's calendar of 8760-hour
/// years and 720-hour months (`0x5a4e50`).
pub(super) fn age_parts(written_at: u32, now: u32) -> [i64; 4] {
    let age = i64::from(now.wrapping_sub(written_at) as i32);
    let years = age / 3600 / 8760;
    let hours = age / 3600 % 8760;
    let months = hours / 720;
    let hours = hours % 720;
    [years, months, hours / 24, hours % 24]
}

pub(super) fn unix_now() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as u32)
}

fn str_or_nil(lua: &Lua, s: Option<&str>) -> mlua::Result<Value> {
    s.map_or(Ok(Value::Nil), |s| lua.create_string(s).map(Value::String))
}

/// Our rank's bank rights for `tab`: `(rights, withdrawals per day)`, 0s outside a guild.
fn tab_rights(model: &Model, tab: u8) -> (u32, u32) {
    let g = &model.guild;
    if !g.in_guild {
        return (0, 0);
    }
    g.ranks
        .get(g.rank_index as usize)
        .and_then(|r| r.bank_tabs.get(usize::from(tab)))
        .copied()
        .unwrap_or((0, 0))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    macro_rules! read {
        ($lua:expr) => {
            $lua.app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data")
        };
    }

    // QueryGuildBankTab(tab) (`0x5a5ce0`): tabs 1-6 ask for their slots.
    g.set(
        "QueryGuildBankTab",
        lua.create_function(|lua, tab: Value| {
            let tab = index_arg(lua, tab, "Usage: QueryGuildBankTab(tab)")?;
            if usize::from(tab) < GUILD_BANK_TABS {
                let mut model = crate::script::model_mut_of(lua);
                model
                    .guild_bank_requests
                    .push(GuildBankRequest::QueryTab(tab));
            }
            Ok(())
        })?,
    )?;

    // SetCurrentGuildBankTab(tab) (`0x5a43c0`) and GetCurrentGuildBankTab() (`0x5a4410`).
    g.set(
        "SetCurrentGuildBankTab",
        lua.create_function(|lua, tab: Value| {
            let tab = index_arg(lua, tab, "Usage: SetCurrentTab(tab)")?;
            if usize::from(tab) < GUILD_BANK_TABS {
                write_live(|l| l.set_guild_bank_current_tab(tab));
            }
            Ok(())
        })?,
    )?;
    g.set(
        "GetCurrentGuildBankTab",
        lua.create_function(|_, ()| Ok(i64::from(current_tab()) + 1))?,
    )?;

    g.set(
        "GetNumGuildBankTabs",
        lua.create_function(|_, ()| Ok(i64::from(num_tabs())))?,
    )?;

    // GetGuildBankItemInfo(tab, slot) (`0x5a6000`) → texture, count, locked; an empty or unlisted
    // slot is `nil, 0, nil`.
    g.set(
        "GetGuildBankItemInfo",
        lua.create_function(|lua, (tab, slot): (Value, Value)| {
            const USAGE: &str = "Usage: GetGuildBankItemInfo(tab, slot)";
            let tab = index_arg(lua, tab, USAGE)?;
            let slot = index_arg(lua, slot, USAGE)?;
            let read = read_live(|l| {
                let (item_id, count) = l.guild_bank_item(tab, slot)?;
                let texture = l.guild_bank_texture(tab, slot);
                Some((texture, count, l.guild_bank_locked(tab, slot, item_id)))
            })
            .flatten();
            let Some((texture, count, locked)) = read else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Integer(0),
                    Value::Nil,
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                str_or_nil(lua, texture.as_deref())?,
                Value::Integer(i64::from(count)),
                flag(locked),
            ]))
        })?,
    )?;

    // GetGuildBankItemLink(tab, slot) (`0x5a4d30`): nothing for an empty slot.
    g.set(
        "GetGuildBankItemLink",
        lua.create_function(|lua, (tab, slot): (Value, Value)| {
            const USAGE: &str = "Usage: GetGuildBankItemLink(tab, slot)";
            let tab = index_arg(lua, tab, USAGE)?;
            let slot = index_arg(lua, slot, USAGE)?;
            match read_live(|l| l.guild_bank_slot(tab, slot))
                .flatten()
                .and_then(|s| s.link)
            {
                Some(link) => Ok(MultiValue::from_vec(vec![Value::String(
                    lua.create_string(link)?,
                )])),
                None => Ok(MultiValue::new()),
            }
        })?,
    )?;

    // PickupGuildBankItem(tab, slot) (`0x5a6200`): a held bag item deposits, a held bank item
    // moves, else the slot's item is taken up and locked.
    g.set(
        "PickupGuildBankItem",
        lua.create_function(|lua, (tab, slot): (Value, Value)| {
            const USAGE: &str = "Usage: PickupGuildBankItem(tab, slot)";
            let tab = index_arg(lua, tab, USAGE)?;
            let slot = index_arg(lua, slot, USAGE)?;
            if usize::from(tab) >= GUILD_BANK_TABS || usize::from(slot) >= GUILD_BANK_TAB_SLOTS {
                return Ok(());
            }
            let take = {
                let mut model = crate::script::model_mut_of(lua);
                match model.cursor.clone() {
                    Some(CursorPayload::Item(item)) => {
                        if bank_open() {
                            model
                                .guild_bank_requests
                                .push(GuildBankRequest::DepositItem {
                                    tab,
                                    slot,
                                    bag: item.bag,
                                    bag_slot: item.slot,
                                    split: item.count,
                                });
                        }
                        false
                    }
                    Some(CursorPayload::GuildBankItem(held)) => {
                        if bank_open() {
                            model.guild_bank_requests.push(GuildBankRequest::MoveItem {
                                tab,
                                slot,
                                src_tab: held.tab,
                                src_slot: held.slot,
                                item_id: held.item_id,
                                split: held.split,
                            });
                        }
                        false
                    }
                    _ => true,
                }
            };
            if take {
                let item = read_live(|l| l.guild_bank_item(tab, slot)).flatten();
                if let Some((item_id, _)) = item {
                    if !is_locked(tab, slot, item_id) {
                        hold(lua, tab, slot, None);
                    }
                }
            }
            Ok(())
        })?,
    )?;

    // AutoStoreGuildBankItem(tab, slot) (`0x5a63f0`): clears the cursor, then stores the slot's
    // whole stack in the bags; the app checks the rank's withdrawals.
    g.set(
        "AutoStoreGuildBankItem",
        lua.create_function(|lua, (tab, slot): (Value, Value)| {
            const USAGE: &str = "Usage: AutoStoreGuildBankItem(tab, slot)";
            let tab = index_arg(lua, tab, USAGE)?;
            let slot = index_arg(lua, slot, USAGE)?;
            if !bank_open()
                || usize::from(tab) >= GUILD_BANK_TABS
                || usize::from(slot) >= GUILD_BANK_TAB_SLOTS
            {
                return Ok(());
            }
            let mut model = crate::script::model_mut_of(lua);
            cursor::clear_cursor(&mut model);
            let Some((item_id, count)) = read_live(|l| l.guild_bank_item(tab, slot)).flatten()
            else {
                return Ok(());
            };
            model.guild_bank_requests.push(GuildBankRequest::AutoStore {
                tab,
                slot,
                item_id,
                count,
            });
            Ok(())
        })?,
    )?;

    // SplitGuildBankItem(tab, slot, amount) (`0x5a6600`): an unlocked slot puts `amount` on the
    // cursor, 1 up to the stack size.
    g.set(
        "SplitGuildBankItem",
        lua.create_function(|lua, (tab, slot, amount): (Value, Value, Value)| {
            const USAGE: &str = "Usage: SplitGuildBankItem(tab, slot, amount)";
            let tab = index_arg(lua, tab, USAGE)?;
            let slot = index_arg(lua, slot, USAGE)?;
            let amount = number_arg(lua, amount, USAGE)?;
            if !crate::script::live::has_control()
                || usize::from(tab) >= GUILD_BANK_TABS
                || usize::from(slot) >= GUILD_BANK_TAB_SLOTS
            {
                return Ok(());
            }
            let Some(s) = read_live(|l| l.guild_bank_slot(tab, slot)).flatten() else {
                return Ok(());
            };
            if is_locked(tab, slot, s.item_id) || amount < 1 || amount as u32 > s.max_stack {
                return Ok(());
            }
            hold(lua, tab, slot, Some(amount as u32));
            Ok(())
        })?,
    )?;

    // GetGuildBankTabInfo(tab) (`0x5a67f0`) → name, icon, isViewable, canDeposit,
    // numWithdrawals, remainingWithdrawals; past the bought tabs `nil, nil, nil, nil, 0, 0`.
    g.set(
        "GetGuildBankTabInfo",
        lua.create_function(|lua, tab: Value| {
            let tab = index_arg(lua, tab, "Usage: GetGuildBankTabInfo(tab)")?;
            let row = (usize::from(tab) < GUILD_BANK_TABS && tab < num_tabs())
                .then(|| read_live(|l| l.guild_bank_tab(tab)).flatten())
                .flatten();
            let Some(row) = row else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Integer(0),
                    Value::Integer(0),
                ]));
            };
            let name = if row.name.is_empty() {
                let template =
                    crate::strings::global(lua, "GUILDBANK_TAB_NUMBER").unwrap_or_default();
                template.replacen("%d", &(u32::from(tab) + 1).to_string(), 1)
            } else {
                row.name
            };
            let (rights, withdrawals) = tab_rights(&read!(lua), tab);
            let remaining = read_live(|l| l.guild_bank_withdrawals()).unwrap_or(0);
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&name)?),
                Value::String(lua.create_string(icon_path(&row.icon))?),
                flag(rights & tab_right::VIEW != 0),
                flag(rights & tab_right::DEPOSIT != 0),
                Value::Integer(i64::from(withdrawals as i32)),
                Value::Integer(i64::from(remaining)),
            ]))
        })?,
    )?;

    // CanEditGuildTabInfo(tab) (`0x5cc290`): our rank's tab right `0x4`.
    g.set(
        "CanEditGuildTabInfo",
        lua.create_function(|lua, tab: Value| {
            let tab = index_arg(lua, tab, "Usage: CanEditGuildTabInfo(tab)")?;
            let model = read!(lua);
            let rights = if usize::from(tab) < GUILD_BANK_TABS {
                tab_rights(&model, tab).0
            } else {
                0
            };
            Ok(flag(rights & tab_right::UPDATE_TEXT != 0))
        })?,
    )?;

    // GetGuildBankTabCost() (`0x5a6b20`): the next tab's price, nil once all six are bought.
    g.set(
        "GetGuildBankTabCost",
        lua.create_function(|_, ()| {
            Ok(TAB_COSTS
                .get(usize::from(num_tabs()))
                .map_or(Value::Nil, |&c| Value::Integer(i64::from(c))))
        })?,
    )?;

    // BuyGuildBankTab() (`0x5a5d30`): purse and bank together must cover the price.
    g.set(
        "BuyGuildBankTab",
        lua.create_function(|lua, ()| {
            if !crate::script::live::has_control() || !bank_open() {
                return Ok(());
            }
            let tab = num_tabs();
            let Some(&cost) = TAB_COSTS.get(usize::from(tab)) else {
                return Ok(());
            };
            let mut model = crate::script::model_mut_of(lua);
            if u64::from(cost) > model.money + bank_money() {
                model.ui_errors.push("ERR_BANKSLOT_INSUFFICIENT_FUNDS");
                return Ok(());
            }
            model
                .guild_bank_requests
                .push(GuildBankRequest::BuyTab(tab));
            Ok(())
        })?,
    )?;

    // DepositGuildBankMoney(money) (`0x5a5e20`): refused locally past the purse.
    g.set(
        "DepositGuildBankMoney",
        lua.create_function(|lua, money: Value| {
            let money = number_arg(lua, money, "Usage: DepositGuildBankMoney(money)")? as u32;
            let mut model = crate::script::model_mut_of(lua);
            if model.money >= u64::from(money) {
                model
                    .guild_bank_requests
                    .push(GuildBankRequest::DepositMoney(money));
            } else {
                model.ui_errors.push("ERR_NOT_ENOUGH_MONEY");
            }
            Ok(())
        })?,
    )?;

    // WithdrawGuildBankMoney(money) (`0x5a5ef0`): sent as asked; the server referees the limit.
    g.set(
        "WithdrawGuildBankMoney",
        lua.create_function(|lua, money: Value| {
            let money = lua
                .coerce_number(money)?
                .ok_or_else(|| mlua::Error::runtime("Usage: WithdrawGuildBankMoney(money)"))?;
            let mut model = crate::script::model_mut_of(lua);
            model
                .guild_bank_requests
                .push(GuildBankRequest::WithdrawMoney(money as i64 as u32));
            Ok(())
        })?,
    )?;

    // CanWithdrawGuildBankMoney() (`0x5a5f60`): a rank with a daily gold limit and the right.
    g.set(
        "CanWithdrawGuildBankMoney",
        lua.create_function(|lua, ()| {
            let model = read!(lua);
            let g = &model.guild;
            let can = g.in_guild
                && g.ranks
                    .get(g.rank_index as usize)
                    .is_some_and(|r| r.bank_money_per_day != 0 && r.rights & WITHDRAW_GOLD != 0);
            Ok(flag(can))
        })?,
    )?;

    // PickupGuildBankMoney(amount) (`0x5a4330`): cursor mode 12, up to the bank's money.
    g.set(
        "PickupGuildBankMoney",
        lua.create_function(|lua, amount: Value| {
            let amount = number_arg(lua, amount, "Usage: PickupGuildBankMoney(amount)")? as u32;
            if amount == 0
                || u64::from(amount) > bank_money()
                || !crate::script::live::has_control()
            {
                return Ok(());
            }
            let mut model = crate::script::model_mut_of(lua);
            cursor::clear_cursor(&mut model);
            model.cursor = Some(CursorPayload::GuildBankMoney(CursorMoney {
                copper: amount,
            }));
            queue(&mut model, "GUILDBANK_UPDATE_MONEY");
            Ok(())
        })?,
    )?;

    // CloseGuildBankFrame() (`0x5a4500`), the banker setter with no banker (`0x5a4270`):
    // `GUILDBANKFRAME_CLOSED` fires first, open or not, then both bankers clear.
    g.set(
        "CloseGuildBankFrame",
        lua.create_function(|lua, ()| {
            super::tick::fire_event_into(lua, "GUILDBANKFRAME_CLOSED", Vec::new());
            write_live(|l| l.clear_guild_banker());
            Ok(())
        })?,
    )?;

    // GetGuildTabardFileNames() (`0x5a6ba0`): the six layer textures, nils with no tabard.
    g.set(
        "GetGuildTabardFileNames",
        lua.create_function(|lua, ()| {
            let model = read!(lua);
            let Some([emblem, emblem_color, border, border_color, background]) = model
                .guild
                .tabard
                .filter(|t| t[0] != -1 && t[2] != -1 && t[4] != -1)
            else {
                return Ok(MultiValue::from_vec(vec![Value::Nil; 6]));
            };
            let names = [
                format!("Textures\\GuildEmblems\\Background_{background:02}_TU_U"),
                format!("Textures\\GuildEmblems\\Background_{background:02}_TL_U"),
                format!("Textures\\GuildEmblems\\Emblem_{emblem:02}_{emblem_color:02}_TU_U"),
                format!("Textures\\GuildEmblems\\Emblem_{emblem:02}_{emblem_color:02}_TL_U"),
                format!("Textures\\GuildEmblems\\Border_{border:02}_{border_color:02}_TU_U"),
                format!("Textures\\GuildEmblems\\Border_{border:02}_{border_color:02}_TL_U"),
            ];
            names
                .iter()
                .map(|n| lua.create_string(n).map(Value::String))
                .collect::<mlua::Result<Vec<_>>>()
                .map(MultiValue::from_vec)
        })?,
    )?;

    // QueryGuildBankLog(tab) (`0x5a6d50`), 7 the money log.
    g.set(
        "QueryGuildBankLog",
        lua.create_function(|lua, tab: Value| {
            let tab = index_arg(lua, tab, "Usage: QueryGuildBankLog(tab)")?;
            let mut model = crate::script::model_mut_of(lua);
            model
                .guild_bank_requests
                .push(GuildBankRequest::QueryLog(tab));
            Ok(())
        })?,
    )?;

    // GetNumGuildBankTransactions(tab) (`0x5a4520`).
    g.set(
        "GetNumGuildBankTransactions",
        lua.create_function(|lua, tab: Value| {
            let tab = index_arg(lua, tab, "Usage: GetNumGuildBankTransactions(tab)")?;
            Ok(read_live(|l| l.guild_bank_log_len(tab)).unwrap_or(0) as i64)
        })?,
    )?;

    // GetGuildBankTransaction(tab, index) (`0x5a6dc0`) → type, name, itemLink, count, tab1, tab2,
    // year, month, day, hour. Deviation: a row past the log answers nothing, where the reference
    // reads the null row.
    g.set(
        "GetGuildBankTransaction",
        lua.create_function(|lua, (tab, index): (Value, Value)| {
            const USAGE: &str = "Usage: GetGuildBankTransaction(tab, index)";
            let tab = index_arg(lua, tab, USAGE)?;
            let index = index_arg(lua, index, USAGE)?;
            let Some(row) = (usize::from(index) < GUILD_BANK_LOG_ROWS)
                .then(|| read_live(|l| l.guild_bank_log(tab, index)).flatten())
                .flatten()
            else {
                return Ok(MultiValue::new());
            };
            let kind = match row.kind {
                1 => "deposit",
                2 => "withdraw",
                3 | 7 => "move",
                _ => "unknown",
            };
            let this = i64::from(tab) + 1;
            let other = i64::from(row.other_tab) + 1;
            let (tab1, tab2) = match row.kind {
                3 => (Value::Integer(this), Value::Integer(other)),
                7 => (Value::Integer(other), Value::Integer(this)),
                _ => (Value::Nil, Value::Nil),
            };
            let mut out = vec![
                Value::String(lua.create_string(kind)?),
                str_or_nil(lua, row.player.as_deref())?,
                Value::String(lua.create_string(&row.link)?),
                Value::Integer(i64::from(row.count)),
                tab1,
                tab2,
            ];
            out.extend(age_parts(row.written_at, unix_now()).map(Value::Integer));
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetNumGuildBankMoneyTransactions() (`0x5a45b0`).
    g.set(
        "GetNumGuildBankMoneyTransactions",
        lua.create_function(|_, ()| {
            Ok(read_live(|l| l.guild_bank_log_len(GUILD_BANK_MONEY_LOG as u8)).unwrap_or(0) as i64)
        })?,
    )?;

    // GetGuildBankMoneyTransaction(index) (`0x5a4e50`) → type, name, amount, years, months,
    // days, hours. Deviation: a row past the log answers nothing.
    g.set(
        "GetGuildBankMoneyTransaction",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, index, "Usage: GetGuildBankMoneyTransaction(index)")?;
            let Some(row) =
                read_live(|l| l.guild_bank_log(GUILD_BANK_MONEY_LOG as u8, index)).flatten()
            else {
                return Ok(MultiValue::new());
            };
            let kind = match row.kind {
                4 => "deposit",
                5 => "withdraw",
                8 => "withdrawForTab",
                9 => "buyTab",
                _ => "repair",
            };
            let mut out = vec![
                Value::String(lua.create_string(kind)?),
                str_or_nil(lua, row.player.as_deref())?,
                Value::Integer(i64::from(row.money)),
            ];
            out.extend(age_parts(row.written_at, unix_now()).map(Value::Integer));
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // QueryGuildBankText(tab) (`0x5a7110` -> `0x5a5940`); its usage text names `QueryGuildBankLog`.
    // A fresh text, or no banker, answers `GUILDBANK_UPDATE_TEXT` inside the call.
    g.set(
        "QueryGuildBankText",
        lua.create_function(|lua, tab: Value| {
            let tab = index_arg(lua, tab, "Usage: QueryGuildBankLog(tab)")?;
            if write_live(|l| l.guild_bank_query_text(tab)).unwrap_or(true) {
                super::tick::fire_event_into(
                    lua,
                    "GUILDBANK_UPDATE_TEXT",
                    vec![super::ScriptValue::Int(i64::from(tab) + 1)],
                );
            }
            Ok(())
        })?,
    )?;

    // GetGuildBankText(tab) (`0x5a45e0`): nil past the bought tabs.
    g.set(
        "GetGuildBankText",
        lua.create_function(|lua, tab: Value| {
            let tab = index_arg(lua, tab, "Usage: GetGuildBankText(tab)")?;
            let text = (usize::from(tab) < GUILD_BANK_TABS && tab < num_tabs())
                .then(|| read_live(|l| l.guild_bank_tab(tab)).flatten())
                .flatten()
                .map(|t| t.text);
            str_or_nil(lua, text.as_deref())
        })?,
    )?;

    // SetGuildBankText(tab, text) (`0x5a7180`).
    g.set(
        "SetGuildBankText",
        lua.create_function(|lua, (tab, text): (Value, Value)| {
            const USAGE: &str = "Usage: SetGuildBankText(tab, text)";
            let tab = index_arg(lua, tab, USAGE)?;
            let text = match text {
                Value::String(s) => s.to_string_lossy(),
                Value::Integer(n) => n.to_string(),
                Value::Number(n) => n.to_string(),
                _ => return Err(mlua::Error::runtime(USAGE)),
            };
            if bank_open() && tab < num_tabs() {
                let mut model = crate::script::model_mut_of(lua);
                model
                    .guild_bank_requests
                    .push(GuildBankRequest::SetText { tab, text });
            }
            Ok(())
        })?,
    )?;

    // SetGuildBankTabInfo(tab, name, iconIndex) (`0x5a6a00`): the icon from the item-icon list,
    // the name through the tab-name check, whose refusal shows as a name error (`0x5a5a00`).
    g.set(
        "SetGuildBankTabInfo",
        lua.create_function(|lua, (tab, name, icon): (Value, Value, Value)| {
            const USAGE: &str = "Usage: SetGuildBankTabInfo(tab, name, iconIndex)";
            let tab = index_arg(lua, tab, USAGE)?;
            let name = match name {
                Value::String(s) => s.to_string_lossy(),
                Value::Integer(n) => n.to_string(),
                Value::Number(n) => n.to_string(),
                _ => return Err(mlua::Error::runtime(USAGE)),
            };
            let icon = number_arg(lua, icon, USAGE)?;
            let mut model = crate::script::model_mut_of(lua);
            build_item_icons(&mut model);
            let Some(icon) = usize::try_from(icon)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| model.macro_item_icons.get(i))
                .map(|p| p.rsplit('\\').next().unwrap_or(p).to_string())
            else {
                return Ok(());
            };
            if let Err(e) = crate::names::check_guild_bank_tab_name(&name) {
                model.ui_errors.push(e.key());
                return Ok(());
            }
            if bank_open() && tab < num_tabs() {
                model
                    .guild_bank_requests
                    .push(GuildBankRequest::SetTabInfo { tab, name, icon });
            }
            Ok(())
        })?,
    )?;

    // GetNumMacroItemIcons() (`0x5664e0`) and GetMacroItemIconInfo(index) (`0x564f60`): the
    // `INV_` icons, the question mark first; past the list `""`.
    g.set(
        "GetNumMacroItemIcons",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            build_item_icons(&mut model);
            Ok(model.macro_item_icons.len() as i64)
        })?,
    )?;
    g.set(
        "GetMacroItemIconInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetMacroItemIconInfo(index)")?;
            let mut model = crate::script::model_mut_of(lua);
            build_item_icons(&mut model);
            let path = (index as u32)
                .checked_sub(1)
                .and_then(|i| model.macro_item_icons.get(i as usize))
                .cloned()
                .unwrap_or_default();
            Ok(path)
        })?,
    )?;

    Ok(())
}

/// Run the item-icon builder once (`0x565840`).
fn build_item_icons(model: &mut Model) {
    if let Some(source) = model.macro_item_icon_source.take() {
        model.macro_item_icons = source();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::live::{lend_test, TestView};
    use crate::script::{GuildRankInfo, GuildState, LiveLoan, UiScript};

    fn slot(item_id: u32, count: u32) -> Option<GuildBankSlotView> {
        Some(GuildBankSlotView {
            item_id,
            texture: Some(format!("Interface\\Icons\\I{item_id}")),
            count,
            link: Some(format!(
                "|cffffffff|Hitem:{item_id}:0:0:0|h[I{item_id}]|h|r"
            )),
            max_stack: 20,
            ..Default::default()
        })
    }

    fn bank() -> GuildBankView {
        let mut slots = vec![Vec::new(); GUILD_BANK_TABS];
        slots[0] = vec![None; GUILD_BANK_TAB_SLOTS];
        slots[0][2] = slot(2589, 7);
        slots[0][97] = slot(4306, 20);
        let mut logs = vec![Vec::new(); GUILD_BANK_TABS + 1];
        logs[0] = vec![GuildBankLogView {
            kind: 3,
            player: Some("Claude".into()),
            link: "L".into(),
            count: 5,
            other_tab: 2,
            written_at: unix_now() - (2 * 24 + 3) * 3600,
            ..Default::default()
        }];
        logs[GUILD_BANK_MONEY_LOG] = vec![GuildBankLogView {
            kind: 8,
            money: 1000,
            written_at: unix_now(),
            ..Default::default()
        }];
        GuildBankView {
            open: true,
            num_tabs: 2,
            tabs: vec![
                GuildBankTabView {
                    name: "Mats".into(),
                    icon: "INV_Fabric_Linen_01".into(),
                    text: "hello".into(),
                },
                GuildBankTabView::default(),
            ]
            .into_iter()
            .chain(std::iter::repeat_with(Default::default).take(4))
            .collect(),
            slots,
            withdrawals_remaining: 3,
            logs,
        }
    }

    fn member(rank_rights: u32, money_per_day: u32, tabs: [(u32, u32); 6]) -> GuildState {
        GuildState {
            in_guild: true,
            rank_index: 1,
            rights: rank_rights,
            ranks: vec![
                GuildRankInfo::default(),
                GuildRankInfo {
                    rights: rank_rights,
                    bank_money_per_day: money_per_day,
                    bank_tabs: tabs.to_vec(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    /// The bank lent as the live view, with control and 50000 copper in the bank.
    fn lend_bank() -> LiveLoan<TestView> {
        lend_test(TestView {
            control: true,
            guild_bank: bank(),
            guild_bank_money: 50_000,
            guild_bank_text_stale: vec![1],
            ..Default::default()
        })
    }

    fn vm() -> (UiScript, LiveLoan<TestView>) {
        let bank = lend_bank();
        let mut s = UiScript::new().unwrap();
        s.set_guild(member(
            WITHDRAW_GOLD,
            100,
            [(0x7, 10), (0x1, 0), (0, 0), (0, 0), (0, 0), (0, 0)],
        ));
        (s, bank)
    }

    #[test]
    fn tab_info_reads_the_view_and_our_rank() {
        let (s, _bank) = vm();
        let row = s
            .eval::<(String, String, Option<i64>, Option<i64>, i64, i64)>(
                "return GetGuildBankTabInfo(1)",
            )
            .unwrap();
        assert_eq!(
            row,
            (
                "Mats".into(),
                "Interface\\Icons\\INV_Fabric_Linen_01".into(),
                Some(1),
                Some(1),
                10,
                3
            )
        );
        let row = s
            .eval::<(Option<String>, String, Option<i64>, Option<i64>)>(
                "local n, i, v, d = GetGuildBankTabInfo(2) return n, i, v, d",
            )
            .unwrap();
        assert_eq!(row.1, "Interface\\Icons\\INV_Misc_QuestionMark");
        assert_eq!((row.2, row.3), (Some(1), None), "tab 2 is view-only");
        assert_eq!(
            s.eval::<(Option<String>, i64, i64)>(
                "local n, _, _, _, w, r = GetGuildBankTabInfo(3) return n, w, r"
            )
            .unwrap(),
            (None, 0, 0),
            "past the bought tabs"
        );
        assert!(s
            .eval::<bool>("return CanEditGuildTabInfo(1) == 1")
            .unwrap());
        assert!(s
            .eval::<bool>("return CanEditGuildTabInfo(2) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return CanWithdrawGuildBankMoney() == 1")
            .unwrap());
        assert_eq!(
            s.eval::<i64>("return GetGuildBankTabCost()").unwrap(),
            5_000_000
        );
        assert_eq!(
            s.eval::<String>("return GetGuildBankText(1)").unwrap(),
            "hello"
        );
        assert!(s.eval::<bool>("return GetGuildBankText(3) == nil").unwrap());
    }

    #[test]
    fn item_info_and_links_read_the_listed_slots() {
        let (s, _bank) = vm();
        assert_eq!(
            s.eval::<(String, i64, Option<i64>)>("return GetGuildBankItemInfo(1, 3)")
                .unwrap(),
            ("Interface\\Icons\\I2589".into(), 7, None)
        );
        assert_eq!(
            s.eval::<(Option<String>, i64, Option<i64>)>("return GetGuildBankItemInfo(1, 1)")
                .unwrap(),
            (None, 0, None)
        );
        assert_eq!(
            s.eval::<(Option<String>, i64)>("return GetGuildBankItemInfo(2, 1)")
                .unwrap(),
            (None, 0),
            "an unlisted tab"
        );
        assert!(s
            .eval::<String>("return GetGuildBankItemLink(1, 98)")
            .unwrap()
            .contains("item:4306:"));
        assert_eq!(s.arity("GetGuildBankItemLink(1, 1)").unwrap(), 0);
    }

    #[test]
    fn a_pickup_locks_the_slot_and_a_drop_on_another_asks_a_move() {
        let (mut s, _bank) = vm();
        s.run("PickupGuildBankItem(1, 3)").unwrap();
        assert!(s
            .eval::<bool>("local _, _, l = GetGuildBankItemInfo(1, 3) return l == 1")
            .unwrap());
        let (kind, id) = s.eval::<(String, i64)>("return GetCursorInfo()").unwrap();
        assert_eq!((kind.as_str(), id), ("item", 2589));
        assert!(s.eval::<bool>("return not CursorHasItem()").unwrap());
        s.run("PickupGuildBankItem(1, 10)").unwrap();
        assert_eq!(
            s.take_guild_bank_requests(),
            vec![GuildBankRequest::MoveItem {
                tab: 0,
                slot: 9,
                src_tab: 0,
                src_slot: 2,
                item_id: 2589,
                split: None
            }]
        );
        // The app sent it: the cursor clears and the slot unlocks.
        s.guild_bank_swap_sent();
        assert!(s.eval::<bool>("return GetCursorInfo() == nil").unwrap());
        assert!(s
            .eval::<bool>("local _, _, l = GetGuildBankItemInfo(1, 3) return l == nil")
            .unwrap());
    }

    #[test]
    fn a_split_takes_one_to_the_stack_size() {
        let (mut s, _bank) = vm();
        s.run("SplitGuildBankItem(1, 98, 21)").unwrap();
        assert!(s.eval::<bool>("return GetCursorInfo() == nil").unwrap());
        s.run("SplitGuildBankItem(1, 98, 5)").unwrap();
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert_eq!(
            s.take_guild_bank_requests(),
            vec![GuildBankRequest::WithdrawItem {
                src_tab: 0,
                src_slot: 97,
                item_id: 4306,
                bag: 0,
                bag_slot: 1,
                split: Some(5)
            }]
        );
        assert!(s.eval::<bool>("return GetCursorInfo() == nil").unwrap());
    }

    #[test]
    fn bank_money_rides_cursor_mode_12() {
        let (s, _bank) = vm();
        assert_eq!(s.eval::<i64>("return GetGuildBankMoney()").unwrap(), 50_000);
        s.run("PickupGuildBankMoney(60000)").unwrap();
        assert!(
            s.eval::<bool>("return GetCursorInfo() == nil").unwrap(),
            "past the bank"
        );
        s.run("PickupGuildBankMoney(1234)").unwrap();
        assert_eq!(
            s.eval::<(String, i64)>("return GetCursorInfo()").unwrap(),
            ("guildbankmoney".into(), 1234)
        );
        assert_eq!(s.eval::<i64>("return GetCursorMoney()").unwrap(), 1234);
        assert!(s.eval::<bool>("return CursorHasMoney() == nil").unwrap());
        s.run("ClearCursor()").unwrap();
        assert!(s.eval::<bool>("return GetCursorInfo() == nil").unwrap());
    }

    #[test]
    fn money_and_tab_sends_queue_their_requests() {
        let (mut s, _bank) = vm();
        s.set_money(500);
        s.run("DepositGuildBankMoney(600)").unwrap();
        assert!(s.take_guild_bank_requests().is_empty());
        assert_eq!(s.take_ui_errors(), vec!["ERR_NOT_ENOUGH_MONEY"]);
        s.run("DepositGuildBankMoney(400) WithdrawGuildBankMoney(7.9) BuyGuildBankTab()")
            .unwrap();
        assert_eq!(
            s.take_guild_bank_requests(),
            vec![
                GuildBankRequest::DepositMoney(400),
                GuildBankRequest::WithdrawMoney(7),
            ]
        );
        assert_eq!(
            s.take_ui_errors(),
            vec!["ERR_BANKSLOT_INSUFFICIENT_FUNDS"],
            "the third tab costs more than purse and bank"
        );
        s.run("QueryGuildBankTab(2) QueryGuildBankTab(7) QueryGuildBankLog(7) SetGuildBankText(1, 'x') AutoStoreGuildBankItem(1, 3)").unwrap();
        assert_eq!(
            s.take_guild_bank_requests(),
            vec![
                GuildBankRequest::QueryTab(1),
                GuildBankRequest::QueryLog(6),
                GuildBankRequest::SetText {
                    tab: 0,
                    text: "x".into()
                },
                GuildBankRequest::AutoStore {
                    tab: 0,
                    slot: 2,
                    item_id: 2589,
                    count: 7
                },
            ]
        );
    }

    /// The lock, the cursor and the text's events fire inside their verb's call, in 12340's order:
    /// the clear's and the setter's `CURSOR_UPDATE` before the lock's (`0x519476`, `0x520da7`, `0x5137f1`); a stale
    /// text sends and a fresh one answers at once (`0x5a5940`); `CloseGuildBankFrame` fires
    /// `GUILDBANKFRAME_CLOSED` before the banker clears, open or not (`0x5a4270`).
    #[test]
    fn the_bank_verbs_fire_their_events_inside_the_call() {
        let (s, bank) = vm();
        s.run(
            r#"
            seen = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"CURSOR_UPDATE", "GUILDBANK_ITEM_LOCK_CHANGED",
                "GUILDBANK_UPDATE_TEXT", "GUILDBANKFRAME_CLOSED"}) do f:RegisterEvent(e) end
            f:SetScript("OnEvent", function(_, e, a)
                local _, _, locked = GetGuildBankItemInfo(1, 3)
                seen[#seen + 1] = e .. ":" .. tostring(a) .. ":" .. tostring(locked)
                    .. ":" .. tostring(GetNumGuildBankTabs())
            end)
            PickupGuildBankItem(1, 3)
            calls = #seen
            QueryGuildBankText(2)
            QueryGuildBankText(1)
            CloseGuildBankFrame()
            CloseGuildBankFrame()
            "#,
        )
        .unwrap();
        assert_eq!(
            s.eval::<i64>("return calls").unwrap(),
            3,
            "inside the pickup"
        );
        let seen: Vec<String> = s.eval("return seen").unwrap();
        assert_eq!(
            seen,
            [
                "CURSOR_UPDATE:nil:nil:2",
                "CURSOR_UPDATE:nil:nil:2",
                "GUILDBANK_ITEM_LOCK_CHANGED:nil:1:2",
                "GUILDBANK_UPDATE_TEXT:1:1:2",
                "GUILDBANKFRAME_CLOSED:nil:1:2",
                "GUILDBANKFRAME_CLOSED:nil:1:2",
            ]
        );
        assert_eq!(bank.with_view(|v| v.guild_bank_text_sent.clone()), [1]);
        assert!(!bank.with_view(|v| v.guild_bank.open), "the banker cleared");
    }

    #[test]
    fn the_logs_read_with_their_ages() {
        let (s, _bank) = vm();
        assert_eq!(
            s.eval::<i64>("return GetNumGuildBankTransactions(1)")
                .unwrap(),
            1
        );
        assert_eq!(
            s.eval::<i64>("return GetNumGuildBankTransactions(8)")
                .unwrap(),
            0
        );
        let row = s
            .eval::<(String, String, String, i64, i64, i64)>(
                "local t, n, l, c, a, b = GetGuildBankTransaction(1, 1) return t, n, l, c, a, b",
            )
            .unwrap();
        assert_eq!(row, ("move".into(), "Claude".into(), "L".into(), 5, 1, 3));
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(
                "local _, _, _, _, _, _, y, m, d, h = GetGuildBankTransaction(1, 1) return y, m, d, h"
            )
            .unwrap(),
            (0, 0, 2, 3)
        );
        assert_eq!(
            s.eval::<i64>("return GetNumGuildBankMoneyTransactions()")
                .unwrap(),
            1
        );
        assert_eq!(
            s.eval::<(String, Option<String>, i64)>("return GetGuildBankMoneyTransaction(1)")
                .unwrap(),
            ("withdrawForTab".into(), None, 1000)
        );
    }

    #[test]
    fn tabard_files_follow_the_tabard_or_are_nil() {
        let (mut s, _bank) = vm();
        assert_eq!(s.arity("GetGuildTabardFileNames()").unwrap(), 6);
        assert!(s
            .eval::<bool>("return GetGuildTabardFileNames() == nil")
            .unwrap());
        let mut g = member(0, 0, [(0, 0); 6]);
        g.tabard = Some([3, 15, 1, 4, 41]);
        s.set_guild(g);
        assert_eq!(
            s.eval::<(String, String, String, String, String, String)>(
                "return GetGuildTabardFileNames()"
            )
            .unwrap(),
            (
                "Textures\\GuildEmblems\\Background_41_TU_U".into(),
                "Textures\\GuildEmblems\\Background_41_TL_U".into(),
                "Textures\\GuildEmblems\\Emblem_03_15_TU_U".into(),
                "Textures\\GuildEmblems\\Emblem_03_15_TL_U".into(),
                "Textures\\GuildEmblems\\Border_01_04_TU_U".into(),
                "Textures\\GuildEmblems\\Border_01_04_TL_U".into(),
            )
        );
    }

    #[test]
    fn the_tab_icon_chooser_and_a_tab_rename() {
        let (mut s, _bank) = vm();
        s.set_macro_item_icon_source(Box::new(|| {
            vec![
                "Interface\\Icons\\INV_Misc_QuestionMark".into(),
                "Interface\\Icons\\INV_Axe_01".into(),
            ]
        }));
        assert_eq!(s.eval::<i64>("return GetNumMacroItemIcons()").unwrap(), 2);
        assert_eq!(
            s.eval::<String>("return GetMacroItemIconInfo(2)").unwrap(),
            "Interface\\Icons\\INV_Axe_01"
        );
        assert_eq!(
            s.eval::<String>("return GetMacroItemIconInfo(3)").unwrap(),
            ""
        );
        s.run("SetGuildBankTabInfo(1, 'Ore 2', 2)").unwrap();
        s.run("SetGuildBankTabInfo(1, \"Ore's\", 2)").unwrap();
        assert_eq!(
            s.take_guild_bank_requests(),
            vec![GuildBankRequest::SetTabInfo {
                tab: 0,
                name: "Ore 2".into(),
                icon: "INV_Axe_01".into()
            }]
        );
        assert_eq!(s.take_ui_errors(), vec!["ERR_NAME_INVALID"]);
    }
}
