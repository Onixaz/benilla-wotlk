//! The unit-cast feed: every unit's cast and channel edges, off its packets, become the
//! `UNIT_SPELLCAST_*` events 3.3.5a's `CastingBarFrame.lua:6-15` registers for, fanned out over the
//! tokens naming the caster (`0x60bf60`: unit, name, rank, castCount), and the cast state
//! `UnitCastingInfo` and `UnitChannelInfo` read. The VM's casting flag is pushed each frame. The
//! own-cast lifecycle it reads is `crate::spell`'s.
//!
//! A cast's end runs one path in the reference (`0x7fecc0`): `SUCCEEDED` for a launch (result
//! `0xbb`), else `FAILED_QUIET`, `INTERRUPTED` or `FAILED` by reason, then `STOP` when it was the
//! unit's running cast (`castCount` and spell, `+0xa5c`/`+0xa6c`).

use std::collections::HashMap;
use std::time::Instant;

use benilla_formats::SpellDisplay;
use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript, UnitCast};

use crate::creature_anim::Casting;
use crate::net::{GuidIndex, NetCommands, ObjectStore, SelfGuid};
use crate::spell::{inflight, PendingCast, QueuedMeleeSpell};
use crate::ui_action::Spells;
use crate::ui_unit::UnitFeed;

/// One edge of a unit's cast, queued by the spell's packet handlers, the cast send and the
/// local cancel.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CastBarEdge {
    /// Our `CMSG_CAST_SPELL` went out (`UNIT_SPELLCAST_SENT`, `0x80b556`), at a unit or none.
    Sent { spell_id: u32, target: Option<u64> },
    /// `SMSG_SPELL_START`; an instant (`cast_time_ms == 0`) opens no cast (`0x805409`).
    Start {
        caster: u64,
        spell_id: u32,
        cast_count: u8,
        cast_time_ms: u32,
        /// The `CAST_FLAG_IMMUNITY` pair, `(school, mechanic)`, kept on the caster until the next.
        immunities: Option<(u32, u32)>,
    },
    /// `SMSG_SPELL_GO`: the cast launched; `cast_flags & 1` fires nothing (`0x80e747`).
    Go {
        caster: u64,
        spell_id: u32,
        cast_count: u8,
        cast_flags: u32,
    },
    /// `SMSG_CAST_FAILED`, `SMSG_SPELL_FAILURE`, `SMSG_SPELL_FAILED_OTHER`, or our own cancel;
    /// `reason` in the shared numbering.
    Failed {
        caster: u64,
        spell_id: u32,
        cast_count: u8,
        reason: u8,
    },
    /// Our own move or Esc cancel (`0x806200`, reason `0x28`): the running cast ends interrupted.
    Cancelled { caster: u64 },
    /// `SMSG_SPELL_DELAYED`: pushback moves the running cast's end out by `delay_ms`.
    Delayed { caster: u64, delay_ms: u32 },
    /// `MSG_CHANNEL_START`.
    ChannelStart {
        caster: u64,
        spell_id: u32,
        duration_ms: u32,
    },
    /// `MSG_CHANNEL_UPDATE`: the time left, 0 when the channel is over.
    ChannelUpdate { caster: u64, remaining_ms: u32 },
}

/// The queue of [`CastBarEdge`]s, drained by [`feed_cast_bar`].
#[derive(Resource, Default)]
pub(crate) struct CastBarFeed(pub(crate) Vec<CastBarEdge>);

/// Queue `UNIT_SPELLCAST_SENT` from a cast send, which holds no feed borrow: the reference fires it
/// beside `CMSG_CAST_SPELL` (`0x80b556`).
pub(crate) fn queue_sent(ecs: &mut Commands, spell_id: u32, target: Option<u64>) {
    ecs.queue(move |world: &mut World| {
        if let Some(mut feed) = world.get_resource_mut::<CastBarFeed>() {
            feed.0.push(CastBarEdge::Sent { spell_id, target });
        }
    });
}

/// Each unit's running cast and channel as the feed tracks them: `(spell, castCount)` and the
/// window on the `GetTime` clock in ms.
#[derive(Resource, Default)]
pub(crate) struct UnitCasts {
    casts: HashMap<u64, Running>,
    channels: HashMap<u64, Running>,
    /// Each unit's state bit `+0xa34 & 8`, set by `ProcessCastNotInterruptible` (`0x7262e0`) and
    /// kept until its next evaluation.
    not_interruptible: HashMap<u64, bool>,
    /// Each caster's last cast immunities, `(school, mechanic)` (`+0xa64`, `+0xa68`).
    immunities: HashMap<u64, (u32, u32)>,
    /// The player's four school masks over its known spells.
    schools: InterruptSchools,
    /// The target and focus the bit was last evaluated for.
    watched: [Option<u64>; 2],
}

impl UnitCasts {
    /// `IsCastNotInterruptible` (`0x71ab20`): the unit's bit, and any of the player's masks, the
    /// two specials only against a non-player.
    fn reads_not_interruptible(&self, caster: u64) -> bool {
        self.not_interruptible
            .get(&caster)
            .copied()
            .unwrap_or(false)
            && self.schools.against(is_player(caster)).any()
    }
}

/// A unit guid of the player kind.
fn is_player(guid: u64) -> bool {
    benilla_protocol::guid::is_player(guid)
}

/// `CGSpellBook::UpdateSpells`' four school masks (`0x53cd48`-`0x53ce75`) over the known spells:
/// an interrupt (effect 68), a silence (an aura 27), and the `AttributesEx7` specials `0x800` and
/// `0x1000`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct InterruptSchools {
    interrupt: u32,
    silence: u32,
    interrupt_special: u32,
    silence_special: u32,
}

impl InterruptSchools {
    pub(crate) fn of<'a>(known: impl IntoIterator<Item = &'a SpellDisplay>) -> Self {
        let mut m = Self::default();
        for d in known {
            for j in 0..3 {
                match d.effects[j] {
                    6 if d.effect_apply_aura[j] == 27 => m.silence |= d.school_mask,
                    68 => m.interrupt |= d.school_mask,
                    _ => {}
                }
            }
            if d.attributes_ex7 & 0x800 != 0 {
                m.interrupt_special |= d.school_mask;
            }
            if d.attributes_ex7 & 0x1000 != 0 {
                m.silence_special |= d.school_mask;
            }
        }
        m
    }

    /// The masks against a caster: the specials count only against a non-player (`0x71ab30`).
    fn against(mut self, caster_is_player: bool) -> Self {
        if caster_is_player {
            self.interrupt_special = 0;
            self.silence_special = 0;
        }
        self
    }

    fn any(self) -> bool {
        self.interrupt | self.silence | self.interrupt_special | self.silence_special != 0
    }

    fn clear_interrupt(&mut self) {
        self.interrupt = 0;
        self.interrupt_special = 0;
    }

    fn clear_silence(&mut self) {
        self.silence = 0;
        self.silence_special = 0;
    }

    fn clear_schools(&mut self, schools: u32) {
        self.interrupt &= !schools;
        self.interrupt_special &= !schools;
        self.silence &= !schools;
        self.silence_special &= !schools;
    }
}

/// `ProcessCastNotInterruptible` (`0x7262e0`): the bit for a cast of `d`. Never under
/// `AttributesEx7 & 0x2000`; always for a spell no silence prevents (`PreventionType != 1`);
/// otherwise when the player's masks against the caster are left empty by its cast immunities
/// (mechanic 26 interrupt, 9 silence, else the school mask) and its aura immunities (37 to 68, 38
/// to 27, 39 by school, 77 to 9 or 26). Masks empty from the start leave the bit clear.
fn not_interruptible_bit<'a>(
    d: &SpellDisplay,
    schools: InterruptSchools,
    have_player: bool,
    (school_imm, mechanic_imm): (u32, u32),
    aura_spells: impl IntoIterator<Item = &'a SpellDisplay>,
) -> bool {
    if d.attributes_ex7 & 0x2000 != 0 {
        return false;
    }
    if !have_player || d.prevention_type != 1 {
        return true;
    }
    let mut m = schools;
    if !m.any() {
        return false;
    }
    if mechanic_imm & 0x0400_0000 != 0 {
        m.clear_interrupt();
    } else if school_imm != 0 {
        m.interrupt &= !school_imm;
        m.interrupt_special &= !school_imm;
    }
    if mechanic_imm & 0x200 != 0 {
        m.clear_silence();
    } else if school_imm != 0 {
        m.silence &= !school_imm;
        m.silence_special &= !school_imm;
    }
    for aura in aura_spells {
        if !m.any() {
            break;
        }
        for j in 0..3 {
            let misc = aura.effect_misc_value[j];
            match aura.effect_apply_aura[j] {
                37 if misc == 68 => m.clear_interrupt(),
                38 if misc == 27 => m.clear_silence(),
                39 => m.clear_schools(misc as u32),
                77 if misc == 9 => m.clear_silence(),
                77 if misc == 26 => m.clear_interrupt(),
                _ => {}
            }
        }
    }
    !m.any()
}

#[derive(Clone, Copy, Debug)]
struct Running {
    spell_id: u32,
    cast_count: u8,
    start_ms: f64,
    end_ms: f64,
}

/// Whether a cast or channel raises its events, the gate both the start (`0x80540f`) and the
/// channel handlers (`0x801cf3`, `0x801e27`) apply: `AttributesEx4 & 0x8000000` always passes,
/// `Attributes & 2` (a ranged-slot spell) never does, the active player passes, and anyone else
/// fails on `Attributes & 0x180` or `AttributesEx4 & 1`, and a channel also on
/// `AttributesEx3 & 0x2000`.
fn raises_events(d: &SpellDisplay, is_self: bool, channel: bool) -> bool {
    if d.attributes_ex4 & 0x0800_0000 != 0 {
        return true;
    }
    if d.attributes & 0x2 != 0 {
        return false;
    }
    if is_self {
        return true;
    }
    d.attributes & 0x180 == 0
        && d.attributes_ex4 & 0x1 == 0
        && !(channel && d.attributes_ex3 & 0x2000 != 0)
}

/// The event an ended cast raises by its reason (`0x7fedd0`-`0x7fee0f`): `DONT_REPORT`,
/// `SPELL_IN_PROGRESS` and `CHARMED` are quiet, the two interrupts are `INTERRUPTED`, the rest
/// `FAILED`.
fn end_event(reason: u8) -> &'static str {
    match crate::ui_action::cast_fail::CAST_FAIL_KEYS
        .get(usize::from(reason))
        .copied()
        .unwrap_or("")
    {
        "SPELL_FAILED_DONT_REPORT" | "SPELL_FAILED_SPELL_IN_PROGRESS" | "SPELL_FAILED_CHARMED" => {
            "UNIT_SPELLCAST_FAILED_QUIET"
        }
        "SPELL_FAILED_INTERRUPTED" | "SPELL_FAILED_INTERRUPTED_COMBAT" => {
            "UNIT_SPELLCAST_INTERRUPTED"
        }
        _ => "UNIT_SPELLCAST_FAILED",
    }
}

/// The spell's `(name, rank)`, `("", "")` for an id with no row.
fn name_rank(spells: Option<&Spells>, id: u32) -> (String, String) {
    spells
        .and_then(|s| s.catalog.get(id))
        .map(|d| (d.name.clone(), d.rank.clone().unwrap_or_default()))
        .unwrap_or_default()
}

/// The event arguments after the token: name, rank, castCount.
fn spell_args(spells: Option<&Spells>, id: u32, cast_count: u8) -> Vec<ScriptValue> {
    let (name, rank) = name_rank(spells, id);
    vec![
        ScriptValue::Str(name),
        ScriptValue::Str(rank),
        ScriptValue::Int(i64::from(cast_count)),
    ]
}

/// The cast as `UnitCastingInfo`/`UnitChannelInfo` read it. A cast's text is its name, empty under
/// `AttributesEx3 & 4` (`0x611e81`); a channel's, as the `"player"` token reads it, is its name
/// only with `AttributesEx` bit 29 (`0x612151`), else the Lua global `CHANNELING`.
fn unit_cast(
    script: &UiScript,
    spells: Option<&Spells>,
    run: Running,
    channel: bool,
    not_interruptible: bool,
) -> UnitCast {
    let display = spells.and_then(|s| s.catalog.get(run.spell_id));
    let (name, rank) = name_rank(spells, run.spell_id);
    let text = match display {
        Some(d) if channel && !d.channel_bar_own_name() => script
            .lua()
            .globals()
            .get::<String>("CHANNELING")
            .unwrap_or_default(),
        Some(d) if !channel && d.no_casting_bar_text() => String::new(),
        _ => name.clone(),
    };
    UnitCast {
        name,
        rank,
        text,
        texture: display.and_then(|d| d.icon.clone()),
        start_ms: run.start_ms,
        end_ms: run.end_ms,
        // `Attributes & 0x20` (`0x611fa8`).
        trade_skill: display.is_some_and(|d| d.attributes & 0x20 != 0),
        cast_count: run.cast_count,
        not_interruptible,
    }
}

/// What the feed reads to name `UNIT_SPELLCAST_SENT`'s target.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SentTarget<'w, 's> {
    names: Res<'w, crate::names::NameCache>,
    commands: Res<'w, NetCommands>,
    stores: Query<'w, 's, &'static ObjectStore>,
}

/// Drain the queue into the `UNIT_SPELLCAST_*` events and the VM's cast state, then push the flag
/// `SpellStopCasting` answers from: auto-repeat or the in-flight slot, never a channel, taken
/// after the local cancel. A queued strike counts, so the first Esc stops it and
/// `ToggleGameMenu` skips `ClearTarget()` (`UIParent.lua:1489-1492`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn feed_cast_bar(
    script: Option<NonSendMut<UiScript>>,
    mut feed: ResMut<CastBarFeed>,
    mut units: ResMut<UnitCasts>,
    spells: Option<Res<Spells>>,
    pending: Res<PendingCast>,
    queued_melee: Res<QueuedMeleeSpell>,
    auto_repeat: Res<crate::spell::AutoRepeatActive>,
    self_guid: Res<SelfGuid>,
    index: Res<GuidIndex>,
    casting: Query<&Casting>,
    sent: SentTarget,
    shield: ShieldInputs,
) {
    let Some(mut script) = script else {
        return;
    };
    let now = Instant::now();
    if let (Some(actions), Some(s)) = (shield.actions.as_ref(), spells.as_ref()) {
        if actions.is_changed() || s.is_changed() {
            units.schools =
                InterruptSchools::of(actions.spells.iter().filter_map(|&id| s.catalog.get(id)));
        }
    }
    let target = shield.selection.as_ref().and_then(|s| s.guid);
    let focus = Some(script.focus_guid()).filter(|&g| g != 0);
    let watched = [target, focus];
    // The auto-repeat key (`0xceac30`), which the reference reads first, then the in-flight slot
    // the cancel resolves, so the flag and the cancel agree.
    let started = self_guid
        .0
        .as_ref()
        .and_then(|g| index.0.get(g))
        .and_then(|&e| casting.get(e).ok())
        .map(|c| c.spell_id);
    script.set_casting(
        auto_repeat.0.is_some() || inflight(&pending, &queued_melee, started, now).is_some(),
    );
    let spells = spells.as_deref();
    let now_ms = script.now() * 1000.0;
    let is_self = |guid: u64| self_guid.0 == Some(guid);
    for edge in feed.0.drain(..) {
        if *crate::net::CAST_TRACE {
            info!("cast-trace: UNIT_SPELLCAST edge {edge:?}");
        }
        match edge {
            CastBarEdge::Sent { spell_id, target } => {
                // `("player", name, rank, target)`, the target's name or `""` (`0x80b533`).
                let target = target
                    .and_then(|g| {
                        let store = index.0.get(&g).and_then(|&e| sent.stores.get(e).ok());
                        sent.names.resolve_unit(g, store, &sent.commands)
                    })
                    .unwrap_or_default()
                    .to_string();
                let (name, rank) = name_rank(spells, spell_id);
                script.fire_event(
                    "UNIT_SPELLCAST_SENT",
                    vec![
                        ScriptValue::Str("player".into()),
                        ScriptValue::Str(name),
                        ScriptValue::Str(rank),
                        ScriptValue::Str(target),
                    ],
                );
            }
            CastBarEdge::Start {
                caster,
                spell_id,
                cast_count,
                cast_time_ms,
                immunities,
            } => {
                // Stored before any gate, and kept until the next start that carries a pair.
                if let Some(pair) = immunities {
                    units.immunities.insert(caster, pair);
                }
                let raises = spells
                    .and_then(|s| s.catalog.get(spell_id))
                    .is_none_or(|d| raises_events(d, is_self(caster), false));
                if cast_time_ms == 0 || !raises {
                    continue;
                }
                let run = Running {
                    spell_id,
                    cast_count,
                    start_ms: now_ms,
                    end_ms: now_ms + f64::from(cast_time_ms),
                };
                units.casts.insert(caster, run);
                // `SetUICasting` evaluates the bit for the target and focus only (`0x72ac50`).
                if watched.contains(&Some(caster)) {
                    shield.evaluate(
                        &mut units,
                        spells,
                        &index,
                        self_guid.0.is_some(),
                        caster,
                        spell_id,
                    );
                }
                let not = units.reads_not_interruptible(caster);
                let cast = unit_cast(&script, spells, run, false, not);
                script.set_unit_cast(caster, false, Some(cast));
                script.fire_unit_event(
                    caster,
                    "UNIT_SPELLCAST_START",
                    spell_args(spells, spell_id, cast_count),
                );
            }
            CastBarEdge::Go {
                caster,
                spell_id,
                cast_count,
                cast_flags,
            } => {
                if cast_flags & 1 != 0 {
                    continue;
                }
                end_cast(
                    &mut script,
                    &mut units,
                    spells,
                    caster,
                    spell_id,
                    cast_count,
                    "UNIT_SPELLCAST_SUCCEEDED",
                );
            }
            CastBarEdge::Failed {
                caster,
                spell_id,
                cast_count,
                reason,
            } => end_cast(
                &mut script,
                &mut units,
                spells,
                caster,
                spell_id,
                cast_count,
                end_event(reason),
            ),
            CastBarEdge::Cancelled { caster } => {
                let Some(run) = units.casts.get(&caster).copied() else {
                    continue;
                };
                end_cast(
                    &mut script,
                    &mut units,
                    spells,
                    caster,
                    run.spell_id,
                    run.cast_count,
                    "UNIT_SPELLCAST_INTERRUPTED",
                );
            }
            CastBarEdge::Delayed { caster, delay_ms } => {
                let Some(run) = units.casts.get_mut(&caster) else {
                    continue;
                };
                run.end_ms += f64::from(delay_ms);
                let run = *run;
                let not = units.reads_not_interruptible(caster);
                let cast = unit_cast(&script, spells, run, false, not);
                script.set_unit_cast(caster, false, Some(cast));
                script.fire_unit_event(
                    caster,
                    "UNIT_SPELLCAST_DELAYED",
                    spell_args(spells, run.spell_id, run.cast_count),
                );
            }
            CastBarEdge::ChannelStart {
                caster,
                spell_id,
                duration_ms,
            } => {
                // No `Spell.dbc` row, no event (`0x801ced`).
                let raises = spells
                    .and_then(|s| s.catalog.get(spell_id))
                    .is_some_and(|d| raises_events(d, is_self(caster), true));
                if !raises {
                    continue;
                }
                let run = Running {
                    spell_id,
                    cast_count: 0,
                    start_ms: now_ms,
                    end_ms: now_ms + f64::from(duration_ms),
                };
                units.channels.insert(caster, run);
                // `SetUIChannel` evaluates the bit for the target and focus only (`0x72ad20`).
                if watched.contains(&Some(caster)) {
                    shield.evaluate(
                        &mut units,
                        spells,
                        &index,
                        self_guid.0.is_some(),
                        caster,
                        spell_id,
                    );
                }
                let not = units.reads_not_interruptible(caster);
                let cast = unit_cast(&script, spells, run, true, not);
                script.set_unit_cast(caster, true, Some(cast));
                script.fire_unit_event(
                    caster,
                    "UNIT_SPELLCAST_CHANNEL_START",
                    spell_args(spells, spell_id, 0),
                );
            }
            // `0x801e85`: nonzero re-times the channel, zero ends it.
            CastBarEdge::ChannelUpdate {
                caster,
                remaining_ms,
            } => {
                let Some(run) = units.channels.get(&caster).copied() else {
                    continue;
                };
                if remaining_ms == 0 {
                    units.channels.remove(&caster);
                    script.set_unit_cast(caster, true, None);
                    script.fire_unit_event(
                        caster,
                        "UNIT_SPELLCAST_CHANNEL_STOP",
                        spell_args(spells, run.spell_id, 0),
                    );
                } else {
                    let run = Running {
                        end_ms: now_ms + f64::from(remaining_ms),
                        ..run
                    };
                    units.channels.insert(caster, run);
                    let not = units.reads_not_interruptible(caster);
                    let cast = unit_cast(&script, spells, run, true, not);
                    script.set_unit_cast(caster, true, Some(cast));
                    script.fire_unit_event(
                        caster,
                        "UNIT_SPELLCAST_CHANNEL_UPDATE",
                        spell_args(spells, run.spell_id, 0),
                    );
                }
            }
        }
    }
    reevaluate_watched(
        &mut script,
        &mut units,
        spells,
        &index,
        self_guid.0.is_some(),
        &shield,
        watched,
    );
}

/// The bit's other two evaluations: a new target re-reads it with no event (`0x524dbc`), and an
/// aura change on the target or focus with a silenceable cast running (`PreventionType` 1)
/// re-reads it and fires `UNIT_SPELLCAST_(NOT_)INTERRUPTIBLE` on a change (`0x72fd30`).
fn reevaluate_watched(
    script: &mut UiScript,
    units: &mut UnitCasts,
    spells: Option<&Spells>,
    index: &GuidIndex,
    have_player: bool,
    shield: &ShieldInputs,
    watched: [Option<u64>; 2],
) {
    for (slot, guid) in watched.into_iter().enumerate() {
        let Some(guid) = guid else { continue };
        let newly = slot == 0 && units.watched[0] != Some(guid);
        let auras_moved = index
            .0
            .get(&guid)
            .and_then(|&e| shield.auras.get(e).ok())
            .is_some_and(|a| a.is_changed());
        if !newly && !auras_moved {
            continue;
        }
        // The casting spell first, else the channel.
        let (run, channel) = match (units.casts.get(&guid), units.channels.get(&guid)) {
            (Some(&r), _) => (r, false),
            (None, Some(&r)) => (r, true),
            (None, None) => continue,
        };
        let silenceable = spells
            .and_then(|s| s.catalog.get(run.spell_id))
            .is_some_and(|d| d.prevention_type == 1);
        if !newly && !silenceable {
            continue;
        }
        let before = units.reads_not_interruptible(guid);
        shield.evaluate(units, spells, index, have_player, guid, run.spell_id);
        let after = units.reads_not_interruptible(guid);
        let cast = unit_cast(script, spells, run, channel, after);
        script.set_unit_cast(guid, channel, Some(cast));
        if !newly && before != after {
            let event = if after {
                "UNIT_SPELLCAST_NOT_INTERRUPTIBLE"
            } else {
                "UNIT_SPELLCAST_INTERRUPTIBLE"
            };
            script.fire_unit_event(guid, event, vec![]);
        }
    }
    units.watched = watched;
    // The bits outlive their casts, as on the unit; bound them by the units still in play.
    if units.not_interruptible.len() > 256 {
        let UnitCasts {
            casts,
            channels,
            not_interruptible,
            immunities,
            ..
        } = units;
        let keep = |g: &u64| {
            casts.contains_key(g) || channels.contains_key(g) || watched.contains(&Some(*g))
        };
        not_interruptible.retain(|g, _| keep(g));
        immunities.retain(|g, _| keep(g));
    }
}

/// What the not-interruptible bit reads beyond the feed: the known spells the school masks come
/// from, the target, and the casters' auras.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ShieldInputs<'w, 's> {
    actions: Option<Res<'w, crate::ui_action::PlayerActions>>,
    selection: Option<Res<'w, crate::target::Selection>>,
    auras: Query<'w, 's, Ref<'static, crate::unit_auras::UnitAuras>>,
}

impl ShieldInputs<'_, '_> {
    /// Run `ProcessCastNotInterruptible` for `caster`'s cast of `spell_id` into its bit.
    fn evaluate(
        &self,
        units: &mut UnitCasts,
        spells: Option<&Spells>,
        index: &GuidIndex,
        have_player: bool,
        caster: u64,
        spell_id: u32,
    ) {
        let Some((spells, d)) = spells.and_then(|s| Some((s, s.catalog.get(spell_id)?))) else {
            return;
        };
        let auras = index.0.get(&caster).and_then(|&e| self.auras.get(e).ok());
        let aura_spells = auras
            .iter()
            .flat_map(|a| a.iter())
            .filter_map(|a| spells.catalog.get(a.spell_id));
        let bit = not_interruptible_bit(
            d,
            units.schools.against(is_player(caster)),
            have_player,
            units.immunities.get(&caster).copied().unwrap_or((0, 0)),
            aura_spells,
        );
        units.not_interruptible.insert(caster, bit);
    }
}

/// `0x7fecc0`: the ending event for any cast, then `STOP` when it was the unit's running one.
fn end_cast(
    script: &mut UiScript,
    units: &mut UnitCasts,
    spells: Option<&Spells>,
    caster: u64,
    spell_id: u32,
    cast_count: u8,
    event: &str,
) {
    script.fire_unit_event(caster, event, spell_args(spells, spell_id, cast_count));
    let running = units
        .casts
        .get(&caster)
        .is_some_and(|r| r.spell_id == spell_id && r.cast_count == cast_count);
    if running {
        units.casts.remove(&caster);
        script.set_unit_cast(caster, false, None);
        script.fire_unit_event(
            caster,
            "UNIT_SPELLCAST_STOP",
            spell_args(spells, spell_id, cast_count),
        );
    }
}

/// The unit-cast feed, drained before the VM ticks so an edge and its first `OnUpdate` share a
/// frame, and after the local cancel so a move or Esc ends the bar on the next tick.
pub(crate) struct UiCastPlugin;

impl Plugin for UiCastPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CastBarFeed>()
            .init_resource::<UnitCasts>()
            .add_systems(
                Update,
                // After the spell's local cancel, whose edges this drains the same frame.
                feed_cast_bar
                    .in_set(UnitFeed)
                    .after(crate::spell::LocalCancel),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(attributes: u32, attributes_ex3: u32, attributes_ex4: u32) -> SpellDisplay {
        SpellDisplay {
            attributes,
            attributes_ex3,
            attributes_ex4,
            ..Default::default()
        }
    }

    #[test]
    fn the_event_gate_follows_the_start_and_channel_handlers() {
        let plain = display(0, 0, 0);
        assert!(raises_events(&plain, false, false));
        assert!(raises_events(&plain, true, true));
        // A ranged-slot spell (Throw, the shots) never, unless Ex4's override.
        assert!(!raises_events(&display(0x2, 0, 0), true, false));
        assert!(raises_events(&display(0x2, 0, 0x0800_0000), false, false));
        // Hidden attributes stop an observer's event, never the player's.
        assert!(!raises_events(&display(0x100, 0, 0), false, false));
        assert!(raises_events(&display(0x100, 0, 0), true, false));
        assert!(!raises_events(&display(0, 0, 0x1), false, false));
        // `AttributesEx3 & 0x2000` (Blood Siphon) gates a channel only.
        assert!(!raises_events(&display(0, 0x2000, 0), false, true));
        assert!(raises_events(&display(0, 0x2000, 0), false, false));
    }

    const ME: u64 = 10;
    const HEAL: u32 = 2050;

    /// A world with the feed's inputs, us as guid 10 and `player`, and Lesser Heal cataloged.
    fn feed_app() -> App {
        let mut app = App::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx))
            .init_resource::<CastBarFeed>()
            .init_resource::<UnitCasts>()
            .init_resource::<PendingCast>()
            .init_resource::<QueuedMeleeSpell>()
            .init_resource::<crate::spell::AutoRepeatActive>()
            .init_resource::<SelfGuid>()
            .init_resource::<GuidIndex>()
            .init_resource::<crate::names::NameCache>()
            .insert_resource(Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(
                        HEAL,
                        SpellDisplay {
                            name: "Lesser Heal".into(),
                            rank: Some("Rank 1".into()),
                            ..Default::default()
                        },
                    )]
                    .into_iter()
                    .collect(),
                ),
                ..Spells::empty_for_tests()
            });
        app.world_mut().resource_mut::<SelfGuid>().0 = Some(ME);
        let mut script = UiScript::new().unwrap();
        script.set_unit_guids(&benilla_ui::script::UnitGuids {
            player: ME,
            ..Default::default()
        });
        script.set_now(100.0);
        script
            .run(
                "SEEN = {} local f = CreateFrame('Frame')
                 for _, e in ipairs({'START', 'STOP', 'FAILED', 'INTERRUPTED', 'SUCCEEDED',
                                     'DELAYED', 'FAILED_QUIET'}) do
                     f:RegisterEvent('UNIT_SPELLCAST_' .. e)
                 end
                 f:SetScript('OnEvent', function(self, e, u, n, r, c)
                     table.insert(SEEN, e:sub(16) .. ':' .. u .. ':' .. n .. ':' .. c) end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app
    }

    fn run(app: &mut App, edges: Vec<CastBarEdge>) -> String {
        use bevy::ecs::system::RunSystemOnce;
        app.world_mut().resource_mut::<CastBarFeed>().0 = edges;
        app.world_mut().run_system_once(feed_cast_bar).unwrap();
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval::<String>("local s = table.concat(SEEN, ' ') SEEN = {} return s")
            .unwrap()
    }

    /// A cast opens on START, reads back through `UnitCastingInfo`, and its launch fires
    /// `SUCCEEDED` then `STOP`; an interrupt fires `INTERRUPTED` then `STOP`; a refusal of a cast
    /// not running fires `FAILED` alone, so the bar (keyed on `castID`) ignores it.
    #[test]
    fn the_feed_raises_the_reference_sequence() {
        let mut app = feed_app();
        let start = |cast_count| CastBarEdge::Start {
            caster: ME,
            spell_id: HEAL,
            cast_count,
            cast_time_ms: 1500,
            immunities: None,
        };
        assert_eq!(run(&mut app, vec![start(7)]), "START:player:Lesser Heal:7");
        assert_eq!(
            app.world_mut()
                .non_send_resource::<UiScript>()
                .eval::<String>(
                    r#"local n, r, t, i, a, b, ts, id = UnitCastingInfo("player")
                       return n .. "|" .. r .. "|" .. a .. "|" .. b .. "|" .. id"#
                )
                .unwrap(),
            "Lesser Heal|Rank 1|100000|101500|7"
        );
        let go = CastBarEdge::Go {
            caster: ME,
            spell_id: HEAL,
            cast_count: 7,
            cast_flags: 0,
        };
        assert_eq!(
            run(&mut app, vec![go]),
            "SUCCEEDED:player:Lesser Heal:7 STOP:player:Lesser Heal:7"
        );
        assert_eq!(
            app.world_mut()
                .non_send_resource::<UiScript>()
                .arity(r#"UnitCastingInfo("player")"#)
                .unwrap(),
            0
        );

        let interrupted = crate::ui_action::cast_fail::CAST_FAIL_KEYS
            .iter()
            .position(|k| *k == "SPELL_FAILED_INTERRUPTED")
            .unwrap() as u8;
        let failed = |cast_count| CastBarEdge::Failed {
            caster: ME,
            spell_id: HEAL,
            cast_count,
            reason: interrupted,
        };
        assert_eq!(
            run(&mut app, vec![start(8), failed(9), failed(8)]),
            "START:player:Lesser Heal:8 INTERRUPTED:player:Lesser Heal:9 \
             INTERRUPTED:player:Lesser Heal:8 STOP:player:Lesser Heal:8"
        );
        assert_eq!(
            run(
                &mut app,
                vec![start(10), CastBarEdge::Cancelled { caster: ME }]
            ),
            "START:player:Lesser Heal:10 INTERRUPTED:player:Lesser Heal:10 \
             STOP:player:Lesser Heal:10"
        );
        // An instant opens nothing; its launch only succeeds.
        let instant = CastBarEdge::Start {
            caster: ME,
            spell_id: HEAL,
            cast_count: 11,
            cast_time_ms: 0,
            immunities: None,
        };
        let go11 = CastBarEdge::Go {
            caster: ME,
            spell_id: HEAL,
            cast_count: 11,
            cast_flags: 0,
        };
        assert_eq!(
            run(&mut app, vec![instant, go11]),
            "SUCCEEDED:player:Lesser Heal:11"
        );
    }

    #[test]
    fn the_reason_picks_the_ending_event() {
        let shared = |key: &str| {
            crate::ui_action::cast_fail::CAST_FAIL_KEYS
                .iter()
                .position(|k| *k == key)
                .unwrap() as u8
        };
        assert_eq!(
            end_event(shared("SPELL_FAILED_INTERRUPTED")),
            "UNIT_SPELLCAST_INTERRUPTED"
        );
        assert_eq!(
            end_event(shared("SPELL_FAILED_DONT_REPORT")),
            "UNIT_SPELLCAST_FAILED_QUIET"
        );
        assert_eq!(
            end_event(shared("SPELL_FAILED_BAD_TARGETS")),
            "UNIT_SPELLCAST_FAILED"
        );
    }

    /// Kick: an interrupt (effect 68) of the physical school.
    fn kick() -> SpellDisplay {
        SpellDisplay {
            effects: [68, 0, 0],
            school_mask: 1,
            ..Default::default()
        }
    }

    /// A silenceable mob cast (`PreventionType` 1).
    fn bolt() -> SpellDisplay {
        SpellDisplay {
            prevention_type: 1,
            ..Default::default()
        }
    }

    /// `ProcessCastNotInterruptible` (`0x7262e0`) over its arms.
    #[test]
    fn the_bit_sets_when_the_players_interrupts_cannot_land() {
        let schools = InterruptSchools::of([&kick()]);
        let none: [&SpellDisplay; 0] = [];
        // Interruptible: the player's Kick lands.
        assert!(!not_interruptible_bit(&bolt(), schools, true, (0, 0), none));
        // A mechanic-26 immunity from the cast, or the caster's school immunity, clears Kick.
        assert!(not_interruptible_bit(
            &bolt(),
            schools,
            true,
            (0, 0x0400_0000),
            none
        ));
        assert!(not_interruptible_bit(&bolt(), schools, true, (1, 0), none));
        // An aura immunity to the interrupt effect does the same.
        let immune = SpellDisplay {
            effect_apply_aura: [37, 0, 0],
            effect_misc_value: [68, 0, 0],
            ..Default::default()
        };
        assert!(not_interruptible_bit(
            &bolt(),
            schools,
            true,
            (0, 0),
            [&immune]
        ));
        // A spell nothing silences always sets it; `AttributesEx7 & 0x2000` never does.
        assert!(not_interruptible_bit(
            &SpellDisplay::default(),
            schools,
            true,
            (0, 0),
            none
        ));
        let exempt = SpellDisplay {
            attributes_ex7: 0x2000,
            ..bolt()
        };
        assert!(!not_interruptible_bit(
            &exempt,
            schools,
            true,
            (0, 0x0400_0000),
            none
        ));
        // No interrupt known: the bit stays clear.
        let empty = InterruptSchools::default();
        assert!(!not_interruptible_bit(
            &bolt(),
            empty,
            true,
            (0, 0x0400_0000),
            none
        ));
    }

    /// The target's immune cast reads `notInterruptible`, and a dispelled immunity aura fires
    /// `UNIT_SPELLCAST_INTERRUPTIBLE` and clears it.
    #[test]
    fn the_targets_cast_carries_the_shield_and_its_edges() {
        const MOB: u64 = 0xF130_0000_0000_0042;
        const BOLT: u32 = 9001;
        const IMMUNE: u32 = 9002;
        const KICK: u32 = 1766;
        let mut app = feed_app();
        {
            let mut spells = app.world_mut().resource_mut::<Spells>();
            let immune = SpellDisplay {
                effect_apply_aura: [77, 0, 0],
                effect_misc_value: [26, 0, 0],
                ..Default::default()
            };
            spells.catalog = benilla_formats::SpellCatalog::from_displays(
                [(KICK, kick()), (BOLT, bolt()), (IMMUNE, immune)]
                    .into_iter()
                    .collect(),
            );
        }
        let mut actions = crate::ui_action::PlayerActions::default();
        actions.spells.insert(KICK);
        app.insert_resource(actions);
        app.insert_resource(crate::target::Selection {
            guid: Some(MOB),
            ..Default::default()
        });
        let mob = app
            .world_mut()
            .spawn(crate::unit_auras::UnitAuras::of(MOB, &[(0, IMMUNE, 1)]))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(MOB, mob);
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.set_unit_guids(&benilla_ui::script::UnitGuids {
                player: ME,
                target: MOB,
                ..Default::default()
            });
            script
                .run(
                    "local g = CreateFrame('Frame')
                     g:RegisterEvent('UNIT_SPELLCAST_INTERRUPTIBLE')
                     g:RegisterEvent('UNIT_SPELLCAST_NOT_INTERRUPTIBLE')
                     g:SetScript('OnEvent', function(self, e, u, n, r, c)
                         table.insert(SEEN, e:sub(16) .. ':' .. u .. ':' .. tostring(n) .. ':'
                             .. tostring(c)) end)",
                )
                .unwrap();
        }
        let start = CastBarEdge::Start {
            caster: MOB,
            spell_id: BOLT,
            cast_count: 0,
            cast_time_ms: 2000,
            immunities: None,
        };
        run(&mut app, vec![start]);
        let shield = |app: &mut App| {
            app.world_mut()
                .non_send_resource::<UiScript>()
                .eval::<bool>(r#"return select(9, UnitCastingInfo("target"))"#)
                .unwrap()
        };
        assert!(shield(&mut app), "the immunity aura shields the cast");
        // The aura goes; the re-read clears the shield and says so.
        app.world_mut()
            .entity_mut(mob)
            .insert(crate::unit_auras::UnitAuras::default());
        assert_eq!(run(&mut app, vec![]), "INTERRUPTIBLE:target:nil:nil");
        assert!(!shield(&mut app));
    }
}
