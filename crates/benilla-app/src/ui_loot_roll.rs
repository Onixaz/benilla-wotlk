//! The group loot rolls: the client's roll list (`0xada7b8`) and the lines its packets announce.
//! `SMSG_LOOT_START_ROLL`, `SMSG_LOOT_ROLL`, `SMSG_LOOT_ROLL_WON` and `SMSG_LOOT_ALL_PASSED` move
//! the list and raise the stock `GroupLootFrame`'s events at their step, from the handler or from
//! the item record's or the name's callback; the roll verbs read the list at the call through
//! [`LootRollsLive`], and `RollOnLoot` votes go back out as `CMSG_LOOT_ROLL`.
//!
//! A roll for another map than ours is dropped at the start (`0x6fb2c0`). A fresh roll reads its
//! item record with the start's callback: cached, `START_LOOT_ROLL` fires at once, else from the
//! callback, and a record the server cannot describe drops the roll (`0x6fb247`). At most
//! [`MAX_SHOWN_ROLLS`] frames are up; a ready roll past that waits and shows when one closes
//! (`0x6fb180`).
//!
//! `rollID` is a client-local serial from 0 (`[0xca0690]`); the wire addresses a roll by
//! `(lootedTarget, itemSlot)`. Our own vote closes our frame at once and finishes the roll, which
//! stays listed until it resolves (`0x6fbc70`). A won or all-passed roll closes once its line is
//! composed, after its item record and the winner's name (`0x6fb700`, `0x6fb470`), and makes its
//! loot window slot lootable again at once (`0x6dc770`).

use benilla_protocol::messages::{LootAllPassed, LootRoll, LootRollWon, RollAnnounce};
use bevy::prelude::*;

use benilla_ui::script::{LootRollEntry, ScriptValue};
use benilla_ui::strings::{fill, Arg};

use crate::entities::ItemDisplays;
use crate::items::{Items, RecordWait, RecordWaits};
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfGuid, SelfPlayer};
use crate::ui_action::signals::SignalSink;
use crate::ui_chat::{ChatEvent, ChatEventKind, ChatLog};

/// The frames up at once (`0x6fb180`, `0x6fb2c0`: `[0xca0694] < 4`).
const MAX_SHOWN_ROLLS: u32 = 4;

/// `item_template.bonding` bind on pickup; only 1 gets `GroupLootFrame_OnShow`'s gold backdrop.
const BIND_WHEN_PICKED_UP: u32 = 1;

/// `Flags2` 0x100, an item only Greed may take: Need's reason 5 (`0x6fa7a0`).
const ITEM_FLAG2_NEED_ROLL_DISABLED: u32 = 0x100;

/// One group loot roll on the client's list.
struct Roll {
    /// The id Lua addresses the roll by (`+0x08`).
    roll_id: u32,
    looted_target: u64,
    item_slot: u32,
    item_id: u32,
    random_suffix: u32,
    /// The drop's random property, the hover's enchant source.
    random_property_id: u32,
    count: u32,
    vote_mask: u8,
    /// The wire's `countdownTime` (`+0x44`), `START_LOOT_ROLL`'s `arg2`.
    countdown_ms: u32,
    /// The clock's milliseconds at which the roll ends (`+0x2c`), set at the start.
    end_ms: u64,
    /// Why Need, Greed and Disenchant are refused (`+0x52`), judged once the record is in.
    reasons: [u8; 3],
    /// Ready, waiting for a free frame (`+0x41`).
    pending_show: bool,
    /// Our vote went out (`+0x42`).
    finished: bool,
}

/// An announcement waiting on its item record and its roller's name, then composed; a won or
/// all-passed node closes its roll once composed.
struct LineNode {
    id: u32,
    line: RollLine,
    /// The roll a won or all-passed line closes, found by its slot (`0x6fb100`); `None` for an
    /// announcement or a roll not on our list.
    closes: Option<u32>,
}

/// Which announcement a line node composes.
enum RollLine {
    Announce(LootRoll),
    Won(LootRollWon),
    AllPassed(LootAllPassed),
}

/// The client's roll list and the lines in flight; cleared on disconnect.
#[derive(Resource, Default)]
pub(crate) struct LootRolls {
    rolls: Vec<Roll>,
    /// The next roll's id (`[0xca0690]`).
    next_id: u32,
    /// The frames up (`[0xca0694]`).
    shown: u32,
    lines: Vec<LineNode>,
    next_line: u32,
    /// The lines waiting on a roller's name, by guid.
    name_waits: Vec<(u64, u32)>,
}

impl LootRolls {
    fn roll_mut(&mut self, roll_id: u32) -> Option<&mut Roll> {
        self.rolls.iter_mut().find(|r| r.roll_id == roll_id)
    }

    /// A ready roll's frame (`0x6fb446`, `0x6fb2a6`): `START_LOOT_ROLL` while a frame is free,
    /// else it waits for one.
    fn show_or_pend(&mut self, roll_id: u32, sink: &mut dyn SignalSink) {
        let free = self.shown < MAX_SHOWN_ROLLS;
        let Some(r) = self.roll_mut(roll_id) else {
            return;
        };
        if !free {
            r.pending_show = true;
            return;
        }
        let countdown = r.countdown_ms;
        self.shown += 1;
        sink.fire(
            "START_LOOT_ROLL",
            vec![
                ScriptValue::Int(i64::from(roll_id)),
                ScriptValue::Int(i64::from(countdown)),
            ],
        );
    }

    /// The waiting rolls' frames, in list order, while one is free (`0x6fb180`).
    fn show_pending(&mut self, sink: &mut dyn SignalSink) {
        for i in 0..self.rolls.len() {
            if self.shown >= MAX_SHOWN_ROLLS {
                return;
            }
            let r = &mut self.rolls[i];
            if !r.pending_show {
                continue;
            }
            r.pending_show = false;
            self.shown += 1;
            sink.fire(
                "START_LOOT_ROLL",
                vec![
                    ScriptValue::Int(i64::from(r.roll_id)),
                    ScriptValue::Int(i64::from(r.countdown_ms)),
                ],
            );
        }
    }

    /// The roll's record is in: its reasons judged (`0x6fa7a0`), the carried-count check run quiet
    /// on its item and stack (`0x6fa7db`), and its frame shown or queued.
    fn ready(
        &mut self,
        roll_id: u32,
        record: &benilla_protocol::messages::ItemInfo,
        carry: &crate::ui_loot::CarryCheck,
        sink: &mut dyn SignalSink,
    ) {
        let Some(r) = self.roll_mut(roll_id) else {
            return;
        };
        let over = carry(r.item_id, record, r.count).is_some();
        r.reasons = reasons(r.vote_mask, record, over);
        self.show_or_pend(roll_id, sink);
    }

    /// A won or all-passed roll closes (`0x6fb953`, `0x6fb4f6`): an unfinished one signals
    /// `CANCEL_LOOT_ROLL` and frees its frame; it leaves the list and a waiting roll may show.
    fn close(&mut self, roll_id: u32, sink: &mut dyn SignalSink) {
        let Some(i) = self.rolls.iter().position(|r| r.roll_id == roll_id) else {
            return;
        };
        let r = self.rolls.remove(i);
        if !r.finished {
            sink.fire(
                "CANCEL_LOOT_ROLL",
                vec![ScriptValue::Int(i64::from(roll_id))],
            );
            self.shown = self.shown.saturating_sub(1);
        }
        self.show_pending(sink);
    }

    /// Our vote on `roll_id` (`0x6fbc70`), the confirm and the vote mask already judged by the
    /// verb: the roll's wire identity once it went out; the frame closes, the roll finishes and a
    /// waiting roll may show. `None` for a roll not listed or already finished.
    fn vote(&mut self, roll_id: u32, sink: &mut dyn SignalSink) -> Option<(u64, u32)> {
        let r = self.roll_mut(roll_id).filter(|r| !r.finished)?;
        r.finished = true;
        let wire = (r.looted_target, r.item_slot);
        sink.fire(
            "CANCEL_LOOT_ROLL",
            vec![ScriptValue::Int(i64::from(roll_id))],
        );
        self.shown = self.shown.saturating_sub(1);
        self.show_pending(sink);
        Some(wire)
    }

    /// A line node for `line`, closing `closes` once composed; its id.
    fn add_line(&mut self, line: RollLine, closes: Option<u32>) -> u32 {
        let id = self.next_line;
        self.next_line = self.next_line.wrapping_add(1);
        self.lines.push(LineNode { id, line, closes });
        id
    }

    /// A line whose record or name the server denied goes, and with it the roll it carried,
    /// unsignalled, as the callback frees the node (`0x6fb9b0`, `0x6fb560`).
    fn drop_line(&mut self, line_id: u32) {
        let Some(i) = self.lines.iter().position(|l| l.id == line_id) else {
            return;
        };
        if let Some(roll_id) = self.lines.remove(i).closes {
            self.rolls.retain(|r| r.roll_id != roll_id);
        }
    }

    /// The world change's close of every roll (`0x6fafd0`): each unfinished roll passes on the
    /// wire and, shown, signals `CANCEL_LOOT_ROLL`; the list empties and no frame is up.
    fn close_all(&mut self, net: &NetCommands, sink: &mut dyn SignalSink) {
        for r in std::mem::take(&mut self.rolls) {
            if r.finished {
                continue;
            }
            let _ = net.0.send(ClientCommand::LootRoll {
                looted_target: r.looted_target,
                item_slot: r.item_slot,
                roll_type: benilla_protocol::messages::roll_vote::PASS,
            });
            if !r.pending_show {
                sink.fire(
                    "CANCEL_LOOT_ROLL",
                    vec![ScriptValue::Int(i64::from(r.roll_id))],
                );
            }
        }
        self.shown = 0;
    }

    /// Session teardown: nothing signalled or sent, as the UI goes down with the session.
    pub(crate) fn clear(&mut self) {
        self.rolls.clear();
        self.shown = 0;
        self.lines.clear();
        self.name_waits.clear();
    }
}

/// The carried-count check over our bags (`0x754dd0`), for Need's reason 2 (`0x6fa7db`); with no
/// player object it refuses nothing.
fn carry_check(
    me: Option<&ObjectStore>,
    objects: &crate::net::Objects,
    items: &Items,
    limits: Option<&crate::ui_trade::ItemLimitCategories>,
    (item_id, record, count): (u32, &benilla_protocol::messages::ItemInfo, u32),
) -> Option<benilla_ui::script::CarryRefusal> {
    crate::ui_items::carry_refusal(
        &me?.0,
        objects,
        items,
        limits.map(|l| &l.0),
        item_id,
        record,
        count,
    )
}

/// The clock the roll's end and its time left read (`0x588d21`).
fn now_ms(time: Option<&Time<bevy::time::Real>>) -> u64 {
    time.map_or(0, |t| t.elapsed().as_millis() as u64)
}

/// The group rolls' packet handlers.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{LootRolls, RollLine};
    use crate::items::{Items, RecordWait, RecordWaits};
    use crate::net::{NetCommands, NetHandlerApp, ObjectStore, SelfPlayer};
    use crate::ui_script::live::UiEvents;

    /// Register the roll handlers, called from [`super::UiLootRollPlugin`].
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::LootStartRoll, on_start)
            .net_handler(K::LootRoll, on_line)
            .net_handler(K::LootRollWon, on_line)
            .net_handler(K::LootAllPassed, on_line)
            .net_handler(K::PlayerName, on_name_answer)
            .net_handler(K::PlayerNameMiss, on_name_answer)
            .net_handler(K::Worldport, on_world_change)
            .net_handler(K::Disconnected, on_session_end);
    }

    /// What a roll step reads: the item cache and its callbacks, the wire, the clock.
    #[derive(bevy::ecs::system::SystemParam)]
    pub(super) struct RollReads<'w, 's> {
        items: Res<'w, Items>,
        waits: NonSendMut<'w, RecordWaits>,
        net: Res<'w, NetCommands>,
        time: Option<Res<'w, Time<bevy::time::Real>>>,
        me: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
        objects: crate::net::Objects<'w, 's>,
        limits: Option<Res<'w, crate::ui_trade::ItemLimitCategories>>,
    }

    /// `SMSG_LOOT_START_ROLL` (`0x6fb2c0`): a roll for another map is dropped (`0x6fb2db`); else it
    /// is listed under the next serial (`0x6fb37f`), its end set (`0x6fb3ee`), and its record read
    /// with the start's callback (`0x6fb410`): cached, its frame shows or waits at once.
    pub(super) fn on_start(
        In(ev): In<SessionEvent>,
        mut rolls: ResMut<LootRolls>,
        mut reads: RollReads,
        map: Option<Res<benilla_world::world_map::CurrentMap>>,
        mut ui: ResMut<UiEvents>,
    ) {
        let SessionEvent::LootStartRoll(p) = ev else {
            return;
        };
        debug!(
            "net: loot roll opened on item {} ({:#x} slot {}, map {}), {} ms, mask {:#x}",
            p.item_id, p.looted_target, p.item_slot, p.map_id, p.countdown_ms, p.vote_mask
        );
        if map.is_some_and(|m| m.0 != p.map_id) {
            debug!("ui_loot_roll: roll on map {} — not ours, dropped", p.map_id);
            return;
        }
        let roll_id = rolls.next_id;
        rolls.next_id = rolls.next_id.wrapping_add(1);
        let end_ms = super::now_ms(reads.time.as_deref()) + u64::from(p.countdown_ms);
        rolls.rolls.push(super::Roll {
            roll_id,
            looted_target: p.looted_target,
            item_slot: p.item_slot,
            item_id: p.item_id,
            random_suffix: p.random_suffix,
            random_property_id: p.random_property_id,
            count: p.count,
            vote_mask: p.vote_mask,
            countdown_ms: p.countdown_ms,
            end_ms,
            reasons: [0; 3],
            pending_show: false,
            finished: false,
        });
        match reads.items.ask_template(p.item_id, 0, &reads.net) {
            Some(record) => {
                let me = reads.me.single().ok();
                let carry = |i, r: &_, c| {
                    super::carry_check(
                        me,
                        &reads.objects,
                        &reads.items,
                        reads.limits.as_deref(),
                        (i, r, c),
                    )
                };
                rolls.ready(roll_id, record, &carry, &mut *ui);
            }
            None => reads
                .waits
                .wait(p.item_id, RecordWait::GroupRollStart(roll_id), false),
        }
    }

    /// `SMSG_LOOT_ROLL` (`0x6fae40`), `SMSG_LOOT_ROLL_WON` (`0x6fbb10`) and `SMSG_LOOT_ALL_PASSED`
    /// (`0x6fb600`): a line node, the won and all-passed ones on the roll their slot finds; each
    /// reads its item record with its callback, then the roller's name. Both resolutions make the
    /// slot lootable in an open window on that object at once (`0x6fbc4d`, `0x6fb6e2` ->
    /// `0x6dc770`).
    pub(super) fn on_line(
        In(ev): In<SessionEvent>,
        mut rolls: ResMut<LootRolls>,
        mut loot: ResMut<crate::ui_loot::LootState>,
        mut ui: ResMut<UiEvents>,
    ) {
        let (line, slot) = match ev {
            SessionEvent::LootRoll(p) => {
                debug!(
                    "net: loot roll announce — roller {:#x} number {} type {} auto {}",
                    p.roller, p.roll_number, p.roll_type, p.auto_pass
                );
                (RollLine::Announce(p), None)
            }
            SessionEvent::LootRollWon(p) => {
                debug!(
                    "net: loot roll won by {:#x} with {} (type {})",
                    p.winner, p.roll_number, p.roll_type
                );
                loot.clear_slot_type(p.looted_target, p.item_slot);
                (RollLine::Won(p), Some((p.looted_target, p.item_slot)))
            }
            SessionEvent::LootAllPassed(p) => {
                debug!("net: loot roll — everyone passed on item {}", p.item_id);
                loot.clear_slot_type(p.looted_target, p.item_slot);
                (RollLine::AllPassed(p), Some((p.looted_target, p.item_slot)))
            }
            _ => return,
        };
        let closes = slot.and_then(|(target, slot)| {
            rolls
                .rolls
                .iter()
                .find(|r| r.looted_target == target && r.item_slot == slot)
                .map(|r| r.roll_id)
        });
        let id = rolls.add_line(line, closes);
        ui.world(move |world| super::advance_line(world, id));
    }

    /// A player name answered: each line waiting on it composes on a found name, a placeholder
    /// counting as found (`0x635894`), and goes on a denied one (`0x6facd0`, `0x6fb9b0`); a retry
    /// asks again and tells no callback (`0x6358b3`), so the lines keep waiting.
    pub(super) fn on_name_answer(
        In(ev): In<SessionEvent>,
        mut rolls: ResMut<LootRolls>,
        mut ui: ResMut<UiEvents>,
    ) {
        use benilla_protocol::messages::NameQueryMiss;
        let (guid, found) = match ev {
            SessionEvent::PlayerName { guid, .. } => (guid, true),
            SessionEvent::PlayerNameMiss {
                miss: NameQueryMiss::Retry,
                ..
            } => return,
            SessionEvent::PlayerNameMiss { guid, miss } => {
                (guid, miss == NameQueryMiss::Placeholder)
            }
            _ => return,
        };
        let waiting: Vec<u32> = rolls
            .name_waits
            .iter()
            .filter(|(g, _)| *g == guid)
            .map(|(_, id)| *id)
            .collect();
        rolls.name_waits.retain(|(g, _)| *g != guid);
        for id in waiting {
            if found {
                ui.world(move |world| super::finish_line(world, id));
            } else {
                rolls.drop_line(id);
            }
        }
    }

    /// `SMSG_NEW_WORLD`'s world load closes every roll (`0x403c03` -> `0x6fafd0`).
    pub(super) fn on_world_change(
        In(ev): In<SessionEvent>,
        mut rolls: ResMut<LootRolls>,
        net: Res<NetCommands>,
        mut ui: ResMut<UiEvents>,
    ) {
        if matches!(ev, SessionEvent::Worldport { .. }) {
            rolls.close_all(&net, &mut *ui);
        }
    }

    fn on_session_end(In(_): In<SessionEvent>, mut rolls: ResMut<LootRolls>) {
        rolls.clear();
    }
}

pub(crate) struct UiLootRollPlugin;

impl Plugin for UiLootRollPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<LootRolls>();
    }
}

/// What one line needs besides its packet: the roller's name and sex, whether the roller is us,
/// the item link and `showLootSpam`.
struct LineInputs<'a> {
    name: Option<&'a str>,
    female: bool,
    is_self: bool,
    link: &'a str,
    spam: bool,
}

/// Compose one announcement from `GlobalStrings.lua`, as `0x6fa840`, `0x6fb700` and `0x6fb470`
/// do; `None` drops it. With `showLootSpam` 0 a vote or dice line drops unless it is our own
/// auto-pass, and the won line takes the `*_NO_SPAM_*` form with the roll number.
fn format_line(
    line: &RollLine,
    inputs: &LineInputs,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let LineInputs {
        name,
        female,
        is_self,
        link,
        spam,
    } = *inputs;
    let name = name.unwrap_or_default();
    let s = |key: &str, args: &[Arg]| get(key).map(|t| fill(&t, args));
    match line {
        RollLine::Announce(p) => {
            if !spam && !(is_self && p.auto_pass) {
                return None;
            }
            let dice =
                |key: &str, n: u8| s(key, &[Arg::D(i64::from(n)), Arg::S(link), Arg::S(name)]);
            match (p.announce(), is_self) {
                (RollAnnounce::NeedRoll(n), _) => dice("LOOT_ROLL_ROLLED_NEED", n),
                (RollAnnounce::GreedRoll(n), _) => dice("LOOT_ROLL_ROLLED_GREED", n),
                (RollAnnounce::DisenchantRoll(n), _) => dice("LOOT_ROLL_ROLLED_DE", n),
                (RollAnnounce::Need, true) => s("LOOT_ROLL_NEED_SELF", &[Arg::S(link)]),
                (RollAnnounce::Greed, true) => s("LOOT_ROLL_GREED_SELF", &[Arg::S(link)]),
                (RollAnnounce::Disenchant, true) => s("LOOT_ROLL_DISENCHANT_SELF", &[Arg::S(link)]),
                (RollAnnounce::Pass, true) if p.auto_pass => {
                    s("LOOT_ROLL_PASSED_SELF_AUTO", &[Arg::S(link)])
                }
                (RollAnnounce::Pass, true) => s("LOOT_ROLL_PASSED_SELF", &[Arg::S(link)]),
                (RollAnnounce::Need, false) => s("LOOT_ROLL_NEED", &[Arg::S(name), Arg::S(link)]),
                (RollAnnounce::Greed, false) => s("LOOT_ROLL_GREED", &[Arg::S(name), Arg::S(link)]),
                (RollAnnounce::Disenchant, false) => {
                    s("LOOT_ROLL_DISENCHANT", &[Arg::S(name), Arg::S(link)])
                }
                // The gendered lookup: the `_FEMALE` key first for a female roller.
                (RollAnnounce::Pass, false) if p.auto_pass => female
                    .then(|| {
                        s(
                            "LOOT_ROLL_PASSED_AUTO_FEMALE",
                            &[Arg::S(name), Arg::S(link)],
                        )
                    })
                    .flatten()
                    .or_else(|| s("LOOT_ROLL_PASSED_AUTO", &[Arg::S(name), Arg::S(link)])),
                (RollAnnounce::Pass, false) => s("LOOT_ROLL_PASSED", &[Arg::S(name), Arg::S(link)]),
            }
        }
        RollLine::Won(p) => {
            let n = Arg::D(i64::from(p.roll_number));
            let kind = match p.roll_type {
                1 => "NEED",
                2 => "GREED",
                3 => "DE",
                _ => "",
            };
            match (is_self, spam) {
                (true, true) => s("LOOT_ROLL_YOU_WON", &[Arg::S(link)]),
                (false, true) => s("LOOT_ROLL_WON", &[Arg::S(name), Arg::S(link)]),
                // Any other roll type composes nothing in the reference; no line here.
                (_, false) if kind.is_empty() => None,
                (true, false) => s(
                    &format!("LOOT_ROLL_YOU_WON_NO_SPAM_{kind}"),
                    &[n, Arg::S(link)],
                ),
                (false, false) => s(
                    &format!("LOOT_ROLL_WON_NO_SPAM_{kind}"),
                    &[Arg::S(name), n, Arg::S(link)],
                ),
            }
        }
        RollLine::AllPassed(_) => s("LOOT_ROLL_ALL_PASSED", &[Arg::S(link)]),
    }
}

/// `0x6fa7a0`: why Need, Greed and Disenchant are refused, 0 when the vote mask allows them. Need
/// is 2 when the carried-count check refuses the drop (`over`), else 5 for a greed-only item, else
/// 1; Greed is 2; Disenchant is 4 for a disenchantable item, else 3.
fn reasons(vote_mask: u8, template: &benilla_protocol::messages::ItemInfo, over: bool) -> [u8; 3] {
    let need = if vote_mask & 0x2 != 0 {
        0
    } else if over {
        2
    } else if template.flags2 & ITEM_FLAG2_NEED_ROLL_DISABLED != 0 {
        5
    } else {
        1
    };
    let greed = if vote_mask & 0x4 != 0 { 0 } else { 2 };
    let disenchant = if vote_mask & 0x8 != 0 {
        0
    } else if template.required_disenchant_skill >= 0 {
        4
    } else {
        3
    };
    [need, greed, disenchant]
}

/// The roll's start callback (`0x6fb1f0`): a found record judges the reasons and shows or queues
/// the frame; a denied one drops the roll, unsignalled.
pub(crate) fn start_callback(world: &mut World, roll_id: u32, entry: u32, landed: bool) {
    let _ = bevy::ecs::system::RunSystemOnce::run_system_once_with(
        world,
        |In((roll_id, entry, landed)): In<(u32, u32, bool)>,
         mut rolls: ResMut<LootRolls>,
         items: Res<Items>,
         me: Query<&ObjectStore, With<SelfPlayer>>,
         objects: crate::net::Objects,
         limits: Option<Res<crate::ui_trade::ItemLimitCategories>>,
         mut ui: ResMut<crate::ui_script::live::UiEvents>| {
            let record = items.template_cached(entry).filter(|_| landed);
            let Some(record) = record else {
                rolls.rolls.retain(|r| r.roll_id != roll_id);
                return;
            };
            let me = me.single().ok();
            let carry =
                |i, r: &_, c| carry_check(me, &objects, &items, limits.as_deref(), (i, r, c));
            rolls.ready(roll_id, record, &carry, &mut *ui);
        },
        (roll_id, entry, landed),
    );
}

/// The item a line links and the roller whose name it reads, none for all-passed.
fn line_keys(line: &RollLine) -> (u32, Option<u64>) {
    match line {
        RollLine::Announce(p) => (p.item_id, Some(p.roller)),
        RollLine::Won(p) => (p.item_id, Some(p.winner)),
        RollLine::AllPassed(p) => (p.item_id, None),
    }
}

/// A line's next wait: its item record read with its callback (`0x6faf04`, `0x6fbc11`,
/// `0x6fb6c6`), then its roller's name with the name's (`0x6faf24`, `0x6fbc31` -> `0x67d770`, which
/// asks again after a deny); with both in, it composes.
pub(crate) fn advance_line(world: &mut World, line_id: u32) {
    let ready = bevy::ecs::system::RunSystemOnce::run_system_once_with(
        &mut *world,
        |In(line_id): In<u32>,
         mut rolls: ResMut<LootRolls>,
         items: Res<Items>,
         names: Res<NameCache>,
         mut waits: NonSendMut<RecordWaits>,
         net: Res<NetCommands>| {
            let Some(node) = rolls.lines.iter().find(|l| l.id == line_id) else {
                return false;
            };
            let (item, roller) = line_keys(&node.line);
            if items.ask_template(item, 0, &net).is_none() {
                waits.wait(item, RecordWait::GroupRollLine(line_id), false);
                return false;
            }
            if let Some(guid) = roller {
                if names.full_name_record(guid, &net).is_none() {
                    rolls.name_waits.push((guid, line_id));
                    return false;
                }
            }
            true
        },
        line_id,
    );
    if ready.unwrap_or(false) {
        finish_line(world, line_id);
    }
}

/// A line's record callback (`0x6fad80`, `0x6fba60`, `0x6fb560`): found, the line goes on to its
/// name; denied, it goes.
pub(crate) fn line_record_callback(world: &mut World, line_id: u32, landed: bool) {
    if landed {
        advance_line(world, line_id);
    } else if let Some(mut rolls) = world.get_resource_mut::<LootRolls>() {
        rolls.drop_line(line_id);
    }
}

/// A line composed (`0x6fa840`, `0x6fb700`, `0x6fb470`): its chat line, then a won or all-passed
/// roll closes.
pub(crate) fn finish_line(world: &mut World, line_id: u32) {
    let _ = bevy::ecs::system::RunSystemOnce::run_system_once_with(
        world,
        |In(line_id): In<u32>,
         mut rolls: ResMut<LootRolls>,
         items: Res<Items>,
         names: Res<NameCache>,
         self_guid: Res<SelfGuid>,
         loot: Res<crate::ui_loot::LootConfig>,
         catalogs: (
            Option<Res<crate::items::RandomProperties>>,
            Option<Res<crate::items::Enchants>>,
        ),
         script: Option<NonSend<benilla_ui::script::UiScript>>,
         mut chat: ResMut<ChatLog>,
         mut ui: ResMut<crate::ui_script::live::UiEvents>| {
            let Some(i) = rolls.lines.iter().position(|l| l.id == line_id) else {
                return;
            };
            let node = rolls.lines.remove(i);
            let catalogs = crate::items::RollCatalogs {
                props: catalogs.0.as_deref(),
                enchants: catalogs.1.as_deref(),
            };
            let get = |key: &str| {
                script
                    .as_deref()
                    .and_then(|s| benilla_ui::strings::global(s.lua(), key))
            };
            let text = compose(
                &node.line,
                self_guid.0,
                &items,
                &names,
                catalogs,
                loot.show_loot_spam,
                &get,
            );
            if let Some(text) = text {
                chat.push_event(ChatEvent::text_only(ChatEventKind::Loot, text));
            }
            if let Some(roll_id) = node.closes {
                rolls.close(roll_id, &mut *ui);
            }
        },
        line_id,
    );
}

/// The line's text off its cached record and name, or none when it composes nothing.
fn compose(
    line: &RollLine,
    self_guid: Option<u64>,
    items: &Items,
    names: &NameCache,
    catalogs: crate::items::RollCatalogs,
    spam: bool,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let (item, property, suffix, roller) = match line {
        RollLine::Announce(p) => (
            p.item_id,
            p.random_property_id,
            p.random_suffix,
            Some(p.roller),
        ),
        RollLine::Won(p) => (
            p.item_id,
            p.random_property_id,
            p.random_suffix,
            Some(p.winner),
        ),
        RollLine::AllPassed(p) => (p.item_id, p.random_property_id, p.random_suffix, None),
    };
    let t = items.template_cached(item)?;
    let link = crate::ui_items::item_link_full(
        item,
        &benilla_ui::script::ItemLinkFields::roll(property as i32, suffix as i32),
        &catalogs.name(&t.name, property),
        t.quality,
    );
    let name = roller.and_then(|g| names.peek(g)).map(str::to_string);
    let female = roller
        .and_then(|g| names.player_traits(g))
        .is_some_and(|(_, _, gender)| gender == 1);
    let inputs = LineInputs {
        name: name.as_deref(),
        female,
        is_self: roller.is_some() && roller == self_guid,
        link: &link,
        spam,
    };
    format_line(line, &inputs, get)
}

/// What the roll verbs read at the call: the list, the item cache, the icons and the clock.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct LootRollsLive<'w> {
    rolls: Res<'w, LootRolls>,
    items: Res<'w, Items>,
    icons: Option<Res<'w, ItemDisplays>>,
    props: Option<Res<'w, crate::items::RandomProperties>>,
    enchants: Option<Res<'w, crate::items::Enchants>>,
    time: Option<Res<'w, Time<bevy::time::Real>>>,
}

impl LootRollsLive<'_> {
    /// Roll `roll_id` resolved at the call (`0x6fb150`), its record read with no callback
    /// (`0x588a51`); its template fields `None` while the record is not cached.
    pub(crate) fn entry(&self, roll_id: i64) -> Option<LootRollEntry> {
        let r = self
            .rolls
            .rolls
            .iter()
            .find(|r| i64::from(r.roll_id) == roll_id)?;
        let catalogs = crate::items::RollCatalogs {
            props: self.props.as_deref(),
            enchants: self.enchants.as_deref(),
        };
        let t = self.items.template_cached(r.item_id);
        // `0x588d29`: the end less the clock in 32 bits, an expired roll wrapping near 2^32.
        let left = r.end_ms.wrapping_sub(now_ms(self.time.as_deref())) as u32;
        Some(LootRollEntry {
            roll_id: r.roll_id,
            name: t.map(|t| catalogs.name(&t.name, r.random_property_id)),
            texture: t
                .and_then(|t| crate::ui_items::item_icon(self.icons.as_deref(), t.display_info_id)),
            quantity: r.count,
            quality: t.map(|t| t.quality),
            bind_on_pickup: t.is_some_and(|t| t.bonding == BIND_WHEN_PICKED_UP),
            vote_mask: r.vote_mask,
            reasons: r.reasons,
            disenchant_skill: t.map_or(-1, |t| t.required_disenchant_skill),
            time_left_ms: left,
            item_id: r.item_id,
            random_property_id: r.random_property_id,
            link: t.map(|t| {
                crate::ui_items::item_link_full(
                    r.item_id,
                    &benilla_ui::script::ItemLinkFields::roll(
                        r.random_property_id as i32,
                        r.random_suffix as i32,
                    ),
                    &catalogs.name(&t.name, r.random_property_id),
                    t.quality,
                )
            }),
            finished: r.finished,
        })
    }
}

/// A vote past its confirm and its mask, inside its call (`0x6fbc70`): `CMSG_LOOT_ROLL` with the
/// roll's wire identity (`0x6fbdab`), then `CANCEL_LOOT_ROLL`, the frame freed and the roll
/// finished (`0x6fbdc2`-`0x6fbdd1`), and a waiting roll shown (`0x6fbdd5`), raised as the call
/// returns.
pub(crate) fn vote_in_call(world: &mut World, roll_id: u32, roll_type: u8) {
    use crate::ui_script::live::UiEvents;
    if !world.contains_resource::<NetCommands>() {
        return;
    }
    let Some(mut rolls) = world.remove_resource::<LootRolls>() else {
        return;
    };
    let mut ui = world.remove_resource::<UiEvents>().unwrap_or_default();
    match rolls.vote(roll_id, &mut ui) {
        Some((looted_target, item_slot)) => {
            debug!("ui_loot_roll: vote {roll_type} on roll {roll_id}");
            let _ = world
                .resource::<NetCommands>()
                .0
                .send(ClientCommand::LootRoll {
                    looted_target,
                    item_slot,
                    roll_type,
                });
        }
        None => debug!("ui_loot_roll: vote on roll {roll_id} — no open roll, ignored"),
    }
    world.insert_resource(rolls);
    world.insert_resource(ui);
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::roll_vote;
    use benilla_protocol::messages::LootStartRoll;

    fn start(target: u64, slot: u32, item_id: u32) -> LootStartRoll {
        LootStartRoll {
            looted_target: target,
            map_id: 0,
            item_slot: slot,
            item_id,
            random_suffix: 0,
            random_property_id: 0,
            count: 1,
            countdown_ms: 60_000,
            vote_mask: 0x0F,
        }
    }

    fn announce(roll_number: u8, roll_type: u8, auto_pass: bool) -> LootRoll {
        LootRoll {
            looted_target: 0xAA,
            item_slot: 0,
            roller: 1,
            item_id: 17182,
            random_suffix: 0,
            random_property_id: 0,
            roll_number,
            roll_type,
            auto_pass,
        }
    }

    fn won(roll_number: u8, roll_type: u8) -> LootRollWon {
        LootRollWon {
            looted_target: 0xAA,
            item_slot: 0,
            item_id: 17182,
            random_suffix: 0,
            random_property_id: 0,
            winner: 1,
            roll_number,
            roll_type,
        }
    }

    const LINK: &str = "[L]";

    /// The 3.3.5a enUS `GlobalStrings.lua` templates the composers read.
    fn strings(key: &str) -> Option<String> {
        Some(
            match key {
                "LOOT_ROLL_ALL_PASSED" => "Everyone passed on: %s",
                "LOOT_ROLL_DISENCHANT" => "%s has selected Disenchant for: %s",
                "LOOT_ROLL_DISENCHANT_SELF" => "You have selected Disenchant for: %s",
                "LOOT_ROLL_GREED" => "%s has selected Greed for: %s",
                "LOOT_ROLL_GREED_SELF" => "You have selected Greed for: %s",
                "LOOT_ROLL_NEED" => "%s has selected Need for: %s",
                "LOOT_ROLL_NEED_SELF" => "You have selected Need for: %s",
                "LOOT_ROLL_PASSED" => "%s passed on: %s",
                "LOOT_ROLL_PASSED_AUTO" => {
                    "%s automatically passed on: %s because he cannot loot that item."
                }
                "LOOT_ROLL_PASSED_AUTO_FEMALE" => {
                    "%s automatically passed on: %s because she cannot loot that item."
                }
                "LOOT_ROLL_PASSED_SELF" => "You passed on: %s",
                "LOOT_ROLL_PASSED_SELF_AUTO" => {
                    "You automatically passed on: %s because you cannot loot that item."
                }
                "LOOT_ROLL_ROLLED_DE" => "Disenchant Roll - %d for %s by %s",
                "LOOT_ROLL_ROLLED_GREED" => "Greed Roll - %d for %s by %s",
                "LOOT_ROLL_ROLLED_NEED" => "Need Roll - %d for %s by %s",
                "LOOT_ROLL_WON" => "%s won: %s",
                "LOOT_ROLL_WON_NO_SPAM_DE" => "%1$s won: %3$s |cff818181(Disenchant - %2$d)|r",
                "LOOT_ROLL_WON_NO_SPAM_GREED" => "%1$s won: %3$s |cff818181(Greed - %2$d)|r",
                "LOOT_ROLL_WON_NO_SPAM_NEED" => "%1$s won: %3$s |cff818181(Need - %2$d)|r",
                "LOOT_ROLL_YOU_WON" => "You won: %s",
                "LOOT_ROLL_YOU_WON_NO_SPAM_DE" => "You won: %2$s |cff818181(Disenchant - %1$d)|r",
                "LOOT_ROLL_YOU_WON_NO_SPAM_GREED" => "You won: %2$s |cff818181(Greed - %1$d)|r",
                "LOOT_ROLL_YOU_WON_NO_SPAM_NEED" => "You won: %2$s |cff818181(Need - %1$d)|r",
                _ => return None,
            }
            .to_string(),
        )
    }

    fn line(l: &RollLine, name: &str, female: bool, is_self: bool, spam: bool) -> Option<String> {
        let inputs = LineInputs {
            name: Some(name),
            female,
            is_self,
            link: LINK,
            spam,
        };
        format_line(l, &inputs, &strings)
    }

    /// `0x6fa840`'s table with `showLootSpam` on: the vote lines have self forms, the dice lines
    /// name the roller either way, and an auto-pass has its own words.
    #[test]
    fn every_announce_shape_takes_its_string() {
        let cases: &[(LootRoll, bool, bool, &str)] = &[
            (
                announce(0, 0, false),
                false,
                false,
                "Bob has selected Need for: [L]",
            ),
            (
                announce(0, 0, false),
                true,
                false,
                "You have selected Need for: [L]",
            ),
            (
                announce(128, 2, false),
                false,
                false,
                "Bob has selected Greed for: [L]",
            ),
            (
                announce(128, 2, false),
                true,
                false,
                "You have selected Greed for: [L]",
            ),
            (
                announce(128, 3, false),
                false,
                false,
                "Bob has selected Disenchant for: [L]",
            ),
            (
                announce(128, 3, false),
                true,
                false,
                "You have selected Disenchant for: [L]",
            ),
            (announce(128, 0, false), false, false, "Bob passed on: [L]"),
            (announce(128, 0, false), true, false, "You passed on: [L]"),
            (
                announce(128, 0, true),
                false,
                false,
                "Bob automatically passed on: [L] because he cannot loot that item.",
            ),
            (
                announce(128, 0, true),
                false,
                true,
                "Bob automatically passed on: [L] because she cannot loot that item.",
            ),
            (
                announce(128, 0, true),
                true,
                false,
                "You automatically passed on: [L] because you cannot loot that item.",
            ),
            (
                announce(57, 1, false),
                true,
                false,
                "Need Roll - 57 for [L] by Bob",
            ),
            (
                announce(57, 2, false),
                false,
                false,
                "Greed Roll - 57 for [L] by Bob",
            ),
            (
                announce(57, 3, false),
                false,
                false,
                "Disenchant Roll - 57 for [L] by Bob",
            ),
        ];
        for (p, is_self, female, want) in cases {
            assert_eq!(
                line(&RollLine::Announce(*p), "Bob", *female, *is_self, true).as_deref(),
                Some(*want),
                "({}, {}) self {is_self}",
                p.roll_number,
                p.roll_type
            );
        }
    }

    /// `showLootSpam` 0 drops every vote and dice line but our own auto-pass.
    #[test]
    fn spam_off_keeps_only_our_own_auto_pass() {
        for (p, is_self, kept) in [
            (announce(128, 0, true), true, true),
            (announce(128, 0, true), false, false),
            (announce(128, 0, false), true, false),
            (announce(57, 1, false), true, false),
        ] {
            assert_eq!(
                line(&RollLine::Announce(p), "Bob", false, is_self, false).is_some(),
                kept
            );
        }
    }

    /// `0x6fb700`: the plain winner lines with spam on, the roll-number forms with it off, the
    /// Disenchant form included.
    #[test]
    fn winner_lines_follow_the_spam_setting() {
        let w = |n, t, is_self, spam| line(&RollLine::Won(won(n, t)), "Bob", false, is_self, spam);
        assert_eq!(w(84, 1, true, true).as_deref(), Some("You won: [L]"));
        assert_eq!(w(84, 1, false, true).as_deref(), Some("Bob won: [L]"));
        assert_eq!(
            w(84, 1, true, false).as_deref(),
            Some("You won: [L] |cff818181(Need - 84)|r")
        );
        assert_eq!(
            w(84, 3, false, false).as_deref(),
            Some("Bob won: [L] |cff818181(Disenchant - 84)|r")
        );
        assert_eq!(
            w(12, 2, false, false).as_deref(),
            Some("Bob won: [L] |cff818181(Greed - 12)|r")
        );
        assert_eq!(w(84, 0, false, false), None, "no form for type 0");
        let passed = LootAllPassed {
            looted_target: 0xAA,
            item_slot: 0,
            item_id: 17182,
            random_property_id: 0,
            random_suffix: 0,
        };
        assert_eq!(
            line(&RollLine::AllPassed(passed), "", false, false, false).as_deref(),
            Some("Everyone passed on: [L]")
        );
    }

    /// `0x6fa7a0` for each mask bit and fallback.
    #[test]
    fn the_refusal_reasons_follow_the_mask_and_the_item() {
        let mut t = crate::items::test_template("Cleaver");
        assert_eq!(reasons(0x0F, &t, false), [0, 0, 0]);
        assert_eq!(
            reasons(0x01, &t, false),
            [1, 2, 3],
            "plain item, not disenchantable"
        );
        t.required_disenchant_skill = 25;
        t.flags2 = ITEM_FLAG2_NEED_ROLL_DISABLED;
        assert_eq!(reasons(0x05, &t, false), [5, 0, 4]);
        assert_eq!(
            reasons(0x05, &t, true),
            [2, 0, 4],
            "the count check refuses"
        );
        assert_eq!(
            reasons(0x07, &t, true),
            [0, 0, 4],
            "the mask's Need bit wins (`0x6fa7e8`)"
        );
    }

    // ── Driven through the handlers and the record callbacks ──

    const ROLLED: u32 = 17182;
    const ME: u64 = 0x0000_0000_0000_0042;

    /// A world with the rolls' reads and a VM whose listener records `rollID:rollTime:name` per
    /// `START_LOOT_ROLL`, the call stock `GroupLootFrame_OnShow` makes, and each `CANCEL_LOOT_ROLL`.
    fn roll_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<LootRolls>()
            .init_resource::<Items>()
            .init_resource::<NameCache>()
            .insert_resource(SelfGuid(Some(ME)))
            .init_resource::<ChatLog>()
            .init_resource::<crate::ui_loot::LootConfig>()
            .init_resource::<crate::ui_loot::LootState>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::ui_script::live::UiEvents>()
            .init_resource::<Time<bevy::time::Real>>()
            .insert_resource(NetCommands(tx));
        app.insert_non_send_resource(RecordWaits::default());
        let script = benilla_ui::script::UiScript::new().unwrap();
        script
            .run(
                "SEEN = {}\n\
                 local f = CreateFrame(\"Frame\")\n\
                 f:RegisterEvent(\"START_LOOT_ROLL\")\n\
                 f:RegisterEvent(\"CANCEL_LOOT_ROLL\")\n\
                 f:SetScript(\"OnEvent\", function(self, event, arg1, arg2)\n\
                   if event == \"CANCEL_LOOT_ROLL\" then tinsert(SEEN, \"cancel \" .. arg1) return end\n\
                   local _, name = GetLootRollItemInfo(arg1)\n\
                   tinsert(SEEN, arg1 .. \":\" .. arg2 .. \":\" .. (name or \"<blank>\"))\n\
                 end)",
            )
            .unwrap();
        // The line templates the composers read, as `GlobalStrings.lua` has them.
        for key in [
            "LOOT_ROLL_WON",
            "LOOT_ROLL_YOU_WON",
            "LOOT_ROLL_NEED",
            "LOOT_ROLL_ROLLED_NEED",
            "LOOT_ROLL_ALL_PASSED",
        ] {
            if let Some(text) = strings(key) {
                script.lua().globals().set(key, text).unwrap();
            }
        }
        app.insert_non_send_resource(script);
        (app, rx)
    }

    /// What the listener saw since the last call.
    fn seen(app: &mut App) -> Vec<String> {
        app.world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>()
            .eval::<Vec<String>>("local t = SEEN SEEN = {} return t")
            .unwrap()
    }

    /// `ev` through its handler, then what it raised fired with the `World` lent.
    fn send(app: &mut App, ev: SessionEvent) {
        let world = app.world_mut();
        let ran = match &ev {
            SessionEvent::LootStartRoll(_) => world.run_system_once_with(net::on_start, ev),
            SessionEvent::Worldport { .. } => world.run_system_once_with(net::on_world_change, ev),
            SessionEvent::PlayerName { .. } | SessionEvent::PlayerNameMiss { .. } => {
                world.run_system_once_with(net::on_name_answer, ev)
            }
            _ => world.run_system_once_with(net::on_line, ev),
        };
        ran.expect("the handler runs as a one-shot system");
        crate::ui_script::live::fire_handler_events(world);
    }

    /// `entry`'s record answered: the callbacks waiting on it run in ask order, each in its turn,
    /// as the answer's handler queues them.
    fn answer(app: &mut App, entry: u32, info: Option<ItemInfo>) {
        let world = app.world_mut();
        let landed = info.is_some();
        world.resource_mut::<Items>().insert_template(entry, info);
        let waits = world.non_send_resource_mut::<RecordWaits>().take(entry);
        let mut ui = world.resource_mut::<crate::ui_script::live::UiEvents>();
        for wait in waits {
            match wait {
                RecordWait::GroupRollStart(id) => {
                    ui.world(move |w| start_callback(w, id, entry, landed))
                }
                RecordWait::GroupRollLine(id) => {
                    ui.world(move |w| line_record_callback(w, id, landed))
                }
                _ => {}
            }
        }
        crate::ui_script::live::fire_handler_events(world);
    }

    fn cache(app: &mut App) {
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(ROLLED, Some(crate::items::test_template("Sulfuras")));
    }

    /// `chunk` run with the `World` lent, as a click's call reads the rolls, then the drain.
    fn vote(app: &mut App, chunk: &str) {
        crate::ui_script::live::with_live_world(app.world_mut(), |s| s.run(chunk))
            .expect("a VM")
            .unwrap();
        app.update();
    }

    fn sent(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<ClientCommand> {
        rx.try_iter().collect()
    }

    use crate::items::{RecordWait, RecordWaits};
    use benilla_protocol::messages::ItemInfo;
    use benilla_protocol::SessionEvent;
    use bevy::ecs::system::RunSystemOnce;

    /// `0x6fb2db`: a roll for another map than ours is dropped; the ids are a serial from 0
    /// (`[0xca0690]`), and the start has no duplicate test.
    #[test]
    fn a_roll_for_another_map_is_dropped_and_ids_count_from_zero() {
        let (mut app, _rx) = roll_app();
        cache(&mut app);
        app.insert_resource(benilla_world::world_map::CurrentMap(571));
        let mut p = start(0xAA, 0, ROLLED);
        p.map_id = 574;
        send(&mut app, SessionEvent::LootStartRoll(p));
        assert!(seen(&mut app).is_empty());
        p.map_id = 571;
        send(&mut app, SessionEvent::LootStartRoll(p));
        send(&mut app, SessionEvent::LootStartRoll(p));
        assert_eq!(seen(&mut app), ["0:60000:Sulfuras", "1:60000:Sulfuras"]);
    }

    /// The start reads its record with its callback (`0x6fb410`): a miss asks, waits and shows
    /// from the callback (`0x6fb1f0`); a denied record drops the roll, unshown.
    #[test]
    fn the_roll_waits_for_its_item_record() {
        let (mut app, rx) = roll_app();
        send(
            &mut app,
            SessionEvent::LootStartRoll(start(0xAA, 0, ROLLED)),
        );
        assert!(seen(&mut app).is_empty());
        assert!(sent(&rx)
            .iter()
            .any(|c| matches!(c, ClientCommand::ItemQuery { entry, .. } if *entry == ROLLED)));
        answer(
            &mut app,
            ROLLED,
            Some(crate::items::test_template("Sulfuras")),
        );
        assert_eq!(seen(&mut app), ["0:60000:Sulfuras"]);

        send(&mut app, SessionEvent::LootStartRoll(start(0xAA, 1, 4242)));
        answer(&mut app, 4242, None);
        assert!(seen(&mut app).is_empty(), "denied: no frame");
        assert!(app
            .world()
            .resource::<LootRolls>()
            .rolls
            .iter()
            .all(|r| r.item_id != 4242));
    }

    /// `0x6fb2c0`/`0x6fb180`: four frames at most; a fifth waits for one to close, here our vote,
    /// which closes its frame at once and finishes its roll (`0x6fbc70`).
    #[test]
    fn a_fifth_roll_waits_for_a_free_frame() {
        let (mut app, rx) = roll_app();
        cache(&mut app);
        for slot in 0..5 {
            send(
                &mut app,
                SessionEvent::LootStartRoll(start(0xAA, slot, ROLLED)),
            );
        }
        assert_eq!(seen(&mut app).len(), 4);
        sent(&rx);
        vote(&mut app, "RollOnLoot(1, 0)");
        assert!(matches!(
            sent(&rx)[..],
            [ClientCommand::LootRoll {
                item_slot: 1,
                roll_type: 0,
                ..
            }]
        ));
        assert_eq!(seen(&mut app), ["cancel 1", "4:60000:Sulfuras"]);
        let still = crate::ui_script::live::with_live_world(app.world_mut(), |s| {
            s.eval::<String>("return (select(2, GetLootRollItemInfo(1)))")
        })
        .expect("a VM")
        .unwrap();
        assert_eq!(still, "Sulfuras", "a finished roll stays listed");
        vote(&mut app, "RollOnLoot(1, 0)");
        assert!(sent(&rx).is_empty(), "a finished roll takes no vote");
    }

    /// A won roll closes once its line is composed, after its record and the winner's name
    /// (`0x6fbb10`); a second win finds no roll and closes nothing; our finished roll closes with
    /// no second cancel (`0x6fb953`).
    #[test]
    fn a_win_closes_its_roll_once_its_line_is_composed() {
        let (mut app, _rx) = roll_app();
        cache(&mut app);
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(1, "Ragnaros".into(), None);
        send(
            &mut app,
            SessionEvent::LootStartRoll(start(0xAA, 0, ROLLED)),
        );
        send(
            &mut app,
            SessionEvent::LootStartRoll(start(0xAA, 1, ROLLED)),
        );
        seen(&mut app);
        let mut w = won(84, roll_vote::NEED);
        w.item_id = ROLLED;
        send(&mut app, SessionEvent::LootRollWon(w));
        assert_eq!(seen(&mut app), ["cancel 0"]);
        send(&mut app, SessionEvent::LootRollWon(w));
        assert!(seen(&mut app).is_empty(), "the roll is gone");
        assert_eq!(
            app.world().resource::<ChatLog>().pending_lines().len(),
            2,
            "both lines print"
        );

        vote(&mut app, "RollOnLoot(1, 0)");
        assert_eq!(seen(&mut app), ["cancel 1"]);
        w.item_slot = 1;
        send(&mut app, SessionEvent::LootRollWon(w));
        assert!(seen(&mut app).is_empty(), "finished: no second cancel");
    }

    /// A line waits on its roller's name (`0x6fae40`): an answer composes it, a denial drops it.
    #[test]
    fn a_line_waits_on_its_rollers_name() {
        let (mut app, rx) = roll_app();
        cache(&mut app);
        let mut a = announce(0, roll_vote::NEED, false);
        a.item_id = ROLLED;
        a.roller = 0x77;
        send(&mut app, SessionEvent::LootRoll(a));
        assert!(sent(&rx)
            .iter()
            .any(|c| matches!(c, ClientCommand::NameQuery { guid: 0x77 })));
        assert!(app.world().resource::<ChatLog>().pending_lines().is_empty());
        send(
            &mut app,
            SessionEvent::PlayerName {
                guid: 0x77,
                name: "Thrall".into(),
                realm: String::new(),
                race: 2,
                class: 7,
                gender: 0,
                declensions: None,
            },
        );
        assert_eq!(app.world().resource::<ChatLog>().pending_lines().len(), 1);

        a.roller = 0x78;
        send(&mut app, SessionEvent::LootRoll(a));
        send(
            &mut app,
            SessionEvent::PlayerNameMiss {
                guid: 0x78,
                miss: benilla_protocol::messages::NameQueryMiss::Deny,
            },
        );
        assert_eq!(app.world().resource::<ChatLog>().pending_lines().len(), 1);
    }

    /// A retry tells no callback (`0x6358b3`): a won line keeps waiting on the name and its roll
    /// closes once the name lands.
    #[test]
    fn a_name_retry_keeps_the_line_waiting() {
        let (mut app, _rx) = roll_app();
        cache(&mut app);
        send(
            &mut app,
            SessionEvent::LootStartRoll(start(0xAA, 0, ROLLED)),
        );
        seen(&mut app);
        let mut w = won(84, roll_vote::NEED);
        w.item_id = ROLLED;
        w.winner = 0x79;
        send(&mut app, SessionEvent::LootRollWon(w));
        send(
            &mut app,
            SessionEvent::PlayerNameMiss {
                guid: 0x79,
                miss: benilla_protocol::messages::NameQueryMiss::Retry,
            },
        );
        assert!(seen(&mut app).is_empty(), "still waiting on the name");
        assert_eq!(app.world().resource::<LootRolls>().rolls.len(), 1);
        send(
            &mut app,
            SessionEvent::PlayerName {
                guid: 0x79,
                name: "Jaina".into(),
                realm: String::new(),
                race: 1,
                class: 8,
                gender: 1,
                declensions: None,
            },
        );
        assert_eq!(seen(&mut app), ["cancel 0"]);
        assert_eq!(app.world().resource::<ChatLog>().pending_lines().len(), 1);
    }

    /// The line's name read asks again for a guid denied before (`0x67d770`), rather than waiting
    /// on an answer that never comes.
    #[test]
    fn a_line_asks_again_for_a_name_denied_before() {
        let (mut app, rx) = roll_app();
        cache(&mut app);
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(0x7A, String::new(), None);
        let mut a = announce(0, roll_vote::NEED, false);
        a.item_id = ROLLED;
        a.roller = 0x7A;
        send(&mut app, SessionEvent::LootRoll(a));
        assert!(sent(&rx)
            .iter()
            .any(|c| matches!(c, ClientCommand::NameQuery { guid: 0x7A })));
        send(
            &mut app,
            SessionEvent::PlayerNameMiss {
                guid: 0x7A,
                miss: benilla_protocol::messages::NameQueryMiss::Placeholder,
            },
        );
        assert_eq!(app.world().resource::<ChatLog>().pending_lines().len(), 1);
    }

    /// `0x588d29`: the time left is the end less the clock in 32 bits, so an expired roll reads
    /// wrapped near 2^32.
    #[test]
    fn an_expired_roll_reads_its_time_left_wrapped() {
        let (mut app, _rx) = roll_app();
        cache(&mut app);
        send(
            &mut app,
            SessionEvent::LootStartRoll(start(0xAA, 0, ROLLED)),
        );
        let left = |app: &mut App| {
            crate::ui_script::live::with_live_world(app.world_mut(), |s| {
                s.eval::<f64>("return GetLootRollTimeLeft(0)")
            })
            .expect("a VM")
            .unwrap()
        };
        assert_eq!(left(&mut app), 60_000.0);
        let mut real = app.world_mut().resource_mut::<Time<bevy::time::Real>>();
        // The first update only anchors the clock; the second moves it.
        real.update_with_duration(std::time::Duration::ZERO);
        real.update_with_duration(std::time::Duration::from_millis(61_000));
        assert_eq!(left(&mut app), 4_294_967_296.0 - 1_000.0);
    }

    /// A roll passed by everyone while its record is in flight: the start's callback, asked first,
    /// shows it, then the line's closes it (`0x6fb1f0`, `0x6fb560`).
    #[test]
    fn a_roll_passed_while_its_record_is_in_flight_shows_then_closes() {
        let (mut app, _rx) = roll_app();
        send(
            &mut app,
            SessionEvent::LootStartRoll(start(0xAA, 0, ROLLED)),
        );
        send(
            &mut app,
            SessionEvent::LootAllPassed(LootAllPassed {
                looted_target: 0xAA,
                item_slot: 0,
                item_id: ROLLED,
                random_property_id: 0,
                random_suffix: 0,
            }),
        );
        assert!(seen(&mut app).is_empty());
        answer(
            &mut app,
            ROLLED,
            Some(crate::items::test_template("Sulfuras")),
        );
        assert_eq!(seen(&mut app), ["0:60000:Sulfuras", "cancel 0"]);
    }

    /// The world change closes every roll (`0x6fafd0`): each unfinished one passes on the wire,
    /// a shown one signals its cancel, a waiting one none.
    #[test]
    fn a_world_change_passes_and_closes_every_roll() {
        let (mut app, rx) = roll_app();
        cache(&mut app);
        for slot in 0..5 {
            send(
                &mut app,
                SessionEvent::LootStartRoll(start(0xAA, slot, ROLLED)),
            );
        }
        vote(&mut app, "RollOnLoot(0, 0)");
        seen(&mut app);
        sent(&rx);
        send(
            &mut app,
            SessionEvent::Worldport {
                map_id: 1,
                position: [0.0; 3],
                orientation: 0.0,
                needs_ack: true,
            },
        );
        let passes = sent(&rx)
            .into_iter()
            .filter(|c| matches!(c, ClientCommand::LootRoll { roll_type: 0, .. }))
            .count();
        assert_eq!(passes, 4, "the four unfinished rolls");
        assert_eq!(
            seen(&mut app),
            ["cancel 1", "cancel 2", "cancel 3", "cancel 4"]
        );
        assert!(app.world().resource::<LootRolls>().rolls.is_empty());
    }

    /// Disenchant on a rare item asks with `CONFIRM_DISENCHANT_ROLL (id, 3)` (`0x6fbc70`).
    #[test]
    fn a_rare_disenchant_raises_its_own_confirm() {
        let (mut app, _rx) = roll_app();
        let mut t = crate::items::test_template("Cleaver");
        t.quality = 3;
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(ROLLED, Some(t));
        send(
            &mut app,
            SessionEvent::LootStartRoll(start(0xAA, 0, ROLLED)),
        );
        app.world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>()
            .run(
                "CONFIRMS = {}\n\
                 local f = CreateFrame(\"Frame\")\n\
                 f:RegisterEvent(\"CONFIRM_DISENCHANT_ROLL\")\n\
                 f:SetScript(\"OnEvent\", function(self, event, id, kind)\n\
                   tinsert(CONFIRMS, event .. \":\" .. id .. \":\" .. kind)\n\
                 end)",
            )
            .unwrap();
        vote(&mut app, "RollOnLoot(0, 3)");
        let script = app
            .world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>();
        assert_eq!(
            script.eval::<String>("return CONFIRMS[1]").unwrap(),
            "CONFIRM_DISENCHANT_ROLL:0:3"
        );
    }
}
