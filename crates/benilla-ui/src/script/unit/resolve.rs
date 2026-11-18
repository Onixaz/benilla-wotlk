//! The unit-token resolver, token to guid, over the state it reads, which the app pushes as
//! [`UnitGuids`] (`0x60abf0`). Every compare folds ASCII case. A token is a base, then hops: the
//! bases are 12340's, in its compare order: `player`, `vehicle`, `pet`, `target`, `partypetN`,
//! `partyN`, `raidpetN`, `raidN`, `bossN`, `arenapetN`, `arenaN`, `commentatorN`, `mouseover`,
//! `focus`, then the exact `npc`, `questnpc` and `none`. Each hop after it (`0x60aaa0`), behind an
//! optional `-`, is `target` or `pet`. A token whose base names nobody with text after it, whose
//! text is not hops, or that no base matches goes to the name lookup (`0x60a630`): a group member's
//! or pet's name, then hops.

use std::collections::HashMap;

use super::super::live::read_live;

/// What the resolver reads: the guid globals, the group tables and, for each unit a token can
/// reach, whether it is held and whom it targets. A zero guid is nobody.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitGuids {
    /// The local player's guid; 0 without the player object, when `player` and `pet` answer nil.
    pub player: u64,
    /// The pet: `UNIT_FIELD_CHARM`, else `UNIT_FIELD_SUMMON`.
    pub pet: u64,
    /// The vehicle the player sits in, its movement transport while seated (`0x60ac87`), else 0.
    pub vehicle: u64,
    /// The target global.
    pub target: u64,
    /// The mouseover global, read only while it is a held unit.
    pub mouseover: u64,
    /// The interaction NPC.
    pub npc: u64,
    /// The focus `[0xbd07d0]`, which `FocusUnit` sets in the VM; a push keeps the VM's.
    pub focus: u64,
    /// `partyN`'s guid, index N-1.
    pub party: [u64; 4],
    /// `partypetN`'s guid, index N-1.
    pub party_pets: [u64; 4],
    /// `raidN`'s guid, one per roster row; a row past the end is nobody.
    pub raid: Vec<u64>,
    /// `raidpetN`'s guid, row for row with [`Self::raid`].
    pub raid_pets: Vec<u64>,
    /// `bossN`'s guid, the encounter's slot N-1 (`0x5ed710`); a slot past the end is nobody.
    pub boss: Vec<u64>,
    /// `arenaN`'s guid, opponent slot N-1 (`0x608180`), whatever the count.
    pub arena: [u64; 5],
    /// `arenapetN`'s guid (`0x60a2f0`): the held opponent's pet, else the slot's pet entry.
    pub arena_pets: [u64; 5],
    /// `GetNumArenaOpponents` (`0x549630`), the leading slots holding an opponent or a pet; the
    /// fan-out walks only these.
    pub arena_count: usize,
    /// Each held unit a token can reach, to its `UNIT_FIELD_TARGET` (0 for none): the verbs' unit
    /// lookup (`0x60c1f0`, type mask 8 at `0x60c21e`) and the descriptor read a `target` hop
    /// makes. A guid missing here is not held.
    pub held: HashMap<u64, u64>,
}

/// A token's base, the resolver's compares in its order (`partypet` before `party`, `raidpet`
/// before `raid`, `arenapet` before `arena`); `npc`, `questnpc` and `none` are tested apart, as
/// full-string compares.
#[derive(Clone, Copy)]
enum Base {
    Player,
    Vehicle,
    Pet,
    Target,
    PartyPet,
    Party,
    RaidPet,
    Raid,
    Boss,
    ArenaPet,
    Arena,
    Commentator,
    Mouseover,
    Focus,
}

/// Each prefix and its base, in the resolver's order (`0x60ac2a`-`0x60af2a`); a prefix test
/// compares `strlen(literal)` bytes, so `"playerfoo"` matches `player`.
const PREFIXES: [(&str, Base); 14] = [
    ("player", Base::Player),
    ("vehicle", Base::Vehicle),
    ("pet", Base::Pet),
    ("target", Base::Target),
    ("partypet", Base::PartyPet),
    ("party", Base::Party),
    ("raidpet", Base::RaidPet),
    ("raid", Base::Raid),
    ("boss", Base::Boss),
    ("arenapet", Base::ArenaPet),
    ("arena", Base::Arena),
    ("commentator", Base::Commentator),
    ("mouseover", Base::Mouseover),
    ("focus", Base::Focus),
];

/// The unit a token starts from, before any hop. A numbered base carries its 0-based row,
/// [`index_of`]'s: a table lookup that fails on the reserved `u32::MAX` of a missing or zero
/// number names nobody.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitBase {
    Player,
    Pet,
    Target,
    Mouseover,
    /// The interaction NPC; an exact compare, so it takes no hop (`0x60af58`).
    Npc,
    Party(u32),
    PartyPet(u32),
    Raid(u32),
    RaidPet(u32),
    /// The unit whose vehicle we ride (`0x60ac87`).
    Vehicle,
    /// The focus global `[0xbd07d0]`.
    Focus,
    /// The quest giver (`0x6081b0`); an exact compare.
    QuestNpc,
    /// An encounter unit, `0x5ed710(row)`.
    Boss(u32),
    /// An arena opponent, `0x608180(row)`.
    Arena(u32),
    /// An arena opponent's pet, `0x60a2f0(row)`.
    ArenaPet(u32),
    /// A spectated player, `0x56b1e0(row)`.
    Commentator(u32),
}

/// One hop after a base or a name (`0x60aaa0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hop {
    /// To the unit's `UNIT_FIELD_TARGET`; off the player, to the target global.
    Target,
    /// To the unit's pet, `UNIT_FIELD_CHARM`, else `UNIT_FIELD_SUMMON`: taken off the player or a
    /// party or raid member, or off any unit while the player's `PLAYER_FLAGS` holds
    /// [`PET_OF_ANYONE`]; off another unit the hops fail.
    Pet,
}

/// The `PLAYER_FLAGS` bit that opens the `pet` hop to any unit (`0x60ab70`).
pub const PET_OF_ANYONE: u32 = 0x8_0000;

/// The text after a token's base, read hop by hop as `0x60aaa0` reads it: an optional `-`, then
/// `target` or `pet`. Text that is neither reads as one `Err`, which ends the hops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hops<'a>(&'a [u8]);

impl Hops<'_> {
    /// No text after the base.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Iterator for Hops<'_> {
    type Item = Result<Hop, ()>;

    fn next(&mut self) -> Option<Self::Item> {
        let text = self.0;
        let after_dash = text.strip_prefix(b"-").unwrap_or(text);
        let (hop, len) = if text.is_empty() {
            return None;
        } else if has_prefix(after_dash, "target") {
            (Hop::Target, 6)
        } else if has_prefix(after_dash, "pet") {
            (Hop::Pet, 3)
        } else {
            self.0 = &[];
            return Some(Err(()));
        };
        self.0 = &after_dash[len..];
        Some(Ok(hop))
    }
}

/// What the reference's unit-token resolver makes of a token's text, before it reads any state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitTokenParse<'a> {
    /// The empty token, which the resolver refuses (`0x60ac1d`) unless its caller defaults it to
    /// the target.
    Empty,
    /// None of the compares match: the whole token goes to the name lookup (`0x60a630`).
    Name,
    /// `none`, the `-1` guid (`0x60afc9`), which names no object.
    Nobody,
    /// A base and the text after it.
    Unit { base: UnitBase, hops: Hops<'a> },
}

/// `_strnicmp(s, lit, strlen(lit)) == 0`: ASCII fold only, bytes of 0x80 and up exact.
fn has_prefix(s: &[u8], lit: &str) -> bool {
    s.len() >= lit.len() && s[..lit.len()].eq_ignore_ascii_case(lit.as_bytes())
}

/// The first prefix the token matches, with its length.
fn base_of(token: &[u8]) -> Option<(Base, usize)> {
    PREFIXES
        .iter()
        .find(|(p, _)| has_prefix(token, p))
        .map(|&(p, b)| (b, p.len()))
}

/// The inline base-10 parse after a numbered base, then `dec` to a 0-based index: digits accumulate
/// with 32-bit wrap, and a missing number leaves 0, whose index `0xffffffff` every bound rejects.
fn index_of(s: &[u8]) -> (u32, usize) {
    let digits = s.iter().take_while(|b| b.is_ascii_digit()).count();
    let n = s[..digits].iter().fold(0u32, |acc, &b| {
        acc.wrapping_mul(10).wrapping_add(u32::from(b - b'0'))
    });
    (n.wrapping_sub(1), digits)
}

/// Read `token`'s text as the reference's unit-token resolver does, before any state: the base by
/// the compares in their order, a numbered base's row, then the text left for the hops.
/// `"party1target"` is party1 and one hop, `"targettarget"` the target and one, and `"npctarget"`
/// is none of the compares, as `npc` is one full-string compare.
pub fn parse_unit_token(token: &str) -> UnitTokenParse<'_> {
    let bytes = token.as_bytes();
    if bytes.is_empty() {
        return UnitTokenParse::Empty;
    }
    let Some((base, len)) = base_of(bytes) else {
        let exact = |base| UnitTokenParse::Unit {
            base,
            hops: Hops(&[]),
        };
        return if token.eq_ignore_ascii_case("npc") {
            exact(UnitBase::Npc)
        } else if token.eq_ignore_ascii_case("questnpc") {
            exact(UnitBase::QuestNpc)
        } else if token.eq_ignore_ascii_case("none") {
            UnitTokenParse::Nobody
        } else {
            UnitTokenParse::Name
        };
    };
    let mut rest = &bytes[len..];
    let mut numbered = |make: fn(u32) -> UnitBase| {
        let (row, digits) = index_of(rest);
        rest = &rest[digits..];
        make(row)
    };
    let base = match base {
        Base::Player => UnitBase::Player,
        Base::Vehicle => UnitBase::Vehicle,
        Base::Pet => UnitBase::Pet,
        Base::Target => UnitBase::Target,
        Base::Mouseover => UnitBase::Mouseover,
        Base::Focus => UnitBase::Focus,
        Base::Boss => numbered(UnitBase::Boss),
        Base::ArenaPet => numbered(UnitBase::ArenaPet),
        Base::Arena => numbered(UnitBase::Arena),
        Base::Commentator => numbered(UnitBase::Commentator),
        Base::PartyPet => numbered(UnitBase::PartyPet),
        Base::Party => numbered(UnitBase::Party),
        Base::RaidPet => numbered(UnitBase::RaidPet),
        Base::Raid => numbered(UnitBase::Raid),
    };
    UnitTokenParse::Unit {
        base,
        hops: Hops(rest),
    }
}

/// The name lookup's buffer (`0x60a630`): up to 48 bytes of the token, stopping where the rest is
/// `-target` or `-pet`, whole, or begins `-target-` or `-pet-`. Any other `-` it copies asks for
/// names with their realm. Returns the name, whether it carries a realm, and the hops after it.
pub fn split_name(token: &str) -> (&[u8], bool, Hops<'_>) {
    let bytes = token.as_bytes();
    let mut with_realm = false;
    let mut len = 0;
    while len < bytes.len() && len < 48 {
        let rest = &bytes[len..];
        if rest[0] == b'-' {
            let whole = |lit: &str| rest.eq_ignore_ascii_case(lit.as_bytes());
            if whole("-target")
                || whole("-pet")
                || has_prefix(rest, "-target-")
                || has_prefix(rest, "-pet-")
            {
                break;
            }
            with_realm = true;
        }
        len += 1;
    }
    (&bytes[..len], with_realm, Hops(&bytes[len..]))
}

/// The name compare (`SStrCmpUTF8I`): the whole text, folding case through UTF-8.
fn same_name(buffer: &[u8], name: &str) -> bool {
    String::from_utf8_lossy(buffer).to_lowercase() == name.to_lowercase()
}

impl UnitGuids {
    /// Equal in everything a push carries: the focus is the VM's own.
    pub(crate) fn same_push(&self, other: &Self) -> bool {
        let Self {
            player,
            pet,
            vehicle,
            target,
            mouseover,
            npc,
            focus: _,
            party,
            party_pets,
            raid,
            raid_pets,
            boss,
            arena,
            arena_pets,
            arena_count,
            held,
        } = self;
        *player == other.player
            && *pet == other.pet
            && *vehicle == other.vehicle
            && *target == other.target
            && *mouseover == other.mouseover
            && *npc == other.npc
            && *party == other.party
            && *party_pets == other.party_pets
            && *raid == other.raid
            && *raid_pets == other.raid_pets
            && *boss == other.boss
            && *arena == other.arena
            && *arena_pets == other.arena_pets
            && *arena_count == other.arena_count
            && *held == other.held
    }

    /// The guid `token` names with no name lookup, which needs the names [`Self::resolve_named`]
    /// reads: `None` for nobody, `Err` where the lookup would run.
    #[cfg(test)]
    pub(crate) fn resolve(&self, token: &str) -> Result<Option<u64>, ()> {
        self.resolve_named(token, |_, _| None)
    }

    /// The guid `token` names (`0x60abf0`, with no target default), `None` for nobody, `Err` where
    /// the reference answers false: the empty token, or a name lookup that finds no one. A base
    /// whose unit is there, or that names nobody with no text after it, takes the hops; a hop to
    /// nobody is nobody. Otherwise the whole token is a name: `name_of(token, with_realm)` is the
    /// name a unit token's unit goes by (`0x60a360`), `"Name-Realm"` with the realm asked for and
    /// known.
    pub(crate) fn resolve_named(
        &self,
        token: &str,
        name_of: impl Fn(&str, bool) -> Option<String>,
    ) -> Result<Option<u64>, ()> {
        match parse_unit_token(token) {
            UnitTokenParse::Empty => return Err(()),
            UnitTokenParse::Nobody => return Ok(None),
            UnitTokenParse::Unit { base, hops } => {
                let guid = self.base_guid(base);
                if guid != 0 || hops.is_empty() {
                    if let Some(end) = self.follow(guid, hops) {
                        return Ok(Some(end).filter(|&g| g != 0));
                    }
                }
            }
            UnitTokenParse::Name => {}
        }
        let (guid, hops) = self.by_name(token, name_of).ok_or(())?;
        let end = self.follow(guid, hops).ok_or(())?;
        Ok(Some(end).filter(|&g| g != 0))
    }

    /// The unit a base names, 0 for nobody.
    pub(crate) fn base_guid(&self, base: UnitBase) -> u64 {
        // The indexed bases: the table's row for the parsed number.
        let at = |table: &[u64], row: u32| {
            usize::try_from(row)
                .ok()
                .and_then(|row| table.get(row))
                .copied()
                .unwrap_or(0)
        };
        match base {
            UnitBase::Player => self.player,
            UnitBase::Pet if self.player == 0 => 0,
            UnitBase::Pet => self.pet,
            UnitBase::Target => self.target,
            UnitBase::Npc => self.npc,
            // Deviation: the reference's party-pet lookup has no bound and reads past its
            // four-entry table, which is stale memory, not a mechanism; past it, nobody.
            UnitBase::PartyPet(row) => at(&self.party_pets, row),
            // Capped at 4.
            UnitBase::Party(row) => at(&self.party, row),
            // Capped at the roster count.
            UnitBase::RaidPet(row) => at(&self.raid_pets, row),
            UnitBase::Raid(row) => at(&self.raid, row),
            // Bounded by the slot count, as `0x5ed710` is.
            UnitBase::Boss(row) => at(&self.boss, row),
            UnitBase::Mouseover if !self.held.contains_key(&self.mouseover) => 0,
            UnitBase::Mouseover => self.mouseover,
            UnitBase::Focus => self.focus,
            UnitBase::Vehicle => self.vehicle,
            // Bounded by the five slots only (`0x608180`, `0x60a2f0`).
            UnitBase::Arena(row) => at(&self.arena, row),
            UnitBase::ArenaPet(row) => at(&self.arena_pets, row),
            // The open quest window's giver, read live (`0x6081b0`).
            UnitBase::QuestNpc => read_live(|l| l.quest_npc()).flatten().unwrap_or(0),
            // The spectated players are listed in commentator mode alone, which benilla never
            // asks for (`CMSG_COMMENTATOR_ENABLE`), so the list is empty.
            UnitBase::Commentator(_) => 0,
        }
    }

    /// Whether `guid` is a party member or on the raid roster (`0x512a00`).
    fn in_group(&self, guid: u64) -> bool {
        guid != 0 && (self.party.contains(&guid) || self.raid.contains(&guid))
    }

    /// The hops from `guid` (`0x60aaa0`): `None` where they fail, which zeroes the guid and sends
    /// the token to the name lookup; a hop to nobody goes on from nobody. A `target` hop off the
    /// player reads the target global; off another unit, its held `UNIT_FIELD_TARGET`. A `pet`
    /// hop reads the held unit's pet live.
    pub(crate) fn follow(&self, mut guid: u64, hops: Hops<'_>) -> Option<u64> {
        let me = (self.player != 0).then_some(self.player);
        for hop in hops {
            guid = match hop.ok()? {
                Hop::Target if Some(guid) == me => self.target,
                Hop::Target => self.held.get(&guid).copied().unwrap_or(0),
                Hop::Pet => {
                    let open = Some(guid) == me
                        || self.in_group(guid)
                        || me
                            .and_then(|me| read_live(|l| l.player_flags(me)).flatten())
                            .is_some_and(|flags| flags & PET_OF_ANYONE != 0);
                    if !open {
                        return None;
                    }
                    read_live(|l| l.unit_pet(guid)).flatten().unwrap_or(0)
                }
            };
        }
        Some(guid)
    }

    /// The name lookup (`0x60a630`): the name in [`split_name`]'s buffer against ours, then the
    /// raid roster's, else the party's, else the counted arena opponents'; then our pet's, and the
    /// same tables' pets. Returns the first match's guid and the hops after the name.
    pub(crate) fn by_name<'t>(
        &self,
        token: &'t str,
        name_of: impl Fn(&str, bool) -> Option<String>,
    ) -> Option<(u64, Hops<'t>)> {
        let (buffer, with_realm, hops) = split_name(token);
        let named = |unit: &str| name_of(unit, with_realm).is_some_and(|n| same_name(buffer, &n));
        let party_count = self.party.iter().take_while(|&&g| g != 0).count();
        let arena_count = self.arena_count.min(self.arena.len());
        let first = |prefix: &str, table: &[u64], count: usize| {
            (0..count.min(table.len()))
                .find(|i| named(&format!("{prefix}{}", i + 1)))
                .map_or(0, |i| table[i])
        };
        let mut guid = if named("player") {
            self.player
        } else if !self.raid.is_empty() {
            first("raid", &self.raid, self.raid.len())
        } else if party_count != 0 {
            first("party", &self.party, party_count)
        } else {
            first("arena", &self.arena, arena_count)
        };
        if guid == 0 {
            guid = if named("pet") {
                self.base_guid(UnitBase::Pet)
            } else if !self.raid.is_empty() {
                first("raidpet", &self.raid_pets, self.raid.len())
            } else if party_count != 0 {
                first("partypet", &self.party_pets, party_count)
            } else {
                first("arenapet", &self.arena_pets, arena_count)
            };
        }
        (guid != 0).then_some((guid, hops))
    }

    /// The guids a hop can end on: each held unit's target, when it names one. `held` holds the
    /// bases' units and each unit a hop reaches from them, so a token with a hop names one of
    /// these or nobody, and a per-guid feed over them covers the chains a script can spell. A guid
    /// repeats when two units share a target, and the last hop's unit need not be held: the lookup
    /// that finds its object is the reader's.
    pub(crate) fn chain_ends(&self) -> impl Iterator<Item = u64> + '_ {
        // The focus and the vehicle too: they read their unit by guid, the way a hop's end is read.
        self.held
            .values()
            .copied()
            .chain([self.focus, self.vehicle])
            .filter(|&guid| guid != 0 && guid != crate::script::focus::NOBODY)
    }

    /// [`Self::resolve`], a token the reference refuses naming nobody.
    #[cfg(test)]
    pub(crate) fn guid_of(&self, token: &str) -> Option<u64> {
        self.resolve(token).unwrap_or(None)
    }

    /// Every token naming `guid`, in the order 12340's unit-event fan-out lists them (`0x60bb70`):
    /// `player`, `vehicle`, `pet`, `partyN`, `partypetN`, `raidN`, `raidpetN`, `arenaN` or
    /// `arenapetN`, `bossN`, `target`, `focus`, `npc`. No hop and no `mouseover`.
    pub(crate) fn tokens_naming(&self, guid: u64) -> Vec<String> {
        let mut out = Vec::new();
        if guid == 0 {
            return out;
        }
        if self.player == guid {
            out.push("player".into());
        }
        if self.vehicle == guid {
            out.push("vehicle".into());
        }
        if self.player != 0 && self.pet == guid {
            out.push("pet".into());
        }
        for (prefix, table) in [
            ("party", &self.party[..]),
            ("partypet", &self.party_pets[..]),
            ("raid", &self.raid[..]),
            ("raidpet", &self.raid_pets[..]),
        ] {
            for (i, &g) in table.iter().enumerate() {
                if g == guid {
                    out.push(format!("{prefix}{}", i + 1));
                }
            }
        }
        // Over the counted slots, an opponent tested before its pet, the first match ending it.
        for i in 0..self.arena_count.min(self.arena.len()) {
            if self.arena[i] == guid {
                out.push(format!("arena{}", i + 1));
                break;
            }
            if self.arena_pets[i] == guid {
                out.push(format!("arenapet{}", i + 1));
                break;
            }
        }
        // The first slot only, as the reference's loop breaks on a match.
        if let Some(i) = self.boss.iter().position(|&g| g == guid) {
            out.push(format!("boss{}", i + 1));
        }
        if self.target == guid {
            out.push("target".into());
        }
        if self.focus == guid {
            out.push("focus".into());
        }
        if self.npc == guid {
            out.push("npc".into());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u64 = 0x10;
    const PET: u64 = 0xF140_0000_0000_0077;
    const MOB: u64 = 0xF130_0000_0000_0001;
    const P1: u64 = 0x21;
    const P1_PET: u64 = 0xF140_0000_0000_0021;
    const R3: u64 = 0x33;
    const FAR: u64 = 0x44;

    /// Us targeting a mob that targets party1, who targets us; party1's pet and raid row 3 held,
    /// `FAR` a roster member out of range.
    /// `focus` names the focus guid, `focustarget` hops off it, and the fan-out lists `focus`
    /// between `target` and `npc` (`0x60bb70`).
    #[test]
    fn the_focus_is_a_base_and_is_fanned_out() {
        let g = UnitGuids {
            focus: P1,
            ..guids()
        };
        assert_eq!(g.guid_of("focus"), Some(P1));
        assert_eq!(g.guid_of("focustarget"), Some(ME));
        assert_eq!(guids().guid_of("focus"), None, "no focus, nobody");
        let mob = UnitGuids {
            focus: MOB,
            ..guids()
        };
        assert_eq!(mob.tokens_naming(MOB), ["target", "focus", "npc"]);
    }

    /// `vehicle` names the seated player's transport, `vehicletarget` hops off it, and the
    /// fan-out lists `vehicle` right after `player` (`0x60bb70`).
    #[test]
    fn the_vehicle_is_a_base_and_is_fanned_out() {
        const CART: u64 = 0xF150_0000_0000_0001;
        let mut g = UnitGuids {
            vehicle: CART,
            pet: CART,
            ..guids()
        };
        g.held.insert(CART, ME);
        assert_eq!(g.guid_of("vehicle"), Some(CART));
        assert_eq!(g.guid_of("vehicletarget"), Some(ME));
        assert_eq!(g.tokens_naming(CART), ["vehicle", "pet"]);
        assert_eq!(guids().guid_of("vehicle"), None, "not seated, nobody");
        assert!(g.chain_ends().any(|e| e == CART));
    }

    /// `bossN` reads the encounter's slot N-1, bounded by the slots (`0x5ed710`), and the fan-out
    /// lists it after the raid tables and before `target` (`0x60bb70`).
    #[test]
    fn boss_tokens_read_the_encounter_slots() {
        let mut g = guids();
        assert_eq!(g.guid_of("boss1"), None, "no slots");
        g.boss = vec![MOB, 0];
        assert_eq!(g.guid_of("boss1"), Some(MOB));
        assert_eq!(g.guid_of("Boss1target"), Some(P1), "a hop off the boss");
        assert_eq!(g.guid_of("boss2"), None, "an empty slot");
        assert_eq!(g.guid_of("boss3"), None, "past the slots");
        assert_eq!(g.guid_of("boss0"), None);
        assert_eq!(g.tokens_naming(MOB), ["boss1", "target", "npc"]);
    }

    /// `arenaN` reads opponent slot N-1 whatever the count (`0x608180`); the fan-out lists the
    /// counted slots after the raid tables and before `bossN`, an opponent ahead of its pet
    /// (`0x60bb70`).
    #[test]
    fn arena_tokens_read_the_opponent_slots() {
        const FOE: u64 = 0x55;
        const FOE_PET: u64 = 0xF140_0000_0000_0055;
        let mut g = guids();
        assert_eq!(g.guid_of("arena1"), None, "no opponents");
        g.arena = [FOE, 0, MOB, 0, 0];
        g.arena_pets = [FOE_PET, 0, 0, 0, 0];
        g.arena_count = 1;
        g.held.insert(FOE, ME);
        assert_eq!(g.guid_of("arena1"), Some(FOE));
        assert_eq!(
            g.guid_of("ARENA1target"),
            Some(ME),
            "a hop off the opponent"
        );
        assert_eq!(g.guid_of("arenapet1"), Some(FOE_PET));
        assert_eq!(g.guid_of("arena3"), Some(MOB), "past the count, still read");
        assert_eq!(g.guid_of("arena6"), None, "past the slots");
        assert_eq!(g.tokens_naming(FOE), ["arena1"]);
        assert_eq!(g.tokens_naming(FOE_PET), ["arenapet1"]);
        assert_eq!(
            g.tokens_naming(MOB),
            ["target", "npc"],
            "slot 3 is past the count"
        );
    }

    fn guids() -> UnitGuids {
        UnitGuids {
            player: ME,
            pet: PET,
            vehicle: 0,
            target: MOB,
            mouseover: MOB,
            npc: MOB,
            focus: 0,
            party: [P1, FAR, 0, 0],
            party_pets: [P1_PET, 0, 0, 0],
            raid: vec![ME, P1, R3],
            raid_pets: vec![PET, P1_PET, 0],
            boss: Vec::new(),
            arena: [0; 5],
            arena_pets: [0; 5],
            arena_count: 0,
            held: HashMap::from([
                (ME, MOB),
                (PET, 0),
                (MOB, P1),
                (P1, ME),
                (P1_PET, MOB),
                (R3, 0),
            ]),
        }
    }

    /// A base token's base and its hops, `None` for any other parse.
    fn read(token: &str) -> Option<(UnitBase, Vec<Result<Hop, ()>>)> {
        match parse_unit_token(token) {
            UnitTokenParse::Unit { base, hops } => Some((base, hops.collect())),
            _ => None,
        }
    }

    fn at(token: &str) -> Option<u64> {
        guids().resolve(token).expect("recognised")
    }

    #[test]
    fn every_base_resolves_through_its_table() {
        assert_eq!(at("player"), Some(ME));
        assert_eq!(at("pet"), Some(PET));
        assert_eq!(at("target"), Some(MOB));
        assert_eq!(at("mouseover"), Some(MOB));
        assert_eq!(at("npc"), Some(MOB));
        assert_eq!(at("party1"), Some(P1));
        // Not held, but a roster member: the base needs no object.
        assert_eq!(at("party2"), Some(FAR));
        assert_eq!(at("partypet1"), Some(P1_PET));
        assert_eq!(at("raid3"), Some(R3));
        assert_eq!(at("raidpet2"), Some(P1_PET));
    }

    #[test]
    fn the_compares_fold_ascii_case() {
        assert_eq!(at("PLAYER"), Some(ME));
        assert_eq!(at("Player"), Some(ME));
        assert_eq!(at("PartyPet1"), Some(P1_PET));
        assert_eq!(at("MouseOver"), Some(MOB));
        assert_eq!(at("NPC"), Some(MOB));
        assert_eq!(at("TargetTarget"), Some(P1));
    }

    #[test]
    fn target_is_a_loop_on_every_base_but_npc() {
        assert_eq!(at("targettarget"), Some(P1));
        assert_eq!(at("targettargettarget"), Some(ME));
        assert_eq!(at("targettargettargettarget"), Some(MOB));
        assert_eq!(at("playertarget"), Some(MOB));
        assert_eq!(at("party1target"), Some(ME));
        assert_eq!(at("partypet1target"), Some(MOB));
        assert_eq!(at("raid2target"), Some(ME));
        assert_eq!(at("raid2targettarget"), Some(MOB));
        assert_eq!(at("mouseovertarget"), Some(P1));
        // A hop to nobody ends the chain, as does a hop off a unit not held.
        assert_eq!(at("pettarget"), None);
        assert_eq!(at("party2target"), None);
        assert!(guids().resolve("npctarget").is_err());
    }

    /// The parse reads the text alone: the base by the resolver's compares, a numbered base's
    /// 0-based row, then `target` hops to the end, whatever the depth.
    #[test]
    fn a_token_parses_to_a_base_and_its_hops() {
        use UnitBase::*;
        let unit = |base, n: usize| Some((base, vec![Ok(Hop::Target); n]));
        for (token, want) in [
            ("player", unit(Player, 0)),
            ("pet", unit(Pet, 0)),
            ("target", unit(Target, 0)),
            ("mouseover", unit(Mouseover, 0)),
            ("npc", unit(Npc, 0)),
            ("party1", unit(Party(0), 0)),
            ("partypet4", unit(PartyPet(3), 0)),
            ("raid40", unit(Raid(39), 0)),
            ("raidpet2", unit(RaidPet(1), 0)),
            ("targettarget", unit(Target, 1)),
            ("targettargettargettarget", unit(Target, 3)),
            ("playertarget", unit(Player, 1)),
            ("pettarget", unit(Pet, 1)),
            ("mouseovertarget", unit(Mouseover, 1)),
            ("party1target", unit(Party(0), 1)),
            ("partypet1target", unit(PartyPet(0), 1)),
            ("raid12targettargettarget", unit(Raid(11), 3)),
            ("raidpet3target", unit(RaidPet(2), 1)),
            // A missing or zero number is the row every bound rejects.
            ("party", unit(Party(u32::MAX), 0)),
            ("raid0target", unit(Raid(u32::MAX), 1)),
            // The compares fold case, the hops too.
            ("PARTY1TARGET", unit(Party(0), 1)),
            ("PartyPet2TargetTarget", unit(PartyPet(1), 2)),
        ] {
            assert_eq!(read(token), want, "{token}");
        }
        let deep = format!("player{}", "target".repeat(1000));
        assert_eq!(read(&deep), unit(Player, 1000), "no depth limit");
    }

    /// Text after the base, or after any hop, that is neither hop reads as one `Err`, which ends
    /// the hops; a token none of the compares match is the name lookup's, `npc` being an exact
    /// compare with no hop.
    #[test]
    fn text_that_is_not_a_hop_ends_the_hops_and_no_base_is_a_name() {
        use Hop::*;
        for (token, want) in [
            ("party1foo", vec![Err(())]),
            ("player1", vec![Err(())]),
            ("pet1", vec![Err(())]),
            ("party1targetfoo", vec![Ok(Target), Err(())]),
            ("party1targettarge", vec![Ok(Target), Err(())]),
            ("party1 target", vec![Err(())]),
            ("raid3x", vec![Err(())]),
            ("party1-", vec![Err(())]),
            ("party1--target", vec![Err(())]),
            ("party1pet", vec![Ok(Pet)]),
            ("party1-pet-target", vec![Ok(Pet), Ok(Target)]),
            ("Party1PETtarget", vec![Ok(Pet), Ok(Target)]),
            ("targetpettarget", vec![Ok(Pet), Ok(Target)]),
        ] {
            assert_eq!(read(token).map(|(_, hops)| hops), Some(want), "{token}");
        }
        assert_eq!(parse_unit_token(""), UnitTokenParse::Empty);
        for token in [
            "npctarget",
            "npc1",
            "bogus",
            "tar",
            "focu",
            "5",
            "targe",
            "nonex",
        ] {
            assert_eq!(parse_unit_token(token), UnitTokenParse::Name, "{token}");
        }
    }

    /// 12340's bases past 1.12.1's (`0x60abf0`): each parses, and names nobody with no guid
    /// pushed; `commentatorN` always, as benilla never enters commentator mode.
    #[test]
    fn the_3_3_5a_bases_parse_and_name_nobody() {
        for (token, base, hops) in [
            ("boss1", UnitBase::Boss(0), 0),
            ("Boss4", UnitBase::Boss(3), 0),
            ("arena2", UnitBase::Arena(1), 0),
            ("arenapet5", UnitBase::ArenaPet(4), 0),
            ("commentator3", UnitBase::Commentator(2), 0),
            ("focus", UnitBase::Focus, 0),
            ("focustarget", UnitBase::Focus, 1),
            ("vehicle", UnitBase::Vehicle, 0),
            ("questnpc", UnitBase::QuestNpc, 0),
        ] {
            assert_eq!(
                read(token),
                Some((base, vec![Ok(Hop::Target); hops])),
                "{token}"
            );
            let want = if hops == 0 { Ok(None) } else { Err(()) };
            assert_eq!(guids().resolve(token), want, "{token}");
        }
        assert_eq!(parse_unit_token("none"), UnitTokenParse::Nobody);
        assert_eq!(parse_unit_token("questnpctarget"), UnitTokenParse::Name);
    }

    #[test]
    fn a_recognised_token_naming_nobody_is_quiet() {
        for t in [
            "party",
            "party0",
            "party5",
            "raid0",
            "raid4",
            "raidpet3",
            "partypet5",
            "none",
        ] {
            assert_eq!(guids().resolve(t), Ok(None), "{t}");
        }
        let solo = UnitGuids {
            player: 0,
            pet: PET,
            ..guids()
        };
        assert_eq!(solo.resolve("pet"), Ok(None), "no player object, no pet");
        let gone = UnitGuids {
            held: HashMap::new(),
            ..guids()
        };
        assert_eq!(gone.resolve("mouseover"), Ok(None), "a mouseover not held");
        assert_eq!(
            gone.resolve("target"),
            Ok(Some(MOB)),
            "the target global is not gated"
        );
    }

    /// The tokens the reference refuses (`0x60abf0` false): the empty one, and any token the name
    /// lookup finds no one for, a base naming nobody with text after it and a base whose text is
    /// not hops among them.
    #[test]
    fn a_token_no_name_matches_is_refused() {
        for t in [
            "",
            "party1foo",
            "playerfoo",
            "targetfoo",
            "partyX",
            "pet1",
            "party5target",
            "raid0target",
            "player-",
        ] {
            assert_eq!(guids().resolve(t), Err(()), "{t}");
        }
    }

    /// A `-` may stand before any hop, and `target` off the player reads the target global.
    #[test]
    fn a_dash_may_lead_each_hop() {
        assert_eq!(at("party1-target"), Some(ME));
        assert_eq!(at("party1-target-target"), Some(MOB));
        assert_eq!(at("PLAYER-TARGET"), Some(MOB));
        let g = UnitGuids {
            target: P1,
            ..guids()
        };
        assert_eq!(
            g.guid_of("playertarget"),
            Some(P1),
            "the global, not the field"
        );
    }

    /// The `pet` hop (`0x60ab2c`-`0x60abb4`): our pet off us, a member's off a party or raid member,
    /// and off any other unit only with [`PET_OF_ANYONE`] in our `PLAYER_FLAGS`; refused otherwise,
    /// which sends the token to the name lookup. A unit with no pet, or not held, hops to nobody.
    #[test]
    fn the_pet_hop_reads_the_held_units_pet() {
        const MOB_PET: u64 = 0xF140_0000_0000_00AA;
        let view = |flags: u32| crate::script::live::TestView {
            units: HashMap::from([(ME, (0, 1))]),
            player_flags: HashMap::from([(ME, flags)]),
            unit_pets: HashMap::from([(ME, PET), (P1, P1_PET), (MOB, MOB_PET), (R3, 0x3355)]),
            ..Default::default()
        };
        let _loan = crate::script::live::lend_test(view(0));
        assert_eq!(at("playerpet"), Some(PET));
        assert_eq!(at("party1pet"), Some(P1_PET));
        assert_eq!(at("Party1Pet-Target"), Some(MOB));
        assert_eq!(at("raid3pet"), Some(0x3355), "a raid member's");
        assert_eq!(at("party2pet"), None, "a member not held has no pet");
        assert_eq!(at("party1pettarget"), Some(MOB));
        assert_eq!(
            at("targettargetpet"),
            Some(P1_PET),
            "the hop's unit is the member"
        );
        assert_eq!(guids().resolve("targetpet"), Err(()), "not ours to read");
        assert_eq!(guids().resolve("petpet"), Err(()));
        drop(_loan);
        let _loan = crate::script::live::lend_test(view(PET_OF_ANYONE));
        assert_eq!(at("targetpet"), Some(MOB_PET), "any unit's under the flag");
        assert_eq!(at("petpet"), None);
    }

    /// Names for the lookup: ours, party1's, party2's, our pet's and party1's pet's, and party1 is
    /// of another realm.
    fn names(unit: &str, with_realm: bool) -> Option<String> {
        Some(match (unit, with_realm) {
            ("player", _) => "Me".into(),
            ("party1", false) => "Pia".into(),
            ("party1", true) => "Pia-Faraway".into(),
            ("party2", _) => "Petra".into(),
            ("pet", _) => "Fluffy".into(),
            ("partypet1", _) => "Wolfie".into(),
            ("raid1", _) => "Ria".into(),
            _ => return None,
        })
    }

    /// The name lookup (`0x60a630`): a token no base reads, or a base whose text is not hops,
    /// matches our name, then the party's, then our pet's and the party's pets', folding case; the
    /// hops after a `-target` or `-pet` follow it, and any other `-` asks for `Name-Realm`.
    #[test]
    fn a_token_that_reads_as_no_unit_is_looked_up_by_name() {
        let g = UnitGuids {
            raid: Vec::new(),
            raid_pets: Vec::new(),
            ..guids()
        };
        let find = |t: &str| g.resolve_named(t, names);
        assert_eq!(find("Me"), Ok(Some(ME)));
        assert_eq!(find("pia"), Ok(Some(P1)));
        assert_eq!(find("Pia-target"), Ok(Some(ME)), "party1 targets us");
        assert_eq!(find("Pia-TARGET-target"), Ok(Some(MOB)));
        assert_eq!(find("Pia-Faraway"), Ok(Some(P1)), "with the realm");
        assert_eq!(
            find("Pia-Faraway-target"),
            Ok(Some(ME)),
            "the realm, then its hop"
        );
        assert_eq!(
            find("Pia-targetx"),
            Err(()),
            "`-target` stops the name whole only"
        );
        assert_eq!(find("Petra"), Ok(Some(FAR)), "`pet` reads `ra` as no hop");
        assert_eq!(find("Fluffy"), Ok(Some(PET)));
        assert_eq!(find("Wolfie-target"), Ok(Some(MOB)));
        assert_eq!(find("Piatarget"), Err(()), "only a `-` ends the name");
        assert_eq!(find("Nobody"), Err(()));
        assert_eq!(
            guids().resolve_named("Pia", names),
            Err(()),
            "in a raid the roster's names are read, not the party's"
        );
        assert_eq!(guids().resolve_named("Ria", names), Ok(Some(ME)));
        let long = format!("Pia{}", "x".repeat(60));
        assert_eq!(find(&long), Err(()), "48 bytes at most");
    }

    #[test]
    fn an_unrecognised_token_is_the_raise() {
        for t in ["bogus", "5", "npc1", "tar"] {
            assert_eq!(guids().resolve(t), Err(()), "{t}");
        }
    }

    #[test]
    fn the_index_parse_wraps_and_stops_at_the_first_non_digit() {
        assert_eq!(at("party01"), Some(P1));
        assert_eq!(at("raid03"), Some(R3));
        assert_eq!(at("raid03target"), None, "raid3 targets nobody");
        assert_eq!(
            index_of(b"4294967297"),
            (0, 10),
            "2^32 + 1 wraps to 1, index 0"
        );
        assert_eq!(index_of(b"x"), (u32::MAX, 0));
    }
}
