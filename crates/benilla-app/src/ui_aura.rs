//! The aura feed: every unit's [`UnitAuras`] table, the player's with the rest, as the ordered
//! [`AuraState`] lists the aura bindings read, and the unit-token resolver's inputs the bindings
//! reach them through ([`units`]). A list walks its table's display order (`0x72c510`) through the
//! display filter; a group member or pet with no live object lists its roster record instead.

mod units;

use bevy::prelude::*;

use benilla_formats::{SpellCatalog, SpellDurationCatalog};
use benilla_protocol::messages::{RecordAura, AURA_UPDATE_FLAG_HARMFUL};
use benilla_ui::script::{AuraState, ScriptValue, TrackingState, UiScript};

use crate::char_select::ClientState;
use crate::net::{
    ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, Reputations, SelfPlayer,
};
use crate::target::{can_assist, Factions, Selection};
use crate::ui_action::Spells;
use crate::ui_script::UiInput;
use crate::ui_unit::UnitFeed;
use crate::unit_auras::{UnitAura, UnitAuras};

/// [`feed_auras`] fires `PLAYER_AURAS_CHANGED` and `UNIT_AURA` inline, so a feed whose state their
/// handlers re-read (the stance feed) runs `.before(AuraEvents)` or they read last frame's.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct AuraEvents;

/// Adds the aura feed and the `CancelPlayerBuff`/`CancelTrackingBuff` drain.
pub(crate) struct UiAuraPlugin;

impl Plugin for UiAuraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AuraFeedMemory>()
            // Feed before the VM dispatch, drain after it.
            .add_systems(Update, feed_auras.in_set(UnitFeed).in_set(AuraEvents))
            .add_systems(Update, drain_aura_cancels.after(UiInput))
            .add_systems(OnExit(ClientState::InWorld), end_session_aura_state);
    }
}

/// The feed's edge memory, behind a [`crate::ui_script::VmMemo`], so a `/reload`'s new VM reads it
/// fresh and `UNIT_AURA` re-fires.
#[derive(Resource, Default)]
struct AuraFeedMemory {
    /// What this VM was last told.
    vm: crate::ui_script::VmMemo<AuraFeedMemo>,
    /// When [`trace_timers`] last printed: host state, outliving any one VM.
    traced_at: f64,
}

/// The per-VM `UNIT_AURA` edge keys.
#[derive(Default)]
struct AuraFeedMemo {
    present: bool,
    /// The player's list inputs as last pushed: a refresh moves an expiry, an apply a spell.
    last: Vec<AuraInput>,
    /// The tracking spell: a tracking switch changes no list, but the minimap needs the event.
    tracking_last: Option<u32>,
    /// Every other unit's list, by guid, the resolver's inputs and their pushes.
    units: units::UnitAuras,
}

/// The `BENILLA_AURA_TRACE` period in seconds; a set value that is not a positive number means 1 s.
fn trace_period() -> Option<f64> {
    static PERIOD: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
    *PERIOD.get_or_init(|| {
        let v = std::env::var("BENILLA_AURA_TRACE").ok()?;
        Some(
            v.trim()
                .parse::<f64>()
                .ok()
                .filter(|p| *p > 0.0)
                .unwrap_or(1.0),
        )
    })
}

/// One trace tick: each of the player's listed auras with its time left.
fn trace_timers(script: &UiScript, list: &[AuraState], bevy_now: f64) {
    let script_now = script.now();
    info!(
        "aura trace: {} aura(s), GetTime()={script_now:.2} app clock={bevy_now:.2} (skew {:+.2})",
        list.len(),
        script_now - bevy_now
    );
    for (i, a) in list.iter().enumerate() {
        let left = if a.expiration_time > 0.0 {
            format!(
                "{:.1}s of {:.1}s",
                a.expiration_time - script_now,
                a.duration
            )
        } else {
            "permanent".to_string()
        };
        info!(
            "  #{:<2} spell {:>5}  {:<26}  {:<6}  left {left}",
            i + 1,
            a.spell_id,
            a.name.as_deref().unwrap_or("<unknown spell>"),
            if a.helpful { "buff" } else { "debuff" },
        );
    }
}

/// What one aura of a list is built from: a list is a pure function of these, the spell tables
/// and the clock offset. `expires_at` is on the real clock, so the inputs hold still until the
/// table moves.
#[derive(Clone, Copy, Debug, PartialEq)]
struct AuraInput {
    spell_id: u32,
    stacks: u8,
    flags: u8,
    caster: u64,
    duration_ms: u32,
    expires_at: f64,
}

impl From<&UnitAura> for AuraInput {
    fn from(a: &UnitAura) -> Self {
        Self {
            spell_id: a.spell_id,
            stacks: a.stacks,
            flags: a.flags,
            caster: a.caster,
            duration_ms: a.duration_ms,
            expires_at: a.expires_at,
        }
    }
}

/// The spell tables a list reads, and `GetTime()` minus the real clock, which rebases an expiry.
#[derive(Clone, Copy)]
struct ListCtx<'a> {
    catalog: Option<&'a SpellCatalog>,
    durations: Option<&'a SpellDurationCatalog>,
    clock_offset: f64,
}

/// One aura as `0x6147c0` pushes it: helpful unless flagged harmful (`0x72ca9e`), cancelable by
/// its flag (`0x72cad0`), and a timed aura's duration and expiry in seconds on the `GetTime()`
/// clock (`0x61493d`), both 0 for an untimed one.
fn aura_state(ctx: ListCtx, a: &AuraInput) -> AuraState {
    let catalog = ctx.catalog;
    let display = catalog.and_then(|cat| cat.get(a.spell_id));
    let cancelable = a.flags & benilla_protocol::messages::AURA_UPDATE_FLAG_CANCELABLE != 0;
    let (duration, expiration_time) = if a.expires_at > 0.0 {
        (
            f64::from(a.duration_ms) / 1000.0,
            a.expires_at + ctx.clock_offset,
        )
    } else {
        (0.0, 0.0)
    };
    AuraState {
        spell_id: a.spell_id,
        name: display.map(|d| d.name.clone()),
        icon: display.and_then(|d| d.icon.clone()),
        count: a.stacks,
        debuff_type: display
            .zip(catalog)
            .and_then(|(d, cat)| cat.dispel_name(d))
            .map(str::to_string),
        duration,
        expiration_time,
        helpful: a.flags & AURA_UPDATE_FLAG_HARMFUL == 0,
        cancelable,
        until_cancelled: until_cancelled(
            display,
            display.and_then(|d| ctx.durations.and_then(|c| c.get(d.duration_index))),
            cancelable,
            expiration_time,
        ),
        channeled: display.is_some_and(|d| d.attributes_ex & SPELL_ATTR_EX_IS_CHANNELED != 0),
        rank: display.and_then(|d| d.rank.clone()),
        caster: a.caster,
        // Not built: the steal test (`0x53d680`) and the consolidation flag (`0x6149c1`).
        ..Default::default()
    }
}

/// The list [`AuraInput`]s make.
fn aura_list(ctx: ListCtx, inputs: &[AuraInput]) -> Vec<AuraState> {
    inputs.iter().map(|a| aura_state(ctx, a)).collect()
}

/// A unit's listed auras through the display filter. `buffs_visible` false drops the helpful
/// ones, the unit-level gate's nil at every index ([`buffs_visible_on`]).
fn table_inputs<'a>(
    table: &'a UnitAuras,
    catalog: Option<&'a SpellCatalog>,
    buffs_visible: bool,
) -> impl Iterator<Item = AuraInput> + 'a {
    table
        .listed()
        .filter(move |a| buffs_visible || a.is_harmful())
        .filter(move |a| shown_in_aura_ui(catalog, a.spell_id))
        .map(AuraInput::from)
}

/// A roster record's aura block as `UnitAura`'s record walk reads it (`0x614a80`-`0x614b9c`):
/// ascending slot, the display filter alone, no unit gate. Each aura's own flags byte says
/// harmful (`0x80`) and cancelable (`0x10`); the stack count is always 1 and nothing is timed
/// (`0x614bab`).
///
/// Deviation: 12340 stops at the first empty slot (`0x614a83`), so a hole an expired aura leaves
/// hides every aura past it; this walks every occupied slot.
fn record_inputs<'a>(
    auras: impl Iterator<Item = RecordAura> + 'a,
    catalog: Option<&'a SpellCatalog>,
) -> impl Iterator<Item = AuraInput> + 'a {
    auras
        .filter(move |a| a.spell_id != 0 && shown_in_aura_ui(catalog, a.spell_id))
        .map(|a| AuraInput {
            spell_id: a.spell_id,
            stacks: 1,
            flags: a.flags,
            caster: 0,
            duration_ms: 0,
            expires_at: 0.0,
        })
}

/// `UnitBuff 0x519500`'s unit-level gate, before any slot: a unit's buffs are listed only if it
/// has `UNIT_FLAG_AURAS_VISIBLE` or the uncharmed player can assist it (`0x5195aa`-`0x5195f7`),
/// else every index is nil; `UnitDebuff` has no gate. vmangos sets the flag for Detect Magic
/// (`UnitDefines.h:572`) and for a GM viewer (`Object.cpp:765-766`), so it is read as received.
fn buffs_visible_on(
    store: &ObjectStore,
    self_store: Option<&ObjectStore>,
    factions: Option<&Factions>,
    reputations: &Reputations,
    owner_store: impl FnOnce(u64) -> Option<ObjectStore>,
) -> bool {
    if store.0.unit_flags() & UNIT_FLAG_AURAS_VISIBLE != 0 {
        return true;
    }
    // While charmed you assist nobody (`0x5195f1`-`0x5195f7`).
    if self_store.is_some_and(|s| s.0.unit_charmed_by().is_some()) {
        return false;
    }
    can_assist(Some(store), factions, reputations, self_store, owner_store)
}

/// `UNIT_FIELD_FLAGS` bit 27, `0x08000000` (`0x5195b6`).
const UNIT_FLAG_AURAS_VISIBLE: u32 = 0x0800_0000;

/// `AttributesEx & 0x4` (vmangos `SpellDefines.h:834`), the cancel gate's DBC arm (`0x4e4a10`).
const SPELL_ATTR_EX_IS_CHANNELED: u32 = 0x4;

/// `GetPlayerBuff`'s second return, cache record `+0xc`, set by `BuildBuffRecord 0x4e44b0` when
/// `DurationIndex` names an infinite `SpellDuration.dbc` row, base < 0 and per level <= 0
/// (`0x4e456e`-`0x4e457a`), or no row (`0x4e4580`); or when the aura is not cancelable and an
/// effect is an area aura, `0x23`, `0x77`, `0x80` or `0x81` (`0x4e4588`-`0x4e45be`).
/// `BuffFrame.lua:124` then shows no countdown. Deviation: on a catalog miss the reference keeps
/// the record's stale value (`0x4e4533`, `0x4e4550` skip the write); this answers from the joined
/// expiry, because a stale value is no mechanism to reproduce.
fn until_cancelled(
    display: Option<&benilla_formats::SpellDisplay>,
    duration_row: Option<&benilla_formats::SpellDuration>,
    cancelable: bool,
    expiration_time: f64,
) -> bool {
    let Some(d) = display else {
        return expiration_time == 0.0;
    };
    let permanent = match duration_row {
        Some(row) => row.base_ms < 0 && row.per_level_ms <= 0,
        // A negative, out-of-range or empty `DurationIndex`: all three reach `0x4e4580`.
        None => true,
    };
    permanent
        || (!cancelable
            && d.effects
                .iter()
                .any(|e| matches!(e, 0x23 | 0x77 | 0x80 | 0x81)))
}

/// The filter of every aura display, the player's bar and other units' rows alike: never-display
/// and tracking spells are hidden. The player's rebuild tests the attributes
/// (`0x4e42b6`-`0x4e42c8`), then diverts a tracking aura to the tracking global instead of the
/// cache (`0x4e42d6`-`0x4e4308`); `IsAuraDisplayable 0x519860` hides both from other units' rows.
/// Deviation: on other units' rows an id the catalog lacks stays visible, because every wire id is
/// a real spell and failing closed would blank every aura on a catalog failure; the reference
/// skips one there (`[0xc0d78c]`), though its player cache inserts one, as here.
fn shown_in_aura_ui(catalog: Option<&SpellCatalog>, spell_id: u32) -> bool {
    catalog
        .and_then(|c| c.get(spell_id))
        .is_none_or(|d| !d.hidden_from_aura_bar() && !d.tracking_aura())
}

/// The player's tracking aura, the reference's global `0xbc6378`: the rebuild's walk overwrites
/// it per tracking aura, so the last wins (`0x4e4302`), after the attribute test; a catalog miss
/// cannot be told apart and goes to the bar. Read by `GetTrackingTexture`, `CancelTrackingBuff`
/// and `GameTooltip:SetTrackingSpell` ([`UiScript::set_tracking`]).
fn tracking_state_of(catalog: Option<&SpellCatalog>, auras: &[&UnitAura]) -> Option<TrackingState> {
    let catalog = catalog?;
    auras.iter().rev().find_map(|a| {
        let d = catalog.get(a.spell_id)?;
        (!d.hidden_from_aura_bar() && d.tracking_aura()).then(|| TrackingState {
            spell_id: a.spell_id,
            name: Some(d.name.clone()),
            icon: d.icon.clone(),
            cancelable: a.is_cancelable(),
        })
    })
}

/// The units the resolver's guid globals name, from `UnitTokens`' own sources.
#[derive(bevy::ecs::system::SystemParam)]
struct AuraUnits<'w> {
    selection: Res<'w, Selection>,
    pet: Res<'w, crate::ui_pet::PetBar>,
    // `Option`: a UI-only harness lacks their plugins; without the party's no group token names
    // anyone.
    group: Option<Res<'w, crate::ui_party::GroupState>>,
    hovered: Option<Res<'w, crate::target::Hovered>>,
    hovered_go: Option<Res<'w, crate::target::HoveredObject>>,
    interact: Option<Res<'w, crate::ui_session::InteractNpc>>,
}

fn feed_auras(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<(&ObjectStore, &Guid, Option<&UnitAuras>), With<SelfPlayer>>,
    units: AuraUnits,
    stores: Query<&ObjectStore>,
    tables: Query<&UnitAuras>,
    spells: Option<Res<Spells>>,
    index: Res<GuidIndex>,
    // The `UnitBuff` gate's inputs, the same reaction the selection ring and nameplates use.
    factions: Option<Res<Factions>>,
    reputations: Res<Reputations>,
    time: Res<Time<Real>>,
    mut mem: ResMut<AuraFeedMemory>,
) {
    let Some(mut script) = script else {
        return;
    };
    let Ok((store, self_guid, self_table)) = self_q.single() else {
        // No avatar between a worldport's frames: the lists stand until it streams back.
        return;
    };

    let bevy_now = time.elapsed_secs_f64();
    let ctx = ListCtx {
        catalog: spells.as_ref().map(|s| &s.catalog),
        durations: spells.as_ref().map(|s| &s.durations),
        clock_offset: script.now() - bevy_now,
    };
    let catalog = ctx.catalog;
    let empty = UnitAuras::default();
    let self_table = self_table.unwrap_or(&empty);
    // The player's own buffs are never gated.
    let inputs: Vec<AuraInput> = table_inputs(self_table, catalog, true).collect();
    let list = aura_list(ctx, &inputs);

    // Before the memory update, so a tracking change joins the edge key.
    let tracking = tracking_state_of(catalog, &self_table.listed().collect::<Vec<_>>());
    let tracking_spell = tracking.as_ref().map(|t| t.spell_id);

    if let Some(period) = trace_period() {
        if bevy_now - mem.traced_at >= period {
            mem.traced_at = bevy_now;
            trace_timers(&script, &list, bevy_now);
        }
    }

    // This VM's keys: a `/reload`'s new VM starts empty, so every edge re-fires.
    let memo = mem.vm.get(&script);

    // Fire on a discrete change, never on the countdown; a refresh and a tracking switch count.
    let changed = !memo.present || inputs != memo.last || tracking_spell != memo.tracking_last;
    memo.last = inputs;
    memo.tracking_last = tracking_spell;
    memo.present = true;

    if changed && std::env::var_os("BENILLA_AURA_DUMP").is_some() {
        info!(
            "aura dump: {} aura(s) on the player",
            self_table.iter().count()
        );
        for a in self_table.listed() {
            let name = catalog
                .and_then(|cat| cat.get(a.spell_id))
                .map(|d| d.name.as_str())
                .unwrap_or("<unknown spell>");
            info!(
                "  slot {:>3}  spell {:>5}  {:<26}  {:<6}  flags {:#04x}  caster {:#x}",
                a.slot,
                a.spell_id,
                name,
                if a.is_harmful() { "debuff" } else { "buff" },
                a.flags,
                a.caster,
            );
        }
    }

    let held = |guid: u64| index.0.get(&guid).and_then(|&e| stores.get(e).ok());
    let table_of = |guid: u64| index.0.get(&guid).and_then(|&e| tables.get(e).ok());
    let pet_guid = units.pet.spells.pet_guid;
    // `UnitBuff`'s unit gate on a live unit. `unit` is never `store`, the player's, which is the
    // gate's second argument. The pet is ungated; the reference gates a pet too, but `CanAssist`'s
    // player-controlled arm (`0x60673e`-`0x60679f`, the controlling players' duel and FFA-PvP
    // state) passes for your own pet, leaving only the charm, selectable and reaction clauses to
    // hide its buffs.
    let buffs_visible = |guid: u64, unit: &ObjectStore| {
        guid == pet_guid
            || buffs_visible_on(
                unit,
                Some(store),
                factions.as_deref(),
                &reputations,
                |owner| held(owner).cloned(),
            )
    };
    let target_guid = units.selection.guid.unwrap_or(0);
    let catalog_moved = spells.as_ref().is_some_and(|s| s.is_changed());
    let unit_changed = memo.units.feed(
        &mut script,
        units::Inputs {
            group: units.group.as_deref(),
            me: (self_guid.0, changed),
            pet: pet_guid,
            target: target_guid,
            // The world pick, as `UnitTokens` resolves `"mouseover"`.
            mouseover: units
                .hovered
                .as_deref()
                .and_then(|h| {
                    h.mouseover(&units.hovered_go.as_deref().copied().unwrap_or_default())
                })
                .map_or(0, |(_, guid)| guid),
            npc: units.interact.as_deref().and_then(|i| i.1).unwrap_or(0),
            held,
            table_of,
            buffs_visible,
            ctx,
            catalog_moved,
        },
    );

    if unit_changed.contains(&"target") && std::env::var_os("BENILLA_AURA_DUMP").is_some() {
        let l = if target_guid == self_guid.0 {
            Some(list.as_slice())
        } else {
            memo.units.list(target_guid)
        };
        let l = l.unwrap_or_default();
        info!("aura dump: {} aura(s) on the target rows", l.len());
        for a in l {
            info!(
                "  spell {:>5}  {:<26}  {:<6}  {}",
                a.spell_id,
                a.name.as_deref().unwrap_or("<unknown spell>"),
                if a.helpful { "buff" } else { "debuff" },
                a.debuff_type.as_deref().unwrap_or("-"),
            );
        }
    }

    script.set_player_auras(list);
    script.set_tracking(tracking);
    if changed {
        script.fire_event("UNIT_AURA", vec![ScriptValue::Str("player".into())]);
        // The 1.12.1 buff bar's own event, with no arguments; `MiniMapTrackingFrame` registers it.
        script.fire_event("PLAYER_AURAS_CHANGED", vec![]);
    }
    // To every token naming a unit whose list moved (`0x515e50`); a token naming a new unit fires
    // too, and a token clearing fires nothing, since the frames react to their own events
    // (`PLAYER_TARGET_CHANGED`, `UNIT_PET`, `PARTY_MEMBERS_CHANGED`).
    for token in unit_changed {
        script.fire_event("UNIT_AURA", vec![ScriptValue::Str(token.into())]);
    }
}

/// The session's end: drops the lists, so nothing reaches the next character's frames. The
/// tables die with their units.
fn end_session_aura_state(script: Option<NonSendMut<UiScript>>) {
    if let Some(mut script) = script {
        script.clear_auras();
        script.set_unit_guids(&Default::default());
        script.set_tracking(None);
    }
    // No `AuraFeedMemory` reset: its edge keys die with the VM.
}

/// A `CMSG_CANCEL_AURA` per queued `CancelPlayerBuff` or `CancelTrackingBuff`, naming the spell,
/// not the slot (`0x6e7040`).
fn drain_aura_cancels(script: Option<NonSendMut<UiScript>>, net: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for spell_id in script.take_cancel_aura_requests() {
        let _ = net.0.send(ClientCommand::CancelAura { spell_id });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{AURA_UPDATE_FLAG_CANCELABLE, AURA_UPDATE_FLAG_SELF_CAST};

    const NO_SPELLS: ListCtx = ListCtx {
        catalog: None,
        durations: None,
        clock_offset: 0.0,
    };

    /// [`table_inputs`] as the list.
    fn other_unit_auras(table: &UnitAuras, buffs_visible: bool) -> Vec<AuraState> {
        let inputs: Vec<AuraInput> = table_inputs(table, None, buffs_visible).collect();
        aura_list(NO_SPELLS, &inputs)
    }

    #[test]
    fn a_timed_aura_lists_its_duration_and_expiry_on_the_script_clock() {
        let a = AuraInput {
            spell_id: 43039,
            stacks: 1,
            flags: AURA_UPDATE_FLAG_SELF_CAST | AURA_UPDATE_FLAG_CANCELABLE | 1,
            caster: 0x2a,
            duration_ms: 60_000,
            expires_at: 130.0,
        };
        let ctx = ListCtx {
            clock_offset: 4870.0,
            ..NO_SPELLS
        };
        let s = aura_state(ctx, &a);
        assert_eq!((s.duration, s.expiration_time), (60.0, 5000.0));
        assert!(s.helpful && s.cancelable);
        assert_eq!(s.caster, 0x2a);
        let untimed = aura_state(
            ctx,
            &AuraInput {
                expires_at: 0.0,
                duration_ms: 0,
                flags: AURA_UPDATE_FLAG_HARMFUL | 1,
                ..a
            },
        );
        assert_eq!((untimed.duration, untimed.expiration_time), (0.0, 0.0));
        assert!(!untimed.helpful && !untimed.cancelable);
    }

    /// The record walk reads harmful and cancelable off each aura's own flags (`0x614a8c`,
    /// `0x614b2c`), whatever its slot.
    #[test]
    fn a_roster_record_reads_harmful_and_cancelable_off_its_flags() {
        let aura = |slot, spell_id, flags| RecordAura {
            slot,
            spell_id,
            flags,
        };
        let block = [aura(0, 5916, 0x19), aura(1, 22766, 0x81), aura(3, 0, 0x19)];
        let inputs: Vec<AuraInput> = record_inputs(block.into_iter(), None).collect();
        let read: Vec<(bool, bool)> = aura_list(NO_SPELLS, &inputs)
            .iter()
            .map(|a| (a.helpful, a.cancelable))
            .collect();
        assert_eq!(
            read,
            [(true, true), (false, false)],
            "the empty slot is skipped"
        );
    }

    // == `UnitBuff`'s unit-level gate (`0x519500`) ==
    // A stealthed Ridge Stalker carries 5916 helpful and 22766 harmful; the reference draws only
    // 22766, as `UnitBuff` lists no hostile creature's buffs.

    /// `UNIT_FIELD_FLAGS`'s index.
    const F_FLAGS: u16 = 46;
    /// `UNIT_FIELD_CHARMEDBY`'s low dword; the high half is the next index.
    const F_CHARMEDBY: u16 = 10;

    fn ridge_stalker_slots(flags: u32) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
            F_FLAGS, flags,
        )]))
    }

    fn ridge_stalker_table() -> UnitAuras {
        UnitAuras::of(7, &[(0, 5916, 1), (1, 22766, AURA_UPDATE_FLAG_HARMFUL | 1)])
    }

    /// Denied, `UnitBuff` is nil at every index; `UnitDebuff` is untouched.
    #[test]
    fn the_unit_gate_drops_the_whole_helpful_half_and_never_the_harmful_one() {
        let table = ridge_stalker_table();

        let denied = other_unit_auras(&table, false);
        assert_eq!(
            denied.iter().map(|a| a.spell_id).collect::<Vec<_>>(),
            [22766],
            "a hostile creature shows its debuffs and NOT its buffs — `UnitBuff`'s unit gate"
        );
        assert!(denied.iter().all(|a| !a.helpful));

        let allowed = other_unit_auras(&table, true);
        assert_eq!(
            allowed.iter().map(|a| a.spell_id).collect::<Vec<_>>(),
            [22766, 5916],
            "with the gate open both enumerate, in the table's display order"
        );
    }

    /// vmangos sets `UNIT_FLAG_AURAS_VISIBLE` for a GM viewer too (`Object.cpp:765-766`).
    #[test]
    fn detect_magic_or_gm_mode_opens_the_gate_on_the_flag_alone() {
        let reputations = Reputations::default();
        // No factions catalog: the reaction is neutral (3), below the `>= 4` bar.
        let hostile = ridge_stalker_slots(0);
        assert!(
            !buffs_visible_on(&hostile, None, None, &reputations, |_| None),
            "neutral reaction, not player-controlled, no PvP flag → no buffs"
        );

        let detected = ridge_stalker_slots(UNIT_FLAG_AURAS_VISIBLE);
        assert!(
            buffs_visible_on(&detected, None, None, &reputations, |_| None),
            "UNIT_FLAG_AURAS_VISIBLE alone opens it, whatever the reaction says"
        );
    }

    /// `NOT_SELECTABLE` is `CanAssist`'s; the charm clause is `UnitBuff`'s (`0x5195f1`-`0x5195f7`).
    #[test]
    fn not_selectable_and_being_charmed_each_close_the_gate_on_their_own() {
        let reputations = Reputations::default();

        // AURAS_VISIBLE is tested before `CanAssist`, so it wins over NOT_SELECTABLE.
        let both = ridge_stalker_slots(UNIT_FLAG_AURAS_VISIBLE | (1 << 25));
        assert!(
            buffs_visible_on(&both, None, None, &reputations, |_| None),
            "AURAS_VISIBLE is tested FIRST and short-circuits CanAssist entirely"
        );

        // Being charmed closes the gate before `CanAssist` runs.
        let friendly_pc = ridge_stalker_slots(0x8);
        let charmed_self = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (F_CHARMEDBY, 0x1234),
            (F_CHARMEDBY + 1, 0),
        ]));
        assert!(
            !buffs_visible_on(
                &friendly_pc,
                Some(&charmed_self),
                None,
                &reputations,
                |_| None
            ),
            "activePlayer.CHARMEDBY != 0 → no buffs on anyone"
        );
    }

    /// The two clauses of `0x4e452e`-`0x4e45c5`.
    #[test]
    fn until_cancelled_follows_the_duration_row_and_the_area_aura_override() {
        use benilla_formats::{SpellDisplay, SpellDuration};

        let row = |base_ms: i32, per_level_ms: i32| SpellDuration {
            base_ms,
            per_level_ms,
            max_ms: base_ms,
        };
        // Real `SpellDuration.dbc` rows: 21 `{-1, 0}` permanent, 30 `{1_800_000, 0}` 30 minutes,
        // 427 `{-600_000, 60_000}`, whose negative base alone is not permanence.
        let permanent_row = row(-1, 0);
        let timed_row = row(1_800_000, 0);
        let scaling_row = row(-600_000, 60_000);

        let spell = |effects: [u32; 3]| SpellDisplay {
            effects,
            ..SpellDisplay::default()
        };
        let ordinary = spell([6, 0, 0]);
        // 0x23 is `SPELL_EFFECT_APPLY_AREA_AURA_PARTY`, one of the four in the override set.
        let area = spell([0x23, 0, 0]);

        // Clause 1: the duration row decides, cancelable or not.
        assert!(!until_cancelled(
            Some(&ordinary),
            Some(&timed_row),
            true,
            0.0
        ));
        assert!(!until_cancelled(
            Some(&ordinary),
            Some(&timed_row),
            false,
            0.0
        ));
        assert!(until_cancelled(
            Some(&ordinary),
            Some(&permanent_row),
            true,
            0.0
        ));
        assert!(
            !until_cancelled(Some(&ordinary), Some(&scaling_row), true, 0.0),
            "a negative base with a positive per-level term is a SCALING duration, not permanence"
        );
        // No row: permanent (`0x4e4580`).
        assert!(until_cancelled(Some(&ordinary), None, true, 0.0));

        // Clause 2: the area-aura override applies only to a non-cancelable aura (`0x4e4588`).
        assert!(until_cancelled(Some(&area), Some(&timed_row), false, 0.0));
        assert!(
            !until_cancelled(Some(&area), Some(&timed_row), true, 0.0),
            "a CANCELABLE aura skips the effect scan entirely"
        );

        // A catalog miss answers from the joined expiry.
        assert!(until_cancelled(None, None, false, 0.0));
        assert!(!until_cancelled(None, None, false, 1234.0));
    }
}
