//! The guild bank's app side, after 12340's guild bank info: the banker, the six tabs' names,
//! slots and locks, the money, the logs and texts, the wire, and the item gates and stack
//! arithmetic of the swap senders. Main-thread state the verbs read and write at the call
//! ([`crate::ui_script::live::MainState`]); its handlers fire their events in turn.

use benilla_protocol::messages::{
    GuildBankLogEntry, GuildBankSend, GuildBankSlot, GuildBankSwap, BAG_PLAYER_INVENTORY,
};
use bevy::prelude::*;

use benilla_ui::script::{
    GuildBankLogView, GuildBankRequest, GuildBankSlotView, GuildBankTabView, UiScript,
    GUILD_BANK_TABS,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::ui_guild::GuildState;
use crate::ui_script::live::UiEvents;
use crate::ui_script::UiDrain;

mod net;

pub(crate) use net::{guild_id_changed, name_landed, record_callback};

/// One slot as the reference keeps it (`0xc1dc40`, `0x2c` bytes): the wire fields and the lock (`+0x24`).
#[derive(Clone, Debug, Default)]
struct BankSlot {
    wire: GuildBankSlot,
    locked: bool,
}

/// One tab as the reference keeps it (`0xc11458`).
#[derive(Clone, Debug)]
struct Tab {
    name: String,
    icon: String,
    text: String,
    /// The slots are stale: the next query asks for the full update (`+0x2140`).
    stale: bool,
    /// The text is stale (`0xc0f8e8`): `QueryGuildBankText` sends only then.
    text_stale: bool,
    /// The slots, 98 once the tab's first list arrived (`0x5a735c`).
    slots: Vec<BankSlot>,
}

impl Default for Tab {
    fn default() -> Self {
        Self {
            name: String::new(),
            icon: String::new(),
            text: String::new(),
            stale: true,
            text_stale: true,
            slots: Vec::new(),
        }
    }
}

/// A log row with the time it was written.
#[derive(Clone, Debug)]
pub(crate) struct LogRow {
    entry: GuildBankLogEntry,
    written_at: u32,
}

/// The callback a record ask registered: the slots' (`0x5a47a0`) or the log's (`0x5a47d0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Callback {
    Slots,
    Log,
}

/// The guild bank (`0xc1dc0c`-`0xc1dc34`): filled by the handlers, read and written by the verbs at the call, reset at session end.
#[derive(Debug, Default)]
pub(crate) struct GuildBankState {
    /// The open banker (`0xc1dc10`).
    banker: Option<u64>,
    /// Asked, not yet answered (`0xc1dc18`): the first list opens the window.
    pending: Option<u64>,
    /// The current tab (`0xc1dc0d`).
    current_tab: u8,
    /// The bought tabs (`0xc1dc0c`).
    num_tabs: u8,
    tabs: [Tab; GUILD_BANK_TABS],
    /// The last list's item withdrawals left (`0xc1dc30`).
    withdrawals_remaining: i32,
    /// The bank's money (`0xc1dc20`).
    money: u64,
    /// Today's money withdrawal left (`0xc1dc34`), -1 unlimited.
    withdraw_money: i32,
    /// The six tab logs and the money log.
    logs: [Vec<LogRow>; GUILD_BANK_TABS + 1],
    /// The slots' records still asked (`0xc1dc28`).
    slot_waits: u32,
    /// The log's records still asked (`0xc1dc2c`).
    log_waits: u32,
    /// The log tab last answered (`0xacf8c8`).
    log_tab: Option<u8>,
    /// The name cache's callbacks registered, in ask order; the item cache's wait on [`crate::items::RecordWaits`].
    name_callbacks: Vec<(u64, Callback)>,
}

impl GuildBankState {
    /// The banker talk (`0x6d23c0`): nothing for the open banker, else the hello, asking for the
    /// tab list while tab 0 is stale; the first list then opens the window.
    pub(crate) fn talk(&mut self, banker: u64, net: &NetCommands) {
        if self.banker == Some(banker) {
            return;
        }
        let _ = net
            .0
            .send(ClientCommand::GuildBank(GuildBankSend::Activate {
                banker,
                full_update: self.tabs[0].stale,
            }));
        self.pending = Some(banker);
    }

    /// The open banker (`0xc1dc10`).
    pub(crate) fn banker(&self) -> Option<u64> {
        self.banker
    }

    /// Whether a banker is open.
    pub(crate) fn is_open(&self) -> bool {
        self.banker.is_some()
    }

    pub(crate) fn current_tab(&self) -> u8 {
        self.current_tab
    }

    /// `SetCurrentGuildBankTab`'s write (`0x5a43c0`).
    pub(crate) fn set_current_tab(&mut self, tab: u8) {
        self.current_tab = tab;
    }

    pub(crate) fn num_tabs(&self) -> u8 {
        self.num_tabs
    }

    pub(crate) fn withdrawals_remaining(&self) -> i32 {
        self.withdrawals_remaining
    }

    pub(crate) fn money(&self) -> u64 {
        self.money
    }

    pub(crate) fn withdraw_money(&self) -> i32 {
        self.withdraw_money
    }

    /// Tab `tab`'s name, icon and text.
    pub(crate) fn tab_view(&self, tab: u8) -> Option<GuildBankTabView> {
        let t = self.tabs.get(usize::from(tab))?;
        Some(GuildBankTabView {
            name: t.name.clone(),
            icon: t.icon.clone(),
            text: t.text.clone(),
        })
    }

    /// The banker setter's clear with no banker (`0x5a4270`): both bankers zeroed.
    pub(crate) fn clear_banker(&mut self) {
        self.banker = None;
        self.pending = None;
    }

    /// The all-tabs clear (`0x5a7760`): every tab forgotten and the current tab reset.
    fn clear_all_tabs(&mut self) {
        for tab in &mut self.tabs {
            tab.slots.clear();
            tab.stale = true;
            tab.text_stale = true;
        }
        self.num_tabs = 0;
        self.current_tab = 0;
    }

    /// The tab query send (`0x5a5800`): only with a banker open.
    fn query_tab(&self, tab: u8, net: &NetCommands) {
        let (Some(banker), Some(t)) = (self.banker, self.tabs.get(usize::from(tab))) else {
            return;
        };
        let _ = net
            .0
            .send(ClientCommand::GuildBank(GuildBankSend::QueryTab {
                banker,
                tab,
                full_update: t.stale,
            }));
    }

    /// `QueryGuildBankText` (`0x5a5940`): a fresh text, or no banker, answers at once; else the ask goes and the text is fresh.
    pub(crate) fn query_text(&mut self, tab: u8, net: &NetCommands) -> bool {
        let stale = self
            .tabs
            .get(usize::from(tab))
            .is_some_and(|t| t.text_stale);
        if self.banker.is_none() || !stale {
            return true;
        }
        let _ = net
            .0
            .send(ClientCommand::GuildBank(GuildBankSend::QueryText { tab }));
        self.tabs[usize::from(tab)].text_stale = false;
        false
    }

    /// Slot `(tab, slot)` when it holds an item (`0x5a4c10`).
    pub(crate) fn slot(&self, tab: u8, slot: u8) -> Option<&GuildBankSlot> {
        self.tabs
            .get(usize::from(tab))?
            .slots
            .get(usize::from(slot))
            .map(|s| &s.wire)
            .filter(|s| s.item_id != 0)
    }

    /// The lock test (`0x5a4c40`): locked, and still holding `item_id`.
    pub(crate) fn is_locked(&self, tab: u8, slot: u8, item_id: u32) -> bool {
        self.tabs
            .get(usize::from(tab))
            .and_then(|t| t.slots.get(usize::from(slot)))
            .is_some_and(|s| s.locked && s.wire.item_id == item_id)
    }

    /// Lock or unlock (`0x5a4c90`, `0x5a4ce0`): only while the slot holds `item_id`.
    pub(crate) fn set_lock(&mut self, tab: u8, slot: u8, item_id: u32, locked: bool) {
        if let Some(s) = self
            .tabs
            .get_mut(usize::from(tab))
            .and_then(|t| t.slots.get_mut(usize::from(slot)))
            .filter(|s| s.wire.item_id == item_id)
        {
            s.locked = locked;
        }
    }

    pub(crate) fn log_len(&self, tab: u8) -> usize {
        self.logs.get(usize::from(tab)).map_or(0, Vec::len)
    }

    pub(crate) fn log_row(&self, tab: u8, index: u8) -> Option<&LogRow> {
        self.logs.get(usize::from(tab))?.get(usize::from(index))
    }
}

pub(crate) struct UiGuildBankPlugin;

impl Plugin for UiGuildBankPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_non_send_resource::<GuildBankState>().add_systems(
            Update,
            crate::ui_script::raises::then_fire(drain_guild_bank)
                .in_set(UiDrain)
                .after(crate::ui_guild::GuildDrain),
        );
    }
}

/// The banker setter with no banker (`0x5a4270`): `GUILDBANKFRAME_CLOSED` with the banker still set, then the interact target's clear on it (`0x5a4291`), whose interaction close re-enters here while the banker held the target (`0x512fd1`), then both bankers clear; queued for the caller's flush.
pub(crate) fn set_guild_banker_none(ui: &mut UiEvents) {
    ui.fire("GUILDBANKFRAME_CLOSED", Vec::new());
    ui.world(|world| {
        if let Some(banker) = banker(world) {
            crate::ui_session::clear_interact_target(world, banker);
        }
        crate::ui_script::live::in_turn(world, |world| {
            if let Some(mut bank) = world.get_non_send_resource_mut::<GuildBankState>() {
                bank.clear_banker();
            }
        });
    });
}

/// The open guild banker (`0xc1dc10`).
pub(crate) fn banker(world: &World) -> Option<u64> {
    world.get_non_send_resource::<GuildBankState>()?.banker
}

/// The interaction close's guild bank leg (`0x512fd1`), its target already zeroed: the banker setter with no banker.
pub(crate) fn guild_bank_leg(world: &mut World) {
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        set_guild_banker_none(&mut ui);
    }
}

fn unix_now() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as u32)
}

/// The caches a slot and a log row resolve through at the call.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct BankCatalogs<'w> {
    items: Res<'w, Items>,
    icons: Option<Res<'w, ItemDisplays>>,
    names: Res<'w, NameCache>,
    props: Option<Res<'w, crate::items::RandomProperties>>,
    enchants: Option<Res<'w, crate::items::Enchants>>,
}

impl BankCatalogs<'_> {
    /// One item's icon, `None` while its template is in flight.
    pub(crate) fn texture(&self, item_id: u32) -> Option<String> {
        let t = self.items.template_cached(item_id)?;
        crate::ui_items::item_icon(self.icons.as_deref(), t.display_info_id)
    }

    /// One slot's view. The template asks once while in flight.
    pub(crate) fn slot_view(
        &self,
        s: &GuildBankSlot,
        commands: &NetCommands,
    ) -> Option<GuildBankSlotView> {
        if s.item_id == 0 {
            return None;
        }
        let rolls = crate::items::RollCatalogs {
            props: self.props.as_deref(),
            enchants: self.enchants.as_deref(),
        };
        let roll = s.random_property_id as u32;
        let Some(t) = self.items.template(s.item_id, 0, commands) else {
            return Some(GuildBankSlotView {
                item_id: s.item_id,
                count: s.count,
                flags: s.flags,
                ..Default::default()
            });
        };
        // The permanent enchant in slot 0, the gems in 2..=4, then the roll's lines, as the block source's tooltip takes them (`0x6304a0`).
        let gems = (0..3u8).map(|i| (2 + i, s.gems[usize::from(i)] as i32, 0u32, None));
        let mut enchants = crate::items::enchant_lines(
            std::iter::once((0u8, s.enchant as i32, 0u32, None)).chain(gems),
            self.enchants.as_deref(),
        );
        if let Some(row) = self
            .props
            .as_deref()
            .and_then(|p| p.0.get(s.random_property_id))
        {
            enchants.extend(crate::items::random_property_lines(
                row,
                self.enchants.as_deref(),
            ));
        }
        let name = rolls.name(&t.name, roll);
        Some(GuildBankSlotView {
            item_id: s.item_id,
            texture: crate::ui_items::item_icon(self.icons.as_deref(), t.display_info_id),
            count: s.count,
            link: Some(crate::ui_items::item_link_full(
                s.item_id,
                &benilla_ui::script::ItemLinkFields {
                    enchant: s.enchant as i32,
                    gems: s.gems.map(|g| g as i32),
                    ..benilla_ui::script::ItemLinkFields::roll(
                        s.random_property_id,
                        s.suffix_factor as i32,
                    )
                },
                &name,
                t.quality,
            )),
            max_stack: t.stackable,
            name: Some(name),
            enchants,
            flags: s.flags,
        })
    }

    /// One log row's view; the name and link wait on their caches.
    pub(crate) fn log_view(&self, row: &LogRow, commands: &NetCommands) -> GuildBankLogView {
        let e = &row.entry;
        let link = (e.item_id != 0)
            .then(|| self.items.template(e.item_id, 0, commands))
            .flatten()
            .map(|t| crate::ui_items::item_link(e.item_id, &t.name, t.quality))
            .unwrap_or_default();
        GuildBankLogView {
            kind: e.kind,
            player: self.names.resolve(e.player, commands).map(str::to_string),
            link,
            count: e.count,
            money: e.money,
            other_tab: e.other_tab,
            written_at: row.written_at,
        }
    }
}

/// What a swap gate reads of the player: the descriptor, the item objects and templates.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SwapInputs<'w, 's> {
    self_q: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    objects: Objects<'w, 's>,
    items: Res<'w, Items>,
    guild: Res<'w, GuildState>,
}

/// Drain the engine's sends: each gated on the open banker as the reference's senders are.
fn drain_guild_bank(
    mut vm: crate::ui_script::InTurn,
    bank: Option<NonSend<GuildBankState>>,
    net: Res<NetCommands>,
    inputs: SwapInputs,
) {
    let Some(mut script) = vm.turn() else {
        return;
    };
    for request in script.take_guild_bank_requests() {
        // The auto-store's event follows its send (`0x5a63f0`).
        let stored = bank
            .as_deref()
            .is_some_and(|state| handle(&mut script, state, &net, &inputs, request));
        if stored {
            script.fire_event("GUILDBANKBAGSLOTS_CHANGED", Vec::new());
        }
    }
}

fn send(net: &NetCommands, send: GuildBankSend) {
    debug!("guild bank: {send:?}");
    let _ = net.0.send(ClientCommand::GuildBank(send));
}

fn handle(
    script: &mut UiScript,
    state: &GuildBankState,
    net: &NetCommands,
    inputs: &SwapInputs,
    request: GuildBankRequest,
) -> bool {
    let banker = state.banker;
    match request {
        GuildBankRequest::QueryTab(tab) => state.query_tab(tab, net),
        GuildBankRequest::BuyTab(tab) => {
            if let Some(banker) = banker {
                send(net, GuildBankSend::BuyTab { banker, tab });
            }
        }
        GuildBankRequest::SetTabInfo { tab, name, icon } => {
            if let (Some(banker), true) = (banker, tab < state.num_tabs) {
                send(
                    net,
                    GuildBankSend::UpdateTab {
                        banker,
                        tab,
                        name,
                        icon,
                    },
                );
            }
        }
        GuildBankRequest::DepositMoney(copper) => {
            if let Some(banker) = banker {
                send(net, GuildBankSend::DepositMoney { banker, copper });
            }
        }
        GuildBankRequest::WithdrawMoney(copper) => {
            if let Some(banker) = banker {
                send(net, GuildBankSend::WithdrawMoney { banker, copper });
            }
        }
        // `QueryGuildBankLog` (`0x5a58c0`).
        GuildBankRequest::QueryLog(tab) => {
            if banker.is_some() {
                send(net, GuildBankSend::QueryLog { tab });
            }
        }
        // `SetGuildBankText` (`0x5a5c40`).
        GuildBankRequest::SetText { tab, text } => {
            if banker.is_some() && tab < state.num_tabs {
                send(net, GuildBankSend::SetText { tab, text });
            }
        }
        GuildBankRequest::AutoStore {
            tab,
            slot,
            item_id,
            count,
        } => {
            let Some(banker) = banker else { return false };
            // `AutoStoreGuildBankItem` (`0x5a63f0`): a rank with no withdrawals from the tab is
            // refused.
            if inputs.guild.own_bank_tab(tab).1 == 0 {
                script.guild_bank_swap_refused("ERR_GUILD_PERMISSIONS");
                return false;
            }
            send(
                net,
                GuildBankSend::Swap {
                    banker,
                    swap: GuildBankSwap::AutoStore {
                        tab,
                        slot,
                        item: item_id,
                        count,
                    },
                },
            );
            return true;
        }
        GuildBankRequest::DepositItem {
            tab,
            slot,
            bag,
            bag_slot,
            split,
        } => deposit_item(script, state, net, inputs, tab, slot, bag, bag_slot, split),
        GuildBankRequest::MoveItem {
            tab,
            slot,
            src_tab,
            src_slot,
            item_id,
            split,
        } => move_item(
            script, state, net, inputs, tab, slot, src_tab, src_slot, item_id, split,
        ),
        GuildBankRequest::WithdrawItem {
            src_tab,
            src_slot,
            item_id,
            bag,
            bag_slot,
            split,
        } => withdraw_item(
            script, state, net, inputs, src_tab, src_slot, item_id, bag, bag_slot, split,
        ),
    }
    false
}

/// The room a stack of `max` holding `held` has left.
fn room(max: u32, held: u32) -> u32 {
    max.saturating_sub(held)
}

/// The bag-to-bank swap (`0x5a50c0`): a bag item onto bank slot `(tab, slot)`, slot `0xFF` for the
/// first that fits. Its refusals keep the cursor; a send clears it.
#[allow(clippy::too_many_arguments)]
fn deposit_item(
    script: &mut UiScript,
    state: &GuildBankState,
    net: &NetCommands,
    inputs: &SwapInputs,
    tab: u8,
    slot: u8,
    bag: i64,
    bag_slot: u32,
    split: Option<u32>,
) {
    let Some(banker) = state.banker else { return };
    let Some(store) = inputs.self_q.iter().next() else {
        return;
    };
    let Some(slot0) = bag_slot.checked_sub(1).and_then(|s| u8::try_from(s).ok()) else {
        return;
    };
    let Some(guid) = crate::ui_items::slot_guid(&store.0, bag, slot0, &inputs.objects) else {
        return;
    };
    let Some(item) = inputs.objects.object(guid) else {
        return;
    };
    let entry = item.object_entry().unwrap_or(0);
    let Some(t) = inputs.items.template_cached(entry) else {
        return;
    };
    let item_flags = item.item_flags().unwrap_or(0);
    // Bound (`ITEM_FIELD_FLAGS & 1`), a quest item's own line first.
    if item_flags & 0x1 != 0 {
        let key = if t.bonding == 4 {
            "ERR_GUILD_BANK_QUEST_ITEM"
        } else {
            "ERR_GUILD_BANK_BOUND_ITEM"
        };
        script.guild_bank_swap_refused(key);
        return;
    }
    // Conjured (template flag `0x2`), or timed. The template's own duration is not read: a timed
    // item streams its `ITEM_FIELD_DURATION`.
    if t.flags & 0x2 != 0 || item.item_duration().unwrap_or(0) != 0 {
        script.guild_bank_swap_refused("ERR_GUILD_BANK_CONJURED_ITEM");
        return;
    }
    if item_flags & 0x8 != 0 {
        script.guild_bank_swap_refused("ERR_GUILD_BANK_WRAPPED_ITEM");
        return;
    }
    let Some((wire_bag, wire_slot)) = crate::ui_items::wire_pos(bag, bag_slot) else {
        return;
    };
    if wire_bag == BAG_PLAYER_INVENTORY && wire_slot < 23 {
        script.guild_bank_swap_refused("ERR_GUILD_BANK_EQUIPPED_ITEM");
        return;
    }
    if inputs.guild.own_bank_tab(tab).0 & 0x2 == 0 {
        script.guild_bank_swap_refused("ERR_GUILD_PERMISSIONS");
        return;
    }
    let bank = state.slot(tab, slot);
    let bank_item = bank.map_or(0, |b| b.item_id);
    let split = split.unwrap_or(0);
    let count = if bank_item == 0 {
        split
    } else if bank_item == entry {
        let room = room(t.stackable, bank.map_or(0, |b| b.count));
        if split != 0 {
            split.min(room)
        } else {
            item.item_stack_count().unwrap_or(1).min(room)
        }
    } else {
        0
    };
    send(
        net,
        GuildBankSend::Swap {
            banker,
            swap: GuildBankSwap::Deposit {
                tab,
                slot,
                bank_item,
                bag: wire_bag,
                bag_slot: wire_slot,
                count,
            },
        },
    );
    script.guild_bank_swap_sent();
}

/// The bank-to-bank swap (`0x5a5410`): a held bank item onto another bank slot.
#[allow(clippy::too_many_arguments)]
fn move_item(
    script: &mut UiScript,
    state: &GuildBankState,
    net: &NetCommands,
    inputs: &SwapInputs,
    tab: u8,
    slot: u8,
    src_tab: u8,
    src_slot: u8,
    item_id: u32,
    split: Option<u32>,
) {
    let Some(banker) = state.banker else { return };
    if inputs.guild.own_bank_tab(tab).0 & 0x2 == 0 {
        script.guild_bank_swap_refused("ERR_GUILD_PERMISSIONS");
        return;
    }
    let bank = state.slot(tab, slot);
    let bank_item = bank.map_or(0, |b| b.item_id);
    let split = split.unwrap_or(0);
    let held = state.slot(src_tab, src_slot).map_or(0, |b| b.count);
    let count = match inputs.items.template_cached(item_id) {
        // Unresolved: back onto its own slot is a no-op that drops the item.
        None if item_id == bank_item => {
            script.guild_bank_drop_held();
            return;
        }
        None => 0,
        Some(_) if bank_item == 0 => split,
        Some(t) if item_id == bank_item => {
            let room = room(t.stackable, bank.map_or(0, |b| b.count));
            if split != 0 {
                split.min(room)
            } else {
                held.min(room)
            }
        }
        Some(_) => 0,
    };
    // Across tabs, both tabs need withdrawals left, the destination only when it holds an item.
    if src_tab != tab
        && ((bank_item != 0 && inputs.guild.own_bank_tab(tab).1 == 0)
            || inputs.guild.own_bank_tab(src_tab).1 == 0)
    {
        script.guild_bank_swap_refused("ERR_GUILD_PERMISSIONS");
        return;
    }
    send(
        net,
        GuildBankSend::Swap {
            banker,
            swap: GuildBankSwap::BankToBank {
                tab,
                slot,
                bank_item,
                src_tab,
                src_slot,
                item: item_id,
                count,
            },
        },
    );
    script.guild_bank_swap_sent();
}

/// `PickupContainerItem`'s mode-11 arm and the bank-to-bag swap (`0x6db880`): a held bank item onto
/// a bag slot. The engine already cleared the cursor.
#[allow(clippy::too_many_arguments)]
fn withdraw_item(
    script: &mut UiScript,
    state: &GuildBankState,
    net: &NetCommands,
    inputs: &SwapInputs,
    src_tab: u8,
    src_slot: u8,
    item_id: u32,
    bag: i64,
    bag_slot: u32,
    split: Option<u32>,
) {
    let Some(banker) = state.banker else { return };
    let Some((wire_bag, wire_slot)) = crate::ui_items::wire_pos(bag, bag_slot) else {
        return;
    };
    let dest = inputs.self_q.iter().next().and_then(|store| {
        let slot0 = u8::try_from(bag_slot.checked_sub(1)?).ok()?;
        let guid = crate::ui_items::slot_guid(&store.0, bag, slot0, &inputs.objects)?;
        let obj = inputs.objects.object(guid)?;
        Some((
            obj.object_entry().unwrap_or(0),
            obj.item_stack_count().unwrap_or(1),
        ))
    });
    let split = split.unwrap_or(0);
    let held = state.slot(src_tab, src_slot).map_or(0, |b| b.count);
    let count = match (inputs.items.template_cached(item_id), dest) {
        (Some(_), None) => split,
        (Some(t), Some((entry, stack))) if entry == item_id => {
            let room = room(t.stackable, stack);
            if split != 0 {
                split.min(room)
            } else {
                held.min(room)
            }
        }
        _ => 0,
    };
    if inputs.guild.own_bank_tab(src_tab).1 == 0 {
        script.guild_bank_swap_refused("ERR_GUILD_PERMISSIONS");
        return;
    }
    send(
        net,
        GuildBankSend::Swap {
            banker,
            swap: GuildBankSwap::Withdraw {
                tab: src_tab,
                slot: src_slot,
                item: item_id,
                bag: wire_bag,
                bag_slot: wire_slot,
                count,
            },
        },
    );
}
