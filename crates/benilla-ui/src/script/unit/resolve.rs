//! The unit-token resolver, token to guid, over the state it reads, which the app pushes as
//! [`UnitGuids`]: a base, then any number of `target` hops, each off the current unit's
//! `UNIT_FIELD_TARGET`. Every compare folds ASCII case. The bases are 12340's (`0x60abf0`, in its
//! compare order): `player`, `vehicle`, `pet`, `target`, `partypetN`, `partyN`, `raidpetN`,
//! `raidN`, `bossN`, `arenapetN`, `arenaN`, `commentatorN`, `mouseover`, `focus`, then the exact
//! `npc`, `questnpc` and `none`. The app pushes no guid for `bossN`, `arenaN`, `arenapetN`,
//! `commentatorN` or `questnpc` yet, so they name nobody (#90); 12340's
//! `pet` and `-` suffixes and its name lookup (`0x60aaa0`, `0x60a630`) are not built.

use std::collections::HashMap;

/// What the resolver reads: the guid globals, the group tables and, for each unit a token can
/// reach, whether it is held and whom it targets. A zero guid is nobody.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitGuids {
    /// The local player's guid; 0 without the player object, when `player` and `pet` answer nil
    /// (`0x5159c9`, `0x515a58`).
    pub player: u64,
    /// The pet: `UNIT_FIELD_CHARM`, else `UNIT_FIELD_SUMMON` (`0x515a64`).
    pub pet: u64,
    /// The vehicle the player sits in, its movement transport while seated (`0x60ac87`), else 0.
    pub vehicle: u64,
    /// The target global `[0xb4e2d8]`.
    pub target: u64,
    /// The mouseover global `[0xb4e2c8]`, read only while it is a held unit (`0x515bd2`).
    pub mouseover: u64,
    /// The interaction NPC `[0xb4e2d0]`.
    pub npc: u64,
    /// The focus `[0xbd07d0]`, which `FocusUnit` sets in the VM; a push keeps the VM's.
    pub focus: u64,
    /// `partyN`'s guid, index N-1 (`0x4e81a0`).
    pub party: [u64; 4],
    /// `partypetN`'s guid, index N-1 (`0x4e81d0`).
    pub party_pets: [u64; 4],
    /// `raidN`'s guid, one per roster row; a row past the end is nobody (`0x491940`).
    pub raid: Vec<u64>,
    /// `raidpetN`'s guid, row for row with [`Self::raid`] (`0x491960`).
    pub raid_pets: Vec<u64>,
    /// Each held unit a token can reach, to its `UNIT_FIELD_TARGET` (0 for none): the object
    /// manager's lookup with the unit typemask (`0x468460`, `ecx = 8`) and the descriptor read a
    /// `target` hop makes (`0x515a0f`-`0x515a25`). A guid missing here is not held.
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

/// The unit a token starts from, before any `target` hop. A numbered base carries its 0-based
/// row, [`index_of`]'s: a table lookup that fails on the reserved `u32::MAX` of a missing or zero
/// number names nobody.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitBase {
    Player,
    Pet,
    Target,
    Mouseover,
    /// The interaction NPC; an exact compare, so it takes no `target` hop (`0x60af58`).
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

/// What `0x515970` makes of a token's text, before it reads any state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitTokenParse {
    /// None of the compares match: the reference raises `Unknown unit name` (`0x515c14`).
    Unknown,
    /// Recognised, and names nobody whatever the state: the empty token (`0x51599d`), or text
    /// after the base, and after each hop, that is not `target` (`0x515c2d`).
    Nobody,
    /// A base and the `target` hops after it, each off the current unit's `UNIT_FIELD_TARGET`
    /// (`0x515a1c`-`0x515a25`); the loop has no depth limit.
    Unit { base: UnitBase, hops: usize },
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

/// The inline base-10 parse after `party`, `partypet`, `raid` and `raidpet`, then `dec` to a
/// 0-based index (`0x515ab1`-`0x515acc`): digits accumulate with 32-bit wrap, and a missing
/// number leaves 0, whose index `0xffffffff` every bound rejects.
fn index_of(s: &[u8]) -> (u32, usize) {
    let digits = s.iter().take_while(|b| b.is_ascii_digit()).count();
    let n = s[..digits].iter().fold(0u32, |acc, &b| {
        acc.wrapping_mul(10).wrapping_add(u32::from(b - b'0'))
    });
    (n.wrapping_sub(1), digits)
}

/// Read `token`'s text as `0x515970` does, before any state: the base by the compares in their
/// order, a numbered base's row, then `target` hops to the end of the text. `"party1target"` is
/// party1 and one hop, `"targettarget"` the target and one, and `"npctarget"` is none of the
/// compares, as `npc` is one full-string compare.
pub fn parse_unit_token(token: &str) -> UnitTokenParse {
    let bytes = token.as_bytes();
    if bytes.is_empty() {
        return UnitTokenParse::Nobody;
    }
    let Some((base, len)) = base_of(bytes) else {
        let exact = |base| UnitTokenParse::Unit { base, hops: 0 };
        return if token.eq_ignore_ascii_case("npc") {
            exact(UnitBase::Npc)
        } else if token.eq_ignore_ascii_case("questnpc") {
            exact(UnitBase::QuestNpc)
        } else if token.eq_ignore_ascii_case("none") {
            // The `-1` guid (`0x60afc9`), which names no object.
            UnitTokenParse::Nobody
        } else {
            UnitTokenParse::Unknown
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
    let mut hops = 0;
    while !rest.is_empty() {
        if !has_prefix(rest, "target") {
            return UnitTokenParse::Nobody;
        }
        rest = &rest[6..];
        hops += 1;
    }
    UnitTokenParse::Unit { base, hops }
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
            && *held == other.held
    }

    /// The guid `token` names, or `None` for nobody; `Err` for a token none of the compares
    /// match, where the reference raises `Unknown unit name: %s` (`0x515c14`). An empty token is
    /// nobody (`0x51599d`).
    pub(crate) fn resolve(&self, token: &str) -> Result<Option<u64>, ()> {
        match parse_unit_token(token) {
            UnitTokenParse::Unknown => Err(()),
            UnitTokenParse::Nobody => Ok(None),
            UnitTokenParse::Unit { base, hops } => Ok(self.resolve_unit(base, hops)),
        }
    }

    /// The guid a parsed token names: its base's guid, then each `target` hop. `None` is nobody.
    pub(crate) fn resolve_unit(&self, base: UnitBase, hops: usize) -> Option<u64> {
        // The indexed bases: the table's row for the parsed number.
        let at = |table: &[u64], row: u32| {
            usize::try_from(row)
                .ok()
                .and_then(|row| table.get(row))
                .copied()
                .unwrap_or(0)
        };
        let guid = match base {
            UnitBase::Player => self.player,
            UnitBase::Pet if self.player == 0 => return None,
            UnitBase::Pet => self.pet,
            UnitBase::Target => self.target,
            UnitBase::Npc => self.npc,
            // Deviation: `0x4e81d0` has no bound and reads past its four-entry table, which is
            // stale memory, not a mechanism; past it, nobody.
            UnitBase::PartyPet(row) => at(&self.party_pets, row),
            // Capped at 4 (`0x4e81b0`).
            UnitBase::Party(row) => at(&self.party, row),
            // Capped at the roster count (`0x491940`, `0x491960`).
            UnitBase::RaidPet(row) => at(&self.raid_pets, row),
            UnitBase::Raid(row) => at(&self.raid, row),
            UnitBase::Mouseover if !self.held.contains_key(&self.mouseover) => return None,
            UnitBase::Mouseover => self.mouseover,
            UnitBase::Focus => self.focus,
            UnitBase::Vehicle => self.vehicle,
            // No guid pushed for these yet (#90).
            UnitBase::QuestNpc
            | UnitBase::Boss(_)
            | UnitBase::Arena(_)
            | UnitBase::ArenaPet(_)
            | UnitBase::Commentator(_) => return None,
        };
        self.follow(guid, hops)
    }

    /// The guids a `target` hop can end on: each held unit's target, when it names one. `held`
    /// holds the bases' units and each unit a hop reaches from them, so a token with a hop names
    /// one of these or nobody, and a per-guid feed over them covers the chains a script can spell.
    /// A guid repeats when two units share a target, and the last hop's unit need not be held: the
    /// lookup that finds its object is the reader's.
    pub(crate) fn chain_ends(&self) -> impl Iterator<Item = u64> + '_ {
        // The focus and the vehicle too: they read their unit by guid, the way a hop's end is read.
        self.held
            .values()
            .copied()
            .chain([self.focus, self.vehicle])
            .filter(|&guid| guid != 0 && guid != crate::script::focus::NOBODY)
    }

    /// [`Self::resolve`], an unrecognised token naming nobody: the client's resolver raises for
    /// none (`Wow.exe` holds no unit-name error string).
    pub(crate) fn guid_of(&self, token: &str) -> Option<u64> {
        self.resolve(token).unwrap_or(None)
    }

    /// Every token naming `guid`, in the order 12340's unit-event fan-out lists them (`0x60bb70`):
    /// `player`, `vehicle`, `pet`, `partyN`, `partypetN`, `raidN`, `raidpetN`, `target`, `focus`,
    /// `npc`. No `target` hop and no `mouseover`. The reference also lists `arenaN`, `arenapetN`
    /// and `bossN`, which this resolver has no base for.
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

    /// The `target` loop (`0x5159d3`-`0x515a2c`): nobody stays nobody, and each hop goes to the held
    /// unit's `UNIT_FIELD_TARGET`; a hop off a unit not held is a quiet nobody (`0x515a16`).
    fn follow(&self, mut guid: u64, hops: usize) -> Option<u64> {
        for _ in 0..hops {
            if guid == 0 {
                return None;
            }
            guid = *self.held.get(&guid)?;
        }
        Some(guid).filter(|&g| g != 0)
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
        // A hop to nobody ends the chain (`0x515a2e`), as does a hop off a unit not held.
        assert_eq!(at("pettarget"), None);
        assert_eq!(at("party2target"), None);
        assert!(guids().resolve("npctarget").is_err());
    }

    /// The parse reads the text alone: the base by the resolver's compares, a numbered base's
    /// 0-based row, then `target` hops to the end, whatever the depth.
    #[test]
    fn a_token_parses_to_a_base_and_its_hops() {
        use UnitBase::*;
        let unit = |base, hops| UnitTokenParse::Unit { base, hops };
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
            assert_eq!(parse_unit_token(token), want, "{token}");
        }
        let deep = format!("player{}", "target".repeat(1000));
        assert_eq!(
            parse_unit_token(&deep),
            unit(Player, 1000),
            "no depth limit"
        );
    }

    /// Text after the base, or after any hop, that is not `target` names nobody; a token none of
    /// the compares match is the raise, `npc` being an exact compare with no chain.
    #[test]
    fn a_token_that_is_not_base_then_hops_parses_to_nobody_or_the_raise() {
        for token in [
            "",
            "party1foo",
            "playerfoo",
            "player1",
            "pet1",
            "party1targetfoo",
            "party1targettarge",
            "party1 target",
            "targetfoo",
            "raid3x",
        ] {
            assert_eq!(parse_unit_token(token), UnitTokenParse::Nobody, "{token}");
        }
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
            assert_eq!(parse_unit_token(token), UnitTokenParse::Unknown, "{token}");
        }
    }

    /// 12340's bases past 1.12.1's (`0x60abf0`): each parses, and names nobody until the app
    /// pushes its guid.
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
                parse_unit_token(token),
                UnitTokenParse::Unit { base, hops },
                "{token}"
            );
            assert_eq!(guids().resolve(token), Ok(None), "{token}");
        }
        assert_eq!(parse_unit_token("none"), UnitTokenParse::Nobody);
        assert_eq!(parse_unit_token("questnpctarget"), UnitTokenParse::Unknown);
    }

    #[test]
    fn a_recognised_token_naming_nobody_is_quiet() {
        for t in [
            "",
            "party",
            "party0",
            "party5",
            "partyX",
            "raid0",
            "raid4",
            "raidpet3",
            "playerfoo",
            "targetfoo",
            "partypet5",
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
