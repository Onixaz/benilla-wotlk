//! The unit-token resolver's game side (`0x60abf0`): each base and hop read live, for the app's resolver and the VM's lent view.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{
    parse_unit_token, Hop, Hops, UiScript, UnitBase, UnitCounts, UnitGuids, UnitTokenParse,
    PET_OF_ANYONE,
};

use crate::net::{Guid, GuidIndex, ObjectStore, SelfPlayer};
use crate::target::{Hovered, HoveredObject, Selection};
use crate::ui_arena::ArenaOpponents;
use crate::ui_instance::EncounterUnits;
use crate::ui_party::GroupState;
use crate::vehicle::Passenger;

/// What the resolver reads from the game, wherever its state is held.
pub(crate) trait UnitWorld {
    /// Our entity and guid, while our object is held.
    fn me(&self) -> Option<(Entity, u64)>;
    /// Our seat while we ride.
    fn seat(&self) -> Option<Passenger>;
    /// The object manager's entity for `guid`.
    fn entity(&self, guid: u64) -> Option<Entity>;
    /// A held object's descriptor.
    fn store(&self, entity: Entity) -> Option<&ObjectStore>;
    /// The target global `[0xbd07b0]`.
    fn target(&self) -> Option<u64>;
    /// The object under the cursor `[0xbd07a0]`, the hovered unit a nearer GameObject did not take.
    fn pick(&self) -> Option<(Entity, u64)>;
    fn group(&self) -> Option<&GroupState>;
    fn encounter(&self) -> Option<&EncounterUnits>;
    fn arena(&self) -> Option<&ArenaOpponents>;
}

/// A held unit's descriptor, the object lookup with the unit type mask (`0x4d4db0`, mask 8); our
/// own object, the active player, answers for us.
fn unit_fields(w: &impl UnitWorld, guid: u64) -> Option<&benilla_protocol::messages::ObjectFields> {
    if let Some((me, _)) = w.me().filter(|&(_, g)| g == guid) {
        return w.store(me).map(|s| &s.0);
    }
    w.store(w.entity(guid)?)
        .filter(|s| s.is_unit())
        .map(|s| &s.0)
}

/// The held descriptor of `guid` for the group tables' pet reads, whatever its type.
fn live_fields(w: &impl UnitWorld, guid: u64) -> Option<&benilla_protocol::messages::ObjectFields> {
    w.entity(guid).and_then(|e| w.store(e)).map(|s| &s.0)
}

/// The guid in 0-based party slot `row`, the one `partyN` and `partypetN` share (`0x52bc50`).
fn party_member(w: &impl UnitWorld, row: u32) -> Option<u64> {
    let slot = usize::try_from(row).ok()?;
    w.group()?.party_slots().nth(slot).map(|m| m.guid)
}

/// The guid on 0-based raid row `row`, the one `raidN` and `GetRaidRosterInfo` share.
fn raid_member(w: &impl UnitWorld, row: u32) -> Option<u64> {
    let index = usize::try_from(row).ok()?.checked_add(1)?;
    crate::ui_party::raid_row_guid(w.group()?, w.me().map(|(_, g)| g), index)
}

/// The guid a token's base names before any hop, held or not; `None` for nobody, and for the
/// bases the VM answers (`focus`) or the view reads apart (`npc`, `questnpc`).
pub(crate) fn base(w: &impl UnitWorld, base: UnitBase) -> Option<u64> {
    match base {
        // The active player while its object is held (`0x60ac4b`).
        UnitBase::Player => w.me().map(|(_, g)| g),
        UnitBase::Target => w.target(),
        // The object track, kept while it is a held unit (`0x60aefa`) or a party or raid member or
        // pet (`0x60af0b`).
        UnitBase::Mouseover => {
            let guid = w.pick()?.1;
            let unit = unit_fields(w, guid).is_some();
            (unit || is_raid_member_or_pet(w, guid)).then_some(guid)
        }
        // Our charm, else our summon (`0x60acc3`-`0x60acd3`).
        UnitBase::Pet => unit_fields(w, w.me()?.1)?.unit_pet_guid(),
        UnitBase::Party(row) => party_member(w, row),
        UnitBase::Raid(row) => raid_member(w, row),
        UnitBase::PartyPet(row) => {
            let m = party_member(w, row)?;
            w.group()?.party_pet_guid(m, live_fields(w, m))
        }
        UnitBase::RaidPet(row) => {
            let m = raid_member(w, row)?;
            w.group()?.raid_pet_guid(m, live_fields(w, m))
        }
        // The encounter's slot (`0x5ed710`).
        UnitBase::Boss(row) => Some(w.encounter()?.boss(usize::try_from(row).ok()?)),
        // The opponent slot (`0x608180`), and its pet (`0x60a2f0`).
        UnitBase::Arena(row) => w
            .arena()?
            .opponents()
            .get(usize::try_from(row).ok()?)
            .copied(),
        UnitBase::ArenaPet(row) => {
            let pet_of = |g: u64| live_fields_store(w, g).and_then(crate::ui_arena::pet_of_store);
            let pets = w.arena()?.pets(&pet_of);
            pets.get(usize::try_from(row).ok()?).copied()
        }
        // The player's transport while seated (`0x60ac87`).
        UnitBase::Vehicle => w
            .seat()
            .filter(|p| p.state == crate::vehicle::SEATED)
            .map(|p| p.vehicle),
        UnitBase::Focus | UnitBase::Npc | UnitBase::QuestNpc => None,
        // Listed in commentator mode alone, which benilla never asks for.
        UnitBase::Commentator(_) => None,
    }
}

/// [`live_fields`]' store, for the arena's pet read.
fn live_fields_store(w: &impl UnitWorld, guid: u64) -> Option<&ObjectStore> {
    w.entity(guid).and_then(|e| w.store(e))
}

/// A held unit's `UNIT_FIELD_TARGET`, as a `target` hop off any unit but us reads it (`0x60aaa0`);
/// `None` for a guid that is no held unit.
pub(crate) fn unit_target(w: &impl UnitWorld, guid: u64) -> Option<u64> {
    if guid == 0 {
        return None;
    }
    unit_fields(w, guid).map(|f| f.unit_target().unwrap_or(0))
}

/// A held unit's pet, `UNIT_FIELD_CHARM`, else `UNIT_FIELD_SUMMON` (`0x60ab9a`).
pub(crate) fn unit_pet(w: &impl UnitWorld, guid: u64) -> Option<u64> {
    unit_fields(w, guid)?.unit_pet_guid()
}

/// Whether `guid` is a party member or on the raid roster (`0x512a00`), the `pet` hop's gate.
pub(crate) fn in_group(w: &impl UnitWorld, guid: u64) -> bool {
    let Some(group) = w.group() else {
        return false;
    };
    guid != 0
        && (group.party_slots().any(|m| m.guid == guid)
            || crate::ui_party::raid_row_guids(group, w.me().map(|(_, g)| g)).contains(&guid))
}

/// `0x512a30`: us or our pet, a party or raid member, or a member's pet.
fn is_raid_member_or_pet(w: &impl UnitWorld, guid: u64) -> bool {
    let own = w.me().and_then(|(_, g)| Some((g, unit_fields(w, g)?)));
    w.group()
        .is_some_and(|g| g.names_member_or_pet(guid, own, |m| live_fields(w, m)))
}

/// The leading rows each numbered base reads, for the name lookup and the unit-event fan-out.
pub(crate) fn counts(w: &impl UnitWorld) -> UnitCounts {
    let me = w.me().map(|(_, g)| g);
    let (party, raid) = w.group().map_or((0, 0), |g| {
        (
            g.party_slots().take(4).count(),
            crate::ui_party::raid_row_guids(g, me).len(),
        )
    });
    UnitCounts {
        party,
        raid,
        // `GetNumArenaOpponents` (`0x549630`).
        arena: w.arena().map_or(0, ArenaOpponents::count),
        boss: w.encounter().map_or(0, |e| e.guids().count()),
    }
}

/// Every base's guid and each numbered base's rows, read once for the VM's walks (`0x60abf0`).
pub(crate) fn tables(w: &impl UnitWorld) -> UnitGuids {
    let c = counts(w);
    let me = w.me().map(|(_, g)| g);
    let at = |b: UnitBase| base(w, b).unwrap_or(0);
    let raid = w
        .group()
        .map(|g| crate::ui_party::raid_row_guids(g, me))
        .unwrap_or_default();
    let raid_pets = raid
        .iter()
        .map(|&m| {
            w.group()
                .and_then(|g| g.raid_pet_guid(m, live_fields(w, m)))
                .unwrap_or(0)
        })
        .collect();
    let row = |i: usize| u32::try_from(i).unwrap_or(u32::MAX);
    UnitGuids {
        player: at(UnitBase::Player),
        pet: at(UnitBase::Pet),
        vehicle: at(UnitBase::Vehicle),
        target: at(UnitBase::Target),
        mouseover: at(UnitBase::Mouseover),
        npc: 0,
        party: std::array::from_fn(|i| at(UnitBase::Party(row(i)))),
        party_pets: std::array::from_fn(|i| at(UnitBase::PartyPet(row(i)))),
        raid,
        raid_pets,
        boss: (0..c.boss).map(|i| at(UnitBase::Boss(row(i)))).collect(),
        arena: std::array::from_fn(|i| at(UnitBase::Arena(row(i)))),
        arena_pets: std::array::from_fn(|i| at(UnitBase::ArenaPet(row(i)))),
        arena_count: c.arena,
        held: Default::default(),
    }
}

/// The hops from `guid` (`0x60aaa0`), `None` where they fail: `target` the target global off us, else `UNIT_FIELD_TARGET`; `pet` the charm, else the summon, off us or a member unless [`PET_OF_ANYONE`].
pub(crate) fn follow(w: &impl UnitWorld, mut guid: u64, hops: Hops<'_>) -> Option<u64> {
    let me = w.me();
    for hop in hops {
        let ours = guid != 0 && me.is_some_and(|(_, g)| g == guid);
        guid = match hop.ok()? {
            Hop::Target if ours => w.target().unwrap_or(0),
            Hop::Target => unit_target(w, guid).unwrap_or(0),
            Hop::Pet => {
                let anyone = me
                    .and_then(|(_, g)| unit_fields(w, g))
                    .is_some_and(|f| f.player_flags() & PET_OF_ANYONE != 0);
                if !(ours || in_group(w, guid) || anyone) {
                    return None;
                }
                unit_pet(w, guid).unwrap_or(0)
            }
        };
    }
    Some(guid)
}

/// The units a token can name at the call: each base's (the focus and the interact target given),
/// then each held unit a `target` chain reaches from them.
pub(crate) struct Walk {
    /// Every held unit visited, which a token names or a chain passes through.
    pub(crate) held: Vec<u64>,
    /// Each held unit's target, the focus and the vehicle, sorted, each once: the units a chain
    /// can end on, read by guid.
    pub(crate) ends: Vec<u64>,
}

/// Walk the bases and their `target` chains (`0x60aaa0`) over the live state.
pub(crate) fn walk(w: &impl UnitWorld, focus: u64, interact: u64) -> Walk {
    let mut stack: Vec<u64> = [
        UnitBase::Player,
        UnitBase::Pet,
        UnitBase::Vehicle,
        UnitBase::Target,
        UnitBase::Mouseover,
    ]
    .into_iter()
    .filter_map(|b| base(w, b))
    .chain([focus, interact])
    .collect();
    let c = counts(w);
    let rows = |n: usize| (0..n).filter_map(|r| u32::try_from(r).ok());
    for row in rows(4) {
        stack.extend(base(w, UnitBase::Party(row)));
        stack.extend(base(w, UnitBase::PartyPet(row)));
    }
    for row in rows(c.raid) {
        stack.extend(base(w, UnitBase::Raid(row)));
        stack.extend(base(w, UnitBase::RaidPet(row)));
    }
    for row in rows(crate::ui_arena::ARENA_SLOTS) {
        stack.extend(base(w, UnitBase::Arena(row)));
        stack.extend(base(w, UnitBase::ArenaPet(row)));
    }
    for row in rows(c.boss) {
        stack.extend(base(w, UnitBase::Boss(row)));
    }
    let mut held = Vec::new();
    let mut ends = Vec::new();
    while let Some(guid) = stack.pop() {
        if guid == 0 || held.contains(&guid) {
            continue;
        }
        if let Some(next) = unit_target(w, guid) {
            held.push(guid);
            ends.push(next);
            stack.push(next);
        }
    }
    ends.extend([focus, base(w, UnitBase::Vehicle).unwrap_or(0)]);
    ends.retain(|&g| g != 0 && g != benilla_ui::script::focus::NOBODY);
    ends.sort_unstable();
    ends.dedup();
    Walk { held, ends }
}

/// The resolver's state as the lent view reads it, every part optional: a UI-only harness lacks
/// their plugins. The target global comes apart ([`Self::at`]): the view lends the selection
/// itself.
#[derive(SystemParam)]
pub(crate) struct TokenView<'w, 's> {
    index: Option<Res<'w, GuidIndex>>,
    hovered: Option<Res<'w, Hovered>>,
    hovered_go: Option<Res<'w, HoveredObject>>,
    encounter: Option<Res<'w, EncounterUnits>>,
    arena: Option<Res<'w, ArenaOpponents>>,
    group: Option<Res<'w, GroupState>>,
    stores: Query<'w, 's, &'static ObjectStore>,
    me: Query<'w, 's, (Entity, &'static Guid, Option<&'static Passenger>), With<SelfPlayer>>,
}

impl<'w, 's> TokenView<'w, 's> {
    /// The view with the target global `target`.
    pub(crate) fn at(&self, target: Option<u64>) -> LiveUnits<'_, 'w, 's> {
        LiveUnits { view: self, target }
    }

    /// The units a token can name now, with the VM's `focus`, the selection's `target` and the
    /// interact target (`0xbd07a8`): the view lends the last two itself.
    pub(crate) fn walk(&self, focus: u64, target: Option<u64>, interact: u64) -> Walk {
        walk(&self.at(target), focus, interact)
    }
}

/// [`TokenView`] with the target global.
pub(crate) struct LiveUnits<'a, 'w, 's> {
    view: &'a TokenView<'w, 's>,
    target: Option<u64>,
}

impl UnitWorld for LiveUnits<'_, '_, '_> {
    fn me(&self) -> Option<(Entity, u64)> {
        self.view.me.iter().next().map(|(e, g, _)| (e, g.0))
    }

    fn seat(&self) -> Option<Passenger> {
        self.view.me.iter().next().and_then(|(_, _, p)| p.copied())
    }

    fn entity(&self, guid: u64) -> Option<Entity> {
        self.view.index.as_ref()?.0.get(&guid).copied()
    }

    fn store(&self, entity: Entity) -> Option<&ObjectStore> {
        self.view.stores.get(entity).ok()
    }

    fn target(&self) -> Option<u64> {
        self.target
    }

    fn pick(&self) -> Option<(Entity, u64)> {
        let go = self.view.hovered_go.as_deref().copied().unwrap_or_default();
        self.view.hovered.as_ref()?.mouseover(&go)
    }

    fn group(&self) -> Option<&GroupState> {
        self.view.group.as_deref()
    }

    fn encounter(&self) -> Option<&EncounterUnits> {
        self.view.encounter.as_deref()
    }

    fn arena(&self) -> Option<&ArenaOpponents> {
        self.view.arena.as_deref()
    }
}

/// The VM's part of a token: the focus `[0xbd07d0]` and the name lookup (`0x60a630`); the default reads neither.
#[derive(Clone, Copy, Default)]
pub(crate) struct VmUnits<'a>(Option<&'a UiScript>);

impl<'a> VmUnits<'a> {
    pub(crate) fn of(vm: &'a UiScript) -> Self {
        Self(Some(vm))
    }

    fn focus(self) -> Option<u64> {
        self.0.map(UiScript::focus_guid)
    }

    /// The VM's name lookup over the host's own bases and rows, read off `w`.
    fn unit_by_name<'t>(self, token: &'t str, w: &impl UnitWorld) -> Option<(u64, Hops<'t>)> {
        let bases = |b: UnitBase| base(w, b).unwrap_or(0);
        self.0?.unit_by_name(token, &bases, counts(w))
    }
}

/// [`VmUnits`] for a system that holds the VM for nothing else.
#[derive(SystemParam)]
pub(crate) struct UnitVm<'w> {
    script: Option<NonSend<'w, UiScript>>,
}

impl UnitVm<'_> {
    pub(crate) fn view(&self) -> VmUnits<'_> {
        VmUnits(self.script.as_deref())
    }
}

/// The app's unit-token resolver (`0x60abf0`), the [`Selection`] and the VM's part given per call.
#[derive(SystemParam)]
pub(crate) struct UnitTokens<'w, 's> {
    /// `Option`, like those below: a UI-only harness lacks their plugins; a bare `Res` panics.
    index: Option<Res<'w, GuidIndex>>,
    hovered: Option<Res<'w, Hovered>>,
    hovered_go: Option<Res<'w, HoveredObject>>,
    encounter: Option<Res<'w, EncounterUnits>>,
    arena: Option<Res<'w, ArenaOpponents>>,
    group: Res<'w, GroupState>,
    /// The interact target (`0xbd07a8`), the `"npc"` base.
    interact: Option<Res<'w, crate::ui_session::InteractTarget>>,
    /// The quest window's source (`0x6081b0`), the `"questnpc"` base.
    giver: Option<NonSend<'w, crate::ui_quest::QuestGiver>>,
    pub(crate) stores: Query<'w, 's, &'static ObjectStore>,
    me: Query<'w, 's, (Entity, &'static Guid, Option<&'static Passenger>), With<SelfPlayer>>,
}

/// [`UnitTokens`] with the selection it is asked against.
struct WithSelection<'a, 'w, 's> {
    tokens: &'a UnitTokens<'w, 's>,
    selection: &'a Selection,
}

impl UnitWorld for WithSelection<'_, '_, '_> {
    fn me(&self) -> Option<(Entity, u64)> {
        self.tokens.me()
    }

    fn seat(&self) -> Option<Passenger> {
        self.tokens
            .me
            .iter()
            .next()
            .and_then(|(_, _, p)| p.copied())
    }

    fn entity(&self, guid: u64) -> Option<Entity> {
        self.tokens.index.as_ref()?.0.get(&guid).copied()
    }

    fn store(&self, entity: Entity) -> Option<&ObjectStore> {
        self.tokens.stores.get(entity).ok()
    }

    fn target(&self) -> Option<u64> {
        self.selection.guid
    }

    fn pick(&self) -> Option<(Entity, u64)> {
        self.tokens.pick()
    }

    fn group(&self) -> Option<&GroupState> {
        Some(&self.tokens.group)
    }

    fn encounter(&self) -> Option<&EncounterUnits> {
        self.tokens.encounter.as_deref()
    }

    fn arena(&self) -> Option<&ArenaOpponents> {
        self.tokens.arena.as_deref()
    }
}

impl UnitTokens<'_, '_> {
    fn me(&self) -> Option<(Entity, u64)> {
        self.me.iter().next().map(|(e, g, _)| (e, g.0))
    }

    fn pick(&self) -> Option<(Entity, u64)> {
        let go = self.hovered_go.as_deref().copied().unwrap_or_default();
        self.hovered.as_ref()?.mouseover(&go)
    }

    /// The guid lookup, `pub(crate)` for `TargetLastEnemy`, which starts from a guid.
    pub(crate) fn held(&self, guid: u64) -> Option<(Entity, u64)> {
        Some((*self.index.as_ref()?.0.get(&guid)?, guid))
    }

    /// The held unit `token` names: [`Self::guid_of`], then the object manager, the player, the
    /// selection and the hover pick answering for their own units.
    pub(crate) fn resolve(
        &self,
        token: &str,
        selection: &Selection,
        vm: VmUnits<'_>,
    ) -> Option<(Entity, u64)> {
        let guid = self.guid_of(token, selection, vm)?;
        self.held(guid)
            .or_else(|| self.me().filter(|&(_, g)| g == guid))
            .or_else(|| {
                selection
                    .target
                    .filter(|_| selection.guid == Some(guid))
                    .map(|e| (e, guid))
            })
            .or_else(|| self.pick().filter(|&(_, g)| g == guid))
    }

    /// The guid `token` names, held or not, as the reference's resolve answers before any object
    /// lookup; `None` for nobody and for a token it refuses. A base whose unit is there, or that
    /// names nobody with no text after it, takes its hops; any other token is a name, which the
    /// VM looks up ([`VmUnits`]), and its hops.
    pub(crate) fn guid_of(
        &self,
        token: &str,
        selection: &Selection,
        vm: VmUnits<'_>,
    ) -> Option<u64> {
        let w = WithSelection {
            tokens: self,
            selection,
        };
        match parse_unit_token(token) {
            UnitTokenParse::Empty | UnitTokenParse::Nobody => return None,
            UnitTokenParse::Unit { base: b, hops } => {
                let guid = match b {
                    // The interact target (`0x60af67`).
                    UnitBase::Npc => self.interact.as_ref().and_then(|t| t.0),
                    // The quest window's source (`0x6081b0`).
                    UnitBase::QuestNpc => self.giver.as_ref().and_then(|g| g.quest_npc()),
                    // The VM's focus `[0xbd07d0]`.
                    UnitBase::Focus => vm.focus(),
                    b => base(&w, b),
                }
                .unwrap_or(0);
                if guid != 0 || hops.is_empty() {
                    if let Some(end) = follow(&w, guid, hops) {
                        return Some(end).filter(|&g| g != 0);
                    }
                }
            }
            UnitTokenParse::Name => {}
        }
        let (guid, hops) = vm.unit_by_name(token, &w)?;
        follow(&w, guid, hops).filter(|&g| g != 0)
    }
}
