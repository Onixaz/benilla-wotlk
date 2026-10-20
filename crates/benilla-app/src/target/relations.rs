//! The three unit-relationship predicates the reference shares across systems, may I attack,
//! interact with or help this unit, as the store-only entry points every consumer calls:
//!
//! - `CanAttack 0x606980` forwards to [`super::ring::can_attack_from_player`],
//! - `CanInteract 0x6067f0` forwards to [`super::ring::can_interact_from_player`],
//! - `CanAssist 0x7293d0` is derived here, in either direction ([`Assist`]).
//!
//! The first two live in [`super::ring`] beside the reaction directions they turn on:
//! `0x6061e0(this = player)` answers a reputation faction with the at-war bit,
//! `0x6061e0(this = unit)` with the standing, and the two often disagree.
//!
//! A corpse has its own reaction gate, `0x6067d0` ([`corpse_friendly`]).

use benilla_protocol::messages::{ObjectType, OwnerFallback};

use crate::net::{ObjectStore, Reputations};

use super::{ring_reaction, Factions};

/// `OBJECT_FIELD_TYPE` bit 4, the reference's own player test (`0x606984` in `CanAttack`,
/// `0x6067fc` in `CanInteract`), read off the store the rest of the predicate walks.
///
/// Relies on the create block carrying field 2: vmangos sends every non-zero field, and a player's
/// is `0x19`. A miss only skips `CanAttack`'s ghost refusal, which the both-player-controlled arm
/// makes anyway; `/reaction` prints the field beside the `NetEntity` kind.
fn is_player_object(store: Option<&ObjectStore>) -> bool {
    store.and_then(|s| s.0.object_type()) == Some(ObjectType::Player)
}

/// `CanAttack 0x606980`, the one attackability question of the world cursor's sword
/// (`0x48269a`), the combat flash, the TAB scan's filter 3, `UnitCanAttack` and hostile spell
/// targeting. Its mixed arm is `UnitReaction(player → target) < 4`, which answers a reputation
/// faction with the at-war bit, never the standing, so a not-at-war neutral faction (Cenarion
/// Circle) is not attackable.
pub(crate) fn can_attack(
    store: Option<&ObjectStore>,
    factions: Option<&Factions>,
    reputations: &Reputations,
    self_store: Option<&ObjectStore>,
) -> bool {
    super::ring::can_attack_from_player(
        factions,
        reputations,
        store,
        self_store,
        is_player_object(store),
    )
}

/// `CanInteract 0x6067f0`, may I take a service from this unit: the world-cursor classifier runs it
/// at `0x482310` (through `CanInteractNow 0x606880`) to choose between the NPC service ladder and
/// the loot, skin and attack block.
pub(crate) fn can_interact(
    store: Option<&ObjectStore>,
    factions: Option<&Factions>,
    reputations: &Reputations,
    self_store: Option<&ObjectStore>,
) -> bool {
    super::ring::can_interact_from_player(factions, reputations, store, self_store)
}

/// `UNIT_FLAG_NOT_SELECTABLE`, bit 25 (vmangos `UnitDefines.h:570`): `CanAssist`'s first refusal
/// and one of `CanAttack`'s.
const UNIT_FLAG_NOT_SELECTABLE: u32 = 1 << 25;

/// `IsSelectable`, CGUnit_C's vtable slot 21 (`0x60be60`): `UNIT_FLAG_NOT_SELECTABLE` clear, or the
/// unit's `UNIT_FIELD_CREATEDBY` is the active player, so your own flagged totems and traps stay
/// selectable. Every vtable but CGUnit_C's and CGPlayer_C's has the `xor eax,eax` stub
/// (`0x469fe0`) there, so a GameObject, item or corpse never becomes the selection.
///
/// `None` passes: `SetSelection 0x493540` makes the call (`0x4935ee`) only on an object the
/// manager resolved (`0x4935c8`), which keeps an out-of-range party member selectable. A store
/// whose `OBJECT_FIELD_TYPE` has not streamed passes the same way.
pub(crate) fn is_selectable(store: Option<&ObjectStore>, self_guid: Option<u64>) -> bool {
    let Some(store) = store else {
        return true; // no resolved object: the reference skips the call (`0x4935c8`)
    };
    // Only CGUnit_C and CGPlayer_C override slot 21.
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
/// active player's guid. The own-pet resolver `0x5ee5a0` (`0x5ee626`, `0x5ee62f`) and the world
/// pick's filter `0x480610` (`0x48064f`-`0x480674`) read this same clause. No store, no owner or
/// no active player is no possession.
pub(crate) fn is_possessed_by(store: Option<&ObjectStore>, self_guid: Option<u64>) -> bool {
    let (Some(store), Some(me)) = (store, self_guid) else {
        return false;
    };
    store.0.unit_flags() & UNIT_FLAG_POSSESSED != 0
        && store.0.unit_owner(OwnerFallback::CreatedBy) == Some(me)
}

/// `0x6067d0(player, corpse)`, the corpse's reaction gate: `0x6064c0` compares the player's
/// template (`UNIT_FIELD_FACTIONTEMPLATE`) toward the dead player's race's (`0x5d7120`: the race
/// byte of `CORPSE_FIELD_BYTES_1`, then `ChrRaces.dbc` column 2) with the template comparator
/// `0x606640` alone, no reputation, and passes at friendly (`setge` 4). A missing row on either
/// side is neutral (`0x60651b`), so it fails, as does a missing catalog or player.
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
/// The cross-flag immunity bits `CanAssist` reads on its target (`0x7293d0`), as `CanAttack` does.
const IMMUNE_TO_PLAYER_CONTROLLED: u32 = 0x100;
const IMMUNE_TO_UNCONTROLLED: u32 = 0x200;
/// Creature template type flags: `CanAidPlayers` (`0x715f90`) and `IsTreatAsRaidUnit`
/// (`0x715df0`).
const TYPE_FLAG_AIDS_PLAYERS: u32 = 1 << 12;
const TYPE_FLAG_TREAT_AS_RAID_UNIT: u32 = 1 << 26;
/// `UNIT_FIELD_BYTES_2` byte 1's free-for-all bit, beside [`BYTE2_PVP`] and [`BYTE2_SANCTUARY`].
const BYTE2_FFA_PVP: u8 = 0x4;

/// The side the local player takes in a `CanAssist` question.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Assist {
    /// May the player assist the unit: every spell, `UnitCanAssist` and targeting caller.
    PlayerToUnit,
    /// May the unit assist the player: the area spirit healer walk (`0x523f80`).
    UnitToPlayer,
}

/// `CanAssist 0x7293d0` from the player, toward `store` ([`Assist::PlayerToUnit`]). `names`
/// resolves the creature template type flags; a unit not in the cache reads none.
pub(crate) fn can_assist(
    store: Option<&ObjectStore>,
    factions: Option<&Factions>,
    reputations: &Reputations,
    self_store: Option<&ObjectStore>,
    names: Option<&crate::names::NameCache>,
) -> bool {
    can_assist_dir(
        Assist::PlayerToUnit,
        store,
        factions,
        reputations,
        self_store,
        names,
    )
}

/// `CanAssist 0x7293d0` with `ignoreFlags` 0, between the player and `store` in `dir`:
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
            super::ring::reaction_from_player(factions, reputations, Some(unit), Some(me)),
        ),
        Assist::UnitToPlayer => (
            unit,
            me,
            unit_type_flags,
            0,
            ring_reaction(factions, reputations, Some(unit), Some(me)),
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

/// `UnitSelectionColor`'s ARGB dword, 12340's resolver `0x521bf0` with its third argument 0, as
/// `0x60da6f` calls it: a unit that is not player-controlled takes the palette entry of its
/// reaction toward the player (`0x521fa7`, no dead grey in this mode). A player-controlled one
/// asks `CanAttack 0x729740` both ways: it can attack us and we it, red; it us only, blue; we it
/// only, yellow (`0x521e45`-`0x521e9e`). Else a PvP-flagged unit, neither side in a sanctuary,
/// takes its reaction's entry, and any other blue. `self_store` is the player's own descriptor;
/// passing it as `store` reads the player's own colour, whose reaction toward itself is 4
/// (`0x7251d5`). Not applied: the active mover's vehicle or charm (`0x6dd060`, `0x5140a0`) and
/// the zone's sanctuary test (`[0xbea570] == 4`).
pub(crate) fn selection_color(
    store: &ObjectStore,
    factions: Option<&Factions>,
    reputations: &Reputations,
    self_store: Option<&ObjectStore>,
) -> u32 {
    let Some(own) = self_store else {
        return SELECTION_BLUE;
    };
    let reaction_color = || {
        let r = if std::ptr::eq(store, own) {
            4
        } else {
            ring_reaction(factions, reputations, Some(store), Some(own))
        };
        SELECTION_REACTION[usize::from(r.min(7))]
    };
    if store.0.unit_flags() & UNIT_FLAG_PLAYER_CONTROLLED == 0 {
        return reaction_color();
    }
    // `CanAttack(unit, player)`: the arm both bit-3 flags select is the same either way round,
    // so the player-side predicate with the two swapped asks it.
    let attacks_us = super::ring::can_attack_from_player(
        factions,
        reputations,
        Some(own),
        Some(store),
        is_player_object(Some(own)),
    );
    let we_attack = can_attack(Some(store), factions, reputations, Some(own));
    match (attacks_us, we_attack) {
        (true, true) => SELECTION_REACTION[1],
        (true, false) => SELECTION_BLUE,
        (false, true) => SELECTION_REACTION[3],
        (false, false) => {
            let (theirs, ours) = (store.0.unit_byte2_flags(), own.0.unit_byte2_flags());
            if theirs & BYTE2_PVP != 0
                && theirs & BYTE2_SANCTUARY == 0
                && ours & BYTE2_SANCTUARY == 0
            {
                reaction_color()
            } else {
                SELECTION_BLUE
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spell::targeting::corpse_fixture as fx;

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
        let color = |store: &ObjectStore| selection_color(store, Some(&factions), &reps, Some(&me));

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
            selection_color(&enemy, Some(&factions), &reps, Some(&flagged_me)),
            0xFFFF_0000,
            "both flagged: red"
        );
        assert_eq!(
            selection_color(
                &unit(2, pc, 0, true),
                Some(&factions),
                &reps,
                Some(&flagged_me)
            ),
            0xFF00_00FF,
            "it can attack us and we not it: blue"
        );

        // The player's own colour: reaction 4 toward itself when flagged, else blue.
        assert_eq!(color(&me), 0xFF00_00FF);
        let flagged = unit(1, pc, BYTE2_PVP, true);
        assert_eq!(
            selection_color(&flagged, Some(&factions), &reps, Some(&flagged)),
            0xFF00_FF00
        );
        // No player: the resolver's blue.
        assert_eq!(
            selection_color(&unit(7, 0, 0, false), Some(&factions), &reps, None),
            0xFF00_00FF
        );
    }

    /// `CanAttack 0x729740` between players reads the PvP byte of `UNIT_FIELD_BYTES_2`: a flagged
    /// enemy is attackable, as in a battleground, an unflagged one is not, sanctuary on either
    /// side refuses, both free-for-all passes, and a flagged ally stays friendly.
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
            can_attack(Some(target), Some(&factions), &reps, Some(me))
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

    /// `CanAssist 0x7293d0` both ways round. A spirit guide may assist the ghost
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
            "type flag 0x1000, CanAidPlayers"
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

    /// `0x6067d0`: the caster's template toward the corpse's race's, friendly or better; every gap
    /// is neutral (`0x60651b`) and fails.
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
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
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

    /// `0x480658`-`0x480667`: the flag, then the charmer if there is one, else the creator; an
    /// unset owner or an unknown active player is no possession.
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
