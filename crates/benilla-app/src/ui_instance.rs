//! The instance packets the 12340 client handles itself: the lockout lines it composes from
//! `GlobalStrings.lua` and hands to chat as `CHAT_MSG_SYSTEM`, the raid welcome it signals as
//! `RAID_INSTANCE_WELCOME`, the instance lock clock behind the `INSTANCE_LOCK` dialog, the
//! encounter's boss slots behind `bossN`, and the bookkeeping behind `CanShowResetInstances()`.
//! The net handlers queue; [`feed_instance`], which has the VM, resolves and fires.
//!
//! Deviation: walking out of a resettable instance ourselves also counts as holding a bind
//! ([`InstanceState::saw_own_dungeon`]), because AzerothCore never sends
//! `SMSG_UPDATE_INSTANCE_OWNERSHIP`, so the server alone would never offer the reset.

use std::time::{Duration, Instant};

use benilla_protocol::messages::{
    EncounterUnitUpdate, InstanceLockWarning, InstanceResetFailure, RaidInstanceMessage,
    RaidInstanceWarning,
};
use benilla_ui::script::{InstanceLock, ScriptValue, UiScript};
use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands};
use crate::ui_chat::{ChatEvent, ChatEventKind, ChatLog};
use crate::ui_script::{UiFeed, UiInput, WorldLeaveSweepMessage};

/// `CanShowResetInstances()`'s window, 25 hours: `cmp eax, 0x15f90` against `time(0)` minus when we
/// left, signed, so the boundary is inclusive (`0x51959e`).
const RESET_OFFER_WINDOW_SECS: u64 = 90_000;

/// The encounter's boss slots, the fixed array `0x5ed7b0` sizes on the first engage.
pub(crate) const BOSS_SLOTS: usize = 16;

/// `bossN` for every slot, in slot order.
pub(crate) const BOSS_TOKENS: [&str; BOSS_SLOTS] = [
    "boss1", "boss2", "boss3", "boss4", "boss5", "boss6", "boss7", "boss8", "boss9", "boss10",
    "boss11", "boss12", "boss13", "boss14", "boss15", "boss16",
];

/// The lockout bookkeeping (`[0xbd0884]`-`[0xbd0890]`) and what the packets owe the VM.
/// Session-scoped, like the reference's zero-initialized globals.
#[derive(Resource, Default)]
pub(crate) struct InstanceState {
    /// "do you hold any bind", `SMSG_UPDATE_INSTANCE_OWNERSHIP` (`0x513b0b`).
    owns_saved: bool,
    /// benilla's half of the same term: we watched the player walk out of a resettable instance.
    /// Raised by [`LatchWriter::WorldEntry`], cleared by a reset and at logout.
    saw_own_dungeon: bool,
    /// The `Map.dbc` id of the last resettable instance left (`[0xbd0888]`).
    last_instance: Option<u32>,
    /// When [`Self::last_instance`] was recorded, in seconds (`[0xbd0884]`). Deviation: the app's
    /// monotonic clock, not `time(0)`, because it cannot run backwards; they differ only across a
    /// suspend.
    last_instance_at: u64,
    /// `SMSG_UPDATE_LAST_INSTANCE` map ids for [`track_instance_state`]: the packet tests against
    /// the current map, which [`benilla_world::world_map::CurrentMap`] reaches only after the
    /// drain's deferred insert.
    pending_last_instance: Vec<u32>,
    /// Lines for [`feed_instance`] to resolve against the VM's `GlobalStrings.lua`.
    lines: Vec<LockoutLine>,
    /// `RAID_INSTANCE_WELCOME`s to fire: the map, its difficulty, seconds, locked, extended.
    welcomes: Vec<(u32, u32, i32, bool, bool)>,
    /// The instance lock clock (`[0xbd0874]`-`[0xbd087c]`).
    lock: LockClock,
    /// `INSTANCE_LOCK_START`/`_STOP` owed, in order.
    lock_events: Vec<&'static str>,
}

/// How a line names its map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineMap {
    /// No `%s` (`INSTANCE_SAVED`, `RESET_FAILED_NOTIFY`).
    None,
    /// The `Map.dbc` name, else the id in decimal (`0x50cda0`).
    Name(u32),
    /// The name with its `MapDifficulty.dbc` difficulty (`0x4fe070`), "" with no map row.
    WithDifficulty(u32, u32),
}

/// One lockout line as the reference holds it between the template lookup and its formatting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockoutLine {
    /// The `GlobalStrings.lua` token, looked up with no ordinal (`push -1`).
    token: &'static str,
    /// The map that fills `%s`.
    map: LineMap,
    /// The `%d` fills, after the name.
    numbers: Vec<i64>,
    /// The `"(Debug-Only Lock Notice) %s"` wrapper, `SMSG_INSTANCE_SAVE_CREATED`'s flag-1 arm.
    debug_notice: bool,
}

impl LockoutLine {
    fn plain(token: &'static str, map: LineMap) -> Self {
        Self {
            token,
            map,
            numbers: Vec::new(),
            debug_notice: false,
        }
    }
}

/// The instance lock clock (`0x513b20`): a deadline, the encounters done, and whether it extends.
#[derive(Debug, Default)]
struct LockClock {
    deadline: Option<Instant>,
    completed_mask: u32,
    extending: bool,
}

impl LockClock {
    /// `0x513b20`: a positive `ms` (as `i32`) arms and signals `INSTANCE_LOCK_START`, any other
    /// clears and signals `INSTANCE_LOCK_STOP`.
    fn set(&mut self, warning: InstanceLockWarning, now: Instant) -> &'static str {
        if warning.ms as i32 <= 0 {
            *self = Self::default();
            "INSTANCE_LOCK_STOP"
        } else {
            *self = Self {
                deadline: Some(now + Duration::from_millis(u64::from(warning.ms))),
                completed_mask: warning.completed_mask,
                extending: warning.extending,
            };
            "INSTANCE_LOCK_START"
        }
    }

    /// `0x513b90`: the clock as the verbs read it, all zero once the deadline has passed.
    fn read(&self, now: Instant) -> InstanceLock {
        match self.deadline.filter(|&d| d > now) {
            Some(d) => InstanceLock {
                running: true,
                secs: u32::try_from((d - now).as_millis() / 1000).unwrap_or(u32::MAX),
                completed_mask: self.completed_mask,
                extending: self.extending,
            },
            None => InstanceLock::default(),
        }
    }
}

/// Which of the last-instance latch's two writers is speaking. The reference does not care;
/// benilla does, as only the world entry is first-hand evidence of a bind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LatchWriter {
    /// The map-load path: we just walked out of this map.
    WorldEntry,
    /// `SMSG_UPDATE_LAST_INSTANCE`: the server's word.
    Packet,
}

/// The two map facts the latch and the reset gate read, at the current dungeon difficulty.
pub(crate) struct MapFacts<'a> {
    /// The map has a `Map.dbc` row.
    pub(crate) known: &'a dyn Fn(u32) -> bool,
    /// A dungeon or raid (`InstanceType` 1 or 2) that resets on leaving: the difficulty is past 3
    /// or its `MapDifficulty.dbc` row has no `RaidDuration` (`0x634a80`).
    pub(crate) resettable: &'a dyn Fn(u32) -> bool,
}

impl InstanceState {
    /// `SMSG_UPDATE_INSTANCE_OWNERSHIP`: one store, no line.
    pub(crate) fn set_ownership(&mut self, owns: bool) {
        self.owns_saved = owns;
    }

    /// The latch (`0x5195c0`): record `map` as the instance just left when it is resettable and
    /// not the map we stand on, tested first (`cmp esi,edi; je`).
    pub(crate) fn note_last_instance(
        &mut self,
        map: u32,
        current_map: Option<u32>,
        resettable: bool,
        now_secs: u64,
        writer: LatchWriter,
    ) {
        if current_map == Some(map) || !resettable {
            return;
        }
        self.last_instance = Some(map);
        self.last_instance_at = now_secs;
        // Walking out of one proves the bind the server took on entry.
        if writer == LatchWriter::WorldEntry {
            self.saw_own_dungeon = true;
        }
    }

    /// `SMSG_INSTANCE_RESET`, before the body is read (`0x5138a0`): any reset clears the offer.
    pub(crate) fn clear_last_instance(&mut self) {
        self.last_instance = None;
        self.last_instance_at = 0;
        // The witness goes with the reset; the reference's bind term is the server's to retract.
        self.forget_witness();
    }

    /// Drop benilla's half of the bind term, the one piece of state no server corrects.
    pub(crate) fn forget_witness(&mut self) {
        self.saw_own_dungeon = false;
    }

    /// `CanShowResetInstances()` (`0x5194e0`), its terms in the reference's order:
    ///
    /// 1. a bind held, or [`InstanceState::saw_own_dungeon`];
    /// 2. not standing in a resettable instance;
    /// 3. the last instance left has a `Map.dbc` row;
    /// 4. it is resettable;
    /// 5. left no more than [`RESET_OFFER_WINDOW_SECS`] ago.
    ///
    /// Deviation: term 4 tests the last instance, where the reference reads the current map's
    /// record a second time (`0x519567`), which contradicts term 2 and keeps its row from ever
    /// showing.
    pub(crate) fn can_reset(&self, current_map: Option<u32>, facts: &MapFacts, now: u64) -> bool {
        if !self.owns_saved && !self.saw_own_dungeon {
            return false;
        }
        if current_map.is_some_and(facts.resettable) {
            return false;
        }
        let Some(last) = self.last_instance.filter(|&m| (facts.known)(m)) else {
            return false;
        };
        if !(facts.resettable)(last) {
            return false;
        }
        now.saturating_sub(self.last_instance_at) <= RESET_OFFER_WINDOW_SECS
    }

    /// The leave-world sweep's leg (`0x5258f6`): the lock clock stops, signalled.
    fn leave_world(&mut self) {
        self.lock = LockClock::default();
        self.lock_events.push("INSTANCE_LOCK_STOP");
    }

    fn push(&mut self, line: LockoutLine) {
        self.lines.push(line);
    }
}

/// One encounter slot (`0x5ed7b0`): a unit, its priority and its place.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct EncounterSlot {
    guid: u64,
    priority: u8,
    index: u8,
}

/// The encounter's boss slots (`[0xc242f8]`), which `bossN` reads (`0x5ed710`): empty, or
/// [`BOSS_SLOTS`] slots once a unit engages, the held ones sorted first.
#[derive(Resource, Debug, Default)]
pub(crate) struct EncounterUnits {
    slots: Vec<EncounterSlot>,
    /// `INSTANCE_ENCOUNTER_ENGAGE_UNIT`s owed, fired by the unit feed after it pushes `bossN`.
    signals: u32,
}

impl EncounterUnits {
    /// `bossN`'s guid for 0-based `row`, 0 past the slots or on an empty one.
    pub(crate) fn boss(&self, row: usize) -> u64 {
        self.slots.get(row).map_or(0, |s| s.guid)
    }

    /// Every slot's guid, 0 for an empty one; none before the first engage.
    pub(crate) fn guids(&self) -> impl Iterator<Item = u64> + '_ {
        self.slots.iter().map(|s| s.guid)
    }

    /// The signals owed, taken; a zero count leaves change detection alone.
    pub(crate) fn take_signals(this: &mut ResMut<Self>) -> u32 {
        if this.signals == 0 {
            return 0;
        }
        std::mem::take(&mut this.signals)
    }

    /// One `SMSG_UPDATE_INSTANCE_ENCOUNTER_UNIT` (`0x5eddd0`). The timers and objectives are kept
    /// by the reference and read by nothing, so they are dropped here.
    pub(crate) fn apply(&mut self, update: EncounterUnitUpdate) {
        use EncounterUnitUpdate as U;
        match update {
            U::Engage { guid, priority } => self.engage(guid, priority),
            U::Disengage { guid } => self.disengage(guid),
            U::Priority { guid, priority } => {
                if let Some(slot) = self.slots.iter_mut().find(|s| s.guid == guid) {
                    slot.priority = priority;
                    self.sort();
                    self.signals += 1;
                }
            }
            U::Resort => {
                self.sort();
                self.signals += 1;
            }
            U::Timer { .. }
            | U::EnableObjective { .. }
            | U::UpdateObjective { .. }
            | U::DisableObjective { .. }
            | U::Unknown(_) => {}
        }
    }

    /// `0x5ed7b0`: a held unit is ignored; else the slots are made on first use and the unit takes
    /// the first empty one, at its own index.
    fn engage(&mut self, guid: u64, priority: u8) {
        if self.slots.iter().any(|s| s.guid == guid) {
            return;
        }
        if self.slots.len() < BOSS_SLOTS {
            self.slots = vec![EncounterSlot::default(); BOSS_SLOTS];
        }
        if let Some((i, slot)) = self.slots.iter_mut().enumerate().find(|(_, s)| s.guid == 0) {
            *slot = EncounterSlot {
                guid,
                priority,
                index: i as u8,
            };
        }
        self.sort();
        self.signals += 1;
    }

    /// `0x5ed870`: a held unit's slot is emptied; with the first slot then empty, the slots go.
    fn disengage(&mut self, guid: u64) {
        let Some(slot) = self.slots.iter_mut().find(|s| s.guid == guid) else {
            return;
        };
        *slot = EncounterSlot::default();
        self.sort();
        self.signals += 1;
        if self.slots.first().is_some_and(|s| s.guid == 0) {
            self.slots.clear();
        }
    }

    /// `0x5ed750`: held slots first, by priority then index (`0x5ed590`), then each held slot's
    /// index renumbered to its place.
    fn sort(&mut self) {
        self.slots
            .sort_by_key(|s| (s.guid == 0, s.priority, s.index));
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.guid != 0 {
                slot.index = i as u8;
            }
        }
    }

    /// The world leave (`0x5edb10`): the slots go, unsignalled.
    fn clear(&mut self) {
        self.slots.clear();
        self.signals = 0;
    }
}

/// One queued line's text: the template, the `%s` and `%d` fills, then the debug wrapper; `None`
/// for a token the player's `GlobalStrings.lua` leaves empty.
fn lockout_text(line: &LockoutLine, map_name: Option<&str>, template: &str) -> Option<String> {
    let mut args: Vec<benilla_ui::strings::Arg<'_>> = Vec::new();
    args.extend(map_name.map(benilla_ui::strings::Arg::S));
    args.extend(line.numbers.iter().map(|n| benilla_ui::strings::Arg::D(*n)));
    let text = benilla_ui::strings::fill(template, &args);
    if text.is_empty() {
        return None;
    }
    Some(if line.debug_notice {
        format!("{DEBUG_LOCK_NOTICE_PREFIX}{text}")
    } else {
        text
    })
}

/// The reference's `"(Debug-Only Lock Notice) %s"`, a literal in the binary, not a GlobalString.
const DEBUG_LOCK_NOTICE_PREFIX: &str = "(Debug-Only Lock Notice) ";

/// A template's `%s`: the `Map.dbc` name, or the id in decimal when there is no row.
fn map_name(map: u32, catalog: Option<&benilla_assets::MapCatalogRes>) -> String {
    catalog
        .and_then(|c| c.0.name(map))
        .filter(|n| !n.is_empty())
        .map_or_else(|| map.to_string(), str::to_string)
}

/// `0x4fe070`: the map's name, as `DUNGEON_NAME_WITH_DIFFICULTY` with its `MapDifficulty.dbc`
/// difficulty's text when the row names one; "" with no map row.
fn map_name_with_difficulty(
    map: u32,
    difficulty: u32,
    catalog: Option<&benilla_assets::MapCatalogRes>,
    difficulties: Option<&crate::ui_transfer::MapDifficulties>,
    text: &dyn Fn(&str) -> String,
) -> String {
    let Some(name) = catalog.and_then(|c| c.0.name(map)) else {
        return String::new();
    };
    let row = difficulties
        .and_then(|d| benilla_formats::find_map_difficulty(&d.0, map as i32, difficulty as i32));
    match row.filter(|r| !r.difficulty_string.is_empty()) {
        Some(r) => benilla_ui::strings::fill(
            &text("DUNGEON_NAME_WITH_DIFFICULTY"),
            &[
                benilla_ui::strings::Arg::S(name),
                benilla_ui::strings::Arg::S(&text(&r.difficulty_string)),
            ],
        ),
        None => name.to_string(),
    }
}

/// The line for one `SMSG_RAID_INSTANCE_MESSAGE` (`0x50ca80`), `None` for the welcome, an event,
/// and for a type the jump table drops. The counts divide signed, truncating.
fn raid_instance_line(message: &RaidInstanceMessage) -> Option<LockoutLine> {
    let warning = RaidInstanceWarning::from_wire(message.message_type)?;
    let token = warning.token()?;
    let numbers = match warning {
        RaidInstanceWarning::Hours => vec![i64::from(message.seconds / 3_600)],
        RaidInstanceWarning::Minutes | RaidInstanceWarning::MinutesSoon => {
            vec![i64::from(message.seconds / 60)]
        }
        _ => Vec::new(),
    };
    Some(LockoutLine {
        token,
        map: LineMap::WithDifficulty(message.map, message.difficulty),
        numbers,
        debug_notice: false,
    })
}

/// The instance packet handlers; the feed, which has the VM, resolves what they queue.
pub(crate) mod net {
    use super::*;

    use benilla_protocol::messages::InstanceResetFailed;
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::RaidInstanceMessage, on_packet)
            .net_handler(K::InstanceSaveCreated, on_packet)
            .net_handler(K::InstanceReset, on_packet)
            .net_handler(K::InstanceResetFailed, on_packet)
            .net_handler(K::ResetFailedNotify, on_packet)
            .net_handler(K::InstanceLockWarning, on_packet)
            .net_handler(K::UpdateLastInstance, on_packet)
            .net_handler(K::UpdateInstanceOwnership, on_packet)
            .net_handler(K::EncounterUnit, on_encounter_unit);
    }

    fn on_packet(In(ev): In<SessionEvent>, mut state: ResMut<InstanceState>) {
        match ev {
            SessionEvent::RaidInstanceMessage { message } => {
                raid_instance_message(&mut state, message)
            }
            SessionEvent::InstanceSaveCreated { flag } => instance_save_created(&mut state, flag),
            SessionEvent::InstanceReset { map } => instance_reset(&mut state, map),
            SessionEvent::InstanceResetFailed { failure } => {
                instance_reset_failed(&mut state, failure)
            }
            SessionEvent::ResetFailedNotify => reset_failed_notify(&mut state),
            SessionEvent::InstanceLockWarning { warning } => {
                instance_lock_warning(&mut state, warning, Instant::now())
            }
            SessionEvent::UpdateLastInstance { map } => update_last_instance(&mut state, map),
            SessionEvent::UpdateInstanceOwnership { owns } => {
                update_instance_ownership(&mut state, owns)
            }
            _ => {}
        }
    }

    fn on_encounter_unit(In(ev): In<SessionEvent>, mut units: ResMut<EncounterUnits>) {
        if let SessionEvent::EncounterUnit { update } = ev {
            debug!("ui_instance: encounter unit {update:?}");
            units.apply(update);
        }
    }

    /// `SMSG_RAID_INSTANCE_MESSAGE`: a countdown or expiry line, or the welcome event.
    pub(crate) fn raid_instance_message(state: &mut InstanceState, message: RaidInstanceMessage) {
        if let Some((locked, extended)) = message.welcome {
            state.welcomes.push((
                message.map,
                message.difficulty,
                message.seconds,
                locked,
                extended,
            ));
            return;
        }
        match raid_instance_line(&message) {
            Some(line) => state.push(line),
            None => debug!(
                "ui_instance: SMSG_RAID_INSTANCE_MESSAGE type {} has no arm — silent, as the \
                 reference's jump table is",
                message.message_type
            ),
        }
    }

    /// `SMSG_INSTANCE_RESET` (`0x50ccd0`): clear the latch before the body is read, then print
    /// `INSTANCE_RESET_SUCCESS`.
    pub(crate) fn instance_reset(state: &mut InstanceState, map: u32) {
        state.clear_last_instance();
        state.push(LockoutLine::plain(
            "INSTANCE_RESET_SUCCESS",
            LineMap::Name(map),
        ));
    }

    /// `SMSG_INSTANCE_RESET_FAILED` (`0x50cda0`): one of three refusals. Deviation: reason 3 and
    /// up prints nothing, where the reference prints an uninitialized stack buffer.
    pub(crate) fn instance_reset_failed(state: &mut InstanceState, failure: InstanceResetFailed) {
        match InstanceResetFailure::from_wire(failure.reason) {
            Some(reason) => state.push(LockoutLine::plain(
                reason.token(),
                LineMap::Name(failure.map),
            )),
            None => debug!(
                "ui_instance: SMSG_INSTANCE_RESET_FAILED reason {} has no line",
                failure.reason
            ),
        }
    }

    /// `SMSG_RESET_FAILED_NOTIFY` (`0x50cee0`): `RESET_FAILED_NOTIFY`, the body unread.
    pub(crate) fn reset_failed_notify(state: &mut InstanceState) {
        state.push(LockoutLine::plain("RESET_FAILED_NOTIFY", LineMap::None));
    }

    /// `SMSG_INSTANCE_LOCK_WARNING_QUERY` (`0x6e2e90`): the lock clock, and its event.
    pub(crate) fn instance_lock_warning(
        state: &mut InstanceState,
        warning: InstanceLockWarning,
        now: Instant,
    ) {
        let event = state.lock.set(warning, now);
        state.lock_events.push(event);
    }

    /// `SMSG_UPDATE_LAST_INSTANCE` (`0x4fe100`): queued for [`track_instance_state`] to weigh
    /// against the map we end up on. No line.
    pub(crate) fn update_last_instance(state: &mut InstanceState, map: u32) {
        state.pending_last_instance.push(map);
    }

    /// `SMSG_UPDATE_INSTANCE_OWNERSHIP` (`0x4fb990`): one store, no line.
    pub(crate) fn update_instance_ownership(state: &mut InstanceState, owns: bool) {
        state.set_ownership(owns);
    }

    /// `SMSG_INSTANCE_SAVE_CREATED` (`0x52bb10`): flag 0 prints `INSTANCE_SAVED` and stops the
    /// lock clock; flag 1 prints it wrapped. Deviation: flag 2 and up prints nothing, where the
    /// reference prints an uninitialized stack buffer.
    pub(crate) fn instance_save_created(state: &mut InstanceState, flag: u32) {
        let debug_notice = match flag {
            0 => {
                let stop = InstanceLockWarning {
                    ms: 0,
                    completed_mask: 0,
                    extending: false,
                };
                instance_lock_warning(state, stop, Instant::now());
                false
            }
            1 => true,
            other => {
                debug!("ui_instance: SMSG_INSTANCE_SAVE_CREATED flag {other} has no arm — no line");
                return;
            }
        };
        state.push(LockoutLine {
            debug_notice,
            ..LockoutLine::plain("INSTANCE_SAVED", LineMap::None)
        });
    }
}

/// Whether `map` is resettable at the current dungeon difficulty (`0x5195f1`-`0x519616`).
fn resettable(
    map: u32,
    maps: Option<&benilla_assets::MapCatalogRes>,
    difficulties: Option<&crate::ui_transfer::MapDifficulties>,
    difficulty: u32,
) -> bool {
    if !matches!(maps.and_then(|c| c.0.instance_type(map)), Some(1 | 2)) {
        return false;
    }
    difficulty > 3
        || difficulties
            .and_then(|d| benilla_formats::find_map_difficulty(&d.0, map as i32, difficulty as i32))
            .is_none_or(|r| r.raid_duration == 0)
}

/// The map catalog, the difficulty rows and the group, the inputs of [`resettable`].
#[derive(bevy::ecs::system::SystemParam)]
struct ResetInputs<'w> {
    maps: Option<Res<'w, benilla_assets::MapCatalogRes>>,
    difficulties: Option<Res<'w, crate::ui_transfer::MapDifficulties>>,
    group: Option<Res<'w, crate::ui_party::GroupState>>,
    current_map: Option<Res<'w, benilla_world::world_map::CurrentMap>>,
}

impl ResetInputs<'_> {
    fn resettable(&self, map: u32) -> bool {
        let difficulty = self
            .group
            .as_ref()
            .map_or(0, |g| g.current_dungeon_difficulty());
        resettable(
            map,
            self.maps.as_deref(),
            self.difficulties.as_deref(),
            difficulty,
        )
    }

    fn known(&self, map: u32) -> bool {
        self.maps
            .as_ref()
            .is_some_and(|c| c.0.instance_type(map).is_some())
    }
}

/// Runs both latch writers, VM-free so the latch survives a `/reload` as the reference's globals
/// do. The first observed map records nothing: there is no map left at login.
fn track_instance_state(
    mut state: ResMut<InstanceState>,
    inputs: ResetInputs,
    time: Res<Time<Real>>,
    mut last_map: Local<Option<u32>>,
) {
    let Some(here) = inputs.current_map.as_ref().map(|m| m.0) else {
        return;
    };
    let now = time.elapsed().as_secs();

    // Writer 1, the world entry: `prev` is the map we are leaving.
    if let Some(prev) = last_map.replace(here) {
        if prev != here {
            let resettable = inputs.resettable(prev);
            state.note_last_instance(prev, Some(here), resettable, now, LatchWriter::WorldEntry);
        }
    }
    // Writer 2, the packet, now that `CurrentMap` has caught up with the transfer.
    if !state.pending_last_instance.is_empty() {
        for map in std::mem::take(&mut state.pending_last_instance) {
            let resettable = inputs.resettable(map);
            state.note_last_instance(map, Some(here), resettable, now, LatchWriter::Packet);
        }
    }
}

/// Show every queued line as `CHAT_MSG_SYSTEM`, fire the welcomes and the lock events, then
/// publish what the instance verbs read; the setters diff.
#[allow(clippy::too_many_arguments)]
fn feed_instance(
    script: Option<NonSendMut<UiScript>>,
    mut state: ResMut<InstanceState>,
    mut chat: ResMut<ChatLog>,
    inputs: ResetInputs,
    time: Res<Time<Real>>,
    mut fed_maps: Local<crate::ui_script::VmMemo<bool>>,
) {
    let Some(mut script) = script else {
        // No VM (an engine-only run): drop the queues rather than let them grow.
        state.lines.clear();
        state.welcomes.clear();
        state.lock_events.clear();
        return;
    };
    let maps = inputs.maps.as_deref();
    let difficulties = inputs.difficulties.as_deref();
    if !state.lines.is_empty() {
        for line in std::mem::take(&mut state.lines) {
            let text = |token: &str| script.engine_text(token);
            let name = match line.map {
                LineMap::None => None,
                LineMap::Name(m) => Some(map_name(m, maps)),
                LineMap::WithDifficulty(m, d) => {
                    Some(map_name_with_difficulty(m, d, maps, difficulties, &text))
                }
            };
            match lockout_text(&line, name.as_deref(), &text(line.token)) {
                Some(text) => {
                    debug!("ui_instance: {} -> {text:?}", line.token);
                    chat.push_event(ChatEvent::text_only(ChatEventKind::System, text));
                }
                None => debug!("ui_instance: {} resolves to nothing — no line", line.token),
            }
        }
    }
    for (map, difficulty, seconds, locked, extended) in std::mem::take(&mut state.welcomes) {
        let text = |token: &str| script.engine_text(token);
        let name = map_name_with_difficulty(map, difficulty, maps, difficulties, &text);
        debug!("ui_instance: RAID_INSTANCE_WELCOME {name:?} {seconds} {locked} {extended}");
        script.fire_event(
            "RAID_INSTANCE_WELCOME",
            vec![
                ScriptValue::Str(name),
                ScriptValue::Int(i64::from(seconds)),
                ScriptValue::Int(i64::from(locked)),
                ScriptValue::Int(i64::from(extended)),
            ],
        );
    }
    // The clock before its events, so `INSTANCE_LOCK_START`'s `OnShow` reads the time.
    script.set_instance_lock(state.lock.read(Instant::now()));
    if !state.lock_events.is_empty() {
        for event in std::mem::take(&mut state.lock_events) {
            debug!("ui_instance: {event}");
            script.fire_event(event, vec![]);
        }
    }

    let fed = fed_maps.get(&script);
    if !*fed {
        if let Some(maps) = maps {
            script.set_dynamic_difficulty_maps(maps.0.dynamic_difficulty_maps());
            *fed = true;
        }
    }
    let here = inputs.current_map.as_ref().map(|m| m.0);
    script.set_instance_type(here.and_then(|m| maps.and_then(|c| c.0.instance_type(m))));
    script.set_map_info(here.and_then(|m| {
        let c = &maps?.0;
        c.instance_type(m)?;
        Some((
            c.name(m).unwrap_or_default().to_string(),
            c.has_dynamic_difficulty(m),
        ))
    }));
    let facts = MapFacts {
        known: &|m| inputs.known(m),
        resettable: &|m| inputs.resettable(m),
    };
    script.set_can_reset_instances(state.can_reset(here, &facts, time.elapsed().as_secs()));
}

/// Turn the `ResetInstances()` calls and `RespondInstanceLock` answers into their sends.
fn drain_instance(script: Option<NonSendMut<UiScript>>, commands: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for _ in 0..script.take_reset_instance_asks() {
        let _ = commands.0.send(ClientCommand::ResetInstances);
    }
    for accept in script.take_instance_lock_responses() {
        let _ = commands
            .0
            .send(ClientCommand::InstanceLockResponse { accept });
    }
}

/// The leave-world sweep (a cross-map worldport, a `/reload`, a logout): the lock clock stops.
fn instance_leave_world(
    mut sweeps: MessageReader<WorldLeaveSweepMessage>,
    mut state: ResMut<InstanceState>,
) {
    if sweeps.read().next().is_some() {
        state.leave_world();
    }
}

/// The encounter's world leave (`0x5edb10`), skipped on a `/reload`: a cross-map worldport and a
/// logout drop the boss slots.
fn encounter_leave_world(
    mut ports: MessageReader<crate::net::WorldportMessage>,
    mut logged_out: MessageReader<crate::net::LoggedOutMessage>,
    mut units: ResMut<EncounterUnits>,
) {
    let ported = ports.read().filter(|w| w.needs_ack).count() > 0;
    let out = logged_out.read().count() > 0;
    if (ported || out) && !units.slots.is_empty() {
        units.clear();
    }
}

/// Drop the witness at logout, as the next login may be another character. The reference's
/// globals stay (its clear runs only on `SMSG_INSTANCE_RESET`).
fn clear_witness_on_logout(
    mut state: ResMut<InstanceState>,
    mut logged_out: MessageReader<crate::net::LoggedOutMessage>,
) {
    if logged_out.read().next().is_some() {
        state.forget_witness();
    }
}

pub(crate) struct UiInstancePlugin;

impl Plugin for UiInstancePlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<InstanceState>()
            .init_resource::<EncounterUnits>()
            .add_message::<WorldLeaveSweepMessage>()
            .add_systems(
                Update,
                (
                    // After the drain that writes the logout and sweep messages; the latch before
                    // the feed, so a map change and its answer share a frame.
                    (
                        clear_witness_on_logout,
                        instance_leave_world,
                        track_instance_state,
                        feed_instance
                            .in_set(UiFeed)
                            .run_if(crate::ui_script::ingame_ui_up),
                    )
                        .chain()
                        .after(crate::net::apply_net_updates)
                        .after(crate::ui_unit::fire_leaving_world_on_worldport),
                    // Before the feeds that read the slots, so a port's frame pushes no boss.
                    encounter_leave_world
                        .after(crate::net::apply_net_updates)
                        .before(UiFeed),
                    drain_instance.after(UiInput),
                ),
            );
    }
}

#[cfg(test)]
mod tests;
