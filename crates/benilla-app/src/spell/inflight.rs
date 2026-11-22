//! The player's own cast state: the pending cast (`SPELLCAST` and the inflight id), the queued
//! on-next-swing strike, the running channel, the auto-repeat key, and the local abort on a move,
//! jump or Esc (the cast abort).

use std::time::{Duration, Instant};

use bevy::prelude::*;

use crate::creature_anim::{CastEvent, CastEventKind, Casting, PlaySeq};
use crate::net::{ClientCommand, GuidIndex, NetCommands, SelfGuid};
use crate::ui_action::Spells;

/// The pending-spell list (`[0xaf5254]`): a record per sent attempt keyed by count, spell and caster, released by its answer alone (`0x805180`, `0x809af0`, `0x809c70`, `0x806200`) or the teardown (`0x807526`).
#[derive(Resource, Default)]
pub(crate) struct PendingCast {
    records: Vec<PendingRecord>,
    /// The `castCount` each cast attempt's record takes (`0x8050dc`-`0x8050ec`).
    counts: benilla_protocol::messages::CastCounter,
    /// Each count whose attempt went, sent or armed the cursor (`0x80d9b0`, `0x80da1a`).
    went: [u64; 4],
}

#[cfg(test)]
impl PendingCast {
    /// The newest record's spell, of any class.
    pub(crate) fn committed(&self) -> Option<u32> {
        self.records.last().map(|r| r.spell_id)
    }

    /// File a player record of `spell_id` with `count`, its caster unknown.
    pub(crate) fn arm(&mut self, spell_id: u32, count: u8) {
        self.file(PendingRecord::player(spell_id, count, 0));
    }
}

/// One pending-spell record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingRecord {
    pub(crate) spell_id: u32,
    /// The record's `castCount`, which its answer and its cancel name (`0x805180`, `0x8062de`).
    pub(crate) count: u8,
    /// The casting object (`[record+0x8]`): the player, the pet, or for an item use the item; 0 where the guid is not known.
    pub(crate) caster: u64,
    /// Sent by the pet's ladder: the player's own tests skip it.
    pub(crate) pet: bool,
    /// The entry of the item that started it, which the current-item test (`0x805fc0`) reads.
    pub(crate) item: Option<u32>,
}

impl PendingRecord {
    /// A record of the player's own cast.
    pub(crate) fn player(spell_id: u32, count: u8, caster: u64) -> Self {
        Self {
            spell_id,
            count,
            caster,
            pet: false,
            item: None,
        }
    }
}

impl PendingCast {
    /// The `castCount` a new cast attempt's record takes: 1 to 255, never 0 (`0x80cda8`, `0x8050dc`).
    pub(crate) fn take_count(&mut self) -> u8 {
        let count = self.counts.take();
        self.went[usize::from(count / 64)] &= !(1 << (count % 64));
        count
    }

    /// The attempt that took `count` went: its own true return (`0x80d9b0`, `0x80da1a`).
    pub(crate) fn mark_went(&mut self, count: u8) {
        self.went[usize::from(count / 64)] |= 1 << (count % 64);
    }

    /// Whether the attempt that took `count` went, whatever its listeners did after.
    pub(crate) fn went(&self, count: u8) -> bool {
        self.went[usize::from(count / 64)] & (1 << (count % 64)) != 0
    }

    /// The `castCount` the next cast attempt takes, left untaken.
    pub(crate) fn next_count(&self) -> u8 {
        self.counts.peek()
    }

    /// File a sent attempt's record at the list's end.
    pub(crate) fn file(&mut self, record: PendingRecord) {
        self.records.push(record);
    }

    /// Every record, oldest first.
    pub(crate) fn records(&self) -> &[PendingRecord] {
        &self.records
    }

    /// A record of `spell_id` cast by `caster`, any caster for 0 (`0x805f60`): the attempt's pending bail. A record filed without its caster's guid matches any.
    pub(crate) fn listed_for(&self, spell_id: u32, caster: u64) -> bool {
        self.records
            .iter()
            .any(|r| r.spell_id == spell_id && (caster == 0 || r.caster == 0 || r.caster == caster))
    }

    /// Any record cast by `caster` (`0x805ef0`): the pet setter's modal test. A record filed without its caster's guid is the player's and never a pet's.
    pub(crate) fn caster_has(&self, caster: u64) -> bool {
        caster != 0 && self.records.iter().any(|r| r.caster == caster)
    }

    /// The newest record of the player's own, the one its cancel and its ring name.
    pub(crate) fn current(&self) -> Option<&PendingRecord> {
        self.records.iter().rev().find(|r| !r.pet)
    }

    /// Whether the player has any record pending.
    pub(crate) fn in_flight(&self) -> bool {
        self.current().is_some()
    }

    /// The record an answer names by spell and `castCount` (`0x805180`).
    pub(crate) fn find(&self, spell_id: u32, count: u8) -> Option<&PendingRecord> {
        self.records
            .iter()
            .find(|r| r.spell_id == spell_id && r.count == count)
    }

    /// The answer's release of the record it names (`0x805180`); a record of another spell or count stays.
    pub(crate) fn release(&mut self, spell_id: u32, count: u8) -> Option<PendingRecord> {
        let at = self
            .records
            .iter()
            .position(|r| r.spell_id == spell_id && r.count == count)?;
        Some(self.records.remove(at))
    }

    /// Release every record cast by `caster`, oldest first (`0x806390`'s walk); none for 0.
    pub(crate) fn release_caster(&mut self, caster: u64) -> Vec<PendingRecord> {
        if caster == 0 {
            return Vec::new();
        }
        self.release_where(|r| r.caster == caster)
    }

    /// Release every record of `spell_id`, oldest first (`0x806440`'s walk).
    pub(crate) fn release_spell(&mut self, spell_id: u32) -> Vec<PendingRecord> {
        self.release_where(|r| r.spell_id == spell_id)
    }

    fn release_where(&mut self, pick: impl Fn(&PendingRecord) -> bool) -> Vec<PendingRecord> {
        let (out, keep) = std::mem::take(&mut self.records)
            .into_iter()
            .partition(|r| pick(r));
        self.records = keep;
        out
    }

    /// Release one record, the one a local cancel ended.
    pub(crate) fn release_record(&mut self, record: &PendingRecord) {
        if let Some(at) = self.records.iter().position(|r| r == record) {
            self.records.remove(at);
        }
    }

    /// Drop every record, answered or not; the `castCount` counter runs on (`0x807526`).
    fn clear(&mut self) {
        self.records.clear();
    }
}

/// The world's teardown empties the pending-spell list (`0x406510` -> `0x742bb0` -> `0x8073a0`, `0x807526`).
pub(crate) fn clear_on_world_leave(mut pending: ResMut<PendingCast>) {
    pending.clear();
}

/// The cast cancel (`0x806200`) of each released record: `CMSG_CANCEL_CAST` with its count when its unit runs it or it is on-next-swing (`0x8062a6`-`0x8062fe`), the cast end (`0x806358`), the finish's selection update (`0x8054f0`).
pub(crate) fn cancel_records(world: &mut World, records: &[(PendingRecord, u64)], wire_reason: u8) {
    for &(record, unit) in records {
        let entity = world.resource::<GuidIndex>().0.get(&unit).copied();
        let running = entity
            .and_then(|e| world.get::<Casting>(e))
            .is_some_and(|c| c.ended_by(record.spell_id, record.count) && !c.hidden);
        let next_swing = world
            .get_resource::<Spells>()
            .and_then(|s| s.catalog.get(record.spell_id))
            .is_some_and(|d| d.attributes & 0x404 != 0);
        if running || next_swing {
            if let Some(net) = world.get_resource::<NetCommands>() {
                let _ = net.0.send(ClientCommand::CancelCast {
                    spell_id: record.spell_id,
                    count: record.count,
                });
            }
        }
        if let Some(e) = entity {
            let ended = world
                .get::<Casting>(e)
                .is_some_and(|c| c.ended_by(record.spell_id, record.count));
            if ended {
                world.entity_mut(e).remove::<Casting>();
            }
            let seq = world.get_resource_or_init::<PlaySeq>().next();
            world.write_message(CastEvent {
                entity: e,
                spell_id: record.spell_id,
                kind: CastEventKind::Fail {
                    interrupt: false,
                    wire_reason,
                    ends_cast: ended,
                },
                cast_count: if ended { record.count } else { 0 },
                seq,
            });
        }
        if let Some(mut ui) = world.get_resource_mut::<crate::ui_script::live::UiEvents>() {
            ui.cast_edge(crate::ui_cast::CastBarEdge::Cancelled {
                caster: unit,
                reason: wire_reason,
            });
            ui.cast_edge(crate::ui_cast::CastBarEdge::Selection);
        }
    }
}

/// The pet setter's modal test and cancel (`0x5d6afb`-`0x5d6b24`): a record of the new head, a changed guid or an empty head cancels the handed guid's records (`0x806390`, reason `0x1b` at `0x8063c1`).
pub(crate) fn pet_setter_cancel(world: &mut World, changed: bool, handed: u64) {
    let head = world
        .get_resource::<crate::ui_pet::PetList>()
        .and_then(|l| l.head());
    let Some(mut pending) = world.get_resource_mut::<PendingCast>() else {
        return;
    };
    if !pending.caster_has(head.unwrap_or(0)) {
        return;
    }
    if !changed && head.is_some_and(|h| h != 0) {
        return;
    }
    let records: Vec<_> = pending
        .release_caster(handed)
        .into_iter()
        .map(|r| (r, handed))
        .collect();
    cancel_records(world, &records, PET_CHANGE_REASON);
}

/// The pet setter's cancel reason (`0x8063c1`).
const PET_CHANGE_REASON: u8 = 0x1b;

/// Our queued on-next-swing spell (Heroic Strike, Cleave: `Attributes & 0x404`). The reference
/// keeps it in the inflight id, and a new cast nests around it (the nested-cast push and pop); this
/// second slot beside [`PendingCast`] gives the same observable.
///
/// No deadline: cleared, keyed on the id, by `SMSG_SPELL_GO` when the swing fires or by the failing
/// `SMSG_CAST_RESULT` and `SMSG_SPELL_FAILED_OTHER` vmangos sends when the melee slot dies. The
/// reference clears it on a matching `SMSG_CAST_RESULT` alone (its GO handler never touches it);
/// vmangos sends the OK result just before the GO (`Spell.cpp:3681`, `Spell.cpp:3715`). A re-arm
/// replaces it (the server's one `CURRENT_MELEE_SPELL`). Re-pressing the queued spell bails
/// silently; it un-queues through the StopAttack chain (the queued-cast cancel), which an
/// auto-repeat press also runs, or through Esc, which cancels it as the inflight spell
/// (`SpellStopCasting`).
#[derive(Resource, Default)]
pub(crate) struct QueuedMeleeSpell(Option<(u32, u8)>);

impl QueuedMeleeSpell {
    pub(crate) fn current(&self) -> Option<u32> {
        self.0.map(|(id, _)| id)
    }

    /// The queued strike's record `castCount`, which its cancel names (`0x8062de`).
    pub(crate) fn count(&self) -> Option<u8> {
        self.0.map(|(_, count)| count)
    }

    pub(crate) fn arm(&mut self, spell_id: u32, count: u8) {
        self.0 = Some((spell_id, count));
    }

    pub(crate) fn clear_if(&mut self, spell_id: u32) {
        if self.current() == Some(spell_id) {
            self.0 = None;
        }
    }

    /// The queued strike's record unlinked (`0x805100`): only the record of this spell and count.
    pub(crate) fn clear_record(&mut self, spell_id: u32, count: u8) {
        if self.0 == Some((spell_id, count)) {
            self.0 = None;
        }
    }
}

/// What the reference's one inflight id holds, joined from our two slots: the in-flight check and
/// the cast abort treat a cast and a queued strike alike.
///
/// A cast started over a queued strike pushes it into the save, so the cast comes first and the
/// first Esc takes it, the second the strike. Deviation: the reference's second Esc re-asserts the
/// strike from the save it never clears, where ours clears, because vmangos drops the melee slot on
/// the first cancel whatever its id (`HandleCancelCastOpcode`) and both clients converge on that
/// echo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Inflight {
    /// An ordinary cast, with a guard, a bar and a `Casting` to reap.
    Cast(u32),
    /// A queued on-next-swing strike: no bar and no [`Casting`], so a cancel is the wire message
    /// and the slot.
    Strike(u32),
}

impl Inflight {
    /// What a cancel names on the wire.
    pub(crate) fn spell_id(self) -> u32 {
        match self {
            Self::Cast(id) | Self::Strike(id) => id,
        }
    }
}

/// The reference's inflight slot from our two. `started` is the self [`Casting`] spell id, which
/// covers a started cast that never armed the send guard.
pub(crate) fn inflight(
    pending: &PendingCast,
    queued_melee: &QueuedMeleeSpell,
    started: Option<u32>,
) -> Option<Inflight> {
    pending
        .current()
        .map(|r| r.spell_id)
        .or(started)
        .map(Inflight::Cast)
        .or_else(|| queued_melee.current().map(Inflight::Strike))
}

/// Slack past the named channel end, so a lost `MSG_CHANNEL_UPDATE(0)` cannot pin the ring on.
const CHANNEL_SLACK: Duration = Duration::from_secs(2);

/// Our running channel, as the client keeps it: `IsCurrentAction` keeps a channeled spell's button
/// checked while it runs. `MSG_CHANNEL_UPDATE(0)` ends it, finished or interrupted.
#[derive(Resource, Default)]
pub(crate) struct ActiveChannel(Option<(u32, Instant)>);

impl ActiveChannel {
    pub(crate) fn current(&self, now: Instant) -> Option<u32> {
        self.0
            .filter(|&(_, until)| now < until)
            .map(|(spell, _)| spell)
    }

    /// `MSG_CHANNEL_START`.
    pub(crate) fn start(&mut self, spell_id: u32, duration_ms: u32, now: Instant) {
        self.0 = Some((
            spell_id,
            now + Duration::from_millis(u64::from(duration_ms)) + CHANNEL_SLACK,
        ));
    }

    /// `MSG_CHANNEL_UPDATE`: 0 ends the channel, anything else re-times it (pushback shortens).
    pub(crate) fn update(&mut self, remaining_ms: u32, now: Instant) {
        if remaining_ms == 0 {
            self.0 = None;
        } else if let Some((_, until)) = &mut self.0 {
            *until = now + Duration::from_millis(u64::from(remaining_ms)) + CHANNEL_SLACK;
        }
    }
}

/// The live auto-repeat spell, as the client keeps it: set at cast send for `AttributesEx2 & 0x20`,
/// cleared by `SMSG_CANCEL_AUTO_REPEAT` and a matching cast failure. `IsAutoRepeatAction` and the
/// button flash read it; the sticky `creature_anim::AutoRepeatArmed` is the idle gate.
///
/// The reference's auto-repeating spell (`0x800a00`). Its other cell, the auto-ranged spell, is
/// set only by the auto-ranged-combat mode's idle handler (`0x6e2be0`), which is not built, so it
/// stays empty and every write here signals: a new spell `START_AUTOREPEAT_SPELL`, the clear
/// `STOP_AUTOREPEAT_SPELL` (`0x807560`), each fired in turn into the writer's sink.
#[derive(Resource, Default)]
pub(crate) struct AutoRepeatActive(pub Option<u32>);

impl AutoRepeatActive {
    /// Set the spell (`0x800a00`): a new one signals the start, the same one nothing.
    pub(crate) fn set(
        &mut self,
        spell_id: u32,
        sink: &mut dyn crate::ui_action::signals::SignalSink,
    ) {
        if self.0 == Some(spell_id) {
            return;
        }
        self.0 = Some(spell_id);
        sink.signal("START_AUTOREPEAT_SPELL");
    }

    /// Clear it (`0x807560`): a held spell signals the stop. Whether one was held.
    pub(crate) fn clear(&mut self, sink: &mut dyn crate::ui_action::signals::SignalSink) -> bool {
        let held = self.0.take().is_some();
        if held {
            sink.signal("STOP_AUTOREPEAT_SPELL");
        }
        held
    }
}

/// The per-spell movement gates of the local self-cancel, tested. The move keybinds' dispatcher
/// cancels on mask `0x10f0` (forward, back, strafe, autorun; not turn or pitch) and the jump verb
/// likewise, both calling the cast abort for `CMSG_CANCEL_CAST` and the cast's end events.
pub(crate) const SPELL_INTERRUPT_MOVEMENT: u32 = 0x1; // SpellRec+0x54 InterruptFlags (SPELL_INTERRUPT_FLAG_MOVEMENT)
const CHANNEL_INTERRUPT_MOVING: u32 = 0x8; // SpellRec+0x5c ChannelInterruptFlags (AURA_INTERRUPT_MOVING_CANCELS)

/// The wire reason the movement cancel ends the cast with (`0x8066ce`): `INTERRUPTED`.
const MOVE_CANCEL_REASON: u8 = 0x28;
/// The reason Esc (`0x809aa8`) and the controls clear (`0x5194cc` -> `0x8063c1`) cancel with: `FAILED_QUIET` (`0x7fee0f`).
const STOP_CANCEL_REASON: u8 = 0x1b;

/// A directional move start or a jump happened this frame (the `0x10f0` mask; there is no
/// autorun yet). Set by `player::control`, consumed by [`local_self_cancel`].
#[derive(Resource, Default)]
pub(crate) struct LocalMoveStart(pub(crate) bool);

/// The client's local cancel on a move, jump or Esc mid-cast (the cast abort):
///
/// - Esc first stops a running auto-repeat (`SpellStopCasting`), and that spends the press; the
///   cast survives to the next one.
/// - The cast: `CMSG_CANCEL_CAST`, `Casting` reaped, the cast ended by its caller's reason ([`CastBarEdge::Cancelled`]); only a move tests `InterruptFlags`.
/// - The channel, on movement only: `CMSG_CANCEL_CHANNELLING` and nothing local; the bar and
///   [`ActiveChannel`] close on `MSG_CHANNEL_UPDATE(0)`. Esc cannot stop a channel: the Esc cancel
///   never calls the channel's, and the inflight id is 0 mid-channel.
///
/// 12340's cancel (`0x806200`) ends the cast itself (`0x7fecc0`), ahead of the `SMSG_SPELL_FAILURE` echo the bar ignores.
pub(super) fn local_self_cancel(mut moved: ResMut<LocalMoveStart>, mut cancel: SelfCancel) {
    if std::mem::take(&mut moved.0) {
        cancel.cancel(CancelBy::Moved);
        // No VM here: the cancel's cast-bar end fires once this system's commands apply.
        cancel.press.flush(&mut cancel.ecs);
    }
}

/// What asked [`SelfCancel`] to cancel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CancelBy {
    Esc,
    Moved,
    Controls,
}

/// What [`local_self_cancel`] reads and writes, shared with the Esc cancel, which is a script call
/// (`SpellStopCasting`) and lands in call order ([`crate::script_calls`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SelfCancel<'w, 's> {
    pending: ResMut<'w, PendingCast>,
    queued_melee: ResMut<'w, QueuedMeleeSpell>,
    channel: Res<'w, ActiveChannel>,
    auto_repeat: ResMut<'w, AutoRepeatActive>,
    /// The cancel's bar end, fired by the caller inside the press (`0x806200`).
    pub(crate) press: crate::ui_cast::PressSignals<'w, 's>,
    spells: Option<Res<'w, Spells>>,
    net: Res<'w, NetCommands>,
    self_guid: Res<'w, SelfGuid>,
    index: Res<'w, GuidIndex>,
    casting: Query<'w, 's, &'static Casting>,
    cast_events: MessageWriter<'w, CastEvent>,
    play_seq: ResMut<'w, PlaySeq>,
    pub(crate) ecs: Commands<'w, 's>,
}

impl SelfCancel<'_, '_> {
    /// `SpellStopCasting()`, the ESC chain's cast rung.
    pub(crate) fn stop_casting(&mut self) {
        self.cancel(CancelBy::Esc);
    }

    /// The client-controls clear's cancel of our casts (`0x5194cc` → `0x806390`): each ends through
    /// `0x806200` with its send set, as Esc's cast rung does, with no auto-repeat rung and no
    /// interrupt-flag test.
    pub(crate) fn cancel_player_spells(&mut self) {
        self.cancel(CancelBy::Controls);
    }

    fn cancel(&mut self, by: CancelBy) {
        let Self {
            pending,
            queued_melee,
            auto_repeat,
            channel,
            press,
            spells,
            net,
            self_guid,
            index,
            casting,
            cast_events,
            play_seq,
            ecs,
        } = self;
        let (pending, queued_melee, auto_repeat) = (
            pending.as_mut(),
            queued_melee.as_mut(),
            auto_repeat.as_mut(),
        );
        let now = Instant::now();
        let self_e = self_guid.0.as_ref().and_then(|g| index.0.get(g)).copied();
        let moved = by == CancelBy::Moved;
        let reason = if moved {
            MOVE_CANCEL_REASON
        } else {
            STOP_CANCEL_REASON
        };
        // Esc stops one thing, auto-repeat first.
        let esc = by == CancelBy::Esc;
        let esc = if esc && auto_repeat.0.is_some() {
            if *crate::net::CAST_TRACE {
                info!("cast-trace: LOCAL auto-repeat self-cancel (esc), CMSG_CANCEL_AUTO_REPEAT_SPELL");
            }
            crate::creature_anim::cancel_auto_repeat_local(
                self_e,
                auto_repeat,
                ecs,
                net,
                &mut *press,
            );
            false // the press is spent
        } else {
            esc
        };
        let forced = esc || by == CancelBy::Controls;
        if !forced && !moved {
            return;
        }
        let flags_open = |pick: fn(&benilla_formats::SpellDisplay) -> bool, spell_id: u32| {
            // An uncataloged spell cancels, as the server interrupts any ordinary cast.
            spells
                .as_ref()
                .and_then(|s| s.catalog.get(spell_id))
                .is_none_or(pick)
        };
        // The cast abort treats a cast and a queued strike alike; only the local teardown differs.
        // An unlisted held cast still cancels: 12340 hands `0x806200` a null pending cast there.
        let started = self_e.and_then(|e| casting.get(e).ok()).map(|c| c.spell_id);
        if let Some(slot) = inflight(pending, queued_melee, started) {
            let spell_id = slot.spell_id();
            // Movement tests `InterruptFlags`, which spares a queued strike: Heroic Strike 78,
            // Cleave 845 and Raptor Strike 2973 ship 0 (Fireball `0xf`).
            if forced
                || flags_open(
                    |d| d.interrupt_flags & SPELL_INTERRUPT_MOVEMENT != 0,
                    spell_id,
                )
            {
                if *crate::net::CAST_TRACE {
                    info!("cast-trace: LOCAL self-cancel — {slot:?} ({by:?}), CMSG_CANCEL_CAST");
                }
                // The cancel names the cancelled record's own count (`0x8062de`).
                let count = match slot {
                    Inflight::Cast(_) => pending
                        .current()
                        .filter(|r| r.spell_id == spell_id)
                        .map(|r| r.count)
                        .or_else(|| {
                            self_e
                                .and_then(|e| casting.get(e).ok())
                                .map(|c| c.cast_count)
                        })
                        .unwrap_or(0),
                    Inflight::Strike(_) => queued_melee.count().unwrap_or(0),
                };
                // The cast's item releases the trade's enchanting item ahead of the send (`0x806258`): an item record's caster is the item (`0x80624d`).
                let item = match slot {
                    Inflight::Cast(_) => pending
                        .current()
                        .filter(|r| r.spell_id == spell_id && r.item.is_some())
                        .map(|r| r.caster),
                    Inflight::Strike(_) => None,
                };
                press.cast_item_cancelled(item);
                let _ = net.0.send(ClientCommand::CancelCast { spell_id, count });
                match slot {
                    Inflight::Cast(_) => {
                        if let Some(record) = pending.current().copied() {
                            pending.release_record(&record);
                        }
                        // The cancel ends the cast by its caller's reason, then `STOP` (`0x806200`, `0x806349` -> `0x7fedd0`).
                        if let Some(caster) = self_guid.0 {
                            press.cancelled(caster, reason);
                        }
                        // `spell_failed_other`'s self reap, run early so the echo finds it done.
                        if let Some(e) = self_e {
                            let running = casting.get(e).ok().filter(|c| c.spell_id == spell_id);
                            if running.is_some() {
                                ecs.entity(e).remove::<Casting>();
                            }
                            cast_events.write(CastEvent {
                                entity: e,
                                spell_id,
                                kind: CastEventKind::Fail {
                                    interrupt: false,
                                    wire_reason: reason,
                                    ends_cast: running.is_some(),
                                },
                                cast_count: running.map_or(0, |c| c.cast_count),
                                seq: play_seq.next(),
                            });
                        }
                    }
                    Inflight::Strike(_) => {
                        // Clearing the slot darkens the checked ring now; a queued strike opened no
                        // cast, so no event ends one.
                        queued_melee.clear_if(spell_id);
                    }
                }
            }
        }
        // The channel: movement only, and the send only.
        if moved {
            if let Some(spell_id) = channel.current(now) {
                if flags_open(
                    |d| d.channel_interrupt_flags & CHANNEL_INTERRUPT_MOVING != 0,
                    spell_id,
                ) {
                    if *crate::net::CAST_TRACE {
                        info!("cast-trace: LOCAL channel self-cancel — spell {spell_id}, CMSG_CANCEL_CHANNELLING");
                    }
                    let _ = net.0.send(ClientCommand::CancelChannelling { spell_id });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREBALL: u32 = 133;
    const ME: u64 = 0x10;

    /// A channel whose bar is suppressed still arms [`ActiveChannel`]: the reference writes at cast
    /// send and the channel-start handler never touches it.
    #[test]
    fn suppressing_the_channel_bar_does_not_clear_the_channel_mirror() {
        let t0 = Instant::now();
        // Blood Siphon 24322, a suppressed row.
        let mut channel = ActiveChannel::default();
        channel.start(24322, 8_000, t0);
        assert_eq!(
            channel.current(t0 + Duration::from_secs(1)),
            Some(24322),
            "the wire edge arms the mirror regardless of what the bar does with it"
        );
    }

    #[test]
    fn a_fresh_list_is_empty() {
        let g = PendingCast::default();
        assert!(!g.in_flight(), "nothing sent yet");
    }

    /// An answer releases the record it names by spell and count (`0x805180`); a proc's answer, with count 0 or another spell, leaves ours.
    #[test]
    fn an_answer_releases_only_the_record_it_names() {
        let mut g = PendingCast::default();
        g.file(PendingRecord::player(FIREBALL, 3, ME));
        assert_eq!(g.release(FIREBALL, 0), None, "a proc's go names count 0");
        assert_eq!(g.release(FIREBALL + 1, 3), None, "another spell");
        assert!(g.in_flight());
        assert_eq!(g.release(FIREBALL, 3).map(|r| r.count), Some(3));
        assert!(!g.in_flight());
    }

    /// The current-item test reads every pending item use's entry (`0x805fc0`); a spell cast has none, and an item record's caster is the item.
    #[test]
    fn an_item_records_entry_and_caster_are_the_items() {
        const HEARTH_SPELL: u32 = 8690;
        const HEARTHSTONE: u32 = 6948;
        const ITEM: u64 = 0x4000_0000_0000_0001;
        let mut g = PendingCast::default();
        g.file(PendingRecord {
            item: Some(HEARTHSTONE),
            ..PendingRecord::player(HEARTH_SPELL, 1, ITEM)
        });
        g.file(PendingRecord::player(FIREBALL, 2, ME));
        let items: Vec<u32> = g.records().iter().filter_map(|r| r.item).collect();
        assert_eq!(items, [HEARTHSTONE]);
        assert!(!g.listed_for(HEARTH_SPELL, ME), "the item, not the player");
        assert!(g.listed_for(HEARTH_SPELL, ITEM));
    }

    /// No deadline: a record no answer names stays (`0x80fee0`, `0x809af0`, `0x809c70` alone release), however long, and other spells' records come and go beside it.
    #[test]
    fn an_unanswered_record_stays_beside_others() {
        let mut g = PendingCast::default();
        g.file(PendingRecord::player(FIREBALL, 1, ME));
        g.file(PendingRecord::player(FIREBALL + 1, 2, ME));
        g.release(FIREBALL + 1, 2);
        assert!(g.listed_for(FIREBALL, ME));
        assert_eq!(g.records().len(), 1);
    }

    /// The by-caster walk (`0x805ef0`, `0x806390`) finds and releases one caster's records; a record filed without its caster never matches a caster, and 0 releases nothing.
    #[test]
    fn the_by_caster_walk_keeps_other_casters_records() {
        const PET: u64 = 0x2A;
        let mut g = PendingCast::default();
        g.file(PendingRecord {
            pet: true,
            ..PendingRecord::player(FIREBALL, 1, PET)
        });
        g.file(PendingRecord::player(FIREBALL, 2, 0));
        assert!(g.caster_has(PET));
        assert!(!g.caster_has(0));
        assert!(g.release_caster(0).is_empty());
        assert_eq!(g.release_caster(PET).len(), 1);
        assert_eq!(g.records().len(), 1);
        assert_eq!(g.current().map(|r| r.count), Some(2));
    }

    /// The pet setter's modal cancel (`0x5d6afb`-`0x5d6b24`), on a world with the pet streamed.
    #[test]
    fn the_world_leave_drops_a_record_no_answer_named() {
        use crate::char_select::ClientState;
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::InWorld)
            .init_resource::<PendingCast>()
            .add_systems(OnExit(ClientState::InWorld), clear_on_world_leave);
        app.update();
        let count = {
            let mut pending = app.world_mut().resource_mut::<PendingCast>();
            let count = pending.take_count();
            pending.file(PendingRecord::player(133, count, 7));
            assert!(pending.in_flight());
            count
        };
        app.world_mut()
            .resource_mut::<NextState<ClientState>>()
            .set(ClientState::CharSelect);
        app.update();
        let mut pending = app.world_mut().resource_mut::<PendingCast>();
        assert!(pending.records().is_empty(), "the leave empties the list");
        assert!(pending.current().is_none(), "nothing reads as current");
        assert_ne!(pending.take_count(), count, "the counter runs on");
    }

    mod pet_setter {
        use super::*;
        use crossbeam_channel::Receiver;

        const PET: u64 = 0x2A;
        const OTHER: u64 = 0x2B;
        const CLAW: u32 = 16829;

        fn world(head: &[u64]) -> (World, Receiver<ClientCommand>, Entity) {
            let (tx, rx) = crossbeam_channel::unbounded();
            let mut world = World::new();
            world.insert_resource(NetCommands(tx));
            world.init_resource::<GuidIndex>();
            world.init_resource::<PendingCast>();
            world.init_resource::<crate::ui_script::live::UiEvents>();
            world.insert_resource(crate::ui_pet::PetList(head.to_vec()));
            let pet = world.spawn(Casting {
                spell_id: CLAW,
                cast_count: 4,
                until: None,
                listed: true,
                hidden: false,
            });
            let pet = pet.id();
            world.resource_mut::<GuidIndex>().0.insert(PET, pet);
            (world, rx, pet)
        }

        fn file(world: &mut World, caster: u64, count: u8) {
            world.resource_mut::<PendingCast>().file(PendingRecord {
                pet: true,
                ..PendingRecord::player(CLAW, count, caster)
            });
        }

        /// A new head with a record of its own cancels it: `CMSG_CANCEL_CAST` with its count (`0x8062a6`, `0x8062de`), its cast end (`0x806358`) and its end edges.
        #[test]
        fn a_new_head_with_a_pending_cast_cancels_it() {
            let (mut world, rx, pet) = world(&[PET]);
            file(&mut world, PET, 4);
            pet_setter_cancel(&mut world, true, PET);
            assert!(matches!(
                rx.try_recv(),
                Ok(ClientCommand::CancelCast {
                    spell_id: CLAW,
                    count: 4
                })
            ));
            assert!(world.resource::<PendingCast>().records().is_empty());
            assert!(world.get::<Casting>(pet).is_none());
            assert_eq!(
                world
                    .resource::<crate::ui_script::live::UiEvents>()
                    .cast_edges(),
                [
                    crate::ui_cast::CastBarEdge::Cancelled {
                        caster: PET,
                        reason: 0x1b
                    },
                    crate::ui_cast::CastBarEdge::Selection
                ]
            );
        }

        /// The same pet re-sent, unchanged and still the head, keeps its record (`0x5d6b0b`).
        #[test]
        fn a_resend_of_the_same_pet_keeps_its_record() {
            let (mut world, rx, _) = world(&[PET]);
            file(&mut world, PET, 4);
            pet_setter_cancel(&mut world, false, PET);
            assert!(rx.try_recv().is_err());
            assert_eq!(world.resource::<PendingCast>().records().len(), 1);
        }

        /// Another caster's record survives the change: the walk ends the handed guid's alone (`0x8063af`).
        #[test]
        fn another_casters_record_survives_the_change() {
            let (mut world, rx, _) = world(&[PET]);
            file(&mut world, PET, 4);
            file(&mut world, OTHER, 5);
            world
                .resource_mut::<PendingCast>()
                .file(PendingRecord::player(133, 6, 0x10));
            pet_setter_cancel(&mut world, true, PET);
            assert!(rx.try_iter().count() == 1);
            let left: Vec<u64> = world
                .resource::<PendingCast>()
                .records()
                .iter()
                .map(|r| r.caster)
                .collect();
            assert_eq!(left, [OTHER, 0x10]);
        }

        /// The teardown cancels nothing: the list is empty and no record's caster is the zero guid (`0x5d6afb` with head 0).
        #[test]
        fn the_teardown_cancels_nothing() {
            let (mut world, rx, _) = world(&[]);
            file(&mut world, PET, 4);
            world
                .resource_mut::<PendingCast>()
                .file(PendingRecord::player(133, 6, 0));
            pet_setter_cancel(&mut world, true, 0);
            assert!(rx.try_recv().is_err());
            assert_eq!(world.resource::<PendingCast>().records().len(), 2);
        }
    }

    #[test]
    fn queued_melee_spell_is_wire_cleared_and_single_slot() {
        const HEROIC_STRIKE: u32 = 78;
        const CLEAVE: u32 = 845;
        let mut q = QueuedMeleeSpell::default();
        assert_eq!(q.current(), None);

        q.arm(HEROIC_STRIKE, 1);
        assert_eq!(q.current(), Some(HEROIC_STRIKE), "queued and checked");

        // Rend's GO while Heroic Strike waits.
        q.clear_if(772);
        assert_eq!(q.current(), Some(HEROIC_STRIKE));

        q.arm(CLEAVE, 1);
        assert_eq!(q.current(), Some(CLEAVE));

        q.clear_if(CLEAVE);
        assert_eq!(q.current(), None, "the wire resolution opened the queue");
    }

    /// [`local_self_cancel`] in a small App, and the Esc cancel: the Esc tests insert a VM, call
    /// the real `SpellStopCasting()` and apply what it queued.
    mod local_cancel {
        use super::*;
        use crate::creature_anim::PlaySeq;
        use crate::net::{Guid, GuidIndex, NetCommands, SelfGuid, SelfPlayer};
        use benilla_formats::{SpellCatalog, SpellDisplay};
        use benilla_ui::script::{ScriptCall, UiScript};
        use bevy::ecs::system::RunSystemOnce;
        use std::collections::HashMap;

        const ARCANE_MISSILES: u32 = 5143;
        const HEROIC_STRIKE: u32 = 78;

        /// `SpellStopCasting()` with the world lent, as the press asks it.
        fn stop_casting(app: &mut App) -> Option<i64> {
            crate::ui_script::live::with_live_world(app.world_mut(), |s| {
                s.eval::<Option<i64>>("return SpellStopCasting()")
            })
            .expect("a VM")
            .unwrap()
        }

        /// Our cast running on the unit, which the local cancel ends (`0x806200`).
        fn running_cast() -> crate::ui_cast::UnitCasts {
            let mut casts = crate::ui_cast::UnitCasts::default();
            casts.run_for_test(10, FIREBALL);
            casts
        }

        fn harness(
            displays: HashMap<u32, SpellDisplay>,
        ) -> (App, crossbeam_channel::Receiver<crate::net::ClientCommand>) {
            let (tx, rx) = crossbeam_channel::unbounded();
            let mut app = App::new();
            app.add_message::<CastEvent>()
                .init_resource::<crate::ui_script::live::UiEvents>()
                .init_resource::<crate::ui_cast::PressEdges>()
                .init_resource::<crate::ui_script::UiClock>()
                .insert_resource(running_cast())
                .init_resource::<crate::names::NameCache>()
                .init_resource::<PendingCast>()
                .init_resource::<QueuedMeleeSpell>()
                .init_resource::<ActiveChannel>()
                .init_resource::<AutoRepeatActive>()
                .init_resource::<LocalMoveStart>()
                .init_resource::<GuidIndex>()
                .init_resource::<SelfGuid>()
                .init_resource::<PlaySeq>()
                .insert_resource(NetCommands(tx))
                .insert_resource(crate::ui_action::Spells {
                    catalog: SpellCatalog::from_displays(displays),
                    ..crate::ui_action::Spells::empty_for_tests()
                });
            let self_e = app.world_mut().spawn((Guid(10), SelfPlayer)).id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(10, self_e);
            app.world_mut().resource_mut::<SelfGuid>().0 = Some(10);
            (app, rx)
        }

        /// The `Casting` an `SMSG_SPELL_START` echo leaves on the self player.
        fn mark_casting(app: &mut App, spell_id: u32) {
            let self_e = app.world().resource::<GuidIndex>().0[&10];
            app.world_mut().entity_mut(self_e).insert(Casting {
                spell_id,
                cast_count: 0,
                until: None,
                listed: true,
                hidden: false,
            });
        }

        fn run(app: &mut App, moved: bool) {
            app.world_mut().resource_mut::<LocalMoveStart>().0 = moved;
            app.world_mut().run_system_once(local_self_cancel).unwrap();
        }

        /// Apply the `SpellStopCasting()` calls the VM queued, as `crate::script_calls` does.
        fn apply_stops(app: &mut App) {
            app.world_mut()
                .run_system_once(|mut script: NonSendMut<UiScript>, mut cancel: SelfCancel| {
                    crate::script_calls::in_call_order(&mut script, |script, call| {
                        if call == ScriptCall::SpellStopCasting {
                            cancel.stop_casting();
                            // The cancel's bar end, fired as the call returns (`0x806200`).
                            cancel.press.fire(script, &mut Vec::new());
                        }
                    });
                })
                .unwrap();
        }

        #[test]
        fn a_move_edge_mid_cast_cancels_locally() {
            let (mut app, rx) = harness(HashMap::from([(
                FIREBALL,
                SpellDisplay {
                    interrupt_flags: 0xf, // the ordinary timed-cast mask (movement bit set)
                    ..Default::default()
                },
            )]));
            app.world_mut()
                .resource_mut::<PendingCast>()
                .arm(FIREBALL, 1);
            mark_casting(&mut app, FIREBALL);

            run(&mut app, true);

            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast {
                        spell_id: FIREBALL,
                        ..
                    })
                ),
                "the move edge ships CMSG_CANCEL_CAST for the in-flight cast"
            );
            assert!(
                !app.world().resource::<PendingCast>().in_flight(),
                "the in-flight guard opens with the local cancel"
            );
            assert!(
                !app.world()
                    .resource::<crate::ui_cast::UnitCasts>()
                    .casting(10),
                "the cancel ends our running cast locally (`0x806200`): INTERRUPTED, then STOP"
            );
            let self_e = app.world().resource::<GuidIndex>().0[&10];
            assert!(
                app.world().entity(self_e).get::<Casting>().is_none(),
                "the self `Casting` state is reaped, so the server's echo (FAILED_OTHER + \
                 CAST_RESULT) finds nothing to repaint"
            );
            assert!(
                !app.world().resource::<LocalMoveStart>().0,
                "the edge is consumed"
            );
        }

        /// `InterruptFlags` without the movement bit keeps casting.
        #[test]
        fn the_interrupt_flags_gate_spares_a_movement_castable_spell() {
            let (mut app, rx) = harness(HashMap::from([(
                FIREBALL,
                SpellDisplay {
                    interrupt_flags: 0xe, // movement bit clear
                    ..Default::default()
                },
            )]));
            app.world_mut()
                .resource_mut::<PendingCast>()
                .arm(FIREBALL, 1);

            run(&mut app, true);

            assert!(rx.try_recv().is_err(), "no wire cancel for a spared spell");
            assert!(
                app.world().resource::<PendingCast>().in_flight(),
                "the guard keeps holding — the cast runs on"
            );
            assert!(app
                .world()
                .resource::<crate::ui_cast::UnitCasts>()
                .casting(10));
        }

        #[test]
        fn an_uncataloged_spell_fails_open() {
            let (mut app, rx) = harness(HashMap::new());
            app.world_mut()
                .resource_mut::<PendingCast>()
                .arm(FIREBALL, 1);

            run(&mut app, true);

            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast {
                        spell_id: FIREBALL,
                        ..
                    })
                ),
                "an unknown spell's cast still cancels on movement"
            );
        }

        /// A started cast with no send guard still cancels through its `Casting`.
        #[test]
        fn a_started_item_cast_cancels_without_the_send_guard() {
            let (mut app, rx) = harness(HashMap::from([(
                FIREBALL,
                SpellDisplay {
                    interrupt_flags: 0xf,
                    ..Default::default()
                },
            )]));
            mark_casting(&mut app, FIREBALL); // PendingCast stays empty

            run(&mut app, true);

            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast {
                        spell_id: FIREBALL,
                        ..
                    })
                ),
                "the started cast cancels through the `Casting` half of the union"
            );
            let self_e = app.world().resource::<GuidIndex>().0[&10];
            assert!(app.world().entity(self_e).get::<Casting>().is_none());
        }

        #[test]
        fn no_edge_means_no_cancel() {
            let (mut app, rx) = harness(HashMap::from([(
                FIREBALL,
                SpellDisplay {
                    interrupt_flags: 0xf,
                    ..Default::default()
                },
            )]));
            app.world_mut()
                .resource_mut::<PendingCast>()
                .arm(FIREBALL, 1);

            run(&mut app, false);

            assert!(rx.try_recv().is_err());
            assert!(app.world().resource::<PendingCast>().in_flight());
            assert!(app
                .world()
                .resource::<crate::ui_cast::UnitCasts>()
                .casting(10));
        }

        /// Mid-channel `SpellStopCasting()` answers nil and nothing is sent.
        #[test]
        fn esc_cannot_stop_a_channel() {
            let (mut app, rx) = harness(HashMap::new());
            app.insert_non_send_resource(UiScript::new().unwrap());
            let now = Instant::now();
            app.world_mut()
                .resource_mut::<ActiveChannel>()
                .start(ARCANE_MISSILES, 5_000, now);

            assert_eq!(
                stop_casting(&mut app),
                None,
                "mid-channel the binding answers nil — the vanilla /stopcasting quirk"
            );
            apply_stops(&mut app);
            assert!(
                rx.try_recv().is_err(),
                "no wire cancel — channels break only on the movement path"
            );
            assert_eq!(
                app.world().resource::<ActiveChannel>().current(now),
                Some(ARCANE_MISSILES)
            );
        }

        /// `SpellStopCasting` stops the auto-repeat first and spends the press.
        #[test]
        fn esc_cancels_the_auto_repeat_first_and_the_cast_on_the_next_press() {
            const AUTO_SHOT: u32 = 75;
            let (mut app, rx) = harness(HashMap::new());
            app.insert_non_send_resource(UiScript::new().unwrap());
            app.world_mut().resource_mut::<AutoRepeatActive>().0 = Some(AUTO_SHOT);
            app.world_mut()
                .resource_mut::<PendingCast>()
                .arm(FIREBALL, 1);
            mark_casting(&mut app, FIREBALL);

            // Press 1 stops the auto-repeat only.
            assert_eq!(stop_casting(&mut app), Some(1));
            apply_stops(&mut app);
            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelAutoRepeat)
                ),
                "the auto-repeat cancel ships first — the reference's priority branch"
            );
            assert!(rx.try_recv().is_err(), "the cast survives the first press");
            assert!(app.world().resource::<AutoRepeatActive>().0.is_none());
            assert!(
                app.world().resource::<PendingCast>().in_flight(),
                "the send guard still holds — one press stopped one thing"
            );
            assert!(
                app.world()
                    .resource::<crate::ui_cast::UnitCasts>()
                    .casting(10),
                "an auto-repeat cancel paints no bar edge"
            );

            // Press 2 cancels the cast.
            assert_eq!(stop_casting(&mut app), Some(1));
            apply_stops(&mut app);
            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast {
                        spell_id: FIREBALL,
                        ..
                    })
                ),
                "the second press reaches the cast half"
            );
        }

        /// The cancel of a cast an item started releases the trade's enchanting item ahead of the cast's end (`0x806258`); another item's cast leaves it locked (`0x80624d`).
        #[test]
        fn cancelling_an_items_cast_releases_the_trades_enchanting_item() {
            use benilla_ui::script::{
                ContainerSlot, ContainerState, TradeCells, TradeItemFacts, TradeTargetItem,
            };
            const SCROLL: u64 = 0x4000_0000_0000_0001;
            let cancelled = |cast_item: u64| {
                let (mut app, _rx) = harness(HashMap::new());
                let mut script = UiScript::new().unwrap();
                let slot = ContainerSlot {
                    guid: SCROLL,
                    item_id: 38_682,
                    count: 1,
                    ..Default::default()
                };
                script.set_container(
                    0,
                    Some(ContainerState {
                        bag_family: 0,
                        name: None,
                        num_slots: 16,
                        slots: HashMap::from([(1, slot)]),
                    }),
                );
                // A partner whose seventh slot holds an item (`0x5d897e`).
                script.open_trade_partner(0x7, false);
                let mut target = [TradeTargetItem::default(); 7];
                target[6].item_id = 6217;
                script.trade_updated(target, TradeCells::default());
                let facts = TradeItemFacts {
                    enchants_on_use: true,
                    ..Default::default()
                };
                script.set_trade_item_facts(0, 1, facts);
                script.run("UseContainerItem(0, 1)").unwrap();
                script.take_script_calls();
                script
                    .run(
                        "LOG = {} local f = CreateFrame('Frame') f:RegisterEvent('ITEM_UNLOCKED') \
                         f:RegisterEvent('UNIT_SPELLCAST_FAILED_QUIET') \
                         f:SetScript('OnEvent', function(_, e) LOG[#LOG + 1] = e end)",
                    )
                    .unwrap();
                app.insert_non_send_resource(script);
                app.world_mut()
                    .resource_mut::<PendingCast>()
                    .file(PendingRecord {
                        item: Some(38_682),
                        ..PendingRecord::player(FIREBALL, 1, cast_item)
                    });
                mark_casting(&mut app, FIREBALL);
                let edges = app
                    .world_mut()
                    .run_system_once(|mut cancel: SelfCancel| {
                        cancel.cancel_player_spells();
                        let edges = cancel.press.pending().to_vec();
                        cancel.press.flush(&mut cancel.ecs);
                        edges
                    })
                    .unwrap();
                let log = app
                    .world()
                    .non_send_resource::<UiScript>()
                    .eval::<String>("return table.concat(LOG, ' ')")
                    .unwrap();
                (edges, log)
            };
            use crate::ui_cast::CastBarEdge;
            let end = [
                // The controls clear's reason (`0x8063c1`).
                CastBarEdge::Cancelled {
                    caster: 10,
                    reason: 0x1b,
                },
                CastBarEdge::Selection,
            ];
            let (edges, log) = cancelled(SCROLL);
            assert_eq!(edges[0], CastBarEdge::CastItemCancelled(SCROLL));
            assert_eq!(edges[1..], end, "the release is ahead of the cast's end");
            assert_eq!(
                log, "ITEM_UNLOCKED UNIT_SPELLCAST_FAILED_QUIET",
                "the unlock fires ahead of the cast's end"
            );
            let (edges, log) = cancelled(SCROLL + 1);
            assert_eq!(edges[0], CastBarEdge::CastItemCancelled(SCROLL + 1));
            assert_eq!(
                log, "UNIT_SPELLCAST_FAILED_QUIET",
                "another item's cast leaves it locked"
            );
        }

        /// The client-controls clear cancels the cast whatever its `InterruptFlags`, and leaves a
        /// running auto-repeat alone.
        #[test]
        fn the_controls_clear_cancels_the_cast_past_auto_repeat_and_flags() {
            const AUTO_SHOT: u32 = 75;
            let (mut app, rx) = harness(HashMap::from([(
                FIREBALL,
                SpellDisplay {
                    interrupt_flags: 0,
                    ..Default::default()
                },
            )]));
            app.world_mut().resource_mut::<AutoRepeatActive>().0 = Some(AUTO_SHOT);
            app.world_mut()
                .resource_mut::<PendingCast>()
                .arm(FIREBALL, 1);
            mark_casting(&mut app, FIREBALL);

            app.world_mut()
                .run_system_once(|mut cancel: SelfCancel| {
                    cancel.cancel_player_spells();
                    cancel.press.flush(&mut cancel.ecs);
                })
                .unwrap();
            assert!(matches!(
                rx.try_recv(),
                Ok(crate::net::ClientCommand::CancelCast {
                    spell_id: FIREBALL,
                    ..
                })
            ));
            assert!(rx.try_recv().is_err(), "no auto-repeat cancel");
            assert_eq!(
                app.world().resource::<AutoRepeatActive>().0,
                Some(AUTO_SHOT)
            );
            assert!(!app
                .world()
                .resource::<crate::ui_cast::UnitCasts>()
                .casting(10));
        }

        /// Esc with a strike queued cancels it and returns 1 (the in-flight check is true), so the
        /// Esc ladder stops before `ClearTarget()`.
        #[test]
        fn esc_unqueues_a_strike_and_eats_the_press() {
            let (mut app, rx) = harness(HashMap::from([(
                HEROIC_STRIKE,
                SpellDisplay {
                    attributes: 0x5_0014, // the shipped row: on-next-swing (`& 0x404`)
                    interrupt_flags: 0,   // no movement bit, also shipped
                    ..Default::default()
                },
            )]));
            app.insert_non_send_resource(UiScript::new().unwrap());
            app.world_mut()
                .resource_mut::<QueuedMeleeSpell>()
                .arm(HEROIC_STRIKE, 1);

            assert_eq!(
                stop_casting(&mut app),
                Some(1),
                "the queued strike sets the in-flight check, so the binding EATS the press — which \
                 is what spares the target three rungs further down the ladder"
            );
            apply_stops(&mut app);
            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast {
                        spell_id: HEROIC_STRIKE,
                        ..
                    })
                ),
                "the cast abort names the queued strike on the wire, exactly as it would a cast"
            );
            assert_eq!(
                app.world().resource::<QueuedMeleeSpell>().current(),
                None,
                "the slot opens locally — the checked ring darkens this frame, not one RTT later"
            );
            assert!(
                app.world()
                    .resource::<crate::ui_cast::UnitCasts>()
                    .casting(10),
                "a queued strike opened no cast, so no event ends one"
            );
        }

        /// Heroic Strike's `InterruptFlags` are 0, so movement leaves it queued.
        #[test]
        fn a_move_edge_leaves_a_queued_strike_queued() {
            let (mut app, rx) = harness(HashMap::from([(
                HEROIC_STRIKE,
                SpellDisplay {
                    interrupt_flags: 0,
                    ..Default::default()
                },
            )]));
            app.world_mut()
                .resource_mut::<QueuedMeleeSpell>()
                .arm(HEROIC_STRIKE, 1);

            run(&mut app, true);

            assert!(rx.try_recv().is_err(), "running at the mob cancels nothing");
            assert_eq!(
                app.world().resource::<QueuedMeleeSpell>().current(),
                Some(HEROIC_STRIKE)
            );
        }

        /// A cast nested over a queued strike (the nested-cast push and pop) dies on the first Esc,
        /// the strike on the second; this pins the local order only.
        #[test]
        fn esc_takes_the_cast_first_and_the_strike_on_the_next_press() {
            let (mut app, rx) = harness(HashMap::new());
            app.insert_non_send_resource(UiScript::new().unwrap());
            app.world_mut()
                .resource_mut::<QueuedMeleeSpell>()
                .arm(HEROIC_STRIKE, 1);
            app.world_mut()
                .resource_mut::<PendingCast>()
                .arm(FIREBALL, 1);
            mark_casting(&mut app, FIREBALL);

            assert_eq!(stop_casting(&mut app), Some(1));
            apply_stops(&mut app);
            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelCast {
                        spell_id: FIREBALL,
                        ..
                    })
                ),
                "the nested cast is what sits in the inflight slot — it dies first"
            );
            assert!(rx.try_recv().is_err());
            assert_eq!(
                app.world().resource::<QueuedMeleeSpell>().current(),
                Some(HEROIC_STRIKE),
                "the strike is the SAVE underneath, untouched by the first press"
            );

            // Press 2: the pop put the strike back in the slot.
            assert_eq!(
                stop_casting(&mut app),
                Some(1),
                "still stoppable — so this press is eaten too, and the target still survives"
            );
            apply_stops(&mut app);
            assert!(matches!(
                rx.try_recv(),
                Ok(crate::net::ClientCommand::CancelCast {
                    spell_id: HEROIC_STRIKE,
                    ..
                })
            ));
            assert_eq!(app.world().resource::<QueuedMeleeSpell>().current(), None);
        }

        /// The reference sends the cancel and fires no event, clearing nothing local.
        #[test]
        fn a_move_edge_mid_channel_sends_the_cancel_and_touches_nothing_local() {
            let (mut app, rx) = harness(HashMap::from([(
                ARCANE_MISSILES,
                SpellDisplay {
                    channel_interrupt_flags: 0x7c0c, // the real 5143 mask (moving bit 0x8 set)
                    ..Default::default()
                },
            )]));
            let now = Instant::now();
            app.world_mut()
                .resource_mut::<ActiveChannel>()
                .start(ARCANE_MISSILES, 5_000, now);

            run(&mut app, true);

            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(crate::net::ClientCommand::CancelChannelling {
                        spell_id: ARCANE_MISSILES
                    })
                ),
                "the move edge ships CMSG_CANCEL_CHANNELLING"
            );
            assert_eq!(
                app.world().resource::<ActiveChannel>().current(now),
                Some(ARCANE_MISSILES),
                "the mirror stays live — the server's UPDATE(0) is what clears it"
            );
            assert!(
                app.world()
                    .resource::<crate::ui_cast::UnitCasts>()
                    .casting(10),
                "no local bar edge — the bar closes on the wire's UNIT_SPELLCAST_CHANNEL_STOP"
            );
        }
    }

    #[test]
    fn active_channel_tracks_the_wire_lifecycle() {
        let t0 = Instant::now();
        let mut ch = ActiveChannel::default();
        assert_eq!(ch.current(t0), None);

        ch.start(10, 5_000, t0); // a 5 s channel
        assert_eq!(ch.current(t0), Some(10));
        assert_eq!(
            ch.current(t0 + Duration::from_secs(4)),
            Some(10),
            "still live near the end"
        );
        assert_eq!(
            ch.current(t0 + Duration::from_secs(8)),
            None,
            "the slack deadline self-clears a lost UPDATE(0)"
        );

        // Pushback: an update naming less time moves the deadline in.
        ch.start(10, 5_000, t0);
        ch.update(1_000, t0);
        assert_eq!(ch.current(t0 + Duration::from_secs(4)), None);

        ch.start(10, 5_000, t0);
        ch.update(0, t0);
        assert_eq!(ch.current(t0), None);
    }
}
