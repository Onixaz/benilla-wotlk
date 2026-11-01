//! `SMSG_LOOT_LIST` (`0x71ca50` -> `0x71bc00`): a corpse's master looter and round-robin looter,
//! kept per unit and dropped with it, which the tooltip names on a dead unit.

use std::collections::HashMap;

use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::UnitState;
use bevy::prelude::*;

use crate::names::NameCache;
use crate::net::{GuidIndex, NetCommands, NetHandlerApp};

/// Each unit's `(master looter, looter)` guids, 0 for none.
#[derive(Resource, Default)]
pub(crate) struct LootLists(HashMap<u64, (u64, u64)>);

pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.init_resource::<LootLists>()
        .net_handler(K::LootList, on_loot_list)
        .net_handler(K::Disconnected, on_session_end)
        .add_systems(
            Update,
            // Before the unit snapshots that read it.
            tend_loot_lists
                .after(benilla_world::schedule::WorldStage::Net)
                .before(crate::ui_script::UiFeed),
        );
}

/// A unit we do not hold drops the message (`0x71ca50`).
fn on_loot_list(In(ev): In<SessionEvent>, mut lists: ResMut<LootLists>, index: Res<GuidIndex>) {
    let SessionEvent::LootList(p) = ev else {
        return;
    };
    debug!(
        "net: loot list for {:#x}: master {:#x}, looter {:#x}",
        p.creature, p.master_looter, p.looter
    );
    if index.0.contains_key(&p.creature) {
        lists.0.insert(p.creature, (p.master_looter, p.looter));
    }
}

fn on_session_end(In(_): In<SessionEvent>, mut lists: ResMut<LootLists>) {
    lists.0.clear();
}

/// The lists live on the unit object, so a unit that streams out takes its own; each named looter
/// is asked of the name cache, as the tooltip's lookup does.
fn tend_loot_lists(
    mut lists: ResMut<LootLists>,
    index: Res<GuidIndex>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
) {
    if lists.0.is_empty() {
        return;
    }
    if lists.0.keys().any(|g| !index.0.contains_key(g)) {
        lists.0.retain(|g, _| index.0.contains_key(g));
    }
    for &(master, looter) in lists.0.values() {
        for g in [master, looter] {
            if g != 0 {
                let _ = names.resolve(g, &commands);
            }
        }
    }
}

/// Fill a unit snapshot's looter names from its list; an unnamed looter shows no line until its
/// name lands, when the tooltip rebuilds.
pub(crate) fn apply_looters(
    state: &mut UnitState,
    guid: u64,
    lists: Option<&LootLists>,
    names: &NameCache,
) {
    let Some(&(master, looter)) = lists.and_then(|l| l.0.get(&guid)) else {
        return;
    };
    let name = |g: u64| {
        (g != 0)
            .then(|| names.peek(g).map(str::to_string))
            .flatten()
    };
    state.master_looter = name(master);
    state.looter = name(looter);
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::LootList;
    use bevy::ecs::system::RunSystemOnce;

    const CORPSE: u64 = 0xF130_0000_1C02_0001;

    fn world() -> World {
        let mut w = World::new();
        w.init_resource::<LootLists>();
        w.init_resource::<NameCache>();
        let e = w.spawn_empty().id();
        let mut index = GuidIndex::default();
        index.0.insert(CORPSE, e);
        w.insert_resource(index);
        w
    }

    fn list(w: &mut World, creature: u64, master: u64, looter: u64) {
        w.run_system_once_with(
            on_loot_list,
            SessionEvent::LootList(LootList {
                creature,
                master_looter: master,
                looter,
            }),
        )
        .unwrap();
    }

    /// Kept for a held unit, dropped for one we do not hold; the names fill once cached.
    #[test]
    fn a_held_unit_keeps_its_list_and_names_its_looters() {
        let mut w = world();
        list(&mut w, CORPSE, 0x2A, 0x2B);
        list(&mut w, 0xDEAD, 0x2A, 0);
        assert_eq!(w.resource::<LootLists>().0.len(), 1);

        w.resource_mut::<NameCache>()
            .insert_player(0x2A, "Claude".into(), None);
        let mut state = UnitState::default();
        apply_looters(
            &mut state,
            CORPSE,
            Some(w.resource::<LootLists>()),
            w.resource::<NameCache>(),
        );
        assert_eq!(state.master_looter.as_deref(), Some("Claude"));
        assert_eq!(state.looter, None, "unnamed yet");
    }

    /// The list goes with the unit.
    #[test]
    fn a_unit_that_streams_out_takes_its_list() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut w = world();
        w.insert_resource(NetCommands(tx));
        list(&mut w, CORPSE, 0x2A, 0);
        w.resource_mut::<GuidIndex>().0.clear();
        w.run_system_once(tend_loot_lists).unwrap();
        assert!(w.resource::<LootLists>().0.is_empty());
    }
}
