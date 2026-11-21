//! The gossip window's app side: [`GossipState`] holds the gossip globals and the NPC-text
//! records, the handlers write them and fire `GOSSIP_SHOW` and `GOSSIP_CLOSED` in their turn
//! (`0x58ac30`, `0x58a550`), and [`GossipView`] lends them to the verbs at their call.

use std::collections::HashMap;

use benilla_protocol::messages::{
    select_block, GossipMenu as WireMenu, GossipOption, NpcTextBlock, QuestOption,
};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{GossipOptionView, GossipQuestRow, GossipRead};

use crate::creature_anim::QueueNpcEmotes;
use crate::names::NameCache;
use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_quest::{row_is_active, row_is_one_click};
use crate::ui_script::live::UiEvents;
use crate::ui_session::NpcSession;

/// The gossip globals, plus the NPC-text records by id, which outlive them. The cache holds whole
/// records: the reference draws the greeting afresh at each draw, from the NPC's gender and a roll.
#[derive(Resource, Default, Clone)]
pub(crate) struct GossipState {
    /// The gossip NPC (`0xc016f0`).
    pub(crate) npc: Option<u64>,
    /// Echoed in every select (`0xc016f8`).
    pub(crate) menu_id: u32,
    /// The text the last menu asked for.
    pub(crate) text_id: u32,
    /// The text-pending latch (`0xc00be8`): set by a draw's cache miss, cleared by a hit.
    pending: bool,
    /// The last drawn greeting (`0xc00ef0`); a close leaves it.
    pub(crate) greeting: String,
    /// The options and quest rows the last menu read (`0xc00bf0`, `0xbfc968`), wire and Lua-facing.
    pub(crate) options: Vec<GossipOption>,
    views: Vec<GossipOptionView>,
    pub(crate) quests: Vec<QuestOption>,
    rows: Vec<GossipQuestRow>,
    /// The text ids a draw's miss asked for, each answer re-running the draw once (`0x58ac43`).
    waits: Vec<u32>,
    records: HashMap<u32, Vec<NpcTextBlock>>,
}

/// `npccache.wdb`: the NPC-text records.
impl crate::wdb::Persisted for GossipState {
    fn drop_persisted(&mut self) {
        self.records.clear();
    }
}

impl GossipState {
    pub(crate) fn cached_record(&self, text_id: u32) -> Option<&[NpcTextBlock]> {
        self.records.get(&text_id).map(Vec::as_slice)
    }

    pub(crate) fn remember_record(&mut self, text_id: u32, blocks: Vec<NpcTextBlock>) {
        self.records.insert(text_id, blocks);
    }

    /// The text-pending latch (`0xc00be8`), under which every select is refused.
    pub(crate) fn text_pending(&self) -> bool {
        self.pending
    }

    /// `0x58a7b0`: the options and quest rows emptied.
    fn clear_menu(&mut self) {
        self.options.clear();
        self.views.clear();
        self.quests.clear();
        self.rows.clear();
    }

    /// A menu's options and rows read off the wire (`0x58b1e2`-`0x58b37b`).
    pub(crate) fn read_menu(&mut self, menu: WireMenu) {
        self.menu_id = menu.menu_id;
        self.text_id = menu.text_id;
        self.views = menu
            .options
            .iter()
            .map(|o| GossipOptionView {
                label: o.message.clone(),
                icon_type: gossip_icon_type(o.icon).into(),
                coded: o.coded,
                box_text: o.box_text.clone(),
                box_money: o.box_money,
            })
            .collect();
        // Active or available by the wire icon: the reference's `{3,4}` test in `0x58a5d0` and `0x58a6c0`.
        self.rows = menu
            .quests
            .iter()
            .map(|q| GossipQuestRow {
                quest_id: q.quest_id,
                title: q.title.clone(),
                level: q.level,
                active: row_is_active(q.icon),
                daily: q.flags & QUEST_FLAG_DAILY != 0,
                repeatable: q.repeatable,
            })
            .collect();
        self.options = menu.options;
        self.quests = menu.quests;
    }
}

/// The greeting draw's roll in `[1.0, 2.0)`, built as the reference builds it: random bits in the
/// mantissa of 1.0 (`(rand & 0x7fffff) | 0x3f800000`), from our own random source.
fn greeting_roll() -> f32 {
    f32::from_bits((rand::random::<u32>() & 0x7f_ffff) | 0x3f80_0000)
}

mod net;

/// The gossip window's handlers and its range close.
pub(crate) struct UiGossipPlugin;

impl Plugin for UiGossipPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<GossipState>();
    }
}

/// The NPC held, for the windows' re-click gate and portrait; every gossip close runs [`set_npc`].
impl NpcSession for GossipState {
    fn npc(&self) -> Option<u64> {
        self.npc
    }

    fn close(&mut self) {
        self.npc = None;
        self.menu_id = 0;
    }
}

/// The gossip set (`0x58a550`): the held NPC again does nothing; a new one takes the interact target, which closes the window holding the old (`0x58a5b4` -> `0x51fce0`), then is held. Zero clears the interact target first unless a quest giver is held (`0x58a570` -> `0x518d50`), whose interaction close re-enters here while the NPC held it (`0x512ef6`), then zeroes the NPC and the menu id and fires `GOSSIP_CLOSED` (`0x58a5a8`): twice in all on that path.
pub(crate) fn set_npc(world: &mut World, guid: u64) {
    let held = world.get_resource::<GossipState>().and_then(|g| g.npc);
    if held.unwrap_or(0) == guid {
        return;
    }
    if guid != 0 {
        crate::ui_session::set_interact_target(world, guid);
        // Held once the target's close has fired its events (`0x58a5bb`).
        crate::ui_script::live::in_turn(world, move |w| {
            if let Some(mut g) = w.get_resource_mut::<GossipState>() {
                g.npc = Some(guid);
            }
        });
        return;
    }
    let giver = world
        .get_non_send_resource::<crate::ui_quest::QuestGiver>()
        .and_then(|q| q.npc());
    if let (None, Some(old)) = (giver, held) {
        crate::ui_session::clear_interact_target(world, old);
    }
    if let Some(mut g) = world.get_resource_mut::<GossipState>() {
        g.npc = None;
        g.menu_id = 0;
    }
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.fire("GOSSIP_CLOSED", vec![]);
    }
}

/// The text draw (`0x58ac30`): a cache miss sets the pending latch and asks with the callback (`0x58ac5e`); a hit clears it (`0x58ac75`) and, with our player and the NPC objects held, draws the greeting by the NPC's gender and a roll, plays its emotes and fires `GOSSIP_SHOW` (`0x58aef6`); a record with no line for that gender closes the gossip (`0x58addd`).
pub(crate) fn set_text(world: &mut World, text_id: u32) {
    let mut state = bevy::ecs::system::SystemState::<DrawParams>::new(world);
    let drawn = state.get_mut(world).draw(text_id);
    state.apply(world);
    match drawn {
        Draw::Asked | Draw::NoObjects => {}
        Draw::Missing => {
            debug!("ui_gossip: missing gossip text (id {text_id}) — ending the interaction");
            set_npc(world, 0);
        }
        Draw::Shown => {
            if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
                ui.fire("GOSSIP_SHOW", vec![]);
            }
        }
    }
}

/// What a draw did.
enum Draw {
    Asked,
    NoObjects,
    Missing,
    Shown,
}

/// What the text draw reads.
#[derive(SystemParam)]
struct DrawParams<'w, 's> {
    gossip: ResMut<'w, GossipState>,
    index: Res<'w, GuidIndex>,
    stores: Query<'w, 's, &'static ObjectStore>,
    self_q: Query<'w, 's, (&'static ObjectStore, &'static Guid), With<SelfPlayer>>,
    names: Res<'w, NameCache>,
    commands: Res<'w, NetCommands>,
    states: Res<'w, crate::world_state::WorldStates>,
    emotes: Option<MessageWriter<'w, QueueNpcEmotes>>,
}

impl DrawParams<'_, '_> {
    fn draw(&mut self, text_id: u32) -> Draw {
        if self.gossip.cached_record(text_id).is_none() {
            self.gossip.pending = true;
            self.gossip.waits.push(text_id);
            // The record cache never asks for id 0.
            if text_id != 0 {
                let guid = self.gossip.npc.unwrap_or(0);
                let _ = self
                    .commands
                    .0
                    .send(ClientCommand::NpcTextQuery { text_id, guid });
            }
            return Draw::Asked;
        }
        self.gossip.pending = false;
        let npc = self
            .gossip
            .npc
            .and_then(|g| self.index.0.get(&g))
            .and_then(|&e| self.stores.get(e).ok());
        let (Some(npc), true) = (npc, self.self_q.iter().next().is_some()) else {
            return Draw::NoObjects;
        };
        // The reference tests `== 1` for female (`0x58ace0`), so genderless 2 reads male.
        let gender = npc.0.unit_gender().unwrap_or(0);
        let blocks = self.gossip.cached_record(text_id).unwrap_or_default();
        let Some(block) = select_block(blocks, gender, greeting_roll()) else {
            return Draw::Missing;
        };
        let line = block.line(gender).to_string();
        let emotes = block.played_emotes();
        let player = crate::npc_text::player_identity(&self.self_q, &self.names, &self.commands);
        self.gossip.greeting = crate::npc_text::substitute(
            &line,
            &crate::npc_text::MacroContext {
                subject: player.as_ref(),
                states: &self.states,
                achievement: None,
            },
        );
        if let (Some(guid), Some(out), false) =
            (self.gossip.npc, self.emotes.as_mut(), emotes.is_empty())
        {
            out.write(QueueNpcEmotes { guid, emotes });
        }
        Draw::Shown
    }
}

/// `SMSG_GOSSIP_MESSAGE` (`0x58b1b0`): the NPC set (`0x58b1ce`), the menu emptied and read (`0x58b1d6`), then the text drawn (`0x58b381`).
pub(crate) fn gossip_menu(world: &mut World, menu: WireMenu) {
    debug!(
        "net: gossip menu {} on {:#x} — {} options, {} quests",
        menu.menu_id,
        menu.npc,
        menu.options.len(),
        menu.quests.len()
    );
    let (npc, text_id) = (menu.npc, menu.text_id);
    crate::ui_script::live::in_turn(world, move |w| set_npc(w, npc));
    crate::ui_script::live::in_turn(world, move |w| {
        if let Some(mut g) = w.get_resource_mut::<GossipState>() {
            g.clear_menu();
            g.read_menu(menu);
        }
    });
    crate::ui_script::live::in_turn(world, move |w| set_text(w, text_id));
}

/// The text's answer (`0x58aae0`): each draw waiting on it runs again; a miss closes the gossip once per wait (`0x58ab07`). Nothing is cached on a miss, so the next draw asks again.
pub(crate) fn text_answered(world: &mut World, text_id: u32, blocks: Option<Vec<NpcTextBlock>>) {
    let found = blocks.is_some();
    let waits = {
        let Some(mut g) = world.get_resource_mut::<GossipState>() else {
            return;
        };
        if let Some(blocks) = blocks {
            g.remember_record(text_id, blocks);
        }
        let before = g.waits.len();
        g.waits.retain(|&w| w != text_id);
        before - g.waits.len()
    };
    for _ in 0..waits {
        if found {
            set_text(world, text_id);
        } else {
            debug!("ui_gossip: no such gossip text (id {text_id}) — ending the interaction");
            set_npc(world, 0);
        }
    }
}

/// A menu as the wire reads it with its greeting already drawn, `GOSSIP_SHOW` fired in turn: the UI fixture's seed.
pub(crate) fn seed_drawn(world: &mut World, menu: WireMenu, greeting: String) {
    set_npc(world, menu.npc);
    let states = crate::world_state::WorldStates::default();
    if let Some(mut g) = world.get_resource_mut::<GossipState>() {
        g.clear_menu();
        g.read_menu(menu);
        g.pending = false;
        g.greeting = crate::npc_text::substitute(
            &greeting,
            &crate::npc_text::MacroContext {
                subject: None,
                states: &states,
                achievement: None,
            },
        );
    }
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.fire("GOSSIP_SHOW", vec![]);
    }
    crate::ui_script::live::fire_handler_events(world);
}

/// The gossip globals the verbs read and write at their call.
#[derive(SystemParam)]
pub(crate) struct GossipView<'w, 's> {
    gossip: Option<ResMut<'w, GossipState>>,
    names: Option<Res<'w, NameCache>>,
    index: Option<Res<'w, GuidIndex>>,
    stores: Query<'w, 's, &'static ObjectStore>,
    me: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    commands: Option<Res<'w, NetCommands>>,
}

impl GossipView<'_, '_> {
    /// The NPC the gossip menu is open on (`0xc016f0`).
    pub(crate) fn npc(&self) -> Option<u64> {
        self.gossip.as_deref().and_then(|g| g.npc)
    }

    /// The globals at the call.
    pub(crate) fn read(&self) -> Option<GossipRead<'_>> {
        let g = self.gossip.as_deref()?;
        // `0x715e1a`: the held NPC's cached record, type flags bit 27.
        let force_gossip = g
            .npc
            .and_then(|n| self.index.as_deref()?.0.get(&n).copied())
            .and_then(|e| self.stores.get(e).ok())
            .and_then(|s| s.0.object_entry())
            .and_then(|entry| self.names.as_deref()?.creature_record(entry))
            .is_some_and(|r| r.type_flags & TYPE_FLAG_FORCE_GOSSIP != 0);
        let money = self
            .me
            .single()
            .ok()
            .and_then(|s| s.0.player_money())
            .unwrap_or(0);
        Some(GossipRead {
            greeting: &g.greeting,
            options: &g.views,
            quests: &g.rows,
            pending: g.pending,
            force_gossip,
            money: u64::from(money),
        })
    }

    /// `CMSG_GOSSIP_SELECT_OPTION` with the menu id, the option's wire index and a coded option's code, to the held NPC (`0x58b040`), then the menu emptied (`0x58a7b0`).
    pub(crate) fn select_option(&mut self, slot: usize, code: Option<String>) {
        let Some(g) = self.gossip.as_mut() else {
            return;
        };
        let Some(opt) = g.options.get(slot) else {
            return;
        };
        if let Some(c) = &self.commands {
            let _ = c.0.send(ClientCommand::GossipSelectOption {
                guid: g.npc.unwrap_or(0),
                menu_id: g.menu_id,
                option: opt.index,
                code,
            });
        }
        g.clear_menu();
    }

    /// A quest row's send to the held NPC (`0x58b070`, `0x58b130`): an active row, or an available one with its one-click flag, is a turn-in; else a query.
    pub(crate) fn select_quest(&self, row: usize) {
        let Some(g) = self.gossip.as_deref() else {
            return;
        };
        let Some(q) = g.quests.get(row) else {
            return;
        };
        let npc = g.npc.unwrap_or(0);
        let quest = q.quest_id;
        let cmd = if row_is_active(q.icon) || row_is_one_click(q.icon) {
            ClientCommand::QuestgiverComplete { npc, quest }
        } else {
            ClientCommand::QuestgiverQuery { npc, quest }
        };
        if let Some(c) = &self.commands {
            let _ = c.0.send(cmd);
        }
    }

    /// `CloseGossip`'s zero set (`0x58a550`): the count of `GOSSIP_CLOSED`s, two when the NPC held the interact target and no quest giver is held (`0x58a588` -> `0x512ef6`), whose clear zeroes the target.
    pub(crate) fn close(
        &mut self,
        target: Option<&mut crate::ui_session::InteractTarget>,
        giver_held: bool,
    ) -> u32 {
        let Some(g) = self.gossip.as_mut() else {
            return 0;
        };
        let Some(npc) = g.npc.take() else {
            return 0;
        };
        g.menu_id = 0;
        if !giver_held && target.is_some_and(|t| t.clear_in_call(npc)) {
            2
        } else {
            1
        }
    }
}

/// The `GetGossipOptions()` type string for a wire icon byte, which the reference's
/// `GetGossipOptions` (`0x58aa0e`) uses as a bare index into the table at `0xacef20`.
///
/// Deviation: 14 and 15 (NULLs, pushed as `nil`) and 21 up (a read past the table's end) give
/// `"gossip"`, because the reference's behaviour there is a missing bounds check.
fn gossip_icon_type(icon: u8) -> &'static str {
    GOSSIP_ICON_TYPES
        .get(icon as usize)
        .copied()
        .flatten()
        .unwrap_or("gossip")
}

/// The reference's icon-type table at `0xacef20`, verbatim; 11-13 and 16-20 are real `gossip`
/// aliases. `auctioneer` names art the install never shipped, so its row has no icon, in the
/// reference too.
const GOSSIP_ICON_TYPES: [Option<&str>; 21] = [
    Some("gossip"),
    Some("vendor"),
    Some("taxi"),
    Some("trainer"),
    Some("healer"),
    Some("binder"),
    Some("banker"),
    Some("petition"),
    Some("tabard"),
    Some("battlemaster"),
    Some("auctioneer"),
    Some("gossip"),
    Some("gossip"),
    Some("gossip"),
    None,
    None,
    Some("gossip"),
    Some("gossip"),
    Some("gossip"),
    Some("gossip"),
    Some("gossip"),
];

/// `ForceGossip`'s creature type flag (`0x715e1a`: the cached record's type flags, bit 27).
const TYPE_FLAG_FORCE_GOSSIP: u32 = 0x0800_0000;

/// The quest flag `GetGossipAvailableQuests` reads as `isDaily` (`0x58b423`).
const QUEST_FLAG_DAILY: u32 = 0x1000;

#[cfg(test)]
mod tests {
    use super::*;

    /// The one type in [`GOSSIP_ICON_TYPES`] with no art in the install.
    const GOSSIP_ICON_TYPE_UNSHIPPED: &str = "auctioneer";

    /// The reference's table (`0xacef20`), each index beside the `GOSSIP_OPTION_*` the world DB
    /// sends it with.
    #[test]
    fn the_icon_byte_indexes_the_clients_own_table() {
        assert_eq!(gossip_icon_type(0), "gossip");
        assert_eq!(gossip_icon_type(1), "vendor"); // GOSSIP_OPTION_VENDOR / _ARMORER
        assert_eq!(gossip_icon_type(2), "taxi"); // _TAXIVENDOR
        assert_eq!(gossip_icon_type(3), "trainer"); // _TRAINER
        assert_eq!(gossip_icon_type(4), "healer"); // _SPIRITHEALER / _SPIRITGUIDE
        assert_eq!(gossip_icon_type(5), "binder"); // _INNKEEPER
        assert_eq!(gossip_icon_type(6), "banker"); // _BANKER
        assert_eq!(gossip_icon_type(7), "petition"); // _PETITIONER
        assert_eq!(gossip_icon_type(8), "tabard"); // _TABARDDESIGNER
        assert_eq!(gossip_icon_type(9), "battlemaster"); // _BATTLEFIELD
        assert_eq!(gossip_icon_type(10), "auctioneer"); // _AUCTIONEER
        assert_eq!(gossip_icon_type(11), "gossip");
        assert_eq!(gossip_icon_type(13), "gossip");
        assert_eq!(gossip_icon_type(20), "gossip");
        // The deviation at the table's NULLs and past its end: see `gossip_icon_type`.
        assert_eq!(gossip_icon_type(14), "gossip");
        assert_eq!(gossip_icon_type(15), "gossip");
        assert_eq!(gossip_icon_type(21), "gossip");
        assert_eq!(gossip_icon_type(255), "gossip");
    }

    /// Stock `GossipFrame.lua:123` builds the icon path from the type, so every type has its BLP
    /// but `auctioneer`, which the install never shipped.
    #[test]
    fn gossip_icon_types_name_the_shipped_art() {
        let data = benilla_formats::wow_data_or_skip!();
        let chain = benilla_formats::open_chain(&data).expect("open chain");
        let blp = |ty: &str| format!("Interface\\GossipFrame\\{ty}GossipIcon.blp");
        for ty in GOSSIP_ICON_TYPES.into_iter().flatten() {
            if ty == GOSSIP_ICON_TYPE_UNSHIPPED {
                continue;
            }
            assert!(chain.contains(&blp(ty)), "{} is not on the chain", blp(ty));
        }
        assert!(
            !chain.contains(&blp(GOSSIP_ICON_TYPE_UNSHIPPED)),
            "the install has grown a {GOSSIP_ICON_TYPE_UNSHIPPED} gossip icon — give the XML its key"
        );
    }
}
