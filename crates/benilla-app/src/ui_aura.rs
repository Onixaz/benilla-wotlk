//! The aura reads the bindings make at the call (`0x6147c0`): a held unit's [`UnitAuras`] table
//! in display order (`0x72c510`) through the display filter, or, for a unit the object manager does
//! not hold, its roster record's block. [`AuraLive`] is the live view's part that walks them.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_formats::{SpellCatalog, SpellDisplay, SpellDurationCatalog};
use benilla_protocol::messages::{
    RecordAura, AURA_UPDATE_FLAG_HARMFUL, AURA_UPDATE_FLAG_SELF_CAST,
};
use benilla_ui::script::{AuraFacts, AuraState, UnitAuraSlots};

use crate::char_select::ClientState;
use crate::net::{ClientCommand, GuidIndex, NetCommands, ObjectStore, Reputations};
use crate::target::{is_friendly, Factions};
use crate::ui_action::Spells;
use crate::ui_script::UiDrain;
use crate::unit_auras::{UnitAura, UnitAuras};

/// Adds the `CancelUnitBuff`/`SetTracking` cancel drain and the session's tracking reset.
pub(crate) struct UiAuraPlugin;

impl Plugin for UiAuraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            crate::ui_script::raises::then_fire(drain_aura_cancels).in_set(UiDrain),
        )
        .add_systems(OnExit(ClientState::InWorld), end_session_tracking);
    }
}

/// What one listed aura is built from, with the spell tables and the clock offset. `expires_at` is
/// on the real clock.
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

/// The spell tables a walk reads, the viewer its filter reads, and `GetTime()` minus the real
/// clock, which rebases an expiry.
#[derive(Clone, Copy)]
struct ListCtx<'a> {
    catalog: Option<&'a SpellCatalog>,
    durations: Option<&'a SpellDurationCatalog>,
    viewer: Viewer,
    clock_offset: f64,
    /// What the `RAID` list reads off the book; `None` lists nothing there.
    raid: Option<RaidBook<'a>>,
    /// The book's steal mask where we can attack the unit walked, else 0 (`0x53d683`, `0x53d6b5`).
    steal_mask: u32,
}

/// The book's side of the finder's second list (`0x72c510`): the player's spells and its pet's,
/// and the dispel types its dispel effects name.
#[derive(Clone, Copy)]
struct RaidBook<'a> {
    known: &'a std::collections::BTreeSet<u32>,
    pet: &'a [benilla_protocol::messages::PetActionEntry],
    dispel_types: &'a [i32],
}

/// The dispel types the book registers at learn (`0x542170`): each `SPELL_EFFECT_DISPEL` (38)
/// effect not aimed at an enemy (implicit target 6) names its misc value once.
pub(crate) fn dispel_types(
    known: &std::collections::BTreeSet<u32>,
    catalog: &SpellCatalog,
) -> Vec<i32> {
    let mut out = Vec::new();
    for d in known.iter().filter_map(|&id| catalog.get(id)) {
        for i in 0..3 {
            let misc = d.effect_misc_value[i];
            if d.effects[i] == 38 && d.effect_implicit_target_a[i] != 6 && !out.contains(&misc) {
                out.push(misc);
            }
        }
    }
    out
}

/// The steal mask a known spell registers at learn (`0x542222`-`0x542270`): its first effect steals
/// a buff (126) from an enemy (implicit target 6), naming the class mask of its misc value.
pub(crate) fn steal_mask(
    known: &std::collections::BTreeSet<u32>,
    catalog: &SpellCatalog,
) -> Option<u32> {
    known
        .iter()
        .filter_map(|&id| catalog.get(id))
        .filter(|d| d.effects[0] == 126 && d.effect_implicit_target_a[0] == 6)
        .map(|d| catalog.dispel_mask(d.effect_misc_value[0] as u32))
        .next_back()
}

/// The area-aura effects (`0x76bed0`'s table): persistent area aura and the area auras.
const AREA_AURA_EFFECTS: [u32; 7] = [27, 35, 65, 119, 128, 129, 143];

/// `isStealable` (`0x53d680`, from `0x614999`): a buff, on a unit we can attack, with the book's
/// steal mask naming its dispel class; never a passive (`Attributes & 0x40`), a channeled
/// (`AttributesEx & 0x44`) or a not-stealable one (`AttributesEx4 & 0x40`), nor one with an area
/// aura among the effects it holds (the flag's effect bits).
fn can_steal(mask: u32, d: Option<&SpellDisplay>, flags: u8) -> bool {
    let Some(d) = d else {
        return false;
    };
    mask != 0
        && flags & AURA_UPDATE_FLAG_HARMFUL == 0
        && d.attributes & 0x40 == 0
        && !(0..3).any(|i| flags & (1 << i) != 0 && AREA_AURA_EFFECTS.contains(&d.effects[i]))
        && d.attributes_ex & 0x44 == 0
        && d.attributes_ex4 & 0x40 == 0
        && 1u32.wrapping_shl(d.dispel) & mask != 0
}

/// Whether the finder's second list holds this aura (`0x72c510`): a buff whose exact spell the
/// player or its pet knows and could cast on another (`0x53d8a0`: `AttributesExC & 0x4000` clear,
/// a first implicit target other than self, no area-aura or persistent-area effect), or a debuff
/// whose dispel type the book's dispels name (`0x53bdf0`).
fn in_raid_list(
    book: Option<RaidBook>,
    d: Option<&SpellDisplay>,
    spell_id: u32,
    harmful: bool,
) -> bool {
    let (Some(book), Some(d)) = (book, d) else {
        return false;
    };
    if harmful {
        return i32::try_from(d.dispel).is_ok_and(|t| book.dispel_types.contains(&t));
    }
    (book.known.contains(&spell_id) || book.pet.iter().any(|e| e.action() == spell_id))
        && d.attributes_ex3 & 0x4000 == 0
        && d.effect_implicit_target_a[0] != 1
        && !d
            .effects
            .iter()
            .any(|e| matches!(e, 27 | 35 | 65 | 119 | 128 | 129 | 143))
}

/// What the aura-show check (`0x53d580`) reads off the active player: the casters its caster-only
/// leg admits, the player and its pet, and the aura vision byte (`PLAYER_FIELD_BYTES2` byte 3).
#[derive(Clone, Copy, Debug, Default)]
struct Viewer {
    player: u64,
    pet: u64,
    vision: u8,
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
        consolidate: display.is_some_and(|d| d.attributes_ex7 & 0x1000_0000 != 0),
        raid: in_raid_list(
            ctx.raid,
            display,
            a.spell_id,
            a.flags & AURA_UPDATE_FLAG_HARMFUL != 0,
        ),
        stealable: can_steal(ctx.steal_mask, display, a.flags),
    }
}

/// The facts the finder's filter and name match read off `a` (`0x72ca9e`-`0x72cae3`).
fn aura_facts<'a>(ctx: ListCtx<'a>, a: &AuraInput) -> AuraFacts<'a> {
    let display = ctx.catalog.and_then(|cat| cat.get(a.spell_id));
    let harmful = a.flags & AURA_UPDATE_FLAG_HARMFUL != 0;
    AuraFacts {
        spell_id: a.spell_id,
        helpful: !harmful,
        cancelable: a.flags & benilla_protocol::messages::AURA_UPDATE_FLAG_CANCELABLE != 0,
        raid: in_raid_list(ctx.raid, display, a.spell_id, harmful),
        caster: a.caster,
        name: display.map(|d| d.name.as_str()),
        rank: display.and_then(|d| d.rank.as_deref()),
    }
}

/// A unit's listed auras through the display filter.
fn table_inputs<'a>(
    table: &'a UnitAuras,
    ctx: ListCtx<'a>,
) -> impl Iterator<Item = AuraInput> + 'a {
    table
        .listed()
        .filter(move |a| shown_in_aura_ui(ctx, a.spell_id, a.flags, a.caster))
        .map(AuraInput::from)
}

/// A roster record's aura block as `UnitAura`'s record walk reads it (`0x614a80`-`0x614b9c`):
/// slots from 0 up to the first empty one (`0x614a83`), the display filter alone, no unit gate,
/// and no caster, so a caster-only aura is never listed. Each aura's own flags byte says harmful
/// (`0x80`) and cancelable (`0x10`); the stack count is always 1 and nothing is timed (`0x614bab`).
fn record_inputs<'a>(
    auras: impl Iterator<Item = RecordAura> + 'a,
    ctx: ListCtx<'a>,
) -> impl Iterator<Item = AuraInput> + 'a {
    auras
        .enumerate()
        .take_while(|(i, a)| usize::from(a.slot) == *i && a.spell_id != 0)
        .map(|(_, a)| a)
        .filter(move |a| shown_in_aura_ui(ctx, a.spell_id, a.flags, 0))
        .map(|a| AuraInput {
            spell_id: a.spell_id,
            stacks: 1,
            flags: a.flags,
            caster: 0,
            duration_ms: 0,
            expires_at: 0.0,
        })
}

/// Aura type 295, unnamed and on no 12340 `Spell.dbc` row: the aura-effect mask's byte 36, bit
/// `0x80` (`unit+0xf20`, built by `0x727e70`), that `UnitAura`'s unit gate tests (`0x6147f3`).
const AURA_TYPE_HIDES_AURAS: u32 = 295;

/// The aura-effect mask's test for [`AURA_TYPE_HIDES_AURAS`]: a held aura whose `Spell.dbc` row
/// names it on an effect the aura's flags apply (bit `1 << effect`, `0x727f87`).
fn hides_its_auras(table: &UnitAuras, catalog: Option<&SpellCatalog>) -> bool {
    let Some(catalog) = catalog else {
        return false;
    };
    table.iter().any(|a| {
        catalog.get(a.spell_id).is_some_and(|d| {
            (0..3)
                .any(|e| a.flags & (1 << e) != 0 && d.effect_apply_aura[e] == AURA_TYPE_HIDES_AURAS)
        })
    })
}

/// `UnitAura`, `UnitBuff` and `UnitDebuff`'s unit-level gate (`0x6147f3`-`0x61482a`), and
/// `GameTooltip:SetUnitAura`'s (`0x625f33`): a unit carrying [`AURA_TYPE_HIDES_AURAS`] that is not
/// friendly to the player lists nothing at any index. The reference's other unit gate, a disabled
/// object (model flag `0x10000`) walking the roster record instead, is the unheld unit here.
fn auras_hidden_on(
    store: &ObjectStore,
    table: &UnitAuras,
    catalog: Option<&SpellCatalog>,
    friendly: impl FnOnce(&ObjectStore) -> bool,
) -> bool {
    hides_its_auras(table, catalog) && !friendly(store)
}

/// `AttributesEx & 0x4` (vmangos `SpellDefines.h:834`), the cancel gate's DBC arm.
const SPELL_ATTR_EX_IS_CHANNELED: u32 = 0x4;

/// `GetPlayerBuff`'s second return, cache record `+0xc`, set by the buff record's build when
/// `DurationIndex` names an infinite `SpellDuration.dbc` row, base < 0 and per level <= 0, or no
/// row; or when the aura is not cancelable and an effect is an area aura, `0x23`, `0x77`, `0x80` or
/// `0x81`. `BuffFrame.lua:124` then shows no countdown. Deviation: on a catalog miss the reference
/// keeps the record's stale value; this answers from the joined expiry, because a stale value is no
/// mechanism to reproduce.
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
        // A negative, out-of-range or empty `DurationIndex`: all three reach.
        None => true,
    };
    permanent
        || (!cancelable
            && d.effects
                .iter()
                .any(|e| matches!(e, 0x23 | 0x77 | 0x80 | 0x81)))
}

/// The filter of every aura display, the player's bar and other units' rows alike, as the display
/// index rebuild admits a slot (`0x72c510`): an id with no `Spell.dbc` row is never indexed
/// (`0x72c73d`-`0x72c75f`), so a server-only spell takes no `UnitAura` index; then the aura-show
/// check (`0x53d580`) on the aura's self-cast flag (`0x08`) and caster: the spell's own legs, a
/// caster-only aura cast by the player or its pet, and the aura vision bit (`0x4f72e0`). With no
/// catalog at all every aura stays listed.
fn shown_in_aura_ui(ctx: ListCtx, spell_id: u32, flags: u8, caster: u64) -> bool {
    let Some(catalog) = ctx.catalog else {
        return true;
    };
    let Some(d) = catalog.get(spell_id) else {
        return false;
    };
    let viewer = ctx.viewer;
    d.shows_aura(flags & AURA_UPDATE_FLAG_SELF_CAST != 0)
        && (!d.only_visible_to_caster()
            || (caster != 0 && (caster == viewer.player || caster == viewer.pet)))
        && (d.required_aura_vision == 0
            || u32::from(viewer.vision) & 1u32.wrapping_shl(d.required_aura_vision - 1) != 0)
}

/// What a walk reads off the book: the player's known spells and the dispel types the book update
/// registered, and its pet's guid and spells.
pub(crate) struct AuraBook<'a> {
    pub(crate) known: Option<&'a std::collections::BTreeSet<u32>>,
    pub(crate) dispel_types: &'a [i32],
    pub(crate) steal_mask: u32,
    pub(crate) pet: u64,
    pub(crate) pet_spells: &'a [benilla_protocol::messages::PetActionEntry],
}

/// The live view's aura reads: the units' tables and the roster records, and what the display
/// filter and the unit gate read.
#[derive(SystemParam)]
pub(crate) struct AuraLive<'w, 's> {
    index: Option<Res<'w, GuidIndex>>,
    stores: Query<'w, 's, &'static ObjectStore>,
    tables: Query<'w, 's, &'static UnitAuras>,
    me: Option<Res<'w, crate::net::SelfGuid>>,
    spells: Option<Res<'w, Spells>>,
    // The gate's reaction, the same the selection ring and nameplates read.
    factions: Option<Res<'w, Factions>>,
    reputations: Option<Res<'w, Reputations>>,
    group: Option<Res<'w, crate::ui_party::GroupState>>,
    names: Option<Res<'w, crate::names::NameCache>>,
    time: Option<Res<'w, Time<Real>>>,
}

impl AuraLive<'_, '_> {
    /// A held unit, the lookup with the unit type mask (`0x6147e1` -> `0x4d4db0`).
    fn held(&self, guid: u64) -> Option<(Entity, &ObjectStore)> {
        let &entity = self.index.as_ref()?.0.get(&guid)?;
        let store = self.stores.get(entity).ok()?;
        store.is_unit().then_some((entity, store))
    }

    /// `GetTime()` minus the real clock the tables stamp expiries on.
    fn clock_offset(&self, ui_now: f64) -> f64 {
        ui_now - self.time.as_ref().map_or(0.0, |t| t.elapsed_secs_f64())
    }

    /// `UnitAura`'s walk (`0x6147c0`): a held unit's listed auras behind its unit gate, else the
    /// roster record's block; the first aura `pick` takes is built.
    pub(crate) fn find(
        &self,
        guid: u64,
        ui_now: f64,
        raid: bool,
        book: AuraBook,
        pick: &mut dyn FnMut(&AuraFacts<'_>) -> bool,
    ) -> Option<AuraState> {
        let spells = self.spells.as_deref();
        let catalog = spells.map(|s| &s.catalog);
        let me = self.me.as_ref().and_then(|m| m.0).unwrap_or(0);
        let my_store = self.held(me).map(|(_, s)| s);
        let empty = std::collections::BTreeSet::new();
        // No reputation store reads as one with no standings, as a fresh session's.
        let none = Reputations::default();
        let reputations = self.reputations.as_deref().unwrap_or(&none);
        let party = crate::target::party_of(self.group.as_deref());
        let held = self.held(guid);
        // The steal test asks for a held unit only, with a mask registered and the unit one we can
        // attack (`0x53d683`, `0x53d6b5`).
        let steal_mask = match held {
            Some((_, store))
                if book.steal_mask != 0
                    && crate::target::can_attack(
                        Some(store),
                        self.factions.as_deref(),
                        reputations,
                        party,
                        my_store,
                    ) =>
            {
                book.steal_mask
            }
            _ => 0,
        };
        let ctx = ListCtx {
            catalog,
            durations: spells.map(|s| &s.durations),
            viewer: Viewer {
                player: me,
                pet: book.pet,
                vision: my_store.and_then(|s| s.0.player_aura_vision()).unwrap_or(0),
            },
            clock_offset: self.clock_offset(ui_now),
            raid: raid.then_some(RaidBook {
                known: book.known.unwrap_or(&empty),
                pet: book.pet_spells,
                dispel_types: book.dispel_types,
            }),
            steal_mask,
        };
        let mut take = |a: &AuraInput| pick(&aura_facts(ctx, a));
        let found = match held {
            Some((entity, store)) => {
                // A unit no aura packet has reached lists nothing.
                let table = self.tables.get(entity).ok()?;
                let friendly = |unit: &ObjectStore| {
                    is_friendly(
                        unit,
                        self.factions.as_deref(),
                        reputations,
                        party,
                        my_store,
                        self.names.as_deref(),
                    )
                    .then_some(())
                };
                // The gate asks only with an active player (`0x6147fc`).
                if my_store.is_some()
                    && auras_hidden_on(store, table, catalog, |u| friendly(u).is_some())
                {
                    return None;
                }
                table_inputs(table, ctx).find(|a| take(a))
            }
            None => {
                let group = self.group.as_deref()?;
                record_inputs(group.roster_auras(guid), ctx).find(|a| take(a))
            }
        };
        found.map(|a| aura_state(ctx, &a))
    }

    /// `guid`'s raw slots: a held unit's every slot (`0x7282a0`), else its roster record; `None`
    /// for nobody.
    pub(crate) fn slots(&self, guid: u64, ui_now: f64) -> Option<UnitAuraSlots> {
        if let Some((entity, _)) = self.held(guid) {
            let offset = self.clock_offset(ui_now);
            let slots = self
                .tables
                .get(entity)
                .map(|t| {
                    t.slot_spells()
                        .into_iter()
                        .map(|(spell, end)| (spell, if end > 0.0 { end + offset } else { 0.0 }))
                        .collect()
                })
                .unwrap_or_default();
            return Some(UnitAuraSlots::Held(slots));
        }
        self.group.as_deref()?.roster_aura_slots(guid)
    }
}

/// The aura vision bits a spell's aura answers to (`0x7fe3e0`): its required vision's bit, a
/// 32-bit shift as the display filter's (`0x7fe3f8`), `0x20` for a stealth-detect effect (aura
/// 17), `0x40` for an invisibility-detect one (aura 19) of misc value 0 or 10.
fn vision_mask(d: &SpellDisplay) -> u32 {
    let mut mask = match d.required_aura_vision {
        0 => 0,
        v => 1u32.wrapping_shl(v - 1),
    };
    for e in 0..3 {
        match d.effect_apply_aura[e] {
            17 => mask |= 0x20,
            19 if matches!(d.effect_misc_value[e], 0 | 10) => mask |= 0x40,
            _ => {}
        }
    }
    mask
}

/// Our aura vision moved by `changed` (`0x6da770`): each held unit holding an aura whose vision
/// bits moved queues `UNIT_AURA` for the frame's flush (`0x727a70`, `0x727bed`).
pub(crate) fn aura_vision_moved(world: &mut World, changed: u8) {
    let mut units = world.query::<(&ObjectStore, &UnitAuras)>();
    let Some(spells) = world.get_resource::<Spells>() else {
        return;
    };
    let catalog = &spells.catalog;
    let moved: Vec<u64> = units
        .iter(world)
        .filter(|(store, table)| {
            store.is_unit()
                && table.slot_spells().iter().any(|&(spell, _)| {
                    catalog
                        .get(spell)
                        .is_some_and(|d| vision_mask(d) & u32::from(changed) != 0)
                })
        })
        .filter_map(|(store, _)| store.0.object_guid())
        .collect();
    for unit in moved {
        crate::ui_script::live::queue_unit_signal(world, unit, "UNIT_AURA");
    }
}

/// The session's end zeroes the tracking spell, as the next session's start does (`0x58383f`), so
/// nothing reaches the next character's minimap.
fn end_session_tracking(tracking: Option<ResMut<crate::unit_auras::TrackingSpell>>) {
    if let Some(mut tracking) = tracking {
        tracking.0 = 0;
    }
}

/// A `CMSG_CANCEL_AURA` per queued `CancelUnitBuff` or `SetTracking`, naming the spell, not the
/// slot.
pub(crate) fn drain_aura_cancels(mut script: crate::ui_script::InTurn, net: Res<NetCommands>) {
    let Some(mut script) = script.turn() else {
        return;
    };
    for spell_id in script.take_cancel_aura_requests() {
        // The aura cancel's form drop at its step, its send after the drop's listeners (`0x80324d`, `0x803458`).
        match script.queue() {
            Some(queue) => {
                queue.world(move |w| crate::ui_shapeshift::cancel_aura_step(w, spell_id))
            }
            None => {
                let _ = net.0.send(ClientCommand::CancelAura { spell_id });
            }
        }
    }
    for slot in script.take_temp_enchant_cancels() {
        let cmd = ClientCommand::CancelTempEnchantment { slot };
        // Behind the aura cancels queued before it.
        match script.queue() {
            Some(queue) => queue.world(move |w| crate::ui_shapeshift::send_after_raises(w, cmd)),
            None => {
                let _ = net.0.send(cmd);
            }
        }
    }
}

#[cfg(test)]
mod live_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{AURA_UPDATE_FLAG_CANCELABLE, AURA_UPDATE_FLAG_SELF_CAST};

    const NO_SPELLS: ListCtx = ListCtx {
        catalog: None,
        durations: None,
        viewer: Viewer {
            player: 0,
            pet: 0,
            vision: 0,
        },
        clock_offset: 0.0,
        raid: None,
        steal_mask: 0,
    };

    /// The auras `inputs` build, in order.
    fn aura_list(ctx: ListCtx, inputs: &[AuraInput]) -> Vec<AuraState> {
        inputs.iter().map(|a| aura_state(ctx, a)).collect()
    }

    /// [`table_inputs`] as the list.
    fn other_unit_auras(table: &UnitAuras) -> Vec<AuraState> {
        let inputs: Vec<AuraInput> = table_inputs(table, NO_SPELLS).collect();
        aura_list(NO_SPELLS, &inputs)
    }

    /// Off the real `Spell.dbc`: a raid buff carries `AttributesEx7 & 0x10000000` and lists for
    /// consolidation; the `RAID` list holds a buff the book knows (`0x53d8a0`) and a debuff whose
    /// dispel type the book's Dispel Magic names (`0x542170`, Magic), and not a poison.
    #[test]
    fn raid_buffs_consolidate_and_the_raid_list_follows_the_book() {
        let Some(spells) = crate::ui_action::real_spells() else {
            return;
        };
        const INTELLECT: u32 = 42995;
        const FORTITUDE: u32 = 48161;
        const DISPEL_MAGIC: u32 = 527;
        const FROSTBOLT: u32 = 42842;
        const DEADLY_POISON: u32 = 57970;
        let known = [INTELLECT, DISPEL_MAGIC].into_iter().collect();
        let types = dispel_types(&known, &spells.catalog);
        assert_eq!(types, [1], "Dispel Magic names Magic");
        let ctx = ListCtx {
            catalog: Some(&spells.catalog),
            durations: Some(&spells.durations),
            raid: Some(RaidBook {
                known: &known,
                pet: &[],
                dispel_types: &types,
            }),
            ..NO_SPELLS
        };
        let state = |spell_id, harmful: bool| {
            aura_state(
                ctx,
                &AuraInput {
                    spell_id,
                    stacks: 1,
                    flags: if harmful { AURA_UPDATE_FLAG_HARMFUL } else { 0 },
                    caster: 0,
                    duration_ms: 0,
                    expires_at: 0.0,
                },
            )
        };
        let intellect = state(INTELLECT, false);
        assert!(intellect.consolidate && intellect.raid);
        let fortitude = state(FORTITUDE, false);
        assert!(fortitude.consolidate, "a raid buff consolidates");
        assert!(!fortitude.raid, "but only a known one lists under RAID");
        assert!(
            state(FROSTBOLT, true).raid,
            "a Magic debuff the book dispels"
        );
        assert!(!state(DEADLY_POISON, true).raid, "a poison it cannot");
        assert!(!state(FROSTBOLT, true).consolidate);
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
        let inputs: Vec<AuraInput> = record_inputs(block.into_iter(), NO_SPELLS).collect();
        let read: Vec<(bool, bool)> = aura_list(NO_SPELLS, &inputs)
            .iter()
            .map(|a| (a.helpful, a.cancelable))
            .collect();
        assert_eq!(
            read,
            [(true, true), (false, false)],
            "the walk ends at the first empty slot"
        );
    }

    /// A server-only spell (AzerothCore's Defensive State 5302, no `Spell.dbc` row) ahead of a
    /// known one takes no index (`0x72c73d`): Battle Shout lists first, by name, on the player's
    /// list and a roster record alike.
    #[test]
    fn an_aura_without_a_spell_row_takes_no_index() {
        let catalog = SpellCatalog::from_displays(std::collections::HashMap::from([(
            6673,
            benilla_formats::SpellDisplay {
                id: 6673,
                name: "Battle Shout".into(),
                ..Default::default()
            },
        )]));
        let table = UnitAuras::of(7, &[(1, 5302, 0x39), (2, 6673, 0x3b)]);
        let ctx = ListCtx {
            catalog: Some(&catalog),
            ..NO_SPELLS
        };
        let inputs: Vec<AuraInput> = table_inputs(&table, ctx).collect();
        let listed: Vec<(u32, Option<String>)> = aura_list(ctx, &inputs)
            .into_iter()
            .map(|a| (a.spell_id, a.name))
            .collect();
        assert_eq!(listed, [(6673, Some("Battle Shout".into()))]);

        let block = [
            RecordAura {
                slot: 0,
                spell_id: 5302,
                flags: 0x19,
            },
            RecordAura {
                slot: 1,
                spell_id: 6673,
                flags: 0x19,
            },
        ];
        let roster: Vec<u32> = record_inputs(block.into_iter(), ctx)
            .map(|a| a.spell_id)
            .collect();
        assert_eq!(roster, [6673]);

        // No catalog at all: nothing is known to be missing, so both stay.
        assert_eq!(table_inputs(&table, NO_SPELLS).count(), 2);
    }

    /// The aura-show check's viewer legs (`0x53d580`): a caster-only aura lists only when the
    /// player or its pet cast it, and never from a roster record; an aura-vision aura only while
    /// the player's vision byte carries its bit; the self-cast flag picks the `AttributesEx5` bit.
    #[test]
    fn the_aura_filter_reads_the_caster_the_vision_and_the_self_cast_flag() {
        use benilla_formats::SpellDisplay;
        const ME: u64 = 0x10;
        const PET: u64 = 0xF140_0000_0000_0001;
        let spell = |id, d: SpellDisplay| (id, SpellDisplay { id, ..d });
        let catalog = SpellCatalog::from_displays(std::collections::HashMap::from([
            spell(
                1,
                SpellDisplay {
                    attributes_ex6: 0x10_0000,
                    ..Default::default()
                },
            ),
            spell(
                2,
                SpellDisplay {
                    required_aura_vision: 3,
                    ..Default::default()
                },
            ),
            spell(
                3,
                SpellDisplay {
                    attributes_ex5: 0x0800_0000,
                    ..Default::default()
                },
            ),
        ]));
        let ctx = ListCtx {
            catalog: Some(&catalog),
            viewer: Viewer {
                player: ME,
                pet: PET,
                vision: 0,
            },
            ..NO_SPELLS
        };
        let shown = |ctx, id, flags, caster| shown_in_aura_ui(ctx, id, flags, caster);
        assert!(shown(ctx, 1, 0, ME) && shown(ctx, 1, 0, PET));
        assert!(
            !shown(ctx, 1, 0, 0x99) && !shown(ctx, 1, 0, 0),
            "nobody's, or no caster"
        );
        assert!(!shown(ctx, 2, 0, ME), "no vision");
        let seeing = ListCtx {
            viewer: Viewer {
                vision: 0b100,
                ..ctx.viewer
            },
            ..ctx
        };
        assert!(shown(seeing, 2, 0, ME), "vision 3 is bit 2");
        assert!(!shown(ctx, 3, AURA_UPDATE_FLAG_SELF_CAST, ME));
        assert!(shown(ctx, 3, 0, 0x99), "bit 27 hides only a self-cast aura");
    }

    // == `UnitAura`'s unit-level gate == A stealthed Ridge Stalker carries 5916 helpful and 22766
    // harmful; 12340 has no assist gate (`0x6147c0`), so a hostile creature lists both.

    fn ridge_stalker_table() -> UnitAuras {
        UnitAuras::of(7, &[(0, 5916, 1), (1, 22766, AURA_UPDATE_FLAG_HARMFUL | 1)])
    }

    /// No assist or `UNIT_FLAG_AURAS_VISIBLE` test: a hostile unit's buffs list beside its debuffs.
    #[test]
    fn a_hostile_units_buffs_list_beside_its_debuffs() {
        let table = ridge_stalker_table();
        let unit = ObjectStore::default();
        assert!(
            !auras_hidden_on(&unit, &table, None, |_| false),
            "no aura-type-295 effect: the gate stays open, friendly or not"
        );
        assert_eq!(
            other_unit_auras(&table)
                .iter()
                .map(|a| (a.spell_id, a.helpful))
                .collect::<Vec<_>>(),
            [(22766, false), (5916, true)],
            "both halves enumerate, in the table's display order"
        );
    }

    /// An applied aura-type-295 effect hides every aura of a unit not friendly to the player, and
    /// none of a friendly one's; an effect the aura's flags do not apply sets no mask bit.
    #[test]
    fn an_applied_aura_type_295_effect_hides_an_unfriendly_units_auras() {
        const HIDER: u32 = 900_001;
        let catalog = SpellCatalog::from_displays(
            [(
                HIDER,
                benilla_formats::SpellDisplay {
                    effect_apply_aura: [0, AURA_TYPE_HIDES_AURAS, 0],
                    ..Default::default()
                },
            )]
            .into(),
        );
        let unit = ObjectStore::default();
        // Effect 1 applied: flags bit `1 << 1`.
        let applied = UnitAuras::of(7, &[(0, 5916, 1), (1, HIDER, 0x2)]);
        assert!(auras_hidden_on(&unit, &applied, Some(&catalog), |_| false));
        assert!(
            !auras_hidden_on(&unit, &applied, Some(&catalog), |_| true),
            "a friendly unit keeps its list"
        );
        // Only effect 0 applied, which names no aura.
        let unapplied = UnitAuras::of(7, &[(0, 5916, 1), (1, HIDER, 0x1)]);
        assert!(!auras_hidden_on(&unit, &unapplied, Some(&catalog), |_| {
            false
        }));
    }

    /// Spellsteal registers Magic's class mask (`0x542222`), and the steal test (`0x53d680`) passes
    /// a Magic buff with that mask, never a debuff, nothing with no mask, nor a class the mask lacks.
    #[test]
    fn a_magic_buff_is_stealable_with_the_mask_spellsteal_registers() {
        let Some(spells) = crate::ui_action::real_spells() else {
            return;
        };
        const SPELLSTEAL: u32 = 30449;
        const ARCANE_INTELLECT: u32 = 42995;
        let catalog = &spells.catalog;
        let magic = catalog.dispel_mask(1);
        let known = [SPELLSTEAL].into_iter().collect();
        assert_eq!(steal_mask(&known, catalog), Some(magic));
        assert_eq!(steal_mask(&Default::default(), catalog), None);
        let stealable = |mask: u32, flags: u8| {
            aura_state(
                ListCtx {
                    catalog: Some(catalog),
                    steal_mask: mask,
                    ..NO_SPELLS
                },
                &AuraInput {
                    spell_id: ARCANE_INTELLECT,
                    stacks: 1,
                    flags,
                    caster: 0,
                    duration_ms: 0,
                    expires_at: 0.0,
                },
            )
            .stealable
        };
        assert!(stealable(magic, 0x1), "a Magic buff");
        assert!(
            !stealable(magic, 0x1 | AURA_UPDATE_FLAG_HARMFUL),
            "a debuff"
        );
        assert!(
            !stealable(0, 0x1),
            "no mask: no steal spell, or a unit we cannot attack"
        );
        assert!(!stealable(catalog.dispel_mask(2), 0x1), "a Curse mask");
    }

    /// The vision bits an aura answers to (`0x7fe3e0`): the required vision's bit, then the
    /// stealth-detect effect's `0x20` and an invisibility-detect effect's `0x40` for misc 0 or 10.
    #[test]
    fn the_vision_mask_reads_the_required_vision_and_the_detect_effects() {
        use benilla_formats::SpellDisplay;
        let spell = |vision, auras: [u32; 3], misc: [i32; 3]| SpellDisplay {
            required_aura_vision: vision,
            effect_apply_aura: auras,
            effect_misc_value: misc,
            ..Default::default()
        };
        assert_eq!(vision_mask(&spell(0, [0; 3], [0; 3])), 0);
        assert_eq!(vision_mask(&spell(3, [0; 3], [0; 3])), 0b100);
        assert_eq!(vision_mask(&spell(0, [0, 17, 0], [0; 3])), 0x20);
        assert_eq!(vision_mask(&spell(0, [0, 0, 19], [0, 0, 10])), 0x40);
        assert_eq!(
            vision_mask(&spell(1, [19, 0, 0], [3, 0, 0])),
            0b1,
            "misc 3 is no detect"
        );
    }

    /// The two clauses.
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
        // No row: permanent.
        assert!(until_cancelled(Some(&ordinary), None, true, 0.0));

        // Clause 2: the area-aura override applies only to a non-cancelable aura.
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
