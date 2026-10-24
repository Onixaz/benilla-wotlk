//! The guild bank's app side, 12340's `CGGuildBankInfo` (`GuildBankFrame.cpp`): the banker, the
//! six tabs' names, slots, logs and texts, the wire, and the item gates and stack arithmetic of
//! the swap senders. The engine keeps the cursor, the locks and the current tab
//! ([`benilla_ui::script::GuildBankView`]); its sends arrive as
//! [`benilla_ui::script::GuildBankRequest`]s.

use benilla_protocol::messages::{
    GuildBankLogEntry, GuildBankSend, GuildBankSlot, GuildBankSwap, BAG_PLAYER_INVENTORY,
    GUILD_BANK_TAB_SLOTS,
};
use bevy::prelude::*;

use benilla_ui::script::{
    GuildBankLogView, GuildBankRequest, GuildBankSlotView, GuildBankTabView, GuildBankView,
    ScriptValue, UiScript, GUILD_BANK_TABS,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::ui_guild::GuildState;
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::{close_npc_session_out_of_range, NpcSession};

mod net;

/// Frames a list or log waits for its names and item templates before its event fires anyway:
/// the reference fires on the last cache callback, a failed answer included.
const RESOLVE_MAX_FRAMES: u16 = 120;

/// One tab as the reference keeps it (`GuildBankTabInfo`, `0xc11458`).
#[derive(Clone, Debug)]
struct Tab {
    name: String,
    icon: String,
    text: String,
    /// The slots are stale: the next query asks for the full update (`+0x2140`, set by world
    /// entry and `ClearAllTabs`, cleared by a list of this tab).
    stale: bool,
    /// The text is stale (`m_tabContentsDirty`, `0xc0f8e8`): `QueryGuildBankText` sends only then.
    text_stale: bool,
    /// The slots, [`GUILD_BANK_TAB_SLOTS`] once the tab's first list arrived.
    slots: Vec<GuildBankSlot>,
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
struct LogRow {
    entry: GuildBankLogEntry,
    written_at: u32,
}

/// The guild bank session: filled by the handlers, read by the feed, reset at world entry.
#[derive(Resource, Debug)]
pub(crate) struct GuildBankState {
    /// The open banker (`m_banker`, `0xc1dc10`).
    banker: Option<u64>,
    /// Asked, not yet answered (`m_pendingBanker`, `0xc1dc18`): the first list opens the window.
    pending: Option<u64>,
    num_tabs: u8,
    tabs: [Tab; GUILD_BANK_TABS],
    /// The last list's item withdrawals left.
    withdrawals_remaining: i32,
    /// The six tab logs and the money log.
    logs: [Vec<LogRow>; GUILD_BANK_TABS + 1],
    /// Events owed, fired by the feed in this order.
    owe_opened: bool,
    owe_closed: bool,
    owe_tabs: u32,
    owe_slots: Option<(u8, u16)>,
    owe_bare_slots: bool,
    owe_log: Option<(u8, u16)>,
    owe_text: Vec<u8>,
    owe_text_changed: Vec<u8>,
    /// Slots a list rewrote, whose locks the engine drops.
    listed: Vec<(u8, Vec<u8>)>,
    /// A guild event asked for the current tab again.
    requery: bool,
    /// The guild the tabs belong to; a change clears them.
    guild: Option<u32>,
    /// The pushed view is stale.
    dirty: bool,
}

impl Default for GuildBankState {
    fn default() -> Self {
        Self {
            banker: None,
            pending: None,
            num_tabs: 0,
            tabs: Default::default(),
            withdrawals_remaining: 0,
            logs: Default::default(),
            owe_opened: false,
            owe_closed: false,
            owe_tabs: 0,
            owe_slots: None,
            owe_bare_slots: false,
            owe_log: None,
            owe_text: Vec::new(),
            owe_text_changed: Vec::new(),
            listed: Vec::new(),
            requery: false,
            guild: None,
            dirty: true,
        }
    }
}

impl GuildBankState {
    /// `TalkToGuildBanker` (`0x6d23c0`): nothing for the open banker, else the hello, asking for
    /// the tab list while tab 0 is stale; the first list then opens the window.
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

    /// Whether a banker is open.
    #[cfg(test)]
    pub(crate) fn is_open(&self) -> bool {
        self.banker.is_some()
    }

    /// `SetGuildBanker(0)` (`0x5a4270`): `GUILDBANKFRAME_CLOSED`, and both bankers clear.
    fn close(&mut self) {
        if self.banker.is_some() {
            self.owe_closed = true;
        }
        self.banker = None;
        self.pending = None;
        self.dirty = true;
    }

    /// `ClearAllTabs` (`0x5a7760`): a guild change forgets every tab.
    fn clear_all_tabs(&mut self) {
        for tab in &mut self.tabs {
            tab.slots.clear();
            tab.stale = true;
            tab.text_stale = true;
        }
        self.num_tabs = 0;
        self.dirty = true;
    }

    /// `SendGuildBankQueryTabOpcode` (`0x5a5800`): only with a banker open.
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

    fn slot(&self, tab: u8, slot: u8) -> Option<&GuildBankSlot> {
        self.tabs
            .get(usize::from(tab))?
            .slots
            .get(usize::from(slot))
            .filter(|s| s.item_id != 0)
    }
}

/// The range guard closes the bank, as walking away from any banker does.
impl NpcSession for GuildBankState {
    fn npc(&self) -> Option<u64> {
        self.banker
    }

    fn close(&mut self) {
        GuildBankState::close(self);
    }
}

pub(crate) struct UiGuildBankPlugin;

impl Plugin for UiGuildBankPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<GuildBankState>().add_systems(
            Update,
            (
                close_npc_session_out_of_range::<GuildBankState>.before(feed_guild_bank),
                // After the guild mirror, whose ranks and tabard it reads.
                feed_guild_bank
                    .in_set(UiFeed)
                    .after(crate::ui_guild::GuildFeed),
                drain_guild_bank
                    .after(UiInput)
                    .after(crate::ui_guild::GuildDrain),
            ),
        );
    }
}

fn unix_now() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as u32)
}

/// The catalogs a slot view resolves through.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct BankCatalogs<'w> {
    items: Res<'w, Items>,
    icons: Option<Res<'w, ItemDisplays>>,
    names: Res<'w, NameCache>,
    props: Option<Res<'w, crate::items::RandomProperties>>,
    enchants: Option<Res<'w, crate::items::Enchants>>,
}

impl BankCatalogs<'_> {
    /// One slot's view; `None` for an empty slot. The template asks once while in flight.
    fn slot_view(&self, s: &GuildBankSlot, commands: &NetCommands) -> Option<GuildBankSlotView> {
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
        // The permanent enchant in slot 0, then the roll's lines, as the block source's tooltip
        // takes them (`0x6304a0`); benilla's tooltips draw no socket lines, so the gems wait.
        let mut enchants = crate::items::enchant_lines(
            [(0u8, s.enchant as i32, 0u32, None)],
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
                s.enchant,
                roll,
                s.suffix_factor,
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
    fn log_view(&self, row: &LogRow, commands: &NetCommands) -> GuildBankLogView {
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

    /// Whether every listed item in `tab` has its template.
    fn tab_resolved(&self, state: &GuildBankState, tab: u8) -> bool {
        state.tabs[usize::from(tab)]
            .slots
            .iter()
            .filter(|s| s.item_id != 0)
            .all(|s| self.items.template_cached(s.item_id).is_some())
    }

    /// Whether every name and item of the `tab` log is cached.
    fn log_resolved(&self, state: &GuildBankState, tab: u8, commands: &NetCommands) -> bool {
        state.logs[usize::from(tab)].iter().all(|r| {
            self.names.resolve(r.entry.player, commands).is_some()
                && (r.entry.item_id == 0 || self.items.template_cached(r.entry.item_id).is_some())
        })
    }
}

/// Push the view and fire the events owed, in the reference's order.
fn feed_guild_bank(
    script: Option<NonSendMut<UiScript>>,
    mut state: ResMut<GuildBankState>,
    guild: Res<GuildState>,
    catalogs: BankCatalogs,
    commands: Res<NetCommands>,
    mut last: Local<crate::ui_script::VmMemo<Option<GuildBankView>>>,
    mut watch: Local<crate::ui_script::VmMemo<FeedWatch>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let state = &mut *state;
    let watch = watch.get(&script);
    // A guild change forgets the tabs, and leaving one closes the bank.
    if state.guild.replace(guild.guild_id()) != Some(guild.guild_id()) {
        state.clear_all_tabs();
        if guild.guild_id() == 0 {
            state.close();
        }
    }
    if state.requery {
        state.requery = false;
        state.query_tab(script.guild_bank_tab(), &commands);
    }
    for (tab, slots) in std::mem::take(&mut state.listed) {
        script.guild_bank_slots_listed(tab, &slots);
    }
    // A landed template or name repaints a slot or a log row.
    let templates = watch.templates.moved(catalogs.items.template_epoch());
    let names = watch.names.moved(catalogs.names.generation());
    let last = last.get(&script);
    if state.dirty || templates || names || last.is_none() {
        state.dirty = false;
        let view = build_view(state, &catalogs, &commands);
        if last.as_ref() != Some(&view) {
            script.set_guild_bank(view.clone());
            *last = Some(view);
        }
    }
    if std::mem::take(&mut state.owe_closed) {
        script.guild_bank_closed();
        script.fire_event("GUILDBANKFRAME_CLOSED", Vec::new());
    }
    if std::mem::take(&mut state.owe_opened) {
        script.fire_event("GUILDBANKFRAME_OPENED", Vec::new());
    }
    for _ in 0..std::mem::take(&mut state.owe_tabs) {
        script.fire_event("GUILDBANK_UPDATE_TABS", Vec::new());
    }
    if std::mem::take(&mut state.owe_bare_slots) {
        script.fire_event("GUILDBANKBAGSLOTS_CHANGED", Vec::new());
    }
    if let Some((tab, waited)) = state.owe_slots {
        if catalogs.tab_resolved(state, tab) || waited >= RESOLVE_MAX_FRAMES {
            state.owe_slots = None;
            if state.banker.is_some() {
                script.fire_event("GUILDBANKBAGSLOTS_CHANGED", Vec::new());
            }
        } else {
            state.owe_slots = Some((tab, waited + 1));
        }
    }
    if let Some((tab, waited)) = state.owe_log {
        if catalogs.log_resolved(state, tab, &commands) || waited >= RESOLVE_MAX_FRAMES {
            state.owe_log = None;
            if state.banker.is_some() {
                script.fire_event("GUILDBANKLOG_UPDATE", Vec::new());
            }
        } else {
            state.owe_log = Some((tab, waited + 1));
        }
    }
    for tab in std::mem::take(&mut state.owe_text) {
        script.fire_event(
            "GUILDBANK_UPDATE_TEXT",
            vec![ScriptValue::Int(i64::from(tab) + 1)],
        );
    }
    for tab in std::mem::take(&mut state.owe_text_changed) {
        script.fire_event(
            "GUILDBANK_TEXT_CHANGED",
            vec![ScriptValue::Int(i64::from(tab) + 1)],
        );
    }
}

/// The caches the pushed view last read.
#[derive(Default)]
struct FeedWatch {
    templates: crate::ui_script::gate::Watch,
    names: crate::ui_script::gate::Watch,
}

fn build_view(
    state: &GuildBankState,
    catalogs: &BankCatalogs,
    commands: &NetCommands,
) -> GuildBankView {
    GuildBankView {
        open: state.banker.is_some(),
        num_tabs: state.num_tabs,
        tabs: state
            .tabs
            .iter()
            .map(|t| GuildBankTabView {
                name: t.name.clone(),
                icon: t.icon.clone(),
                text: t.text.clone(),
            })
            .collect(),
        slots: state
            .tabs
            .iter()
            .map(|t| {
                t.slots
                    .iter()
                    .map(|s| catalogs.slot_view(s, commands))
                    .collect()
            })
            .collect(),
        withdrawals_remaining: state.withdrawals_remaining,
        logs: state
            .logs
            .iter()
            .map(|rows| {
                rows.iter()
                    .map(|r| catalogs.log_view(r, commands))
                    .collect()
            })
            .collect(),
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
    script: Option<NonSendMut<UiScript>>,
    mut state: ResMut<GuildBankState>,
    net: Res<NetCommands>,
    inputs: SwapInputs,
) {
    let Some(mut script) = script else {
        return;
    };
    for request in script.take_guild_bank_requests() {
        handle(&mut script, &mut state, &net, &inputs, request);
    }
}

fn send(net: &NetCommands, send: GuildBankSend) {
    debug!("guild bank: {send:?}");
    let _ = net.0.send(ClientCommand::GuildBank(send));
}

fn handle(
    script: &mut UiScript,
    state: &mut GuildBankState,
    net: &NetCommands,
    inputs: &SwapInputs,
    request: GuildBankRequest,
) {
    let banker = state.banker;
    match request {
        GuildBankRequest::QueryTab(tab) => state.query_tab(tab, net),
        GuildBankRequest::Close => state.close(),
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
        // `QueryGuildBankText` (`0x5a5940`): a fresh text, or no banker, answers at once.
        GuildBankRequest::QueryText(tab) => {
            let stale = state
                .tabs
                .get(usize::from(tab))
                .is_some_and(|t| t.text_stale);
            if banker.is_none() || !stale {
                state.owe_text.push(tab);
            } else {
                send(net, GuildBankSend::QueryText { tab });
                state.tabs[usize::from(tab)].text_stale = false;
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
            let Some(banker) = banker else { return };
            // `AutoStoreGuildBankItem` (`0x5a63f0`): a rank with no withdrawals from the tab is
            // refused.
            if inputs.guild.own_bank_tab(tab).1 == 0 {
                script.guild_bank_swap_refused("ERR_GUILD_PERMISSIONS");
                return;
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
            script.fire_event("GUILDBANKBAGSLOTS_CHANGED", Vec::new());
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
}

/// The room a stack of `max` holding `held` has left.
fn room(max: u32, held: u32) -> u32 {
    max.saturating_sub(held)
}

/// `SwapItems` (`0x5a50c0`): a bag item onto bank slot `(tab, slot)`, slot `0xFF` for the first
/// that fits. Its refusals keep the cursor; a send clears it.
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

/// `SwapBankItems` (`0x5a5410`): a held bank item onto another bank slot.
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

/// `PickupContainerItem`'s mode-11 arm and `SwapGuildBankItem` (`0x6db880`): a held bank item
/// onto a bag slot. The engine already cleared the cursor.
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

/// Slots per tab, for the handlers.
pub(super) const TAB_SLOTS: u8 = GUILD_BANK_TAB_SLOTS;
