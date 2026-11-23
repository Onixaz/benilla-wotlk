//! The app side of the loot window. [`LootState`] is the client's loot info: the wire's rows and the
//! window's copy of them (`0xbfa690`), its coins (`[0xbfa8d0]`) and object (`[0xbfa8d8]`). The packet
//! handlers move it and raise its events at their step (`0x5896b0`, `0x589920`, `0x589100`,
//! `0x589a30`), a record that lands late raises from its callback (`0x589460`), the verbs read it at
//! the call through [`LootLive`], and the acting verbs run inside their call ([`run_verb`]).

use benilla_protocol::messages::{slot_type, ItemPushResult, LootItem, BAG_PLAYER_INVENTORY};
use bevy::prelude::*;

use benilla_ui::script::{LootRow, LootState as LootWindow, LootVerb, ScriptValue, UiScript};

use crate::entities::ItemDisplays;
use crate::items::{Items, RecordWait, RecordWaits, RollCatalogs};
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_action::signals::SignalSink;
use crate::ui_action::UiError;
use crate::ui_items::KEYRING_CONTAINER;
use crate::ui_party::{GroupState, GROUPTYPE_RAID, GROUP_MEMBER_SUBGROUP};
use crate::ui_script::UiDrain;

mod looters;
mod net;
mod push;

pub(crate) use looters::{apply_looters, LootLists};
pub(crate) use push::push_callback;

/// `ItemBondingType::BIND_WHEN_PICKED_UP`, the first conjunct of the `LOOT_BIND` confirm: the take
/// dispatcher `0x589140` compares the item record's `+0x178` against 1.
const BIND_WHEN_PICKED_UP: u32 = 1;

/// The second conjunct: uncommon or better (`0x589140`: `1 < [rec+0x14]`).
const BIND_CONFIRM_MIN_QUALITY: u32 = 2;

/// The third: not `ITEM_FLAG_MULTI_DROP` (`0x589140`: `[rec+0x18] & 0x800`), whose every looter
/// gets a copy, so taking it costs nobody else the drop.
const ITEM_FLAG_MULTI_DROP: u32 = 0x800;

/// The coin row's quality, 0, so its text is grey: the reference's coin guard returns 0, and
/// `LootFrame_Update` has no coin case (`LootFrame.lua:81-85`).
const COIN_QUALITY: u32 = 0;
/// The slots the item-push result handler treats as the keyring: `slot - 0x56 <= 0x1f`
/// (`0x523a36`).
const PUSH_KEYRING_SLOTS: std::ops::RangeInclusive<u32> = 86..=117;

/// `ITEM_PUSH`'s `arg1`, the reference's selector: a wire bag other than 255 gives `bag + 1`
/// (20..=23 for an equipped bag, the ids its bar buttons carry), a keyring slot gives
/// [`KEYRING_CONTAINER`], and anything else 0, the backpack.
fn push_container(bag: u8, slot: u32) -> i64 {
    if bag != BAG_PLAYER_INVENTORY {
        return i64::from(bag) + 1;
    }
    if PUSH_KEYRING_SLOTS.contains(&slot) {
        return KEYRING_CONTAINER;
    }
    0
}

/// A table row's record wait (`[row+0]`): none, the open's (1), or a slot change's (`0xfb`), which
/// signals its row when the record lands (`0x589460`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RowWait {
    #[default]
    None,
    Open,
    Changed,
}

/// The open loot as the wire delivered it, the window's record waits, plus the push queue, which
/// outlives the window. The layout is fixed at open: a looted row is a gap, as `LOOT_SLOT_CLEARED`
/// hides its button in place (`LootFrame.lua:22-37`).
#[derive(Resource, Default)]
pub(crate) struct LootState {
    /// The looted object (`[0xbfa8d8]`); `None` when no loot is open.
    source: Option<u64>,
    /// The coin pile in copper, 0 once taken.
    gold: u32,
    /// Whether layout position 1 is the coin pile: fixed at open, kept after the coin is taken.
    coin_slot: bool,
    /// The table rows in wire order; never shrinks while open. A looted row is zeroed in place
    /// (`0x589980`-`0x5899aa`), item 0 its gap.
    items: Vec<LootItem>,
    /// Each row's record wait, beside `items`.
    waits: Vec<RowWait>,
    /// The records the window still waits on (`[0xbfa688]`); it opens at 0.
    pending: u32,
    /// The pushes waiting on their item record, each to run again from its callback (`0x512b60`).
    receives: Vec<ItemPushResult>,
    /// The open's wire `loot_type` (`[0xbfa68c]`), which `IsFishingLoot` and the close's sweep
    /// read; a close leaves it.
    loot_type: u8,
    /// The master-loot candidates (`0xca0550`), in wire order: the last `SMSG_LOOT_MASTER_LIST`'s,
    /// which only the next list rewrites (`0x6fa690`); no window's open or close touches them.
    master_candidates: Vec<u64>,
    /// The display row a `LOOT_BIND_CONFIRM` is open for: set by `LootSlot`, cleared by the
    /// confirmed send and with each window. The reference keeps the item index at `[0xacee24]`
    /// (`0x589140`), and `ConfirmLootSlot` turns its slot into the item index the same way
    /// (`0x589300`), so the two compare alike; the event carries the display row.
    pending_bind_confirm: Option<u32>,
    /// The candidates whose name a `GetMasterLootCandidate` asked for, one per miss, each owing
    /// `UPDATE_MASTER_LOOT_LIST` when its name lands (`0x588981` -> `0x588150`).
    name_waits: Vec<u64>,
    /// The player's auto-loot flag (`[player+0x18e8]`), armed at the interact, spent by the sweep.
    auto_loot: bool,
    /// Wire slots whose take raises `LOOT_BIND_CONFIRM`, judged as their records land.
    binds: Vec<u8>,
    /// Sweep takes' pickup sounds (`0x58907f`), by display id, for the next system that can play.
    pickups: Vec<u32>,
    /// Steps at every open and close: a record callback left by another window finds it moved, as
    /// the set cancels each row's callback (`0x58975a`).
    generation: u64,
}

/// What a step of the window reads and raises through: the item cache and its callbacks, the
/// wire, the latch and the caller's sink.
pub(crate) struct LootStep<'a> {
    pub(crate) items: &'a Items,
    /// The carried-count check at the call (`0x754dd0`), over the bags as they stand.
    pub(crate) carry: &'a CarryCheck<'a>,
    pub(crate) waits: &'a mut RecordWaits,
    pub(crate) net: &'a NetCommands,
    pub(crate) latch: &'a mut LootLatch,
    pub(crate) sink: &'a mut dyn SignalSink,
}

/// The carried-count check a take runs on `(item_id, record, count)`.
pub(crate) type CarryCheck<'a> =
    dyn Fn(u32, &benilla_protocol::ItemInfo, u32) -> Option<benilla_ui::script::CarryRefusal> + 'a;

/// A check that refuses nothing, for a step with no bags.
#[cfg(test)]
pub(crate) fn no_carry_limit(
    _: u32,
    _: &benilla_protocol::ItemInfo,
    _: u32,
) -> Option<benilla_ui::script::CarryRefusal> {
    None
}

/// `CloseLoot`'s flags (`0x523640`): send the release, keep an item's loot open, show
/// `ERR_LOOT_NO_UI`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CloseFlags {
    pub(crate) release: bool,
    pub(crate) keep_item: bool,
    pub(crate) no_ui: bool,
}

/// The loot window's rows (`0x589a30` scans 18).
const LOOT_WINDOW_ROWS: usize = 18;

/// The client's candidate array: 40 slots, bound-checked by the getter.
const MASTER_LOOT_CANDIDATE_SLOTS: usize = 40;
/// A raid subgroup's block within it.
const MEMBERS_PER_RAID_GROUP: usize = 5;

/// A clicked row resolved: the coin pile, or an item at its wire loot slot.
enum LootAction {
    Money,
    Item {
        wire_slot: u8,
        display_id: u32,
        item_id: u32,
        /// The row's stack, the carried-count check's `count` (`0x58927c`).
        count: u32,
        /// `MASTER` opens the master-loot dropdown instead of a take.
        slot_type: u8,
    },
}

/// What a take leaves for after its signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Took {
    Nothing,
    /// The coins' clear emptied the window: `CloseLoot(1, 0, 0)` (`0x5891b8`).
    CloseOwed,
    /// The bind confirm signalled: the row is stashed after it (`0x5892d1`).
    BindConfirm,
}

impl LootState {
    /// The set's first half (`0x5896b0`-`0x5897bb`): a window open on another object lets it go,
    /// an item unlocking (`0x589736`), and signals `LOOT_CLOSED`.
    pub(crate) fn let_go(&mut self, sink: &mut dyn SignalSink) {
        let Some(old) = self.source else {
            return;
        };
        // The outgoing item unlocks whatever the new object is, itself included.
        if benilla_protocol::guid::is_item(old) {
            sink.unlock_item(old);
        }
        self.close_table(sink);
    }

    /// The window's set (`0x5264f0` -> `0x5896b0`): the window let go first ([`Self::let_go`]); then
    /// the rows are copied and each record is read with the window's callback, and with none pending
    /// the window opens (`0x5898bf`). Quest items ride the same rows at `slot = items.len() + i`.
    /// Returns the object a sweep's close let go.
    pub(crate) fn open(
        &mut self,
        source: u64,
        loot_type: u8,
        gold: u32,
        items: Vec<LootItem>,
        step: &mut LootStep,
    ) -> Option<u64> {
        self.let_go(step.sink);
        self.generation += 1;
        self.binds.clear();
        self.source = Some(source);
        self.gold = gold;
        self.coin_slot = gold > 0; // fixed for the window's lifetime
        self.items = items;
        self.items.truncate(LOOT_WINDOW_ROWS);
        self.pending_bind_confirm = None; // the set resets the stash (`0x589805`)
        self.loot_type = loot_type;
        self.pending = 0;
        self.waits = vec![RowWait::None; self.items.len()];
        for row in 0..self.items.len() {
            if !self.read_record(row, step) {
                self.waits[row] = RowWait::Open;
            }
        }
        if self.pending == 0 {
            return self.opened(step);
        }
        None
    }

    /// One row's record read with the window's callback (`0x589899`, `0x589af3`): a miss asks,
    /// leaves the callback and counts one more pending. Whether the record was there.
    fn read_record(&mut self, row: usize, step: &mut LootStep) -> bool {
        let entry = self.items[row].item_id;
        if step.items.ask_template(entry, 0, step.net).is_some() {
            return true;
        }
        step.waits
            .wait(entry, RecordWait::Loot(self.generation), false);
        self.pending += 1;
        false
    }

    /// The window's last record is in (`0x5898f2`, `0x58990b`; `0x5894fb`, `0x589511`): armed, the auto-loot sweep runs;
    /// otherwise `LOOT_OPENED 0`.
    fn opened(&mut self, step: &mut LootStep) -> Option<u64> {
        self.judge_binds(step.items);
        if self.auto_loot {
            return self.auto_loot_sweep(true, step.latch, step.net, step.sink);
        }
        step.sink.fire("LOOT_OPENED", vec![ScriptValue::Int(0)]);
        None
    }

    /// The window's record callback (`0x589460`): every row waiting on `entry` stops, a changed one
    /// signalling `LOOT_SLOT_CHANGED` with its slot, and the window opens once none is pending. A
    /// callback another window left finds the generation moved, as that window's set cancelled it.
    /// Deviation: a denied record counts as landed, where the reference returns on it and the window
    /// never opens, because an entry the server cannot describe would otherwise keep the loot shut.
    pub(crate) fn record_landed(
        &mut self,
        entry: u32,
        generation: u64,
        step: &mut LootStep,
    ) -> Option<u64> {
        if self.source.is_none() || generation != self.generation {
            return None;
        }
        let coin = u32::from(self.coin_slot);
        for row in 0..self.items.len() {
            if self.waits[row] == RowWait::None || self.items[row].item_id != entry {
                continue;
            }
            if self.waits[row] == RowWait::Changed {
                step.sink.fire(
                    "LOOT_SLOT_CHANGED",
                    vec![ScriptValue::Int(i64::from(coin + 1 + row as u32))],
                );
            }
            self.waits[row] = RowWait::None;
        }
        self.pending = self.pending.saturating_sub(1);
        if self.pending == 0 {
            return self.opened(step);
        }
        None
    }

    /// `0x6dc770`: a roll that ended makes its slot lootable, only in an open window on that
    /// object; the take reads the type at click time, so nothing redraws.
    pub(crate) fn clear_slot_type(&mut self, looted_target: u64, wire_slot: u32) {
        if self.source != Some(looted_target) {
            return;
        }
        for it in &mut self.items {
            if u32::from(it.slot) == wire_slot {
                it.slot_type = slot_type::ALLOW_LOOT;
            }
        }
    }

    /// `SMSG_LOOT_SLOT_CHANGED` (`0x6d58d0` -> `0x589a30`), only for the open object: the row
    /// holding the wire slot, else the first looted row, else a new row while fewer than 18. The
    /// slot type is the wire slot's own, kept from the response. Its record is read with the
    /// window's callback; with none pending the row signals `LOOT_SLOT_CHANGED` at once
    /// (`0x589b3a`), else the callback signals it (`0x5894ac`).
    pub(crate) fn slot_changed(
        &mut self,
        p: &benilla_protocol::messages::LootSlotChanged,
        step: &mut LootStep,
    ) {
        if self.source != Some(p.looted_target) {
            return;
        }
        let row = self
            .items
            .iter()
            .position(|it| live(it) && it.slot == p.slot)
            .or_else(|| self.items.iter().position(|it| !live(it)));
        // The wire slot's own type; a removal zeroed it (`0x6d58ba`).
        let kind = self
            .items
            .iter()
            .find(|it| live(it) && it.slot == p.slot)
            .map_or(slot_type::ALLOW_LOOT, |it| it.slot_type);
        let entry = LootItem {
            slot: p.slot,
            item_id: p.item_id,
            count: p.count,
            display_info_id: p.display_info_id,
            random_property_id: p.random_property_id,
            random_suffix: p.random_suffix,
            slot_type: kind,
        };
        let row = match row {
            Some(i) => {
                self.items[i] = entry;
                i
            }
            None if self.items.len() < LOOT_WINDOW_ROWS => {
                self.items.push(entry);
                self.waits.push(RowWait::None);
                self.items.len() - 1
            }
            None => return,
        };
        self.waits[row] = if self.read_record(row, step) {
            RowWait::None
        } else {
            RowWait::Changed
        };
        if self.pending == 0 {
            self.judge_binds(step.items);
            let slot = u32::from(self.coin_slot) + 1 + row as u32;
            step.sink
                .fire("LOOT_SLOT_CHANGED", vec![ScriptValue::Int(i64::from(slot))]);
        }
    }

    /// `SMSG_LOOT_MASTER_LIST` (`0x6fa690`): the list replaced whole, window or none.
    pub(crate) fn set_master_candidates(&mut self, candidates: Vec<u64>) {
        self.master_candidates = candidates;
    }

    /// `SMSG_LOOT_REMOVED` (`0x589920`): the row holding the wire slot is zeroed (`0x589980`-`0x5899aa`), its
    /// record callback cancelled, and signals `LOOT_SLOT_CLEARED` with its slot. Whether the window
    /// owes `CloseLoot(1, 0, 0)` after the signal (`0x589a19`): it has nothing left, or its object
    /// is not `held` (`0x5899e3`).
    pub(crate) fn remove_slot(
        &mut self,
        wire_slot: u8,
        held: bool,
        waits: &mut RecordWaits,
        sink: &mut dyn SignalSink,
    ) -> bool {
        let Some(row) = self
            .items
            .iter()
            .position(|it| live(it) && it.slot == wire_slot)
        else {
            return false;
        };
        if self.waits[row] != RowWait::None {
            // Every callback of the window's on that record goes (`0x589973`); the count stays.
            let (entry, generation) = (self.items[row].item_id, self.generation);
            waits.cancel_on(entry, |w| w == RecordWait::Loot(generation));
            self.waits[row] = RowWait::None;
        }
        self.items[row] = LootItem {
            slot: 0,
            item_id: 0,
            count: 0,
            display_info_id: 0,
            random_property_id: 0,
            random_suffix: 0,
            slot_type: 0,
        };
        let slot = u32::from(self.coin_slot) + 1 + row as u32;
        sink.fire("LOOT_SLOT_CLEARED", vec![ScriptValue::Int(i64::from(slot))]);
        !self.has_loot(held)
    }

    /// The coins go (`SMSG_LOOT_CLEAR_MONEY`, `0x589100`; the coin take, `0x589184`): slot 1 signals
    /// `LOOT_SLOT_CLEARED`. Whether the window owes `CloseLoot(1, 0, 0)` after the signal
    /// (`0x589134`, `0x5891b8`): it has nothing left, or its object is not `held` (`0x588eb0`).
    pub(crate) fn clear_money(&mut self, held: bool, sink: &mut dyn SignalSink) -> bool {
        if self.source.is_none() || !self.has_coin() {
            return false;
        }
        self.gold = 0;
        sink.fire("LOOT_SLOT_CLEARED", vec![ScriptValue::Int(1)]);
        !self.has_loot(held)
    }

    /// The window still has loot (`0x588eb0`): its object `held`, with coins or a row left.
    fn has_loot(&self, held: bool) -> bool {
        held && !self.is_empty()
    }

    /// The table cleared by the set with no object (`0x5897b0`-`0x5897bb`): `LOOT_CLOSED`.
    fn close_table(&mut self, sink: &mut dyn SignalSink) {
        self.source = None;
        self.gold = 0;
        self.coin_slot = false;
        self.items.clear();
        self.waits.clear();
        self.pending = 0;
        self.pending_bind_confirm = None;
        self.binds.clear();
        self.generation += 1;
        sink.signal("LOOT_CLOSED");
    }

    /// Session end: drops the window, the queued pushes and the flag, raising nothing.
    pub(crate) fn clear_session(&mut self) {
        self.close_table(&mut Vec::new());
        self.loot_type = 0;
        self.master_candidates.clear();
        self.receives.clear();
        self.name_waits.clear();
        self.auto_loot = false;
        self.pickups.clear();
    }

    /// The auto-loot flag's setter (`0x6cde50`): arming it with a loot target latched sweeps at
    /// once (`0x6cde75`), which with no window spends the flag and closes nothing. Returns the
    /// object the sweep's close let go.
    pub(crate) fn set_auto_loot(
        &mut self,
        on: bool,
        latch: &mut LootLatch,
        net: &NetCommands,
        sink: &mut dyn SignalSink,
    ) -> Option<u64> {
        self.auto_loot = on;
        if on && latch.0.is_some() {
            return self.auto_loot_sweep(true, latch, net, sink);
        }
        None
    }

    /// The auto-loot sweep (`0x588f90`), with no record pending: the coins and every lootable row
    /// taken, the first bind-on-pickup row held back with `LOOT_BIND_CONFIRM`, the flag spent; then
    /// a window left with nothing closes (`0x5890e6`), else `LOOT_OPENED 1` when `signal`
    /// (`0x5890d3`). Returns the object that close let go.
    fn auto_loot_sweep(
        &mut self,
        signal: bool,
        latch: &mut LootLatch,
        net: &NetCommands,
        sink: &mut dyn SignalSink,
    ) -> Option<u64> {
        if self.pending != 0 {
            return None;
        }
        self.sweep(latch, net, sink);
        if self.is_empty() {
            let flags = CloseFlags {
                release: true,
                ..default()
            };
            return close(self, latch, net, flags, sink);
        }
        if signal {
            sink.fire("LOOT_OPENED", vec![ScriptValue::Int(1)]);
        }
        None
    }

    /// The sweep's takes (`0x588fd5`-`0x5890a7`): its sends do not move the window, whose rows go
    /// as the server's removals land.
    fn sweep(&mut self, latch: &LootLatch, net: &NetCommands, sink: &mut dyn SignalSink) {
        // Any nonzero coins, taken (-1) included (`0x588fcb`).
        if self.coin_slot && latch.0.is_some() {
            let _ = net.0.send(ClientCommand::LootMoney);
        }
        let mut confirmed = false;
        let rows = self.items.len() as u32 + u32::from(self.coin_slot);
        for index in 1..=rows {
            let Some(LootAction::Item {
                wire_slot,
                display_id,
                slot_type,
                ..
            }) = self.action_at(index)
            else {
                continue;
            };
            if slot_type != slot_type::ALLOW_LOOT && slot_type != slot_type::OWNER {
                continue;
            }
            if slot_type == slot_type::ALLOW_LOOT && self.binds.contains(&wire_slot) {
                if !confirmed {
                    confirmed = true;
                    self.pending_bind_confirm = Some(index);
                    sink.fire(
                        "LOOT_BIND_CONFIRM",
                        vec![ScriptValue::Int(i64::from(index))],
                    );
                }
                continue;
            }
            let _ = net
                .0
                .send(ClientCommand::AutostoreLootItem { slot: wire_slot });
            self.pickups.push(display_id);
        }
        self.auto_loot = false;
    }

    /// `SMSG_INVENTORY_CHANGE_FAILURE`'s loot tail (`0x6e321c`-`0x6e32a9`): a failure on the
    /// latched item closes its loot, and an open window with the flag armed signals
    /// `LOOT_OPENED 1`. Returns the object the close let go.
    pub(crate) fn inventory_failed(
        &mut self,
        item_guid: u64,
        latch: &mut LootLatch,
        net: &NetCommands,
        sink: &mut dyn SignalSink,
    ) -> Option<u64> {
        let mut closed = None;
        if item_guid != 0 && latch.0 == Some(item_guid) {
            closed = close_interaction(self, latch, net, true, sink);
        }
        if self.source.is_some() && self.auto_loot {
            sink.fire("LOOT_OPENED", vec![ScriptValue::Int(1)]);
        }
        closed
    }

    /// The take dispatcher (`0x589140`) on display row `index`: the coins send and clear with their
    /// signal (`0x589184`-`0x58919f`); an item row branches on its slot type, a roll in progress and
    /// a locked row refusing with their line (`0x589200`, `0x589217`) and a master-loot row opening
    /// the candidate list (`0x589230`); else its record, read with no callback, is needed
    /// (`0x589259`), and a bind-on-pickup row runs the carried-count check, refusing with its line
    /// (`0x589298`), then signals `LOOT_BIND_CONFIRM` (`0x5892c4`), where any other plays its pickup
    /// sound and sends (`0x5892df`, `0x5892f1`).
    fn take(&mut self, index: u32, held: bool, step: &mut LootStep) -> Took {
        match self.action_at(index) {
            Some(LootAction::Money) => {
                debug!("ui_loot: loot coin (row {index})");
                let _ = step.net.0.send(ClientCommand::LootMoney);
                if self.clear_money(held, step.sink) {
                    return Took::CloseOwed;
                }
            }
            Some(LootAction::Item { slot_type, .. }) if slot_type == slot_type::ROLL_ONGOING => {
                step.sink.line(UiError::key("ERR_LOOT_ROLL_PENDING"));
            }
            Some(LootAction::Item { slot_type, .. }) if slot_type == slot_type::LOCKED => {
                step.sink.line(UiError::key("ERR_LOOT_CANT_LOOT_THAT"));
            }
            Some(LootAction::Item { slot_type, .. }) if slot_type == slot_type::MASTER => {
                step.sink.fire("OPEN_MASTER_LOOT_LIST", Vec::new());
            }
            Some(LootAction::Item {
                wire_slot,
                display_id,
                item_id,
                count,
                slot_type,
            }) => {
                let Some(record) = step.items.template_cached(item_id) else {
                    debug!("ui_loot: row {index} has no cached record, no take");
                    return Took::Nothing;
                };
                if bind_confirm_required(record, slot_type) {
                    if let Some(refusal) = (step.carry)(item_id, record, count) {
                        debug!("ui_loot: row {index} refused by the carried count");
                        step.sink.line(carry_line(refusal));
                        return Took::Nothing;
                    }
                    step.sink.fire(
                        "LOOT_BIND_CONFIRM",
                        vec![ScriptValue::Int(i64::from(index))],
                    );
                    return Took::BindConfirm;
                }
                debug!("ui_loot: autostore row {index} (wire slot {wire_slot})");
                self.pickups.push(display_id);
                let _ = step
                    .net
                    .0
                    .send(ClientCommand::AutostoreLootItem { slot: wire_slot });
            }
            None => debug!("ui_loot: LootSlot({index}) names no row, ignored"),
        }
        Took::Nothing
    }

    /// `ConfirmLootSlot` (`0x589300`): only the stashed row takes, its pickup sound then its send,
    /// and the stash clears on the send (`0x589383`), so a doubled accept cannot loot twice; a row
    /// gone under the dialog sends nothing and keeps the stash (`0x58934e`).
    fn confirm(&mut self, index: u32, net: &NetCommands) {
        if self.pending_bind_confirm != Some(index) {
            return;
        }
        let Some(LootAction::Item {
            wire_slot,
            display_id,
            ..
        }) = self.action_at(index)
        else {
            return;
        };
        debug!("ui_loot: bind-confirmed row {index} (wire slot {wire_slot})");
        self.pickups.push(display_id);
        let _ = net
            .0
            .send(ClientCommand::AutostoreLootItem { slot: wire_slot });
        self.pending_bind_confirm = None;
    }

    /// `GiveMasterLoot` (`0x5893a0`) of display row `index` to `target`: a window open, the row an
    /// item, its slot type master (`0x5893ee`), then `CMSG_LOOT_MASTER_GIVE` (`0x589448`).
    fn give_master_loot(&self, index: u32, target: u64, net: &NetCommands) {
        let Some(guid) = self.source else {
            return;
        };
        let Some(LootAction::Item {
            wire_slot,
            slot_type,
            ..
        }) = self.action_at(index)
        else {
            return;
        };
        if slot_type != slot_type::MASTER {
            return;
        }
        debug!("ui_loot: master-give row {index} (wire slot {wire_slot}) to {target:#x}");
        let _ = net.0.send(ClientCommand::LootMasterGive {
            guid,
            slot: wire_slot,
            target,
        });
    }

    /// Judges every row's bind confirm off its cached record.
    fn judge_binds(&mut self, items: &Items) {
        self.binds = self
            .items
            .iter()
            .filter(|it| {
                items
                    .template_cached(it.item_id)
                    .is_some_and(|t| bind_confirm_required(t, it.slot_type))
            })
            .map(|it| it.slot)
            .collect();
    }

    /// A `GetMasterLootCandidate` miss on `guid`'s name: its callback waits (`0x588981`).
    fn wait_candidate_name(&mut self, guid: u64) {
        self.name_waits.push(guid);
    }

    /// The name callbacks left on `guid`, unlinked.
    pub(crate) fn take_name_waits(&mut self, guid: u64) -> usize {
        let before = self.name_waits.len();
        self.name_waits.retain(|&g| g != guid);
        before - self.name_waits.len()
    }

    fn take_pickups(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.pickups)
    }

    /// A test's window set on `source` with no record cached and nothing listening.
    #[cfg(test)]
    pub(crate) fn open_for_test(
        &mut self,
        source: u64,
        loot_type: u8,
        gold: u32,
        rows: Vec<LootItem>,
    ) {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut step = LootStep {
            items: &Items::default(),
            carry: &no_carry_limit,
            waits: &mut RecordWaits::default(),
            net: &NetCommands(tx),
            latch: &mut LootLatch::default(),
            sink: &mut Vec::new(),
        };
        self.open(source, loot_type, gold, rows, &mut step);
    }

    /// A test's row: `entry` at wire slot `slot` on the open window, its record taken as cached.
    #[cfg(test)]
    pub(crate) fn slot_changed_for_test(&mut self, source: u64, slot: u8, entry: u32) {
        if self.source != Some(source) {
            return;
        }
        self.items.push(LootItem {
            slot,
            item_id: entry,
            count: 1,
            display_info_id: 0,
            random_property_id: 0,
            random_suffix: 0,
            slot_type: slot_type::ALLOW_LOOT,
        });
        self.waits.push(RowWait::None);
    }

    /// The auto-loot flag, for the interact tests.
    #[cfg(test)]
    pub(crate) fn auto_loot_armed(&self) -> bool {
        self.auto_loot
    }

    /// Queued pushes, for the net handler's self-gate test.
    #[cfg(test)]
    pub(crate) fn pending_receive_count(&self) -> usize {
        self.receives.len()
    }

    /// Whether the window has coins left to take (`[0xbfa8d0]` above 0).
    fn has_coin(&self) -> bool {
        self.gold > 0
    }

    /// The open loot's guid; the chest lid watcher in [`crate::go_anim`] reads it too.
    pub(crate) fn source(&self) -> Option<u64> {
        self.source
    }

    /// Whether the last window's loot type (`[0xbfa68c]`) is fishing (wire `loot_type` 3).
    fn fishing(&self) -> bool {
        self.loot_type == benilla_protocol::messages::loot_type::FISHING
    }

    /// Nothing lootable left: no coins and no row with an item.
    fn is_empty(&self) -> bool {
        !self.has_coin() && !self.items.iter().any(live)
    }

    /// A 1-based display row of the fixed layout; a gap answers `None`, like the hidden button.
    fn action_at(&self, index_1based: u32) -> Option<LootAction> {
        let mut index = index_1based.checked_sub(1)? as usize; // 0-based layout position
        if self.coin_slot {
            if index == 0 {
                return self.has_coin().then_some(LootAction::Money);
            }
            index -= 1;
        }
        let it = self.items.get(index)?;
        live(it).then_some(LootAction::Item {
            wire_slot: it.slot,
            display_id: it.display_info_id,
            item_id: it.item_id,
            count: it.count,
            slot_type: it.slot_type,
        })
    }

    /// The candidate slots as the reference lays them out: wire order in a party; in a raid, each
    /// guid in the first free slot of its subgroup's block of five, holes kept for
    /// `GroupLootDropDown_Initialize`'s "Group N" submenus (`LootFrame.lua:197-213`). The feed and
    /// the drain share it, so an index means one candidate.
    fn placed_candidates(&self, group: &GroupState) -> Vec<Option<u64>> {
        if group.group_type != GROUPTYPE_RAID {
            return self.master_candidates.iter().copied().map(Some).collect();
        }
        let mut slots: Vec<Option<u64>> = vec![None; MASTER_LOOT_CANDIDATE_SLOTS];
        for &guid in &self.master_candidates {
            // A guid missing from the roster is ours: `SMSG_GROUP_LIST` lists only the others,
            // and `Group::MasterLoot` includes us.
            let flags = group
                .members
                .iter()
                .find(|m| m.guid == guid)
                .map_or(group.own_flags, |m| m.flags);
            let base = usize::from(flags & GROUP_MEMBER_SUBGROUP) * MEMBERS_PER_RAID_GROUP;
            let end = (base + MEMBERS_PER_RAID_GROUP).min(MASTER_LOOT_CANDIDATE_SLOTS);
            if let Some(free) = (base..end).find(|&i| slots[i].is_none()) {
                slots[free] = Some(guid);
            }
        }
        while slots.last().is_some_and(Option::is_none) {
            slots.pop(); // trailing empties read as nil either way
        }
        slots
    }

    /// The candidate at a 1-based menu index, through [`LootState::placed_candidates`].
    fn master_candidate(&self, index_1based: u32, group: &GroupState) -> Option<u64> {
        self.placed_candidates(group)
            .get(index_1based.checked_sub(1)? as usize)
            .copied()
            .flatten()
    }
}

/// The loot CVars: `autoLootDefault` (`[0xbd0914]`), read at the interact, and `showLootSpam`.
#[derive(Resource)]
pub(crate) struct LootConfig {
    pub(crate) auto_loot: bool,
    pub(crate) show_loot_spam: bool,
}

/// The auto-loot test (`0x513700`): `autoLootDefault`, inverted while `AUTOLOOTTOGGLE` is held.
pub(crate) fn is_auto_looting(cfg: &LootConfig, script: Option<&UiScript>) -> bool {
    cfg.auto_loot != script.is_some_and(|s| s.is_modified_click("AUTOLOOTTOGGLE"))
}

/// Pickpocket, skinning and skin-corpse effects arm the flag from `autoLootDefault` (`0x80f787`).
const ARMS_FROM_DEFAULT: [u32; 3] = [71, 95, 116];
/// Disenchant, prospecting and milling arm it only while the auto-loot test passes
/// (`0x80f796`-`0x80f7ae`).
const ARMS_WHILE_AUTO_LOOTING: [u32; 3] = [99, 127, 158];

/// Our own `SMSG_SPELL_GO`'s auto-loot arm (`0x80f780`-`0x80f7da`), unless already armed; returns
/// the object the sweep's close let go.
pub(crate) fn arm_on_spell_go(
    effects: &[u32; 3],
    loot: &mut LootState,
    cfg: &LootConfig,
    script: Option<&UiScript>,
    latch: &mut LootLatch,
    commands: &NetCommands,
    sink: &mut dyn SignalSink,
) -> Option<u64> {
    let arms = effects.iter().any(|e| {
        ARMS_FROM_DEFAULT.contains(e)
            || (ARMS_WHILE_AUTO_LOOTING.contains(e) && is_auto_looting(cfg, script))
    });
    if arms && !loot.auto_loot {
        return loot.set_auto_loot(cfg.auto_loot, latch, commands, sink);
    }
    None
}

impl Default for LootConfig {
    fn default() -> Self {
        Self {
            // Off leaves 1.12's shift-click as the only auto-loot.
            auto_loot: false,
            // The reference's registered `"1"`.
            show_loot_spam: true,
        }
    }
}

/// The loot-target latch, the reference's `[player+0x18e0]` guid; kneeling is [`LootKneel`]'s.
///
/// Deviation: our closes clear it guid-matched, where `CloseLoot` clears it always (`0x52370b`).
#[derive(Resource, Default)]
pub(crate) struct LootLatch(pub(crate) Option<u64>);

/// Whether the character kneels at the latched object, the reference's predicate B: yes for a
/// corpse or any GameObject but a fishing bobber, no for a live unit, an item or an unresolved
/// guid. Recomputed each frame between the net drain and the anim driver, as the reference
/// force-plays the pose at the arm itself.
#[derive(Resource, Default)]
pub(crate) struct LootKneel(pub(crate) bool);

/// `GAMEOBJECT_TYPE_ID` 17, `FISHINGNODE`: the one GameObject type that does not kneel.
const GO_TYPE_FISHINGNODE: i32 = 17;

/// Recomputes [`LootKneel`]; `pub(crate)` so [`crate::creature_anim`]'s driver can order after it.
pub(crate) fn resolve_loot_kneel(
    latch: Res<LootLatch>,
    index: Res<crate::net::GuidIndex>,
    objects: Query<(&crate::net::NetEntity, &crate::net::ObjectStore)>,
    mut kneel: ResMut<LootKneel>,
) {
    let allowed = latch
        .0
        .and_then(|guid| index.0.get(&guid).copied())
        .and_then(|e| objects.get(e).ok())
        .is_some_and(|(net_entity, store)| match net_entity.kind {
            // A GameObject kneels unless it is the bobber.
            benilla_protocol::EntityKind::GameObject => {
                store.0.gameobject_type_id() != GO_TYPE_FISHINGNODE
            }
            // A unit kneels once its health reads 0 off the descriptor, where an unsent field is 0
            // too; not `unit_is_dead`, whose `max_health` guard it lacks.
            benilla_protocol::EntityKind::Unit | benilla_protocol::EntityKind::Player => {
                store.0.unit_health().unwrap_or(0) == 0
            }
            // An item never kneels; we stream none, so a lockbox latch never resolves.
            _ => false,
        });
    if kneel.0 != allowed {
        kneel.0 = allowed;
    }
}

impl LootLatch {
    /// Drops the latch only if it still names `guid`.
    pub(crate) fn clear_for(&mut self, guid: u64) {
        if self.0 == Some(guid) {
            self.0 = None;
        }
    }
}

/// A movement start that closes the loot: the reference's guard, called by every movement-start
/// emitter, turning included, but not by mouse-look facing. No loot target reaches the range gate,
/// so there is no distance leash. vmangos also releases on movement (`MovementHandler.cpp:1104`),
/// but the close is the client's. A loot response that lands while we move sets it too.
#[derive(Resource, Default)]
pub(crate) struct LootMoveStart(pub(crate) bool);

pub(crate) struct UiLootPlugin;

/// Applies a loot CVar change.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut loot: ResMut<LootConfig>) {
    match ev.key().as_str() {
        "autolootdefault" => loot.auto_loot = ev.flag(),
        "showlootspam" => loot.show_loot_spam = ev.flag(),
        _ => {}
    }
}

impl Plugin for UiLootPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        looters::register(app);
        app.add_observer(on_cvar);
        app.init_resource::<LootState>()
            .init_resource::<LootConfig>()
            .init_resource::<LootLatch>()
            .init_resource::<LootKneel>()
            .init_resource::<LootMoveStart>()
            .add_systems(
                Update,
                (
                    // The drain's deselect lands before the selection writers read it, and their
                    // teardown's close finds this frame's window.
                    drain_loot
                        .in_set(UiDrain)
                        .before(crate::target::TargetUpdate),
                    // After the net drain and the selection writers, which arm and drop the
                    // latch; the anim driver orders after this.
                    resolve_loot_kneel
                        .after(benilla_world::schedule::WorldStage::Net)
                        .after(crate::target::TargetUpdate),
                ),
            );
    }
}

/// The coin row's icon: the reference's six-step ladder, shared with `GetCoinIcon` and the money
/// cursor.
fn coin_icon(copper: u32) -> &'static str {
    benilla_ui::script::coin_icon(i64::from(copper))
}

/// One wire row for the Lua: the icon from the wire display id, the rest from the item template.
/// The link's suffix factor is 0, as the wire's `randomSuffix` always is (`LootMgr.cpp:842`).
fn resolve_item(
    item: &LootItem,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: RollCatalogs,
) -> LootRow {
    let (name, quality, link) = match items.template_cached(item.item_id) {
        Some(t) => {
            // The rolled name: the reference composes every display of it.
            let name = rolls.name(&t.name, item.random_property_id);
            (
                Some(name.clone()),
                Some(t.quality),
                Some(crate::ui_items::item_link_full(
                    item.item_id,
                    &benilla_ui::script::ItemLinkFields::roll(
                        item.random_property_id as i32,
                        item.random_suffix as i32,
                    ),
                    &name,
                    t.quality,
                )),
            )
        }
        None => (None, None, None),
    };
    let texture = crate::ui_items::item_icon(icons, item.display_info_id);
    LootRow {
        name,
        texture,
        quantity: item.count,
        quality,
        is_coin: false,
        coins: 0,
        item_id: item.item_id,
        link,
        // The raw id, as the client's loot record keeps it; the tooltip resolves it.
        random_property_id: item.random_property_id,
        // `GetLootSlotInfo`'s fifth return reads the slot type (`0x588570`).
        locked: item.slot_type == slot_type::LOCKED,
    }
}

/// The open loot's display slots as the verbs read them at the call: the coin row when the window
/// has a coin slot (the coins, or a gap once taken), then the 18 table rows, an empty one a gap.
/// Each record is read with no callback, so a miss asks nothing (`0x588310`, `0x588430`,
/// `0x5884a0`).
fn window_rows(
    loot: &LootState,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: RollCatalogs,
) -> Vec<Option<LootRow>> {
    let mut rows = Vec::with_capacity(LOOT_WINDOW_ROWS + 1);
    if loot.coin_slot {
        rows.push(loot.has_coin().then(|| LootRow {
            // Its text is composed at the call from the global strings (`0x588430`).
            name: None,
            texture: Some(coin_icon(loot.gold).into()),
            // 0: the reference's coin guard returns before the record read.
            quantity: 0,
            quality: Some(COIN_QUALITY),
            is_coin: true,
            coins: loot.gold,
            item_id: 0,
            // No item behind it: a modified click finds nil.
            link: None,
            random_property_id: 0,
            locked: false,
        }));
    }
    for it in &loot.items {
        rows.push(live(it).then(|| resolve_item(it, items, icons, rolls)));
    }
    rows.resize(LOOT_WINDOW_ROWS + usize::from(loot.coin_slot), None);
    rows
}

/// What the loot verbs read at the call: the window, the item cache, the names and the source.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct LootLive<'w, 's> {
    loot: ResMut<'w, LootState>,
    items: Res<'w, Items>,
    icons: Option<Res<'w, ItemDisplays>>,
    props: Option<Res<'w, crate::items::RandomProperties>>,
    enchants: Option<Res<'w, crate::items::Enchants>>,
    group: Res<'w, GroupState>,
    names: Res<'w, NameCache>,
    commands: Res<'w, NetCommands>,
    objects: LootSourceObjects<'w, 's>,
}

impl LootLive<'_, '_> {
    /// The open loot resolved at the call; `None` with none open.
    pub(crate) fn window(&self) -> Option<LootWindow> {
        let source = self.loot.source?;
        let rolls = RollCatalogs {
            props: self.props.as_deref(),
            enchants: self.enchants.as_deref(),
        };
        let level = self.objects.unit_level(source);
        Some(LootWindow {
            rows: window_rows(&self.loot, &self.items, self.icons.as_deref(), rolls),
            coin_slot: self.loot.coin_slot,
            fishing: self.loot.fishing(),
            master_candidates: Vec::new(),
            source_unit: level.is_some(),
            source_level: level.unwrap_or(0),
        })
    }

    /// The looted object, 0 with none.
    pub(crate) fn source(&self) -> u64 {
        self.loot.source.unwrap_or(0)
    }

    /// Whether the last window's loot type is fishing (`0x5888e0`).
    pub(crate) fn fishing(&self) -> bool {
        self.loot.fishing()
    }

    /// The candidate at 0-based `index` named off the name cache; a miss asks the name and leaves
    /// the list's callback (`0x58899d`).
    pub(crate) fn master_candidate(&mut self, index: u32) -> Option<String> {
        let guid = self.loot.master_candidate(index + 1, &self.group)?;
        if let Some(name) = self.names.peek(guid).filter(|n| !n.is_empty()) {
            return Some(name.to_string());
        }
        let _ = self.names.resolve(guid, &self.commands);
        self.loot.wait_candidate_name(guid);
        None
    }
}

/// `f` over the window with its step lent from `world`, raising into the event queue; the object a
/// close let go deselects in turn when dead. Nothing runs where the loot is not set up.
pub(super) fn with_window(
    world: &mut World,
    f: impl FnOnce(&mut LootState, &mut LootStep) -> Option<u64>,
) {
    use crate::ui_script::live::UiEvents;
    if !world.contains_resource::<Items>() || !world.contains_resource::<NetCommands>() {
        return;
    }
    let Some(mut loot) = world.remove_resource::<LootState>() else {
        return;
    };
    let mut latch = world.remove_resource::<LootLatch>().unwrap_or_default();
    let mut ui = world.remove_resource::<UiEvents>().unwrap_or_default();
    let mut waits = world
        .remove_non_send_resource::<RecordWaits>()
        .unwrap_or_default();
    let carry = |item_id: u32, record: &benilla_protocol::ItemInfo, count: u32| {
        crate::ui_items::carry_refusal_in(world, item_id, record, count)
    };
    let closed = f(
        &mut loot,
        &mut LootStep {
            items: world.resource::<Items>(),
            carry: &carry,
            waits: &mut waits,
            net: world.resource::<NetCommands>(),
            latch: &mut latch,
            sink: &mut ui,
        },
    );
    deselect_dead_in_turn(&mut ui, closed);
    let pickups = loot.take_pickups();
    world.insert_resource(loot);
    world.insert_resource(latch);
    world.insert_resource(ui);
    world.insert_non_send_resource(waits);
    play_pickups(world, pickups);
}

/// The takes' pickup sounds (`0x5892df`, `0x58936c`, `0x58907f`), to the sound player.
fn play_pickups(world: &mut World, display_ids: Vec<u32>) {
    use bevy::ecs::message::Messages;
    let Some(mut sounds) = world.get_resource_mut::<Messages<crate::sound::LootPickupSound>>()
    else {
        return;
    };
    for display_id in display_ids {
        sounds.write(crate::sound::LootPickupSound { display_id });
    }
}

/// Whether the open window's object is held, as the window's loot test reads it (`0x588eb0`).
fn source_held(world: &World) -> bool {
    let source = world
        .get_resource::<LootState>()
        .and_then(LootState::source);
    world
        .get_resource::<crate::net::GuidIndex>()
        .zip(source)
        .is_some_and(|(index, guid)| index.0.contains_key(&guid))
}

/// A loot verb inside its call, with the `World` lent: it acts on the window at its step and
/// raises there, so the code after it in the same call reads what it did.
pub(crate) fn run_verb(world: &mut World, verb: LootVerb) {
    use crate::ui_script::live::in_turn;
    match verb {
        LootVerb::Slot(index) => {
            let held = source_held(world);
            let mut took = Took::Nothing;
            with_window(world, |loot, step| {
                took = loot.take(index, held, step);
                None
            });
            match took {
                // The emptied window closes after the clear's signal (`0x58919f`, `0x5891b8`).
                Took::CloseOwed => in_turn(world, close_emptied),
                // The row is stashed after its signal (`0x5892c4`, `0x5892d1`): a listener's own
                // `ConfirmLootSlot` meets the stash as it stood.
                Took::BindConfirm => in_turn(world, move |w| {
                    if let Some(mut loot) = w.get_resource_mut::<LootState>() {
                        loot.pending_bind_confirm = Some(index);
                    }
                }),
                Took::Nothing => {}
            }
        }
        LootVerb::Confirm(index) => with_window(world, |loot, step| {
            loot.confirm(index, step.net);
            None
        }),
        LootVerb::MasterGive { slot, candidate } => {
            // The binding resolves the candidate first; none sends nothing (`0x589676`, `0x589680`).
            let target = world
                .get_resource::<LootState>()
                .zip(world.get_resource::<GroupState>())
                .and_then(|(loot, group)| loot.master_candidate(candidate, group));
            let (Some(target), Some(loot), Some(net)) = (
                target,
                world.get_resource::<LootState>(),
                world.get_resource::<NetCommands>(),
            ) else {
                return;
            };
            loot.give_master_loot(slot, target, net);
        }
        LootVerb::Roll { roll_id, roll_type } => {
            crate::ui_loot_roll::vote_in_call(world, roll_id, roll_type);
        }
        // `CloseLoot(1, 0, noUI)` (`0x5888c0`): the close chain up to `LOOT_CLOSED` (`0x523784`).
        LootVerb::Close { no_ui } => with_window(world, |loot, step| {
            let flags = CloseFlags {
                release: true,
                keep_item: false,
                no_ui,
            };
            close(loot, step.latch, step.net, flags, step.sink)
        }),
    }
}

/// The window's record callback (`0x589460`) in its turn, with the `World` lent.
pub(crate) fn record_callback(world: &mut World, entry: u32, generation: u64) {
    with_window(world, |loot, step| {
        loot.record_landed(entry, generation, step)
    });
}

/// The response's set of the window (`0x6d5798` -> `0x5264f0`), in the handler's turn after the
/// outgoing window's `LOOT_CLOSED`; a window opened while we move closes after the open's own
/// signals (`0x6d57b5`).
pub(crate) fn set_window(
    world: &mut World,
    source: u64,
    loot_type: u8,
    gold: u32,
    rows: Vec<LootItem>,
    moving: bool,
) {
    with_window(world, |loot, step| {
        loot.open(source, loot_type, gold, rows, step)
    });
    if moving {
        crate::ui_script::live::in_turn(world, |world| {
            with_window(world, |loot, step| {
                close_on_move_start(loot, step.latch, step.net, step.sink)
            });
        });
    }
}

/// `CloseLoot(1, 0, 0)` owed by a window its clear left with nothing (`0x589a19`, `0x589134`), in
/// turn after the clear's signal.
pub(crate) fn close_emptied(world: &mut World) {
    with_window(world, |loot, step| {
        loot.source?;
        close_interaction(loot, step.latch, step.net, true, step.sink)
    });
}

/// The object manager as `SetLootPortrait` asks it (typemask 8): whether the loot source is a unit
/// object we hold.
#[derive(bevy::ecs::system::SystemParam)]
struct LootSourceObjects<'w, 's> {
    index: Option<Res<'w, crate::net::GuidIndex>>,
    stores: Query<'w, 's, &'static crate::net::ObjectStore>,
}

impl LootSourceObjects<'_, '_> {
    /// The source's level when it resolves as a unit (`[unit+0xd0]+0xc0`, `0x61e7bf`), else `None`.
    fn unit_level(&self, guid: u64) -> Option<u32> {
        self.index
            .as_ref()
            .and_then(|index| index.0.get(&guid))
            .and_then(|&e| self.stores.get(e).ok())
            .filter(|s| s.is_unit())
            .map(|s| s.0.unit_level().unwrap_or(0))
    }
}

/// The carried-count check's message (`0x754e2a`, `0x754ea7`): the category's quantity, then its
/// name.
pub(crate) fn carry_line(refusal: benilla_ui::script::CarryRefusal) -> crate::ui_action::UiError {
    use crate::ui_action::{FillArg, UiError};
    let key = refusal.key();
    match refusal {
        benilla_ui::script::CarryRefusal::MaxCount => UiError::key(key),
        benilla_ui::script::CarryRefusal::Category { quantity, name } => {
            UiError::args(key, vec![FillArg::D(i64::from(quantity)), FillArg::S(name)])
        }
    }
}

/// Whether a take must raise `LOOT_BIND_CONFIRM` first (`0x589140`): bind on pickup, uncommon or
/// better, not multi-drop, and not an `OWNER` slot; the take then runs the carried-count check
/// before the dialog (`0x589298`).
fn bind_confirm_required(t: &benilla_protocol::ItemInfo, kind: u8) -> bool {
    kind != slot_type::OWNER
        && t.bonding == BIND_WHEN_PICKED_UP
        && t.quality >= BIND_CONFIRM_MIN_QUALITY
        && t.flags & ITEM_FLAG_MULTI_DROP == 0
}

/// A row in the table: one with an item; a looted row is item 0.
fn live(it: &LootItem) -> bool {
    it.item_id != 0
}

/// Disenchanting, prospecting and milling: a close takes their rows left (`0x523681`-`0x5236ab`).
const SWEPT_ON_CLOSE: [u8; 3] = {
    use benilla_protocol::messages::loot_type as t;
    [t::DISENCHANTING, t::PROSPECTING, t::MILLING]
};

/// `CloseLoot(release, 0, 0)` (`0x523640`); returns the closed source for [`DeadUnitDeselect`].
pub(crate) fn close_interaction(
    loot: &mut LootState,
    latch: &mut LootLatch,
    commands: &NetCommands,
    release: bool,
    sink: &mut dyn SignalSink,
) -> Option<u64> {
    let flags = CloseFlags {
        release,
        ..default()
    };
    close(loot, latch, commands, flags, sink)
}

/// `CloseLoot` (`0x523640`): a disenchant, prospect or mill window takes what is left
/// (`0x523681`-`0x5236ab`), `ERR_LOOT_NO_UI` shows (`0x5236be`), an item's loot unlocks it or,
/// kept, stays open (`0x5236d5`-`0x5236fc`); then the latch (`0x52370b`), the release (`0x523754`)
/// and the set with no object, which signals `LOOT_CLOSED` (`0x523784`).
pub(crate) fn close(
    loot: &mut LootState,
    latch: &mut LootLatch,
    commands: &NetCommands,
    flags: CloseFlags,
    sink: &mut dyn SignalSink,
) -> Option<u64> {
    let source = loot.source?;
    // The sweep returns at once while a record is pending (`0x588f99`).
    if SWEPT_ON_CLOSE.contains(&loot.loot_type)
        && !loot.is_empty()
        && !flags.keep_item
        && loot.pending == 0
    {
        loot.sweep(latch, commands, sink);
    }
    if flags.no_ui {
        sink.line(UiError::key("ERR_LOOT_NO_UI"));
    }
    if benilla_protocol::guid::is_item(source) {
        if flags.keep_item {
            return None;
        }
        sink.unlock_item(source);
    }
    latch.clear_for(source);
    if flags.release {
        let _ = commands.0.send(ClientCommand::LootRelease { guid: source });
    }
    loot.close_table(sink);
    Some(source)
}

/// `CloseLoot`'s dead-unit test (`0x52378c`-`0x5237ae`), for a closer without [`DeadUnitDeselect`].
pub(crate) fn dead_unit_source(objects: &crate::net::Objects, guid: u64) -> bool {
    use benilla_protocol::messages::ObjectType;
    objects.object(guid).is_some_and(|f| {
        matches!(f.object_type(), Some(ObjectType::Unit | ObjectType::Player))
            && (f.unit_health().unwrap_or(0) as i32) <= 0
    })
}

/// The interaction close's last step: a closed source that is a unit, a player included, with
/// `UNIT_FIELD_HEALTH` at or below 0 is deselected if it is still the selection (`(guid, 1)`). It
/// reads the health, never the loot type, so a skinned corpse loses the selection and a live
/// pickpocket target keeps it; an unstreamed source is left alone.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct DeadUnitDeselect<'w, 's> {
    index: Res<'w, crate::net::GuidIndex>,
    objects: Query<
        'w,
        's,
        (
            &'static crate::net::NetEntity,
            &'static crate::net::ObjectStore,
        ),
    >,
    asks: MessageWriter<'w, crate::target::DeselectGuid>,
}

impl DeadUnitDeselect<'_, '_> {
    pub(crate) fn after_close(&mut self, closed: Option<u64>) {
        let Some(source) = closed else {
            return;
        };
        let dead_unit = self
            .index
            .0
            .get(&source)
            .and_then(|&e| self.objects.get(e).ok())
            .is_some_and(|(net_entity, store)| {
                matches!(
                    net_entity.kind,
                    benilla_protocol::EntityKind::Unit | benilla_protocol::EntityKind::Player
                ) && (store.0.unit_health().unwrap_or(0) as i32) <= 0 // signed, `jg`
            });
        if dead_unit {
            self.asks.write(crate::target::DeselectGuid(source));
        }
    }
}

/// The close's dead-unit deselect (`0x52378c`) in `ui`'s turn, for a closer that holds no
/// [`DeadUnitDeselect`].
pub(crate) fn deselect_dead_in_turn(
    ui: &mut crate::ui_script::live::UiEvents,
    closed: Option<u64>,
) {
    let Some(source) = closed else {
        return;
    };
    ui.world(move |world| {
        let dead = world
            .get_resource::<crate::net::GuidIndex>()
            .and_then(|index| index.0.get(&source).copied())
            .and_then(|e| {
                let kind = world.get::<crate::net::NetEntity>(e)?.kind;
                let store = world.get::<crate::net::ObjectStore>(e)?;
                Some(
                    matches!(
                        kind,
                        benilla_protocol::EntityKind::Unit | benilla_protocol::EntityKind::Player
                    ) && (store.0.unit_health().unwrap_or(0) as i32) <= 0,
                )
            })
            .unwrap_or(false);
        // Inside the close, after its `LOOT_CLOSED` (`0x5237ae`): the code after the close reads
        // the selection dropped.
        if dead && !crate::target::deselect_guid_now(world, source) {
            world.write_message(crate::target::DeselectGuid(source));
        }
    });
}

/// The held item `guid` unlocked (`0x513770`): a lock the open's send made clears and signals at
/// the item's place (`0x707020`), then the bar's usable update runs (`0x51379b`).
pub(crate) fn unlock_item_now(world: &mut World, guid: u64) {
    if let Some(args) = unlock_item_args(world, guid) {
        if let Some(mut ui) = world.get_resource_mut::<crate::ui_script::live::UiEvents>() {
            crate::ui_items::watch::fire_lock_edge(&mut ui, false, args);
        }
    }
    crate::ui_script::live::in_turn(
        world,
        crate::ui_action::usable_cache::step(crate::ui_action::usable_cache::Pass::Bar),
    );
}

/// The held item `guid` locked (`0x513740`): the edge's signals at the item's place when its lock
/// moved (`0x706fe0`), then the bar's usable update (`0x51376b`).
pub(crate) fn lock_item_now(world: &mut World, guid: u64) {
    let args = world
        .run_system_cached_with(
            |In(guid): In<u64>, objects: crate::net::Objects| {
                crate::ui_items::watch::lock_args(&objects, guid)
            },
            guid,
        )
        .ok()
        .flatten();
    crate::ui_items::watch::lock_now(
        world,
        crate::ui_items::watch::LockStep {
            guid,
            locked: true,
            args,
        },
    );
}

/// The held item `guid`'s lock cleared; the unlock edge's arguments when a lock moved (`0x707020`).
pub(crate) fn unlock_item_args(world: &mut World, guid: u64) -> Option<Vec<ScriptValue>> {
    let unlocked = world
        .get_resource_mut::<crate::pending_item_ops::PendingItemOps>()
        .is_some_and(|mut p| p.unlock(guid));
    if !unlocked {
        return None;
    }
    world
        .run_system_cached_with(
            |In(guid): In<u64>, objects: crate::net::Objects| {
                crate::ui_items::watch::lock_args(&objects, guid)
            },
            guid,
        )
        .ok()
        .flatten()
}

/// A capture fixture's window set where no handler runs, raised at once with the `World` lent.
pub(crate) fn open_in_world(
    world: &mut World,
    source: u64,
    loot_type: u8,
    gold: u32,
    rows: Vec<LootItem>,
) {
    set_window(world, source, loot_type, gold, rows, false);
    crate::ui_script::live::fire_handler_events(world);
}

/// `CloseLoot(1, 1, 0)` on a movement start (`0x72e653`): no sweep, and an item's loot survives.
pub(crate) fn close_on_move_start(
    loot: &mut LootState,
    latch: &mut LootLatch,
    commands: &NetCommands,
    sink: &mut dyn SignalSink,
) -> Option<u64> {
    let source = loot.source()?;
    debug!("ui_loot: movement started with {source:#x} open — release");
    let flags = CloseFlags {
        release: true,
        keep_item: true,
        no_ui: false,
    };
    close(loot, latch, commands, flags, sink)
}

/// The movement start's close (`0x72e653`) in its own step, then the pickup sounds of the sweeps
/// made outside a window step (`0x58907f`).
fn drain_loot(world: &mut World) {
    let moved = world
        .get_resource_mut::<LootMoveStart>()
        .is_some_and(|mut m| std::mem::take(&mut m.0));
    if moved {
        with_window(world, |loot, step| {
            close_on_move_start(loot, step.latch, step.net, step.sink)
        });
    }
    let pickups = world
        .get_resource_mut::<LootState>()
        .map(|mut l| l.take_pickups())
        .unwrap_or_default();
    play_pickups(world, pickups);
    crate::ui_script::live::fire_handler_events(world);
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::loot_type;
    use benilla_protocol::messages::GroupMemberEntry;
    use benilla_protocol::messages::ItemInfo;
    use benilla_protocol::messages::ObjectFields;
    use benilla_protocol::EntityKind;

    /// A raise as a test reads it: the event and its arguments, or a line's key.
    #[derive(Default)]
    struct Fired(Vec<String>);

    impl SignalSink for Fired {
        fn signal(&mut self, event: &'static str) {
            self.0.push(event.into());
        }

        fn fire(&mut self, event: &'static str, args: Vec<ScriptValue>) {
            let mut line = event.to_string();
            for arg in args {
                match arg {
                    ScriptValue::Int(i) => line.push_str(&format!(" {i}")),
                    other => line.push_str(&format!(" {other:?}")),
                }
            }
            self.0.push(line);
        }

        fn line(&mut self, line: UiError) {
            self.0.push(line.key.into());
        }

        fn unlock_item(&mut self, guid: u64) {
            self.0.push(format!("<unlock {guid:#x}>"));
        }
    }

    /// What a window step reads outside an app, its raises recorded.
    struct Bench {
        items: Items,
        waits: RecordWaits,
        net: NetCommands,
        rx: crossbeam_channel::Receiver<ClientCommand>,
        latch: LootLatch,
        fired: Fired,
    }

    impl Bench {
        fn new() -> Self {
            let (tx, rx) = crossbeam_channel::unbounded();
            Self {
                items: Items::default(),
                waits: RecordWaits::default(),
                net: NetCommands(tx),
                rx,
                latch: LootLatch::default(),
                fired: Fired::default(),
            }
        }

        fn step(&mut self) -> LootStep<'_> {
            LootStep {
                items: &self.items,
                carry: &no_carry_limit,
                waits: &mut self.waits,
                net: &self.net,
                latch: &mut self.latch,
                sink: &mut self.fired,
            }
        }

        /// `entry`'s record answered: the window's callbacks waiting on it run, in ask order.
        fn answer(&mut self, loot: &mut LootState, entry: u32, info: Option<ItemInfo>) {
            self.items.insert_template(entry, info);
            for wait in self.waits.take(entry) {
                if let RecordWait::Loot(generation) = wait {
                    loot.record_landed(entry, generation, &mut self.step());
                }
            }
        }

        /// The raises since the last take.
        fn fired(&mut self) -> Vec<String> {
            std::mem::take(&mut self.fired.0)
        }

        fn sent(&self) -> Vec<ClientCommand> {
            self.rx.try_iter().collect()
        }

        /// A record answered, as the cache holds it.
        fn template(&mut self, entry: u32, name: &str) {
            self.items
                .insert_template(entry, Some(crate::items::test_template(name)));
        }
    }

    /// `loot` set on `source` through a fresh bench, its raises dropped.
    fn open_bare(loot: &mut LootState, source: u64, kind: u8, gold: u32, rows: Vec<LootItem>) {
        loot.open(source, kind, gold, rows, &mut Bench::new().step());
    }

    /// `f` over the app's window with its step lent, then what it raised fired.
    fn window_step(
        world: &mut World,
        f: impl FnOnce(&mut LootState, &mut LootStep) -> Option<u64>,
    ) {
        if !world.contains_resource::<crate::ui_script::live::UiEvents>() {
            world.init_resource::<crate::ui_script::live::UiEvents>();
        }
        super::with_window(world, f);
        crate::ui_script::live::fire_handler_events(world);
    }

    /// `src` run with the `World` lent, as every call into Lua runs: the loot verbs act inside it.
    fn run_lua(world: &mut World, src: &str) {
        if !world.contains_resource::<crate::ui_script::live::UiEvents>() {
            world.init_resource::<crate::ui_script::live::UiEvents>();
        }
        crate::ui_script::safe_world::lend(world, |s| s.run(src))
            .expect("a VM")
            .expect("the chunk runs");
    }

    /// The app's window set on `source`, as the response's set runs it.
    fn open_window(world: &mut World, source: u64, kind: u8, gold: u32, rows: Vec<LootItem>) {
        window_step(world, |loot, step| {
            loot.open(source, kind, gold, rows, step)
        });
    }

    /// Descriptor field indices the predicate-B table reads.
    const F_GO_TYPE_ID: u16 = benilla_protocol::field::FIELD_GAMEOBJECT_BYTES_1;
    const F_UNIT_HEALTH: u16 = benilla_protocol::field::FIELD_UNIT_HEALTH;

    /// Predicate B's answer for one latched object; `None` latches a guid that does not resolve.
    fn kneels_at(object: Option<(EntityKind, &[(u16, u32)])>) -> bool {
        const GUID: u64 = 0xF110_0000_0000_0042;
        let mut app = App::new();
        app.init_resource::<LootLatch>()
            .init_resource::<LootKneel>()
            .init_resource::<crate::net::GuidIndex>()
            .add_systems(Update, resolve_loot_kneel);
        app.world_mut().resource_mut::<LootLatch>().0 = Some(GUID);
        if let Some((kind, fields)) = object {
            let e = app
                .world_mut()
                .spawn((
                    crate::net::NetEntity {
                        kind,
                        display_id: None,
                        scale: 1.0,
                    },
                    crate::net::ObjectStore(ObjectFields::from_pairs(fields)),
                ))
                .id();
            app.world_mut()
                .resource_mut::<crate::net::GuidIndex>()
                .0
                .insert(GUID, e);
        }
        app.update();
        app.world().resource::<LootKneel>().0
    }

    #[test]
    fn predicate_b_decides_which_loot_targets_are_knelt_at() {
        // A GameObject other than the bobber: a chest (3), a fishing hole (25).
        assert!(kneels_at(Some((
            EntityKind::GameObject,
            &[(F_GO_TYPE_ID, 3 << 8)]
        ))));
        assert!(kneels_at(Some((
            EntityKind::GameObject,
            &[(F_GO_TYPE_ID, 25 << 8)]
        ))));
        // Type 17, `FISHINGNODE`.
        assert!(!kneels_at(Some((
            EntityKind::GameObject,
            &[(F_GO_TYPE_ID, 17 << 8)]
        ))));
        // A corpse kneels, a live (pickpocketed) target does not.
        assert!(kneels_at(Some((EntityKind::Unit, &[(F_UNIT_HEALTH, 0)]))));
        assert!(!kneels_at(Some((EntityKind::Unit, &[(F_UNIT_HEALTH, 1)]))));
        // An unresolved guid, which an item latch is for us.
        assert!(!kneels_at(None));
    }

    #[test]
    fn a_cold_latch_never_kneels() {
        let mut app = App::new();
        app.init_resource::<LootLatch>()
            .init_resource::<LootKneel>()
            .init_resource::<crate::net::GuidIndex>()
            .add_systems(Update, resolve_loot_kneel);
        app.update();
        assert!(!app.world().resource::<LootKneel>().0);
    }

    // ── The soulbind confirm ──────────────────────────────────────────────────
    //
    // vmangos `item_template` rows: Felstriker a BoP epic, Flurry Axe a BoE epic, Tough Jerky a
    // plain white; 9999 is a synthetic white BoP, the quality control.
    const FELSTRIKER: u32 = 12590;
    const FLURRY_AXE: u32 = 871;
    const TOUGH_JERKY: u32 = 117;
    const WHITE_BOP: u32 = 9999;
    /// Brutality Blade, a second BoP epic, for the sweep's one-dialog latch.
    const SECOND_BOP: u32 = 18832;
    /// A BoP epic flagged `ITEM_FLAG_MULTI_DROP`, which takes no confirm.
    const MULTI_DROP_BOP: u32 = 90_001;

    /// A loot window over `rows` with every template landed, the display rows `clicks` taken
    /// through `LootSlot` as the stock row click does (`LootFrame.lua:221`), then `lua` run, then
    /// one drain.
    fn drain_with(
        gold: u32,
        rows: Vec<LootItem>,
        clicks: &[u32],
        lua: &str,
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_message::<crate::sound::LootPickupSound>()
            .add_message::<crate::target::DeselectGuid>()
            .init_resource::<LootState>()
            .init_resource::<LootLatch>()
            .init_resource::<LootMoveStart>()
            .init_resource::<GroupState>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::ui_script::live::UiEvents>()
            .init_resource::<crate::pending_item_ops::PendingItemOps>()
            .insert_resource(NetCommands(tx));

        let mut items = app.world_mut().resource_mut::<Items>();
        let mut tmpl = |entry: u32, name: &str, quality: u32, bonding: u32| {
            items.insert_template(
                entry,
                Some(ItemInfo {
                    quality,
                    bonding,
                    ..crate::items::test_template(name)
                }),
            );
        };
        tmpl(FELSTRIKER, "Felstriker", 4, 1);
        tmpl(FLURRY_AXE, "Flurry Axe", 4, 2);
        tmpl(TOUGH_JERKY, "Tough Jerky", 1, 0);
        tmpl(WHITE_BOP, "A White Soulbound Thing", 1, 1);
        items.insert_template(
            MULTI_DROP_BOP,
            Some(ItemInfo {
                quality: 4,
                bonding: 1,
                flags: ITEM_FLAG_MULTI_DROP,
                ..crate::items::test_template("A Shared Epic")
            }),
        );

        // The window's object is held, as the window's loot test reads it (`0x588eb0`).
        let held = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(0x42, held);
        open_window(app.world_mut(), 0x42, loot_type::CORPSE, gold, rows);

        let script = UiScript::new().unwrap();
        script
            .run(
                "BIND_CONFIRMS = {}\n\
                 local f = CreateFrame(\"Frame\")\n\
                 f:RegisterEvent(\"LOOT_BIND_CONFIRM\")\n\
                 f:SetScript(\"OnEvent\", function(self, event, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) tinsert(BIND_CONFIRMS, arg1) end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        for &row in clicks {
            run_lua(app.world_mut(), &format!("LootSlot({row})"));
        }
        if !lua.is_empty() {
            run_lua(app.world_mut(), lua);
        }
        (app, rx)
    }

    /// `SetLootPortrait` answers 1 only while the loot source is a unit object we hold: a
    /// creature's corpse, never a chest or a source out of view, read at the call.
    #[test]
    fn the_loot_portrait_follows_a_unit_source_only() {
        use benilla_protocol::ObjectFields;
        for (unit, expected) in [(true, true), (false, false)] {
            let (tx, _rx) = crossbeam_channel::unbounded();
            let mut app = App::new();
            app.init_resource::<LootState>()
                .init_resource::<LootLatch>()
                .init_resource::<GroupState>()
                .init_resource::<NameCache>()
                .init_resource::<Items>()
                .init_resource::<crate::net::GuidIndex>()
                .insert_resource(NetCommands(tx));
            // OBJECT_FIELD_TYPE (2): TYPEMASK_OBJECT | TYPEMASK_UNIT for a creature, | 0x20 for a
            // game object.
            let kind = if unit { 0x9 } else { 0x21 };
            let e = app
                .world_mut()
                .spawn(crate::net::ObjectStore(ObjectFields::from_pairs(&[(
                    benilla_protocol::field::FIELD_OBJECT_TYPE,
                    kind,
                )])))
                .id();
            app.world_mut()
                .resource_mut::<crate::net::GuidIndex>()
                .0
                .insert(0x42, e);
            open_window(app.world_mut(), 0x42, loot_type::CORPSE, 5, Vec::new());
            let script = UiScript::new().unwrap();
            script
                .run(r#"CreateFrame("Frame", "LootHost"):CreateTexture("LootTex")"#)
                .unwrap();
            app.insert_non_send_resource(script);
            let answered = crate::ui_script::live::with_live_world(app.world_mut(), |s| {
                s.eval::<bool>("return SetLootPortrait(LootTex) == 1")
            })
            .expect("a VM")
            .unwrap();
            assert_eq!(answered, expected, "unit source: {unit}");
        }
    }

    /// The window opens at its last record's callback (`0x589460`), once; here a denied record
    /// opens it too.
    #[test]
    fn the_window_waits_for_every_item_template() {
        for answer in [
            Some(ItemInfo {
                quality: 1,
                ..crate::items::test_template("Thin Cloth Gloves")
            }),
            None,
        ] {
            let mut b = Bench::new();
            let mut loot = LootState::default();
            loot.open(
                0x42,
                loot_type::CORPSE,
                0,
                vec![item(3, TOUGH_JERKY, 1)],
                &mut b.step(),
            );
            assert!(
                b.fired().is_empty(),
                "no LOOT_OPENED while a record is pending"
            );
            assert!(
                b.sent().iter().any(
                    |c| matches!(c, ClientCommand::ItemQuery { entry, .. } if *entry == TOUGH_JERKY)
                ),
                "and the record was asked for"
            );
            b.answer(&mut loot, TOUGH_JERKY, answer.clone());
            assert_eq!(b.fired(), ["LOOT_OPENED 0"], "opened at the last answer");
            b.answer(&mut loot, TOUGH_JERKY, answer.clone());
            assert!(b.fired().is_empty(), "and only once");
        }
    }

    /// `0x589a30`/`0x589460`: a cached row redraws at once, an uncached one when its record lands,
    /// whose callback opens the window again (`0x589511`); the row number counts the coin row;
    /// another object's message is dropped.
    #[test]
    fn a_changed_slot_fires_once_its_template_is_cached() {
        const CLEAVER: u32 = 4949;
        let mut b = Bench::new();
        b.template(TOUGH_JERKY, "Tough Jerky");
        let mut loot = LootState::default();
        loot.open(
            0x42,
            loot_type::CORPSE,
            5,
            vec![item(0, TOUGH_JERKY, 1), item(1, TOUGH_JERKY, 2)],
            &mut b.step(),
        );
        assert_eq!(b.fired(), ["LOOT_OPENED 0"]);
        let msg = |guid: u64, slot: u8, item_id: u32, count: u32| {
            benilla_protocol::messages::LootSlotChanged {
                looted_target: guid,
                slot,
                item_id,
                display_info_id: 7,
                random_suffix: 0,
                random_property_id: 0,
                count,
            }
        };

        loot.slot_changed(&msg(0x99, 1, TOUGH_JERKY, 5), &mut b.step());
        assert!(b.fired().is_empty(), "another object's change is dropped");
        loot.slot_changed(&msg(0x42, 1, TOUGH_JERKY, 5), &mut b.step());
        assert_eq!(
            b.fired(),
            ["LOOT_SLOT_CHANGED 3"],
            "wire slot 1 is row 2, after the coin row"
        );
        assert_eq!(loot.items[1].count, 5);

        loot.slot_changed(&msg(0x42, 0, CLEAVER, 1), &mut b.step());
        assert!(b.fired().is_empty(), "the uncached row waits");
        b.answer(
            &mut loot,
            CLEAVER,
            Some(crate::items::test_template("Orcish Cleaver")),
        );
        assert_eq!(b.fired(), ["LOOT_SLOT_CHANGED 2", "LOOT_OPENED 0"]);
        b.answer(
            &mut loot,
            CLEAVER,
            Some(crate::items::test_template("Orcish Cleaver")),
        );
        assert!(b.fired().is_empty(), "once");
    }

    /// `0x6dc770`: a finished roll makes its slot lootable in the window on that object only.
    #[test]
    fn a_finished_roll_clears_the_slot_type() {
        let mut loot = LootState::default();
        let mut rolling = item(2, TOUGH_JERKY, 1);
        rolling.slot_type = slot_type::ROLL_ONGOING;
        open_bare(&mut loot, 0x42, loot_type::CORPSE, 0, vec![rolling]);
        loot.clear_slot_type(0x99, 2);
        assert_eq!(loot.items[0].slot_type, slot_type::ROLL_ONGOING);
        loot.clear_slot_type(0x42, 2);
        assert_eq!(loot.items[0].slot_type, slot_type::ALLOW_LOOT);
    }

    /// The rows `LOOT_BIND_CONFIRM` has named so far, in order.
    fn confirms(app: &mut App) -> Vec<i64> {
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        let n = script.eval::<i64>("return getn(BIND_CONFIRMS)").unwrap();
        (1..=n)
            .map(|i| {
                script
                    .eval::<i64>(&format!("return BIND_CONFIRMS[{i}]"))
                    .unwrap()
            })
            .collect()
    }

    fn sent(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<ClientCommand> {
        rx.try_iter().collect()
    }

    fn pending_confirm(app: &App) -> Option<u32> {
        app.world().resource::<LootState>().pending_bind_confirm
    }

    /// `LootSlot`'s deferral (`0x589140`).
    #[test]
    fn a_bop_row_confirms_instead_of_sending() {
        let (mut app, rx) = drain_with(0, vec![item(0, FELSTRIKER, 1)], &[1], "");
        assert!(sent(&rx).is_empty(), "the deferred take sends nothing");
        assert_eq!(pending_confirm(&app), Some(1), "and stashes the row");

        assert_eq!(
            confirms(&mut app),
            vec![1],
            "the event carries the row out, 1-based and display-side"
        );
    }

    /// `0x589298`: a bind-on-pickup row runs the carried-count check before the dialog; at the
    /// limit it shows the line and neither confirms nor sends, below it the dialog opens.
    #[test]
    fn a_bop_row_past_the_carried_count_is_refused_before_the_dialog() {
        use benilla_protocol::field::*;
        use benilla_protocol::ObjectFields;
        const ME: u64 = 0x77;
        let (mut app, rx) = drain_with(0, vec![item(0, FELSTRIKER, 1)], &[], "");
        let world = app.world_mut();
        world.resource_mut::<Items>().insert_template(
            FELSTRIKER,
            Some(ItemInfo {
                quality: 4,
                bonding: 1,
                max_count: 1,
                ..crate::items::test_template("Felstriker")
            }),
        );
        let me = world
            .spawn(crate::net::ObjectStore(ObjectFields::from_pairs(&[])))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(ME, me);
        world.insert_resource(crate::net::SelfGuid(Some(ME)));
        let _ = crate::ui_action::take_lines_in_turn();
        run_lua(world, "LootSlot(1)");
        assert_eq!(confirms(&mut app), vec![1], "none held: below the count");
        assert_eq!(pending_confirm(&app), Some(1));

        // One held in the backpack: the next is past the count.
        let world = app.world_mut();
        crate::items::test_spawn_item(
            world,
            0xF1,
            ObjectFields::from_pairs(&[
                (FIELD_OBJECT_ENTRY, FELSTRIKER),
                (FIELD_ITEM_STACK_COUNT, 1),
            ]),
            false,
        );
        let mut store = world.get_mut::<crate::net::ObjectStore>(me).unwrap();
        store.0 = ObjectFields::from_pairs(&[
            (FIELD_PLAYER_PACK_SLOT_1, 0xF1),
            (FIELD_PLAYER_PACK_SLOT_1 + 1, 0),
        ]);
        run_lua(world, "LootSlot(1)");
        assert_eq!(confirms(&mut app), vec![1], "no second dialog");
        assert!(sent(&rx).is_empty(), "nothing goes out");
        let lines: Vec<_> = crate::ui_action::take_lines_in_turn()
            .into_iter()
            .map(|l| l.key)
            .collect();
        assert_eq!(lines, ["ERR_ITEM_MAX_COUNT"], "shown in the take's call");
    }

    /// `ConfirmLootSlot` (`0x589300`).
    #[test]
    fn confirm_loot_slot_completes_the_pending_confirm_exactly_once() {
        let (mut app, rx) = drain_with(0, vec![item(7, FELSTRIKER, 1)], &[1], "ConfirmLootSlot(1)");
        assert!(
            matches!(
                sent(&rx)[..],
                [ClientCommand::AutostoreLootItem { slot: 7 }]
            ),
            "the accept sends the row's wire slot, once"
        );
        assert_eq!(pending_confirm(&app), None, "and the stash clears");

        // A second accept over the same dialog: nothing left to complete.
        run_lua(app.world_mut(), "ConfirmLootSlot(1)");
        assert!(sent(&rx).is_empty(), "a second accept sends nothing");
    }

    /// `0x589300` compares the slot with the pending one, `[0xacee24]`, first.
    #[test]
    fn confirm_loot_slot_on_an_ordinary_row_sends_nothing() {
        let (_app, rx) = drain_with(
            0,
            vec![item(0, TOUGH_JERKY, 1), item(1, FLURRY_AXE, 1)],
            &[],
            "ConfirmLootSlot(1) ConfirmLootSlot(2)",
        );
        assert!(
            sent(&rx).is_empty(),
            "no confirm is pending, so neither call reaches the wire"
        );
    }

    /// `LootSlot` on an ordinary row is the take: one autostore of its wire slot.
    #[test]
    fn loot_slot_takes_an_ordinary_row() {
        let (_app, rx) = drain_with(
            0,
            vec![item(0, TOUGH_JERKY, 1), item(1, FLURRY_AXE, 1)],
            &[2, 1],
            "",
        );
        assert!(matches!(
            sent(&rx)[..],
            [
                ClientCommand::AutostoreLootItem { slot: 1 },
                ClientCommand::AutostoreLootItem { slot: 0 }
            ]
        ));
    }

    /// `LootSlot` on the coin row takes the money.
    #[test]
    fn loot_slot_on_the_coin_row_takes_the_money() {
        let (_app, rx) = drain_with(120, vec![item(0, TOUGH_JERKY, 1)], &[1], "");
        assert!(matches!(sent(&rx)[..], [ClientCommand::LootMoney]));
    }

    /// The slot-type branches of `0x589140`: a roll in progress and a locked row refuse with their
    /// red errors and send nothing.
    #[test]
    fn a_rolling_or_locked_row_refuses_with_its_error() {
        for (kind, key) in [
            (slot_type::ROLL_ONGOING, "ERR_LOOT_ROLL_PENDING"),
            (slot_type::LOCKED, "ERR_LOOT_CANT_LOOT_THAT"),
        ] {
            let row = LootItem {
                slot_type: kind,
                ..item(3, TOUGH_JERKY, 1)
            };
            let _ = crate::ui_action::take_lines_in_turn();
            let (_app, rx) = drain_with(0, vec![row], &[1], "");
            assert!(sent(&rx).is_empty(), "slot type {kind} sends nothing");
            let errors: Vec<_> = crate::ui_action::take_lines_in_turn()
                .into_iter()
                .map(|l| l.key)
                .collect();
            assert_eq!(
                errors,
                vec![key],
                "slot type {kind}: shown in the take's call"
            );
        }
    }

    #[test]
    fn the_bind_gate_needs_every_conjunct() {
        for (entry, kind, why) in [
            (FLURRY_AXE, 0, "bind-on-equip is not bind-on-pickup"),
            (WHITE_BOP, 0, "quality 1 is below the floor of 2"),
            (TOUGH_JERKY, 0, "neither"),
            (MULTI_DROP_BOP, 0, "a multi-drop item costs nobody the drop"),
            (
                FELSTRIKER,
                slot_type::OWNER,
                "an OWNER slot skips the binding checks",
            ),
        ] {
            let row = LootItem {
                slot_type: kind,
                ..item(3, entry, 1)
            };
            let (app, rx) = drain_with(0, vec![row], &[1], "");
            assert!(
                matches!(
                    sent(&rx)[..],
                    [ClientCommand::AutostoreLootItem { slot: 3 }]
                ),
                "entry {entry} should be taken outright: {why}"
            );
            assert_eq!(
                pending_confirm(&app),
                None,
                "entry {entry}: no confirm ({why})"
            );
        }
    }

    /// The display row on both halves (the deviation on `LootState::pending_bind_confirm`).
    #[test]
    fn the_coin_row_does_not_shift_the_confirm_out_from_under_itself() {
        let (mut app, rx) = drain_with(
            120, // gold, so display row 1 is the coin and the item is row 2
            vec![item(4, FELSTRIKER, 1)],
            &[2],
            "",
        );
        assert_eq!(
            pending_confirm(&app),
            Some(2),
            "the DISPLAY row, coin included"
        );
        assert_eq!(confirms(&mut app), vec![2]);
        assert!(sent(&rx).is_empty());

        // The accept, with the same number, reaches the right wire slot.
        run_lua(app.world_mut(), "ConfirmLootSlot(2)");
        assert!(matches!(
            sent(&rx)[..],
            [ClientCommand::AutostoreLootItem { slot: 4 }]
        ));
    }

    /// `0x589300`: the continuation returns on an emptied record.
    #[test]
    fn an_accept_for_a_row_that_was_taken_away_sends_nothing() {
        let (mut app, rx) = drain_with(0, vec![item(2, FELSTRIKER, 1)], &[1], "");
        assert_eq!(pending_confirm(&app), Some(1));

        window_step(app.world_mut(), |loot, step| {
            loot.remove_slot(2, true, step.waits, step.sink);
            None
        });
        run_lua(app.world_mut(), "ConfirmLootSlot(1)");

        assert!(
            sent(&rx)
                .iter()
                .all(|c| !matches!(c, ClientCommand::AutostoreLootItem { .. })),
            "nothing is autostored for a row that is gone"
        );
    }

    /// The response copier resets the stash.
    #[test]
    fn a_pending_confirm_does_not_survive_the_window() {
        let (mut app, _rx) = drain_with(0, vec![item(0, FELSTRIKER, 1)], &[1], "");
        assert_eq!(pending_confirm(&app), Some(1));

        window_step(app.world_mut(), |loot, step| {
            close_interaction(loot, step.latch, step.net, false, step.sink)
        });
        assert_eq!(pending_confirm(&app), None, "the close drops it");

        open_window(
            app.world_mut(),
            0x43,
            loot_type::CORPSE,
            0,
            vec![item(0, TOUGH_JERKY, 1)],
        );
        assert_eq!(
            pending_confirm(&app),
            None,
            "and a fresh window opens with none"
        );
    }

    /// The sweep's one-shot confirm (`0x589062`), over two BoP epics and a white, then the
    /// sweep's `LOOT_OPENED 1` (`0x5890d3`).
    #[test]
    fn the_auto_loot_sweep_raises_exactly_one_bind_confirm() {
        let mut b = Bench::new();
        for (entry, name, quality, bonding) in [
            (FELSTRIKER, "Felstriker", 4, 1),
            (SECOND_BOP, "Brutality Blade", 4, 1),
            (TOUGH_JERKY, "Tough Jerky", 1, 0),
        ] {
            b.items.insert_template(
                entry,
                Some(ItemInfo {
                    quality,
                    bonding,
                    ..crate::items::test_template(name)
                }),
            );
        }
        let mut loot = LootState {
            auto_loot: true, // armed at the interact
            ..LootState::default()
        };
        // Wire slots 5/6/7 in display order: BoP epic, BoP epic, white.
        loot.open(
            0x42,
            loot_type::CORPSE,
            0,
            vec![
                item(5, FELSTRIKER, 1),
                item(6, SECOND_BOP, 1),
                item(7, TOUGH_JERKY, 1),
            ],
            &mut b.step(),
        );
        assert_eq!(
            b.fired(),
            ["LOOT_BIND_CONFIRM 1", "LOOT_OPENED 1"],
            "one dialog, for the FIRST bind-on-pickup row only"
        );
        assert_eq!(loot.pending_bind_confirm, Some(1));
        assert!(
            matches!(b.sent()[..], [ClientCommand::AutostoreLootItem { slot: 7 }]),
            "the white is swept; neither blue is sent"
        );
    }

    fn item(slot: u8, entry: u32, count: u32) -> LootItem {
        LootItem {
            slot,
            item_id: entry,
            count,
            display_info_id: 1000 + entry,
            random_property_id: 0,
            random_suffix: 0,
            slot_type: 0,
        }
    }

    /// The same row stamped `MASTER`, as vmangos sends every row under master loot.
    fn master_item(slot: u8, entry: u32, count: u32) -> LootItem {
        LootItem {
            slot_type: slot_type::MASTER,
            ..item(slot, entry, count)
        }
    }

    /// The list is the client's global (`0xca0550`): `SMSG_LOOT_MASTER_LIST` replaces it whole
    /// (`0x6fa690`) and the getter reads it with or without a window (`0x6fa770`); no open or close
    /// touches it.
    #[test]
    fn the_candidate_list_is_the_last_one_sent_window_or_none() {
        let mut loot = LootState::default();
        // A default GroupState is a party (`group_type` 0): the flat placement path.
        let party = GroupState::default();

        loot.set_master_candidates(vec![0xA, 0xB, 0xC]);
        assert_eq!(
            loot.master_candidate(1, &party),
            Some(0xA),
            "no window needed"
        );

        open_bare(
            &mut loot,
            0x42,
            loot_type::CORPSE,
            0,
            vec![master_item(0, 117, 1)],
        );
        assert_eq!(
            loot.master_candidate(1, &party),
            Some(0xA),
            "the set keeps it"
        );
        assert_eq!(loot.master_candidate(3, &party), Some(0xC));
        assert_eq!(loot.master_candidate(4, &party), None, "past the end");
        assert_eq!(
            loot.master_candidate(0, &party),
            None,
            "0 is not a Lua index"
        );

        // A new list replaces it in place.
        loot.set_master_candidates(vec![0xA, 0xB]);
        assert_eq!(loot.master_candidate(2, &party), Some(0xB));
        assert_eq!(loot.master_candidate(3, &party), None, "the list shrank");

        loot.close_table(&mut Fired::default());
        open_bare(&mut loot, 0x43, loot_type::CORPSE, 0, vec![item(0, 117, 1)]);
        assert_eq!(
            loot.master_candidate(2, &party),
            Some(0xB),
            "a close and a new window leave it"
        );
        loot.clear_session();
        assert_eq!(
            loot.master_candidate(1, &party),
            None,
            "a disconnect drops it"
        );
    }

    /// The reference's raid placement: each guid in the first free slot of its subgroup's block.
    #[test]
    fn raid_candidates_file_into_their_own_subgroup_block() {
        let member = |guid: u64, name: &str, subgroup: u8| GroupMemberEntry {
            name: name.into(),
            guid,
            status: 0,
            flags: subgroup,
            roles: 0,
        };
        let mut raid = GroupState {
            group_type: GROUPTYPE_RAID,
            own_flags: 0, // we are in subgroup 0
            ..GroupState::default()
        };
        raid.members = vec![
            member(0xB, "Cairne", 0),
            member(0xC, "Vol", 2),
            member(0xD, "Sylvanas", 2),
        ];

        let mut loot = LootState::default();
        // The wire order interleaves the subgroups; placement ignores it.
        loot.set_master_candidates(vec![0xC, 0xA, 0xD, 0xB]);
        open_bare(
            &mut loot,
            0x42,
            loot_type::CORPSE,
            0,
            vec![master_item(0, 117, 1)],
        );

        // Subgroup 0 fills slots 1-2 (us at 0xA, then Cairne); subgroup 2 fills 11-12, in the
        // order the wire listed them.
        assert_eq!(loot.master_candidate(1, &raid), Some(0xA), "us, group 1");
        assert_eq!(
            loot.master_candidate(2, &raid),
            Some(0xB),
            "Cairne, group 1"
        );
        for empty in [3, 4, 5, 6, 7, 8, 9, 10] {
            assert_eq!(
                loot.master_candidate(empty, &raid),
                None,
                "slot {empty} belongs to an empty group"
            );
        }
        assert_eq!(loot.master_candidate(11, &raid), Some(0xC), "Vol, group 3");
        assert_eq!(
            loot.master_candidate(12, &raid),
            Some(0xD),
            "Sylvanas, group 3"
        );
        assert_eq!(loot.master_candidate(13, &raid), None);

        // In a party the same list is flat, in wire order.
        let party = GroupState {
            members: raid.members.clone(),
            ..GroupState::default()
        };
        assert_eq!(loot.master_candidate(1, &party), Some(0xC));
        assert_eq!(loot.master_candidate(4, &party), Some(0xB));
        assert_eq!(loot.master_candidate(5, &party), None);
    }

    #[test]
    fn a_master_row_reports_its_slot_type() {
        let mut loot = LootState::default();
        open_bare(
            &mut loot,
            0x42,
            loot_type::CORPSE,
            0,
            vec![master_item(0, 117, 1), item(1, 2589, 5)],
        );
        assert!(matches!(
            loot.action_at(1),
            Some(LootAction::Item {
                wire_slot: 0,
                slot_type: slot_type::MASTER,
                ..
            })
        ));
        assert!(matches!(
            loot.action_at(2),
            Some(LootAction::Item {
                wire_slot: 1,
                slot_type: slot_type::ALLOW_LOOT,
                ..
            })
        ));
    }

    /// Our own push, shown in chat, with no random property.
    fn push(entry: u32, count: u32, from_npc: bool, created: bool) -> ItemPushResult {
        ItemPushResult {
            player_guid: 0x1,
            from_npc,
            created,
            show_in_chat: true,
            bag_slot: 0xFF,
            item_slot: 0,
            item_entry: entry,
            suffix_factor: 0,
            random_property_id: 0,
            count,
            total_count: count,
        }
    }

    #[test]
    fn action_maps_coin_first_then_items_by_wire_slot() {
        let mut loot = LootState::default();
        assert!(loot.source.is_none());
        open_bare(
            &mut loot,
            0x42,
            loot_type::CORPSE,
            1234,
            vec![item(0, 117, 1), item(1, 2589, 5)],
        );
        assert!(loot.source.is_some());
        // Row 1 is the coin, rows 2 and 3 the items; the helper's display id is `1000 + entry`.
        assert!(matches!(loot.action_at(1), Some(LootAction::Money)));
        assert!(matches!(
            loot.action_at(2),
            Some(LootAction::Item {
                wire_slot: 0,
                display_id: 1117,
                ..
            })
        ));
        assert!(matches!(
            loot.action_at(3),
            Some(LootAction::Item {
                wire_slot: 1,
                display_id: 3589,
                ..
            })
        ));
        assert!(loot.action_at(4).is_none());
        assert!(loot.action_at(0).is_none());
    }

    #[test]
    fn action_maps_items_directly_when_no_coin() {
        let mut loot = LootState::default();
        open_bare(
            &mut loot,
            0x42,
            loot_type::CORPSE,
            0,
            vec![item(3, 117, 1), item(7, 2589, 5)],
        );
        assert!(matches!(
            loot.action_at(1),
            Some(LootAction::Item { wire_slot: 3, .. })
        ));
        assert!(matches!(
            loot.action_at(2),
            Some(LootAction::Item { wire_slot: 7, .. })
        ));
        assert!(loot.action_at(3).is_none());
    }

    /// `0x589920`: the row becomes a gap at its fixed place and signals with its display slot.
    #[test]
    fn remove_slot_leaves_a_gap_at_the_fixed_position() {
        let mut b = Bench::new();
        let mut loot = LootState::default();
        loot.open(
            0x42,
            loot_type::CORPSE,
            0,
            vec![item(0, 117, 1), item(1, 2589, 5), item(2, 4306, 2)],
            &mut b.step(),
        );
        b.fired();
        assert!(!loot.remove_slot(1, true, &mut b.waits, &mut b.fired));
        assert_eq!(b.fired(), ["LOOT_SLOT_CLEARED 2"]);
        assert!(matches!(
            loot.action_at(1),
            Some(LootAction::Item { wire_slot: 0, .. })
        ));
        assert!(loot.action_at(2).is_none(), "the looted row is a gap");
        assert!(matches!(
            loot.action_at(3),
            Some(LootAction::Item { wire_slot: 2, .. })
        ));
        assert!(!loot.remove_slot(1, true, &mut b.waits, &mut b.fired));
        assert!(b.fired().is_empty(), "a slot no row holds clears nothing");
    }

    /// `0x589100`: the coin slot becomes a gap and signals slot 1.
    #[test]
    fn clear_money_leaves_the_coin_slot_as_a_gap() {
        let mut b = Bench::new();
        let mut loot = LootState::default();
        loot.open(
            0x42,
            loot_type::CORPSE,
            500,
            vec![item(0, 117, 1)],
            &mut b.step(),
        );
        b.fired();
        assert!(loot.has_coin());
        assert!(matches!(loot.action_at(1), Some(LootAction::Money)));
        assert!(!loot.clear_money(true, &mut b.fired));
        assert_eq!(b.fired(), ["LOOT_SLOT_CLEARED 1"]);
        assert!(!loot.has_coin());
        assert!(loot.action_at(1).is_none(), "the looted coin slot is a gap");
        assert!(matches!(
            loot.action_at(2),
            Some(LootAction::Item { wire_slot: 0, .. })
        ));
        assert!(!loot.clear_money(true, &mut b.fired));
        assert!(b.fired().is_empty(), "taken coins clear once");
    }

    /// The clear that leaves nothing owes `CloseLoot(1, 0, 0)` (`0x589a19`, `0x589134`), the
    /// last row or the coins alike; an empty window at open stays up (`LOOTWINDOWOPENEMPTY`).
    #[test]
    fn the_clear_that_empties_the_window_owes_its_close() {
        let mut b = Bench::new();
        let mut loot = LootState::default();
        loot.open(
            0x42,
            loot_type::CORPSE,
            500,
            vec![item(0, 117, 1), item(1, 2589, 5)],
            &mut b.step(),
        );
        assert!(
            !loot.remove_slot(0, true, &mut b.waits, &mut b.fired),
            "items + coin remain"
        );
        assert!(!loot.clear_money(true, &mut b.fired), "an item remains");
        assert!(
            loot.remove_slot(1, true, &mut b.waits, &mut b.fired),
            "the last row"
        );

        loot.open(0x43, loot_type::CORPSE, 500, vec![], &mut b.step());
        assert!(
            loot.clear_money(true, &mut b.fired),
            "coin-only loot empties"
        );

        b.fired();
        loot.open(0x44, loot_type::CORPSE, 0, vec![], &mut b.step());
        assert_eq!(
            b.fired(),
            ["LOOT_CLOSED", "LOOT_OPENED 0"],
            "an empty window opens"
        );
        assert_eq!(loot.source(), Some(0x44));
    }

    #[test]
    fn latch_clears_guid_matched_only() {
        // Loot B requested while A was open: A's release response keeps B's latch.
        let mut latch = LootLatch(Some(0xB));
        latch.clear_for(0xA);
        assert_eq!(latch.0, Some(0xB), "a stale release leaves the new latch");
        latch.clear_for(0xB);
        assert_eq!(latch.0, None, "the matching release drops it");
    }

    /// A slot redrawn after removals fills the first zeroed row and nothing else: a looted row
    /// that carried the same wire slot stays a gap (`0x589980`-`0x5899aa`), and the refill takes
    /// the slot's zeroed type (`0x6d58ba`), not a looted row's.
    #[test]
    fn a_redrawn_slot_fills_one_gap_and_restores_no_looted_row() {
        let mut b = Bench::new();
        b.template(117, "Tough Jerky");
        b.template(118, "Linen Cloth");
        let mut loot = LootState::default();
        let rolling = LootItem {
            slot_type: slot_type::ROLL_ONGOING,
            ..item(1, 118, 1)
        };
        loot.open(
            0x42,
            loot_type::CORPSE,
            0,
            vec![item(0, 117, 1), rolling],
            &mut b.step(),
        );
        assert!(!loot.remove_slot(0, true, &mut b.waits, &mut b.fired));
        assert!(loot.remove_slot(1, true, &mut b.waits, &mut b.fired));
        b.fired();
        let msg = benilla_protocol::messages::LootSlotChanged {
            looted_target: 0x42,
            slot: 1,
            item_id: 117,
            display_info_id: 7,
            random_suffix: 0,
            random_property_id: 0,
            count: 3,
        };
        loot.slot_changed(&msg, &mut b.step());
        assert_eq!(b.fired(), ["LOOT_SLOT_CHANGED 1"], "the first gap, row 1");
        assert!(matches!(
            loot.action_at(1),
            Some(LootAction::Item {
                wire_slot: 1,
                slot_type: slot_type::ALLOW_LOOT,
                ..
            })
        ));
        assert!(loot.action_at(2).is_none(), "the looted row stays a gap");
    }

    /// A removal leaves a window whose object is not held with no loot (`0x5899e3`, `0x588eb0`),
    /// so it owes the close with rows left; the table holds at most 18 rows (`0x589a30`).
    #[test]
    fn an_unheld_window_owes_its_close_and_the_table_holds_eighteen_rows() {
        let mut b = Bench::new();
        let mut loot = LootState::default();
        let rows: Vec<LootItem> = (0..20).map(|s| item(s, 117, 1)).collect();
        loot.open(0x42, loot_type::CORPSE, 0, rows, &mut b.step());
        assert_eq!(loot.items.len(), LOOT_WINDOW_ROWS);
        assert!(loot.remove_slot(0, false, &mut b.waits, &mut b.fired));
        assert!(!loot.remove_slot(1, true, &mut b.waits, &mut b.fired));
    }

    /// The same item's window set again unlocks it on the way (`0x589736`), whatever the new
    /// object is, before `LOOT_CLOSED`.
    #[test]
    fn a_window_set_again_on_its_item_unlocks_it() {
        const LOCKBOX: u64 = 0x4000_0000_0000_0007;
        let mut b = Bench::new();
        let mut loot = LootState::default();
        loot.open(LOCKBOX, loot_type::CORPSE, 0, vec![], &mut b.step());
        b.fired();
        loot.open(LOCKBOX, loot_type::CORPSE, 0, vec![], &mut b.step());
        assert_eq!(
            b.fired(),
            [
                "<unlock 0x4000000000000007>",
                "LOOT_CLOSED",
                "LOOT_OPENED 0"
            ]
        );
    }

    /// The setter sweeps whenever a loot target is latched (`0x6cde75`): with no window the sweep
    /// spends the flag, sends nothing and closes nothing (`0x5890a7`, `0x5890e6`).
    #[test]
    fn arming_auto_loot_with_no_window_spends_the_flag() {
        let mut b = Bench::new();
        b.latch.0 = Some(CORPSE);
        let mut loot = LootState::default();
        assert_eq!(
            loot.set_auto_loot(true, &mut b.latch, &b.net, &mut b.fired),
            None
        );
        assert!(!loot.auto_loot, "spent");
        assert!(b.fired().is_empty());
        assert!(b.sent().is_empty());
        b.latch.0 = None;
        loot.set_auto_loot(true, &mut b.latch, &b.net, &mut b.fired);
        assert!(loot.auto_loot, "no latch: the flag waits");
    }

    /// `CloseLoot`'s sweep of a disenchant window returns while a record is pending (`0x588f99`):
    /// nothing is taken and the flag stays.
    #[test]
    fn a_close_with_a_record_pending_sweeps_nothing() {
        let mut b = Bench::new();
        b.latch.0 = Some(0x42);
        let mut loot = LootState {
            auto_loot: true,
            ..LootState::default()
        };
        loot.open(
            0x42,
            loot_type::DISENCHANTING,
            0,
            vec![item(0, 117, 1)],
            &mut b.step(),
        );
        b.sent();
        close_interaction(&mut loot, &mut b.latch, &b.net, true, &mut b.fired);
        assert!(
            matches!(b.sent()[..], [ClientCommand::LootRelease { guid: 0x42 }]),
            "the release alone"
        );
        assert!(loot.auto_loot, "the flag stays");
    }

    /// `LootSlot` reads the row's record with no callback (`0x589252`): with none cached it takes
    /// nothing and asks nothing (`0x589259`).
    #[test]
    fn a_take_with_no_record_cached_does_nothing() {
        let (_app, rx) = drain_with(0, vec![item(0, 4242, 1)], &[1], "");
        assert!(
            matches!(
                sent(&rx)[..],
                [ClientCommand::ItemQuery { entry: 4242, .. }]
            ),
            "the set's own ask (`0x589899`) alone: no take, and the take asks nothing"
        );
    }

    /// `IsFishingLoot` reads the last window's loot type (`0x5888e0`, `[0xbfa68c]`), which the set
    /// with no object leaves (`0x5897b0`).
    #[test]
    fn the_loot_type_outlives_its_window() {
        let mut b = Bench::new();
        let mut loot = LootState::default();
        loot.open(
            0x42,
            loot_type::FISHING,
            0,
            vec![item(0, 117, 1)],
            &mut b.step(),
        );
        assert!(loot.fishing());
        loot.close_table(&mut b.fired);
        assert!(loot.fishing(), "the close leaves the type");
        loot.open(
            0x42,
            loot_type::CORPSE,
            0,
            vec![item(0, 117, 1)],
            &mut b.step(),
        );
        assert!(!loot.fishing());
        loot.clear_session();
        assert!(!loot.fishing(), "a disconnect forgets it");
    }

    /// A push waiting on its record outlives the window; a disconnect drops it.
    #[test]
    fn a_waiting_push_outlives_the_window() {
        let mut loot = LootState::default();
        open_bare(&mut loot, 0x42, loot_type::CORPSE, 0, vec![item(0, 117, 1)]);
        loot.receives.push(push(117, 1, false, false));
        loot.close_table(&mut Fired::default());
        assert!(loot.source.is_none());
        assert_eq!(loot.receives.len(), 1, "a receive line outlives the window");
        loot.clear_session();
        assert!(loot.receives.is_empty(), "disconnect drops receive lines");
    }

    /// The reference joins the suffix by `ITEM_SUFFIX_TEMPLATE`, for the row and tooltip.
    #[test]
    fn a_rolled_drop_reads_its_suffix_in_the_row_the_link_and_the_lines() {
        use benilla_formats::{RandomProperty, RandomPropertyCatalog};
        // "of the Monkey" (row 584 of the shipped table): Agility +7, Stamina +7.
        let props = crate::items::RandomProperties(RandomPropertyCatalog::from_rows(
            [(
                584,
                RandomProperty {
                    suffix: "of the Monkey".into(),
                    enchants: [74, 71, 0, 0, 0],
                },
            )]
            .into_iter()
            .collect(),
        ));
        let enchants = crate::items::Enchants(
            benilla_formats::EnchantCatalog::from_rows(
                std::collections::HashMap::new(),
                [
                    (74, "Agility +7".to_string()),
                    (71, "Stamina +7".to_string()),
                ]
                .into_iter()
                .collect(),
                [(74, 0), (71, 0)].into_iter().collect(),
            ),
            Default::default(),
        );
        let rolls = RollCatalogs {
            props: Some(&props),
            enchants: Some(&enchants),
        };
        assert_eq!(rolls.name("Bloodrazor", 584), "Bloodrazor of the Monkey");
        assert_eq!(rolls.name("Bloodrazor", 0), "Bloodrazor");
        // The roll's lines land in slots 7..11, the property band the renderer draws white.
        let lines = rolls.lines(584);
        assert_eq!(
            lines
                .iter()
                .map(|l| (l.slot, l.name.as_str()))
                .collect::<Vec<_>>(),
            vec![(7, "Agility +7"), (8, "Stamina +7")],
        );
        assert!(rolls.lines(0).is_empty(), "no roll, no lines");
    }

    /// [`push_container`] against the selector, arm by arm.
    #[test]
    fn push_container_maps_the_wire_destination_onto_a_bag_bar_button() {
        // An equipped bag: wire 19..22 give `bag + 1`, 20..23, the ids its buttons carry.
        assert_eq!(push_container(19, 0), 20);
        assert_eq!(push_container(22, 5), 23);
        // `bag == 255` in the keyring window, 86..=117.
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 86), KEYRING_CONTAINER);
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 100), KEYRING_CONTAINER);
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 117), KEYRING_CONTAINER);
        // Anywhere else is the backpack: its slots 23..38, and 85 and 118 just outside the keyring.
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 23), 0);
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 85), 0);
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 118), 0);
        // A stack merge reports no slot (`0xFFFF_FFFF`): the backpack.
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, u32::MAX), 0);
        // A bank bag (wire 67..73) lands on an id no bag-bar button carries.
        assert!(!(20..=23).contains(&push_container(67, 0)));
    }

    /// The reference's six-step ladder, both sides of every boundary.
    #[test]
    fn coin_icon_walks_the_references_six_step_ladder() {
        let icon = |n: u32| coin_icon(n).rsplit('\\').next().unwrap().to_string();
        assert_eq!(icon(0), "INV_Misc_Coin_05");
        assert_eq!(icon(9), "INV_Misc_Coin_05");
        assert_eq!(icon(10), "INV_Misc_Coin_06");
        assert_eq!(icon(99), "INV_Misc_Coin_06");
        assert_eq!(icon(100), "INV_Misc_Coin_03");
        assert_eq!(icon(999), "INV_Misc_Coin_03");
        assert_eq!(icon(1_000), "INV_Misc_Coin_04");
        assert_eq!(icon(9_999), "INV_Misc_Coin_04");
        assert_eq!(icon(10_000), "INV_Misc_Coin_01");
        assert_eq!(icon(99_999), "INV_Misc_Coin_01");
        assert_eq!(icon(100_000), "INV_Misc_Coin_02");
        assert_eq!(icon(u32::MAX), "INV_Misc_Coin_02");
    }

    #[test]
    fn coin_row_carries_its_coins_and_the_ladders_icon() {
        let items = Items::default();
        let mut loot = LootState::default();
        // A pure-copper drop: its coins for the call's text, icon the copper coin pile (_05).
        open_bare(&mut loot, 0x42, loot_type::CORPSE, 4, vec![]);
        let rows = window_rows(&loot, &items, None, RollCatalogs::NONE);
        assert_eq!(
            rows.len(),
            1 + LOOT_WINDOW_ROWS,
            "the coin slot, then the table"
        );
        let coin = rows[0].as_ref().expect("coin row present");
        assert!(coin.is_coin);
        assert_eq!((coin.name.as_deref(), coin.coins), (None, 4));
        assert_eq!(coin.texture.as_deref(), Some(coin_icon(4)));
        assert!(rows[1..].iter().all(Option::is_none), "an empty table");
    }

    /// The rows a verb reads at the call: the coin slot first, an item's record read with no
    /// callback, and a looted slot a gap in place.
    #[test]
    fn the_rows_read_at_the_call_keep_their_places() {
        let items = Items::default();
        let mut b = Bench::new();
        let mut loot = LootState::default();
        loot.open(
            0x42,
            loot_type::CORPSE,
            12_345,
            vec![item(0, 117, 3)],
            &mut b.step(),
        );
        b.sent();
        let rows = window_rows(&loot, &items, None, RollCatalogs::NONE);
        let coin = rows[0].as_ref().expect("coin row present");
        assert!(coin.is_coin);
        assert_eq!((coin.name.as_deref(), coin.coins), (None, 12_345));
        assert_eq!(coin.texture.as_deref(), Some(coin_icon(12_345)));
        // The item's name is nil while its record is in flight; its quantity is present.
        let row = rows[1].as_ref().expect("item row present");
        assert!(!row.is_coin);
        assert!(row.name.is_none());
        assert_eq!(row.quantity, 3);
        assert!(b.sent().is_empty(), "the read asks nothing");

        // Looting the coin turns row 1 into a gap; the item keeps its position.
        loot.clear_money(true, &mut b.fired);
        let rows = window_rows(&loot, &items, None, RollCatalogs::NONE);
        assert!(rows[0].is_none(), "the looted coin slot is a gap");
        assert!(rows[1].is_some(), "the item stays at position 2");

        assert!(
            loot.remove_slot(0, true, &mut b.waits, &mut b.fired),
            "nothing lootable left"
        );
        let rows = window_rows(&loot, &items, None, RollCatalogs::NONE);
        assert!(rows.iter().all(Option::is_none));
    }

    #[test]
    fn page_math_two_pages_of_three_and_two() {
        // Four rows a page, three when there are more than four (`LootFrame.lua:70-73`, `:112`).
        let num_items = 5u32;
        let per_page = if num_items > 4 { 3 } else { 4 };
        let pages = num_items.div_ceil(per_page);
        assert_eq!(per_page, 3);
        assert_eq!(pages, 2);
        let page1: Vec<u32> = (1..=per_page).filter(|&i| i <= num_items).collect();
        let page2: Vec<u32> = (per_page + 1..=2 * per_page)
            .filter(|&i| i <= num_items)
            .collect();
        assert_eq!(page1, vec![1, 2, 3]);
        assert_eq!(page2, vec![4, 5]);
        // Four items: one page of four.
        assert_eq!(if 4u32 > 4 { 3 } else { 4 }, 4);
    }

    /// A creature corpse and a live creature, both streamed.
    const CORPSE: u64 = 0xF130_0000_0000_0042;
    const LIVE: u64 = 0xF130_0000_0000_0043;

    /// `drain_loot` scheduled, with [`CORPSE`] streamed at health 0 and [`LIVE`] at 100.
    fn close_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_message::<crate::sound::LootPickupSound>()
            .add_message::<crate::target::DeselectGuid>()
            .init_resource::<LootState>()
            .init_resource::<LootLatch>()
            .init_resource::<LootMoveStart>()
            .init_resource::<GroupState>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::ui_script::live::UiEvents>()
            .init_resource::<crate::pending_item_ops::PendingItemOps>()
            .insert_resource(NetCommands(tx))
            .add_systems(Update, drain_loot);
        // The rows' records are in, so a set asks nothing.
        for (entry, name) in [(117, "Tough Jerky"), (118, "Linen Cloth")] {
            app.world_mut()
                .resource_mut::<Items>()
                .insert_template(entry, Some(crate::items::test_template(name)));
        }
        for (guid, health) in [(CORPSE, 0), (LIVE, 100)] {
            let e = app
                .world_mut()
                .spawn((
                    crate::net::NetEntity {
                        kind: EntityKind::Unit,
                        display_id: None,
                        scale: 1.0,
                    },
                    crate::net::ObjectStore(ObjectFields::from_pairs(&[(F_UNIT_HEALTH, health)])),
                ))
                .id();
            app.world_mut()
                .resource_mut::<crate::net::GuidIndex>()
                .0
                .insert(guid, e);
        }
        (app, rx)
    }

    /// The deselects asked for since the last call.
    fn deselects(app: &mut App) -> Vec<u64> {
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<crate::target::DeselectGuid>>()
            .drain()
            .map(|d| d.0)
            .collect()
    }

    /// Opens a window as an admitted response does: the rows and the latch.
    fn open_loot(app: &mut App, guid: u64, kind: u8, gold: u32, rows: Vec<LootItem>) {
        open_window(app.world_mut(), guid, kind, gold, rows);
        app.world_mut().resource_mut::<LootLatch>().0 = Some(guid);
    }

    /// The server's removal of wire slot `slot`, as its handler runs it: the clear, then the
    /// emptied window's close in turn.
    fn remove(app: &mut App, slot: u8) {
        window_step(app.world_mut(), |loot, step| {
            if loot.remove_slot(slot, true, step.waits, step.sink) {
                close_interaction(loot, step.latch, step.net, true, step.sink)
            } else {
                None
            }
        });
    }

    /// Every close reaches `CloseLoot`'s dead-unit deselect (`0x52378c`-`0x5237ae`): `CloseLoot()`
    /// from the Lua, the empty-window close after the last row (`0x589a19`) or the coin
    /// (`0x589134`), each deselecting a dead unit and only a dead unit.
    #[test]
    fn every_loot_close_deselects_a_dead_unit() {
        let (mut app, rx) = close_app();
        let released = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<u64> {
            rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::LootRelease { guid } => Some(guid),
                    _ => None,
                })
                .collect()
        };
        app.insert_non_send_resource(UiScript::new().unwrap());
        let close_loot = |app: &mut App| {
            run_lua(app.world_mut(), "CloseLoot()");
            app.update();
        };

        // `CloseLoot`, as the close button, Escape or any other hide reaches it.
        open_loot(
            &mut app,
            CORPSE,
            loot_type::CORPSE,
            0,
            vec![item(0, 117, 1)],
        );
        close_loot(&mut app);
        assert_eq!(released(&rx), [CORPSE]);
        assert_eq!(app.world().resource::<LootLatch>().0, None);
        assert_eq!(
            deselects(&mut app),
            [CORPSE],
            "CloseLoot deselects the corpse"
        );

        // The last row taken.
        open_loot(
            &mut app,
            CORPSE,
            loot_type::CORPSE,
            0,
            vec![item(0, 117, 1)],
        );
        remove(&mut app, 0);
        app.update();
        assert_eq!(released(&rx), [CORPSE]);
        assert_eq!(
            deselects(&mut app),
            [CORPSE],
            "the last row's close deselects"
        );

        // The coin taken from a coin-only window.
        open_loot(&mut app, CORPSE, loot_type::CORPSE, 250, vec![]);
        window_step(app.world_mut(), |loot, step| {
            if loot.clear_money(true, step.sink) {
                close_interaction(loot, step.latch, step.net, true, step.sink)
            } else {
                None
            }
        });
        app.update();
        assert_eq!(released(&rx), [CORPSE]);
        assert_eq!(deselects(&mut app), [CORPSE], "the coin's close deselects");

        // A live pickpocket target keeps the selection.
        open_loot(
            &mut app,
            LIVE,
            loot_type::PICKPOCKETING,
            0,
            vec![item(0, 117, 1)],
        );
        close_loot(&mut app);
        assert_eq!(released(&rx), [LIVE]);
        assert!(
            deselects(&mut app).is_empty(),
            "a live unit is not deselected"
        );

        // With nothing open, a second `CloseLoot` sends and asks nothing.
        close_loot(&mut app);
        assert!(released(&rx).is_empty());
        assert!(deselects(&mut app).is_empty());
    }

    /// `CloseLoot(1, 1, 0)` on a move start, with no server help and no VM.
    #[test]
    fn a_movement_start_closes_and_releases_the_open_loot() {
        const BOBBER: u64 = 0xF110_0000_0000_0011;
        const LOCKBOX: u64 = 0x4000_0000_0000_0007;

        // No UiScript mounted: the move-start leg must not depend on the VM.
        let (mut app, rx) = close_app();
        let open = |app: &mut App, guid: u64, kind: u8, rows: Vec<LootItem>| {
            open_loot(app, guid, kind, 0, rows);
        };
        let step = |app: &mut App| {
            app.world_mut().resource_mut::<LootMoveStart>().0 = true;
            app.update();
        };

        open(&mut app, CORPSE, loot_type::CORPSE, vec![item(0, 117, 1)]);
        app.update();
        assert_eq!(app.world().resource::<LootState>().source(), Some(CORPSE));
        assert!(
            rx.try_recv().is_err(),
            "nothing goes out without a move start"
        );

        step(&mut app);
        assert_eq!(app.world().resource::<LootState>().source(), None);
        assert_eq!(app.world().resource::<LootLatch>().0, None);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == CORPSE),
            "the move-start close owes the wire the release"
        );
        assert!(rx.try_recv().is_err(), "and sends it once");
        assert_eq!(deselects(&mut app), vec![CORPSE]);

        // The deselect reads the health (`0x5237a4`), so a skinned corpse is deselected ...
        open(&mut app, CORPSE, loot_type::SKINNING, vec![item(0, 117, 1)]);
        step(&mut app);
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == CORPSE));
        assert_eq!(
            deselects(&mut app),
            vec![CORPSE],
            "a skinned corpse loses it"
        );
        // ... and a live pickpocket target is not.
        open(
            &mut app,
            LIVE,
            loot_type::PICKPOCKETING,
            vec![item(0, 117, 1)],
        );
        step(&mut app);
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == LIVE));
        assert!(deselects(&mut app).is_empty(), "a live target keeps it");

        // A fishing bobber is a GameObject: the same path, minus the unit deselect.
        open(&mut app, BOBBER, loot_type::FISHING, vec![]);
        step(&mut app);
        assert_eq!(app.world().resource::<LootState>().source(), None);
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == BOBBER));
        assert!(deselects(&mut app).is_empty());

        // A lockbox survives movement, unreleased.
        open(&mut app, LOCKBOX, loot_type::CORPSE, vec![item(0, 117, 1)]);
        step(&mut app);
        assert_eq!(app.world().resource::<LootState>().source(), Some(LOCKBOX));
        assert!(rx.try_recv().is_err());
        window_step(app.world_mut(), |loot, step| {
            loot.close_table(step.sink);
            None
        });

        // A disenchant is an item's loot and survives; `keepItem` also skips the close's sweep
        // (`0x5236a3`), so one on anything else closes with its rows untaken.
        open(
            &mut app,
            LOCKBOX,
            loot_type::DISENCHANTING,
            vec![item(0, 117, 1)],
        );
        step(&mut app);
        assert_eq!(app.world().resource::<LootState>().source(), Some(LOCKBOX));
        assert!(rx.try_recv().is_err());
        window_step(app.world_mut(), |loot, step| {
            loot.close_table(step.sink);
            None
        });
        open(
            &mut app,
            CORPSE,
            loot_type::DISENCHANTING,
            vec![item(0, 117, 1)],
        );
        step(&mut app);
        assert_eq!(app.world().resource::<LootState>().source(), None);
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(sent[..], [ClientCommand::LootRelease { guid }] if guid == CORPSE),
            "{sent:?}"
        );
    }

    /// `CloseLoot` with `keepItem` 0 takes every row a disenchant, prospect or mill window has
    /// left (`0x523681`-`0x5236ab`), the coin while the latch holds, before the release.
    #[test]
    fn closing_a_disenchant_prospect_or_mill_window_takes_what_is_left() {
        const ITEM: u64 = 0x4000_0000_0000_0009;
        for (kind, swept) in [
            (loot_type::DISENCHANTING, true),
            (loot_type::PROSPECTING, true),
            (loot_type::MILLING, true),
            (loot_type::CORPSE, false),
            (loot_type::SKINNING, false),
        ] {
            let (mut app, rx) = close_app();
            app.insert_non_send_resource(UiScript::new().unwrap());
            open_loot(
                &mut app,
                ITEM,
                kind,
                0,
                vec![item(0, 117, 1), item(1, 118, 2)],
            );
            remove(&mut app, 0);
            run_lua(app.world_mut(), "CloseLoot()");
            app.update();
            let sent: Vec<_> = rx.try_iter().collect();
            if swept {
                assert!(
                    matches!(
                        sent[..],
                        [ClientCommand::AutostoreLootItem { slot: 1 }, ClientCommand::LootRelease { guid }]
                            if guid == ITEM
                    ),
                    "type {kind}: {sent:?}"
                );
            } else {
                assert!(
                    matches!(sent[..], [ClientCommand::LootRelease { guid }] if guid == ITEM),
                    "type {kind}: {sent:?}"
                );
            }
            assert!(
                app.world()
                    .resource::<crate::pending_item_ops::PendingItemOps>()
                    .is_empty(),
                "nothing was locked"
            );
        }
    }

    /// `CloseLoot(noUI)` shows `ERR_LOOT_NO_UI` over an open window (`0x5236be`), the stock
    /// `CloseLoot(autoLoot == 0)` when the frame failed to show (`LootFrame.lua:21`).
    #[test]
    fn close_loot_with_no_ui_shows_its_error_over_an_open_window() {
        let (mut app, rx) = close_app();
        app.insert_non_send_resource(UiScript::new().unwrap());
        let close = |app: &mut App, chunk: &str| {
            let _ = crate::ui_action::take_lines_in_turn();
            run_lua(app.world_mut(), chunk);
            app.update();
            crate::ui_action::take_lines_in_turn()
                .into_iter()
                .map(|l| l.key)
                .collect::<Vec<_>>()
        };
        open_loot(&mut app, LIVE, loot_type::CORPSE, 0, vec![item(0, 117, 1)]);
        assert!(close(&mut app, "CloseLoot(false)").is_empty());
        open_loot(&mut app, LIVE, loot_type::CORPSE, 0, vec![item(0, 117, 1)]);
        assert_eq!(close(&mut app, "CloseLoot(true)"), ["ERR_LOOT_NO_UI"]);
        assert!(
            close(&mut app, "CloseLoot(true)").is_empty(),
            "nothing open"
        );
        assert_eq!(rx.try_iter().count(), 2, "one release per open window");
    }

    /// `LootSlot` on the coin row (`0x589184`-`0x5891b8`): the money goes out and the row clears
    /// at the click; a coin-only window then closes, released.
    #[test]
    fn taking_the_coin_clears_its_row_at_the_click() {
        for (rows, closes) in [(vec![], true), (vec![item(0, 117, 1)], false)] {
            let (mut app, rx) = close_app();
            app.insert_non_send_resource(UiScript::new().unwrap());
            open_loot(&mut app, LIVE, loot_type::CORPSE, 250, rows);
            run_lua(app.world_mut(), "LootSlot(1)");
            app.update();
            let loot = app.world().resource::<LootState>();
            assert!(!loot.has_coin(), "the coin row is gone at once");
            assert_eq!(loot.source().is_none(), closes);
            let sent: Vec<_> = rx.try_iter().collect();
            assert!(
                matches!(sent.first(), Some(ClientCommand::LootMoney)),
                "{sent:?}"
            );
            assert_eq!(
                sent.iter()
                    .any(|c| matches!(c, ClientCommand::LootRelease { guid } if *guid == LIVE)),
                closes,
                "{sent:?}"
            );
        }
    }

    /// Arming the auto-loot flag (`0x6cde50`) over an open window sweeps it at once and spends the
    /// flag, `LOOT_OPENED 1` after; a window still waiting on a record sweeps when it opens
    /// (`0x588f99`, `0x5894fb`).
    #[test]
    fn arming_auto_loot_over_an_open_window_sweeps_it() {
        let mut b = Bench::new();
        b.latch.0 = Some(CORPSE);
        let mut loot = LootState::default();
        loot.open(
            CORPSE,
            loot_type::CORPSE,
            0,
            vec![item(4, 117, 1)],
            &mut b.step(),
        );
        b.sent();
        loot.set_auto_loot(true, &mut b.latch, &b.net, &mut b.fired);
        assert!(
            loot.auto_loot,
            "a record pending: the open's sweep will spend it"
        );
        assert!(b.sent().is_empty());
        b.answer(&mut loot, 117, Some(crate::items::test_template("Linen")));
        assert_eq!(b.fired(), ["LOOT_OPENED 1"]);
        assert!(!loot.auto_loot);
        assert!(matches!(
            b.sent()[..],
            [ClientCommand::AutostoreLootItem { slot: 4 }]
        ));

        loot.open(
            CORPSE,
            loot_type::CORPSE,
            0,
            vec![item(5, 117, 1)],
            &mut b.step(),
        );
        assert_eq!(b.fired(), ["LOOT_CLOSED", "LOOT_OPENED 0"]);
        loot.set_auto_loot(true, &mut b.latch, &b.net, &mut b.fired);
        assert!(!loot.auto_loot);
        assert_eq!(b.fired(), ["LOOT_OPENED 1"]);
        assert!(matches!(
            b.sent()[..],
            [ClientCommand::AutostoreLootItem { slot: 5 }]
        ));
    }

    /// Our own cast's effect loop (`0x80f780`-`0x80f7da`): pickpocket, skinning and skinning a
    /// corpse arm the flag from `autoLootDefault`; disenchant, prospect and mill only while the
    /// auto-loot test passes; an armed flag stays.
    #[test]
    fn our_own_cast_arms_auto_loot_by_its_effects() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let arm = |effects: [u32; 3], default: bool, armed: bool| {
            let mut latch = LootLatch::default();
            let cfg = LootConfig {
                auto_loot: default,
                ..LootConfig::default()
            };
            let mut loot = LootState {
                auto_loot: armed,
                ..LootState::default()
            };
            arm_on_spell_go(
                &effects,
                &mut loot,
                &cfg,
                None,
                &mut latch,
                &net,
                &mut Fired::default(),
            );
            loot.auto_loot
        };
        for skin in [71, 95, 116] {
            assert!(arm([0, skin, 0], true, false), "{skin}");
            assert!(!arm([skin, 0, 0], false, false), "{skin}");
        }
        for craft in [99, 127, 158] {
            assert!(arm([craft, 0, 0], true, false), "{craft}");
            assert!(
                !arm([craft, 0, 0], false, false),
                "{craft}: not auto-looting"
            );
        }
        assert!(!arm([33, 0, 0], true, false), "OPEN_LOCK arms nothing here");
        assert!(arm([95, 0, 0], false, true), "already armed: left set");
    }

    /// `LOOT_OPENED`'s argument is 1 only from the auto-loot sweep (`0x5890d3`), and 0 for a
    /// plain open (`0x58990b`); an armed window with nothing in it closes with no open at all
    /// (`0x5890e6`).
    #[test]
    fn loot_opened_carries_the_auto_loot_argument() {
        let mut b = Bench::new();
        b.template(TOUGH_JERKY, "Tough Jerky");
        b.latch.0 = Some(0x42);
        let mut loot = LootState::default();
        loot.open(
            0x42,
            loot_type::CORPSE,
            0,
            vec![item(3, TOUGH_JERKY, 1)],
            &mut b.step(),
        );
        assert_eq!(b.fired(), ["LOOT_OPENED 0"]);
        assert!(b.sent().is_empty(), "a plain open takes nothing");
        loot.close_table(&mut b.fired);
        assert_eq!(b.fired(), ["LOOT_CLOSED"]);

        loot.auto_loot = true;
        loot.open(
            0x42,
            loot_type::CORPSE,
            0,
            vec![item(3, TOUGH_JERKY, 1)],
            &mut b.step(),
        );
        assert_eq!(b.fired(), ["LOOT_OPENED 1"]);
        assert!(matches!(
            b.sent()[..],
            [ClientCommand::AutostoreLootItem { slot: 3 }]
        ));
        assert!(!loot.auto_loot, "the sweep spent it");
        loot.close_table(&mut b.fired);
        b.fired();

        loot.auto_loot = true;
        loot.open(0x42, loot_type::CORPSE, 0, vec![], &mut b.step());
        assert_eq!(b.fired(), ["LOOT_CLOSED"]);
        assert!(matches!(
            b.sent()[..],
            [ClientCommand::LootRelease { guid: 0x42 }]
        ));
        assert_eq!(loot.source(), None);
    }

    /// A new object over an open window lets it go with `LOOT_CLOSED` before the new open.
    #[test]
    fn a_window_replacing_the_open_one_closes_it_first() {
        let mut b = Bench::new();
        b.template(TOUGH_JERKY, "Tough Jerky");
        let mut loot = LootState::default();
        loot.open(
            0x42,
            loot_type::CORPSE,
            0,
            vec![item(3, TOUGH_JERKY, 1)],
            &mut b.step(),
        );
        assert_eq!(b.fired(), ["LOOT_OPENED 0"]);
        loot.open(
            0x43,
            loot_type::SKINNING,
            0,
            vec![item(0, TOUGH_JERKY, 1)],
            &mut b.step(),
        );
        assert_eq!(b.fired(), ["LOOT_CLOSED", "LOOT_OPENED 0"]);
    }

    /// A listener reads the window as of its raise (`0x81b530` runs it in the caller): the
    /// response lets the old window go, and the new one is set in turn after the close's
    /// listeners, so the close's listener reads no loot.
    #[test]
    fn a_replaced_windows_close_listener_does_not_see_the_new_window() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<LootLatch>()
            .init_resource::<LootState>()
            .init_resource::<GroupState>()
            .init_resource::<NameCache>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::ui_script::live::UiEvents>()
            .insert_resource(NetCommands(tx));
        let script = UiScript::new().unwrap();
        script
            .run(
                "SEEN = {} local f = CreateFrame('Frame') f:RegisterEvent('LOOT_CLOSED')
                 f:SetScript('OnEvent', function() SEEN[#SEEN + 1] = GetNumLootItems() end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        open_window(app.world_mut(), 0x42, loot_type::CORPSE, 5, Vec::new());
        let shown = |app: &mut App| -> i64 {
            crate::ui_script::live::with_live_world(app.world_mut(), |s| {
                s.eval("return GetNumLootItems()")
            })
            .expect("a VM")
            .unwrap()
        };
        assert!(shown(&mut app) > 0, "fixture: the first window is open");
        // The response handler's legs: the let-go in its turn, then the set.
        let world = app.world_mut();
        world.resource_scope(|w, mut loot: Mut<LootState>| {
            loot.let_go(&mut *w.resource_mut::<crate::ui_script::live::UiEvents>());
        });
        world
            .resource_mut::<crate::ui_script::live::UiEvents>()
            .world(|w| set_window(w, 0x43, loot_type::CORPSE, 9, Vec::new(), false));
        crate::ui_script::live::fire_handler_events(world);
        let seen: String = app
            .world()
            .non_send_resource::<UiScript>()
            .eval("return table.concat(SEEN, ',')")
            .unwrap();
        assert_eq!(seen, "0", "the close's listener reads no window");
        assert!(
            shown(&mut app) > 0,
            "the new window is set after the listener"
        );
    }
}
