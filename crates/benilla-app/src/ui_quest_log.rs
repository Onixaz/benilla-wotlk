//! The quest log's app side: the template cache, the row table the rebuild writes into the VM ([`rows`]) and the per-row answers the verbs compute at the call. The progress lines and `QUEST_WATCH_UPDATE` come from the packets ([`progress`]). Abandons and shares drain from here.

mod progress;
pub(crate) mod rows;
pub(crate) mod tooltip;

use crate::ui_items::{count_of, InventoryScope};
use std::collections::HashSet;

use bevy::prelude::*;

use benilla_protocol::messages::{quest_slot_state, QuestLogSlot, QuestObjective, QuestTemplate};
use benilla_protocol::ObjectFields;
#[cfg(test)]
use benilla_ui::script::carry_collapse_filter;
use benilla_ui::script::{QuestItemView, QuestLogDetail, QuestLogObjectiveView, UiScript};
use benilla_ui::strings::{fill, Arg};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::query_cache::QueryCache;
use crate::ui_action::Spells;
use crate::ui_script::UiDrain;
use rows::QuestAsk;

/// Marks a GameObject objective's entry on the wire (vmangos `Quest.cpp:512-516`).
const GO_OBJECTIVE_BIT: u32 = 0x8000_0000;

/// The template cache, asked once per quest id. Only handlers and the cache's own systems write it, so the live view lends it to every verb.
#[derive(Resource, Default)]
pub(crate) struct QuestLog {
    templates: QueryCache<u32, QuestTemplate>,
}

/// The log's marks beside the templates: the accepts waiting on theirs and the progress packets not yet announced.
#[derive(Resource, Default)]
pub(crate) struct QuestLogMarks {
    /// The accepts whose template the slot watch asked for (`0x6df414`).
    accepts: HashSet<u32>,
    /// The progress packets not yet announced, in arrival order.
    notices: Vec<progress::QuestNotice>,
}

/// Quests marked expired, by a `_FAILEDTIMER` (`0x5e5840`) or a timer run out on the client (`0x5e57e0`), until they leave the log. Main-thread state `GetQuestTimers` marks inside its call (`0x5e634a`), lent to it.
#[derive(Default)]
pub(crate) struct QuestExpiries {
    expired: HashSet<u32>,
    /// Quests whose expiry line waits on their template, shown by its answer (`0x5de980`).
    failed_lines: HashSet<u32>,
}

impl QuestExpiries {
    /// Whether `quest_id` is marked expired (`0x5dee60`).
    pub(crate) fn is_expired(&self, quest_id: u32) -> bool {
        self.expired.contains(&quest_id)
    }

    /// Mark `quest_id` expired; whether it was not already.
    pub(crate) fn mark(&mut self, quest_id: u32) -> bool {
        self.expired.insert(quest_id)
    }

    /// Keep only the marks `keep` accepts: a mark lives on its log row and leaves with it.
    fn retain(&mut self, keep: impl Fn(u32) -> bool) {
        self.expired.retain(|&id| keep(id));
    }

    /// Whether `quest_id`'s expiry line waited on the template that just landed.
    fn take_waiting(&mut self, quest_id: u32) -> bool {
        self.failed_lines.remove(&quest_id)
    }

    /// Disconnect: drops every mark.
    pub(crate) fn clear(&mut self) {
        self.expired.clear();
        self.failed_lines.clear();
    }

    /// `GetQuestTimers`' expiry of a run-out timer, inside the call: the mark (`0x5e634a`), every questgiver's status asked again (`0x5e582a`), and the quest's title for `ERR_QUEST_FAILED_S` when its template is cached; a miss asks it with a callback that shows the line on the answer (`0x5e635c`, `0x5de980`). `None` when the quest was already marked.
    pub(crate) fn expire_in_call(
        &mut self,
        quest_id: u32,
        log: &QuestLog,
        commands: &NetCommands,
    ) -> Option<Option<String>> {
        if !self.mark(quest_id) {
            return None;
        }
        let _ = commands
            .0
            .send(ClientCommand::QuestgiverStatusMultipleQuery);
        match log.template(quest_id, commands) {
            Some(t) => Some(Some(t.title.clone())),
            None => {
                self.failed_lines.insert(quest_id);
                Some(None)
            }
        }
    }
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

    /// Whether the server answered `quest_id` as unknown, not still pending.
    pub(crate) fn template_answered_unknown(&self, quest_id: u32) -> bool {
        self.templates.answered_unknown(quest_id)
    }

    /// Record a template answer (`SMSG_QUEST_QUERY_RESPONSE`).
    pub(crate) fn insert_template(&mut self, template: QuestTemplate) {
        self.templates.insert(template.quest_id, Some(template));
    }

    /// Disconnect: drops every template.
    pub(crate) fn clear_session(&mut self) {
        self.templates.clear();
    }
}

impl QuestLogMarks {
    /// Disconnect: drops every mark.
    pub(crate) fn clear_session(&mut self) {
        self.accepts.clear();
        self.notices.clear();
    }
}

/// The quest log's packet handlers.
mod net {
    use benilla_protocol::messages::QuestTemplate;
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::rows::{QuestAsk, QuestAsks, RebuildMarks};
    use super::{QuestExpiries, QuestLog, QuestLogMarks};
    use crate::net::NetHandlerApp;
    use crate::ui_script::live::{UiEvents, UniqueSignals};

    /// Registers the template handler, the record answers' and the session-end listener.
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::QuestTemplate, on_template)
            .net_handler(K::CreatureName, on_record)
            .net_handler(K::GameObjectInfo, on_record)
            .net_handler(K::ServerUnixTime, on_server_time)
            .net_handler(K::Disconnected, on_session_end);
    }

    /// A creature or GameObject record a quest verb asked for landed: `QUEST_LOG_UPDATE` waits as a unique signal (`0x5de9fa`, `0x5dea2a`); an item record's runs on the item cache's list.
    fn on_record(
        In(ev): In<SessionEvent>,
        mut asks: NonSendMut<QuestAsks>,
        mut signals: NonSendMut<UniqueSignals>,
    ) {
        let ask = match ev {
            SessionEvent::CreatureName {
                entry,
                name: Some(_),
                ..
            } => QuestAsk::Creature(entry),
            SessionEvent::GameObjectInfo { entry, .. } => QuestAsk::GameObject(entry),
            _ => return,
        };
        if asks.landed(ask) {
            signals.unique_signal("QUEST_LOG_UPDATE");
        }
    }

    /// `SMSG_QUERY_TIME_RESPONSE` sets the log's clock (`0x5dff40`), arms the rebuild's re-ask (`0x5dff77`), then queues `QUEST_LOG_UPDATE` (`0x5dff89`) in turn with the frame's other unique signals.
    fn on_server_time(
        In(ev): In<SessionEvent>,
        clock: Option<Res<crate::net::ServerWallClock>>,
        mut rebuild: NonSendMut<RebuildMarks>,
        mut signals: NonSendMut<UniqueSignals>,
    ) {
        if let SessionEvent::ServerUnixTime { .. } = ev {
            rebuild.arm_reask(clock.and_then(|c| c.reask_at()));
            signals.unique_signal("QUEST_LOG_UPDATE");
        }
    }

    /// The answer lands in the cache, then its callbacks run: an expiry line that waited on it (`0x5de980`), a verb's ask, a pending accept and the rebuild's counter ([`super::rows::template_landed`]).
    fn on_template(
        In(ev): In<SessionEvent>,
        mut quest_log: ResMut<QuestLog>,
        (mut expiries, mut asks, mut signals): (
            NonSendMut<QuestExpiries>,
            NonSendMut<QuestAsks>,
            NonSendMut<UniqueSignals>,
        ),
        (mut marks, mut rebuild): (ResMut<QuestLogMarks>, NonSendMut<RebuildMarks>),
        mut ui: ResMut<UiEvents>,
    ) {
        if let SessionEvent::QuestTemplate(t) = ev {
            let (id, title) = (t.quest_id, t.title.clone());
            quest_template(t, &mut quest_log);
            super::rows::template_landed(
                id,
                &mut marks,
                &mut rebuild,
                &mut asks,
                &mut signals,
                &mut ui,
            );
            ui.world(move |world| crate::ui_quest::lines::quest_landed(world, id));
            if expiries.take_waiting(id) {
                crate::ui_action::show_in_turn(
                    &mut ui,
                    "ui_quest_log",
                    crate::ui_action::UiError::s("ERR_QUEST_FAILED_S", title),
                );
            }
        }
    }

    /// The log's state dies with the socket: a second handler on the kind, after the bridge's own.
    fn on_session_end(
        In(_): In<SessionEvent>,
        mut quest_log: ResMut<QuestLog>,
        mut marks: ResMut<QuestLogMarks>,
        (mut expiries, mut asks, mut rebuild): (
            NonSendMut<QuestExpiries>,
            NonSendMut<QuestAsks>,
            NonSendMut<RebuildMarks>,
        ),
    ) {
        quest_log.clear_session();
        marks.clear_session();
        expiries.clear();
        asks.clear();
        // The next game UI's start disarms the re-ask (`0x5e71ee`) with the rest of the rebuild's marks.
        rebuild.clear();
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
        tooltip::register(app);
        crate::query_cache::register::<QuestLog>(app);
        app.init_resource::<QuestLog>()
            .init_resource::<QuestLogMarks>()
            .init_non_send_resource::<rows::RebuildMarks>()
            .init_non_send_resource::<QuestExpiries>()
            .init_non_send_resource::<rows::QuestAsks>()
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
                    drain_quest_log_abandons.in_set(UiDrain),
                    drain_quest_log_pushes.in_set(UiDrain),
                    progress::drain_quest_progress_sends.in_set(UiDrain),
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
        intermediate: false,
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
        intermediate: false,
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
    miss: &dyn Fn(QuestAsk),
) -> QuestItemView {
    let template = items.template(entry, 0, commands).cloned();
    if template.is_none() {
        miss(QuestAsk::Item(entry));
    }
    let name = template.as_ref().map(|t| t.name.clone());
    let quality = template.as_ref().map(|t| t.quality).unwrap_or(1);
    let texture = template
        .as_ref()
        .and_then(|t| crate::ui_items::item_icon(icons, t.display_info_id));
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
        usable: true, // the getter's can-use test (`0x5e45eb`) clears it
        link,
    }
}

/// What the leaderboard's later lines read beside the template: the GameObject names, the
/// reputation objective, the player's sex for its standing labels, and the completed quest's `log`
/// line.
#[derive(Default)]
pub(crate) struct ObjectiveExtras<'a> {
    pub(crate) gos: Option<&'a crate::go_templates::GameObjectTemplates>,
    /// `(faction name, current rank, needed rank, met)`, ranks 0..=7.
    pub(crate) reputation: Option<(String, u8, u8, bool)>,
    pub(crate) female: bool,
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

/// What the completion test reads beside the template and the slot.
pub(crate) struct CompletionInputs<'a> {
    /// An item's count in the bags and the bank (`0x754d00` with mask 8).
    pub(crate) carried: &'a dyn Fn(u32) -> u32,
    /// A faction's standing (`0x5d05b0`).
    pub(crate) standing: &'a dyn Fn(u32) -> i32,
    /// The player's coinage.
    pub(crate) money: u64,
}

/// The completion test (`0x5e0ea0`, with the second argument 1 every caller here passes): a
/// failed or expired quest, and one with no requirement at all, read incomplete; an exploration
/// or escort quest (`flags & 6`) needs the slot's complete bit; then the friendly standing at or
/// above its value, the opposite one at or below its own, the money a negative reward demands,
/// every kill counter, every required item in the bags and the players slain.
pub(crate) fn quest_complete(
    t: &QuestTemplate,
    slot: &QuestLogSlot,
    expired: bool,
    w: &CompletionInputs<'_>,
) -> bool {
    if slot.state & quest_slot_state::FAIL != 0 || expired {
        return false;
    }
    let special = t.flags & 6 != 0;
    if !special && t.rep_objective_faction == 0 && t.opposite_rep_faction == 0 && t.money == 0 {
        let any = t.objectives.iter().any(|o| o.creature_or_go != 0)
            || t.required_items.iter().any(|&(id, _)| id != 0);
        if !any && t.players_slain == 0 {
            return false;
        }
    }
    if special && slot.state & quest_slot_state::COMPLETE == 0 {
        return false;
    }
    if t.rep_objective_faction != 0
        && (w.standing)(t.rep_objective_faction) < t.rep_objective_value as i32
    {
        return false;
    }
    if t.opposite_rep_faction != 0
        && (w.standing)(t.opposite_rep_faction) > t.opposite_rep_value as i32
    {
        return false;
    }
    if t.money < 0 && w.money < u64::from(t.money.unsigned_abs()) {
        return false;
    }
    let kills = t
        .objectives
        .iter()
        .zip(slot.counters)
        .all(|(o, n)| o.creature_or_go == 0 || u32::from(n) >= o.required_count);
    let items = t
        .required_items
        .iter()
        .all(|&(id, req)| id == 0 || (w.carried)(id) >= req);
    kills && items && i32::from(slot.counters[0]) >= t.players_slain as i32
}

/// The objective-less test (`0x5e10b0`): not failed or expired, no kill row, no required item but
/// a lone starting item, no players slain, no exploration or escort flag, no faction, and no
/// money demanded.
pub(crate) fn no_objectives(t: &QuestTemplate, slot: &QuestLogSlot, expired: bool) -> bool {
    slot.state & quest_slot_state::FAIL == 0
        && !expired
        && t.objectives.iter().all(|o| o.creature_or_go == 0)
        && t.required_items
            .iter()
            .all(|&(id, req)| id == 0 || (id == t.src_item_id && req as i32 <= 1))
        && t.players_slain == 0
        && t.flags & 6 == 0
        && t.rep_objective_faction == 0
        && t.opposite_rep_faction == 0
        && t.money >= 0
}

/// Whether a slot's timer ran out on the server clock, as the rebuild tests it (`0x5e6aef`): a
/// timed, unfailed slot whose `deadline - now - 1` is negative.
fn timer_run_out(slot: &QuestLogSlot, now: Option<f64>) -> bool {
    slot.timer != 0
        && slot.state & quest_slot_state::FAIL == 0
        && now.is_some_and(|now| (f64::from(slot.timer) - now - 1.0).floor() < 0.0)
}

/// An item template's bonding for a quest item (`[+0x178] == 4`, `0x5e3e19`).
const BIND_QUEST: u32 = 4;

/// `GetAbandonQuestItems` for `t` (`0x5e3d60`): its required and starting items carried and bound
/// to the quest, and its intermediate drops carried, less every item another logged quest requires
/// or starts with, by name in that order, joined by `", "`. `item` is an item's name and bonding
/// from the cache.
fn abandon_items(
    t: &QuestTemplate,
    logged: &[&QuestTemplate],
    carried: &dyn Fn(u32) -> u32,
    item: &dyn Fn(u32) -> Option<(String, u32)>,
) -> Option<String> {
    let mut ids: Vec<u32> = Vec::new();
    let bound = |id: u32| item(id).is_some_and(|(_, bonding)| bonding == BIND_QUEST);
    let starting = (t.src_item_id, 0);
    for &(id, _) in t.required_items.iter().chain(std::iter::once(&starting)) {
        if id as i32 > 0 && carried(id) > 0 && bound(id) && !ids.contains(&id) {
            ids.push(id);
        }
    }
    for o in &t.objectives {
        if o.item_drop as i32 > 0 && carried(o.item_drop) > 0 && !ids.contains(&o.item_drop) {
            ids.push(o.item_drop);
        }
    }
    for other in logged.iter().filter(|o| o.quest_id != t.quest_id) {
        let claimed =
            |id: u32| other.required_items.iter().any(|&(r, _)| r == id) || other.src_item_id == id;
        ids.retain(|&id| !claimed(id));
    }
    // An uncached drop has no name to list; the reference reads a null record there.
    let names: Vec<String> = ids
        .iter()
        .filter_map(|&id| item(id).map(|(n, _)| n))
        .collect();
    (!names.is_empty()).then(|| names.join(", "))
}

/// One quest's objective lines, for every quest, as the watch frame reads any quest's. The
/// leaderboard (`0x5e1460`) walks `event` (the end text), the four creature or GameObject rows,
/// the six required items, the intermediate drops while the quest is incomplete, then one `player`
/// line, else one `reputation` line, else a done quest's objectives text as its `log` line.
/// `complete` is the completion test's answer for the quest.
#[allow(clippy::too_many_arguments)] // the leaderboard's whole input set
fn build_objectives(
    template: &QuestTemplate,
    log_slot: &QuestLogSlot,
    complete: bool,
    carried: &dyn Fn(u32) -> u32,
    items: &Items,
    names: &NameCache,
    commands: &NetCommands,
    // The VM's own `GlobalStrings.lua`, for the leaderboard's format keys.
    get: &dyn Fn(&str) -> Option<String>,
    extras: &ObjectiveExtras<'_>,
    // Each record the walk asked for, its answer owing `QUEST_LOG_UPDATE`.
    miss: &dyn Fn(QuestAsk),
) -> Vec<QuestLogObjectiveView> {
    let mut objectives = Vec::with_capacity(template.objectives.len());
    // The end text, verbatim, done when the slot's state is.
    if !template.end_text.is_empty() {
        let done = log_slot.state & quest_slot_state::COMPLETE != 0;
        objectives.push(QuestLogObjectiveView {
            text: template.end_text.clone(),
            kind: "event".into(),
            finished: done,
            intermediate: false,
            cur: u32::from(done),
            req: 1,
        });
    }
    // Creature or GameObject: the counter index is the array index, empty elements included.
    for (i, obj) in template.objectives.iter().enumerate() {
        let is_go = obj.creature_or_go & GO_OBJECTIVE_BIT != 0;
        let name = match (obj.creature_or_go, is_go) {
            (0, _) => None,
            (entry, false) => {
                let name = names.resolve_creature(entry, 0, commands);
                if name.is_none() {
                    miss(QuestAsk::Creature(entry));
                }
                name.map(str::to_string)
            }
            (entry, true) => {
                let entry = entry & !GO_OBJECTIVE_BIT;
                let name = extras.gos.and_then(|g| g.name_by_entry(entry, commands));
                if name.is_none() {
                    miss(QuestAsk::GameObject(entry));
                }
                name.map(str::to_string)
            }
        };
        if let Some(line) = creature_line(obj, log_slot.counters[i], name.as_deref(), get) {
            objectives.push(line);
        }
    }
    // Required items, a positive id each (`0x5e16d3`).
    for &(item_id, req) in template.required_items.iter() {
        if item_id as i32 <= 0 {
            continue;
        }
        let item_name = items.template(item_id, 0, commands).map(|t| t.name.clone());
        if item_name.is_none() {
            miss(QuestAsk::Item(item_id));
        }
        if let Some(line) = item_line(
            (item_id, req),
            template.src_item_id,
            item_name.as_deref(),
            carried(item_id),
            get,
        ) {
            objectives.push(line);
        }
    }
    // The intermediate drops, while incomplete (`0x5e1a6b`): one carried is enough.
    if !complete {
        for obj in &template.objectives {
            if obj.item_drop as i32 <= 0 {
                continue;
            }
            let name = items
                .template(obj.item_drop, 0, commands)
                .map(|t| t.name.clone());
            if name.is_none() {
                miss(QuestAsk::Item(obj.item_drop));
            }
            let have = carried(obj.item_drop);
            objectives.push(QuestLogObjectiveView {
                text: get("QUEST_INTERMEDIATE_ITEMS_NEEDED").map_or_else(String::new, |f| {
                    fill(
                        &f,
                        &[
                            Arg::S(name.as_deref().unwrap_or(NAME_PLACEHOLDER)),
                            Arg::D(i64::from(have)),
                        ],
                    )
                }),
                kind: "item".into(),
                finished: have >= 1,
                intermediate: true,
                cur: have.min(1),
                req: 1,
            });
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
            intermediate: false,
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
            intermediate: false,
            cur: u32::from(*met),
            req: 1,
        });
    } else if complete {
        // Nothing left to name: the objectives text, once the quest is done (`0x5e1f5e`).
        objectives.push(QuestLogObjectiveView {
            text: extras.objectives_text.clone(),
            kind: "log".into(),
            finished: true,
            intermediate: false,
            cur: 1,
            req: 1,
        });
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
    miss: &dyn Fn(QuestAsk),
) -> QuestLogDetail {
    let (required_money, reward_money) = money_split(template.money);
    let rewards = template
        .rewards
        .iter()
        .filter(|&&(id, _)| id != 0)
        .map(|&(id, count)| resolve_template_item(id, count, items, icons, commands, miss))
        .collect();
    let choices = template
        .choices
        .iter()
        .filter(|&&(id, _)| id != 0)
        .map(|&(id, count)| resolve_template_item(id, count, items, icons, commands, miss))
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

/// `QuestLogPushQuest()`: the quest id taken at the click goes out; the server finds the party.
fn drain_quest_log_pushes(
    (script, _live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    for quest in script.take_quest_log_pushes() {
        debug!("ui_quest_log: sharing quest {quest} with the party");
        let _ = commands.0.send(ClientCommand::PushQuestToParty { quest });
    }
}

fn drain_quest_log_abandons(
    (script, _live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    for quest in script.take_quest_log_abandons() {
        match script.quest_log_slot(quest) {
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
    use crate::net::{Guid, ObjectStore, SelfPlayer};
    use benilla_protocol::messages::quest_flags;

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

    use benilla_ui::script::QuestLogRow;
    use rows::remap_selection;

    fn header(title: &str) -> QuestLogRow {
        QuestLogRow {
            title: title.into(),
            is_header: true,
            ..Default::default()
        }
    }

    fn quest(id: u32) -> QuestLogRow {
        QuestLogRow {
            quest_id: id,
            ..Default::default()
        }
    }

    #[test]
    fn selection_follows_its_quest_across_reindexing() {
        // A new header lands above: quest 9 moves to entry 5.
        let new = vec![header("A"), quest(7), header("B"), header("C"), quest(9)];
        assert_eq!(remap_selection(9, &new), 5);
    }

    /// The selection is a quest id (`0x5dffa0`): a fold moves it past the shown rows, it stays.
    #[test]
    fn a_folded_selection_follows_its_quest_past_the_shown_rows() {
        let mut folded = quest(7);
        folded.hidden = true;
        let new = vec![header("A"), header("B"), quest(9), folded];
        assert_eq!(remap_selection(7, &new), 4);
    }

    #[test]
    fn no_selection_and_a_quest_gone_stay_zero() {
        let new = vec![header("A"), quest(7)];
        assert_eq!(remap_selection(0, &new), 0);
        assert_eq!(remap_selection(99, &new), 0); // left the log: nothing to follow
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
        let items = Items::default();
        let names = NameCache::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);

        let objectives = build_objectives(
            &template,
            &log_slot,
            false,
            &|_| 0, // empty bags: every item objective reads 0
            &items,
            &names,
            &commands,
            &probe_strings,
            &ObjectiveExtras::default(),
            &|_| {},
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
        let items = Items::default();
        let names = NameCache::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);

        let objectives = build_objectives(
            &template,
            &log_slot,
            false,
            &|_| 0, // empty bags: every item objective reads 0
            &items,
            &names,
            &commands,
            &probe_strings,
            &ObjectiveExtras::default(),
            &|_| {},
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
        let items = Items::default();
        let names = NameCache::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);

        let objectives = build_objectives(
            &template,
            &log_slot,
            false,
            &|_| 0, // empty bags: every item objective reads 0
            &items,
            &names,
            &commands,
            &probe_strings,
            &ObjectiveExtras::default(),
            &|_| {},
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
        let items = Items::default();
        let names = NameCache::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);

        let objectives = build_objectives(
            &template,
            &log_slot,
            false,
            &|_| 0, // empty bags: every item objective reads 0
            &items,
            &names,
            &commands,
            &probe_strings,
            &ObjectiveExtras::default(),
            &|_| {},
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
        let items = Items::default();
        let names = NameCache::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);

        let objectives = build_objectives(
            &template,
            &log_slot,
            false,
            &|_| 0, // empty bags: every item objective reads 0
            &items,
            &names,
            &commands,
            &probe_strings,
            &ObjectiveExtras::default(),
            &|_| {},
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
            .init_resource::<crate::ui_quest::lines::TurnIns>()
            .init_non_send_resource::<crate::ui_script::live::UniqueSignals>();
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
        app.insert_resource(log)
            .init_resource::<QuestLogMarks>()
            .init_non_send_resource::<rows::RebuildMarks>()
            .init_non_send_resource::<QuestExpiries>();
        app.world_mut().spawn((
            ObjectStore(ObjectFields::from_pairs(&pairs)),
            Guid(0x2A),
            SelfPlayer,
        ));
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.add_systems(Update, (rows::rebuild, drain_quest_log_abandons).chain());
        let run = |app: &mut App, lua: &str| {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .run(lua)
                .unwrap();
        };

        app.update();
        run(&mut app, "SelectQuestLogEntry(4); SetAbandonQuest()");
        assert_eq!(
            crate::ui_script::live::with_live_world(app.world_mut(), |s| s
                .eval::<String>("return GetAbandonQuestName()")
                .unwrap()),
            Some("B".to_string())
        );
        // With the popup up, fold Z1: B moves to row 3, C takes row 4 and A sorts last.
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
    fn the_slot_watcher_announces_a_changed_slot_and_a_cleared_failure_only() {
        use quest_slot_state::FAIL;
        let prior = [(0, 0), (7, 0), (8, FAIL), (9, FAIL), (10, 0), (11, 0)];
        let now = [(783, 0), (7, 0), (8, 0), (9, FAIL), (12, 0), (0, 0)];
        let taken: Vec<u32> = prior
            .iter()
            .zip(now)
            .map(|(&was, now)| rows::SlotEdge { was, now })
            .filter(rows::SlotEdge::takes_quest)
            .map(|e| e.now.0)
            .collect();
        assert_eq!(
            taken,
            [783, 8, 12],
            "an empty slot filled, the same quest with its fail bit cleared and a slot swapping \
             quests; a kept quest, a kept failure and a cleared slot say nothing"
        );
    }

    // ── The completion test (`0x5e0ea0`) ───────────────────────────────────────────────────────

    fn slot(counters: [u16; 4], state: u8) -> QuestLogSlot {
        QuestLogSlot {
            quest_id: 1,
            counters,
            state,
            timer: 0,
        }
    }

    fn inputs<'a>(
        carried: &'a dyn Fn(u32) -> u32,
        standing: &'a dyn Fn(u32) -> i32,
        money: u64,
    ) -> CompletionInputs<'a> {
        CompletionInputs {
            carried,
            standing,
            money,
        }
    }

    fn empty() -> Row {
        obj(0, 0, 0, 0, "")
    }

    /// Every requirement is read from the counters, the bags, the standings and the purse; the
    /// slot's complete bit alone decides nothing.
    #[test]
    fn the_completion_test_counts_kills_items_players_reputation_and_money() {
        use quest_slot_state::{COMPLETE, FAIL};
        let mut t = quest_template([obj(100, 10, 2000, 5, ""), empty(), empty(), empty()]);
        let five = |id: u32| if id == 2000 { 5 } else { 0 };
        let none = |_: u32| 0;
        let neutral = |_: u32| 0;
        let done = slot([10, 0, 0, 0], 0);
        assert!(quest_complete(
            &t,
            &done,
            false,
            &inputs(&five, &neutral, 0)
        ));
        assert!(
            !quest_complete(
                &t,
                &slot([9, 0, 0, 0], COMPLETE),
                false,
                &inputs(&five, &neutral, 0)
            ),
            "a short counter, whatever the slot says"
        );
        assert!(!quest_complete(
            &t,
            &done,
            false,
            &inputs(&none, &neutral, 0)
        ));
        assert!(!quest_complete(
            &t,
            &slot([10, 0, 0, 0], FAIL),
            false,
            &inputs(&five, &neutral, 0)
        ));
        assert!(
            !quest_complete(&t, &done, true, &inputs(&five, &neutral, 0)),
            "expired"
        );

        // The friendly standing at or above its value, the opposite one at or below its own.
        t.rep_objective_faction = 72;
        t.rep_objective_value = 3000;
        t.opposite_rep_faction = 68;
        t.opposite_rep_value = (-3000i32) as u32;
        let rep = |hi: i32, lo: i32| {
            move |f: u32| {
                if f == 72 {
                    hi
                } else if f == 68 {
                    lo
                } else {
                    0
                }
            }
        };
        assert!(quest_complete(
            &t,
            &done,
            false,
            &inputs(&five, &rep(3000, -3000), 0)
        ));
        assert!(!quest_complete(
            &t,
            &done,
            false,
            &inputs(&five, &rep(2999, -3000), 0)
        ));
        assert!(!quest_complete(
            &t,
            &done,
            false,
            &inputs(&five, &rep(3000, -2999), 0)
        ));

        // A negative reward is money demanded at the turn-in.
        t.money = -500;
        let rep_ok = rep(3000, -3000);
        assert!(!quest_complete(
            &t,
            &done,
            false,
            &inputs(&five, &rep_ok, 499)
        ));
        assert!(quest_complete(
            &t,
            &done,
            false,
            &inputs(&five, &rep_ok, 500)
        ));

        // Players slain read counter 0.
        t.players_slain = 12;
        assert!(!quest_complete(
            &t,
            &done,
            false,
            &inputs(&five, &rep_ok, 500)
        ));
    }

    /// An exploration or escort quest reads the slot's bit; one with nothing to do is incomplete.
    #[test]
    fn an_exploration_quest_needs_the_slot_bit_and_a_bare_one_reads_incomplete() {
        use quest_slot_state::COMPLETE;
        let mut t = quest_template([empty(), empty(), empty(), empty()]);
        let none = |_: u32| 0u32;
        let neutral = |_: u32| 0i32;
        assert!(!quest_complete(
            &t,
            &slot([0; 4], COMPLETE),
            false,
            &inputs(&none, &neutral, 0)
        ));
        t.flags = quest_flags::EXPLORATION;
        assert!(!quest_complete(
            &t,
            &slot([0; 4], 0),
            false,
            &inputs(&none, &neutral, 0)
        ));
        assert!(quest_complete(
            &t,
            &slot([0; 4], COMPLETE),
            false,
            &inputs(&none, &neutral, 0)
        ));
    }

    /// `0x5e10b0`: a lone starting item is no objective; a second one, a cost or a failure is.
    #[test]
    fn the_objective_less_test_allows_a_lone_starting_item() {
        let mut t = quest_template([obj(0, 0, 2834, 1, ""), empty(), empty(), empty()]);
        t.src_item_id = 2834;
        assert!(no_objectives(&t, &slot([0; 4], 0), false));
        assert!(!no_objectives(
            &t,
            &slot([0; 4], quest_slot_state::FAIL),
            false
        ));
        assert!(!no_objectives(&t, &slot([0; 4], 0), true));
        t.required_items[0].1 = 2;
        assert!(!no_objectives(&t, &slot([0; 4], 0), false));
        t.required_items[0].1 = 1;
        t.money = -1;
        assert!(!no_objectives(&t, &slot([0; 4], 0), false));
    }

    /// The drops walk before the player line while incomplete (`0x5e1a6b`); a done quest with
    /// nothing else to close on reads its objectives text as `log`, after its other lines.
    #[test]
    fn the_leaderboard_lists_drops_while_incomplete_and_closes_on_the_log_line() {
        let mut t = quest_template([obj(100, 1, 0, 0, ""), empty(), empty(), empty()]);
        t.objectives[1].item_drop = 3000;
        t.objectives[1].item_drop_count = 1;
        t.players_slain = 2;
        let get = |key: &str| match key {
            "QUEST_INTERMEDIATE_ITEMS_NEEDED" => Some("<DROP %s %d>".to_string()),
            "QUEST_PLAYERS_KILLED" => Some("<PLAYERS %d of %d>".to_string()),
            _ => probe_strings(key),
        };
        let items = Items::default();
        let names = NameCache::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let carried = |id: u32| u32::from(id == 3000);
        let walk = |t: &QuestTemplate, complete: bool, extras: &ObjectiveExtras<'_>| {
            build_objectives(
                t,
                &slot([1, 0, 0, 0], 0),
                complete,
                &carried,
                &items,
                &names,
                &commands,
                &get,
                extras,
                &|_| {},
            )
        };
        let kinds = |lines: &[QuestLogObjectiveView]| -> Vec<String> {
            lines.iter().map(|l| l.kind.clone()).collect()
        };

        let open = walk(&t, false, &ObjectiveExtras::default());
        assert_eq!(kinds(&open), ["monster", "item", "player"]);
        assert!(open[1].intermediate && open[1].finished);
        assert_eq!(open[1].text, "<DROP ... 1>");
        assert_eq!(
            kinds(&walk(&t, true, &ObjectiveExtras::default())),
            ["monster", "player"]
        );

        t.players_slain = 0;
        let extras = ObjectiveExtras {
            objectives_text: "Report back.".into(),
            ..Default::default()
        };
        let done = walk(&t, true, &extras);
        assert_eq!(kinds(&done), ["monster", "log"]);
        assert_eq!(done[1].text, "Report back.");
        assert_eq!(kinds(&walk(&t, false, &extras)), ["monster", "item"]);
    }

    /// `0x5e3d60`: the carried quest-bound required and starting items and the carried drops,
    /// less what another logged quest requires or starts with.
    #[test]
    fn the_abandon_items_skip_unbound_items_and_those_another_quest_needs() {
        let mut a = quest_template([
            obj(0, 0, 10, 5, ""),
            obj(0, 0, 11, 1, ""),
            obj(0, 0, 12, 1, ""),
            obj(0, 0, 15, 1, ""),
        ]);
        a.quest_id = 1;
        a.src_item_id = 13;
        a.objectives[0].item_drop = 14;
        let mut b = quest_template([obj(0, 0, 11, 3, ""), empty(), empty(), empty()]);
        b.quest_id = 2;
        let carried = |id: u32| u32::from(id != 12);
        let item = |id: u32| {
            let (name, bonding) = match id {
                10 => ("Kobold Ear", BIND_QUEST),
                11 => ("Wolf Claw", BIND_QUEST),
                12 => ("Boar Fang", BIND_QUEST),
                13 => ("Sealed Letter", BIND_QUEST),
                14 => ("Torch", 0),
                15 => ("Rough Stone", 1),
                _ => return None,
            };
            Some((name.to_string(), bonding))
        };
        assert_eq!(
            abandon_items(&a, &[&a, &b], &carried, &item).as_deref(),
            Some("Kobold Ear, Sealed Letter, Torch"),
            "11 is quest 2's too, 12 is not carried, 15 is not quest-bound"
        );
        assert_eq!(abandon_items(&b, &[&a, &b], &carried, &item), None);
    }

    /// The rebuild carries each sort type's bit to its new place (`0x5e6bba`).
    #[test]
    fn the_collapse_filter_follows_each_sort_type_to_its_new_place() {
        let filter = !1; // 12 folded, 40 open
        let carried = carry_collapse_filter(filter, &[12, 40], &[0, 12, 40]);
        assert_eq!(
            carried & 0b111,
            0b101,
            "0 is new and opens; 12 stays folded"
        );
        assert_eq!(
            carried >> 3,
            filter >> 3,
            "bits past the sort types keep their value"
        );
        assert_eq!(carry_collapse_filter(!1, &[12, 40], &[40]) & 1, 1);
    }

    // ── The row table ───────────────────────────────────────────────────────────────────────────

    /// A feed over `quests` as `(slot, id, zone)`, each template cached and empty.
    fn log_app(
        quests: &[(u8, u32, i32)],
    ) -> (App, Entity, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx));
        app.init_resource::<crate::net::GuidIndex>()
            .init_resource::<NameCache>()
            .init_resource::<Items>()
            .init_resource::<crate::world_state::WorldStates>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::ui_quest::lines::TurnIns>()
            .init_non_send_resource::<crate::ui_script::live::UniqueSignals>()
            .init_resource::<crate::net::ServerWallClock>();
        let mut log = QuestLog::default();
        for &(_, id, zos) in quests {
            let mut t = quest_template([empty(), empty(), empty(), empty()]);
            t.quest_id = id;
            t.zone_or_sort = zos;
            t.title = format!("Quest {id}");
            log.insert_template(t);
        }
        app.insert_resource(log)
            .init_resource::<QuestLogMarks>()
            .init_non_send_resource::<rows::RebuildMarks>()
            .init_non_send_resource::<QuestExpiries>();
        let player = app
            .world_mut()
            .spawn((ObjectStore(store_of(quests)), Guid(0x2A), SelfPlayer))
            .id();
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.add_systems(Update, rows::rebuild);
        (app, player, rx)
    }

    /// `f` on the VM with the app's `World` lent, as every call into Lua is.
    fn live<R>(app: &mut App, f: impl FnOnce(&mut UiScript) -> R) -> R {
        crate::ui_script::live::with_live_world(app.world_mut(), f).expect("the VM")
    }

    fn store_of(quests: &[(u8, u32, i32)]) -> ObjectFields {
        use benilla_protocol::messages::field::FIELD_PLAYER_QUEST_LOG_1_1;
        let pairs: Vec<(u16, u32)> = quests
            .iter()
            .map(|&(slot, id, _)| {
                (
                    FIELD_PLAYER_QUEST_LOG_1_1
                        + benilla_protocol::messages::QUEST_LOG_SLOT_FIELDS * u16::from(slot),
                    id,
                )
            })
            .collect();
        ObjectFields::from_pairs(&pairs)
    }

    fn script(app: &mut App) -> Mut<'_, UiScript> {
        app.world_mut().non_send_resource_mut::<UiScript>()
    }

    /// The row's text lookups read the global strings raw (`0x81802d`): an addon's `__index` on `_G` that calls a verb never runs inside the row's write.
    #[test]
    fn a_global_index_metamethod_never_runs_inside_the_rows_write() {
        let (mut app, _, _rx) = log_app(&[(0, 201, 7)]);
        let mut t = quest_template([obj(100, 1, 0, 0, ""), empty(), empty(), empty()]);
        (t.quest_id, t.zone_or_sort) = (201, 7);
        app.world_mut()
            .resource_mut::<QuestLog>()
            .insert_template(t);
        app.update();
        let (text, calls) = live(&mut app, |s| {
            s.eval::<(String, i64)>(
                "local calls = 0 \
                 setmetatable(_G, { __index = function() calls = calls + GetNumArenaOpponents() + 1 end }) \
                 local text = GetQuestLogLeaderBoard(1, 2) \
                 setmetatable(_G, nil) \
                 return text, calls",
            )
            .unwrap()
        });
        assert_eq!(
            text, "",
            "no QUEST_MONSTERS_KILLED in this VM: an empty line"
        );
        assert_eq!(calls, 0, "the lookups skip the metamethod");
    }

    /// A faction with no row counts a line that reads nothing until complete (`0x5e1300`, `0x5e1460`).
    #[test]
    fn a_faction_with_no_row_counts_a_line_that_reads_nothing() {
        let (mut app, _, _rx) = log_app(&[(0, 201, 7)]);
        let mut read = |t: &QuestTemplate| {
            let mut t = t.clone();
            t.quest_id = 201;
            t.zone_or_sort = 7;
            t.rep_objective_faction = u32::MAX;
            app.world_mut()
                .resource_mut::<QuestLog>()
                .insert_template(t);
            app.update();
            live(&mut app, |s| {
                let kind = |i: i32| {
                    s.eval::<Option<String>>(&format!(
                        "return (select(2, GetQuestLogLeaderBoard({i}, 2)))"
                    ))
                    .unwrap()
                };
                let count = s.eval::<i64>("return GetNumQuestLeaderBoards(2)").unwrap();
                (count, kind(1), kind(2))
            })
        };

        let mut t = quest_template([obj(100, 1, 0, 0, ""), empty(), empty(), empty()]);
        t.rep_objective_value = 3000;
        assert_eq!(read(&t), (2, Some("monster".into()), None));

        // A standing that meets the value completes it, and the walk closes on the log line.
        let mut t = quest_template([empty(), empty(), empty(), empty()]);
        t.rep_objective_value = 0;
        assert_eq!(read(&t), (1, Some("log".into()), Some("log".into())));

        // The player line is counted beside the faction and answers both indices.
        t.rep_objective_value = 3000;
        t.players_slain = 2;
        assert_eq!(read(&t), (2, Some("player".into()), Some("player".into())));
    }

    /// A folded quest stays a row past the shown count (`0x5e08f0`), every index verb reaching it,
    /// and its fold rides `questLogCollapseFilter` to its header's new place across a rebuild.
    #[test]
    fn a_folded_quest_stays_a_row_and_its_fold_survives_a_rebuild() {
        let (mut app, player, _rx) = log_app(&[(0, 201, 7)]);
        script(&mut app).register_cvars([("questLogCollapseFilter", "-1")]);
        app.update();
        script(&mut app)
            .run("AddQuestWatch(2); CollapseQuestHeader(1)")
            .unwrap();
        assert_eq!(script(&mut app).quest_log_collapse_filter(), -2);

        // Quest 101 lands under the zone-0 header, which sorts first.
        let mut t = quest_template([empty(), empty(), empty(), empty()]);
        t.quest_id = 101;
        t.title = "Quest 101".into();
        app.world_mut()
            .resource_mut::<QuestLog>()
            .insert_template(t);
        app.world_mut()
            .entity_mut(player)
            .insert(ObjectStore(store_of(&[(0, 201, 7), (1, 101, 0)])));
        app.update();
        let s = &mut *script(&mut app);
        assert_eq!(
            s.cvar("questLogCollapseFilter").as_deref(),
            Some("-3"),
            "zone 0 opens at bit 0; zone 7's fold moves to bit 1"
        );
        assert_eq!(
            s.eval::<(i64, i64)>("return GetNumQuestLogEntries()")
                .unwrap(),
            (3, 2),
            "both headers and quest 101 shown; quest 201 folded but counted"
        );
        assert_eq!(
            live(&mut app, |s| s
                .eval::<String>("return (GetQuestLogTitle(4))")
                .unwrap()),
            "Quest 201"
        );
        let s = &mut *script(&mut app);
        assert_eq!(s.quest_log_index(201), 4);
        assert_eq!(s.eval::<i64>("return GetQuestIndexForWatch(1)").unwrap(), 4);
        assert_eq!(
            s.quest_log_watched(),
            vec![201],
            "the fold never prunes the watch"
        );
    }

    /// A time response as its two handlers take it: the clock's sample, then the rebuild's re-ask armed from it (`0x5dff40`, `0x5dff77`).
    fn answer_time(app: &mut App, unix: u32, reset_in: u32) {
        let w = app.world_mut();
        w.resource_mut::<crate::net::ServerWallClock>()
            .sample(unix, reset_in);
        let at = w.resource::<crate::net::ServerWallClock>().reask_at();
        w.non_send_resource_mut::<rows::RebuildMarks>()
            .arm_reask(at);
    }

    fn time_asks(rx: &crossbeam_channel::Receiver<ClientCommand>) -> usize {
        rx.try_iter()
            .filter(|c| matches!(c, ClientCommand::QueryTime))
            .count()
    }

    /// The rebuild's tail asks the time once the answer's re-ask is due, then disarms it (`0x5e6d60`-`0x5e6daa`); frames alone ask nothing.
    #[test]
    fn the_rebuild_asks_the_time_once_its_reask_is_due() {
        let (mut app, _, rx) = log_app(&[(0, 201, 7)]);
        answer_time(&mut app, 1_000_000, 3_000);
        app.update();
        assert_eq!(time_asks(&rx), 0, "the reset is 50 minutes out");
        answer_time(&mut app, 1_000_000, 0);
        app.update();
        assert_eq!(time_asks(&rx), 1, "due: the rebuild asks");
        app.update();
        assert_eq!(time_asks(&rx), 0, "disarmed until the next answer");
    }

    /// The rebuild's tail sends the POI query, then the due time query (`0x5e6cd1`, `0x5e6d79`), and every rebuild asks again for each quest still without a POI, in log-slot order (`0x5e6aab`).
    #[test]
    fn the_rebuild_asks_the_pois_before_the_time_and_again_each_rebuild() {
        let (mut app, _, rx) = log_app(&[(1, 202, 7), (0, 201, 9)]);
        let tail = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<String> {
            rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::QuestPoiQuery { quests } => Some(format!("poi {quests:?}")),
                    ClientCommand::QueryTime => Some("time".into()),
                    _ => None,
                })
                .collect()
        };
        answer_time(&mut app, 1_000_000, 0);
        app.update();
        assert_eq!(tail(&rx), ["poi [201, 202]", "time"]);
        app.update();
        assert_eq!(tail(&rx), ["poi [201, 202]"], "no answer yet: asked again");
        script(&mut app).apply_quest_pois(vec![benilla_ui::script::QuestPoiView {
            quest_id: 201,
            regions: Vec::new(),
        }]);
        app.update();
        assert_eq!(tail(&rx), ["poi [202]"]);
        // `GetQuestResetTime`'s rebuild inside the call keeps the order.
        answer_time(&mut app, 1_000_000, 0);
        live(&mut app, |s| {
            s.eval::<i64>("return GetQuestResetTime()").unwrap()
        });
        assert_eq!(tail(&rx), ["poi [202]", "time"]);
    }

    /// `GetQuestResetTime` runs a due rebuild inside its call (`0x5e6dfa`, `0x5e6e00`): the rows a later verb in the same chunk reads are the rebuilt ones, `QUEST_LOG_UPDATE` waits, the time is asked once.
    #[test]
    fn get_quest_reset_time_runs_a_due_rebuild_inside_the_call() {
        let (mut app, player, rx) = log_app(&[(0, 201, 7)]);
        app.update();
        let _ = time_asks(&rx);
        // A second quest lands in a slot with no rebuild yet, and the reset is due.
        app.world_mut()
            .entity_mut(player)
            .insert(ObjectStore(store_of(&[(0, 201, 7), (1, 202, 7)])));
        app.world_mut().resource_mut::<QuestLog>().insert_template({
            let mut t = quest_template([empty(), empty(), empty(), empty()]);
            (t.quest_id, t.zone_or_sort) = (202, 7);
            t
        });
        answer_time(&mut app, 1_000_000, 0);
        app.world_mut()
            .non_send_resource_mut::<crate::ui_script::live::UniqueSignals>()
            .clear();
        let counts = live(&mut app, |s| {
            s.eval::<(i64, i64, i64)>(
                "local before = select(2, GetNumQuestLogEntries()) \
                 local left = GetQuestResetTime() \
                 return before, select(2, GetNumQuestLogEntries()), left",
            )
            .unwrap()
        });
        assert_eq!(counts, (1, 2, 0), "the call rebuilt the rows");
        assert_eq!(time_asks(&rx), 1);
        assert_eq!(
            app.world()
                .non_send_resource::<crate::ui_script::live::UniqueSignals>()
                .names(),
            ["QUEST_LOG_UPDATE"]
        );
        live(&mut app, |s| {
            s.eval::<i64>("return GetQuestResetTime()").unwrap()
        });
        assert_eq!(time_asks(&rx), 0, "the call's rebuild disarmed the re-ask");
    }

    /// A rebuild skipped on a pending template leaves the re-ask armed (`0x5e6973`), so the next call retries it.
    #[test]
    fn a_skipped_rebuild_keeps_the_reask_armed() {
        let (mut app, player, rx) = log_app(&[(0, 201, 7)]);
        app.update();
        app.world_mut()
            .entity_mut(player)
            .insert(ObjectStore(store_of(&[(0, 201, 7), (1, 303, 7)])));
        app.update();
        let _ = time_asks(&rx);
        answer_time(&mut app, 1_000_000, 0);
        live(&mut app, |s| {
            s.eval::<i64>("return GetQuestResetTime()").unwrap()
        });
        assert_eq!(time_asks(&rx), 0, "303's template is pending");
        assert!(app
            .world()
            .non_send_resource::<rows::RebuildMarks>()
            .reask_due(std::time::Instant::now()));
    }

    /// The stack watch walks the rebuilt quest rows (`0x5e1270`): an item a row's quest requires queues `QUEST_LOG_UPDATE` (`0x5e12bc`) and re-asks every giver's status (`0x5e12e5`), with no player only the signal; another item, or a quest in a slot the rebuild skipped, neither.
    #[test]
    fn a_required_items_stack_move_signals_the_log_and_reasks_the_givers() {
        let (mut app, player, rx) = log_app(&[(0, 201, 7)]);
        let needs = |id: u32, item: u32| {
            let mut t = quest_template([obj(0, 0, item, 5, ""), empty(), empty(), empty()]);
            (t.quest_id, t.zone_or_sort) = (id, 7);
            t
        };
        app.world_mut()
            .resource_mut::<QuestLog>()
            .insert_template(needs(201, 2000));
        app.update();
        let reasks = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
            rx.try_iter()
                .filter(|c| matches!(c, ClientCommand::QuestgiverStatusMultipleQuery))
                .count()
        };
        let _ = reasks(&rx);
        let moved = |app: &mut App, item: u32, me: bool| {
            let w = app.world();
            let mut signals = crate::ui_script::live::UniqueSignals::default();
            rows::item_moved(
                item,
                me,
                w.resource::<QuestLog>(),
                w.non_send_resource::<rows::RebuildMarks>(),
                w.resource::<NetCommands>(),
                &mut signals,
            );
            signals.names()
        };
        assert_eq!(moved(&mut app, 2000, true), ["QUEST_LOG_UPDATE"]);
        assert_eq!(reasks(&rx), 1);
        assert_eq!(moved(&mut app, 2000, false), ["QUEST_LOG_UPDATE"]);
        assert_eq!(reasks(&rx), 0, "no player: the signal alone");
        assert!(moved(&mut app, 2001, true).is_empty());
        assert_eq!(reasks(&rx), 0);
        // 303's template is pending, so the rebuild that would list 202 skips (`0x5e6973`) and its row is not walked.
        app.world_mut()
            .entity_mut(player)
            .insert(ObjectStore(store_of(&[(0, 201, 7), (2, 303, 7)])));
        app.update();
        app.world_mut()
            .resource_mut::<QuestLog>()
            .insert_template(needs(202, 3000));
        app.world_mut()
            .entity_mut(player)
            .insert(ObjectStore(store_of(&[
                (0, 201, 7),
                (1, 202, 7),
                (2, 303, 7),
            ])));
        app.update();
        assert!(moved(&mut app, 3000, true).is_empty());
        assert_eq!(moved(&mut app, 2000, true), ["QUEST_LOG_UPDATE"]);
        assert_eq!(reasks(&rx), 1);
    }

    /// The stack watch reads a header row's sort type as a quest id from the cache (`0x5e1285`): an uncached one sends no query (`0x67debd`), a cached quest of that id requiring the item signals.
    #[test]
    fn the_stack_watch_reads_a_header_sort_type_as_a_quest_id() {
        let (mut app, _, rx) = log_app(&[(0, 201, 7)]);
        app.update();
        let _ = rx.try_iter().count();
        let moved = |app: &mut App, item: u32| {
            let w = app.world();
            let mut signals = crate::ui_script::live::UniqueSignals::default();
            rows::item_moved(
                item,
                true,
                w.resource::<QuestLog>(),
                w.non_send_resource::<rows::RebuildMarks>(),
                w.resource::<NetCommands>(),
                &mut signals,
            );
            signals.names()
        };
        assert!(moved(&mut app, 4000).is_empty());
        assert!(
            !rx.try_iter()
                .any(|c| matches!(c, ClientCommand::QuestQuery { .. })),
            "quest 7 uncached: the lookup asks nothing"
        );
        let mut seven = quest_template([obj(0, 0, 4000, 1, ""), empty(), empty(), empty()]);
        (seven.quest_id, seven.zone_or_sort) = (7, 12);
        app.world_mut()
            .resource_mut::<QuestLog>()
            .insert_template(seven);
        assert_eq!(
            moved(&mut app, 4000),
            ["QUEST_LOG_UPDATE"],
            "zone 7's header row reads cached quest 7"
        );
    }

    /// A timer run out marks its quest expired at the rebuild (`0x5e6b15` -> `0x5e57e0`): it reads
    /// failed and the bare status query goes out.
    #[test]
    fn a_run_out_timer_fails_its_quest_at_the_rebuild_and_asks_the_status_again() {
        use benilla_protocol::messages::field::FIELD_PLAYER_QUEST_LOG_1_1;
        let (mut app, player, rx) = log_app(&[(0, 201, 7)]);
        answer_time(&mut app, 1_000_950, 0);
        app.world_mut()
            .entity_mut(player)
            .insert(ObjectStore(ObjectFields::from_pairs(&[
                (FIELD_PLAYER_QUEST_LOG_1_1, 201),
                (FIELD_PLAYER_QUEST_LOG_1_1 + 4, 1_000_900),
            ])));
        app.update();
        assert!(app
            .world()
            .non_send_resource::<QuestExpiries>()
            .is_expired(201));
        assert!(live(&mut app, |s| s
            .eval::<bool>("local _, _, _, _, _, _, done = GetQuestLogTitle(2) return done == -1")
            .unwrap()));
        assert_eq!(
            rx.try_iter()
                .filter(|c| matches!(c, ClientCommand::QuestgiverStatusMultipleQuery))
                .count(),
            1
        );
    }
}
