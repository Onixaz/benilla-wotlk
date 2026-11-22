//! A unit's aura array as 3.3.5a's `SMSG_AURA_UPDATE` feed fills it (`unit+0xc50`, `0x72f5d0`).
//! 3.3.5a has no aura descriptor fields, so this component is the only aura state a unit has:
//! every aura reader, the state-kit watcher, the UI feed and the cast checks, reads it, and its
//! change ticks are the aura edge.

use std::collections::BTreeMap;

use benilla_protocol::messages::{AuraRecord, AuraUpdate};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use crate::net::{GuidIndex, NetHandlerApp, ObjectStore};

/// One live aura of a unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct UnitAura {
    pub(crate) slot: u8,
    pub(crate) spell_id: u32,
    /// The wire flags ([`benilla_protocol::messages::AURA_UPDATE_FLAG_HARMFUL`] and kin).
    pub(crate) flags: u8,
    pub(crate) level: u8,
    /// Raw stack count or charges.
    pub(crate) stacks: u8,
    /// The caster's guid; a self-cast aura names the unit itself (`0x716558`).
    pub(crate) caster: u64,
    /// The full duration in ms, 0 untimed.
    pub(crate) duration_ms: u32,
    /// The real-clock second it runs out, 0 untimed: arrival plus the remaining time
    /// (`0x7165a4`).
    pub(crate) expires_at: f64,
}

impl UnitAura {
    fn from_record(slot: u8, unit: u64, r: &AuraRecord, now: f64) -> Self {
        let (duration_ms, expires_at) = match r.duration_ms {
            Some((max, remaining)) => (max, now + f64::from(remaining) / 1000.0),
            None => (0, 0.0),
        };
        Self {
            slot,
            spell_id: r.spell_id,
            flags: r.flags,
            level: r.level,
            stacks: r.stacks,
            caster: r.caster.unwrap_or(unit),
            duration_ms,
            expires_at,
        }
    }
    pub(crate) fn is_harmful(&self) -> bool {
        self.flags & benilla_protocol::messages::AURA_UPDATE_FLAG_HARMFUL != 0
    }
    pub(crate) fn is_cancelable(&self) -> bool {
        self.flags & benilla_protocol::messages::AURA_UPDATE_FLAG_CANCELABLE != 0
    }
}

/// One aura change the combat log reads (`0x727760`, `0x722090`, `0x752860`, `0x752ba0`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum AuraEdge {
    Added(UnitAura),
    Removed(UnitAura),
    /// The same aura with another stack count: the old count and the aura now.
    Dose {
        old: u8,
        aura: UnitAura,
    },
    /// The same aura and count with a new expiry.
    Refreshed(UnitAura),
}

/// A unit's live auras by slot, and the order the aura UI lists them in. A record without an
/// effect bit is an empty slot to every reader the client has (`0x72f7a5`, `0x72c5e6`), so it is
/// not kept.
#[derive(Component, Debug, Default, Clone, PartialEq)]
#[component(storage = "SparseSet")]
pub(crate) struct UnitAuras {
    by_slot: BTreeMap<u8, UnitAura>,
    /// `(slot, spell)` in display order, the index the aura finder walks (`unit+0xdd4`, rebuilt
    /// by `0x72c510`): a survivor keeps its place, a new aura appends, a batch of new ones in
    /// descending slot order (the rebuild's slot walk runs down, `0x72c6e9`).
    order: Vec<(u8, u32)>,
    /// Packets folded in: each signals `UNIT_AURA` for its unit, whether or not a slot moved
    /// (`0x72f5d0`, deduplicated per unit by the queue `0x6143f0`).
    packets: u64,
    /// Slots holding a spell with no effect bit: no reader sees them but the spell-id walk.
    inactive: BTreeMap<u8, u32>,
}

impl UnitAuras {
    /// Packets folded in so far, which a `UNIT_AURA` feed compares.
    pub(crate) fn packets(&self) -> u64 {
        self.packets
    }

    /// Folds one packet in: `ALL` empties every slot first (`0x72f659`-`0x72f69f`), then each
    /// named slot takes its record or clears; then the display order is rebuilt. The reference
    /// rebuilds lazily, before the next finder walk (`0x72c9de`), so two packets read by no walk
    /// between them order as one there and as two here.
    pub(crate) fn apply(&mut self, update: &AuraUpdate, now: f64) {
        self.packets = self.packets.wrapping_add(1);
        if update.all {
            self.by_slot.clear();
            self.inactive.clear();
        }
        for (slot, record) in &update.slots {
            self.inactive.remove(slot);
            match record.filter(AuraRecord::is_live) {
                Some(r) => {
                    self.by_slot
                        .insert(*slot, UnitAura::from_record(*slot, update.unit, &r, now));
                }
                None => {
                    self.by_slot.remove(slot);
                    if let Some(r) = record.filter(|r| r.spell_id != 0) {
                        self.inactive.insert(*slot, r.spell_id);
                    }
                }
            }
        }
        self.reorder();
    }
    /// What one packet would change, as `0x72f5d0` diffs it for the log: an `ALL` packet nothing,
    /// else every removal first, then each slot's arrival, stack change or new expiry.
    pub(crate) fn edges(&self, update: &AuraUpdate, now: f64) -> Vec<AuraEdge> {
        if update.all {
            return Vec::new();
        }
        let mut after = self.clone();
        after.apply(update, now);
        let slots: Vec<u8> = update.slots.iter().map(|(slot, _)| *slot).collect();
        let pair = |slot: &u8| (self.by_slot.get(slot), after.by_slot.get(slot));
        let mut out: Vec<AuraEdge> = slots
            .iter()
            .filter_map(|slot| match pair(slot) {
                (Some(old), new) if new.is_none_or(|n| n.spell_id != old.spell_id) => {
                    Some(AuraEdge::Removed(*old))
                }
                _ => None,
            })
            .collect();
        out.extend(slots.iter().filter_map(|slot| match pair(slot) {
            (None, Some(new)) => Some(AuraEdge::Added(*new)),
            (Some(old), Some(new)) if old.spell_id != new.spell_id => Some(AuraEdge::Added(*new)),
            (Some(old), Some(new)) if old.stacks != new.stacks => Some(AuraEdge::Dose {
                old: old.stacks,
                aura: *new,
            }),
            (Some(old), Some(new)) if old.expires_at != new.expires_at => {
                Some(AuraEdge::Refreshed(*new))
            }
            _ => None,
        }));
        out
    }

    /// `0x72c510`'s two passes: drop each entry whose slot no longer holds its spell, closing the
    /// gap (`0x72c535`-`0x72c695`), then append each live slot not listed, highest slot first.
    fn reorder(&mut self) {
        let by_slot = &self.by_slot;
        self.order
            .retain(|&(slot, spell)| by_slot.get(&slot).is_some_and(|a| a.spell_id == spell));
        for a in self.by_slot.values().rev() {
            if !self.order.contains(&(a.slot, a.spell_id)) {
                self.order.push((a.slot, a.spell_id));
            }
        }
    }
    /// Any slot holds `spell_id`, an effect applied or not (`0x7282a0`).
    pub(crate) fn holds_spell(&self, spell_id: u32) -> bool {
        self.by_slot.values().any(|a| a.spell_id == spell_id)
            || self.inactive.values().any(|&s| s == spell_id)
    }

    /// A slot holds `spell_id` with an effect bit set (`0x728300`).
    pub(crate) fn has_applied(&self, spell_id: u32) -> bool {
        use benilla_protocol::messages::AURA_UPDATE_EFF_INDEX_MASK;
        self.by_slot
            .values()
            .any(|a| a.spell_id == spell_id && a.flags & AURA_UPDATE_EFF_INDEX_MASK != 0)
    }

    /// Every slot's spell in slot order, an effect applied or not, as the spell-id walks read them
    /// (`0x7282a0`, `0x7289c0`), each with its real-clock expiry, 0 untimed or unknown.
    pub(crate) fn slot_spells(&self) -> Vec<(u32, f64)> {
        let mut slots: Vec<(u8, u32, f64)> = self
            .by_slot
            .values()
            .map(|a| (a.slot, a.spell_id, a.expires_at))
            .chain(
                self.inactive
                    .iter()
                    .map(|(&slot, &spell)| (slot, spell, 0.0)),
            )
            .collect();
        slots.sort_by_key(|&(slot, ..)| slot);
        slots
            .into_iter()
            .map(|(_, spell, end)| (spell, end))
            .collect()
    }

    /// Live auras by ascending slot.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &UnitAura> + '_ {
        self.by_slot.values()
    }
    /// Live auras in display order, the order `UnitAura`'s index counts (`0x72c9b0`).
    pub(crate) fn listed(&self) -> impl Iterator<Item = &UnitAura> + '_ {
        self.order
            .iter()
            .filter_map(|(slot, _)| self.by_slot.get(slot))
    }
    /// A table of untimed `(slot, spell, flags)` auras cast by `caster`, applied in one batch, for
    /// tests.
    #[cfg(test)]
    pub(crate) fn of(caster: u64, auras: &[(u8, u32, u8)]) -> Self {
        let rows: Vec<_> = auras.iter().map(|&(s, id, f)| (s, id, f, caster)).collect();
        Self::of_rows(&rows)
    }
    /// A table of untimed `(slot, spell, caster)` auras, flags 1, applied in one batch, for tests.
    #[cfg(test)]
    pub(crate) fn of_casters(auras: &[(u8, u32, u64)]) -> Self {
        let rows: Vec<_> = auras.iter().map(|&(s, id, c)| (s, id, 1, c)).collect();
        Self::of_rows(&rows)
    }
    #[cfg(test)]
    fn of_rows(auras: &[(u8, u32, u8, u64)]) -> Self {
        let mut t = Self::default();
        for &(slot, spell_id, flags, caster) in auras {
            let a = UnitAura {
                slot,
                spell_id,
                flags,
                level: 1,
                stacks: 1,
                caster,
                duration_ms: 0,
                expires_at: 0.0,
            };
            t.by_slot.insert(slot, a);
        }
        t.reorder();
        t
    }
}

/// A streamed unit holding live `spells`, one per slot from 0, for tests.
#[cfg(test)]
pub(crate) fn aura_unit_for_test(spells: &[u32]) -> (ObjectStore, UnitAuras) {
    let store = benilla_protocol::ObjectFields::default()
        .into_created(benilla_protocol::messages::ObjectType::Unit);
    (ObjectStore(store), table_for_test(spells))
}

/// Replaces `unit`'s table with live `spells`, as an aura packet would, for tests.
#[cfg(test)]
pub(crate) fn set_auras_for_test(world: &mut World, unit: Entity, spells: &[u32]) {
    world.entity_mut(unit).insert(table_for_test(spells));
}

#[cfg(test)]
fn table_for_test(spells: &[u32]) -> UnitAuras {
    let slots: Vec<(u8, u32, u8)> = (0u8..).zip(spells).map(|(i, &s)| (i, s, 1)).collect();
    UnitAuras::of(0, &slots)
}

/// Registers the feed's handler and the minimap's tracking spell.
pub(crate) struct UnitAurasPlugin;

impl Plugin for UnitAurasPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TrackingSpell>()
            .net_handler(SessionEventKind::AuraUpdate, on_aura_update);
    }
}

/// The minimap's tracking spell (`0xbeba68`), 0 for none: a session global our tracking aura's
/// add and removal write whether or not a UI stands, which a reload keeps; zeroed at the session's
/// start (`0x58383f`) and `SetTracking()` (`0x57f4c2`).
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TrackingSpell(pub(crate) u32);

/// What our own auras' effect watch reads: our guid, the catalog, the stance bar, the book's
/// tracking spells, the tracking global and the queue it fires into.
type OwnAuraWatch<'w> = (
    Option<Res<'w, crate::net::SelfGuid>>,
    Option<Res<'w, crate::ui_action::Spells>>,
    Option<Res<'w, crate::ui_action::usable_cache::UsableCaches>>,
    Option<Res<'w, crate::ui_spellbook::SpellBook>>,
    Option<ResMut<'w, TrackingSpell>>,
    Option<ResMut<'w, crate::ui_script::live::UiEvents>>,
);

/// A packet for a unit not in the object manager is skipped whole (`0x7300f1`); a unit's first
/// packet gives it the table.
fn on_aura_update(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    mut units: Query<(&ObjectStore, Option<&mut UnitAuras>)>,
    mut commands: Commands,
    clock: Res<Time<Real>>,
    own: OwnAuraWatch<'_>,
) {
    let SessionEvent::AuraUpdate(update) = ev else {
        return;
    };
    let now = clock.elapsed_secs_f64();
    debug!(
        "aura: {} {:#x} @ {now:.2}: {:?}",
        if update.all { "ALL" } else { "UPDATE" },
        update.unit,
        update
            .slots
            .iter()
            .map(|(slot, r)| (*slot, r.map(|r| (r.spell_id, r.flags, r.duration_ms))))
            .collect::<Vec<_>>()
    );
    let Some(&entity) = index.0.get(&update.unit) else {
        return;
    };
    let Ok((store, auras)) = units.get_mut(entity) else {
        return;
    };
    if !store.is_unit() {
        return; // the lookup is unit-typed (`0x4d4db0`, mask 8)
    }
    let (me, spells, caches, book, mut tracking, mut ui) = own;
    let mine = me.is_some_and(|m| m.0 == Some(update.unit));
    // Whether the packet adds or removes an aura, the reader's `changed` (`0x72fb6e`).
    let all_moves = own_aura_moves(auras.as_deref(), &update);
    let changed = !all_moves.is_empty();
    let moves = mine.then_some(all_moves);
    let dump = mine && std::env::var_os("BENILLA_AURA_DUMP").is_some();
    let catalog = spells.as_deref().map(|s| &s.catalog);
    match auras {
        Some(mut auras) => {
            auras.apply(&update, now);
            if dump {
                dump_auras(&auras, catalog);
            }
        }
        None => {
            let mut auras = UnitAuras::default();
            auras.apply(&update, now);
            if dump {
                dump_auras(&auras, catalog);
            }
            commands.entity(entity).insert(auras);
        }
    }
    // Our own effects that move usability, then the reader's usable update (`0x724a20`,
    // `0x71ea1e`, `0x72fbfb`), then its not-interruptible re-read for a watched caster
    // (`0x72fd30`), after the apply.
    let unit = update.unit;
    // Each aura of ours that came or went: its effect's usable update, then the removal's
    // (`0x722090`) or the add's (`0x727760`) bar update, tracking and form; an `ALL` packet runs
    // the removal for each aura it drops and the bar update and tracking for every live one
    // (`0x72f9a2`, `0x72f9b5`).
    let live: Vec<u32> = if update.all && mine {
        update
            .slots
            .iter()
            .filter_map(|(_, r)| r.map(|r| r.spell_id))
            .collect()
    } else {
        Vec::new()
    };
    // A known tracking spell (`0x53bd10`), off the book's list.
    let is_tracking = |spell: u32| {
        book.as_ref()
            .is_some_and(|b| b.live.tracking.iter().any(|s| s.spell_id == spell))
    };
    for &(spell, removed) in moves.iter().flatten() {
        let moves_usability = spells
            .as_ref()
            .and_then(|s| s.catalog.get(spell))
            .is_some_and(crate::ui_action::usable_cache::moves_usability);
        if let (true, Some(ui)) = (moves_usability, ui.as_mut()) {
            ui.world(crate::ui_action::usable_cache::step(
                crate::ui_action::usable_cache::Pass::Full,
            ));
        }
        if update.all && !removed {
            continue;
        }
        if let Some(ui) = ui.as_mut() {
            ui.world(move |world| crate::ui_action::slots::aura_changed(world, spell));
        }
        // A tracking aura's removal clears the spell (`0x722113`), one added that is not already
        // the spell sets it (`0x727804`).
        if let (true, Some(t)) = (is_tracking(spell), tracking.as_mut()) {
            let to = if removed {
                Some(0)
            } else {
                (t.0 != spell).then_some(spell)
            };
            if let Some(to) = to {
                t.0 = to;
                if let Some(ui) = ui.as_mut() {
                    ui.write(move |script| script.tracking_spell_moved(to));
                }
            }
        }
        // A stance-bar spell with no shapeshift effect (aura 36): its add or removal moves no
        // form byte, so it signals the form itself (`0x727835`, `0x722144`).
        let on_stance_bar = caches
            .as_ref()
            .is_some_and(|c| c.stances.iter().any(|s| s.spell == spell));
        let shapeshifts = spells
            .as_ref()
            .and_then(|s| s.catalog.get(spell))
            .is_some_and(|d| d.effect_apply_aura.contains(&SPELL_AURA_MOD_SHAPESHIFT));
        if let (true, Some(ui)) = (on_stance_bar && !shapeshifts, ui.as_mut()) {
            ui.fire("UPDATE_SHAPESHIFT_FORM", Vec::new());
        }
    }
    for spell in live {
        if let Some(ui) = ui.as_mut() {
            ui.world(move |world| crate::ui_action::slots::aura_changed(world, spell));
        }
        // An `ALL` packet's live tracking aura sets the spell whatever it was (`0x72f9b5`).
        if let (true, Some(t)) = (is_tracking(spell), tracking.as_mut()) {
            t.0 = spell;
            if let Some(ui) = ui.as_mut() {
                ui.write(move |script| script.tracking_spell_moved(spell));
            }
        }
    }
    if let Some(ui) = ui.as_mut() {
        ui.world(move |world| crate::ui_action::usable_cache::aura_read(world, unit, changed));
        ui.world(move |world| crate::ui_cast::auras_changed(world, unit, changed));
    }
    // Our own auras' effects come and go with the packet (`0x71e930`, `0x724820`).
    if let (Some(moves), Some(spells), Some(ui)) = (moves, spells, ui.as_mut()) {
        for (spell_id, removed) in moves {
            crate::ui_companion::mount_updates(spell_id, &spells.catalog, ui);
            // An open-stable effect's removal closes the stable, forced (`0x71ea25`).
            let opens_stable = || {
                spells.catalog.get(spell_id).is_some_and(|rec| {
                    rec.effect_apply_aura
                        .contains(&crate::ui_stable::SPELL_AURA_OPEN_STABLE)
                })
            };
            if removed && opens_stable() {
                ui.world(crate::ui_stable::close_forced);
            }
        }
    }
    // `UNIT_AURA` for the unit, once however many packets reach it this frame, fired at the
    // frame's flush (`0x72fde8` -> `0x6143f0`).
    if let Some(ui) = ui.as_mut() {
        ui.world(move |world| {
            crate::ui_script::live::queue_unit_signal(world, unit, "UNIT_AURA");
        });
    }
}

/// `SPELL_AURA_MOD_SHAPESHIFT`, the effect `0x7fde50` tests for (`0x727822`, `0x722131`).
const SPELL_AURA_MOD_SHAPESHIFT: u32 = 36;

/// `BENILLA_AURA_DUMP`: our table after each of our aura packets, slot by slot.
fn dump_auras(table: &UnitAuras, catalog: Option<&benilla_formats::SpellCatalog>) {
    info!("aura dump: {} aura(s) on the player", table.iter().count());
    for a in table.iter() {
        let name = catalog
            .and_then(|c| c.get(a.spell_id))
            .map_or("<unknown spell>", |d| d.name.as_str());
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

/// One packet's removed then applied spells, flagged removed, read off the table before it applies (`0x72f803`, `0x72f904`, `0x72f9dc`).
fn own_aura_moves(held: Option<&UnitAuras>, update: &AuraUpdate) -> Vec<(u32, bool)> {
    let old = |slot: &u8| held.and_then(|h| h.by_slot.get(slot)).map(|a| a.spell_id);
    let mut new: BTreeMap<u8, Option<u32>> = BTreeMap::new();
    if update.all {
        new.extend(
            held.into_iter()
                .flat_map(|h| h.by_slot.keys())
                .map(|&s| (s, None)),
        );
    }
    for (slot, record) in &update.slots {
        new.insert(
            *slot,
            record.filter(AuraRecord::is_live).map(|r| r.spell_id),
        );
    }
    let moved =
        |(slot, spell): (&u8, &Option<u32>)| (old(slot) != *spell).then_some((old(slot), *spell));
    let removed = new.iter().filter_map(moved).filter_map(|(o, _)| o);
    let added: Vec<u32> = if update.all {
        new.values().flatten().copied().collect()
    } else {
        new.iter()
            .filter_map(moved)
            .filter_map(|(_, n)| n)
            .collect()
    };
    removed
        .map(|s| (s, true))
        .chain(added.into_iter().map(|s| (s, false)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{
        AURA_UPDATE_FLAG_DURATION, AURA_UPDATE_FLAG_HARMFUL, AURA_UPDATE_FLAG_SELF_CAST,
    };

    const UNIT: u64 = 0x2a;

    fn rec(spell_id: u32, flags: u8, caster: Option<u64>) -> AuraRecord {
        AuraRecord {
            spell_id,
            flags,
            level: 80,
            stacks: 1,
            caster,
            duration_ms: (flags & AURA_UPDATE_FLAG_DURATION != 0).then_some((60_000, 30_000)),
        }
    }

    fn update(all: bool, slots: Vec<(u8, Option<AuraRecord>)>) -> AuraUpdate {
        AuraUpdate {
            unit: UNIT,
            all,
            slots,
        }
    }

    /// The effect walk: a delta removes then adds only the slots that moved; an `ALL` packet adds every live aura it carries, one we already hold too (`0x72f904`), after the removals.
    #[test]
    fn an_all_packet_reapplies_every_aura_it_carries() {
        let mut held = UnitAuras::default();
        held.apply(
            &update(
                false,
                vec![
                    (0, Some(rec(458, 0x01, None))),
                    (1, Some(rec(7, 0x01, None))),
                ],
            ),
            0.0,
        );
        let moves = |all, slots| own_aura_moves(Some(&held), &update(all, slots));
        assert_eq!(moves(false, vec![(0, Some(rec(458, 0x01, None)))]), vec![]);
        let all = vec![
            (0, Some(rec(458, 0x01, None))),
            (2, Some(rec(9, 0x01, None))),
        ];
        assert_eq!(moves(true, all), vec![(7, true), (458, false), (9, false)]);
        assert_eq!(
            moves(false, vec![(1, Some(rec(8, 0x01, None)))]),
            vec![(7, true), (8, false)]
        );
        // A removal record, and a unit's first packet with nothing held.
        assert_eq!(moves(false, vec![(0, None)]), vec![(458, true)]);
        assert_eq!(
            own_aura_moves(None, &update(false, vec![(3, Some(rec(5, 0x01, None)))])),
            vec![(5, false)]
        );
    }

    /// `0x72f5d0`'s log diff: removals first, then arrivals, a stack change, a new expiry; an
    /// `ALL` packet and an unchanged slot log nothing.
    #[test]
    fn edges_follow_the_reference_diff() {
        let timed = AURA_UPDATE_FLAG_DURATION | 1;
        let mut t = UnitAuras::default();
        t.apply(
            &update(
                false,
                vec![
                    (0, Some(rec(100, 1, Some(9)))),
                    (1, Some(rec(200, timed, Some(9)))),
                    (2, Some(rec(300, 1, Some(9)))),
                ],
            ),
            0.0,
        );
        assert!(t.edges(&update(true, vec![]), 1.0).is_empty());
        let stacked = AuraRecord {
            stacks: 3,
            ..rec(300, 1, Some(9))
        };
        let edges = t.edges(
            &update(
                false,
                vec![
                    (0, Some(rec(101, 1, Some(9)))),
                    (1, Some(rec(200, timed, Some(9)))),
                    (2, Some(stacked)),
                    (3, Some(rec(400, AURA_UPDATE_FLAG_HARMFUL | 1, Some(9)))),
                ],
            ),
            5.0,
        );
        let kinds: Vec<(&str, u32)> = edges
            .iter()
            .map(|e| match e {
                AuraEdge::Added(a) => ("added", a.spell_id),
                AuraEdge::Removed(a) => ("removed", a.spell_id),
                AuraEdge::Dose { aura, .. } => ("dose", aura.spell_id),
                AuraEdge::Refreshed(a) => ("refreshed", a.spell_id),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                ("removed", 100),
                ("added", 101),
                ("refreshed", 200),
                ("dose", 300),
                ("added", 400)
            ]
        );
        assert!(matches!(edges[3], AuraEdge::Dose { old: 1, .. }));
    }

    #[test]
    fn a_self_cast_aura_names_the_unit_and_runs_out_from_arrival() {
        let mut t = UnitAuras::default();
        let flags = AURA_UPDATE_FLAG_SELF_CAST | AURA_UPDATE_FLAG_DURATION | 1;
        t.apply(
            &update(false, vec![(3, Some(rec(43039, flags, None)))]),
            10.0,
        );
        let a = t.iter().next().unwrap();
        assert_eq!((a.slot, a.spell_id, a.caster), (3, 43039, UNIT));
        assert_eq!(a.duration_ms, 60_000);
        assert!((a.expires_at - 40.0).abs() < 1e-9);
    }

    #[test]
    fn a_cleared_slot_drops_and_others_stay() {
        let mut t = UnitAuras::default();
        t.apply(
            &update(
                false,
                vec![
                    (0, Some(rec(1, 1, Some(7)))),
                    (1, Some(rec(2, AURA_UPDATE_FLAG_HARMFUL | 1, Some(7)))),
                ],
            ),
            0.0,
        );
        t.apply(&update(false, vec![(0, None)]), 1.0);
        let left: Vec<u32> = t.iter().map(|a| a.spell_id).collect();
        assert_eq!(left, vec![2]);
        assert!(t.iter().next().unwrap().is_harmful());
    }

    #[test]
    fn all_replaces_the_array_and_a_record_without_an_effect_bit_is_empty() {
        let mut t = UnitAuras::default();
        t.apply(&update(false, vec![(5, Some(rec(9, 1, Some(7))))]), 0.0);
        t.apply(
            &update(
                true,
                vec![(2, Some(rec(4, 1, Some(7)))), (6, Some(rec(8, 0, Some(7))))],
            ),
            0.0,
        );
        let left: Vec<(u8, u32)> = t.iter().map(|a| (a.slot, a.spell_id)).collect();
        assert_eq!(left, vec![(2, 4)]);
        assert!(!t.iter().any(|a| a.spell_id == 9 || a.spell_id == 8));
    }

    #[test]
    fn the_display_order_keeps_survivors_and_appends_a_batch_highest_slot_first() {
        let mut t = UnitAuras::default();
        let r = |spell| Some(rec(spell, 1, Some(7)));
        t.apply(&update(false, vec![(1, r(10)), (4, r(40))]), 0.0);
        let listed = |t: &UnitAuras| t.listed().map(|a| a.slot).collect::<Vec<_>>();
        assert_eq!(listed(&t), vec![4, 1]);
        // A new low slot appends after the survivors, never sorted in.
        t.apply(&update(false, vec![(0, r(5))]), 0.0);
        assert_eq!(listed(&t), vec![4, 1, 0]);
        // A dropped aura closes its gap; a recycled slot with a new spell appends fresh.
        t.apply(&update(false, vec![(4, None), (1, r(11))]), 0.0);
        assert_eq!(listed(&t), vec![0, 1]);
        assert_eq!(
            t.listed().map(|a| a.spell_id).collect::<Vec<_>>(),
            vec![5, 11]
        );
    }

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<GuidIndex>()
            .init_resource::<Time<Real>>()
            .add_plugins(UnitAurasPlugin);
        app
    }

    fn unit_store() -> ObjectStore {
        // `OBJECT_FIELD_TYPE` (index 2) with the unit bit.
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
            benilla_protocol::field::FIELD_OBJECT_TYPE,
            0x9,
        )]))
    }

    #[test]
    fn the_handler_gives_a_known_unit_its_table_and_skips_a_stranger() {
        let mut app = app();
        let e = app.world_mut().spawn(unit_store()).id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(UNIT, e);
        let ev = |unit| {
            SessionEvent::AuraUpdate(AuraUpdate {
                unit,
                all: true,
                slots: vec![(0, Some(rec(42917, AURA_UPDATE_FLAG_HARMFUL | 1, Some(7))))],
            })
        };
        crate::net::handlers::dispatch(app.world_mut(), vec![ev(UNIT), ev(0x99)]);
        let t = app.world().get::<UnitAuras>(e).expect("the table");
        assert!(t.iter().any(|a| a.spell_id == 42917));
        assert_eq!(
            app.world_mut()
                .query::<&UnitAuras>()
                .iter(app.world())
                .count(),
            1
        );
    }
}
