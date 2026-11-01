//! The group loot roll feed: `SMSG_LOOT_START_ROLL`, `SMSG_LOOT_ROLL`, `SMSG_LOOT_ROLL_WON` and
//! `SMSG_LOOT_ALL_PASSED` become the stock `GroupLootFrame`'s events and the roll chat lines, and
//! its `RollOnLoot` votes go back out as `CMSG_LOOT_ROLL`.
//!
//! A roll for another map than ours is dropped at the start (`0x6fb2c0`). A fresh roll's
//! `START_LOOT_ROLL` waits for its item template, as in the reference: its roll start ends in the
//! item cache lookup, whose hit fires the event at once and whose miss fires it from the arrival
//! callback. Deviation: a negative answer, or none within [`TEMPLATE_HOLD_MAX_MS`], opens the frame
//! unresolved, because a frame that never opens cannot even be passed on; the reference's callback
//! fires on success only. At most [`MAX_SHOWN_ROLLS`] frames are up; a ready roll past that waits
//! and shows when one closes (`0x6fb180`).
//!
//! `rollID` is a client-local serial; the wire addresses a roll by `(lootedTarget, itemSlot)`. Our
//! own vote closes our frame at once (`0x6fbc70`). A won or all-passed roll makes its loot window
//! slot lootable again (`0x6dc770`).

use benilla_protocol::messages::{
    LootAllPassed, LootRoll, LootRollWon, LootStartRoll, RollAnnounce,
};
use bevy::prelude::*;

use benilla_ui::script::{LootRollEntry, LootRollsState, ScriptValue, UiScript};
use benilla_ui::strings::{fill, Arg};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfGuid, SelfPlayer};
use crate::ui_chat::{ChatEvent, ChatEventKind, ChatLog};
use crate::ui_script::{UiFeed, UiInput};

/// Frames a pending line retries its names before dropping: a negative-cached name never resolves.
const LINE_MAX_TRIES: u16 = 120;

/// How long a fresh roll waits for its item template before `START_LOOT_ROLL` fires anyway: wall
/// clock, not frames, as the wait comes out of the roll's own minute.
const TEMPLATE_HOLD_MAX_MS: u32 = 2_000;

/// The frames up at once (`0x6fb180`, `0x6fb2c0`: `s_numShownLootRolls < 4`).
const MAX_SHOWN_ROLLS: usize = 4;

/// `item_template.bonding` bind on pickup; only 1 gets `GroupLootFrame_OnShow`'s gold backdrop.
const BIND_WHEN_PICKED_UP: u32 = 1;

/// `Flags2` 0x100, an item only Greed may take: Need's reason 5 (`0x6fa7a0`).
const ITEM_FLAG2_NEED_ROLL_DISABLED: u32 = 0x100;

/// Where a roll stands on our screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shown {
    /// Waiting for its item template.
    Held,
    /// Ready, waiting for a free frame.
    Pending,
    /// `START_LOOT_ROLL` has gone out.
    Up,
}

/// One group loot roll open for us.
struct ActiveRoll {
    /// The id Lua addresses the roll by.
    roll_id: u32,
    looted_target: u64,
    item_slot: u32,
    item_id: u32,
    random_suffix: u32,
    /// The drop's random property, the hover's enchant source.
    random_property_id: u32,
    count: u32,
    vote_mask: u8,
    /// The wire's `countdownTime`, kept whole: `START_LOOT_ROLL`'s `arg2` and the bar's maximum.
    countdown_ms: u32,
    /// Milliseconds left, saturating at 0; the server's resolution closes the roll, not the tick.
    remaining_ms: u32,
    /// How long the roll has waited for its item template.
    held_ms: u32,
    shown: Shown,
}

/// One queued chat announcement, awaiting the names it needs.
struct PendingLine {
    line: RollLine,
    tries: u16,
}

/// Which announcement a [`PendingLine`] renders.
enum RollLine {
    Announce(LootRoll),
    Won(LootRollWon),
    AllPassed(LootAllPassed),
}

/// Every group loot roll open for us; cleared on disconnect.
#[derive(Resource, Default)]
pub(crate) struct LootRolls {
    active: Vec<ActiveRoll>,
    /// Never reused within a session, so a late packet for a closed roll cannot hit a fresh one.
    next_id: u32,
    pending: Vec<PendingLine>,
    /// Closed rolls, one `CANCEL_LOOT_ROLL` each.
    cancelled: Vec<u32>,
}

impl LootRolls {
    /// `SMSG_LOOT_START_ROLL`: list the roll under a fresh id, unless it is for another map
    /// (`0x6fb2c0`). A duplicate `(looted_target, item_slot)` is ignored: a server may re-send
    /// every active roll on reconnect.
    pub(crate) fn start(&mut self, p: LootStartRoll, current_map: Option<u32>) {
        if current_map.is_some_and(|m| m != p.map_id) {
            debug!(
                "ui_loot_roll: roll on map {} while on {current_map:?} — dropped",
                p.map_id
            );
            return;
        }
        if self
            .active
            .iter()
            .any(|r| r.looted_target == p.looted_target && r.item_slot == p.item_slot)
        {
            debug!(
                "ui_loot_roll: duplicate start for {:#x} slot {} — ignored",
                p.looted_target, p.item_slot
            );
            return;
        }
        self.next_id += 1;
        self.active.push(ActiveRoll {
            roll_id: self.next_id,
            looted_target: p.looted_target,
            item_slot: p.item_slot,
            item_id: p.item_id,
            random_suffix: p.random_suffix,
            random_property_id: p.random_property_id,
            count: p.count,
            vote_mask: p.vote_mask,
            countdown_ms: p.countdown_ms,
            remaining_ms: p.countdown_ms,
            held_ms: 0,
            shown: Shown::Held,
        });
    }

    /// `SMSG_LOOT_ROLL`: queue the chat line.
    pub(crate) fn announce(&mut self, p: LootRoll) {
        self.pending.push(PendingLine {
            line: RollLine::Announce(p),
            tries: 0,
        });
    }

    /// `SMSG_LOOT_ROLL_WON`: queue the line and close the frame.
    pub(crate) fn won(&mut self, p: LootRollWon) {
        self.pending.push(PendingLine {
            line: RollLine::Won(p),
            tries: 0,
        });
        self.close(p.looted_target, p.item_slot);
    }

    /// `SMSG_LOOT_ALL_PASSED`: queue the line and close the frame; the item stays lootable.
    pub(crate) fn all_passed(&mut self, p: LootAllPassed) {
        self.pending.push(PendingLine {
            line: RollLine::AllPassed(p),
            tries: 0,
        });
        self.close(p.looted_target, p.item_slot);
    }

    /// Drop the roll if we still hold it unvoted and queue its `CANCEL_LOOT_ROLL`, shown or not:
    /// the reference's won and all-passed handlers test only the node's finished byte (`0x6fb700`,
    /// `0x6fb470`). Its frame count drops for any unfinished node; here only for a shown one, so a
    /// held roll that closes cannot free a frame it never took.
    fn close(&mut self, looted_target: u64, item_slot: u32) {
        if let Some(i) = self
            .active
            .iter()
            .position(|r| r.looted_target == looted_target && r.item_slot == item_slot)
        {
            let r = self.active.remove(i);
            self.cancelled.push(r.roll_id);
        }
    }

    /// Our own vote: close the frame and return the roll's wire identity; `None` for a stale click.
    fn vote(&mut self, roll_id: u32) -> Option<(u64, u32)> {
        let i = self.active.iter().position(|r| r.roll_id == roll_id)?;
        let r = self.active.remove(i);
        self.cancelled.push(r.roll_id);
        Some((r.looted_target, r.item_slot))
    }

    /// Tick every open roll's bar down, and the template hold of every held roll up.
    fn tick(&mut self, delta_ms: u32) {
        for r in &mut self.active {
            r.remaining_ms = r.remaining_ms.saturating_sub(delta_ms);
            if r.shown == Shown::Held {
                r.held_ms = r.held_ms.saturating_add(delta_ms);
            }
        }
    }

    /// The frames up now.
    fn shown_count(&self) -> usize {
        self.active.iter().filter(|r| r.shown == Shown::Up).count()
    }

    /// Session teardown: no `CANCEL_LOOT_ROLL`, as the UI goes down with the session.
    pub(crate) fn clear(&mut self) {
        self.active.clear();
        self.pending.clear();
        self.cancelled.clear();
    }
}

/// The group rolls' packet handlers.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::LootRolls;
    use crate::net::NetHandlerApp;

    /// Register the roll handlers, called from [`super::UiLootRollPlugin`].
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::LootStartRoll, on_packet)
            .net_handler(K::LootRoll, on_packet)
            .net_handler(K::LootRollWon, on_packet)
            .net_handler(K::LootAllPassed, on_packet)
            .net_handler(K::Disconnected, on_session_end);
    }

    fn on_packet(
        In(ev): In<SessionEvent>,
        mut rolls: ResMut<LootRolls>,
        mut loot: ResMut<crate::ui_loot::LootState>,
        map: Option<Res<benilla_world::world_map::CurrentMap>>,
    ) {
        match ev {
            SessionEvent::LootStartRoll(p) => {
                debug!(
                    "net: loot roll opened on item {} ({:#x} slot {}, map {}), {} ms, mask {:#x}",
                    p.item_id, p.looted_target, p.item_slot, p.map_id, p.countdown_ms, p.vote_mask
                );
                rolls.start(p, map.map(|m| m.0));
            }
            SessionEvent::LootRoll(p) => {
                debug!(
                    "net: loot roll announce — roller {:#x} number {} type {} auto {}",
                    p.roller, p.roll_number, p.roll_type, p.auto_pass
                );
                rolls.announce(p);
            }
            // Both resolutions make the slot lootable in an open window on that object
            // (`0x6fbc4d`, `0x6fb6e2` -> `0x6dc770`).
            SessionEvent::LootRollWon(p) => {
                debug!(
                    "net: loot roll won by {:#x} with {} (type {})",
                    p.winner, p.roll_number, p.roll_type
                );
                loot.clear_slot_type(p.looted_target, p.item_slot);
                rolls.won(p);
            }
            SessionEvent::LootAllPassed(p) => {
                debug!("net: loot roll — everyone passed on item {}", p.item_id);
                loot.clear_slot_type(p.looted_target, p.item_slot);
                rolls.all_passed(p);
            }
            _ => {}
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
        app.init_resource::<LootRolls>().add_systems(
            Update,
            (
                // The feed before the input pass shows a new roll the same frame; the drain after
                // it sends a vote the same frame.
                feed_loot_rolls.in_set(UiFeed),
                drain_loot_rolls.after(UiInput),
            ),
        );
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

/// The inputs a line waits for: its item template and, but for all-passed, its roller's name.
/// `None` while one is in flight.
fn render(
    line: &RollLine,
    self_guid: Option<u64>,
    items: &Items,
    names: &NameCache,
    commands: &NetCommands,
    catalogs: crate::items::RollCatalogs,
    spam: bool,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<Option<String>> {
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
    let t = items.template(item, 0, commands)?;
    let link = crate::ui_items::item_link_full(
        item,
        0,
        property,
        suffix,
        &catalogs.name(&t.name, property),
        t.quality,
    );
    // The reference asks the name cache for the roller on every line but all-passed.
    let name = match roller {
        Some(g) => Some(names.resolve(g, commands)?.to_string()),
        None => None,
    };
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
    Some(format_line(line, &inputs, get))
}

/// Post the queued announcements as `CHAT_MSG_LOOT` lines, as the reference does.
#[allow(clippy::too_many_arguments)]
fn drain_lines(
    rolls: &mut LootRolls,
    self_guid: Option<u64>,
    items: &Items,
    names: &NameCache,
    commands: &NetCommands,
    chat: &mut ChatLog,
    catalogs: crate::items::RollCatalogs,
    spam: bool,
    get: &dyn Fn(&str) -> Option<String>,
) {
    let pending = std::mem::take(&mut rolls.pending);
    let mut still = Vec::new();
    for mut p in pending {
        match render(
            &p.line, self_guid, items, names, commands, catalogs, spam, get,
        ) {
            Some(Some(text)) => chat.push_event(ChatEvent::text_only(ChatEventKind::Loot, text)),
            Some(None) => {}
            None => {
                p.tries += 1;
                if p.tries < LINE_MAX_TRIES {
                    still.push(p);
                }
            }
        }
    }
    rolls.pending = still;
}

/// `0x6fa7a0`: why Need, Greed and Disenchant are refused, 0 when the vote mask allows them. Need
/// is 2 when the drop would pass the unique count (worn, carried and banked), else 5 for a
/// greed-only item, else 1; Greed is 2; Disenchant is 4 for a disenchantable item, else 3. Not
/// applied: the item limit category leg of the count check, whose field is not decoded.
fn reasons(
    vote_mask: u8,
    count: u32,
    template: &benilla_protocol::messages::ItemInfo,
    held: u32,
) -> [u8; 3] {
    let need = if vote_mask & 0x2 != 0 {
        0
    } else if template.max_count != 0 && held.saturating_add(count) > template.max_count {
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

/// The Lua-facing snapshot; its template fields are `None` or `false` until the template answers.
fn snapshot(
    rolls: &LootRolls,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
    catalogs: crate::items::RollCatalogs,
    held: &dyn Fn(u32) -> u32,
) -> LootRollsState {
    let entries = rolls
        .active
        .iter()
        .map(|r| {
            let t = items.template(r.item_id, 0, commands);
            let texture = t
                .and_then(|t| icons?.catalog.get(t.display_info_id))
                .and_then(|d| d.icon.clone());
            LootRollEntry {
                roll_id: r.roll_id,
                name: t.map(|t| catalogs.name(&t.name, r.random_property_id)),
                texture,
                quantity: r.count,
                quality: t.map(|t| t.quality),
                bind_on_pickup: t.is_some_and(|t| t.bonding == BIND_WHEN_PICKED_UP),
                vote_mask: r.vote_mask,
                reasons: t.map_or([0; 3], |t| {
                    reasons(r.vote_mask, r.count, t, held(r.item_id))
                }),
                disenchant_skill: t.map_or(-1, |t| t.required_disenchant_skill),
                time_left_ms: r.remaining_ms,
                item_id: r.item_id,
                random_property_id: r.random_property_id,
                link: t.map(|t| {
                    crate::ui_items::item_link_full(
                        r.item_id,
                        0,
                        r.random_property_id,
                        r.random_suffix,
                        &catalogs.name(&t.name, r.random_property_id),
                        t.quality,
                    )
                }),
            }
        })
        .collect();
    LootRollsState { rolls: entries }
}

/// Tick the open rolls, push them into the VM, fire their events and post their chat lines.
#[allow(clippy::too_many_arguments)]
fn feed_loot_rolls(
    script: Option<NonSendMut<UiScript>>,
    mut rolls: ResMut<LootRolls>,
    items: Res<Items>,
    names: Res<NameCache>,
    icons: Option<Res<ItemDisplays>>,
    commands: Res<NetCommands>,
    self_guid: Res<SelfGuid>,
    mut chat: ResMut<ChatLog>,
    time: Res<Time>,
    mut last: Local<crate::ui_script::VmMemo<LootRollsState>>,
    catalogs: (
        Option<Res<crate::items::RandomProperties>>,
        Option<Res<crate::items::Enchants>>,
    ),
    // `showLootSpam`, read as each line is composed, as the reference's composers read it.
    loot: Res<crate::ui_loot::LootConfig>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    objects: crate::net::Objects,
) {
    rolls.tick(time.delta().as_millis() as u32);

    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let catalogs = crate::items::RollCatalogs {
        props: catalogs.0.as_deref(),
        enchants: catalogs.1.as_deref(),
    };
    {
        let lua = script.lua();
        let get = |key: &str| benilla_ui::strings::global(lua, key);
        drain_lines(
            &mut rolls,
            self_guid.0,
            &items,
            &names,
            &commands,
            &mut chat,
            catalogs,
            loot.show_loot_spam,
            &get,
        );
    }

    // The snapshot goes in before the events: a `GroupLootFrame`'s `OnShow` reads its item from it.
    let self_store = self_q.single().ok();
    let held = |entry: u32| {
        self_store.map_or(0, |s| {
            crate::ui_items::count_of(
                &s.0,
                &objects,
                entry,
                crate::ui_items::InventoryScope::QUEST_ITEMS,
            )
        })
    };
    let fresh = snapshot(&rolls, &items, icons.as_deref(), &commands, catalogs, &held);
    if fresh != *last {
        script.set_loot_rolls(fresh.clone());
        *last = fresh;
    }

    // Closed frames first, so a waiting roll takes the frame one freed (`0x6fb180`).
    for roll_id in std::mem::take(&mut rolls.cancelled) {
        debug!("ui_loot_roll: roll {roll_id} cancelled");
        script.fire_event("CANCEL_LOOT_ROLL", vec![ScriptValue::Int(roll_id as i64)]);
    }
    // A held roll is ready once its template is in the snapshot above; ready rolls show in list
    // order while a frame is free, waiting ones before fresh ones.
    for r in &mut rolls.active {
        if r.shown != Shown::Held {
            continue;
        }
        if items.template(r.item_id, 0, &commands).is_none() {
            // The module doc's deviation: an entry the server does not know, or an expired hold.
            let unknown = items.template_answered_unknown(r.item_id);
            if !unknown && r.held_ms < TEMPLATE_HOLD_MAX_MS {
                continue;
            }
            debug!(
                "ui_loot_roll: roll {} opening unresolved ({})",
                r.roll_id,
                if unknown {
                    "the server does not know the entry"
                } else {
                    "template hold expired"
                }
            );
        }
        r.shown = Shown::Pending;
    }
    let mut free = MAX_SHOWN_ROLLS.saturating_sub(rolls.shown_count());
    for r in &mut rolls.active {
        if free == 0 {
            break;
        }
        if r.shown != Shown::Pending {
            continue;
        }
        r.shown = Shown::Up;
        free -= 1;
        debug!(
            "ui_loot_roll: roll {} opened ({} ms, held {} ms)",
            r.roll_id, r.countdown_ms, r.held_ms
        );
        script.fire_event(
            "START_LOOT_ROLL",
            vec![
                ScriptValue::Int(r.roll_id as i64),
                ScriptValue::Int(r.countdown_ms as i64),
            ],
        );
    }
}

/// Drain the Lua votes: each queued vote is one `CMSG_LOOT_ROLL` and closes our frame; each
/// confirm raises its popup, `CONFIRM_DISENCHANT_ROLL (id, 3)` for Disenchant, else
/// `CONFIRM_LOOT_ROLL (id, rollType)` (`0x6fbc70`).
fn drain_loot_rolls(
    script: Option<NonSendMut<UiScript>>,
    mut rolls: ResMut<LootRolls>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    for (roll_id, roll_type) in script.take_loot_roll_confirms() {
        debug!("ui_loot_roll: confirm for {roll_type} on roll {roll_id}");
        let event = if roll_type == benilla_protocol::messages::roll_vote::DISENCHANT {
            "CONFIRM_DISENCHANT_ROLL"
        } else {
            "CONFIRM_LOOT_ROLL"
        };
        script.fire_event(
            event,
            vec![
                ScriptValue::Int(roll_id as i64),
                ScriptValue::Int(roll_type as i64),
            ],
        );
    }
    for (roll_id, roll_type) in script.take_loot_roll_votes() {
        match rolls.vote(roll_id) {
            Some((looted_target, item_slot)) => {
                debug!("ui_loot_roll: vote {roll_type} on roll {roll_id}");
                let _ = commands.0.send(ClientCommand::LootRoll {
                    looted_target,
                    item_slot,
                    roll_type,
                });
            }
            None => debug!("ui_loot_roll: RollOnLoot({roll_id}) — no such roll, ignored"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::roll_vote;

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
        assert_eq!(reasons(0x0F, 1, &t, 0), [0, 0, 0]);
        assert_eq!(
            reasons(0x01, 1, &t, 0),
            [1, 2, 3],
            "plain item, not disenchantable"
        );
        t.required_disenchant_skill = 25;
        t.flags2 = ITEM_FLAG2_NEED_ROLL_DISABLED;
        assert_eq!(reasons(0x05, 1, &t, 0), [5, 0, 4]);
        t.max_count = 1;
        assert_eq!(reasons(0x05, 1, &t, 1), [2, 0, 4], "one already held");
    }

    #[test]
    fn a_roll_for_another_map_is_dropped() {
        let mut r = LootRolls::default();
        let mut p = start(0xAA, 0, 17182);
        p.map_id = 574;
        r.start(p, Some(571));
        assert!(r.active.is_empty());
        r.start(p, Some(574));
        assert_eq!(r.active.len(), 1);
        r.start(start(0xBB, 0, 1), None);
        assert_eq!(r.active.len(), 2, "no map yet admits it");
    }

    #[test]
    fn duplicate_start_is_ignored() {
        let mut r = LootRolls::default();
        r.start(start(0xAA, 0, 17182), Some(0));
        r.start(start(0xAA, 0, 17182), Some(0));
        assert_eq!(r.active.len(), 1);
        assert_eq!(r.next_id, 1, "and the duplicate burns no id");
    }

    #[test]
    fn resolution_closes_the_matching_roll_only_and_once() {
        let mut r = LootRolls::default();
        r.start(start(0xAA, 0, 17182), Some(0));
        r.start(start(0xAA, 1, 4306), Some(0));
        r.won(won(84, roll_vote::NEED));
        r.won(won(84, roll_vote::NEED));
        assert_eq!(r.cancelled, vec![1], "only the first close cancels");
        assert_eq!(r.active.len(), 1);
        assert_eq!(r.active[0].item_slot, 1);
        assert_eq!(r.pending.len(), 2, "both lines still print");
    }

    #[test]
    fn our_vote_closes_the_frame_and_yields_the_wire_identity() {
        let mut r = LootRolls::default();
        r.start(start(0xAA, 3, 17182), Some(0));
        assert_eq!(r.vote(1), Some((0xAA, 3)));
        assert!(r.active.is_empty());
        assert_eq!(r.cancelled, vec![1]);
        assert_eq!(r.vote(1), None);
    }

    #[test]
    fn tick_saturates_and_ages_only_the_hold() {
        let mut r = LootRolls::default();
        r.start(start(0xAA, 0, 17182), Some(0));
        r.tick(500);
        assert_eq!(r.active[0].held_ms, 500);
        r.active[0].shown = Shown::Up;
        r.tick(999_999);
        assert_eq!(r.active[0].held_ms, 500, "the hold is over");
        assert_eq!(r.active[0].remaining_ms, 0);
        assert_eq!(r.active.len(), 1, "the server closes it, not the tick");
    }

    // ── Driven through a real schedule: the snapshot memo is a `Local`. ──

    const ROLLED: u32 = 17182;

    /// A `feed_loot_rolls` app whose listener records `rollID:rollTime:name` per
    /// `START_LOOT_ROLL`, the call stock `GroupLootFrame_OnShow` makes.
    fn feed_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<LootRolls>()
            .init_resource::<Items>()
            .init_resource::<NameCache>()
            .init_resource::<SelfGuid>()
            .init_resource::<ChatLog>()
            .init_resource::<crate::ui_loot::LootConfig>()
            .init_resource::<crate::net::GuidIndex>()
            // `Time::default()` has a zero delta, so the hold only ages where a test says it does.
            .init_resource::<Time>()
            .insert_resource(NetCommands(tx))
            .add_systems(Update, (feed_loot_rolls, drain_loot_rolls).chain());
        let script = UiScript::new().unwrap();
        script
            .run(
                "STARTS = {}\n\
                 local f = CreateFrame(\"Frame\")\n\
                 f:RegisterEvent(\"START_LOOT_ROLL\")\n\
                 f:SetScript(\"OnEvent\", function(self, event, arg1, arg2)\n\
                   local _, name = GetLootRollItemInfo(arg1)\n\
                   tinsert(STARTS, arg1 .. \":\" .. arg2 .. \":\" .. (name or \"<blank>\"))\n\
                 end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        (app, rx)
    }

    fn starts(app: &mut App) -> Vec<String> {
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        let n = script.eval::<i64>("return getn(STARTS)").unwrap();
        (1..=n)
            .map(|i| {
                script
                    .eval::<String>(&format!("return STARTS[{i}]"))
                    .unwrap()
            })
            .collect()
    }

    fn advance(app: &mut App, ms: u64) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(ms));
    }

    fn cache(app: &mut App) {
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(ROLLED, Some(crate::items::test_template("Sulfuras")));
    }

    /// A miss waits for the template and fires once on its arrival; a negative answer releases
    /// the roll unresolved, the module doc's deviation.
    #[test]
    fn the_roll_waits_for_its_item_template() {
        for (answer, painted) in [
            (Some(crate::items::test_template("Sulfuras")), "Sulfuras"),
            (None, "<blank>"),
        ] {
            let (mut app, rx) = feed_app();
            app.world_mut()
                .resource_mut::<LootRolls>()
                .start(start(0xAA, 0, ROLLED), Some(0));
            app.update();
            assert!(starts(&mut app).is_empty());
            assert!(rx
                .try_iter()
                .any(|c| matches!(c, ClientCommand::ItemQuery { entry, .. } if entry == ROLLED)));
            app.world_mut()
                .resource_mut::<Items>()
                .insert_template(ROLLED, answer.clone());
            app.update();
            assert_eq!(starts(&mut app), vec![format!("1:60000:{painted}")]);
            app.update();
            assert_eq!(starts(&mut app).len(), 1, "and only once");
        }
    }

    #[test]
    fn an_unanswered_template_opens_the_roll_at_the_deadline() {
        let (mut app, _rx) = feed_app();
        app.world_mut()
            .resource_mut::<LootRolls>()
            .start(start(0xAA, 0, ROLLED), Some(0));
        advance(&mut app, u64::from(TEMPLATE_HOLD_MAX_MS) - 1);
        app.update();
        assert!(starts(&mut app).is_empty());
        advance(&mut app, 2);
        app.update();
        assert_eq!(starts(&mut app), vec!["1:60000:<blank>".to_string()]);
    }

    /// `0x6fb2c0`/`0x6fb180`: four frames at most; a fifth waits for one to close, here our vote.
    #[test]
    fn a_fifth_roll_waits_for_a_free_frame() {
        let (mut app, rx) = feed_app();
        cache(&mut app);
        for slot in 0..5 {
            app.world_mut()
                .resource_mut::<LootRolls>()
                .start(start(0xAA, slot, ROLLED), Some(0));
        }
        app.update();
        assert_eq!(starts(&mut app).len(), 4);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("RollOnLoot(2, 0)")
            .unwrap();
        app.update(); // the drain sends the pass and closes roll 2
        assert!(rx.try_iter().any(|c| matches!(
            c,
            ClientCommand::LootRoll {
                item_slot: 1,
                roll_type: 0,
                ..
            }
        )));
        app.update(); // the feed frees its frame for roll 5
        assert_eq!(
            starts(&mut app).last().map(String::as_str),
            Some("5:60000:Sulfuras")
        );
        assert_eq!(starts(&mut app).len(), 5);
    }

    /// A roll that closed while held never opens, though its `CANCEL_LOOT_ROLL` goes out.
    #[test]
    fn a_roll_resolved_while_held_never_announces() {
        let (mut app, _rx) = feed_app();
        app.world_mut()
            .resource_mut::<LootRolls>()
            .start(start(0xAA, 0, ROLLED), Some(0));
        app.update();
        app.world_mut()
            .resource_mut::<LootRolls>()
            .all_passed(LootAllPassed {
                looted_target: 0xAA,
                item_slot: 0,
                item_id: ROLLED,
                random_property_id: 0,
                random_suffix: 0,
            });
        cache(&mut app);
        app.update();
        app.update();
        assert!(starts(&mut app).is_empty());
    }

    /// Disenchant on a rare item asks with `CONFIRM_DISENCHANT_ROLL (id, 3)` (`0x6fbc70`).
    #[test]
    fn a_rare_disenchant_raises_its_own_confirm() {
        let (mut app, _rx) = feed_app();
        let mut t = crate::items::test_template("Cleaver");
        t.quality = 3;
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(ROLLED, Some(t));
        app.world_mut()
            .resource_mut::<LootRolls>()
            .start(start(0xAA, 0, ROLLED), Some(0));
        app.update();
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        script
            .run(
                "CONFIRMS = {}\n\
                 local f = CreateFrame(\"Frame\")\n\
                 f:RegisterEvent(\"CONFIRM_DISENCHANT_ROLL\")\n\
                 f:SetScript(\"OnEvent\", function(self, event, id, kind)\n\
                   tinsert(CONFIRMS, event .. \":\" .. id .. \":\" .. kind)\n\
                 end)\n\
                 RollOnLoot(1, 3)",
            )
            .unwrap();
        app.update();
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        assert_eq!(
            script.eval::<String>("return CONFIRMS[1]").unwrap(),
            "CONFIRM_DISENCHANT_ROLL:1:3"
        );
    }
}
