//! The quest progress lines, raised from the packets as 12340 raises them: the `SMSG_QUESTUPDATE_*`
//! group (`0x6d8030`), the forced removal (`0x6d8370`) and the item push's quest-item check
//! (`0x6d7de0`). Each line names the quest through the template cache and fires
//! `QUEST_WATCH_UPDATE` with the quest's log row. The completed-quest answer (`0x5b5190`) lands
//! here too, as it fires its event from the packet, and so do `GetQuestTimers`' expiries.

use benilla_protocol::messages::{
    quest_flags, quest_slot_state, ItemPushResult, QuestLogSlot, QuestTemplate,
};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{ScriptValue, UiScript};
use benilla_ui::strings::{fill, Arg};
use bevy::prelude::*;

use super::{quest_complete, CompletionInputs, QuestLog, GO_OBJECTIVE_BIT};
use crate::go_templates::GameObjectTemplates;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, Guid, NetCommands, NetHandlerApp, ObjectStore, SelfPlayer};
use crate::ui_action::{show_messages, MessageSink, Shown};

/// One progress packet, queued in arrival order for [`announce_quest_progress`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum QuestNotice {
    /// `SMSG_QUESTUPDATE_ADD_KILL`: `entry` a creature, or `(-id)|0x80000000` for a GameObject.
    Kill {
        quest_id: u32,
        entry: u32,
        count: u32,
        required: u32,
    },
    /// `SMSG_QUESTUPDATE_ADD_PVP_KILL`.
    PvpKill { quest_id: u32, count: u32 },
    /// `SMSG_QUESTUPDATE_COMPLETE`.
    Complete { quest_id: u32 },
    /// `SMSG_QUESTUPDATE_FAILED`, or `_FAILEDTIMER` when `timed`.
    Failed { quest_id: u32, timed: bool },
    /// `SMSG_QUEST_FORCE_REMOVE`.
    ForceRemoved { quest_id: u32 },
    /// `SMSG_ITEM_PUSH_RESULT` to the player: the entry, the count added and the bags' total.
    ItemPush { item: u32, count: u32, total: u32 },
    /// An item push held for one logged quest's template, which 12340's callback finishes
    /// (`0x6d7d70`).
    ItemPushFor {
        quest_id: u32,
        item: u32,
        count: u32,
        total: u32,
    },
}

/// The packet handlers that queue the notices.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::QuestObjectiveKill, on_kill)
        .net_handler(K::QuestObjectivePvpKill, on_pvp_kill)
        .net_handler(K::QuestObjectivesComplete, on_complete)
        .net_handler(K::QuestFailed, on_failed)
        .net_handler(K::QuestForceRemoved, on_force_removed)
        .net_handler(K::QuestsCompleted, on_quests_completed)
        .net_handler(K::ItemPushResult, on_item_push);
}

fn on_kill(In(ev): In<SessionEvent>, mut log: ResMut<QuestLog>) {
    if let SessionEvent::QuestObjectiveKill {
        quest_id,
        entry,
        count,
        required,
    } = ev
    {
        log.notices.push(QuestNotice::Kill {
            quest_id,
            entry,
            count,
            required,
        });
    }
}

fn on_pvp_kill(In(ev): In<SessionEvent>, mut log: ResMut<QuestLog>) {
    if let SessionEvent::QuestObjectivePvpKill { quest_id, count } = ev {
        log.notices.push(QuestNotice::PvpKill { quest_id, count });
    }
}

fn on_complete(In(ev): In<SessionEvent>, mut log: ResMut<QuestLog>) {
    if let SessionEvent::QuestObjectivesComplete { quest_id } = ev {
        log.notices.push(QuestNotice::Complete { quest_id });
    }
}

fn on_failed(In(ev): In<SessionEvent>, mut log: ResMut<QuestLog>) {
    if let SessionEvent::QuestFailed { quest_id, timed } = ev {
        log.notices.push(QuestNotice::Failed { quest_id, timed });
    }
}

fn on_force_removed(In(ev): In<SessionEvent>, mut log: ResMut<QuestLog>) {
    if let SessionEvent::QuestForceRemoved { quest_id } = ev {
        log.notices.push(QuestNotice::ForceRemoved { quest_id });
    }
}

fn on_quests_completed(In(ev): In<SessionEvent>, mut log: ResMut<QuestLog>) {
    if let SessionEvent::QuestsCompleted(ids) = ev {
        log.completed_answer = Some(ids);
    }
}

/// Only a push to the player itself reaches the quest-item check (`0x6d7de8`).
fn on_item_push(
    In(ev): In<SessionEvent>,
    mut log: ResMut<QuestLog>,
    me: Query<&Guid, With<SelfPlayer>>,
) {
    if let SessionEvent::ItemPushResult(p) = ev {
        if me.iter().next().is_some_and(|g| g.0 == p.player_guid) {
            log.notices.push(item_push(&p));
        }
    }
}

fn item_push(p: &ItemPushResult) -> QuestNotice {
    QuestNotice::ItemPush {
        item: p.item_entry,
        count: p.count,
        total: p.total_count,
    }
}

/// What a notice needs from the world besides the template cache.
pub(super) struct Lookups<'a> {
    pub(super) store: Option<&'a benilla_protocol::ObjectFields>,
    pub(super) names: &'a NameCache,
    pub(super) gos: Option<&'a GameObjectTemplates>,
    pub(super) items: &'a Items,
    pub(super) commands: &'a NetCommands,
    /// The bags and standings an auto-complete's completion test reads.
    pub(super) carried: &'a dyn Fn(u32) -> u32,
    pub(super) standing: &'a dyn Fn(u32) -> i32,
    /// The player's guid, which an auto-complete names as its questgiver (`0x6d7d41`).
    pub(super) player_guid: u64,
}

impl Lookups<'_> {
    /// The player's slot holding `quest_id`.
    fn slot_of(&self, quest_id: u32) -> Option<QuestLogSlot> {
        self.store.and_then(|s| {
            (0..benilla_protocol::messages::PLAYER_QUEST_LOG_SLOTS)
                .filter_map(|slot| s.player_quest_log(slot))
                .find(|l| l.quest_id == quest_id)
        })
    }

    /// The player's slot state for `quest_id`, 0 when it holds no slot.
    fn slot_state(&self, quest_id: u32) -> u8 {
        self.slot_of(quest_id).map_or(0, |l| l.state)
    }
}

/// One line the announce shows, then the watch row it signals, if any.
#[derive(Debug, PartialEq)]
pub(super) struct Announce {
    pub(super) key: &'static str,
    pub(super) args: Vec<AnnounceArg>,
    /// `QUEST_WATCH_UPDATE`'s quest, whose row the event carries.
    pub(super) watch: Option<u32>,
}

#[derive(Debug, PartialEq)]
pub(super) enum AnnounceArg {
    S(String),
    D(u32),
}

fn line(key: &'static str, args: Vec<AnnounceArg>, watch: Option<u32>) -> Announce {
    Announce { key, args, watch }
}

/// Resolve the queued notices into lines, in order. A notice whose quest-item check waits on a
/// template comes back held, for the next pass.
pub(super) fn resolve(
    log: &mut QuestLog,
    notices: Vec<QuestNotice>,
    w: &Lookups<'_>,
) -> (Vec<Announce>, Vec<QuestNotice>) {
    let mut out = Vec::new();
    let mut held = Vec::new();
    for notice in notices {
        match notice {
            QuestNotice::Kill {
                quest_id,
                entry,
                count,
                required,
            } => {
                if expired(log, quest_id, w) {
                    continue;
                }
                let template = log.template(quest_id, w.commands);
                // An objective with its own text names the line.
                if let Some(o) = template.and_then(|t| {
                    t.objectives
                        .iter()
                        .find(|o| o.creature_or_go == entry && !o.text.is_empty())
                }) {
                    out.push(line(
                        "ERR_QUEST_ADD_FOUND_SII",
                        vec![
                            AnnounceArg::S(o.text.clone()),
                            AnnounceArg::D(count),
                            AnnounceArg::D(required),
                        ],
                        Some(quest_id),
                    ));
                    continue;
                }
                // Else the cached creature or GameObject name; an uncached one shows nothing.
                let (key, name) = if entry & GO_OBJECTIVE_BIT == 0 {
                    (
                        "ERR_QUEST_ADD_KILL_SII",
                        w.names
                            .resolve_creature(entry, 0, w.commands)
                            .map(str::to_string),
                    )
                } else {
                    (
                        "ERR_QUEST_ADD_FOUND_SII",
                        w.gos
                            .and_then(|g| g.name_by_entry(entry & !GO_OBJECTIVE_BIT, w.commands))
                            .map(str::to_string),
                    )
                };
                if let Some(name) = name {
                    out.push(line(
                        key,
                        vec![
                            AnnounceArg::S(name),
                            AnnounceArg::D(count),
                            AnnounceArg::D(required),
                        ],
                        Some(quest_id),
                    ));
                }
            }
            QuestNotice::PvpKill { quest_id, count } => {
                if expired(log, quest_id, w) {
                    continue;
                }
                if let Some(t) = log.template(quest_id, w.commands) {
                    out.push(line(
                        "ERR_QUEST_ADD_PLAYER_KILL_SII",
                        vec![AnnounceArg::D(count), AnnounceArg::D(t.players_slain)],
                        Some(quest_id),
                    ));
                }
            }
            QuestNotice::Complete { quest_id } => {
                if expired(log, quest_id, w) {
                    continue;
                }
                match log.template(quest_id, w.commands) {
                    // No end text: the bare line, and no watch signal.
                    Some(t) if t.end_text.is_empty() => {
                        out.push(line("ERR_QUEST_UNKNOWN_COMPLETE", vec![], None));
                    }
                    Some(t) => out.push(line(
                        "ERR_QUEST_OBJECTIVE_COMPLETE_S",
                        vec![AnnounceArg::S(t.end_text.clone())],
                        Some(quest_id),
                    )),
                    None => {}
                }
            }
            QuestNotice::Failed { quest_id, timed } => {
                // The expiry is read before the timer arm marks the quest.
                let was_expired = expired(log, quest_id, w);
                if timed {
                    log.expired.insert(quest_id);
                }
                if was_expired {
                    continue;
                }
                if let Some(t) = log.template(quest_id, w.commands) {
                    out.push(line(
                        "ERR_QUEST_FAILED_S",
                        vec![AnnounceArg::S(t.title.clone())],
                        None,
                    ));
                }
            }
            QuestNotice::ForceRemoved { quest_id } => {
                if let Some(t) = log.template(quest_id, w.commands) {
                    out.push(line(
                        "ERR_QUEST_FORCE_REMOVED_S",
                        vec![AnnounceArg::S(t.title.clone())],
                        None,
                    ));
                }
            }
            QuestNotice::ItemPush { item, count, total } => {
                // Each logged, unexpired quest in slot order; the first that asks for the item
                // ends the walk. An uncached template is asked and finished by its own pass.
                let Some(store) = w.store else { continue };
                for slot in 0..benilla_protocol::messages::PLAYER_QUEST_LOG_SLOTS {
                    let Some(log_slot) = store.player_quest_log(slot) else {
                        continue;
                    };
                    let quest_id = log_slot.quest_id;
                    if quest_id as i32 <= 0 || log.expired.contains(&quest_id) {
                        continue;
                    }
                    match log.template(quest_id, w.commands) {
                        Some(t) => {
                            let push = Push { item, count, total };
                            if quest_item_loot(t, push, Some((log_slot, false)), w, &mut out) {
                                break;
                            }
                        }
                        None if log.template_answered_unknown(quest_id) => {}
                        None => held.push(QuestNotice::ItemPushFor {
                            quest_id,
                            item,
                            count,
                            total,
                        }),
                    }
                }
            }
            QuestNotice::ItemPushFor {
                quest_id,
                item,
                count,
                total,
            } => match log.template(quest_id, w.commands) {
                Some(t) => {
                    let entry = w
                        .slot_of(quest_id)
                        .map(|s| (s, log.expired.contains(&quest_id)));
                    quest_item_loot(t, Push { item, count, total }, entry, w, &mut out);
                }
                None if log.template_answered_unknown(quest_id) => {}
                None => held.push(QuestNotice::ItemPushFor {
                    quest_id,
                    item,
                    count,
                    total,
                }),
            },
        }
    }
    (out, held)
}

/// 12340's expiry test (`0x6d8030`): the client's timer mark, else the slot's fail bit.
fn expired(log: &QuestLog, quest_id: u32, w: &Lookups<'_>) -> bool {
    log.expired.contains(&quest_id) || w.slot_state(quest_id) & quest_slot_state::FAIL != 0
}

/// One item push: the entry, the count added and the bags' total.
#[derive(Clone, Copy)]
struct Push {
    item: u32,
    count: u32,
    total: u32,
}

/// `0x6d7c30`: whether `template` collects `item`, which ends the slot walk, and its line when the
/// push carried the count past what the bags held; then an auto-complete quest (`0x10000`) whose
/// log row, `entry` as its slot and expiry mark, passes the completion test is completed toward
/// the player itself (`0x6d7d1b` -> `0x6d4d40`). An uncached item shows nothing.
fn quest_item_loot(
    template: &QuestTemplate,
    Push { item, count, total }: Push,
    entry: Option<(QuestLogSlot, bool)>,
    w: &Lookups<'_>,
    out: &mut Vec<Announce>,
) -> bool {
    let Some(&(_, required)) = template
        .required_items
        .iter()
        .find(|&&(id, _)| id != 0 && id == item)
    else {
        return false;
    };
    let Some(name) = w
        .items
        .template(item, 0, w.commands)
        .map(|t| t.name.clone())
    else {
        return true;
    };
    if total as i32 - count as i32 >= required as i32 {
        return true;
    }
    out.push(line(
        "ERR_QUEST_ADD_ITEM_SII",
        vec![
            AnnounceArg::S(name),
            AnnounceArg::D(total.min(required)),
            AnnounceArg::D(required),
        ],
        Some(template.quest_id),
    ));
    if template.flags & quest_flags::AUTO_COMPLETE != 0 {
        if let (Some((slot, expired)), Some(store)) = (entry, w.store) {
            let inputs = CompletionInputs {
                carried: w.carried,
                standing: w.standing,
                money: u64::from(store.player_money().unwrap_or(0)),
            };
            if quest_complete(template, &slot, expired, &inputs) {
                debug!("ui_quest_log: quest {} auto-completes", template.quest_id);
                let _ = w.commands.0.send(ClientCommand::QuestgiverComplete {
                    npc: w.player_guid,
                    quest: template.quest_id,
                });
            }
        }
    }
    true
}

/// `GetQuestTimers`' expiries (`0x5e6349`): each new one keeps its mark, asks every questgiver's
/// status again (`0x5e582a`) and shows `ERR_QUEST_FAILED_S` with the quest's title.
fn expiry_lines(log: &mut QuestLog, expiries: Vec<u32>, commands: &NetCommands) -> Vec<Announce> {
    let mut out = Vec::new();
    for quest_id in expiries {
        if !log.expired.insert(quest_id) {
            continue;
        }
        let _ = commands
            .0
            .send(ClientCommand::QuestgiverStatusMultipleQuery);
        if let Some(t) = log.template(quest_id, commands) {
            out.push(line(
                "ERR_QUEST_FAILED_S",
                vec![AnnounceArg::S(t.title.clone())],
                None,
            ));
        }
    }
    out
}

/// Drains the notices, the completed-quest answer and the timer expiries, and sends
/// `QueryQuestsCompleted()`'s requests. Before the log feed: `QUEST_WATCH_UPDATE` fires from the
/// packet, ahead of the frame's `QUEST_LOG_UPDATE`, which the reference flushes after the packets.
#[allow(clippy::too_many_arguments)] // one Bevy system's full input set
pub(super) fn announce_quest_progress(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    mut log: ResMut<QuestLog>,
    names: Res<NameCache>,
    gos: Option<Res<GameObjectTemplates>>,
    items: Res<Items>,
    commands: Res<NetCommands>,
    mut sink: MessageSink,
    // The auto-complete's completion test: the bag walk and the standings.
    (objects, rewards): (crate::net::Objects, crate::ui_quest::rewards::RewardInputs),
) {
    let Some(mut script) = script else {
        return;
    };
    for _ in 0..script.take_quests_completed_queries() {
        let _ = commands.0.send(ClientCommand::QueryQuestsCompleted);
    }
    let expiries = script.take_quest_log_expiries();
    // Read first, so an idle frame leaves the log unmarked for the feed's change gate.
    if log.notices.is_empty() && log.completed_answer.is_none() && expiries.is_empty() {
        return;
    }
    let mut lines = expiry_lines(&mut log, expiries, &commands);
    if let Some(ids) = log.completed_answer.take() {
        script.set_quests_completed(ids);
        script.fire_event("QUEST_QUERY_COMPLETE", vec![]);
    }
    let me = self_q.iter().next();
    let store = me.map(|(s, _)| &s.0);
    let carried = |id: u32| {
        store.map_or(0, |s| {
            crate::ui_items::count_of(
                s,
                &objects,
                id,
                crate::ui_items::InventoryScope::QUEST_ITEMS,
            )
        })
    };
    let standing = |faction: u32| store.map_or(0, |s| rewards.standing(s, faction));
    let lookups = Lookups {
        store,
        names: &names,
        gos: gos.as_deref(),
        items: &items,
        commands: &commands,
        carried: &carried,
        standing: &standing,
        player_guid: me.map_or(0, |(_, g)| g.0),
    };
    let notices = std::mem::take(&mut log.notices);
    let (resolved, held) = resolve(&mut log, notices, &lookups);
    log.notices = held;
    lines.extend(resolved);
    for a in lines {
        let args: Vec<Arg<'_>> = a
            .args
            .iter()
            .map(|x| match x {
                AnnounceArg::S(s) => Arg::S(s),
                AnnounceArg::D(d) => Arg::D(i64::from(*d)),
            })
            .collect();
        let text = script
            .lua()
            .globals()
            .get::<String>(a.key)
            .ok()
            .map(|t| fill(&t, &args))
            .filter(|t| !t.is_empty());
        if let Some(text) = text {
            show_messages(
                &mut script,
                &mut sink,
                "ui_quest_log",
                [Shown::keyed(a.key, text)],
            );
        }
        if let Some(quest_id) = a.watch {
            let index = script.quest_log_index(quest_id);
            script.fire_event("QUEST_WATCH_UPDATE", vec![ScriptValue::Int(index as i64)]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::field::FIELD_PLAYER_QUEST_LOG_1_1;
    use benilla_protocol::messages::QuestObjective;

    const QUEST: u32 = 33;

    fn template() -> QuestTemplate {
        let mut t = QuestTemplate {
            quest_id: QUEST,
            title: "Wolves Across the Border".into(),
            end_text: "Return to Eagan Peltskinner.".into(),
            players_slain: 5,
            ..Default::default()
        };
        t.objectives[0] = QuestObjective {
            creature_or_go: 299,
            required_count: 8,
            ..Default::default()
        };
        t.objectives[1] = QuestObjective {
            creature_or_go: 0x8000_0000 | 1561,
            required_count: 1,
            text: "Ember found".into(),
            ..Default::default()
        };
        t.required_items[0] = (750, 8);
        t
    }

    /// Resolve `notices` with the quest in slot 0 at `state`, the template cached.
    fn run(notices: Vec<QuestNotice>, state: u32) -> (Vec<Announce>, QuestLog) {
        let mut log = QuestLog::default();
        log.insert_template(template());
        let store = benilla_protocol::ObjectFields::from_pairs(&[
            (FIELD_PLAYER_QUEST_LOG_1_1, QUEST),
            (FIELD_PLAYER_QUEST_LOG_1_1 + 1, state),
        ]);
        let mut names = NameCache::default();
        names.insert_creature(
            299,
            Some(crate::names::CreatureRecord {
                name: "Diseased Young Wolf".into(),
                ..Default::default()
            }),
        );
        let mut items = Items::default();
        items.insert_template(750, Some(crate::items::test_template("Tough Wolf Meat")));
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let w = Lookups {
            store: Some(&store),
            names: &names,
            gos: None,
            items: &items,
            commands: &commands,
            carried: &|_| 0,
            standing: &|_| 0,
            player_guid: 0x2A,
        };
        let (out, held) = resolve(&mut log, notices, &w);
        assert!(held.is_empty());
        (out, log)
    }

    fn s(v: &str) -> AnnounceArg {
        AnnounceArg::S(v.into())
    }

    fn d(v: u32) -> AnnounceArg {
        AnnounceArg::D(v)
    }

    /// `0x6d8030`'s kill arm: an objective with its own text names the line, else the creature.
    #[test]
    fn a_kill_names_its_objective_text_else_the_creature() {
        let (out, _) = run(
            vec![
                QuestNotice::Kill {
                    quest_id: QUEST,
                    entry: 299,
                    count: 3,
                    required: 8,
                },
                QuestNotice::Kill {
                    quest_id: QUEST,
                    entry: 0x8000_0000 | 1561,
                    count: 1,
                    required: 1,
                },
            ],
            0,
        );
        assert_eq!(
            out,
            [
                line(
                    "ERR_QUEST_ADD_KILL_SII",
                    vec![s("Diseased Young Wolf"), d(3), d(8)],
                    Some(QUEST)
                ),
                line(
                    "ERR_QUEST_ADD_FOUND_SII",
                    vec![s("Ember found"), d(1), d(1)],
                    Some(QUEST)
                ),
            ]
        );
    }

    /// An uncached GameObject name shows nothing, as the reference's cache miss does.
    #[test]
    fn an_unnamed_gameobject_kill_shows_nothing() {
        let (out, _) = run(
            vec![QuestNotice::Kill {
                quest_id: QUEST,
                entry: 0x8000_0000 | 9999,
                count: 1,
                required: 2,
            }],
            0,
        );
        assert!(out.is_empty());
    }

    /// The PvP arm reads the required count from the template, the complete arm the end text.
    #[test]
    fn pvp_kills_and_completion_read_the_template() {
        let (out, _) = run(
            vec![
                QuestNotice::PvpKill {
                    quest_id: QUEST,
                    count: 2,
                },
                QuestNotice::Complete { quest_id: QUEST },
            ],
            0,
        );
        assert_eq!(
            out,
            [
                line(
                    "ERR_QUEST_ADD_PLAYER_KILL_SII",
                    vec![d(2), d(5)],
                    Some(QUEST)
                ),
                line(
                    "ERR_QUEST_OBJECTIVE_COMPLETE_S",
                    vec![s("Return to Eagan Peltskinner.")],
                    Some(QUEST)
                ),
            ]
        );
    }

    /// A failed slot silences the progress arms; the first timer failure still speaks and marks
    /// the quest, and a second is silent.
    #[test]
    fn a_failed_quest_is_silent_and_a_timer_failure_speaks_once() {
        let (out, _) = run(
            vec![QuestNotice::PvpKill {
                quest_id: QUEST,
                count: 2,
            }],
            u32::from(quest_slot_state::FAIL),
        );
        assert!(out.is_empty(), "slot state & 2 is expired");

        let failed = || QuestNotice::Failed {
            quest_id: QUEST,
            timed: true,
        };
        let (out, log) = run(vec![failed(), failed()], 0);
        assert_eq!(
            out,
            [line(
                "ERR_QUEST_FAILED_S",
                vec![s("Wolves Across the Border")],
                None
            )]
        );
        assert!(log.expired.contains(&QUEST));
    }

    /// `0x6d7c30`: a push that lifts the bags past what they held shows the capped count; one
    /// that was already enough shows nothing.
    #[test]
    fn an_item_push_counts_against_the_bags_total() {
        let push = |count, total| QuestNotice::ItemPush {
            item: 750,
            count,
            total,
        };
        let (out, _) = run(vec![push(2, 5), push(3, 10), push(1, 9)], 0);
        assert_eq!(
            out,
            [
                line(
                    "ERR_QUEST_ADD_ITEM_SII",
                    vec![s("Tough Wolf Meat"), d(5), d(8)],
                    Some(QUEST)
                ),
                line(
                    "ERR_QUEST_ADD_ITEM_SII",
                    vec![s("Tough Wolf Meat"), d(8), d(8)],
                    Some(QUEST)
                ),
            ],
            "9 - 1 >= 8: the bags already held enough"
        );
    }

    /// `0x6d7c30`: an auto-complete quest the push completes is completed toward the player's own
    /// guid; one still short, or without the flag, sends nothing.
    #[test]
    fn an_item_push_completes_an_auto_complete_quest_toward_the_player() {
        let complete_sent = |flags: u32, in_bags: u32| {
            let mut log = QuestLog::default();
            let mut t = QuestTemplate {
                quest_id: QUEST,
                flags,
                ..Default::default()
            };
            t.required_items[0] = (750, 8);
            log.insert_template(t);
            let store =
                benilla_protocol::ObjectFields::from_pairs(&[(FIELD_PLAYER_QUEST_LOG_1_1, QUEST)]);
            let names = NameCache::default();
            let mut items = Items::default();
            items.insert_template(750, Some(crate::items::test_template("Tough Wolf Meat")));
            let (tx, rx) = crossbeam_channel::unbounded();
            let commands = NetCommands(tx);
            let carried = move |id: u32| if id == 750 { in_bags } else { 0 };
            let w = Lookups {
                store: Some(&store),
                names: &names,
                gos: None,
                items: &items,
                commands: &commands,
                carried: &carried,
                standing: &|_| 0,
                player_guid: 0x2A,
            };
            let push = QuestNotice::ItemPush {
                item: 750,
                count: 1,
                total: in_bags,
            };
            let (out, _) = resolve(&mut log, vec![push], &w);
            assert_eq!(out.len(), 1, "the line shows either way");
            rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::QuestgiverComplete { npc, quest } => Some((npc, quest)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            complete_sent(quest_flags::AUTO_COMPLETE, 8),
            [(0x2A, QUEST)]
        );
        assert!(complete_sent(quest_flags::AUTO_COMPLETE, 7).is_empty());
        assert!(complete_sent(0, 8).is_empty());
    }

    /// `GetQuestTimers`' expiry keeps the mark, re-asks the status and shows the failure, once.
    #[test]
    fn a_timer_expiry_is_marked_announced_and_reasked_once() {
        let mut log = QuestLog::default();
        log.insert_template(template());
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let out = expiry_lines(&mut log, vec![QUEST, QUEST], &commands);
        assert_eq!(
            out,
            [line(
                "ERR_QUEST_FAILED_S",
                vec![s("Wolves Across the Border")],
                None
            )]
        );
        assert!(log.is_expired(QUEST));
        assert_eq!(
            rx.try_iter()
                .filter(|c| matches!(c, ClientCommand::QuestgiverStatusMultipleQuery))
                .count(),
            1
        );
    }

    /// A logged quest whose template is not cached holds the push for its own pass.
    #[test]
    fn an_item_push_waits_on_an_uncached_template() {
        let mut log = QuestLog::default();
        let store = benilla_protocol::ObjectFields::from_pairs(&[(FIELD_PLAYER_QUEST_LOG_1_1, 77)]);
        let names = NameCache::default();
        let items = Items::default();
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let w = Lookups {
            store: Some(&store),
            names: &names,
            gos: None,
            items: &items,
            commands: &commands,
            carried: &|_| 0,
            standing: &|_| 0,
            player_guid: 0x2A,
        };
        let (out, held) = resolve(
            &mut log,
            vec![QuestNotice::ItemPush {
                item: 750,
                count: 1,
                total: 1,
            }],
            &w,
        );
        assert!(out.is_empty());
        assert_eq!(
            held,
            [QuestNotice::ItemPushFor {
                quest_id: 77,
                item: 750,
                count: 1,
                total: 1
            }]
        );
        assert!(
            rx.try_iter()
                .any(|c| matches!(c, ClientCommand::QuestQuery { quest: 77 })),
            "the miss asks for the template"
        );
    }
}
