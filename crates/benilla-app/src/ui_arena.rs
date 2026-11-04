//! Arena opponents: 12340's five opponent slots (`0xbe9f48`) and their pet slots (`0xbe9ef8`, the
//! pet then its owner), behind the `arenaN` and `arenapetN` tokens, `GetNumArenaOpponents` and
//! `ARENA_OPPONENT_UPDATE(token, what)`.
//!
//! An opponent comes from `SMSG_PARTY_MEMBER_STATS_FULL`'s arena byte (`0x6cf9fa`), its pet guid
//! with it (`0x6cfc45`), or from a player of the other arena side streaming in while the active
//! battlefield is an arena (`0x6e8300`). A slot keeps its guid until a world entry clears them all
//! (`0x54cb69`); `SMSG_ARENA_UNIT_DESTROYED` only announces the loss (`0x54ad10`).
//!
//! Not built: the `"unseen"` a slot's unit fires on leaving range (`0x4d7230` -> `0x54abf0`), the
//! stats 12340 keeps for an opponent out of range (`0x5497c0`), and the forced full stats refresh
//! of an opponent first seen streamed (`0xbe9ee0`).

use benilla_assets::MapCatalogRes;
use benilla_protocol::messages::ObjectType;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{ScriptValue, UiScript};
use bevy::ecs::system::NonSendMut;
use bevy::prelude::*;

use crate::net::{
    Guid, GuidIndex, NetHandlerApp, ObjectStore, SelfPlayer, WorldEnterCascadeMessage,
};
use crate::ui_battlefield::BattlefieldQueue;
use crate::ui_script::UiFeed;
use crate::ui_unit::UnitFeed;

/// The opponent and pet slots.
pub(crate) const ARENA_SLOTS: usize = 5;

pub(crate) const ARENA_TOKENS: [&str; ARENA_SLOTS] =
    ["arena1", "arena2", "arena3", "arena4", "arena5"];

pub(crate) const ARENA_PET_TOKENS: [&str; ARENA_SLOTS] = [
    "arenapet1",
    "arenapet2",
    "arenapet3",
    "arenapet4",
    "arenapet5",
];

/// An arena map's `Map.dbc` `InstanceType`, the active battlefield's `[0xbea570]`.
const INSTANCE_TYPE_ARENA: u32 = 4;

/// The slots and the `ARENA_OPPONENT_UPDATE`s owed to the VM, in the order 12340 fires them.
#[derive(Resource, Default)]
pub(crate) struct ArenaOpponents {
    opponents: [u64; ARENA_SLOTS],
    /// `(pet, owner)`: a pet whose owner holds no opponent slot.
    pets: [(u64, u64); ARENA_SLOTS],
    owed: Vec<(&'static str, &'static str)>,
}

/// The object manager's unit lookup as these functions use it: `None` when the guid names no
/// held unit, else that unit's pet, `UNIT_FIELD_CHARM` or, when zero, `UNIT_FIELD_SUMMON`.
type PetOf<'a> = &'a dyn Fn(u64) -> Option<u64>;

impl ArenaOpponents {
    /// `0x54d100`: the slot whose pet entry names the guid as owner, else the first slot with no
    /// pet that is free or already this guid's.
    fn add(&mut self, guid: u64, pet_of: PetOf) {
        if let Some(slot) = self.pets.iter().position(|&(_, owner)| owner == guid) {
            self.add_to_slot(guid, slot, pet_of);
            return;
        }
        let slot = (0..ARENA_SLOTS).find(|&i| {
            self.pets[i].0 == 0 && (self.opponents[i] == guid || self.opponents[i] == 0)
        });
        if let Some(slot) = slot {
            self.add_to_slot(guid, slot, pet_of);
        }
    }

    /// `0x54b840`: fills the slot, then `"seen"`, and `"unseen"` too when the unit is not held.
    fn add_to_slot(&mut self, guid: u64, slot: usize, pet_of: PetOf) {
        self.opponents[slot] = guid;
        self.owed.push((ARENA_TOKENS[slot], "seen"));
        if pet_of(guid).is_none() {
            self.owed.push((ARENA_TOKENS[slot], "unseen"));
        }
    }

    /// `0x54d1c0`: the owner's opponent slot or the slot whose pet entry it owns, else the first
    /// slot with neither an opponent nor a pet.
    fn add_pet(&mut self, pet: u64, owner: u64, pet_of: PetOf) {
        let slot = (0..ARENA_SLOTS)
            .find(|&i| self.opponents[i] == owner || self.pets[i].1 == owner)
            .or_else(|| (0..ARENA_SLOTS).find(|&i| self.opponents[i] == 0 && self.pets[i].0 == 0));
        if let Some(slot) = slot {
            // `0x54b920`.
            self.pets[slot] = (pet, owner);
            self.owed.push((ARENA_PET_TOKENS[slot], "seen"));
            if pet_of(pet).is_none() {
                self.owed.push((ARENA_PET_TOKENS[slot], "unseen"));
            }
        }
    }

    /// `0x54ad10`: an opponent, or a held opponent's live pet, announces `"destroyed"` and keeps
    /// its slot; otherwise a pet entry naming the guid is cleared and announces it.
    fn destroyed(&mut self, guid: u64, pet_of: PetOf) {
        for (i, &opponent) in self.opponents.iter().enumerate() {
            if opponent == guid {
                self.owed.push((ARENA_TOKENS[i], "destroyed"));
                return;
            }
            if opponent != 0 && pet_of(opponent) == Some(guid) {
                self.owed.push((ARENA_PET_TOKENS[i], "destroyed"));
                return;
            }
        }
        if let Some(i) = self.pets.iter().position(|&(pet, _)| pet == guid) {
            self.pets[i] = (0, 0);
            self.owed.push((ARENA_PET_TOKENS[i], "destroyed"));
        }
    }

    /// The world-entry clear (`0x54cb69`): every slot, then `"cleared"` for `arenaN` and
    /// `arenapetN` in turn.
    fn clear(&mut self) {
        self.opponents = [0; ARENA_SLOTS];
        self.pets = [(0, 0); ARENA_SLOTS];
        for (arena, pet) in ARENA_TOKENS.into_iter().zip(ARENA_PET_TOKENS) {
            self.owed.push((arena, "cleared"));
            self.owed.push((pet, "cleared"));
        }
    }

    /// `GetNumArenaOpponents` (`0x549630`): the leading slots holding an opponent or a pet.
    pub(crate) fn count(&self) -> usize {
        (0..ARENA_SLOTS)
            .take_while(|&i| self.opponents[i] != 0 || self.pets[i].0 != 0)
            .count()
    }

    /// `arenaN`'s guid, slot N-1 (`0x608180`).
    pub(crate) fn opponents(&self) -> [u64; ARENA_SLOTS] {
        self.opponents
    }

    /// `arenapetN`'s guid (`0x60a2f0`): a held opponent's live pet, else the slot's pet entry.
    pub(crate) fn pets(&self, pet_of: PetOf) -> [u64; ARENA_SLOTS] {
        std::array::from_fn(|i| pet_of(self.opponents[i]).unwrap_or(self.pets[i].0))
    }
}

/// The held unit's pet as `0x60a2f0` and `0x54ad10` read it: `UNIT_FIELD_CHARM`, else
/// `UNIT_FIELD_SUMMON`.
pub(crate) fn pet_of<'a, 'w, 's, 'd>(
    index: &'a GuidIndex,
    stores: &'a Query<'w, 's, &'d ObjectStore>,
) -> impl Fn(u64) -> Option<u64> + use<'a, 'w, 's, 'd> {
    move |guid| {
        let store = index.0.get(&guid).and_then(|&e| stores.get(e).ok())?;
        if !store.is_unit() {
            return None;
        }
        let charm = store.0.unit_charm().unwrap_or(0);
        Some(if charm != 0 {
            charm
        } else {
            store.0.unit_summon().unwrap_or(0)
        })
    }
}

/// Whether the active battlefield is an arena: the active map's `Map.dbc` instance type, which the
/// active status records.
fn in_arena(queue: &BattlefieldQueue, maps: Option<&MapCatalogRes>) -> bool {
    queue.active_map().and_then(|m| maps?.0.instance_type(m)) == Some(INSTANCE_TYPE_ARENA)
}

fn on_party_stats(
    In(ev): In<SessionEvent>,
    mut arena: ResMut<ArenaOpponents>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
) {
    let SessionEvent::PartyMemberStats { guid, info, .. } = ev else {
        return;
    };
    if !info.arena_opponent {
        return;
    }
    let pet_of = pet_of(&index, &stores);
    arena.add(guid, &pet_of);
    if let Some(pet) = info.pet_guid.filter(|&p| p != 0) {
        arena.add_pet(pet, guid, &pet_of);
    }
}

/// `SMSG_ARENA_UNIT_DESTROYED` (`0x54b5e0`): read only while the active battlefield is an arena.
fn on_unit_destroyed(
    In(ev): In<SessionEvent>,
    mut arena: ResMut<ArenaOpponents>,
    queue: Res<BattlefieldQueue>,
    maps: Option<Res<MapCatalogRes>>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
) {
    let SessionEvent::ArenaUnitDestroyed(guid) = ev else {
        return;
    };
    if in_arena(&queue, maps.as_deref()) {
        arena.destroyed(guid, &pet_of(&index, &stores));
    } else {
        debug!("ui_arena: unit {guid:#x} destroyed outside an arena — dropped");
    }
}

/// The world-entry clear, then each player of the other arena side that streams in while in an
/// arena (`0x6e8300`-`0x6e832c`); our own side's byte is the status's arena faction.
#[allow(clippy::type_complexity)] // a Bevy system's full input set
fn track_streamed_players(
    mut cascades: MessageReader<WorldEnterCascadeMessage>,
    mut arena: ResMut<ArenaOpponents>,
    queue: Res<BattlefieldQueue>,
    maps: Option<Res<MapCatalogRes>>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    added: Query<(&Guid, &ObjectStore), (Added<ObjectStore>, Without<SelfPlayer>)>,
) {
    if cascades.read().count() > 0 {
        arena.clear();
    }
    if added.is_empty() || !in_arena(&queue, maps.as_deref()) {
        return;
    }
    let pet_of = pet_of(&index, &stores);
    for (guid, store) in &added {
        let side = store.0.player_arena_faction();
        if store.0.object_type() == Some(ObjectType::Player)
            && side.is_some_and(|s| s != queue.arena_faction())
        {
            arena.add(guid.0, &pet_of);
        }
    }
}

/// The owed `ARENA_OPPONENT_UPDATE`s, after the aura feed pushed the tokens they name.
fn feed_arena(script: Option<NonSendMut<UiScript>>, mut arena: ResMut<ArenaOpponents>) {
    let Some(mut script) = script else {
        return;
    };
    if arena.owed.is_empty() {
        return;
    }
    // The drain leaves change detection to the slots, which the unit feed rebuilds on.
    for (token, what) in std::mem::take(&mut arena.bypass_change_detection().owed) {
        script.fire_event(
            "ARENA_OPPONENT_UPDATE",
            vec![
                ScriptValue::Str(token.into()),
                ScriptValue::Str(what.into()),
            ],
        );
    }
}

pub(crate) struct UiArenaPlugin;

impl Plugin for UiArenaPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ArenaOpponents>()
            .net_handler(SessionEventKind::PartyMemberStats, on_party_stats)
            .net_handler(SessionEventKind::ArenaUnitDestroyed, on_unit_destroyed)
            // The slots fill ahead of the unit feed, so the tokens an event names resolve when it
            // fires, as the reference fills a slot before it signals.
            .add_systems(
                Update,
                track_streamed_players
                    .after(benilla_world::schedule::WorldStage::Net)
                    .before(UiFeed),
            )
            .add_systems(
                Update,
                feed_arena
                    .run_if(crate::ui_script::ingame_ui_up)
                    .in_set(UiFeed)
                    .after(UnitFeed),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u64 = 0x21;
    const B: u64 = 0x22;
    const PET: u64 = 0xF140_0000_0000_0031;

    fn held(guids: &'static [(u64, u64)]) -> impl Fn(u64) -> Option<u64> {
        move |g| guids.iter().find(|(u, _)| *u == g).map(|&(_, pet)| pet)
    }

    /// `0x54d100`/`0x54b840`: the first free slot, `"seen"`, and `"unseen"` for a unit not held; a
    /// repeat lands on its own slot and announces again.
    #[test]
    fn an_opponent_takes_the_first_free_slot_and_is_seen() {
        let mut arena = ArenaOpponents::default();
        let pet_of = held(&[(A, 0)]);
        arena.add(A, &pet_of);
        arena.add(B, &pet_of);
        arena.add(A, &pet_of);
        assert_eq!(arena.opponents(), [A, B, 0, 0, 0]);
        assert_eq!(arena.count(), 2);
        assert_eq!(
            arena.owed,
            [
                ("arena1", "seen"),
                ("arena2", "seen"),
                ("arena2", "unseen"),
                ("arena1", "seen")
            ]
        );
    }

    /// `0x54d1c0`: a pet whose owner holds no slot takes a free one, its owner then joins that
    /// same slot (`0x54d100`'s owner match), and `arenapetN` reads the slot's pet entry until the
    /// owner is held with a live pet (`0x60a2f0`).
    #[test]
    fn a_pet_ahead_of_its_owner_keeps_the_owners_slot() {
        let mut arena = ArenaOpponents::default();
        let none = held(&[]);
        arena.add(A, &none);
        arena.add_pet(PET, B, &none);
        assert_eq!(arena.count(), 2, "slot 2 holds a pet only");
        arena.add(B, &none);
        assert_eq!(arena.opponents(), [A, B, 0, 0, 0]);
        assert_eq!(arena.pets(&none), [0, PET, 0, 0, 0]);
        assert_eq!(arena.pets(&held(&[(B, 0x99)])), [0, 0x99, 0, 0, 0]);
    }

    /// `0x54ad10`: the opponent's own slot, then a held opponent's live pet, then the pet table,
    /// whose entry alone is cleared.
    #[test]
    fn destroyed_names_the_slot_holding_the_guid() {
        let mut arena = ArenaOpponents::default();
        let pet_of = held(&[(A, PET)]);
        arena.add(A, &pet_of);
        arena.add_pet(0x77, B, &pet_of);
        arena.owed.clear();
        arena.destroyed(A, &pet_of);
        arena.destroyed(PET, &pet_of);
        arena.destroyed(0x77, &pet_of);
        arena.destroyed(0x1234, &pet_of);
        assert_eq!(
            arena.owed,
            [
                ("arena1", "destroyed"),
                ("arenapet1", "destroyed"),
                ("arenapet2", "destroyed")
            ]
        );
        assert_eq!(
            arena.opponents(),
            [A, 0, 0, 0, 0],
            "the opponent keeps its slot"
        );
        assert_eq!(arena.count(), 1, "the pet entry went");
    }

    /// `0x54cb69`: every slot cleared, `"cleared"` for `arenaN` then `arenapetN`, slot by slot.
    #[test]
    fn world_entry_clears_every_slot() {
        let mut arena = ArenaOpponents::default();
        arena.add(A, &held(&[(A, 0)]));
        arena.owed.clear();
        arena.clear();
        assert_eq!(arena.count(), 0);
        assert_eq!(arena.owed.len(), 10);
        assert_eq!(
            arena.owed[..3],
            [
                ("arena1", "cleared"),
                ("arenapet1", "cleared"),
                ("arena2", "cleared")
            ]
        );
    }
}
