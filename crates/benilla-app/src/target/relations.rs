//! The three unit-relationship predicates the reference shares across systems, may I attack,
//! interact with or help this unit, as the store-only entry points every consumer calls:
//!
//! - The attack check forwards to [`super::ring::can_attack_from_player`],
//! - The interact check forwards to [`super::ring::can_interact_from_player`],
//! - The assist check (`0x7293d0`) is derived here, in either direction ([`Assist`]).
//!
//! The first two live in [`super::ring`] beside the reaction directions they turn on: the reaction
//! asked of the player answers a reputation faction with the at-war bit, asked of the unit with the
//! standing, and the two often disagree.
//!
//! A corpse has its own reaction gate ([`corpse_friendly`]).

use benilla_protocol::messages::{ObjectType, OwnerFallback};

use crate::net::{ObjectStore, Reputations};
use crate::ui_party::GroupState;

use super::{ring_reaction, Factions, GroupMembers};

/// `OBJECT_FIELD_TYPE` bit 4, the reference's own player test (in the attack check, in the interact
/// check), read off the store the rest of the predicate walks.
///
/// Relies on the create block carrying field 2: vmangos sends every non-zero field, and a player's
/// is `0x19`. A miss only skips the attack check's ghost refusal, which the both-player-controlled
/// arm makes anyway; `/reaction` prints the field beside the `NetEntity` kind.
fn is_player_object(store: Option<&ObjectStore>) -> bool {
    store.and_then(|s| s.0.object_type()) == Some(ObjectType::Player)
}

/// The attack check, the one attackability question of the world cursor's sword, the combat flash,
/// the TAB scan's filter 3, `UnitCanAttack` and hostile spell targeting. Its mixed arm is
/// `UnitReaction(player → target) < 4`, which answers a reputation faction with the at-war bit,
/// never the standing, so a not-at-war neutral faction (Cenarion Circle) is not attackable.
pub(crate) fn can_attack(
    store: Option<&ObjectStore>,
    factions: Option<&Factions>,
    reputations: &Reputations,
    party: &dyn GroupMembers,
    self_store: Option<&ObjectStore>,
) -> bool {
    super::ring::can_attack_from_player(
        factions,
        reputations,
        party,
        store,
        self_store,
        is_player_object(store),
    )
}

/// The interact check, may I take a service from this unit: the world-cursor classifier runs it
/// (through the interact check's timed wrapper) to choose between the NPC service ladder and the
/// loot, skin and attack block.
pub(crate) fn can_interact(
    store: Option<&ObjectStore>,
    factions: Option<&Factions>,
    reputations: &Reputations,
    party: &dyn GroupMembers,
    self_store: Option<&ObjectStore>,
) -> bool {
    super::ring::can_interact_from_player(factions, reputations, party, store, self_store)
}

/// `UNIT_FLAG_NOT_SELECTABLE`, bit 25 (vmangos `UnitDefines.h:570`): the assist check's first
/// refusal and one of the attack check's.
const UNIT_FLAG_NOT_SELECTABLE: u32 = 1 << 25;

/// The selectable test, the unit object's vtable slot 21: `UNIT_FLAG_NOT_SELECTABLE` clear, or the
/// unit's `UNIT_FIELD_CREATEDBY` is the active player, so your own flagged totems and traps stay
/// selectable. Every vtable but the unit object's and the player object's has the `xor eax,eax`
/// stub there, so a GameObject, item or corpse never becomes the selection.
///
/// `None` passes: the selection set makes the call only on an object the manager resolved, which
/// keeps an out-of-range party member selectable. A store whose `OBJECT_FIELD_TYPE` has not
/// streamed passes the same way.
pub(crate) fn is_selectable(store: Option<&ObjectStore>, self_guid: Option<u64>) -> bool {
    let Some(store) = store else {
        return true; // no resolved object: the reference skips the call
    };
    // Only the unit object and the player object override slot 21.
    if !matches!(
        store.0.object_type(),
        None | Some(ObjectType::Unit) | Some(ObjectType::Player)
    ) {
        return false;
    }
    store.0.unit_flags() & UNIT_FLAG_NOT_SELECTABLE == 0
        || (self_guid.is_some() && store.0.unit_created_by() == self_guid)
}

/// `UNIT_FLAG_POSSESSED`, `UNIT_FIELD_FLAGS` bit 24 (vmangos `UnitDefines.h:569`), which the
/// reference reads as `[[unit+0x110]+0xA3] & 1`.
pub(crate) const UNIT_FLAG_POSSESSED: u32 = 0x0100_0000;

/// Whether the active player possesses this unit (Mind Control, an Eye of Kilrogg, Eyes of the
/// Beast): `UNIT_FLAG_POSSESSED` set and `UNIT_FIELD_CHARMEDBY`, else `UNIT_FIELD_CREATEDBY`, the
/// active player's guid. The own-pet resolver and the world pick's filter read this same clause. No
/// store, no owner or no active player is no possession.
pub(crate) fn is_possessed_by(store: Option<&ObjectStore>, self_guid: Option<u64>) -> bool {
    let (Some(store), Some(me)) = (store, self_guid) else {
        return false;
    };
    store.0.unit_flags() & UNIT_FLAG_POSSESSED != 0
        && store.0.unit_owner(OwnerFallback::CreatedBy) == Some(me)
}

/// The corpse's reaction gate, asked as `(player, corpse)`: the reference compares the player's
/// template (`UNIT_FIELD_FACTIONTEMPLATE`) toward the dead player's race's (the race byte of
/// `CORPSE_FIELD_BYTES_1`, then `ChrRaces.dbc` column 2) with the template comparator alone, no
/// reputation, and passes at friendly (`setge` 4). A missing row on either side is neutral, so it
/// fails, as does a missing catalog or player.
pub(crate) fn corpse_friendly(
    corpse: &ObjectStore,
    factions: Option<&Factions>,
    self_store: Option<&ObjectStore>,
) -> bool {
    use benilla_formats::Reaction;
    let reaction = (|| {
        let catalog = factions?.catalog();
        let own = catalog.template(self_store?.0.unit_faction_template()?)?;
        Some(own.reaction_toward(catalog.race_template(corpse.0.corpse_race())?))
    })();
    reaction.unwrap_or(Reaction::Neutral) >= Reaction::Friendly
}

/// `UNIT_FIELD_FLAGS` bit 3, player-controlled (vmangos `UnitDefines.h:548`), the bit
/// [`ring_reaction`]'s duel leg selects on.
const UNIT_FLAG_PLAYER_CONTROLLED: u32 = 0x8;
/// The cross-flag immunity bits the assist check reads on its target (`0x7293d0`), as the attack
/// check does.
const IMMUNE_TO_PLAYER_CONTROLLED: u32 = 0x100;
const IMMUNE_TO_UNCONTROLLED: u32 = 0x200;
/// Creature template type flags: the aids-players test (`0x715f90`) and the raid-unit test
/// (`0x715df0`).
const TYPE_FLAG_AIDS_PLAYERS: u32 = 1 << 12;
const TYPE_FLAG_TREAT_AS_RAID_UNIT: u32 = 1 << 26;
/// `UNIT_FIELD_BYTES_2` byte 1's free-for-all bit, beside [`BYTE2_PVP`] and [`BYTE2_SANCTUARY`].
const BYTE2_FFA_PVP: u8 = 0x4;

/// The side the local player takes in an assist check.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Assist {
    /// May the player assist the unit: every spell, `UnitCanAssist` and targeting caller.
    PlayerToUnit,
    /// May the unit assist the player: the area spirit healer walk (`0x523f80`).
    UnitToPlayer,
}

/// The assist check (`0x7293d0`) from the player, toward `store` ([`Assist::PlayerToUnit`]).
/// `names` resolves the creature template type flags; a unit not in the cache reads none.
pub(crate) fn can_assist(
    store: Option<&ObjectStore>,
    factions: Option<&Factions>,
    reputations: &Reputations,
    party: &dyn GroupMembers,
    self_store: Option<&ObjectStore>,
    names: Option<&crate::names::NameCache>,
) -> bool {
    can_assist_dir(
        Assist::PlayerToUnit,
        store,
        factions,
        reputations,
        party,
        self_store,
        names,
    )
}

/// The assist check (`0x7293d0`) with its ignore-flags argument 0, between the player and `store`
/// in `dir`:
/// 1. the target's `UNIT_FLAG_NOT_SELECTABLE` refuses, then the immunity bit facing the actor;
/// 2. the actor's reaction toward the target below 4 refuses, unless the actor is a raid-unit
///    creature;
/// 3. a player-controlled target refuses a duel opponent and an FFA target when the actor is not
///    FFA, then passes unless the actor stands in a sanctuary toward a flagged target outside one;
/// 4. an uncontrolled actor passes;
/// 5. else the target needs the PvP bit or an aiding or raid-unit creature template.
///
/// Not applied: the controller resolve, which reads a pet's duel state off its owner.
pub(crate) fn can_assist_dir(
    dir: Assist,
    store: Option<&ObjectStore>,
    factions: Option<&Factions>,
    reputations: &Reputations,
    party: &dyn GroupMembers,
    self_store: Option<&ObjectStore>,
    names: Option<&crate::names::NameCache>,
) -> bool {
    let (Some(unit), Some(me)) = (store, self_store) else {
        return false;
    };
    let unit_type_flags = unit
        .0
        .object_entry()
        .filter(|_| !is_player_object(Some(unit)))
        .and_then(|e| names?.creature_record(e))
        .map_or(0, |r| r.type_flags);
    let (actor, target, actor_type_flags, target_type_flags, reaction) = match dir {
        Assist::PlayerToUnit => (
            me,
            unit,
            0,
            unit_type_flags,
            super::ring::reaction_from_player(factions, reputations, party, Some(unit), Some(me)),
        ),
        Assist::UnitToPlayer => (
            unit,
            me,
            unit_type_flags,
            0,
            ring_reaction(factions, reputations, party, Some(unit), Some(me)),
        ),
    };
    let (aflags, tflags) = (actor.0.unit_flags(), target.0.unit_flags());
    if tflags & UNIT_FLAG_NOT_SELECTABLE != 0 {
        return false;
    }
    let actor_pc = aflags & UNIT_FLAG_PLAYER_CONTROLLED != 0;
    if actor_pc && tflags & IMMUNE_TO_PLAYER_CONTROLLED != 0
        || !actor_pc && tflags & IMMUNE_TO_UNCONTROLLED != 0
    {
        return false;
    }
    if reaction < 4 && actor_type_flags & TYPE_FLAG_TREAT_AS_RAID_UNIT == 0 {
        return false;
    }
    let (abyte, tbyte) = (actor.0.unit_byte2_flags(), target.0.unit_byte2_flags());
    if tflags & UNIT_FLAG_PLAYER_CONTROLLED != 0 {
        let team = target.0.player_duel_team();
        if team != 0
            && (actor.0.player_duel_arbiter() != target.0.player_duel_arbiter()
                || actor.0.player_duel_team() != team)
        {
            return false;
        }
        if tbyte & BYTE2_FFA_PVP != 0 && abyte & BYTE2_FFA_PVP == 0 {
            return false;
        }
        return abyte & BYTE2_SANCTUARY == 0
            || tbyte & BYTE2_SANCTUARY != 0
            || tbyte & BYTE2_PVP == 0;
    }
    if !actor_pc {
        return true;
    }
    tbyte & BYTE2_PVP != 0
        || target_type_flags & (TYPE_FLAG_AIDS_PLAYERS | TYPE_FLAG_TREAT_AS_RAID_UNIT) != 0
}

/// 12340's reaction palette at `0xbd0c94`, ARGB, indexed by `0x7251c0`'s 0-based answer: hated
/// and hostile red, unfriendly orange, neutral yellow, friendly and up green (`0x521bf4`).
const SELECTION_REACTION: [u32; 8] = [
    0xFFFF_0000,
    0xFFFF_0000,
    0xFFFF_8000,
    0xFFFF_FF00,
    0xFF00_FF00,
    0xFF00_FF00,
    0xFF00_FF00,
    0xFF00_FF00,
];
/// `0xbd0c8c[0]`, a player-controlled unit neither side can attack and no PvP flag colours.
const SELECTION_BLUE: u32 = 0xFF00_00FF;
/// `UNIT_BYTE2_FLAG_PVP` and `_SANCTUARY`, `UNIT_FIELD_BYTES_2` byte 1 (`0x521ea5`).
const BYTE2_PVP: u8 = 0x1;
const BYTE2_SANCTUARY: u8 = 0x8;
/// `UNIT_DYNFLAG_DEAD`, the second half of the resolver's dead test (`0x521f6b`).
const DYNFLAG_DEAD: u32 = 0x20;

/// The resolver's player-controlled verdict, from the attack check (`0x729740`) asked both ways.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Matrix {
    /// The unit can attack us and we it (`0x521e60`).
    Mutual,
    /// It can attack us; we cannot attack it.
    TheyOnly,
    /// We can attack it; it cannot attack us (`0x521e9e`).
    WeOnly,
    /// Neither can; `pvp` is its PvP bit with neither side in a sanctuary (`0x521ea5`).
    Neither { pvp: bool },
}

/// `0x521bf0`'s question for a player-controlled unit, as both of its modes ask it.
fn attack_matrix(
    store: &ObjectStore,
    own: &ObjectStore,
    factions: Option<&Factions>,
    reputations: &Reputations,
    party: &dyn GroupMembers,
) -> Matrix {
    // The attack check of the unit on the player: the arm both bit-3 flags select is the same
    // either way round, so the player-side predicate with the two swapped asks it.
    let attacks_us = super::ring::can_attack_from_player(
        factions,
        reputations,
        party,
        Some(own),
        Some(store),
        is_player_object(Some(own)),
    );
    let we_attack = can_attack(Some(store), factions, reputations, party, Some(own));
    match (attacks_us, we_attack) {
        (true, true) => Matrix::Mutual,
        (true, false) => Matrix::TheyOnly,
        (false, true) => Matrix::WeOnly,
        (false, false) => {
            let (theirs, ours) = (store.0.unit_byte2_flags(), own.0.unit_byte2_flags());
            Matrix::Neither {
                pvp: theirs & BYTE2_PVP != 0
                    && theirs & BYTE2_SANCTUARY == 0
                    && ours & BYTE2_SANCTUARY == 0,
            }
        }
    }
}

/// `UnitSelectionColor`'s ARGB dword, 12340's resolver `0x521bf0` with its third argument 0, as
/// `0x60da6f` calls it: a unit that is not player-controlled takes the palette entry of its
/// reaction toward the player (`0x521fa7`, no dead grey in this mode). A player-controlled one asks
/// the attack check (`0x729740`) both ways: it can attack us and we it, red; it us only, blue; we
/// it only, yellow (`0x521e45`-`0x521e9e`). Else a PvP-flagged unit, neither side in a sanctuary,
/// takes its reaction's entry, and any other blue. `self_store` is the player's own descriptor;
/// passing it as `store` reads the player's own colour, whose reaction toward itself is 4
/// (`0x7251c0`). Not applied: the active mover's vehicle or charm (`0x6dd060`, `0x5140a0`) and the
/// zone's sanctuary test (`[0xbea570] == 4`).
pub(crate) fn selection_color(
    store: &ObjectStore,
    factions: Option<&Factions>,
    reputations: &Reputations,
    party: &dyn GroupMembers,
    self_store: Option<&ObjectStore>,
) -> u32 {
    let Some(own) = self_store else {
        return SELECTION_BLUE;
    };
    let reaction_color = || {
        let r = ring_reaction(factions, reputations, party, Some(store), Some(own));
        SELECTION_REACTION[usize::from(r.min(7))]
    };
    if store.0.unit_flags() & UNIT_FLAG_PLAYER_CONTROLLED == 0 {
        return reaction_color();
    }
    match attack_matrix(store, own, factions, reputations, party) {
        Matrix::Mutual => SELECTION_REACTION[1],
        Matrix::TheyOnly => SELECTION_BLUE,
        Matrix::WeOnly => SELECTION_REACTION[3],
        Matrix::Neither { pvp: true } => reaction_color(),
        Matrix::Neither { pvp: false } => SELECTION_BLUE,
    }
}

/// The colour class of the ground ring and the overhead name: `0x521bf0` with its third argument
/// 1, as the unit's highlight colour `0x718ac0` asks it for the ring draw and the name string
/// (`0x725980`). It runs the same matrix as [`selection_color`], and differs in four places:
/// - a dead unit that is not player-controlled is grey (`0x521f6b`);
/// - an unattackable player-controlled unit in our party (our subgroup, in a raid), or its pet,
///   takes the party colour, pale green when PvP-flagged (`0xbd0c84`) and pale blue when not (`0xbd0c88`);
/// - else one on our friend list takes the friend colour (`0xbd0c80`);
/// - the blue entry is the second of `0xbd0c8c`, a soft blue.
///
/// The combat flash, which `0x718ac0` checks before the resolver, is the callers'. Not applied, as
/// in [`selection_color`]: the active mover's vehicle or charm, and the zone sanctuary test.
pub(crate) fn highlight_variant(
    store: &ObjectStore,
    factions: Option<&Factions>,
    reputations: &Reputations,
    party: &GroupState,
    self_store: Option<&ObjectStore>,
    is_friend: impl Fn(u64) -> bool,
) -> super::RingVariant {
    use super::RingVariant as V;
    let Some(own) = self_store else {
        return V::Player;
    };
    let reaction_variant =
        || match ring_reaction(factions, reputations, party, Some(store), Some(own)) {
            0..=1 => V::Hostile,
            2 => V::Unfriendly,
            3 => V::Neutral,
            _ => V::Friendly,
        };
    if store.0.unit_flags() & UNIT_FLAG_PLAYER_CONTROLLED == 0 {
        if store.0.unit_is_dead() || store.0.unit_dynamic_flags() & DYNFLAG_DEAD != 0 {
            return V::Dead;
        }
        return reaction_variant();
    }
    let pvp = match attack_matrix(store, own, factions, reputations, party) {
        Matrix::Mutual => return V::Hostile,
        Matrix::TheyOnly => return V::Player,
        Matrix::WeOnly => return V::Neutral,
        Matrix::Neither { pvp } => pvp,
    };
    // The party test: the unit, or the owner of a pet, in the party slots, which in a raid hold
    // our own subgroup; none once a leave or a kick emptied them.
    let guid = store.0.object_guid();
    let owner = store.0.unit_owner(OwnerFallback::CreatedBy);
    let grouped = !party.slots_emptied
        && [guid, owner]
            .into_iter()
            .flatten()
            .any(|g| party.party_slots().any(|m| m.guid == g));
    if grouped {
        return if pvp { V::PartyPvp } else { V::Party };
    }
    if guid.is_some_and(is_friend) {
        return V::Friend;
    }
    if pvp {
        reaction_variant()
    } else {
        V::Player
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spell::targeting::corpse_fixture as fx;
    use crate::target::NoGroup;

    /// Free-for-all, every unit and our player carrying `UNIT_BYTE2_FLAG_FFA_PVP`: the rungs ahead
    /// of the FFA rung (`0x7251c0`) decide self, our pet, the group and a duel, and the ring and
    /// name colour (`0x521bf0` mode 1) follows the attack-check matrix they produce.
    mod ffa {
        use super::*;
        use crate::target::{reaction_from_player, ring_reaction, RingVariant};
        use benilla_formats::{FactionCatalog, FactionTemplate};
        use benilla_protocol::messages::GroupMemberEntry;
        use std::collections::HashMap;

        const ME: u64 = 0x10;
        const MEMBER: u64 = 0x20;
        const RAIDER: u64 = 0x30;
        const STRANGER: u64 = 0x40;
        const FRIEND: u64 = 0x50;
        const OUR_PET: u64 = 0xF140_0000_0000_0001;
        const MEMBER_PET: u64 = 0xF140_0000_0000_0002;

        fn factions() -> Factions {
            let human = FactionTemplate {
                faction: 1,
                group_mask: 3,
                friend_group_mask: 2,
                enemy_group_mask: 12,
                enemies: [0; 4],
                friends: [0; 4],
            };
            Factions::from_catalog(FactionCatalog::from_rows(
                HashMap::from([(1, human)]),
                HashMap::new(),
            ))
        }

        /// A Human player-controlled unit with the FFA bit, plus `byte2` and any `extra` fields:
        /// `OBJECT_FIELD_GUID` 0-1, type 2, template 35, flags 46, `UNIT_FIELD_BYTES_2` 164.
        fn unit(guid: u64, player: bool, byte2: u8, extra: &[(u16, u32)]) -> ObjectStore {
            let mut pairs = vec![
                (0, guid as u32),
                (1, (guid >> 32) as u32),
                (2, if player { 0x19 } else { 0x9 }),
                (35, 1),
                (46, UNIT_FLAG_PLAYER_CONTROLLED),
                (164, u32::from(byte2 | BYTE2_FFA_PVP) << 8),
            ];
            pairs.extend_from_slice(extra);
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&pairs))
        }

        /// `UNIT_FIELD_CREATEDBY` (14-15) naming `owner`.
        fn pet(guid: u64, owner: u64) -> ObjectStore {
            unit(
                guid,
                false,
                0,
                &[(14, owner as u32), (15, (owner >> 32) as u32)],
            )
        }

        /// A raid: [`MEMBER`] in our subgroup 0, [`RAIDER`] in subgroup 1.
        fn group() -> GroupState {
            let member = |guid, flags| GroupMemberEntry {
                name: String::new(),
                guid,
                status: 1,
                flags,
                roles: 0,
            };
            GroupState {
                in_group: true,
                group_type: 1,
                members: vec![member(MEMBER, 0), member(RAIDER, 1)],
                ..GroupState::default()
            }
        }

        /// Both reactions, both attack-check directions and the ring and name colour of `them`.
        struct Seen {
            toward_us: u8,
            from_us: u8,
            we_attack: bool,
            attacks_us: bool,
            ring: RingVariant,
        }

        fn seen(them: &ObjectStore, me: &ObjectStore, party: &GroupState) -> Seen {
            let f = factions();
            let reps = Reputations::default();
            Seen {
                toward_us: ring_reaction(Some(&f), &reps, party, Some(them), Some(me)),
                from_us: reaction_from_player(Some(&f), &reps, party, Some(them), Some(me)),
                we_attack: can_attack(Some(them), Some(&f), &reps, party, Some(me)),
                attacks_us: crate::target::ring::can_attack_from_player(
                    Some(&f),
                    &reps,
                    party,
                    Some(me),
                    Some(them),
                    true,
                ),
                ring: highlight_variant(them, Some(&f), &reps, party, Some(me), |g| g == FRIEND),
            }
        }

        const FRIENDLY: u8 = 4;
        const HOSTILE: u8 = 1;

        #[test]
        fn our_player_is_friendly_to_itself_and_never_red() {
            let me = unit(ME, true, 0, &[]);
            // The same store, and a second store carrying our guid: both are the same unit.
            for them in [&me, &unit(ME, true, 0, &[])] {
                let s = seen(them, &me, &group());
                assert_eq!((s.toward_us, s.from_us), (FRIENDLY, FRIENDLY));
                assert!(!s.we_attack && !s.attacks_us, "nobody attacks himself");
                assert_eq!(s.ring, RingVariant::Player, "the soft blue, not red");
                assert_eq!(
                    selection_color(
                        them,
                        Some(&factions()),
                        &Reputations::default(),
                        &group(),
                        Some(&me)
                    ),
                    SELECTION_BLUE,
                    "UnitSelectionColor(\"player\") agrees"
                );
            }
        }

        #[test]
        fn our_own_pet_shares_our_controller() {
            let me = unit(ME, true, 0, &[]);
            let s = seen(&pet(OUR_PET, ME), &me, &GroupState::default());
            assert_eq!((s.toward_us, s.from_us), (FRIENDLY, FRIENDLY));
            assert!(!s.we_attack && !s.attacks_us);
            assert_ne!(s.ring, RingVariant::Hostile);
        }

        #[test]
        fn a_party_member_and_their_pet_stay_friendly() {
            let me = unit(ME, true, 0, &[]);
            let s = seen(&unit(MEMBER, true, 0, &[]), &me, &group());
            assert_eq!((s.toward_us, s.from_us), (FRIENDLY, FRIENDLY));
            assert!(
                !s.we_attack && !s.attacks_us,
                "the party rung skips the FFA rung"
            );
            assert_eq!(s.ring, RingVariant::Party, "pale blue");

            let flagged = seen(&unit(MEMBER, true, BYTE2_PVP, &[]), &me, &group());
            assert_eq!(
                flagged.ring,
                RingVariant::PartyPvp,
                "pale green when PvP-flagged"
            );

            let s = seen(&pet(MEMBER_PET, MEMBER), &me, &group());
            assert_eq!((s.toward_us, s.from_us), (FRIENDLY, FRIENDLY));
            assert_eq!(
                s.ring,
                RingVariant::Party,
                "a member's pet takes the party colour"
            );
        }

        #[test]
        fn a_raid_member_outside_our_subgroup_is_friendly_but_not_party_coloured() {
            let me = unit(ME, true, 0, &[]);
            let s = seen(&unit(RAIDER, true, 0, &[]), &me, &group());
            assert_eq!(
                (s.toward_us, s.from_us),
                (FRIENDLY, FRIENDLY),
                "the raid roster counts"
            );
            assert!(!s.we_attack && !s.attacks_us);
            assert_eq!(
                s.ring,
                RingVariant::Player,
                "the party colour reads the party slots"
            );
        }

        #[test]
        fn a_stranger_is_hostile_and_red() {
            let me = unit(ME, true, 0, &[]);
            let s = seen(&unit(STRANGER, true, 0, &[]), &me, &group());
            assert_eq!((s.toward_us, s.from_us), (HOSTILE, HOSTILE));
            assert!(s.we_attack && s.attacks_us);
            assert_eq!(s.ring, RingVariant::Hostile);
            // The same stranger out of the FFA area is a friendly Human again.
            let calm = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (0, STRANGER as u32),
                (2, 0x19),
                (35, 1),
                (46, UNIT_FLAG_PLAYER_CONTROLLED),
            ]));
            assert_eq!(seen(&calm, &me, &group()).toward_us, FRIENDLY);
        }

        #[test]
        fn a_duel_outranks_the_party() {
            // `PLAYER_DUEL_ARBITER` 188-189 shared, `PLAYER_DUEL_TEAM` 196 opposed.
            let duel = |team: u32| [(188, 0xD0), (196, team)];
            let me = unit(ME, true, 0, &duel(1));
            let s = seen(&unit(MEMBER, true, 0, &duel(2)), &me, &group());
            assert_eq!((s.toward_us, s.from_us), (HOSTILE, HOSTILE));
            assert!(s.we_attack && s.attacks_us);
            assert_eq!(s.ring, RingVariant::Hostile);
        }

        #[test]
        fn a_friend_takes_the_friend_colour_only_when_unattackable() {
            let me = unit(ME, true, 0, &[]);
            assert_eq!(
                seen(&unit(FRIEND, true, 0, &[]), &me, &group()).ring,
                RingVariant::Hostile,
                "in FFA the friend is attackable both ways: red first"
            );
            let calm = |guid: u64| {
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (0, guid as u32),
                    (2, 0x19),
                    (35, 1),
                    (46, UNIT_FLAG_PLAYER_CONTROLLED),
                ]))
            };
            let calm_me = calm(ME);
            assert_eq!(
                seen(&calm(FRIEND), &calm_me, &group()).ring,
                RingVariant::Friend
            );
            assert_eq!(
                seen(&calm(STRANGER), &calm_me, &group()).ring,
                RingVariant::Player
            );
        }

        #[test]
        fn a_dead_npc_is_grey_and_a_dead_player_is_not() {
            let me = unit(ME, true, 0, &[]);
            let f = factions();
            let reps = Reputations::default();
            // Health 22 zero against max health 28.
            let dead_npc = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x9),
                (22, 0),
                (28, 100),
                (35, 1),
            ]));
            assert_eq!(
                highlight_variant(&dead_npc, Some(&f), &reps, &group(), Some(&me), |_| false),
                RingVariant::Dead
            );
            let dead_member = unit(MEMBER, true, 0, &[(22, 0), (28, 100)]);
            assert_eq!(seen(&dead_member, &me, &group()).ring, RingVariant::Party);
        }
    }

    /// `0x521bf0` with its third argument 0, over templates 1 (Human) and 2 (Orc) and three NPC
    /// rows: a beast no mask names (neutral), a monster enemy-masked against players (hostile)
    /// and a guard friend-masked toward the Alliance (friendly).
    #[test]
    fn the_selection_colour_follows_the_reference_resolver() {
        use benilla_formats::{FactionCatalog, FactionTemplate};
        use std::collections::HashMap;

        let tpl = |faction, group_mask, friend_group_mask, enemy_group_mask| FactionTemplate {
            faction,
            group_mask,
            friend_group_mask,
            enemy_group_mask,
            enemies: [0; 4],
            friends: [0; 4],
        };
        let factions = Factions::from_catalog(FactionCatalog::from_rows(
            HashMap::from([
                (1, tpl(1, 3, 2, 12)),
                (2, tpl(2, 5, 4, 10)),
                (7, tpl(7, 0, 0, 0)),
                (14, tpl(14, 8, 0, 1)),
                (11, tpl(11, 2, 2, 0)),
            ]),
            HashMap::new(),
        ));
        let reps = Reputations::default();
        // `OBJECT_FIELD_TYPE` 2, the template 35, `UNIT_FIELD_FLAGS` 46, `UNIT_FIELD_BYTES_2` 164.
        let unit = |template: u32, flags: u32, byte2: u8, player: bool| {
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, if player { 0x19 } else { 0x9 }),
                (35, template),
                (46, flags),
                (164, u32::from(byte2) << 8),
            ]))
        };
        let me = unit(1, UNIT_FLAG_PLAYER_CONTROLLED, 0, true);
        let color = |store: &ObjectStore| {
            selection_color(store, Some(&factions), &reps, &NoGroup, Some(&me))
        };

        // NPCs: the reaction palette.
        assert_eq!(
            color(&unit(7, 0, 0, false)),
            0xFFFF_FF00,
            "a neutral beast is yellow"
        );
        assert_eq!(
            color(&unit(14, 0, 0, false)),
            0xFFFF_0000,
            "a hostile monster is red"
        );
        assert_eq!(
            color(&unit(11, 0, 0, false)),
            0xFF00_FF00,
            "a friendly guard is green"
        );

        // Players: attackability both ways, then the PvP byte.
        let pc = UNIT_FLAG_PLAYER_CONTROLLED;
        assert_eq!(
            color(&unit(1, pc, 0, true)),
            0xFF00_00FF,
            "an unflagged ally is blue"
        );
        assert_eq!(
            color(&unit(1, pc, BYTE2_PVP, true)),
            0xFF00_FF00,
            "a PvP-flagged ally takes its reaction, friendly"
        );
        assert_eq!(
            color(&unit(1, pc, BYTE2_PVP | BYTE2_SANCTUARY, true)),
            0xFF00_00FF,
            "in a sanctuary the flag does not count"
        );
        assert_eq!(
            color(&unit(2, pc, 0, true)),
            0xFF00_00FF,
            "nobody can attack: blue"
        );
        assert_eq!(
            color(&unit(2, pc, BYTE2_PVP, true)),
            0xFFFF_FF00,
            "a flagged enemy we can attack and who cannot attack us is yellow"
        );
        let flagged_me = unit(1, pc, BYTE2_PVP, true);
        let enemy = unit(2, pc, BYTE2_PVP, true);
        assert_eq!(
            selection_color(&enemy, Some(&factions), &reps, &NoGroup, Some(&flagged_me)),
            0xFFFF_0000,
            "both flagged: red"
        );
        assert_eq!(
            selection_color(
                &unit(2, pc, 0, true),
                Some(&factions),
                &reps,
                &NoGroup,
                Some(&flagged_me)
            ),
            0xFF00_00FF,
            "it can attack us and we not it: blue"
        );

        // The player's own colour: reaction 4 toward itself when flagged, else blue.
        assert_eq!(color(&me), 0xFF00_00FF);
        let flagged = unit(1, pc, BYTE2_PVP, true);
        assert_eq!(
            selection_color(&flagged, Some(&factions), &reps, &NoGroup, Some(&flagged)),
            0xFF00_FF00
        );
        // No player: the resolver's blue.
        assert_eq!(
            selection_color(
                &unit(7, 0, 0, false),
                Some(&factions),
                &reps,
                &NoGroup,
                None
            ),
            0xFF00_00FF
        );
    }

    /// The attack check (`0x729740`) between players reads the PvP byte of `UNIT_FIELD_BYTES_2`: a
    /// flagged enemy is attackable, as in a battleground, an unflagged one is not, sanctuary on
    /// either side refuses, both free-for-all passes, and a flagged ally stays friendly.
    #[test]
    fn can_attack_reads_the_pvp_byte_between_players() {
        use benilla_formats::{FactionCatalog, FactionTemplate};
        use std::collections::HashMap;

        let tpl = |faction, group_mask, friend_group_mask, enemy_group_mask| FactionTemplate {
            faction,
            group_mask,
            friend_group_mask,
            enemy_group_mask,
            enemies: [0; 4],
            friends: [0; 4],
        };
        let factions = Factions::from_catalog(FactionCatalog::from_rows(
            HashMap::from([(1, tpl(1, 3, 2, 12)), (2, tpl(2, 5, 4, 10))]),
            HashMap::new(),
        ));
        let reps = Reputations::default();
        let pc = UNIT_FLAG_PLAYER_CONTROLLED;
        let unit = |template: u32, byte2: u8, player_flags: u32| {
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x19),
                (35, template),
                (46, pc),
                (164, u32::from(byte2) << 8),
                (190, player_flags),
            ]))
        };
        let attack = |target: &ObjectStore, me: &ObjectStore| {
            can_attack(Some(target), Some(&factions), &reps, &NoGroup, Some(me))
        };
        let me = unit(1, BYTE2_PVP, 0);

        assert!(attack(&unit(2, BYTE2_PVP, 0), &me), "a flagged enemy");
        assert!(
            attack(&unit(2, BYTE2_PVP, 0), &unit(1, 0, 0)),
            "the target's flag alone decides"
        );
        assert!(!attack(&unit(2, 0, 0), &me), "an unflagged enemy");
        assert!(
            !attack(&unit(2, BYTE2_PVP | BYTE2_SANCTUARY, 0), &me),
            "a flagged enemy in a sanctuary"
        );
        assert!(
            !attack(
                &unit(2, BYTE2_PVP, 0),
                &unit(1, BYTE2_PVP | BYTE2_SANCTUARY, 0)
            ),
            "from a sanctuary"
        );
        assert!(!attack(&unit(1, BYTE2_PVP, 0), &me), "a flagged ally");
        assert!(
            attack(&unit(1, 0x4, 0), &unit(1, 0x4, 0)),
            "both free-for-all: an ally too"
        );
        assert!(
            !attack(&unit(2, BYTE2_PVP, 0), &unit(1, BYTE2_PVP, 0x8_0000)),
            "PLAYER_FLAGS 0x80000 refuses the actor"
        );
        assert!(!attack(&unit(2, BYTE2_PVP, 0x10), &me), "a ghost");
    }

    /// The assist check (`0x7293d0`) both ways round. A spirit guide may assist the ghost
    /// (`0x523f80` asks it so) though the player may not assist the guide, which carries neither
    /// the PvP bit nor an aiding template. Between players the FFA, sanctuary and duel legs decide.
    #[test]
    fn can_assist_follows_the_actor_and_its_target() {
        use benilla_formats::{FactionCatalog, FactionTemplate};
        use std::collections::HashMap;

        let tpl = |faction, group_mask, friend_group_mask, enemy_group_mask| FactionTemplate {
            faction,
            group_mask,
            friend_group_mask,
            enemy_group_mask,
            enemies: [0; 4],
            friends: [0; 4],
        };
        let factions = Factions::from_catalog(FactionCatalog::from_rows(
            HashMap::from([
                (1, tpl(1, 3, 2, 12)),
                (2, tpl(2, 5, 4, 10)),
                (11, tpl(11, 2, 2, 0)),
                (14, tpl(14, 8, 0, 1)),
            ]),
            HashMap::new(),
        ));
        let reps = Reputations::default();
        const GUIDE: u32 = 13116;
        const AIDER: u32 = 13117;
        let mut names = crate::names::NameCache::default();
        names.insert_creature(GUIDE, Some(crate::names::CreatureRecord::default()));
        names.insert_creature(
            AIDER,
            Some(crate::names::CreatureRecord {
                type_flags: TYPE_FLAG_AIDS_PLAYERS,
                ..Default::default()
            }),
        );
        // `OBJECT_FIELD_TYPE` 2, `OBJECT_FIELD_ENTRY` 3, the template 35, `UNIT_FIELD_FLAGS` 46,
        // `UNIT_FIELD_BYTES_2` 164, `PLAYER_DUEL_ARBITER` 188, `PLAYER_DUEL_TEAM` 196.
        let npc = |entry: u32, template: u32, flags: u32, byte2: u8| {
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x9),
                (3, entry),
                (35, template),
                (46, flags),
                (164, u32::from(byte2) << 8),
            ]))
        };
        let player = |template: u32, byte2: u8, duel: (u32, u32)| {
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x19),
                (35, template),
                (46, UNIT_FLAG_PLAYER_CONTROLLED),
                (164, u32::from(byte2) << 8),
                (188, duel.0),
                (196, duel.1),
            ]))
        };
        let me = player(1, 0, (0, 0));
        let ask = |dir, unit: &ObjectStore, me: &ObjectStore| {
            can_assist_dir(
                dir,
                Some(unit),
                Some(&factions),
                &reps,
                &NoGroup,
                Some(me),
                Some(&names),
            )
        };

        let guide = npc(GUIDE, 11, 0, 0);
        assert!(
            ask(Assist::UnitToPlayer, &guide, &me),
            "the guide may assist the ghost: an uncontrolled actor passes"
        );
        assert!(
            !ask(Assist::PlayerToUnit, &guide, &me),
            "the player may not assist an unflagged NPC with no aiding template"
        );
        assert!(
            ask(Assist::PlayerToUnit, &npc(AIDER, 11, 0, 0), &me),
            "type flag 0x1000, the aids-players test"
        );
        assert!(
            ask(Assist::PlayerToUnit, &npc(GUIDE, 11, 0, BYTE2_PVP), &me),
            "or the NPC's PvP bit"
        );
        assert!(
            !ask(Assist::UnitToPlayer, &npc(GUIDE, 14, 0, 0), &me),
            "a hostile NPC assists nobody"
        );
        assert!(
            !ask(
                Assist::PlayerToUnit,
                &npc(AIDER, 11, UNIT_FLAG_NOT_SELECTABLE, 0),
                &me
            ),
            "a target that is not selectable refuses"
        );
        assert!(
            ask(
                Assist::UnitToPlayer,
                &npc(GUIDE, 11, UNIT_FLAG_NOT_SELECTABLE, 0),
                &me
            ),
            "the flag is read on the target only: a guide that is not selectable still assists"
        );

        let ally = |byte2, duel| player(1, byte2, duel);
        assert!(ask(Assist::PlayerToUnit, &ally(0, (0, 0)), &me), "an ally");
        assert!(
            !ask(Assist::PlayerToUnit, &ally(BYTE2_FFA_PVP, (0, 0)), &me),
            "an FFA ally while we are not"
        );
        assert!(
            !ask(
                Assist::PlayerToUnit,
                &ally(BYTE2_FFA_PVP, (0, 0)),
                &player(1, BYTE2_FFA_PVP, (0, 0))
            ),
            "both FFA: hostile to each other (`0x7253af`), so the reaction leg refuses"
        );
        assert!(
            !ask(
                Assist::PlayerToUnit,
                &ally(BYTE2_PVP, (0, 0)),
                &player(1, BYTE2_SANCTUARY, (0, 0))
            ),
            "from a sanctuary toward a flagged ally outside one"
        );
        assert!(
            ask(
                Assist::PlayerToUnit,
                &ally(BYTE2_PVP | BYTE2_SANCTUARY, (0, 0)),
                &player(1, BYTE2_SANCTUARY, (0, 0))
            ),
            "both in the sanctuary"
        );
        assert!(
            !ask(Assist::PlayerToUnit, &ally(0, (0x77, 1)), &me),
            "an ally duelling someone else"
        );
        assert!(
            !ask(
                Assist::PlayerToUnit,
                &ally(0, (0x77, 2)),
                &player(1, 0, (0x77, 1))
            ),
            "our duel opponent"
        );
        assert!(
            !ask(Assist::PlayerToUnit, &player(2, 0, (0, 0)), &me),
            "an enemy"
        );
    }

    /// The caster's template toward the corpse's race's, friendly or better; every gap is neutral
    /// and fails.
    #[test]
    fn the_corpse_gate_reads_the_race_template() {
        let factions = fx::factions();
        let me = fx::caster(1.5);
        let gate = |corpse: &ObjectStore| corpse_friendly(corpse, Some(&factions), Some(&me));
        assert!(
            gate(&fx::corpse(fx::HUMAN, false)),
            "a human toward a human"
        );
        assert!(!gate(&fx::corpse(fx::ORC, false)), "a human toward an orc");
        assert!(!gate(&fx::corpse(0, false)), "race 0 has no ChrRaces row");
        assert!(
            !gate(&ObjectStore(Default::default())),
            "no BYTES_1 reads race 0"
        );
        let human = fx::corpse(fx::HUMAN, false);
        assert!(!corpse_friendly(&human, None, Some(&me)), "no catalog");
        assert!(!corpse_friendly(&human, Some(&factions), None), "no player");
        let no_template = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(35, 999)]));
        assert!(
            !corpse_friendly(&human, Some(&factions), Some(&no_template)),
            "a template the table lacks"
        );
    }

    /// The same gate on the shipped `ChrRaces.dbc` and `FactionTemplate.dbc`: a human (template 1)
    /// stands friendly toward every Alliance race's corpse and not toward a Horde one's.
    #[test]
    fn the_corpse_gate_splits_the_sides_on_the_shipped_tables() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let factions = Factions::from_catalog(
            benilla_formats::load_faction_catalog(&mut chain).expect("faction tables"),
        );
        let me = fx::caster(1.5);
        let gate = |race| corpse_friendly(&fx::corpse(race, false), Some(&factions), Some(&me));
        for race in [1, 3, 4, 7] {
            assert!(gate(race), "Alliance race {race}");
        }
        for race in [2, 5, 6, 8] {
            assert!(!gate(race), "Horde race {race}");
        }
        assert!(!gate(0) && !gate(10), "a race with no row");
    }

    /// The flag, then the charmer if there is one, else the creator; an unset owner or an unknown
    /// active player is no possession.
    #[test]
    fn possession_is_the_flag_and_the_charmer_else_the_creator() {
        let store = |flags: u32, charmed_by: u32, created_by: u32| {
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (46, flags),
                (10, charmed_by),
                (11, 0),
                (14, created_by),
                (15, 0),
            ]))
        };
        let me = Some(0x77);
        let flag = UNIT_FLAG_POSSESSED;
        assert!(
            is_possessed_by(Some(&store(flag, 0x77, 0)), me),
            "charmed by us"
        );
        assert!(
            is_possessed_by(Some(&store(flag, 0, 0x77)), me),
            "created by us"
        );
        assert!(
            !is_possessed_by(Some(&store(flag, 0x99, 0x77)), me),
            "a charmer other than us comes before our creation"
        );
        assert!(!is_possessed_by(Some(&store(0, 0x77, 0x77)), me), "no flag");
        assert!(!is_possessed_by(Some(&store(flag, 0, 0)), me), "no owner");
        assert!(
            !is_possessed_by(Some(&store(flag, 0, 0)), None),
            "no active player"
        );
        assert!(!is_possessed_by(None, me), "no descriptor");
    }
}
