//! The quest log's app side: reads the player's `PLAYER_QUEST_LOG` slots every frame, resolves
//! each quest through the `SMSG_QUEST_QUERY_RESPONSE` cache and pushes the log, every quest with
//! its objective lines, firing `QUEST_LOG_UPDATE` on a change. The progress lines and
//! `QUEST_WATCH_UPDATE` come from the packets ([`progress`]). Abandons, shares and header folds
//! drain from here.

mod progress;

use crate::ui_items::{count_of, InventoryScope};
use std::collections::HashSet;

use bevy::prelude::*;

use benilla_protocol::messages::{
    quest_flags, quest_slot_state, QuestLogSlot, QuestObjective, QuestTemplate,
    PLAYER_QUEST_LOG_SLOTS,
};
use benilla_protocol::ObjectFields;
use benilla_ui::script::{
    QuestItemView, QuestLogDetail, QuestLogEntryView, QuestLogObjectiveView, QuestLogState,
    UiScript,
};
use benilla_ui::strings::{fill, Arg};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, Guid, NetCommands, ObjectStore, SelfPlayer};
use crate::query_cache::QueryCache;
use crate::ui_action::Spells;
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_unit::UnitFeed;

/// Marks a GameObject objective's entry on the wire (vmangos `Quest.cpp:512-516`).
const GO_OBJECTIVE_BIT: u32 = 0x8000_0000;

/// One occupied `PLAYER_QUEST_LOG` slot this frame.
struct Row {
    slot: u8,
    quest_id: u32,
    log_slot: QuestLogSlot,
}

/// The template cache, asked once per quest id, and this frame's quests as `(quest id, slot)`,
/// folded ones included: the reference's row table, which an abandon searches.
#[derive(Resource, Default)]
pub(crate) struct QuestLog {
    templates: QueryCache<u32, QuestTemplate>,
    row_slots: Vec<(u32, u8)>,
    /// The collapsed headers, by title.
    collapsed: HashSet<String>,
    /// Per pushed entry, the header's title, `None` for a quest: the fold drain's index map.
    header_keys: Vec<Option<String>>,
    /// The progress packets not yet announced, in arrival order.
    notices: Vec<progress::QuestNotice>,
    /// Quests a `_FAILEDTIMER` marked expired (`0x5e5840`), until they leave the log.
    expired: HashSet<u32>,
    /// The last `SMSG_QUERY_QUESTS_COMPLETED_RESPONSE`, not yet pushed.
    completed_answer: Option<Vec<u32>>,
}

/// `questcache.wdb`.
impl crate::wdb::Persisted for QuestLog {
    fn drop_persisted(&mut self) {
        self.templates.clear();
    }
}

impl crate::query_cache::AskOnce for QuestLog {
    fn clear_pending(&mut self) {
        self.templates.clear_pending();
    }
}

impl QuestLog {
    /// Every logged quest whose template is cached, folded ones included.
    pub(crate) fn quest_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.row_slots.iter().map(|&(id, _)| id)
    }

    /// The template, asked once: a miss sends `CMSG_QUEST_QUERY` (once while in flight); an id
    /// the server does not know stays `None` without a re-ask.
    pub(crate) fn template(&self, quest_id: u32, commands: &NetCommands) -> Option<&QuestTemplate> {
        self.templates.get_or_ask(quest_id, || {
            debug!("ui_quest_log: asking quest template (quest {quest_id})");
            let _ = commands
                .0
                .send(ClientCommand::QuestQuery { quest: quest_id });
        })
    }

    /// Whether a `_FAILEDTIMER` marked `quest_id` expired (`0x5dee60`).
    pub(crate) fn is_expired(&self, quest_id: u32) -> bool {
        self.expired.contains(&quest_id)
    }

    /// Whether the server answered `quest_id` as unknown, not still pending.
    pub(crate) fn template_answered_unknown(&self, quest_id: u32) -> bool {
        self.templates.answered_unknown(quest_id)
    }

    /// Record a template answer (`SMSG_QUEST_QUERY_RESPONSE`).
    pub(crate) fn insert_template(&mut self, template: QuestTemplate) {
        self.templates.insert(template.quest_id, Some(template));
    }

    /// Disconnect: drops everything, the templates included.
    pub(crate) fn clear_session(&mut self) {
        self.templates.clear();
        self.row_slots.clear();
        self.collapsed.clear();
        self.header_keys.clear();
        self.notices.clear();
        self.expired.clear();
        self.completed_answer = None;
    }
}

/// The quest log's packet handlers.
mod net {
    use benilla_protocol::messages::QuestTemplate;
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::QuestLog;
    use crate::net::NetHandlerApp;

    /// Registers the template handler and the session-end listener.
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::QuestTemplate, on_template)
            .net_handler(K::Disconnected, on_session_end);
    }

    fn on_template(In(ev): In<SessionEvent>, mut quest_log: ResMut<QuestLog>) {
        if let SessionEvent::QuestTemplate(t) = ev {
            quest_template(t, &mut quest_log);
        }
    }

    /// The log's state dies with the socket: a second handler on the kind, after the bridge's own.
    fn on_session_end(In(_): In<SessionEvent>, mut quest_log: ResMut<QuestLog>) {
        quest_log.clear_session();
    }

    /// `SMSG_QUEST_QUERY_RESPONSE`, the answer to our `CMSG_QUEST_QUERY`, cached by quest id.
    fn quest_template(t: Box<QuestTemplate>, quest_log: &mut QuestLog) {
        debug!("net: quest template {} ({})", t.quest_id, t.title);
        quest_log.insert_template(*t);
    }
}

pub(crate) struct UiQuestLogPlugin;

impl Plugin for UiQuestLogPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        progress::register(app);
        crate::query_cache::register::<QuestLog>(app);
        app.init_resource::<QuestLog>()
            .add_systems(
                Startup,
                (
                    load_quest_header_names,
                    load_quest_tag_names,
                    crate::ui_quest::rewards::load_quest_reward_tables,
                )
                    .after(benilla_assets::AssetSet::Open),
            )
            .add_systems(
                Update,
                (
                    progress::announce_quest_progress
                        .in_set(UnitFeed)
                        .before(feed_quest_log),
                    feed_quest_log.in_set(UnitFeed),
                    // Before the script tick, so `QuestTimerFrame` reads this frame's clock.
                    feed_server_clock.in_set(UiFeed),
                    drain_quest_log_abandons.after(UiInput),
                    drain_quest_log_pushes.after(UiInput),
                    drain_quest_log_collapses.after(UiInput),
                ),
            );
    }
}

/// Deviation: an in-flight name reads `...`, where the reference's placeholder is `" "`, because a
/// blank looks like a nameless objective.
const NAME_PLACEHOLDER: &str = "...";

/// One leaderboard line from its `%s`/`%d`/`%d` key. The keys are the leaderboard's own, not the
/// toast's `ERR_QUEST_ADD_*` twins that share their enUS sentences: `GetQuestLogLeaderBoard` takes
/// `QUEST_MONSTERS_KILLED` (creature), `QUEST_OBJECTS_FOUND` (override text, GameObject) and
/// `QUEST_ITEMS_NEEDED` (item). A key the install lacks gives an empty line, not no line, as
/// `GetNumQuestLeaderBoards` still counts it.
fn leaderboard_text(
    // The VM's own `GlobalStrings.lua`.
    get: &dyn Fn(&str) -> Option<String>,
    key: &str,
    name: &str,
    cur: u32,
    req: u32,
) -> String {
    get(key).map_or_else(String::new, |f| {
        fill(
            &f,
            &[Arg::S(name), Arg::D(i64::from(cur)), Arg::D(i64::from(req))],
        )
    })
}

/// One creature or GameObject line, for `ReqCreatureOrGOId[i] != 0` (`0x5e15ec`); `counter` is the
/// slot's `u16` counter for index i (`0x5e161c`). A non-empty `text` is the name and takes
/// `QUEST_OBJECTS_FOUND`, while `type` stays `"monster"`.
fn creature_line(
    obj: &QuestObjective,
    counter: u16,
    creature_name: Option<&str>,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<QuestLogObjectiveView> {
    if obj.creature_or_go == 0 {
        return None;
    }
    let req = obj.required_count;
    let is_go = obj.creature_or_go & GO_OBJECTIVE_BIT != 0;
    let cur = u32::from(counter).min(req);
    // A GameObject takes `QUEST_OBJECTS_FOUND` on both legs, named from the GameObject cache.
    let (key, name) = if !obj.text.is_empty() {
        ("QUEST_OBJECTS_FOUND", obj.text.as_str())
    } else if is_go {
        (
            "QUEST_OBJECTS_FOUND",
            creature_name.unwrap_or(NAME_PLACEHOLDER),
        )
    } else {
        (
            "QUEST_MONSTERS_KILLED",
            creature_name.unwrap_or(NAME_PLACEHOLDER),
        )
    };
    Some(QuestLogObjectiveView {
        text: leaderboard_text(get, key, name, cur, req),
        kind: if is_go { "object" } else { "monster" }.into(),
        finished: cur >= req,
        cur,
        req,
    })
}

/// One required-item line, for `ReqItemId[i] > 0` unless it is the quest's own source item and
/// one is enough (`0x5e16d9`). `bag_count` is the live inventory count, as an item has no slot
/// counter; the name comes from the item cache alone.
fn item_line(
    (item_id, req): (u32, u32),
    src_item_id: u32,
    item_name: Option<&str>,
    bag_count: u32,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<QuestLogObjectiveView> {
    if item_id == 0 || (item_id == src_item_id && req < 2) {
        return None;
    }
    let cur = bag_count.min(req);
    Some(QuestLogObjectiveView {
        text: leaderboard_text(
            get,
            "QUEST_ITEMS_NEEDED",
            item_name.unwrap_or(NAME_PLACEHOLDER),
            cur,
            req,
        ),
        kind: "item".into(),
        finished: cur >= req,
        cur,
        req,
    })
}

/// `GetQuestLevel` (`0x5e0d20`): a level -1 quest, a holiday's, reads the player's level.
fn quest_level(t: &QuestTemplate, player_level: u32) -> u32 {
    if t.level == u32::MAX {
        player_level
    } else {
        t.level
    }
}

/// The template's `money` as (required, reward): a negative value is demanded at turn-in.
fn money_split(money: i32) -> (u32, u32) {
    if money < 0 {
        ((-money) as u32, 0)
    } else {
        (0, money as u32)
    }
}

/// One reward or choice `(item id, count)` as a [`QuestItemView`]. `SMSG_QUEST_QUERY_RESPONSE`
/// carries no display id, so the icon comes from the item template's `display_info_id`.
fn resolve_template_item(
    entry: u32,
    count: u32,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> QuestItemView {
    let template = items.template(entry, 0, commands).cloned();
    let name = template.as_ref().map(|t| t.name.clone());
    let quality = template.as_ref().map(|t| t.quality).unwrap_or(1);
    let texture = template.as_ref().and_then(|t| {
        icons
            .and_then(|i| i.catalog.get(t.display_info_id))
            .and_then(|d| d.icon.clone())
    });
    // `GetQuestLogItemLink`'s payload; `None` until the template gives the name and quality.
    let link = name
        .as_ref()
        .map(|n| crate::ui_items::item_link(entry, n, quality));
    QuestItemView {
        name,
        texture,
        count,
        quality,
        item_id: entry,
        usable: true, // not computed; the stock frame tints unusable ones red
        link,
    }
}

/// What the leaderboard's later lines read beside the template: the GameObject names, the
/// reputation objective, the player's sex for its standing labels, and the money check of the
/// completed quest's `log` line.
#[derive(Default)]
pub(crate) struct ObjectiveExtras<'a> {
    pub(crate) gos: Option<&'a crate::go_templates::GameObjectTemplates>,
    /// `(faction name, current rank, needed rank, met)`, ranks 0..=7.
    pub(crate) reputation: Option<(String, u8, u8, bool)>,
    pub(crate) female: bool,
    /// The player's coinage, for a quest that costs money.
    pub(crate) money: u64,
    /// The objectives text through the quest text parser, the `log` line.
    pub(crate) objectives_text: String,
}

/// A standing to its rank 0..=7, hated to exalted, on the leaderboard's thresholds (`0x5e1460`).
pub(crate) fn standing_rank(standing: i32) -> u8 {
    match standing {
        42000.. => 7,
        21000.. => 6,
        9000.. => 5,
        3000.. => 4,
        0.. => 3,
        -3000.. => 2,
        -6000.. => 1,
        _ => 0,
    }
}

/// One quest's objective lines, for every quest, as the watch frame reads any quest's. The
/// leaderboard (`0x5e1460`) walks `event` (the end text), the four creature or GameObject rows,
/// the six required items, then one `player` line, else one `reputation` line; a quest with none
/// of these that is done reads its objectives text as a `log` line. Deviation: the intermediate
/// drops, which the reference walks before the player line while the quest is incomplete, are not
/// listed: `GetNumQuestLeaderBoards` (`0x5e1300`) never counts them.
#[allow(clippy::too_many_arguments)] // the leaderboard's whole input set
fn build_objectives(
    template: &QuestTemplate,
    log_slot: &QuestLogSlot,
    store: &ObjectFields,
    objects: &crate::net::Objects,
    items: &Items,
    names: &NameCache,
    commands: &NetCommands,
    // The VM's own `GlobalStrings.lua`, for the leaderboard's format keys.
    get: &dyn Fn(&str) -> Option<String>,
    extras: &ObjectiveExtras<'_>,
) -> Vec<QuestLogObjectiveView> {
    let mut objectives = Vec::with_capacity(template.objectives.len());
    // The end text, verbatim, done when the slot's state is.
    if !template.end_text.is_empty() {
        let done = log_slot.state & quest_slot_state::COMPLETE != 0;
        objectives.push(QuestLogObjectiveView {
            text: template.end_text.clone(),
            kind: "event".into(),
            finished: done,
            cur: u32::from(done),
            req: 1,
        });
    }
    // Creature or GameObject: the counter index is the array index, empty elements included.
    for (i, obj) in template.objectives.iter().enumerate() {
        let is_go = obj.creature_or_go & GO_OBJECTIVE_BIT != 0;
        let name = match (obj.creature_or_go, is_go) {
            (0, _) => None,
            (entry, false) => names
                .resolve_creature(entry, 0, commands)
                .map(str::to_string),
            (entry, true) => extras
                .gos
                .and_then(|g| g.name_by_entry(entry & !GO_OBJECTIVE_BIT, commands))
                .map(str::to_string),
        };
        if let Some(line) = creature_line(obj, log_slot.counters[i], name.as_deref(), get) {
            objectives.push(line);
        }
    }
    // Required items.
    for &(item_id, req) in template.required_items.iter() {
        if item_id == 0 {
            continue;
        }
        let item_name = items.template(item_id, 0, commands).map(|t| t.name.clone());
        let bag_count = count_of(store, objects, item_id, InventoryScope::QUEST_ITEMS);
        if let Some(line) = item_line(
            (item_id, req),
            template.src_item_id,
            item_name.as_deref(),
            bag_count,
            get,
        ) {
            objectives.push(line);
        }
    }
    // Players slain read counter 0.
    if template.players_slain != 0 {
        let cur = u32::from(log_slot.counters[0]);
        let req = template.players_slain;
        objectives.push(QuestLogObjectiveView {
            text: get("QUEST_PLAYERS_KILLED").map_or_else(String::new, |f| {
                fill(&f, &[Arg::D(i64::from(cur)), Arg::D(i64::from(req))])
            }),
            kind: "player".into(),
            finished: cur >= req,
            cur: cur.min(req),
            req,
        });
    } else if let Some((faction, current, needed, met)) = &extras.reputation {
        // The labels are the player's own, the `_FEMALE` form first for a woman.
        let label = |rank: u8| {
            let key = format!("FACTION_STANDING_LABEL{}", rank + 1);
            extras
                .female
                .then(|| get(&format!("{key}_FEMALE")))
                .flatten()
                .or_else(|| get(&key))
                .unwrap_or_default()
        };
        objectives.push(QuestLogObjectiveView {
            text: get("QUEST_FACTION_NEEDED").map_or_else(String::new, |f| {
                fill(
                    &f,
                    &[
                        Arg::S(faction),
                        Arg::S(&label(*current)),
                        Arg::S(&label(*needed)),
                    ],
                )
            }),
            kind: "reputation".into(),
            finished: *met,
            cur: u32::from(*met),
            req: 1,
        });
    } else if objectives.is_empty() && log_slot.state & quest_slot_state::FAIL == 0 {
        // Nothing to count: the objectives text, once the quest is done. The reference's done
        // test passes such a quest only for an exploration or escort flag (`flags & 6`) with the
        // slot complete and any cost in hand (`0x5e0ea0`).
        let cost = (-i64::from(template.money)).max(0) as u64;
        let done = template.flags & 6 != 0
            && log_slot.state & quest_slot_state::COMPLETE != 0
            && extras.money >= cost;
        if done {
            objectives.push(QuestLogObjectiveView {
                text: extras.objectives_text.clone(),
                kind: "log".into(),
                finished: true,
                cur: 1,
                req: 1,
            });
        }
    }
    objectives
}

/// The reference's POI objective mask (`0x5e2950`), the objectives a quest's POI regions may serve:
/// bit `i` an unfinished kill or GameObject row, `4 + i` a required item short, `10 + i` a drop
/// item not carried (the starting item excepted), `0x10000` an unmet event; all ones once complete,
/// 0 when failed.
fn poi_objectives(
    template: &QuestTemplate,
    log_slot: &QuestLogSlot,
    store: &ObjectFields,
    objects: &crate::net::Objects,
) -> u32 {
    if log_slot.state & quest_slot_state::FAIL != 0 {
        return 0;
    }
    if log_slot.state & quest_slot_state::COMPLETE != 0 {
        return u32::MAX;
    }
    let carried = |id: u32| count_of(store, objects, id, InventoryScope::QUEST_ITEMS);
    let mut mask = 0;
    for (i, o) in template.objectives.iter().enumerate() {
        if o.creature_or_go != 0 && u32::from(log_slot.counters[i]) < o.required_count {
            mask |= 1 << i;
        }
        if o.item_drop != 0 && o.item_drop != template.src_item_id && carried(o.item_drop) == 0 {
            mask |= 1 << (i + 10);
        }
    }
    for (i, &(id, req)) in template.required_items.iter().enumerate() {
        if id != 0 && carried(id) < req {
            mask |= 1 << (i + 4);
        }
    }
    // Exploration or event quests (`flags & 6`) not yet complete.
    if template.flags & 6 != 0 {
        mask |= 0x10000;
    }
    mask
}

/// `GetQuestLogCompletionText` (`0x5e06d0`): the end text, or the objectives text when the end
/// text is empty or the template's flag `0x40000` is set.
fn completion_text(template: &QuestTemplate, macros: &crate::npc_text::MacroContext) -> String {
    let raw = if template.flags & 0x40000 != 0 || template.completed_text.is_empty() {
        &template.objectives_text
    } else {
        &template.completed_text
    };
    crate::npc_text::substitute(raw, macros)
}

/// A quest's detail pane from its template: description, money, rewards and choices.
fn build_detail(
    template: &QuestTemplate,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
    macros: &crate::npc_text::MacroContext,
    spells: Option<&Spells>,
) -> QuestLogDetail {
    let (required_money, reward_money) = money_split(template.money);
    let rewards = template
        .rewards
        .iter()
        .filter(|&&(id, _)| id != 0)
        .map(|&(id, count)| resolve_template_item(id, count, items, icons, commands))
        .collect();
    let choices = template
        .choices
        .iter()
        .filter(|&&(id, _)| id != 0)
        .map(|&(id, count)| resolve_template_item(id, count, items, icons, commands))
        .collect();

    QuestLogDetail {
        // Quest text arrives with its `$N`, `$B`, `$G` and `$<n>w` macros unexpanded.
        description: crate::npc_text::substitute(&template.details, macros),
        objectives_text: crate::npc_text::substitute(&template.objectives_text, macros),
        required_money,
        reward_money,
        choices,
        rewards,
        reward_spell: crate::ui_quest::reward_spell_view(
            template.reward_spell,
            template.reward_spell_cast,
            spells,
        ),
        extras: Default::default(),
    }
}

/// `AbandonQuest`'s search: the slot (`+0x4`) of the non-header row of the 40 that carries
/// `quest_id`; with no such row, nothing is sent.
fn abandon_slot(quest_id: u32, row_slots: &[(u32, u8)]) -> Option<u8> {
    row_slots
        .iter()
        .find(|&&(id, _)| id == quest_id)
        .map(|&(_, slot)| slot)
}

/// Moves the 1-based selection to its quest's new index. A header is never a selection
/// (`QuestLog_GetFirstSelectableQuest` skips them), so one with no visible quest becomes 0 and
/// `QuestLog_Update` selects the first quest (`QuestLogFrame.lua:293-296`).
fn remap_selection(old: &[QuestLogEntryView], new: &[QuestLogEntryView], sel: u32) -> u32 {
    let Some(prev) = (sel as usize)
        .checked_sub(1)
        .and_then(|i| old.get(i))
        .filter(|e| !e.is_header)
    else {
        return 0;
    };
    new.iter()
        .position(|e| !e.is_header && e.quest_id == prev.quest_id)
        .map(|i| i as u32 + 1)
        .unwrap_or(0)
}

/// Pushes the server's wall clock every frame, for `GetQuestTimers()`; before the first
/// `SMSG_QUERY_TIME_RESPONSE` nothing is pushed and the bindings report no timer.
fn feed_server_clock(
    script: Option<NonSendMut<UiScript>>,
    clock: Res<crate::net::ServerWallClock>,
) {
    let (Some(mut script), Some(now)) = (script, clock.now_unix()) else {
        return;
    };
    script.set_server_unix_time(now);
    if let Some(reset) = clock.quest_reset_unix() {
        script.set_quest_reset_unix(reset);
    }
}

/// The reference's header for `zoneOrSort` 0. Only a cached template makes a header, so it never
/// stands for a template in flight.
const MISSING_HEADER: &str = "Missing header! (quest designers)";

/// One row's grouping and ordering inputs.
struct GroupRow {
    zone_or_sort: i32,
    /// [`MISSING_HEADER`] for 0, empty for an id neither `AreaTable` nor `QuestSort` names.
    header: String,
    level: u32,
    title: String,
    slot: u8,
}

/// The quest log's display order: the header groups, each with its rows. Groups put id 0 first,
/// then go by name through the case-insensitive collator (`_strnicmp`), an unnamed id as `""`. Rows
/// go by level, then title, never by slot; the reference's `qsort` is unstable, so a full tie has
/// no order there, and ties break on slot here.
fn order_groups(rows: &[GroupRow]) -> Vec<(i32, String, Vec<usize>)> {
    let mut groups: Vec<(i32, String, Vec<usize>)> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        match groups.iter_mut().find(|(z, _, _)| *z == r.zone_or_sort) {
            Some((_, _, idxs)) => idxs.push(i),
            None => groups.push((r.zone_or_sort, r.header.clone(), vec![i])),
        }
    }
    groups.sort_by_key(|(zos, name, _)| {
        if *zos == 0 {
            (0u8, String::new())
        } else {
            (1, name.to_lowercase())
        }
    });
    for (_, _, idxs) in &mut groups {
        idxs.sort_by_key(|&i| {
            let r = &rows[i];
            (r.level, r.title.to_lowercase(), r.slot)
        });
    }
    groups
}

/// Reads the player's `PLAYER_QUEST_LOG` slots and pushes a [`QuestLogState`] on a change; also
/// refreshes `row_slots` for the abandon drain. A frame on which no input moved rebuilds nothing.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
pub(crate) fn feed_quest_log(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    mut quest_log: ResMut<QuestLog>,
    names: Res<NameCache>,
    // The bag walk behind an item objective's count.
    objects: crate::net::Objects,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    commands: Res<NetCommands>,
    // The player's store, and the item and container stores the bag walk reads, moving.
    (header_names, tag_names, stirred_self, stirred_items, gos): (
        Option<Res<QuestHeaderNamesRes>>,
        Option<Res<QuestTagNamesRes>>,
        Query<(), (With<SelfPlayer>, Changed<ObjectStore>)>,
        Query<(), (Changed<ObjectStore>, Without<crate::net::NetEntity>)>,
        // The GameObject objectives' names.
        Option<Res<crate::go_templates::GameObjectTemplates>>,
    ),
    states: Res<crate::world_state::WorldStates>,
    // The XP, honor, title and reputation extras each row's detail carries.
    rewards: crate::ui_quest::rewards::RewardInputs,
    spells: Option<Res<Spells>>,
    mut last: Local<crate::ui_script::VmMemo<QuestLogState>>,
    // The accept line's queue.
    mut quest_lines: ResMut<crate::ui_quest::QuestLines>,
    mut prior_slots: Local<crate::ui_script::VmMemo<Option<SlotStates>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (last, fresh) = last.get_reset(&script);
    let prior_slots = prior_slots.get(&script);
    let Some((store, _)) = self_q.iter().next() else {
        return;
    };
    // Every row reads only these, so an unmoved set rebuilds the state the diff below would drop;
    // a fresh VM takes its first push whatever moved.
    let stirred = fresh
        || !stirred_self.is_empty()
        || !stirred_items.is_empty()
        || quest_log.is_changed()
        || names.is_changed()
        || items.is_changed()
        || icons.as_ref().is_some_and(|r| r.is_changed())
        || header_names.as_ref().is_some_and(|r| r.is_changed())
        || tag_names.as_ref().is_some_and(|r| r.is_changed())
        || gos.as_ref().is_some_and(|r| r.is_changed())
        || states.is_changed()
        || spells.as_ref().is_some_and(|r| r.is_changed())
        || rewards.changed();
    if !stirred {
        return;
    }

    let mut rows = Vec::new();
    let mut slots = Vec::with_capacity(usize::from(PLAYER_QUEST_LOG_SLOTS));
    for slot in 0..PLAYER_QUEST_LOG_SLOTS {
        let Some(log_slot) = store.0.player_quest_log(slot) else {
            slots.push((0, 0));
            continue;
        };
        slots.push((log_slot.quest_id, log_slot.state));
        if log_slot.quest_id == 0 {
            continue; // a slot an abandon or turn-in cleared
        }
        rows.push(Row {
            slot,
            quest_id: log_slot.quest_id,
            log_slot,
        });
    }

    // A timer's expiry mark lives on the quest's row, so it leaves with the quest.
    quest_log
        .expired
        .retain(|id| slots.iter().any(|&(q, _)| q == *id));

    // The quest-slot watcher's accept line, whose catalog row carries `QUESTADDED`. The first
    // snapshot after login is silent: the initial create fills the whole log.
    if let Some(prior) = prior_slots.as_ref() {
        for quest_id in accepted_quests(prior, &slots) {
            quest_lines.push(crate::ui_quest::QuestLine::Accepted(quest_id));
        }
    }
    *prior_slots = Some(slots);

    // The player's own `GlobalStrings.lua`, for the leaderboard's three format keys.
    let get = |key: &str| {
        script
            .lua()
            .globals()
            .get::<String>(key)
            .ok()
            .filter(|t| !t.is_empty())
    };

    // ── In-flight templates: only cached quests are listed ──────────────────────────────────────
    // The rebuild skips a quest whose template is uncached, counting it, and lists every other one,
    // so `GetNumQuestLogEntries()` counts cached quests. The reference then holds that list until
    // the last template lands; here each quest shows as its template lands. `template()` runs for
    // every row before the filter: the miss is what sends the query.
    let cached: Vec<bool> = rows
        .iter()
        .map(|r| quest_log.template(r.quest_id, &commands).is_some())
        .collect();
    let rows: Vec<Row> = rows
        .into_iter()
        .zip(cached)
        .filter_map(|(r, hit)| hit.then_some(r))
        .collect();

    let player_level = store.0.unit_level().unwrap_or(0);

    // ── Section headers ─────────────────────────────────────────────────────────────────────────
    // One per `ZoneOrSort`: a positive id names an `AreaTable` zone, a negative a `QuestSort`.
    let ordered = {
        let keyed: Vec<GroupRow> = rows
            .iter()
            .map(|r| {
                let t = quest_log.templates.get(r.quest_id);
                let zos = t.map(|t| t.zone_or_sort).unwrap_or(0);
                GroupRow {
                    zone_or_sort: zos,
                    header: if zos == 0 {
                        MISSING_HEADER.to_string()
                    } else {
                        header_names
                            .as_ref()
                            .and_then(|h| h.0.resolve(zos))
                            .unwrap_or_default()
                            .to_string()
                    },
                    level: t.map_or(0, |t| quest_level(t, player_level)),
                    title: t.map(|t| t.title.clone()).unwrap_or_default(),
                    slot: r.slot,
                }
            })
            .collect();
        order_groups(&keyed)
    };
    let groups = ordered;

    let player = crate::npc_text::player_identity(&self_q, &names, &commands);
    let macros = crate::npc_text::MacroContext {
        subject: player.as_ref(),
        states: &states,
        achievement: None,
    };

    let mut entries: Vec<QuestLogEntryView> = Vec::new();
    let mut header_keys: Vec<Option<String>> = Vec::new();
    // Quests under a collapsed header: out of the visible list, still in the log, and counted by
    // the watch prune, as the reference's prune scans the whole row array.
    let mut hidden_quest_ids: Vec<u32> = Vec::new();
    for (_, name, row_idxs) in &groups {
        let collapsed = quest_log.collapsed.contains(name);
        entries.push(QuestLogEntryView {
            quest_id: 0,
            title: name.clone(),
            level: 0,
            tag: None,
            is_header: true,
            collapsed,
            complete: 0,
            timer: 0,
            pushable: false,
            suggested_group: 0,
            is_daily: false,
            objectives: Vec::new(),
            detail: None,
            completion_text: String::new(),
            poi_objectives: 0,
        });
        header_keys.push(Some(name.clone()));
        for &ri in row_idxs {
            let r = &rows[ri];
            if collapsed {
                hidden_quest_ids.push(r.quest_id);
                continue;
            }
            let (title, level, tag, pushable, suggested_group, is_daily, objectives, detail) =
                match quest_log.template(r.quest_id, &commands) {
                    Some(t) => (
                        t.title.clone(),
                        quest_level(t, player_level),
                        // The tag: `Type` is a `QuestInfo.dbc` id, 0 naming none. No table gives
                        // `None`, never `""`: the row's Lua tests presence.
                        tag_names
                            .as_ref()
                            .and_then(|tags| tags.0.resolve(t.quest_type))
                            .map(str::to_string),
                        // `QUEST_FLAGS_SHARABLE` (vmangos `QuestDef.h:153`) is the client's test
                        // alone: vmangos pushes without it (`QuestHandler.cpp:403-459`) and checks
                        // it only at the accept (`QuestHandler.cpp:114`).
                        t.flags & quest_flags::SHARABLE != 0,
                        t.suggested_players,
                        t.flags & quest_flags::DAILY != 0,
                        build_objectives(
                            t,
                            &r.log_slot,
                            &store.0,
                            &objects,
                            &items,
                            &names,
                            &commands,
                            &get,
                            &ObjectiveExtras {
                                gos: gos.as_deref(),
                                reputation: rewards.rep_objective(&store.0, t),
                                female: store.0.unit_gender() == Some(1),
                                money: u64::from(store.0.player_money().unwrap_or(0)),
                                objectives_text: crate::npc_text::substitute(
                                    &t.objectives_text,
                                    &macros,
                                ),
                            },
                        ),
                        // Every row's: the bindings resolve the selection per call.
                        Some(QuestLogDetail {
                            // At the level cap the reward is at least the money paid in place
                            // of XP (`0x5e48f0`).
                            reward_money: {
                                let paid = t.money.max(0) as u32;
                                match (store.0.unit_level(), store.0.player_max_level()) {
                                    (Some(l), Some(m)) if l >= m => paid.max(t.money_max_level),
                                    _ => paid,
                                }
                            },
                            extras: rewards.for_template(
                                t,
                                &store.0,
                                player.as_ref().map_or("", |p| p.name.as_str()),
                                spells.as_deref(),
                            ),
                            ..build_detail(
                                t,
                                &items,
                                icons.as_deref(),
                                &commands,
                                &macros,
                                spells.as_deref(),
                            )
                        }),
                    ),
                    // The in-flight filter above kept only cached rows.
                    None => unreachable!("the in-flight gate leaves only cached rows"),
                };
            let (completion_text, poi_objectives) = quest_log
                .template(r.quest_id, &commands)
                .map(|t| {
                    (
                        completion_text(t, &macros),
                        poi_objectives(t, &r.log_slot, &store.0, &objects),
                    )
                })
                .unwrap_or_default();
            let complete = if r.log_slot.state & quest_slot_state::COMPLETE != 0 {
                1
            } else if r.log_slot.state & quest_slot_state::FAIL != 0 {
                -1
            } else {
                0
            };
            entries.push(QuestLogEntryView {
                quest_id: r.quest_id,
                title,
                level,
                tag,
                is_header: false,
                collapsed: false,
                complete,
                pushable,
                suggested_group,
                is_daily,
                // Absolute unix seconds, 0 untimed; the bindings subtract the server clock.
                timer: r.log_slot.timer,
                objectives,
                detail,
                completion_text,
                poi_objectives,
            });
            header_keys.push(None);
        }
    }

    // ── Selection remap ─────────────────────────────────────────────────────────────────────────
    let sel = script.quest_log_selection();
    let new_sel = remap_selection(&last.entries, &entries, sel);
    if new_sel != sel {
        script.set_quest_log_selection(new_sel);
    }
    // Every cached row, folded or not: the table `AbandonQuest` searches.
    quest_log.row_slots = rows.iter().map(|r| (r.quest_id, r.slot)).collect();
    quest_log.header_keys = header_keys;

    let fresh = QuestLogState {
        entries,
        num_quests: rows.len() as u32,
        daily_completed: store.0.player_daily_quests_completed(),
        hidden_quest_ids,
    };
    if fresh == *last {
        return;
    }
    script.set_quest_log(fresh.clone());
    // `QUEST_LOG_UPDATE` is a unique signal flushed after the frame's packets (`0x614760`), so it
    // follows the packets' `QUEST_WATCH_UPDATE` ([`progress`]) and the tracker redraws with the
    // progress watch already added.
    script.fire_event("QUEST_LOG_UPDATE", vec![]);
    *last = fresh;
}

/// Every quest-log slot as `(quest id, state)`, the slot watcher's input.
type SlotStates = Vec<(u32, u8)>;

/// The quests the slot watcher announces, per slot as `(quest id, state)`: a quest in a slot that
/// held none, or the same quest with its fail bit cleared. A slot that swaps one quest for another
/// announces nothing.
fn accepted_quests(prior: &[(u32, u8)], now: &[(u32, u8)]) -> Vec<u32> {
    now.iter()
        .zip(prior)
        .filter(|&(&(id, state), &(was_id, was_state))| {
            id != 0
                && (was_id == 0
                    || (was_id == id
                        && state & quest_slot_state::FAIL == 0
                        && was_state & quest_slot_state::FAIL != 0))
        })
        .map(|(&(id, _), _)| id)
        .collect()
}

/// The `ZoneOrSort` header names; without client data only id 0's header has a name.
#[derive(Resource)]
pub(crate) struct QuestHeaderNamesRes(pub benilla_formats::QuestHeaderNames);

/// Startup: reads the `AreaTable` and `QuestSort` names off the patch chain.
fn load_quest_header_names(
    mut commands: Commands,
    assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    use benilla_assets::LockRecover;
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_quest_header_names(&mut chain)
    };
    match loaded {
        Ok(names) => commands.insert_resource(QuestHeaderNamesRes(names)),
        Err(e) => warn!("ui_quest_log: quest header names failed to load: {e:#}"),
    }
}

/// The quest tag names by `Type` (`QuestInfo.dbc`); without client data no quest has a tag.
#[derive(Resource)]
pub(crate) struct QuestTagNamesRes(pub benilla_formats::QuestTagNames);

/// Startup: reads the `QuestInfo.dbc` tag names off the patch chain.
fn load_quest_tag_names(mut commands: Commands, assets: Option<Res<benilla_assets::WorldAssets>>) {
    use benilla_assets::LockRecover;
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_quest_tag_names(&mut chain)
    };
    match loaded {
        Ok(names) => commands.insert_resource(QuestTagNamesRes(names)),
        Err(e) => warn!("ui_quest_log: quest tag names failed to load: {e:#}"),
    }
}

/// Drains `CollapseQuestHeader` and `ExpandQuestHeader`: an entry index names a header by its
/// title, and 0 is every header (the collapse-all button).
fn drain_quest_log_collapses(
    script: Option<NonSendMut<UiScript>>,
    mut quest_log: ResMut<QuestLog>,
) {
    let Some(mut script) = script else {
        return;
    };
    for (idx, collapse) in script.take_quest_log_collapses() {
        if idx == 0 {
            if collapse {
                let all: Vec<String> = quest_log.header_keys.iter().flatten().cloned().collect();
                quest_log.collapsed.extend(all);
            } else {
                quest_log.collapsed.clear();
            }
        } else if let Some(Some(name)) = quest_log.header_keys.get(idx as usize - 1).cloned() {
            if collapse {
                quest_log.collapsed.insert(name);
            } else {
                quest_log.collapsed.remove(&name);
            }
        }
    }
}

/// `QuestLogPushQuest()`: the quest id taken at the click goes out; the server finds the party.
fn drain_quest_log_pushes(script: Option<NonSendMut<UiScript>>, commands: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for quest in script.take_quest_log_pushes() {
        debug!("ui_quest_log: sharing quest {quest} with the party");
        let _ = commands.0.send(ClientCommand::PushQuestToParty { quest });
    }
}

fn drain_quest_log_abandons(
    script: Option<NonSendMut<UiScript>>,
    quest_log: Res<QuestLog>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    for quest in script.take_quest_log_abandons() {
        match abandon_slot(quest, &quest_log.row_slots) {
            Some(slot) => {
                debug!("ui_quest_log: abandon quest {quest} → slot {slot}");
                let _ = commands.0.send(ClientCommand::QuestlogRemove { slot });
            }
            None => debug!("ui_quest_log: abandon quest {quest} is not in the log — ignored"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── order_groups ────────────────────────────────────────────────────────────────────────────

    fn grow(zos: i32, header: &str, level: u32, title: &str, slot: u8) -> GroupRow {
        GroupRow {
            zone_or_sort: zos,
            header: header.into(),
            level,
            title: title.into(),
            slot,
        }
    }

    #[test]
    fn groups_put_id_zero_first_then_unnamed_then_case_folded_names() {
        let rows = vec![
            grow(12, "Elwynn Forest", 1, "a", 0),
            grow(-3, "warlock", 1, "b", 1), // a byte compare sorts it after "Westfall"
            grow(0, MISSING_HEADER, 1, "c", 2),
            grow(40, "Westfall", 1, "d", 3),
            grow(999, "", 1, "e", 4), // an id neither table names
        ];
        let order: Vec<i32> = order_groups(&rows).into_iter().map(|(z, _, _)| z).collect();
        assert_eq!(order, vec![0, 999, 12, -3, 40]);
    }

    #[test]
    fn the_zero_header_is_the_references_literal() {
        let rows = vec![grow(0, MISSING_HEADER, 1, "a", 0)];
        let groups = order_groups(&rows);
        assert_eq!(groups[0].1, "Missing header! (quest designers)");
    }

    #[test]
    fn within_a_group_it_is_level_then_title_not_slot_order() {
        // Seeded in the reverse of the expected order by slot.
        let rows = vec![
            grow(12, "Elwynn Forest", 9, "Zzz", 0),
            grow(12, "Elwynn Forest", 3, "beta", 1),
            grow(12, "Elwynn Forest", 3, "Alpha", 2),
            grow(12, "Elwynn Forest", 1, "Anything", 3),
        ];
        let groups = order_groups(&rows);
        assert_eq!(groups.len(), 1);
        let titles: Vec<&str> = groups[0]
            .2
            .iter()
            .map(|&i| rows[i].title.as_str())
            .collect();
        assert_eq!(titles, vec!["Anything", "Alpha", "beta", "Zzz"]);
    }

    #[test]
    fn title_order_is_case_insensitive() {
        let rows = vec![
            grow(12, "Zone", 5, "apple", 0),
            grow(12, "Zone", 5, "Banana", 1),
        ];
        let groups = order_groups(&rows);
        let titles: Vec<&str> = groups[0]
            .2
            .iter()
            .map(|&i| rows[i].title.as_str())
            .collect();
        assert_eq!(titles, vec!["apple", "Banana"]);
    }

    // ── remap_selection ─────────────────────────────────────────────────────────────────────────

    fn header(title: &str) -> QuestLogEntryView {
        QuestLogEntryView {
            detail: None,
            quest_id: 0,
            title: title.into(),
            level: 0,
            tag: None,
            is_header: true,
            collapsed: false,
            complete: 0,
            timer: 0,
            pushable: false,
            suggested_group: 0,
            is_daily: false,
            objectives: Vec::new(),
            completion_text: String::new(),
            poi_objectives: 0,
        }
    }

    fn quest(id: u32) -> QuestLogEntryView {
        QuestLogEntryView {
            detail: None,
            quest_id: id,
            title: format!("Quest {id}"),
            level: 1,
            tag: None,
            is_header: false,
            collapsed: false,
            complete: 0,
            timer: 0,
            pushable: false,
            suggested_group: 0,
            is_daily: false,
            objectives: Vec::new(),
            completion_text: String::new(),
            poi_objectives: 0,
        }
    }

    #[test]
    fn selection_follows_its_quest_across_reindexing() {
        let old = vec![header("A"), quest(7), quest(9)];
        // A new header lands above: quest 9 moves from entry 3 to entry 5.
        let new = vec![header("A"), quest(7), header("B"), header("C"), quest(9)];
        assert_eq!(remap_selection(&old, &new, 3), 5);
    }

    #[test]
    fn fold_hiding_the_selection_resets_to_zero_never_the_header() {
        // A header is never a selection: 0 lets `QuestLog_SetFirstValidSelection` pick a quest.
        let old = vec![header("A"), quest(7)];
        let new = vec![header("A")]; // folded: the quest left the visible list
        assert_eq!(remap_selection(&old, &new, 2), 0);
    }

    #[test]
    fn a_header_selection_is_evicted_not_preserved() {
        let old = vec![header("A"), quest(7)];
        let new = vec![header("A"), quest(7)];
        assert_eq!(remap_selection(&old, &new, 1), 0);
    }

    #[test]
    fn zero_and_stale_selections_stay_zero() {
        let old = vec![header("A"), quest(7)];
        let new = vec![header("A"), quest(7)];
        assert_eq!(remap_selection(&old, &new, 0), 0);
        assert_eq!(remap_selection(&old, &new, 99), 0); // out-of-range: nothing to follow
    }

    /// One row of a test template: objective `i`'s creature or GameObject, its count and text,
    /// and required item `i`, which 3.3.5a keeps in its own array.
    #[derive(Clone, Copy)]
    struct Row {
        creature_or_go: u32,
        required_count: u32,
        item_id: u32,
        item_count: u32,
        text: &'static str,
    }

    fn obj(
        creature_or_go: u32,
        required_count: u32,
        item_id: u32,
        item_count: u32,
        text: &'static str,
    ) -> Row {
        Row {
            creature_or_go,
            required_count,
            item_id,
            item_count,
            text,
        }
    }

    impl Row {
        fn objective(&self) -> QuestObjective {
            QuestObjective {
                creature_or_go: self.creature_or_go,
                required_count: self.required_count,
                text: self.text.into(),
                ..Default::default()
            }
        }
        fn item(&self) -> (u32, u32) {
            (self.item_id, self.item_count)
        }
    }

    // ── creature_line / item_line ───────────────────────────────────────────────────────────────

    /// Templates that name their key, as enUS gives each leaderboard key the same sentence as its
    /// `ERR_QUEST_ADD_*` twin.
    fn probe_strings(key: &str) -> Option<String> {
        match key {
            "QUEST_MONSTERS_KILLED" => Some("<KILLED %s %d of %d>".into()),
            "QUEST_OBJECTS_FOUND" => Some("<FOUND %s %d of %d>".into()),
            "QUEST_ITEMS_NEEDED" => Some("<NEEDED %s %d of %d>".into()),
            _ => None,
        }
    }

    #[test]
    fn creature_objective_with_resolved_name() {
        let o = obj(100, 10, 0, 0, "");
        let line = creature_line(&o.objective(), 3, Some("Kobold Vermin"), &probe_strings).unwrap();
        assert_eq!(line.text, "<KILLED Kobold Vermin 3 of 10>");
        assert_eq!(line.kind, "monster");
        assert!(!line.finished);
    }

    #[test]
    fn creature_objective_in_flight_shows_a_placeholder() {
        let o = obj(100, 10, 0, 0, "");
        let line = creature_line(&o.objective(), 0, None, &probe_strings).unwrap();
        assert_eq!(line.text, "<KILLED ... 0 of 10>");
    }

    /// The GameObject key is fetched above the cache test, so a nameless one keeps it.
    #[test]
    fn go_objective_falls_back_without_a_name_cache() {
        let go_id = ((-57i32) as u32) | 0x8000_0000;
        let o = obj(go_id, 1, 0, 0, "");
        let line = creature_line(&o.objective(), 1, None, &probe_strings).unwrap();
        assert_eq!(line.text, "<FOUND ... 1 of 1>");
        assert_eq!(line.kind, "object");
        assert!(line.finished); // cur (1) >= req (1)
    }

    #[test]
    fn go_objective_prefers_its_custom_text_over_the_fallback() {
        let go_id = ((-57i32) as u32) | 0x8000_0000;
        let o = obj(go_id, 4, 0, 0, "Destroy the barricade");
        let line = creature_line(&o.objective(), 1, None, &probe_strings).unwrap();
        assert_eq!(line.text, "<FOUND Destroy the barricade 1 of 4>");
        assert_eq!(line.kind, "object");
    }

    #[test]
    fn item_objective_counts_bags_and_clamps_to_required() {
        let o = obj(0, 0, 2000, 5, "");
        // 8 carried, 5 required: the line clamps.
        let line = item_line(o.item(), 0, Some("Kobold Ear"), 8, &probe_strings).unwrap();
        assert_eq!(line.text, "<NEEDED Kobold Ear 5 of 5>");
        assert_eq!(line.kind, "item");
        assert!(line.finished);
    }

    #[test]
    fn item_objective_in_flight_shows_a_placeholder() {
        let o = obj(0, 0, 2000, 5, "");
        let line = item_line(o.item(), 0, None, 2, &probe_strings).unwrap();
        assert_eq!(line.text, "<NEEDED ... 2 of 5>");
    }

    /// An override takes `QUEST_OBJECTS_FOUND`; `type` is `"monster"`.
    #[test]
    fn custom_text_overrides_the_creature_auto_line() {
        let o = obj(100, 10, 0, 0, "Slay the vermin");
        let line = creature_line(&o.objective(), 4, Some("Kobold Vermin"), &probe_strings).unwrap();
        assert_eq!(line.text, "<FOUND Slay the vermin 4 of 10>");
        assert_eq!(line.kind, "monster");
    }

    /// Against the real `GlobalStrings.lua`; skips without client data.
    #[test]
    fn the_leaderboard_keys_resolve_in_the_real_global_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let vm = UiScript::new().expect("VM");
        vm.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let get = |key: &str| {
            vm.lua()
                .globals()
                .get::<String>(key)
                .ok()
                .filter(|t| !t.is_empty())
        };
        for key in [
            "QUEST_MONSTERS_KILLED",
            "QUEST_OBJECTS_FOUND",
            "QUEST_ITEMS_NEEDED",
        ] {
            assert!(get(key).is_some(), "{key} missing");
        }
        let o = obj(100, 10, 0, 0, "");
        let line = creature_line(&o.objective(), 3, Some("Kobold Vermin"), &get).unwrap();
        assert_eq!(line.text, "Kobold Vermin slain: 3/10");
    }

    /// A line's `finished` is `cur >= req` alone: `GetQuestLogLeaderBoard` reads the COMPLETE bit
    /// only for the `event` line; the turn-in predicate reads both.
    #[test]
    fn a_complete_quest_does_not_mark_a_short_objective_finished() {
        let o = obj(100, 10, 0, 0, "");
        // 1 of 10: not finished, whatever the quest's state.
        let line = creature_line(&o.objective(), 1, Some("Kobold"), &probe_strings).unwrap();
        assert_eq!(line.text, "<KILLED Kobold 1 of 10>");
        assert!(!line.finished);
    }

    #[test]
    fn empty_objective_slot_emits_no_line() {
        let o = obj(0, 0, 0, 0, "");
        assert!(creature_line(&o.objective(), 0, None, &probe_strings).is_none());
        assert!(item_line(o.item(), 0, None, 0, &probe_strings).is_none());
    }

    // ── build_objectives ────────────────────────────────────────────────────────────────────────

    fn quest_template(rows: [Row; 4]) -> QuestTemplate {
        let mut required_items = [(0, 0); 6];
        for (slot, row) in required_items.iter_mut().zip(&rows) {
            *slot = row.item();
        }
        QuestTemplate {
            quest_id: 1,
            level: 1,
            title: "A Threat Within".into(),
            objectives: rows.map(|r| r.objective()),
            required_items,
            ..Default::default()
        }
    }

    #[test]
    fn build_objectives_reads_every_occupied_quad_via_the_slots_counters_and_skips_empties() {
        let template = quest_template([
            obj(100, 10, 0, 0, ""), // creature objective, counter[0]
            obj(0, 0, 0, 0, ""),    // unused quad: no line
            obj(0, 0, 2000, 5, ""), // item objective (counted from the bags, not a counter)
            obj(0, 0, 0, 0, ""),    // unused quad
        ]);
        let log_slot = QuestLogSlot {
            quest_id: 1,
            counters: [3, 0, 0, 0],
            state: 0,
            timer: 0,
        };
        let store = ObjectFields::default(); // empty bags: the item objective reads 0/5
        let items = Items::default();
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let names = NameCache::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);

        let objectives = build_objectives(
            &template,
            &log_slot,
            &store,
            &objects,
            &items,
            &names,
            &commands,
            &probe_strings,
            &ObjectiveExtras::default(),
        );

        // The unused quads emit nothing; no names are seeded, so both lines wear the placeholder.
        assert_eq!(objectives.len(), 2);
        assert_eq!(objectives[0].text, "<KILLED ... 3 of 10>");
        assert_eq!(objectives[0].kind, "monster");
        assert!(!objectives[0].finished);
        assert_eq!(objectives[1].text, "<NEEDED ... 0 of 5>");
        assert_eq!(objectives[1].kind, "item");
    }

    /// Creatures and items are separate passes, so a quad carrying both gives two lines. The
    /// fixture is quest 358's shape: creature 1941 ×8 and item 2834 ×8 at index 0, a second
    /// creature at 1.
    #[test]
    fn a_quad_carrying_both_a_creature_and_an_item_emits_both_lines() {
        let template = quest_template([
            obj(1941, 8, 2834, 8, ""), // both kinds, one index
            obj(1675, 5, 0, 0, ""),
            obj(0, 0, 0, 0, ""),
            obj(0, 0, 0, 0, ""),
        ]);
        let log_slot = QuestLogSlot {
            quest_id: 358,
            counters: [8, 1, 0, 0],
            state: 0,
            timer: 0,
        };
        let store = ObjectFields::default();
        let items = Items::default();
        // Empty bags: every item objective reads 0.
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let names = NameCache::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);

        let objectives = build_objectives(
            &template,
            &log_slot,
            &store,
            &objects,
            &items,
            &names,
            &commands,
            &probe_strings,
            &ObjectiveExtras::default(),
        );

        // Pass order: both creatures, then the item.
        assert_eq!(objectives.len(), 3, "{objectives:#?}");
        assert_eq!(objectives[0].text, "<KILLED ... 8 of 8>");
        assert_eq!(objectives[1].text, "<KILLED ... 1 of 5>");
        assert_eq!(objectives[2].text, "<NEEDED ... 0 of 8>");
        assert_eq!(objectives[2].kind, "item");
    }

    /// The override text is the creature branch's, never the item line's.
    #[test]
    fn a_shared_quads_objective_text_names_the_creature_not_the_item() {
        let template = quest_template([
            obj(1941, 8, 2834, 8, "Graverobbers routed"),
            obj(0, 0, 0, 0, ""),
            obj(0, 0, 0, 0, ""),
            obj(0, 0, 0, 0, ""),
        ]);
        let log_slot = QuestLogSlot {
            quest_id: 358,
            counters: [8, 0, 0, 0],
            state: 0,
            timer: 0,
        };
        let store = ObjectFields::default();
        let items = Items::default();
        // Empty bags: every item objective reads 0.
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let names = NameCache::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);

        let objectives = build_objectives(
            &template,
            &log_slot,
            &store,
            &objects,
            &items,
            &names,
            &commands,
            &probe_strings,
            &ObjectiveExtras::default(),
        );
        assert_eq!(objectives.len(), 2);
        // The override is the creature's name, under `QUEST_OBJECTS_FOUND`.
        assert_eq!(objectives[0].text, "<FOUND Graverobbers routed 8 of 8>");
        // The item line wears the in-flight placeholder, never the creature's text.
        assert_eq!(objectives[1].text, "<NEEDED ... 0 of 8>");
    }

    /// The quest's own source item is no line when one is enough (`0x5e16d9`).
    #[test]
    fn the_quests_own_source_item_is_not_a_leaderboard_line() {
        let mut template = quest_template([
            obj(0, 0, 2834, 1, ""),
            obj(0, 0, 0, 0, ""),
            obj(0, 0, 0, 0, ""),
            obj(0, 0, 0, 0, ""),
        ]);
        template.src_item_id = 2834;
        let log_slot = QuestLogSlot {
            quest_id: 1,
            counters: [0; 4],
            state: 0,
            timer: 0,
        };
        let store = ObjectFields::default();
        let items = Items::default();
        // Empty bags: every item objective reads 0.
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let names = NameCache::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);

        let objectives = build_objectives(
            &template,
            &log_slot,
            &store,
            &objects,
            &items,
            &names,
            &commands,
            &probe_strings,
            &ObjectiveExtras::default(),
        );
        assert!(objectives.is_empty(), "{objectives:#?}");
    }

    #[test]
    fn a_complete_slot_state_does_not_finish_a_short_line() {
        let template = quest_template([
            obj(100, 10, 0, 0, ""),
            obj(0, 0, 0, 0, ""),
            obj(0, 0, 0, 0, ""),
            obj(0, 0, 0, 0, ""),
        ]);
        let log_slot = QuestLogSlot {
            quest_id: 1,
            counters: [1, 0, 0, 0],            // 1 of the 10 required
            state: quest_slot_state::COMPLETE, // yet the quest is complete
            timer: 0,
        };
        let store = ObjectFields::default();
        let items = Items::default();
        // Empty bags: every item objective reads 0.
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let names = NameCache::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);

        let objectives = build_objectives(
            &template,
            &log_slot,
            &store,
            &objects,
            &items,
            &names,
            &commands,
            &probe_strings,
            &ObjectiveExtras::default(),
        );
        assert_eq!(objectives.len(), 1);
        assert!(
            !objectives[0].finished,
            "the counter is 1/10 — the quest's COMPLETE bit is not this line's business"
        );
        assert_eq!(objectives[0].text, "<KILLED ... 1 of 10>");
    }

    // ── money_split ──────────────────────────────────────────────────────────────────────────────

    #[test]
    fn money_sign_splits_into_required_or_reward() {
        assert_eq!(money_split(-500), (500, 0));
        assert_eq!(money_split(500), (0, 500));
        assert_eq!(money_split(0), (0, 0));
    }

    // ── abandon_slot ────────────────────────────────────────────────────────────────────────────

    #[test]
    fn abandon_maps_quest_id_to_slot_across_a_gap() {
        // Slots 0 and 2, slot 1 empty: each id finds its own slot.
        let row_slots = vec![(783u32, 0u8), (7, 2)];
        assert_eq!(abandon_slot(783, &row_slots), Some(0));
        assert_eq!(abandon_slot(7, &row_slots), Some(2));
        assert_eq!(abandon_slot(8, &row_slots), None); // not in the log: nothing to remove
        assert_eq!(abandon_slot(0, &row_slots), None); // 0 is never a quest
    }

    // ── The abandon mark is a quest id ──────────────────────────────────────────────────────────

    /// `SetAbandonQuest` copies the selection, a quest id, into the mark, and `AbandonQuest` finds
    /// its row again, so a fold while the popup is up cannot retarget the abandon.
    #[test]
    fn a_fold_between_mark_and_confirm_does_not_retarget_the_abandon() {
        use benilla_protocol::messages::field::FIELD_PLAYER_QUEST_LOG_1_1;

        // Z1 = zone 0 ("Missing header!", forced first) holding A; Z2 = zone 7 holding B and C.
        // Entries: [Z1, A, Z2, B, C]; B is row 4.
        const A: u32 = 101;
        const B: u32 = 201;
        const C: u32 = 202;
        let quests = [(0u8, A, 0i32, "A"), (1, B, 7, "B"), (2, C, 7, "C")];

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx));
        app.init_resource::<crate::net::GuidIndex>()
            .init_resource::<NameCache>()
            .init_resource::<Items>()
            .init_resource::<crate::world_state::WorldStates>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::ui_quest::QuestLines>();
        let mut log = QuestLog::default();
        let mut pairs = Vec::new();
        for (slot, id, zos, title) in quests {
            let mut t = quest_template(std::array::from_fn(|_| obj(0, 0, 0, 0, "")));
            t.quest_id = id;
            t.zone_or_sort = zos;
            t.title = title.into();
            log.insert_template(t);
            pairs.push((
                FIELD_PLAYER_QUEST_LOG_1_1
                    + benilla_protocol::messages::QUEST_LOG_SLOT_FIELDS * u16::from(slot),
                id,
            ));
        }
        app.insert_resource(log);
        app.world_mut().spawn((
            ObjectStore(ObjectFields::from_pairs(&pairs)),
            Guid(0x2A),
            SelfPlayer,
        ));
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.add_systems(
            Update,
            (
                drain_quest_log_collapses,
                feed_quest_log,
                drain_quest_log_abandons,
            )
                .chain(),
        );
        let run = |app: &mut App, lua: &str| {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .run(lua)
                .unwrap();
        };

        app.update();
        run(&mut app, "SelectQuestLogEntry(4); SetAbandonQuest()");
        assert_eq!(
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<String>("return GetAbandonQuestName()")
                .unwrap(),
            "B"
        );
        // With the popup up, fold Z1: B moves to row 3 and C takes row 4.
        run(&mut app, "CollapseQuestHeader(1)");
        app.update();
        run(&mut app, "AbandonQuest()");
        app.update();

        let sent: Vec<_> = rx
            .try_iter()
            .filter(|c| matches!(c, ClientCommand::QuestlogRemove { .. }))
            .collect();
        assert!(
            matches!(sent.as_slice(), [ClientCommand::QuestlogRemove { slot: 1 }]),
            "the abandon removes B's slot (1), never C's (2): {sent:?}"
        );
    }

    // ── The accept line ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn the_slot_watcher_announces_a_filled_slot_and_a_cleared_failure_only() {
        use quest_slot_state::FAIL;
        let prior = [(0, 0), (7, 0), (8, FAIL), (9, FAIL), (10, 0), (11, 0)];
        let now = [(783, 0), (7, 0), (8, 0), (9, FAIL), (12, 0), (0, 0)];
        assert_eq!(
            accepted_quests(&prior, &now),
            [783, 8],
            "an empty slot filled, and the same quest with its fail bit cleared; a kept quest, a \
             kept failure, a slot swapping quests and a cleared slot say nothing"
        );
    }

    /// The watcher runs on the log's slots, not its rows: an accepted quest is announced before its
    /// template lands, and the login snapshot is silent.
    #[test]
    fn a_quest_entering_the_log_queues_its_accept_line() {
        use crate::ui_quest::{QuestLine, QuestLines};
        use benilla_protocol::messages::field::FIELD_PLAYER_QUEST_LOG_1_1;

        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx));
        app.init_resource::<crate::net::GuidIndex>()
            .init_resource::<NameCache>()
            .init_resource::<Items>()
            .init_resource::<crate::world_state::WorldStates>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<QuestLines>()
            .init_resource::<QuestLog>();
        let player = app
            .world_mut()
            .spawn((
                ObjectStore(ObjectFields::from_pairs(&[(FIELD_PLAYER_QUEST_LOG_1_1, 7)])),
                Guid(0x2A),
                SelfPlayer,
            ))
            .id();
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.add_systems(Update, feed_quest_log);

        app.update();
        assert!(
            app.world().resource::<QuestLines>().queued().is_empty(),
            "the login snapshot fills the log silently"
        );

        app.world_mut()
            .entity_mut(player)
            .insert(ObjectStore(ObjectFields::from_pairs(&[
                (FIELD_PLAYER_QUEST_LOG_1_1, 7),
                (
                    FIELD_PLAYER_QUEST_LOG_1_1 + benilla_protocol::messages::QUEST_LOG_SLOT_FIELDS,
                    783,
                ),
            ])));
        app.update();
        assert_eq!(
            app.world().resource::<QuestLines>().queued(),
            [QuestLine::Accepted(783)]
        );
        assert!(
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .take_sounds()
                .is_empty(),
            "the sound is the accept row's, played with its line"
        );
    }

    /// An idle frame rebuilds nothing: the log resource, which the rebuild rewrites, stays
    /// unmarked until an input moves.
    #[test]
    fn an_idle_frame_leaves_the_quest_log_unwritten() {
        use crate::ui_quest::QuestLines;
        use benilla_protocol::messages::field::FIELD_PLAYER_QUEST_LOG_1_1;

        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx));
        app.init_resource::<crate::net::GuidIndex>()
            .init_resource::<NameCache>()
            .init_resource::<Items>()
            .init_resource::<crate::world_state::WorldStates>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<QuestLines>()
            .init_resource::<QuestLog>();
        let player = app
            .world_mut()
            .spawn((
                ObjectStore(ObjectFields::from_pairs(&[(FIELD_PLAYER_QUEST_LOG_1_1, 7)])),
                Guid(0x2A),
                SelfPlayer,
            ))
            .id();
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_quest::QuestGiver>()
            .init_resource::<crate::go_templates::GameObjectTemplates>();
        // The item-stats and quest feeds share caches others are gated on: idle, they must not
        // mark them.
        app.add_systems(
            Update,
            (
                crate::ui_items::feed::feed_item_stats,
                crate::ui_quest::feed_quest,
                feed_quest_log,
            )
                .chain(),
        );
        app.update();
        app.update();
        let items_marked = app.world().resource_ref::<Items>().last_changed();
        let giver_marked = app
            .world()
            .resource_ref::<crate::ui_quest::QuestGiver>()
            .last_changed();
        let marked = |app: &App| app.world().resource_ref::<QuestLog>().last_changed();
        let settled = marked(&app);
        app.update();
        app.update();
        assert_eq!(marked(&app), settled, "nothing moved: no rebuild");
        assert_eq!(
            app.world().resource_ref::<Items>().last_changed(),
            items_marked,
            "an idle item feed leaves the cache unmarked"
        );
        assert_eq!(
            app.world()
                .resource_ref::<crate::ui_quest::QuestGiver>()
                .last_changed(),
            giver_marked,
            "an idle quest feed leaves the giver unmarked"
        );

        app.world_mut()
            .get_mut::<ObjectStore>(player)
            .unwrap()
            .set_changed();
        app.update();
        assert_ne!(marked(&app), settled, "the player's store moved: rebuilt");
    }
}
