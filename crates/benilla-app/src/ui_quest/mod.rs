//! The questgiver window's app side: the handlers fill [`QuestGiver`] and publish it with its event ([`publish_panel`]),
//! and the panel's buttons send and close inside their call ([`act`], [`select`]).

use std::collections::HashMap;

use benilla_protocol::messages::{
    quest_flags, QuestDetails, QuestGiverList, QuestOfferReward, QuestRequestItems,
    QuestRewardItem, QuestShareMsg,
};
use bevy::prelude::*;

use benilla_ui::script::{
    QuestActOutcome, QuestAction, QuestItemView, QuestPanel, QuestRewardSpell, QuestSelect,
    QuestState, UiScript,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfPlayer};
#[cfg(test)]
use crate::ui_action::ui_error_text;
use crate::ui_action::{Spells, UiError};
use crate::ui_script::UiDrain;
use crate::ui_session::NpcSession;

/// The wire packet that opened the current panel.
pub(crate) enum QuestView {
    Greeting(QuestGiverList),
    Detail(QuestDetails),
    Progress(QuestRequestItems),
    Reward(QuestOfferReward),
}

/// The open questgiver window: main-thread state the panel's buttons write inside their call, lent to it ([`crate::ui_script::live::Lent`]).
#[derive(Default)]
pub(crate) struct QuestGiver {
    /// The panel's source: an NPC, a GameObject, a quest-starter item or a sharing player.
    pub(crate) npc: Option<u64>,
    pub(crate) view: Option<QuestView>,
    /// The reference's close-on-cancel latch, written by DETAILS and OFFER_REWARD and by
    /// REQUEST_ITEMS' `closeOnCancel`, and zeroed only on world enter. Non-zero turns a plain
    /// unit's decline into the silent teardown ([`decline_leg`]). vmangos sends 1 on every DETAILS
    /// and OFFER_REWARD; REQUEST_ITEMS carries 1 on a single-quest auto-open and 0 on
    /// `COMPLETE_QUEST` (`QuestHandler.cpp:390-395`).
    pub(crate) close_on_cancel: u32,
    /// The reference's already-acted latch, set by `AcceptQuest` and by a re-opening
    /// `DeclineQuest`, which keeps the panel up; both refuse while it is set until a panel publish
    /// clears it ([`Self::open`]).
    pub(crate) acted: bool,
    /// The reference's chosen-reward cell: the chosen reward's item id, latched by `GetQuestReward`
    /// for the turn-in's item line, and zeroed by a panel publish, world enter and the turn-in.
    pub(crate) chosen_reward: u32,
    /// A refusal handler's lines, shown in its turn, each a GlobalStrings key and its fills.
    messages: Vec<UiError>,
}

/// The per-unit dialog statuses `SMSG_QUESTGIVER_STATUS` stores on each unit (`[unit+0xcb8]`), apart from the window, so they outlive its close: the markers', the minimap's and the click's read.
#[derive(Resource, Default)]
pub(crate) struct QuestStatuses {
    /// Per-guid dialog status, the `!`/`?` marker's value.
    statuses: HashMap<u64, u32>,
    reask: u32,
    /// The dialog statuses dropped at the reference's typemask-8 lookup (GameObject answers, which
    /// vmangos sends), per guid, so a live probe can tell a drop from a query never sent.
    refused: HashMap<u64, u32>,
    refused_count: u32,
}

impl QuestGiver {
    /// Open (or replace) the window with a fresh wire view for `npc`.
    pub(crate) fn open(&mut self, npc: u64, view: QuestView) {
        self.npc = Some(npc);
        self.view = Some(view);
        // The reference clears the already-acted latch and the chosen reward on every panel
        // publish.
        self.acted = false;
        self.chosen_reward = 0;
    }

    pub(crate) fn is_open(&self) -> bool {
        self.view.is_some()
    }

    /// The `"questnpc"` unit (`0x6081b0`): the panel's inform guid, a detail panel's sharer, when
    /// set, else its source; nobody with the window closed.
    pub(crate) fn quest_npc(&self) -> Option<u64> {
        match self.view.as_ref()? {
            QuestView::Detail(d) if d.sharer != 0 => Some(d.sharer),
            _ => self.npc,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.npc = None;
        self.view = None;
    }

    /// The open panel's title if it shows `quest_id`: the `%s` of a refusal naming the quest.
    pub(crate) fn view_title(&self, quest_id: u32) -> Option<String> {
        match self.view.as_ref()? {
            QuestView::Detail(d) if d.quest_id == quest_id => Some(d.title.clone()),
            QuestView::Progress(p) if p.quest_id == quest_id => Some(p.title.clone()),
            QuestView::Reward(o) if o.quest_id == quest_id => Some(o.title.clone()),
            _ => None,
        }
    }

    /// Queue a refusal line; its key's catalog row picks the surface.
    pub(crate) fn push_message(&mut self, msg: UiError) {
        self.messages.push(msg);
    }

    pub(crate) fn take_messages(&mut self) -> Vec<UiError> {
        std::mem::take(&mut self.messages)
    }

    pub(crate) fn clear_session(&mut self) {
        self.clear();
        self.messages.clear();
        self.close_on_cancel = 0;
        self.acted = false;
        self.chosen_reward = 0;
    }
}

impl QuestStatuses {
    pub(crate) fn set_status(&mut self, npc: u64, status: u32) {
        self.statuses.insert(npc, status);
    }

    /// Every stored dialog status, which [`crate::quest_markers`] renders.
    pub(crate) fn statuses(&self) -> &HashMap<u64, u32> {
        &self.statuses
    }

    /// Drop the status of every guid `live` rejects: the reference keeps it on the unit
    /// (`unit+0xcb8`), so it dies with the object, and a map needs the prune.
    pub(crate) fn retain_statuses(&mut self, live: impl Fn(u64) -> bool) {
        self.statuses.retain(|&npc, _| live(npc));
        // The refusals follow the same lifetime.
        self.refused.retain(|&npc, _| live(npc));
    }

    /// Drop one guid's status when it stops being a questgiver, as the reference's teardown does.
    pub(crate) fn clear_status(&mut self, npc: u64) {
        self.statuses.remove(&npc);
    }

    /// Re-ask every visible questgiver's status, as the reference does after
    /// `SMSG_SET_FACTION_STANDING`, `SMSG_GROUP_LIST`, a turn-in and `SMSG_QUESTUPDATE_*`. A
    /// counter, folded into the marker query's generation, so no bump is lost to ordering.
    pub(crate) fn bump_reask(&mut self) {
        self.reask = self.reask.wrapping_add(1);
    }

    pub(crate) fn reask_epoch(&self) -> u32 {
        self.reask
    }

    pub(crate) fn refuse_status(&mut self, npc: u64, status: u32) {
        self.refused.insert(npc, status);
        self.refused_count = self.refused_count.saturating_add(1);
    }

    pub(crate) fn refused_for(&self, npc: u64) -> Option<u32> {
        self.refused.get(&npc).copied()
    }

    pub(crate) fn refused_count(&self) -> u32 {
        self.refused_count
    }

    /// One guid's stored dialog status; the markers read the whole map instead.
    #[allow(dead_code)]
    pub(crate) fn status(&self, npc: u64) -> Option<u32> {
        self.statuses.get(&npc).copied()
    }

    /// The session's end: every unit and its status go with it.
    pub(crate) fn clear_session(&mut self) {
        self.statuses.clear();
    }
}

/// `SMSG_QUESTGIVER_QUEST_INVALID`'s `QuestFailedReason` as a GlobalStrings key, per 12340's
/// jump table on `reason - 1` (`0x6cd7d2`-`0x6cd7e1`), whose default, `ERR_QUEST_NEED_PREREQS`,
/// takes reason 0 and everything unlisted.
pub(crate) fn questgiver_invalid_key(reason: u32) -> &'static str {
    match reason {
        1 => "ERR_QUEST_FAILED_LOW_LEVEL",                // msgId 0x97
        6 => "ERR_QUEST_FAILED_WRONG_RACE",               // msgId 0x99
        7 => "ERR_QUEST_ALREADY_DONE",                    // msgId 0x9f
        12 => "ERR_QUEST_ONLY_ONE_TIMED",                 // msgId 0x9c
        13 | 18 => "ERR_QUEST_ALREADY_ON",                // msgId 0x9e
        16 => "ERR_QUEST_FAILED_EXPANSION",               // msgId 0x9b
        21 => "ERR_QUEST_FAILED_MISSING_ITEMS",           // msgId 0x98
        23 => "ERR_QUEST_FAILED_NOT_ENOUGH_MONEY",        // msgId 0x9a
        26 => "ERR_QUEST_FAILED_TOO_MANY_DAILY_QUESTS_I", // msgId 0x22e, `%d` 25
        27 => "ERR_QUEST_FAILED_CAIS",                    // msgId 0x234
        29 => "ERR_QUEST_ALREADY_DONE_DAILY",             // msgId 0xa0
        _ => "ERR_QUEST_NEED_PREREQS",                    // msgId 0x9d, the default
    }
}

/// The daily cap the two `TOO_MANY_DAILY_QUESTS_I` arms print (`0x6cd827`, `0x6cdaf6`).
pub(crate) const DAILY_QUEST_CAP: i64 = 25;

/// `SMSG_QUESTGIVER_QUEST_FAILED`'s reason as a GlobalStrings key, per 12340's jump table on
/// `reason - 4` (`0x6cdab5`-`0x6cdac4`); each `%s` line's fill is the quest's title.
pub(crate) fn questgiver_failed_key(reason: u32) -> &'static str {
    match reason {
        4 | 16 | 50 => "ERR_QUEST_FAILED_BAG_FULL_S", // msgId 0x95
        17 => "ERR_QUEST_FAILED_MAX_COUNT_S",         // msgId 0x96
        74 => "ERR_QUEST_FAILED_TOO_MANY_DAILY_QUESTS_I", // msgId 0x22e, `%d` 25
        _ => "ERR_QUEST_FAILED_S",                    // msgId 0x94
    }
}

pub(crate) mod lines;
mod net;
pub(crate) mod rewards;

pub(crate) use lines::coin_text;

pub(crate) struct UiQuestPlugin;

impl Plugin for UiQuestPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        lines::register(app);
        app.init_non_send_resource::<QuestGiver>()
            .init_resource::<QuestStatuses>()
            .init_resource::<TrivialQuestTracking>()
            .add_systems(
                Update,
                (
                    // The range guard before the input pass; the drain after it, so a click goes out the same frame.
                    close_quest_out_of_range
                        .before(crate::ui_script::UiPress)
                        .before(crate::ui_unit::UnitFeed),
                    read_trivial_tracking.in_set(UiDrain),
                ),
            );
    }
}

/// Whether the minimap tracks low-level quests (`0x57bf30`), which shows the low-level quest
/// markers over their givers.
#[derive(Resource, Default, PartialEq)]
pub(crate) struct TrivialQuestTracking(pub(crate) bool);

/// Read the tracking choice a `SetTracking` in the tick made.
pub(crate) fn read_trivial_tracking(
    (script, _live): (Option<NonSend<UiScript>>, crate::ui_script::LendLive),
    mut tracking: ResMut<TrivialQuestTracking>,
) {
    let on = script.is_some_and(|s| s.tracking_trivial_quests());
    tracking.set_if_neq(TrivialQuestTracking(on));
}

impl NpcSession for QuestGiver {
    /// `None` for a quest-starter item, whose guid the range guard would read as a despawned giver
    /// and close the panel at once. The drain still sends to the item guid, which vmangos resolves
    /// as a giver (`QuestHandler.cpp:109`).
    fn npc(&self) -> Option<u64> {
        self.npc.filter(|g| !benilla_protocol::guid::is_item(*g))
    }

    fn close(&mut self) {
        self.clear();
    }
}

/// A greeting row is active when its wire icon is 3 or 4 and available otherwise, never by the
/// quest log (gossip's rows too). vmangos's 3 and 4 are `DIALOG_STATUS_INCOMPLETE` and
/// `_REWARD_REP` (`QuestDef.h:118-130`), and it marks an auto-complete quest, never in the log,
/// `_REWARD_REP` (`Player.cpp:12577`).
pub(crate) fn row_is_active(icon: u32) -> bool {
    matches!(icon, 3 | 4)
}

/// An available row with icon 0 is one-click: its select sends `COMPLETE_QUEST`, not `QUERY_QUEST`.
/// vmangos never sends icon 0 on this packet.
pub(crate) fn row_is_one_click(icon: u32) -> bool {
    icon == 0
}

/// One greeting pool as `(quest_id, wire icon)` rows; the icon feeds [`row_is_one_click`].
type Pool = Vec<(u32, u32)>;

/// One wire item row for Lua; the name and quality are `None` and white until the template lands.
fn resolve_item(
    it: &QuestRewardItem,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> QuestItemView {
    let template = items.template(it.item_id, 0, commands);
    let name = template.map(|t| t.name.clone());
    let quality = template.map(|t| t.quality).unwrap_or(1);
    let texture = crate::ui_items::item_icon(icons, it.display_id);
    // `GetQuestItemLink`'s payload; `None` until the template gives the name and quality.
    let link = name
        .as_ref()
        .map(|n| crate::ui_items::item_link(it.item_id, n, quality));
    QuestItemView {
        name,
        texture,
        count: it.count,
        quality,
        item_id: it.item_id,
        usable: true, // the getter's can-use test (`0x58d32c`) clears it
        link,
    }
}

fn resolve_items(
    src: &[QuestRewardItem],
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> Vec<QuestItemView> {
    src.iter()
        .map(|it| resolve_item(it, items, icons, commands))
        .collect()
}

/// The quest's reward spell from `rewSpell`, named through `Spell.dbc`; 0 is none, and an id the
/// catalog lacks still counts, with nothing to paint. `cast` is `rewSpellCast`, the spell cast at
/// turn-in.
pub(crate) fn reward_spell_view(
    spell_id: u32,
    cast: i32,
    spells: Option<&Spells>,
) -> Option<QuestRewardSpell> {
    if spell_id == 0 {
        return None;
    }
    let d = spells.and_then(|s| s.catalog.get(spell_id));
    Some(QuestRewardSpell {
        spell_id,
        name: d.map(|d| d.name.clone()),
        texture: d.and_then(|d| d.icon.clone()),
        // `isTradeskillSpell`: `Spell.dbc` col 6 `Attributes` bit `0x20`.
        tradeskill: d.is_some_and(|d| d.attributes & 0x20 != 0),
        // `isSpellLearned`: the cast spell has a learn-spell (36) or learn-pet-spell (57) effect.
        learned: u32::try_from(cast)
            .ok()
            .and_then(|c| spells?.catalog.get(c))
            .is_some_and(|c| c.effects.iter().any(|&e| e == 36 || e == 57)),
    })
}

fn snapshot(
    giver: &QuestGiver,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
    macros: &crate::npc_text::MacroContext,
    spells: Option<&Spells>,
) -> Option<QuestState> {
    // The wire sends quest text unexpanded; the client substitutes `$N`, `$B` and `$G`.
    let sub = |t: &str| crate::npc_text::substitute(t, macros);
    Some(match giver.view.as_ref()? {
        QuestView::Greeting(l) => QuestState {
            panel: QuestPanel::Greeting,
            greeting: sub(&l.greeting),
            greeting_quests: l
                .quests
                .iter()
                .map(|q| benilla_ui::script::GossipQuestRow {
                    quest_id: q.quest_id,
                    title: q.title.clone(),
                    // The wire's level is a signed word: -1 scales to the player.
                    level: q.level as i32,
                    active: row_is_active(q.icon),
                    daily: q.flags & quest_flags::DAILY != 0,
                    repeatable: q.repeatable,
                })
                .collect(),
            ..Default::default()
        },
        QuestView::Detail(d) => QuestState {
            panel: QuestPanel::Detail,
            title: sub(&d.title),
            body: sub(&d.details),
            objectives: sub(&d.objectives),
            choices: resolve_items(&d.choices, items, icons, commands),
            rewards: resolve_items(&d.rewards, items, icons, commands),
            reward_money: d.extra.money.max(0) as u32,
            // A negative reward is a cost: `GetQuestMoneyToGet` (`0x58de68`-`0x58de70`).
            required_money: d.extra.money.min(0).unsigned_abs(),
            reward_spell: reward_spell_view(d.extra.spell, d.extra.spell_cast, spells),
            ..Default::default()
        },
        QuestView::Progress(p) => QuestState {
            panel: QuestPanel::Progress,
            title: sub(&p.title),
            body: sub(&p.request_text),
            required: resolve_items(&p.required_items, items, icons, commands),
            required_money: p.required_money,
            completable: p.is_complete,
            ..Default::default()
        },
        QuestView::Reward(o) => QuestState {
            panel: QuestPanel::Reward,
            title: sub(&o.title),
            body: sub(&o.offer_text),
            choices: resolve_items(&o.choices, items, icons, commands),
            rewards: resolve_items(&o.rewards, items, icons, commands),
            reward_money: o.extra.money.max(0) as u32,
            // The cost the stock Complete button confirms (`QuestFrame.lua:94-101`).
            required_money: o.extra.money.min(0).unsigned_abs(),
            reward_spell: reward_spell_view(o.extra.spell, o.extra.spell_cast, spells),
            ..Default::default()
        },
    })
}

fn panel_event(panel: QuestPanel) -> &'static str {
    match panel {
        QuestPanel::Greeting => "QUEST_GREETING",
        QuestPanel::Detail => "QUEST_DETAIL",
        QuestPanel::Progress => "QUEST_PROGRESS",
        QuestPanel::Reward => "QUEST_COMPLETE",
    }
}

/// The panel as its publisher stores it (`0x58dd50`, `0x58ba80`): the text, the item rows as the item cache answers now, the extras and flags.
#[allow(clippy::too_many_arguments)] // one Bevy system's full input set
fn panel_state(
    giver: NonSend<QuestGiver>,
    objects: crate::net::Objects,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    commands: Res<NetCommands>,
    names: Res<NameCache>,
    states: Res<crate::world_state::WorldStates>,
    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    spells: Option<Res<Spells>>,
    (go_templates, materials): (
        Res<crate::go_templates::GameObjectTemplates>,
        Option<Res<crate::ui_item_text::PageMaterials>>,
    ),
    // A PvP realm (`Cfg_Configs.dbc`'s `PlayerKillingAllowed`), where `QuestFlagsPVP` answers nil (set at `0x4055c7`).
    (rewards, roster): (
        rewards::RewardInputs,
        Option<Res<crate::char_select::Roster>>,
    ),
) -> Option<QuestState> {
    let player = crate::npc_text::player_identity(&self_q, &names, &commands);
    let mut st = snapshot(
        &giver,
        &items,
        icons.as_deref(),
        &commands,
        &crate::npc_text::MacroContext {
            subject: player.as_ref(),
            states: &states,
            achievement: None,
        },
        spells.as_deref(),
    )?;
    // The background is the source object's material, never a creature's and never on the wire (`GetQuestBackgroundMaterial`).
    st.background_material = giver.npc.and_then(|source| {
        crate::ui_item_text::object_material(
            source,
            &objects,
            &items,
            &go_templates,
            materials.as_deref(),
            &commands,
        )
    });
    let (Some(view), Some((store, _))) = (giver.view.as_ref(), self_q.iter().next()) else {
        return Some(st);
    };
    let name = player.as_ref().map_or("", |p| p.name.as_str());
    let (extra, flags, suggested) = match view {
        QuestView::Detail(d) => (Some(&d.extra), d.flags, d.suggested_players),
        QuestView::Reward(o) => (Some(&o.extra), o.quest_flags, o.suggested_players),
        QuestView::Progress(p) => (None, p.quest_flags, p.suggested_players),
        QuestView::Greeting(_) => (None, 0, 0),
    };
    if let Some(e) = extra {
        st.extras = rewards.for_panel(e, &store.0, name);
    }
    st.suggested_group = suggested;
    st.flags_pvp =
        flags & quest_flags::FLAGS_PVP != 0 && !roster.as_ref().is_some_and(|r| r.realm_is_pvp());
    st.auto_accept = flags & quest_flags::AUTO_ACCEPT != 0;
    Some(st)
}

/// A panel packet's publish and event (`0x58dfb5`, `0x58bb4c`, `0x58dfc9`, `0x58ba77`), in the handler's turn; a different giver's open window closes first (`0x58c6cc`).
pub(crate) fn publish_panel(world: &mut World) {
    let state = match world.run_system_cached(panel_state) {
        Ok(state) => state,
        Err(e) => {
            warn!("ui_quest: the panel was not published: {e}");
            return;
        }
    };
    crate::ui_script::live::with_live_world(world, |script| {
        let event = state.as_ref().map(|s| panel_event(s.panel));
        script.set_quest(state);
        // The panel events carry no argument: the frame reads `UnitName("npc")` (`QuestFrame.lua:63`).
        if let Some(event) = event {
            script.fire_event(event, vec![]);
        }
    });
}

/// The questgiver window's close (`0x58ca70`): nothing with no giver (`0x58ca85`); a decline owed to a player in view (`0x58ca91`-`0x58cb1f`); unless `keep_open`, the giver zeroed and `QUEST_FINISHED` fired (`0x58cb6b`, `0x58cb98`). The panel's text stays, as the publisher's globals do.
pub(crate) fn giver_finished(world: &mut World, keep_open: bool, quest: u32, decline: bool) {
    let Some(giver) = world.get_non_send_resource::<QuestGiver>() else {
        return;
    };
    let Some(npc) = giver.npc else {
        return;
    };
    let in_view = world
        .get_resource::<GuidIndex>()
        .is_some_and(|i| i.0.contains_key(&npc));
    if decline && benilla_protocol::guid::is_player(npc) && in_view {
        let to = giver.quest_npc().unwrap_or(npc);
        if let Some(net) = world.get_resource::<NetCommands>() {
            let _ = net.0.send(ClientCommand::QuestPushResult {
                sharer: to,
                quest,
                msg: QuestShareMsg::DECLINE_QUEST,
            });
        }
    }
    if keep_open {
        return;
    }
    world.non_send_resource_mut::<QuestGiver>().clear();
    crate::ui_script::live::with_live_world(world, |script| {
        script.fire_event("QUEST_FINISHED", vec![]);
    });
}

/// The interaction close's quest leg (`0x512f16`): the walk-away and every other window's close end the quest window with a decline owed (`0x512f1a`).
pub(crate) fn close_interaction(world: &mut World) {
    giver_finished(world, false, 0, true);
}

/// The range guard (`0x512e60`'s watchdog) for the quest window: a walk-away or a despawned giver closes it, applied at the sync point before the input pass.
pub(crate) fn close_quest_out_of_range(
    giver: NonSend<QuestGiver>,
    index: Res<GuidIndex>,
    self_q: Query<&Transform, With<SelfPlayer>>,
    transforms: Query<&Transform>,
    mut commands: Commands,
) {
    let out = giver.npc().is_some_and(|npc| {
        crate::ui_session::npc_out_of_range(npc, &index, &self_q, &transforms) == Some(true)
    });
    if out {
        commands.queue(close_interaction);
    }
}

/// The close's decline owed to a sharing player (`0x58cadc`-`0x58cb09`): to the inform guid when set, else the source, naming the open quest. The player test is the guid's shape; the reference's also needs the sharer in view, which the range guard ensures.
fn close_quest(npc: u64, reply_to: u64, quest: u32, commands: &NetCommands) {
    if benilla_protocol::guid::is_player(npc) {
        debug!("ui_quest: closing {npc:#x}'s shared quest {quest} — declining it");
        let _ = commands.0.send(ClientCommand::QuestPushResult {
            sharer: reply_to,
            quest,
            msg: QuestShareMsg::DECLINE_QUEST,
        });
    }
}

/// The leg of `DeclineQuest()`'s core a source takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeclineLeg {
    /// The already-acted latch is set or the source does not resolve.
    Nothing,
    /// The teardown: declines to a sharer, closes the window.
    Teardown,
    /// A gossip-flagged unit: `CMSG_GOSSIP_HELLO`; the window stays.
    GossipHello,
    /// A plain unit: `CMSG_QUESTGIVER_HELLO`; the window stays.
    QuestgiverHello,
    /// A GameObject: its right-click, the use; the window stays.
    UseObject,
}

/// `DeclineQuest`'s fork; the re-open legs leave the window up, and items and players are taken by
/// guid shape. In order:
///
/// 1. the already-acted latch set, or the source unresolvable: nothing;
/// 2. a unit with the gossip NPC flag: `CMSG_GOSSIP_HELLO`, whatever the close-on-cancel latch
///    says;
/// 3. an item, a player, or a non-zero close-on-cancel latch: the teardown;
/// 4. a GameObject: its right-click `[vtbl+0xb0]` (`0x711140`), the use (`0x58ceb2`);
/// 5. anything else: `CMSG_QUESTGIVER_HELLO`.
fn decline_leg(npc: u64, acted: bool, close_on_cancel: u32, npc_flags: Option<u32>) -> DeclineLeg {
    use benilla_protocol::guid;
    if acted {
        return DeclineLeg::Nothing;
    }
    if guid::is_player(npc) || guid::is_item(npc) {
        return DeclineLeg::Teardown;
    }
    let Some(flags) = npc_flags else {
        return DeclineLeg::Nothing;
    };
    let unit = guid::is_creature_or_pet(npc);
    if unit && flags & crate::target::cursor_mode::npc_flags::GOSSIP != 0 {
        return DeclineLeg::GossipHello;
    }
    if close_on_cancel != 0 {
        return DeclineLeg::Teardown;
    }
    if guid::is_gameobject(npc) {
        return DeclineLeg::UseObject;
    }
    DeclineLeg::QuestgiverHello
}

/// The open panel's quest, `None` on a greeting.
fn view_quest(giver: &QuestGiver) -> Option<u32> {
    giver.view.as_ref().and_then(|v| match v {
        QuestView::Detail(d) => Some(d.quest_id),
        QuestView::Progress(p) => Some(p.quest_id),
        QuestView::Reward(o) => Some(o.quest_id),
        QuestView::Greeting(_) => None,
    })
}

/// The close's teardown inside a button's call (`0x58ca70`): the decline owed, the giver zeroed, `QUEST_FINISHED` owed (`0x58cb6b`, `0x58cb98`).
fn teardown(giver: &mut QuestGiver, npc: u64, commands: &NetCommands) -> QuestActOutcome {
    let reply_to = giver.quest_npc().unwrap_or(npc);
    close_quest(npc, reply_to, view_quest(giver).unwrap_or(0), commands);
    giver.clear();
    QuestActOutcome {
        finished: true,
        ..Default::default()
    }
}

/// `SelectAvailableQuest` (`0x58cc20`) and `SelectActiveQuest` (`0x58ccb0`) inside their call: the greeting row's send, refused while the already-acted latch is set (`0x58cc2f`, `0x58ccbf`).
pub(crate) fn select(
    giver: &mut QuestGiver,
    sel: QuestSelect,
    commands: &NetCommands,
) -> QuestActOutcome {
    let none = QuestActOutcome::default();
    let (Some(npc), Some(QuestView::Greeting(list))) = (giver.npc, giver.view.as_ref()) else {
        return none;
    };
    let (mut active, mut available): (Pool, Pool) = (Vec::new(), Vec::new());
    for q in &list.quests {
        if row_is_active(q.icon) {
            active.push((q.quest_id, q.icon));
        } else {
            available.push((q.quest_id, q.icon));
        }
    }
    let pool = if sel.active { &active } else { &available };
    let Some(&(quest, icon)) = sel.index.checked_sub(1).and_then(|i| pool.get(i as usize)) else {
        debug!("ui_quest: greeting select {sel:?} out of range — ignored");
        return none;
    };
    if giver.acted {
        return none;
    }
    // `SelectActiveQuest` always sends COMPLETE_QUEST, after its tutorial (`0x58ccea`); `SelectAvailableQuest` does too for a one-click row, else QUERY_QUEST.
    let cmd = if sel.active || row_is_one_click(icon) {
        ClientCommand::QuestgiverComplete { npc, quest }
    } else {
        ClientCommand::QuestgiverQuery { npc, quest }
    };
    let _ = commands.0.send(cmd);
    giver.acted = true;
    QuestActOutcome {
        tutorial: sel.active.then_some(crate::tutorial::id::QUEST_TURN_IN),
        ..Default::default()
    }
}

/// A panel button inside its call: `AcceptQuest` (`0x58cd20`), `DeclineQuest` (`0x58ce00`), `CloseQuest` (`0x58d35a` -> `0x58ca70`), `CompleteQuest` (`0x58cf40`) and `GetQuestReward` (`0x58cfa0`); `npc_flags` is the giver's live `UNIT_NPC_FLAGS`, `None` when it is not held.
pub(crate) fn act(
    giver: &mut QuestGiver,
    action: QuestAction,
    commands: &NetCommands,
    npc_flags: Option<u32>,
) -> QuestActOutcome {
    let none = QuestActOutcome::default();
    let Some(npc) = giver.npc else {
        return none;
    };
    let quest = view_quest(giver);
    match action {
        QuestAction::Close => teardown(giver, npc, commands),
        QuestAction::Decline => {
            let leg = decline_leg(npc, giver.acted, giver.close_on_cancel, npc_flags);
            debug!("ui_quest: decline on {npc:#x} — {leg:?}");
            match leg {
                DeclineLeg::Nothing => none,
                // `0x58ce9d`.
                DeclineLeg::Teardown => teardown(giver, npc, commands),
                DeclineLeg::GossipHello => {
                    let _ = commands.0.send(ClientCommand::GossipHello { guid: npc });
                    giver.acted = true;
                    none
                }
                DeclineLeg::QuestgiverHello => {
                    let _ = commands.0.send(ClientCommand::QuestgiverHello { npc });
                    giver.acted = true;
                    none
                }
                DeclineLeg::UseObject => {
                    giver.acted = true;
                    QuestActOutcome {
                        use_object: Some(npc),
                        ..Default::default()
                    }
                }
            }
        }
        // Refused while the already-acted latch is set, and off the detail panel (`0x58cd26`).
        QuestAction::Accept => {
            let Some(QuestView::Detail(d)) = giver.view.as_ref().filter(|_| !giver.acted) else {
                return none;
            };
            let Some(quest) = quest else {
                return none;
            };
            // A shared quest is accepted toward its sharer, the details' start-cheat byte riding along (`0x58cd65`, `0x58cd7d`).
            let to = if d.sharer != 0 { d.sharer } else { npc };
            let start_cheat = u32::from(d.start_cheat);
            let _ = commands.0.send(ClientCommand::QuestgiverAccept {
                npc: to,
                quest,
                start_cheat,
            });
            giver.acted = true;
            // The send, the giver zeroed, `QUEST_FINISHED` in the call (`0x58cdc5`, `0x58cded`).
            // With no gossip held, 12340 clears the interact target here (`0x58cdb7`, `0x58cde4` -> `0x518d50`); the giver's clear frees [`crate::ui_session::InteractNpc`], whose clear plays the goodbye and re-picks the base animation.
            giver.clear();
            QuestActOutcome {
                finished: true,
                ..Default::default()
            }
        }
        // Each sends once per panel, from its own panel, after its tutorial (`0x58cf40`, `0x58cf76`).
        QuestAction::Continue => {
            if giver.acted || !matches!(giver.view, Some(QuestView::Progress(_))) {
                return none;
            }
            let Some(quest) = quest else {
                return none;
            };
            giver.acted = true;
            let _ = commands
                .0
                .send(ClientCommand::QuestgiverRequestReward { npc, quest });
            QuestActOutcome {
                tutorial: Some(crate::tutorial::id::QUEST_TURN_IN),
                ..Default::default()
            }
        }
        // `0x58cfa3`, the tutorial at `0x58d00d`.
        QuestAction::Reward(choice) => {
            let Some(QuestView::Reward(o)) = giver.view.as_ref().filter(|_| !giver.acted) else {
                return none;
            };
            let Some(quest) = quest else {
                return none;
            };
            // The chosen row's item is latched when the panel offers a choice.
            let chosen = o.choices.get(choice as usize).map(|it| it.item_id);
            giver.acted = true;
            if let Some(item) = chosen {
                giver.chosen_reward = item;
            }
            let _ = commands
                .0
                .send(ClientCommand::QuestgiverChooseReward { npc, quest, choice });
            QuestActOutcome {
                tutorial: Some(crate::tutorial::id::QUEST_TURN_IN),
                ..Default::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::dialog_status;

    fn triple(id: u32) -> QuestRewardItem {
        QuestRewardItem {
            item_id: id,
            count: 1,
            display_id: 100 + id,
        }
    }

    #[test]
    fn greeting_split_reads_the_wire_icon() {
        // 3 = DIALOG_STATUS_INCOMPLETE, 4 = DIALOG_STATUS_REWARD_REP.
        assert!(row_is_active(3));
        assert!(row_is_active(4));
        // 5 = DIALOG_STATUS_AVAILABLE, 2 = DIALOG_STATUS_CHAT.
        assert!(!row_is_active(5));
        assert!(!row_is_active(2));
        // Every other value is available, including ones vmangos never sends.
        for icon in [0, 1, 6, 7, 100, u32::MAX] {
            assert!(!row_is_active(icon), "icon {icon} must fall to AVAILABLE");
        }
    }

    /// vmangos marks an auto-complete quest's row `DIALOG_STATUS_REWARD_REP` though the quest is
    /// never in the log (`Player.cpp:12577`).
    #[test]
    fn an_autocomplete_row_is_active_though_it_is_not_in_the_log() {
        const REWARD_REP: u32 = 4;
        assert!(
            row_is_active(REWARD_REP),
            "an auto-complete turn-in must bin ACTIVE without ever entering the quest log"
        );
        assert!(!row_is_one_click(REWARD_REP), "it is active, not one-click");
    }

    #[test]
    fn the_one_click_flag_is_icon_zero_alone() {
        assert!(row_is_one_click(0));
        for icon in [1, 2, 3, 4, 5, u32::MAX] {
            assert!(!row_is_one_click(icon));
        }
    }

    #[test]
    fn an_item_giver_is_not_an_npc_session() {
        let item = 0x4000_0000_0000_0BAD_u64; // HIGHGUID_ITEM
        let creature = 0xF130_0000_00C5_0001_u64; // HIGHGUID_UNIT
        let detail = |npc: u64| {
            QuestView::Detail(QuestDetails {
                npc,
                quest_id: 373,
                title: "The Unsent Letter".into(),
                details: String::new(),
                objectives: String::new(),
                auto_launched: false,
                choices: Vec::new(),
                rewards: Vec::new(),
                ..Default::default()
            })
        };
        let mut giver = QuestGiver::default();
        giver.open(item, detail(item));
        assert_eq!(NpcSession::npc(&giver), None, "an item is not a live NPC");
        assert_eq!(giver.npc, Some(item), "the wire address is unchanged");
        giver.open(creature, detail(creature));
        assert_eq!(NpcSession::npc(&giver), Some(creature));
    }

    #[test]
    fn detail_snapshot_carries_text_and_rows() {
        let mut giver = QuestGiver::default();
        giver.open(
            0x42,
            QuestView::Detail(QuestDetails {
                npc: 0x42,
                quest_id: 100,
                title: "A Threat Within".into(),
                details: "Kill kobolds, $N.".into(),
                objectives: "Slay 10.".into(),
                auto_launched: true,
                choices: vec![triple(2000)],
                rewards: vec![triple(3000)],
                extra: benilla_protocol::messages::QuestExtraRewards {
                    money: 1234,
                    ..Default::default()
                },
                ..Default::default()
            }),
        );
        assert!(giver.is_open());
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let player = crate::npc_text::Subject {
            name: "Tri".into(),
            race: 1,
            class: 1,
            gender: 0,
        };
        let snap = snapshot(
            &giver,
            &items,
            None,
            &commands,
            &crate::npc_text::MacroContext {
                subject: Some(&player),
                states: &crate::world_state::WorldStates::default(),
                achievement: None,
            },
            None,
        )
        .expect("open");
        assert_eq!(snap.panel, QuestPanel::Detail);
        assert_eq!(snap.title, "A Threat Within");
        assert_eq!(snap.body, "Kill kobolds, Tri.");
        assert_eq!(snap.objectives, "Slay 10.");
        assert_eq!(snap.choices.len(), 1);
        assert_eq!(snap.rewards.len(), 1);
        assert_eq!(snap.reward_money, 1234);
        // No template yet: no name, and white.
        assert!(snap.rewards[0].name.is_none());
        assert_eq!(snap.rewards[0].quality, 1);
    }

    #[test]
    fn progress_snapshot_carries_completability() {
        let mut giver = QuestGiver::default();
        giver.open(
            0x42,
            QuestView::Progress(QuestRequestItems {
                npc: 0x42,
                quest_id: 100,
                title: "A Threat Within".into(),
                request_text: "Bring me the tusks.".into(),
                emote_delay: 0,
                emote: 0,
                close_on_cancel: 1,
                quest_flags: 0,
                suggested_players: 0,
                required_money: 500,
                required_items: vec![triple(2001)],
                is_complete: true,
            }),
        );
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let snap = snapshot(
            &giver,
            &items,
            None,
            &commands,
            &crate::npc_text::MacroContext {
                subject: None,
                states: &crate::world_state::WorldStates::default(),
                achievement: None,
            },
            None,
        )
        .unwrap();
        assert_eq!(snap.panel, QuestPanel::Progress);
        assert_eq!(snap.required.len(), 1);
        assert_eq!(snap.required_money, 500);
        assert!(snap.completable);
    }

    #[test]
    fn status_store_survives_close() {
        let (mut giver, mut statuses) = (QuestGiver::default(), QuestStatuses::default());
        statuses.set_status(0x99, dialog_status::AVAILABLE);
        giver.open(
            0x99,
            QuestView::Greeting(QuestGiverList {
                npc: 0x99,
                greeting: "Hi".into(),
                emote_delay: 0,
                emote: 0,
                quests: vec![],
            }),
        );
        giver.clear();
        assert!(!giver.is_open());
        assert_eq!(statuses.status(0x99), Some(dialog_status::AVAILABLE));
    }

    /// Reasons 2 to 5 sit inside the handler's jump table and still take its default.
    #[test]
    fn questgiver_invalid_keys_match_the_reference_table() {
        assert_eq!(questgiver_invalid_key(13), "ERR_QUEST_ALREADY_ON");
        assert_eq!(questgiver_invalid_key(1), "ERR_QUEST_FAILED_LOW_LEVEL");
        assert_eq!(questgiver_invalid_key(6), "ERR_QUEST_FAILED_WRONG_RACE");
        assert_eq!(questgiver_invalid_key(12), "ERR_QUEST_ONLY_ONE_TIMED");
        assert_eq!(questgiver_invalid_key(21), "ERR_QUEST_FAILED_MISSING_ITEMS");
        assert_eq!(
            questgiver_invalid_key(23),
            "ERR_QUEST_FAILED_NOT_ENOUGH_MONEY"
        );
        assert_eq!(questgiver_invalid_key(7), "ERR_QUEST_ALREADY_DONE");
        assert_eq!(questgiver_invalid_key(16), "ERR_QUEST_FAILED_EXPANSION");
        assert_eq!(questgiver_invalid_key(18), "ERR_QUEST_ALREADY_ON");
        assert_eq!(
            questgiver_invalid_key(26),
            "ERR_QUEST_FAILED_TOO_MANY_DAILY_QUESTS_I"
        );
        assert_eq!(questgiver_invalid_key(27), "ERR_QUEST_FAILED_CAIS");
        assert_eq!(questgiver_invalid_key(29), "ERR_QUEST_ALREADY_DONE_DAILY");
        for reason in [0, 2, 3, 4, 5, 11, 14, 19, 20, 22, 28, 30, 99, u32::MAX] {
            assert_eq!(
                questgiver_invalid_key(reason),
                "ERR_QUEST_NEED_PREREQS",
                "reason {reason} must take the ref's `ja` default"
            );
        }
    }

    #[test]
    fn questgiver_failed_keys_match_the_reference_table() {
        assert_eq!(questgiver_failed_key(4), "ERR_QUEST_FAILED_BAG_FULL_S");
        assert_eq!(questgiver_failed_key(50), "ERR_QUEST_FAILED_BAG_FULL_S");
        assert_eq!(questgiver_failed_key(16), "ERR_QUEST_FAILED_BAG_FULL_S");
        assert_eq!(questgiver_failed_key(17), "ERR_QUEST_FAILED_MAX_COUNT_S");
        assert_eq!(
            questgiver_failed_key(74),
            "ERR_QUEST_FAILED_TOO_MANY_DAILY_QUESTS_I"
        );
        for reason in [0, 1, 5, 18, 49, 51, 73, 75] {
            assert_eq!(questgiver_failed_key(reason), "ERR_QUEST_FAILED_S");
        }
    }

    #[test]
    fn view_title_answers_only_for_the_open_quest() {
        let mut giver = QuestGiver::default();
        assert_eq!(giver.view_title(100), None);
        giver.open(
            0x42,
            QuestView::Detail(QuestDetails {
                npc: 0x42,
                quest_id: 100,
                title: "A Threat Within".into(),
                details: String::new(),
                objectives: String::new(),
                auto_launched: false,
                choices: vec![],
                rewards: vec![],
                ..Default::default()
            }),
        );
        assert_eq!(giver.view_title(100).as_deref(), Some("A Threat Within"));
        assert_eq!(giver.view_title(101), None);
    }

    /// Reads the install's `GlobalStrings.lua`; skips without it.
    #[test]
    fn every_quest_refusal_key_resolves_in_the_real_global_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| s.lua().globals().get::<String>(key).ok();

        for reason in 0..=60u32 {
            for key in [
                questgiver_invalid_key(reason),
                questgiver_failed_key(reason),
            ] {
                let text = g(key).unwrap_or_default();
                assert!(!text.is_empty(), "{key} (reason {reason}) missing");
            }
            // The FAILED family names the quest; the INVALID family never does.
            assert!(g(questgiver_failed_key(reason))
                .unwrap_or_default()
                .contains("%s"));
            assert!(!g(questgiver_invalid_key(reason))
                .unwrap_or_default()
                .contains("%s"));
        }
        assert_eq!(
            ui_error_text(&UiError::key(questgiver_invalid_key(13)), &g).as_deref(),
            Some("You are already on that quest")
        );
        assert_eq!(
            ui_error_text(&UiError::s(questgiver_failed_key(4), "A Threat Within"), &g).as_deref(),
            Some("A Threat Within failed: Inventory is full.")
        );
        // The log-full line takes no fill.
        assert_eq!(
            ui_error_text(&UiError::key("ERR_QUEST_LOG_FULL"), &g).as_deref(),
            Some("Your quest log is full.")
        );
    }

    // ── DeclineQuest and CloseQuest ──────────────────────────────────────────────

    /// Run `lua` on a window open on `giver` with the close-on-cancel latch at `close_on_cancel`
    /// and return the sends; `npc_flags` streams the giver with those flags, `None` leaves it
    /// unstreamed.
    fn drain_after(
        giver: u64,
        npc_flags: Option<u32>,
        close_on_cancel: u32,
        lua: &str,
    ) -> Vec<ClientCommand> {
        drain_world(giver, npc_flags, close_on_cancel, lua).1
    }

    /// [`drain_after`], keeping the world so a test can read the [`QuestGiver`] afterwards.
    fn drain_world(
        giver: u64,
        npc_flags: Option<u32>,
        close_on_cancel: u32,
        lua: &str,
    ) -> (App, Vec<ClientCommand>) {
        let view = QuestView::Detail(QuestDetails {
            npc: giver,
            quest_id: 1,
            title: "A Threat Within".into(),
            details: String::new(),
            objectives: String::new(),
            auto_launched: close_on_cancel != 0,
            choices: Vec::new(),
            rewards: Vec::new(),
            ..Default::default()
        });
        drain_view(giver, npc_flags, close_on_cancel, view, lua)
    }

    /// [`drain_world`] on any panel.
    fn drain_view(
        giver: u64,
        npc_flags: Option<u32>,
        close_on_cancel: u32,
        view: QuestView,
        lua: &str,
    ) -> (App, Vec<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx));
        app.init_resource::<GuidIndex>();
        if let Some(flags) = npc_flags {
            // 147 = `UNIT_NPC_FLAGS` (the descriptor index `unit_npc_flags()` reads).
            let fields = benilla_protocol::ObjectFields::from_pairs(&[(
                benilla_protocol::field::FIELD_UNIT_NPC_FLAGS,
                flags,
            )]);
            let e = app.world_mut().spawn(ObjectStore(fields)).id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(giver, e);
        }
        let mut quest = QuestGiver::default();
        quest.open(giver, view);
        // The latch is written by the packet, after the open (as `net::quest_detail` does).
        quest.close_on_cancel = close_on_cancel;
        app.insert_non_send_resource(quest);
        app.insert_non_send_resource(UiScript::new().unwrap());
        // The verbs send and close inside the chunk's own calls.
        crate::ui_script::live::with_live_world(app.world_mut(), |s| s.run(lua).unwrap());
        let sent = rx.try_iter().collect();
        (app, sent)
    }

    /// The tutorials the calls triggered, as `TriggerTutorial` queues them (`0x530840`).
    fn tutorials(app: &mut App) -> Vec<u32> {
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .take_tutorial_triggers()
    }

    /// The GameObject uses the calls queued, in call order.
    fn object_uses(app: &mut App) -> Vec<u64> {
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .take_script_calls()
            .into_iter()
            .filter_map(|c| match c {
                benilla_ui::script::ScriptCall::InteractObject(guid) => Some(guid),
                _ => None,
            })
            .collect()
    }

    /// 12340's accept raises no tutorial (`0x58cd20`); the three turn-in verbs raise theirs before
    /// the send (`0x58ccea`, `0x58cf74`, `0x58d00b`), and an available row's select raises none.
    #[test]
    fn the_turn_in_verbs_raise_the_tutorial_and_accept_does_not() {
        use crate::tutorial::id::QUEST_TURN_IN;
        let (mut app, sent) = drain_world(NPC, Some(0), 0, "AcceptQuest()");
        assert!(matches!(
            sent.as_slice(),
            [ClientCommand::QuestgiverAccept { .. }]
        ));
        assert!(tutorials(&mut app).is_empty(), "accept triggers nothing");

        let greeting = || {
            QuestView::Greeting(benilla_protocol::messages::QuestGiverList {
                npc: NPC,
                quests: vec![
                    benilla_protocol::messages::QuestListEntry {
                        quest_id: 5,
                        icon: 3,
                        ..Default::default()
                    },
                    benilla_protocol::messages::QuestListEntry {
                        quest_id: 6,
                        icon: 8,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            })
        };
        let (mut app, _) = drain_view(NPC, Some(0), 0, greeting(), "SelectActiveQuest(1)");
        assert_eq!(tutorials(&mut app), [QUEST_TURN_IN]);
        let (mut app, _) = drain_view(NPC, Some(0), 0, greeting(), "SelectAvailableQuest(1)");
        assert!(tutorials(&mut app).is_empty());

        let progress = QuestView::Progress(QuestRequestItems {
            npc: NPC,
            quest_id: 1,
            is_complete: true,
            ..Default::default()
        });
        let (mut app, sent) = drain_view(NPC, Some(0), 0, progress, "CompleteQuest()");
        assert!(matches!(
            sent.as_slice(),
            [ClientCommand::QuestgiverRequestReward { .. }]
        ));
        assert_eq!(tutorials(&mut app), [QUEST_TURN_IN]);

        let reward = QuestView::Reward(benilla_protocol::messages::QuestOfferReward {
            npc: NPC,
            quest_id: 1,
            ..Default::default()
        });
        let (mut app, sent) = drain_view(NPC, Some(0), 0, reward, "GetQuestReward(0)");
        assert!(matches!(
            sent.as_slice(),
            [ClientCommand::QuestgiverChooseReward { .. }]
        ));
        assert_eq!(tutorials(&mut app), [QUEST_TURN_IN]);
    }

    /// A GameObject's decline re-uses the object through its right-click and keeps the window
    /// (`0x58ceab`-`0x58cebf`); with the close-on-cancel latch set it takes the teardown.
    #[test]
    fn declining_a_gameobjects_quest_uses_the_object_again() {
        const CHEST: u64 = 0xF110_0000_0000_0042; // HIGHGUID_GAMEOBJECT
        let (mut app, sent) = drain_world(CHEST, Some(0), 0, "DeclineQuest() DeclineQuest()");
        assert!(sent.is_empty(), "{sent:?}");
        assert_eq!(object_uses(&mut app), [CHEST], "once: the acted latch");
        assert!(app.world().non_send_resource::<QuestGiver>().is_open());

        let (mut app, sent) = drain_world(CHEST, Some(0), 1, "DeclineQuest()");
        assert!(sent.is_empty(), "{sent:?}");
        assert!(object_uses(&mut app).is_empty());
        assert!(!app.world().non_send_resource::<QuestGiver>().is_open());
    }

    const SHARER: u64 = 0x0000_0000_0000_002A; // HIGHGUID_PLAYER: a zero high word
    const NPC: u64 = 0xF130_0000_0000_0007; // HIGHGUID_UNIT

    /// `AcceptQuest` clears the interact target unless a gossip menu holds one (`0x58cdb7`, `0x58cde4` -> `0x518d50`): the giver's clear frees [`crate::ui_session::InteractNpc`], which the goodbye and the base re-pick read, and a gossip menu on the NPC keeps it.
    #[test]
    fn accept_clears_the_interact_target_unless_a_gossip_menu_holds_it() {
        use crate::ui_gossip::GossipState;
        use crate::ui_session::{feed_interact_npc, InteractNpc};
        for gossip in [None, Some(NPC)] {
            let (mut app, _) = drain_world(NPC, Some(0), 0, "");
            app.init_resource::<InteractNpc>()
                .init_resource::<GossipState>()
                .add_systems(Update, feed_interact_npc);
            app.world_mut().resource_mut::<GossipState>().npc = gossip;
            app.update();
            assert_eq!(app.world().resource::<InteractNpc>().1, Some(NPC));
            crate::ui_script::live::with_live_world(app.world_mut(), |s| {
                s.run("AcceptQuest()").unwrap()
            });
            assert!(!app.world().non_send_resource::<QuestGiver>().is_open());
            app.update();
            assert_eq!(
                app.world().resource::<InteractNpc>().1,
                gossip,
                "the target clears only with no gossip menu on it"
            );
        }
    }

    /// Both verbs reach the teardown: `CloseQuest` directly, `DeclineQuest` through its player leg.
    #[test]
    fn every_way_out_of_a_shared_quest_answers_the_sharer() {
        for verb in ["DeclineQuest()", "CloseQuest()"] {
            let sent = drain_after(SHARER, None, 0, verb);
            assert!(
                matches!(
                    sent.as_slice(),
                    [ClientCommand::QuestPushResult {
                        sharer: SHARER,
                        quest: 1,
                        msg: QuestShareMsg::DECLINE_QUEST,
                    }]
                ),
                "{verb} must answer the sharer, naming the open quest: {sent:?}"
            );
        }
    }

    #[test]
    fn ending_the_session_on_an_npc_reopens_its_list() {
        use crate::target::cursor_mode::npc_flags;

        let sent = drain_after(NPC, Some(0), 0, "DeclineQuest()");
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::QuestgiverHello { npc: NPC }]
            ),
            "a plain questgiver gets QUESTGIVER_HELLO: {sent:?}"
        );

        let sent = drain_after(NPC, Some(npc_flags::GOSSIP), 0, "DeclineQuest()");
        assert!(
            matches!(sent.as_slice(), [ClientCommand::GossipHello { guid: NPC }]),
            "a gossip-flagged NPC gets GOSSIP_HELLO: {sent:?}"
        );
    }

    /// The close-on-cancel latch's one reader.
    #[test]
    fn a_non_zero_latch_suppresses_a_plain_units_reopen() {
        let sent = drain_after(NPC, Some(0), 1, "DeclineQuest()");
        assert!(sent.is_empty(), "flag 1 suppresses the re-open: {sent:?}");
    }

    /// `CloseQuest` tears the window down; `DeclineQuest` bails first and leaves it.
    #[test]
    fn an_unstreamed_giver_ends_the_session_silently() {
        let (app, sent) = drain_world(NPC, None, 0, "CloseQuest()");
        assert!(sent.is_empty(), "no unit, no re-open: {sent:?}");
        assert!(!app.world().non_send_resource::<QuestGiver>().is_open());

        let (app, sent) = drain_world(NPC, None, 0, "DeclineQuest()");
        assert!(sent.is_empty(), "{sent:?}");
        assert!(app.world().non_send_resource::<QuestGiver>().is_open());
    }

    #[test]
    fn declining_an_item_quest_closes_silently() {
        const ITEM: u64 = 0x4000_0000_0000_0099; // HIGHGUID_ITEM
        let (app, sent) = drain_world(ITEM, None, 0, "DeclineQuest()");
        assert!(sent.is_empty(), "{sent:?}");
        assert!(!app.world().non_send_resource::<QuestGiver>().is_open());
    }

    /// ESC, the X and the greeting's Goodbye all reach `CloseQuest` through `QuestFrame_OnHide`
    /// (`QuestFrame.lua:293`).
    #[test]
    fn close_quest_on_an_npc_is_network_silent() {
        use crate::target::cursor_mode::npc_flags;
        for flags in [0, npc_flags::GOSSIP] {
            for latch in [0, 1] {
                let (app, sent) = drain_world(NPC, Some(flags), latch, "CloseQuest()");
                assert!(
                    sent.is_empty(),
                    "CloseQuest on an NPC (flags {flags:#x}, latch {latch}) sends nothing: {sent:?}"
                );
                assert!(
                    !app.world().non_send_resource::<QuestGiver>().is_open(),
                    "…and tears the window down"
                );
            }
        }
    }

    /// The gossip leg is tested before the latch.
    #[test]
    fn the_gossip_leg_precedes_the_latch() {
        use crate::target::cursor_mode::npc_flags;
        let sent = drain_after(NPC, Some(npc_flags::GOSSIP), 1, "DeclineQuest()");
        assert!(
            matches!(sent.as_slice(), [ClientCommand::GossipHello { guid: NPC }]),
            "a latched gossip NPC still gets GOSSIP_HELLO: {sent:?}"
        );
    }

    /// The acted latch refuses the second `DeclineQuest` and the `AcceptQuest`.
    #[test]
    fn a_reopening_decline_keeps_the_window_and_latches_acted() {
        let (app, sent) = drain_world(
            NPC,
            Some(0),
            0,
            "DeclineQuest() DeclineQuest() AcceptQuest()",
        );
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::QuestgiverHello { npc: NPC }]
            ),
            "one re-open, and nothing after it: {sent:?}"
        );
        let giver = app.world().non_send_resource::<QuestGiver>();
        assert!(
            giver.is_open(),
            "the window stays up for the server's answer"
        );
    }

    /// The latch-diverted leg takes the teardown.
    #[test]
    fn a_latched_decline_closes_the_window() {
        let (app, sent) = drain_world(NPC, Some(0), 1, "DeclineQuest()");
        assert!(sent.is_empty(), "{sent:?}");
        assert!(!app.world().non_send_resource::<QuestGiver>().is_open());
    }

    /// The panel world: a VM recording each quest event with the first reward's name at it.
    fn panel_world() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GuidIndex>()
            .init_non_send_resource::<QuestGiver>()
            .init_resource::<Items>()
            .init_resource::<NameCache>()
            .init_resource::<crate::world_state::WorldStates>()
            .init_resource::<crate::go_templates::GameObjectTemplates>();
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                for _, e in ipairs({"QUEST_DETAIL", "QUEST_ITEM_UPDATE", "QUEST_FINISHED"}) do
                    f:RegisterEvent(e)
                end
                f:SetScript("OnEvent", function(_, e)
                    SEEN[#SEEN + 1] = e .. " " .. (GetQuestItemInfo("reward", 1)) .. " " .. GetTitleText()
                end)
                "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        (app, rx)
    }

    fn seen(app: &App) -> Vec<String> {
        app.world()
            .non_send_resource::<UiScript>()
            .eval::<Vec<String>>("return SEEN")
            .unwrap()
    }

    /// The publish fires the panel's event over the stored panel (`0x58dfb5`); a row's item answer reads the cache again before `QUEST_ITEM_UPDATE` (`0x58b92d`); the close zeroes the giver and keeps the text (`0x58cb6b`, `0x58cb98`), and a second close fires nothing (`0x58ca85`).
    #[test]
    fn the_panel_publishes_in_turn_and_its_item_answer_updates_it() {
        let (mut app, _rx) = panel_world();
        let mut d = QuestDetails {
            npc: 0xF130_0000_0000_0007,
            quest_id: 7,
            title: "Charz".into(),
            ..Default::default()
        };
        d.rewards = vec![triple(6_529)];
        app.world_mut()
            .non_send_resource_mut::<QuestGiver>()
            .open(d.npc, QuestView::Detail(d));
        publish_panel(app.world_mut());
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(6_529, Some(crate::items::test_template("Shiny Bauble")));
        // The row's callback on the answer (`0x58b8f0`).
        crate::ui_script::live::with_live_world(app.world_mut(), |script| {
            script.item_record_callback(6_529, true, benilla_ui::script::ItemRecordWait::QuestItem)
        });
        giver_finished(app.world_mut(), false, 7, true);
        giver_finished(app.world_mut(), false, 7, true);
        assert_eq!(
            seen(&app),
            [
                "QUEST_DETAIL  Charz",
                "QUEST_ITEM_UPDATE Shiny Bauble Charz",
                "QUEST_FINISHED Shiny Bauble Charz",
            ]
        );
        assert!(!app.world().non_send_resource::<QuestGiver>().is_open());
    }

    /// A chained turn-in's close keeps the window and fires nothing (`0x58cb4c`); a player's window closed with a decline owed answers the sharer when in view (`0x58ca91`-`0x58cb1f`).
    #[test]
    fn the_close_keeps_a_chained_window_and_declines_to_a_sharer_in_view() {
        let (mut app, rx) = panel_world();
        const SHARER: u64 = 0x2A;
        app.world_mut().non_send_resource_mut::<QuestGiver>().open(
            SHARER,
            QuestView::Detail(QuestDetails {
                npc: SHARER,
                quest_id: 7,
                ..Default::default()
            }),
        );
        giver_finished(app.world_mut(), true, 7, false);
        assert!(app.world().non_send_resource::<QuestGiver>().is_open());
        let e = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(SHARER, e);
        giver_finished(app.world_mut(), false, 7, true);
        assert!(matches!(
            rx.try_iter().collect::<Vec<_>>().as_slice(),
            [ClientCommand::QuestPushResult {
                sharer: SHARER,
                quest: 7,
                msg: QuestShareMsg::DECLINE_QUEST,
            }]
        ));
        assert_eq!(seen(&app), ["QUEST_FINISHED  "]);
    }
}
