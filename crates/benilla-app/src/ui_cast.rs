//! Every unit's cast and channel edges, off its packets, the cast send and the local cancel: each
//! applies to the caster's cast state in the turn of the handler or press that made it and fires
//! the `UNIT_SPELLCAST_*` events 3.3.5a's `CastingBarFrame.lua:6-15` registers for, fanned out over
//! the tokens naming the caster (`0x60bf60`: unit, name, rank, castCount). `UnitCastingInfo` and
//! `UnitChannelInfo` read the state through the live view when called. The own-cast lifecycle it
//! reads is `crate::spell`'s.
//!
//! A cast's end runs one path in the reference (`0x7fecc0`): `SUCCEEDED` for a launch (result
//! `0xbb`), else `FAILED_QUIET`, `INTERRUPTED` or `FAILED` by reason, then `STOP` when it was the
//! unit's running cast (`castCount` and spell, `+0xa5c`/`+0xa6c`).

use std::collections::HashMap;

use benilla_formats::SpellDisplay;
use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript, UnitCast};

use crate::net::{GuidIndex, NetCommands, ObjectStore, SelfGuid};
use crate::ui_action::Spells;
use crate::ui_script::live::UiEvents;

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
        /// `castFlags & 1`: the immunities are kept, nothing else runs (`0x80694e`).
        hidden: bool,
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
    /// `SMSG_RESUME_CAST_BAR`: a cast or channel already running, ending `remaining_ms` from now
    /// after `total_ms` in all.
    Resume {
        caster: u64,
        spell_id: u32,
        remaining_ms: u32,
        total_ms: u32,
    },
}

/// The edges presses made and not yet fired, shared by every ladder and cancel.
#[derive(Resource, Default)]
pub(crate) struct PressEdges(Vec<CastBarEdge>);

/// The edges a press makes: our cast send's `UNIT_SPELLCAST_SENT` (`0x80b556`) and our local
/// cancel's end (`0x806200`). The reference signals both inside the press, so a system holding the
/// VM fires them with it as soon as the ladder or the cancel returns ([`Self::fire`]); a packet
/// handler hands them to its turn's queue ([`Self::defer`]), and a system without the VM fires
/// them once its commands apply ([`Self::flush`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PressSignals<'w, 's> {
    // `Option`: a ladder runs in harnesses without the cast plugin, which signal nothing.
    edges: Option<ResMut<'w, PressEdges>>,
    casts: Option<crate::ui_script::OwnedMut<'w, UnitCasts>>,
    spells: Option<Res<'w, Spells>>,
    sent: Option<SentTarget<'w, 's>>,
    index: Option<Res<'w, GuidIndex>>,
}

impl PressSignals<'_, '_> {
    /// `CMSG_CAST_SPELL` went out at `target`, or none.
    pub(crate) fn sent(&mut self, spell_id: u32, target: Option<u64>) {
        if let Some(edges) = self.edges.as_mut() {
            edges.0.push(CastBarEdge::Sent { spell_id, target });
        }
    }

    /// Our running cast was cancelled here.
    pub(crate) fn cancelled(&mut self, caster: u64) {
        if let Some(edges) = self.edges.as_mut() {
            edges.0.push(CastBarEdge::Cancelled { caster });
        }
    }

    fn take(&mut self) -> Vec<CastBarEdge> {
        self.edges
            .as_mut()
            .map(|e| std::mem::take(&mut e.0))
            .unwrap_or_default()
    }

    /// Hand the edges to the queue a packet handler fires in its turn.
    pub(crate) fn defer(&mut self, ui: &mut UiEvents) {
        for edge in self.take() {
            ui.cast_edge(edge);
        }
    }

    /// Hand the edges to the queue and fire it once the calling system's commands apply: a system
    /// without the VM.
    pub(crate) fn flush(&mut self, ecs: &mut Commands) {
        let edges = self.take();
        if edges.is_empty() {
            return;
        }
        ecs.queue(move |world: &mut World| {
            if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
                for edge in edges {
                    ui.cast_edge(edge);
                }
            }
            crate::ui_script::live::fire_handler_events(world);
        });
    }

    /// The edges waiting, in order.
    #[cfg(test)]
    pub(crate) fn pending(&self) -> &[CastBarEdge] {
        self.edges.as_ref().map_or(&[], |e| &e.0)
    }

    /// Fire the waiting edges into `script` now, in order: `SENT` with the target's name
    /// (`0x80b533`), and a cancel's `INTERRUPTED`, the unit's cast cleared, then `STOP`
    /// (`0x8066ce`, `0x7fee38`).
    pub(crate) fn fire(&mut self, script: &mut UiScript) {
        let edges = self.take();
        let (Some(sent), Some(index), Some(casts)) = (
            self.sent.as_ref(),
            self.index.as_deref(),
            self.casts.as_mut(),
        ) else {
            return;
        };
        let spells = self.spells.as_deref();
        for edge in edges {
            match edge {
                CastBarEdge::Sent { spell_id, target } => {
                    let args = sent.args(index, spells, spell_id, target);
                    script.fire_event("UNIT_SPELLCAST_SENT", args);
                }
                CastBarEdge::Cancelled { caster } => {
                    let run = casts.read().casts.get(&caster).copied();
                    let Some(run) = run else { continue };
                    let args = spell_args(spells, run.spell_id, run.cast_count);
                    script.fire_unit_event(caster, "UNIT_SPELLCAST_INTERRUPTED", args.clone());
                    let mut casts = casts.write();
                    if casts.casts.get(&caster).is_some_and(|r| {
                        (r.spell_id, r.cast_count) == (run.spell_id, run.cast_count)
                    }) {
                        casts.casts.remove(&caster);
                        drop(casts);
                        script.fire_unit_event(caster, "UNIT_SPELLCAST_STOP", args);
                    }
                }
                // A press makes no other edge.
                _ => {}
            }
        }
    }
}

/// Each unit's running cast and channel as the unit holds them: `(spell, castCount)` and the
/// window on the `GetTime` clock in ms. Every loan of the live view owns it: the edges and the
/// aura path write it in turn, and the units' feed re-reads a new target's bit.
#[derive(Resource, Default)]
pub(crate) struct UnitCasts {
    casts: HashMap<u64, Running>,
    channels: HashMap<u64, Running>,
    /// Each unit's state bit `+0xa34 & 8`, set by the reference's not-interruptible evaluation
    /// (`0x7262e0`) and kept until its next evaluation.
    not_interruptible: HashMap<u64, bool>,
    /// Each caster's last cast immunities, `(school, mechanic)` (`+0xa64`, `+0xa68`).
    immunities: HashMap<u64, (u32, u32)>,
}

impl UnitCasts {
    /// The reference's not-interruptible test (`0x71ab20`): the unit's bit, and any of the player's
    /// masks, the two specials only against a non-player.
    fn reads_not_interruptible(&self, caster: u64, schools: InterruptSchools) -> bool {
        self.not_interruptible
            .get(&caster)
            .copied()
            .unwrap_or(false)
            && schools.against(is_player(caster)).any()
    }

    /// The unit's cast, or with `channel` its channel, as `UnitCastingInfo` and `UnitChannelInfo`
    /// read it at the call.
    pub(crate) fn live_cast(
        &self,
        guid: u64,
        channel: bool,
        spells: Option<&Spells>,
        schools: InterruptSchools,
    ) -> Option<UnitCast> {
        let run = *if channel {
            self.channels.get(&guid)
        } else {
            self.casts.get(&guid)
        }?;
        Some(unit_cast(
            spells,
            run,
            channel,
            self.reads_not_interruptible(guid, schools),
        ))
    }

    /// What the plate's cast setter reads off `guid` (`0x720e50`): its channel first, else its
    /// cast.
    pub(crate) fn plate_source(
        &self,
        guid: u64,
        schools: InterruptSchools,
    ) -> Option<crate::vplates::CastSource> {
        let (run, channel) = match (self.channels.get(&guid), self.casts.get(&guid)) {
            (Some(r), _) => (r, true),
            (None, Some(r)) => (r, false),
            (None, None) => return None,
        };
        Some(crate::vplates::CastSource {
            spell_id: run.spell_id,
            channel,
            start_ms: run.start_ms,
            end_ms: run.end_ms,
            not_interruptible: self.reads_not_interruptible(guid, schools),
        })
    }
}

/// A unit guid of the player kind.
fn is_player(guid: u64) -> bool {
    benilla_protocol::guid::is_player(guid)
}

/// The spell book update's four school masks (`0x53cd48`-`0x53ce75`) over the known spells: an
/// interrupt (effect 68), a silence (an aura 27), and the `AttributesEx7` specials `0x800` and
/// `0x1000`. The book's handlers rebuild them as the book changes ([`book_updated`]).
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
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

/// The reference's not-interruptible evaluation (`0x7262e0`): the bit for a cast of `d`. Never
/// under `AttributesEx7 & 0x2000`; always for a spell no silence prevents (`PreventionType != 1`);
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
pub(crate) fn raises_events(d: &SpellDisplay, is_self: bool, channel: bool) -> bool {
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
/// only with `AttributesEx` bit 29 (`0x612151`), else the Lua global `CHANNELING`, which the verb
/// reads.
fn unit_cast(
    spells: Option<&Spells>,
    run: Running,
    channel: bool,
    not_interruptible: bool,
) -> UnitCast {
    let display = spells.and_then(|s| s.catalog.get(run.spell_id));
    let (name, rank) = name_rank(spells, run.spell_id);
    let text = match display {
        Some(d) if !channel && d.no_casting_bar_text() => String::new(),
        _ => name.clone(),
    };
    UnitCast {
        channeling_text: channel && display.is_some_and(|d| !d.channel_bar_own_name()),
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

/// What names `UNIT_SPELLCAST_SENT`'s target.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SentTarget<'w, 's> {
    names: Res<'w, crate::names::NameCache>,
    commands: Res<'w, NetCommands>,
    stores: Query<'w, 's, &'static ObjectStore>,
}

impl SentTarget<'_, '_> {
    /// `("player", name, rank, target)`, the target's name or `""` (`0x80b533`).
    fn args(
        &self,
        index: &GuidIndex,
        spells: Option<&Spells>,
        spell_id: u32,
        target: Option<u64>,
    ) -> Vec<ScriptValue> {
        let target = target
            .and_then(|g| {
                let store = index.0.get(&g).and_then(|&e| self.stores.get(e).ok());
                self.names.resolve_unit(g, store, &self.commands)
            })
            .unwrap_or_default()
            .to_string();
        let (name, rank) = name_rank(spells, spell_id);
        vec![
            ScriptValue::Str("player".into()),
            ScriptValue::Str(name),
            ScriptValue::Str(rank),
            ScriptValue::Str(target),
        ]
    }
}

/// Apply `edge` to its caster's cast state and queue its `UNIT_SPELLCAST_*` events, with the
/// `World` lent between the events around it.
pub(crate) fn apply_edge(world: &mut World, edge: CastBarEdge) {
    let focus = focus_of(world);
    if let Err(e) = world.run_system_cached_with(edge_system, (edge, focus)) {
        warn!("ui_cast: a cast edge did not apply: {e}");
    }
}

/// The focus guid the VM holds (`FocusUnit` sets it), read off the world between calls into Lua.
fn focus_of(world: &World) -> Option<u64> {
    world
        .get_non_send_resource::<UiScript>()
        .map(UiScript::focus_guid)
        .filter(|&g| g != 0)
}

/// The watched units the cast and channel setters evaluate the bit for: the target and the focus
/// (`0x72ac50`, `0x72ad20`).
fn watched(shield: &ShieldInputs, focus: Option<u64>) -> [Option<u64>; 2] {
    let target = shield.selection.as_ref().and_then(|s| s.guid);
    [target, focus]
}

/// One edge, in the turn of the handler or press that made it.
#[allow(clippy::too_many_arguments)]
fn edge_system(
    In((edge, focus)): In<(CastBarEdge, Option<u64>)>,
    mut units: ResMut<UnitCasts>,
    spells: Option<Res<Spells>>,
    self_guid: Res<SelfGuid>,
    index: Res<GuidIndex>,
    sent: SentTarget,
    shield: ShieldInputs,
    clock: Res<crate::ui_script::UiClock>,
    mut ui: ResMut<UiEvents>,
) {
    if *crate::net::CAST_TRACE {
        info!("cast-trace: UNIT_SPELLCAST edge {edge:?}");
    }
    let spells = spells.as_deref();
    let now_ms = clock.ui_now * 1000.0;
    let is_self = |guid: u64| self_guid.0 == Some(guid);
    let watched = watched(&shield, focus);
    let have_player = self_guid.0.is_some();
    let units = &mut *units;
    match edge {
        CastBarEdge::Sent { spell_id, target } => {
            ui.fire(
                "UNIT_SPELLCAST_SENT",
                sent.args(&index, spells, spell_id, target),
            );
        }
        CastBarEdge::Start {
            caster,
            spell_id,
            cast_count,
            cast_time_ms,
            immunities,
            hidden,
        } => {
            // Stored before any gate, and kept until the next start that carries a pair.
            if let Some(pair) = immunities {
                units.immunities.insert(caster, pair);
            }
            let raises = spells
                .and_then(|s| s.catalog.get(spell_id))
                .is_none_or(|d| raises_events(d, is_self(caster), false));
            if hidden || cast_time_ms == 0 || !raises {
                return;
            }
            let run = Running {
                spell_id,
                cast_count,
                start_ms: now_ms,
                end_ms: now_ms + f64::from(cast_time_ms),
            };
            units.casts.insert(caster, run);
            // The UI's cast setter evaluates the bit for the target and focus only (`0x72ac50`).
            if watched.contains(&Some(caster)) {
                shield.evaluate(units, spells, &index, have_player, caster, spell_id);
            }
            unit_event(
                &mut ui,
                caster,
                "UNIT_SPELLCAST_START",
                spells,
                spell_id,
                cast_count,
            );
        }
        CastBarEdge::Go {
            caster,
            spell_id,
            cast_count,
            cast_flags,
        } => {
            if cast_flags & 1 != 0 {
                return;
            }
            end_cast(
                &mut ui,
                units,
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
            &mut ui,
            units,
            spells,
            caster,
            spell_id,
            cast_count,
            end_event(reason),
        ),
        CastBarEdge::Cancelled { caster } => {
            let Some(run) = units.casts.get(&caster).copied() else {
                return;
            };
            end_cast(
                &mut ui,
                units,
                spells,
                caster,
                run.spell_id,
                run.cast_count,
                "UNIT_SPELLCAST_INTERRUPTED",
            );
        }
        CastBarEdge::Delayed { caster, delay_ms } => {
            let Some(run) = units.casts.get_mut(&caster) else {
                return;
            };
            run.end_ms += f64::from(delay_ms);
            let run = *run;
            unit_event(
                &mut ui,
                caster,
                "UNIT_SPELLCAST_DELAYED",
                spells,
                run.spell_id,
                run.cast_count,
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
                return;
            }
            let run = Running {
                spell_id,
                cast_count: 0,
                start_ms: now_ms,
                end_ms: now_ms + f64::from(duration_ms),
            };
            units.channels.insert(caster, run);
            // The UI's channel setter evaluates the bit for the target and focus only
            // (`0x72ad20`).
            if watched.contains(&Some(caster)) {
                shield.evaluate(units, spells, &index, have_player, caster, spell_id);
            }
            unit_event(
                &mut ui,
                caster,
                "UNIT_SPELLCAST_CHANNEL_START",
                spells,
                spell_id,
                0,
            );
        }
        // `0x8020c0`: no attribute gate; a channel by AttributesEx `0x4` (`0x80215e`), each setter
        // clearing the other's window (`0x72ac50`, `0x72ae00`), with cast count 0.
        CastBarEdge::Resume {
            caster,
            spell_id,
            remaining_ms,
            total_ms,
        } => {
            let Some(d) = spells.and_then(|s| s.catalog.get(spell_id)) else {
                return;
            };
            let channel = d.tooltip_channeled();
            let end_ms = now_ms + f64::from(remaining_ms);
            let run = Running {
                spell_id,
                cast_count: 0,
                start_ms: end_ms - f64::from(total_ms),
                end_ms,
            };
            let (mine, other) = if channel {
                (&mut units.channels, &mut units.casts)
            } else {
                (&mut units.casts, &mut units.channels)
            };
            mine.insert(caster, run);
            other.remove(&caster);
            if watched.contains(&Some(caster)) {
                shield.evaluate(units, spells, &index, have_player, caster, spell_id);
            }
            let event = if channel {
                "UNIT_SPELLCAST_CHANNEL_START"
            } else {
                "UNIT_SPELLCAST_START"
            };
            unit_event(&mut ui, caster, event, spells, spell_id, 0);
        }
        // `0x801e85`: nonzero re-times the channel, zero ends it.
        CastBarEdge::ChannelUpdate {
            caster,
            remaining_ms,
        } => {
            let Some(run) = units.channels.get(&caster).copied() else {
                return;
            };
            if remaining_ms == 0 {
                units.channels.remove(&caster);
                unit_event(
                    &mut ui,
                    caster,
                    "UNIT_SPELLCAST_CHANNEL_STOP",
                    spells,
                    run.spell_id,
                    0,
                );
            } else {
                let run = Running {
                    end_ms: now_ms + f64::from(remaining_ms),
                    ..run
                };
                units.channels.insert(caster, run);
                unit_event(
                    &mut ui,
                    caster,
                    "UNIT_SPELLCAST_CHANNEL_UPDATE",
                    spells,
                    run.spell_id,
                    0,
                );
            }
        }
    }
    units.bound(watched);
}

/// Queue `event` to every token naming `caster`, with the spell's name, rank and `castCount`.
fn unit_event(
    ui: &mut UiEvents,
    caster: u64,
    event: &'static str,
    spells: Option<&Spells>,
    spell_id: u32,
    cast_count: u8,
) {
    let args = spell_args(spells, spell_id, cast_count);
    ui.write(move |script| script.fire_unit_event(caster, event, args));
}

impl UnitCasts {
    /// The bits outlive their casts, as on the unit; bound them by the units still in play.
    fn bound(&mut self, watched: [Option<u64>; 2]) {
        if self.not_interruptible.len() <= 256 {
            return;
        }
        let Self {
            casts,
            channels,
            not_interruptible,
            immunities,
        } = self;
        let keep = |g: &u64| {
            casts.contains_key(g) || channels.contains_key(g) || watched.contains(&Some(*g))
        };
        not_interruptible.retain(|g, _| keep(g));
        immunities.retain(|g, _| keep(g));
    }

    /// The unit's running cast first, else its channel.
    fn running(&self, guid: u64) -> Option<Running> {
        self.casts
            .get(&guid)
            .or_else(|| self.channels.get(&guid))
            .copied()
    }
}

/// The aura reader's re-read (`0x72fd30`): an aura change on the target or the focus while it runs a
/// cast no silence prevents is not (`PreventionType` 1) re-evaluates the bit and fires
/// `UNIT_SPELLCAST_(NOT_)INTERRUPTIBLE` on a change (`0x72fdbb`); queued by the aura handler, after
/// its changes apply.
pub(crate) fn auras_changed(world: &mut World, guid: u64) {
    let focus = focus_of(world);
    if let Err(e) = world.run_system_cached_with(aura_reread, (guid, focus)) {
        warn!("ui_cast: the aura re-read did not run: {e}");
    }
}

fn aura_reread(
    In((guid, focus)): In<(u64, Option<u64>)>,
    mut units: ResMut<UnitCasts>,
    spells: Option<Res<Spells>>,
    self_guid: Res<SelfGuid>,
    index: Res<GuidIndex>,
    shield: ShieldInputs,
    mut ui: ResMut<UiEvents>,
) {
    if !watched(&shield, focus).contains(&Some(guid)) {
        return;
    }
    let spells = spells.as_deref();
    let Some(run) = units.running(guid) else {
        return;
    };
    let silenceable = spells
        .and_then(|s| s.catalog.get(run.spell_id))
        .is_some_and(|d| d.prevention_type == 1);
    if !silenceable {
        return;
    }
    let schools = shield.schools();
    let before = units.reads_not_interruptible(guid, schools);
    shield.evaluate(
        &mut units,
        spells,
        &index,
        self_guid.0.is_some(),
        guid,
        run.spell_id,
    );
    let after = units.reads_not_interruptible(guid, schools);
    if before != after {
        let event = if after {
            "UNIT_SPELLCAST_NOT_INTERRUPTIBLE"
        } else {
            "UNIT_SPELLCAST_INTERRUPTIBLE"
        };
        ui.write(move |script| script.fire_unit_event(guid, event, vec![]));
    }
}

/// The new target's re-read (`0x524dbc`): its running cast's bit evaluated again, with no event.
/// The units' feed runs it beside `PLAYER_TARGET_CHANGED`, which it fires on the change (#1289).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct TargetReread<'w, 's> {
    units: crate::ui_script::OwnedMut<'w, UnitCasts>,
    spells: Option<Res<'w, Spells>>,
    self_guid: Res<'w, SelfGuid>,
    index: Res<'w, GuidIndex>,
    shield: ShieldInputs<'w, 's>,
}

impl TargetReread<'_, '_> {
    pub(crate) fn reread(&mut self, target: u64) {
        let Some(run) = self.units.read().running(target) else {
            return;
        };
        self.shield.evaluate(
            &mut self.units.write(),
            self.spells.as_deref(),
            &self.index,
            self.self_guid.0.is_some(),
            target,
            run.spell_id,
        );
    }
}

/// What the not-interruptible bit reads beyond the cast state: the book's school masks, the
/// target, and the casters' auras.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ShieldInputs<'w, 's> {
    schools: Option<Res<'w, InterruptSchools>>,
    selection: Option<Res<'w, crate::target::Selection>>,
    auras: Query<'w, 's, &'static crate::unit_auras::UnitAuras>,
}

impl ShieldInputs<'_, '_> {
    fn schools(&self) -> InterruptSchools {
        self.schools.as_deref().copied().unwrap_or_default()
    }

    /// Run the not-interruptible evaluation for `caster`'s cast of `spell_id` into its bit.
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
            self.schools().against(is_player(caster)),
            have_player,
            units.immunities.get(&caster).copied().unwrap_or((0, 0)),
            aura_spells,
        );
        units.not_interruptible.insert(caster, bit);
    }
}

/// `0x7fecc0`: the ending event for any cast, then, when it was the unit's running one, the unit's
/// cast cleared and `STOP` (`0x7fee38`-`0x7fee6a`): the ending event's handlers still read it.
fn end_cast(
    ui: &mut UiEvents,
    units: &UnitCasts,
    spells: Option<&Spells>,
    caster: u64,
    spell_id: u32,
    cast_count: u8,
    event: &'static str,
) {
    unit_event(ui, caster, event, spells, spell_id, cast_count);
    let running = units
        .casts
        .get(&caster)
        .is_some_and(|r| r.spell_id == spell_id && r.cast_count == cast_count);
    if running {
        ui.world(move |world| {
            if let Some(mut units) = world.get_resource_mut::<UnitCasts>() {
                units.casts.remove(&caster);
            }
        });
        unit_event(
            ui,
            caster,
            "UNIT_SPELLCAST_STOP",
            spells,
            spell_id,
            cast_count,
        );
    }
}

/// The book update's mask rebuild (`0x53cd48`-`0x53ce75`), run by each handler that changes the
/// known spells.
pub(crate) fn book_updated(
    schools: &mut InterruptSchools,
    known: &std::collections::BTreeSet<u32>,
    spells: Option<&Spells>,
) {
    let Some(spells) = spells else {
        return;
    };
    *schools = InterruptSchools::of(known.iter().filter_map(|&id| spells.catalog.get(id)));
}

/// The cast state and the masks; nothing runs by frame.
pub(crate) struct UiCastPlugin;

impl Plugin for UiCastPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UnitCasts>()
            .init_resource::<InterruptSchools>()
            .init_resource::<PressEdges>()
            .init_resource::<UiEvents>();
    }
}

#[cfg(test)]
impl UnitCasts {
    /// A cast of `spell_id` running on `caster` with no end, for a test.
    pub(crate) fn run_for_test(&mut self, caster: u64, spell_id: u32) {
        self.casts.insert(
            caster,
            Running {
                spell_id,
                cast_count: 0,
                start_ms: 0.0,
                end_ms: f64::MAX,
            },
        );
    }

    /// Whether `caster` holds a running cast.
    pub(crate) fn casting(&self, caster: u64) -> bool {
        self.casts.contains_key(&caster)
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
            .init_resource::<UiEvents>()
            .init_resource::<UnitCasts>()
            .init_resource::<InterruptSchools>()
            .insert_resource(crate::ui_script::UiClock {
                anchor: std::time::Instant::now(),
                ui_now: 100.0,
            })
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

    /// Apply `edges` as one handler's queue and fire what they raised; what the observer saw.
    fn run(app: &mut App, edges: Vec<CastBarEdge>) -> String {
        let mut ui = app.world_mut().resource_mut::<UiEvents>();
        for edge in edges {
            ui.cast_edge(edge);
        }
        crate::ui_script::live::fire_handler_events(app.world_mut());
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval::<String>("local s = table.concat(SEEN, ' ') SEEN = {} return s")
            .unwrap()
    }

    /// `chunk`'s answer with the `World` lent, as a verb reads it.
    fn live<R: mlua::FromLuaMulti>(app: &mut App, chunk: &str) -> R {
        crate::ui_script::live::with_live_world(app.world_mut(), |s| s.eval::<R>(chunk))
            .expect("a VM")
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
            hidden: false,
        };
        assert_eq!(run(&mut app, vec![start(7)]), "START:player:Lesser Heal:7");
        assert_eq!(
            live::<String>(
                &mut app,
                r#"local n, r, t, i, a, b, ts, id = UnitCastingInfo("player")
                   return n .. "|" .. r .. "|" .. a .. "|" .. b .. "|" .. id"#
            ),
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
            live::<i64>(&mut app, "return select('#', UnitCastingInfo('player'))"),
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
            hidden: false,
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
        // A hidden start keeps its immunities and opens nothing (`0x80694e`).
        let hidden = CastBarEdge::Start {
            caster: ME,
            spell_id: HEAL,
            cast_count: 12,
            cast_time_ms: 1500,
            immunities: Some((1, 2)),
            hidden: true,
        };
        assert_eq!(run(&mut app, vec![hidden]), "");
        assert_eq!(
            app.world().resource::<UnitCasts>().immunities.get(&ME),
            Some(&(1, 2))
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

    /// The not-interruptible evaluation (`0x7262e0`) over its arms.
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
        let known = std::collections::BTreeSet::from([KICK]);
        {
            let spells = app.world().resource::<Spells>().clone();
            book_updated(
                &mut app.world_mut().resource_mut::<InterruptSchools>(),
                &known,
                Some(&spells),
            );
        }
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
            hidden: false,
        };
        run(&mut app, vec![start]);
        let shield =
            |app: &mut App| live::<bool>(app, r#"return select(9, UnitCastingInfo("target"))"#);
        assert!(shield(&mut app), "the immunity aura shields the cast");
        // The aura goes; the aura handler's re-read clears the shield and says so (`0x72fd30`).
        app.world_mut()
            .entity_mut(mob)
            .insert(crate::unit_auras::UnitAuras::default());
        app.world_mut()
            .resource_mut::<UiEvents>()
            .world(|w| auras_changed(w, MOB));
        assert_eq!(run(&mut app, vec![]), "INTERRUPTIBLE:target:nil:nil");
        assert!(!shield(&mut app));
    }

    /// `0x8020c0`: a running cast reopens with its back-dated window and cast count 0; a channel
    /// (AttributesEx `0x4`) opens as one and clears the cast's window.
    #[test]
    fn a_resumed_cast_bar_opens_back_dated() {
        const BLIZZARD: u32 = 10;
        let mut app = feed_app();
        let catalog = benilla_formats::SpellCatalog::from_displays(
            [
                (
                    HEAL,
                    SpellDisplay {
                        name: "Lesser Heal".into(),
                        ..Default::default()
                    },
                ),
                (
                    BLIZZARD,
                    SpellDisplay {
                        name: "Blizzard".into(),
                        attributes_ex: 0x4,
                        ..Default::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        );
        app.world_mut().resource_mut::<Spells>().catalog = catalog;
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(
                "local f = CreateFrame('Frame') f:RegisterEvent('UNIT_SPELLCAST_CHANNEL_START')
                 f:SetScript('OnEvent', function(self, e, u, n, r, c)
                     table.insert(SEEN, e:sub(16) .. ':' .. u .. ':' .. n .. ':' .. c) end)",
            )
            .unwrap();
        let resume = |spell_id, remaining_ms, total_ms| CastBarEdge::Resume {
            caster: ME,
            spell_id,
            remaining_ms,
            total_ms,
        };
        let window = |app: &mut App, verb: &str| {
            live::<String>(
                app,
                &format!("local n, r, t, i, a, b = {verb}('player') return a .. '|' .. b"),
            )
        };
        assert_eq!(
            run(&mut app, vec![resume(HEAL, 1_000, 1_500)]),
            "START:player:Lesser Heal:0"
        );
        assert_eq!(window(&mut app, "UnitCastingInfo"), "99500|101000");
        assert_eq!(
            run(&mut app, vec![resume(BLIZZARD, 3_000, 8_000)]),
            "CHANNEL_START:player:Blizzard:0"
        );
        assert_eq!(window(&mut app, "UnitChannelInfo"), "95000|103000");
        assert_eq!(
            live::<i64>(&mut app, "return select('#', UnitCastingInfo('player'))"),
            0,
            "the channel setter clears the cast"
        );
        // No `Spell.dbc` row: nothing.
        assert_eq!(run(&mut app, vec![resume(99, 1_000, 2_000)]), "");
    }

    /// A press's cancel fires inside the system holding the VM as the cancel returns
    /// (`0x806200`): `INTERRUPTED` while the unit still holds its cast, then the cast cleared and
    /// `STOP`, before the system makes any other call into Lua.
    #[test]
    fn a_press_cancel_fires_inside_the_holding_system() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = feed_app();
        app.init_resource::<PressEdges>();
        let start = CastBarEdge::Start {
            caster: ME,
            spell_id: HEAL,
            cast_count: 3,
            cast_time_ms: 1500,
            immunities: None,
            hidden: false,
        };
        assert_eq!(run(&mut app, vec![start]), "START:player:Lesser Heal:3");
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(
                "HELD = {} local f = CreateFrame('Frame')
                 f:RegisterEvent('UNIT_SPELLCAST_INTERRUPTED') f:RegisterEvent('UNIT_SPELLCAST_STOP')
                 f:SetScript('OnEvent', function(_, e)
                     HELD[#HELD + 1] = e:sub(16) .. '=' .. tostring((UnitCastingInfo('player'))) end)",
            )
            .unwrap();
        let after = app
            .world_mut()
            .run_system_once(
                |(mut script, _live): (NonSendMut<UiScript>, crate::ui_script::LendLive),
                 mut press: PressSignals| {
                    press.cancelled(ME);
                    assert_eq!(press.pending(), [CastBarEdge::Cancelled { caster: ME }]);
                    press.fire(&mut script);
                    script
                        .eval::<String>("return table.concat(HELD, ' ')")
                        .unwrap()
                },
            )
            .unwrap();
        assert_eq!(after, "INTERRUPTED=Lesser Heal STOP=nil");
        assert!(!app.world().resource::<UnitCasts>().casting(ME));
    }
}
