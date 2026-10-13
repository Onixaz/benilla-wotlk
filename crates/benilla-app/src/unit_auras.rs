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

/// A unit's live auras by slot, and the order the aura UI lists them in. A record without an
/// effect bit is an empty slot to every reader the client has (`0x72f7a5`, `0x72c5e6`), so it is
/// not kept.
#[derive(Component, Debug, Default, Clone, PartialEq)]
pub(crate) struct UnitAuras {
    by_slot: BTreeMap<u8, UnitAura>,
    /// `(slot, spell)` in display order, the index the aura finder walks (`unit+0xdd4`, rebuilt
    /// by `0x72c510`): a survivor keeps its place, a new aura appends, a batch of new ones in
    /// descending slot order (the rebuild's slot walk runs down, `0x72c6e9`).
    order: Vec<(u8, u32)>,
}

impl UnitAuras {
    /// Folds one packet in: `ALL` empties every slot first (`0x72f659`-`0x72f69f`), then each
    /// named slot takes its record or clears; then the display order is rebuilt. The reference
    /// rebuilds lazily, before the next finder walk (`0x72c9de`), so two packets read by no walk
    /// between them order as one there and as two here.
    pub(crate) fn apply(&mut self, update: &AuraUpdate, now: f64) {
        if update.all {
            self.by_slot.clear();
        }
        for (slot, record) in &update.slots {
            match record.filter(AuraRecord::is_live) {
                Some(r) => {
                    self.by_slot
                        .insert(*slot, UnitAura::from_record(*slot, update.unit, &r, now));
                }
                None => {
                    self.by_slot.remove(slot);
                }
            }
        }
        self.reorder();
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
        let mut t = Self::default();
        for &(slot, spell_id, flags) in auras {
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

/// Registers the feed's handler.
pub(crate) struct UnitAurasPlugin;

impl Plugin for UnitAurasPlugin {
    fn build(&self, app: &mut App) {
        app.net_handler(SessionEventKind::AuraUpdate, on_aura_update);
    }
}

/// A packet for a unit not in the object manager is skipped whole (`0x7300f1`); a unit's first
/// packet gives it the table.
fn on_aura_update(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    mut units: Query<(&ObjectStore, Option<&mut UnitAuras>)>,
    mut commands: Commands,
    clock: Res<Time<Real>>,
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
    match auras {
        Some(mut auras) => auras.apply(&update, now),
        None => {
            let mut auras = UnitAuras::default();
            auras.apply(&update, now);
            commands.entity(entity).insert(auras);
        }
    }
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
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(2, 0x9)]))
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
