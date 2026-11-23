//! The unit-token resolver, token to guid (`0x60abf0`), over the client's state read at the call
//! through the lent view ([`super::super::LiveState::unit_base`]). Every compare folds ASCII case.
//! A token is a base, then hops: the bases are 12340's, in its compare order: `player`, `vehicle`,
//! `pet`, `target`, `partypetN`, `partyN`, `raidpetN`, `raidN`, `bossN`, `arenapetN`, `arenaN`,
//! `commentatorN`, `mouseover`, `focus`, then the exact `npc`, `questnpc` and `none`. Each hop
//! after it (`0x60aaa0`), behind an optional `-`, is `target` or `pet`. A token whose base names
//! nobody with text after it, whose text is not hops, or that no base matches goes to the name
//! lookup (`0x60a630`): a group member's or pet's name, then hops.

use std::borrow::Cow;
use std::collections::HashMap;

use super::super::live::read_live;

/// A fixed set of the resolver's tables, for a test or a capture fixture with no game behind the
/// VM: the guid globals, the group tables and, for each unit a token can reach, whether it is held
/// and whom it targets. A zero guid is nobody. The game's own are read at the call, never copied.
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
    /// The interact target, which `"npc"` names when no view is lent.
    pub npc: u64,
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

/// The rows of each numbered base's table at the call, which the name lookup and the unit-event
/// fan-out walk: the party's filled slots, the raid roster's rows, `GetNumArenaOpponents`
/// (`0x549630`) and the encounter's slots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UnitCounts {
    pub party: usize,
    pub raid: usize,
    pub arena: usize,
    pub boss: usize,
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

/// A numbered base by its 0-based row.
type Numbered = fn(u32) -> UnitBase;

/// Where a [`Tokens`] reads the tables.
#[derive(Clone, Copy)]
enum Src<'a> {
    /// The game's, through the lent view, at each read.
    Live,
    /// A test's or a capture fixture's.
    Fixture(&'a UnitGuids),
    /// The host's own read of the game's bases and rows, for a host resolving while it holds
    /// the state itself.
    Host(HostBases<'a>, UnitCounts),
    /// No game and no fixture: every table names nobody.
    Empty,
}

/// A host's read of a token base's guid, 0 for nobody.
pub type HostBases<'a> = &'a dyn Fn(UnitBase) -> u64;

/// One unit the name lookup asks the name of (`0x60a360`), by its token and its guid.
#[derive(Clone, Copy)]
pub(crate) struct NameAsk<'t> {
    pub(crate) token: &'t str,
    pub(crate) guid: u64,
    /// Whether the unit is us, whose name the local name buffer holds.
    pub(crate) ours: bool,
    pub(crate) with_realm: bool,
}

/// A numbered base's 0-based row from an index.
fn row(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

/// The resolver's tables read once for a call, with the rows each numbered base walks.
struct Tables<'a> {
    t: Cow<'a, UnitGuids>,
    counts: UnitCounts,
    /// Whether the game's, read through the lent view, which carries the interact target.
    live: bool,
}

impl<'a> Tables<'a> {
    /// `t` with the rows its own tables hold.
    fn of(t: Cow<'a, UnitGuids>, live: bool) -> Self {
        let counts = UnitCounts {
            party: t.party.iter().take_while(|&&m| m != 0).count(),
            raid: t.raid.len(),
            arena: t.arena_count.min(t.arena.len()),
            boss: t.boss.len(),
        };
        Self { t, counts, live }
    }

    /// The first `count` rows of a numbered base, in order.
    fn rows(&self, base: Numbered, count: usize) -> impl Iterator<Item = u64> + '_ {
        (0..count).map(move |i| self.t.table_base(base(row(i))))
    }

    fn in_party(&self, guid: u64) -> bool {
        guid != 0 && self.t.party.contains(&guid)
    }

    fn in_raid(&self, guid: u64) -> bool {
        guid != 0
            && self
                .rows(UnitBase::Raid, self.counts.raid)
                .any(|g| g == guid)
    }

    /// `0x512a30`: us, our pet, a party member or its pet, a raid member or its pet.
    fn is_member_or_pet(&self, guid: u64) -> bool {
        let raid = self.counts.raid;
        guid != 0
            && (self.t.table_base(UnitBase::Player) == guid
                || self.t.table_base(UnitBase::Pet) == guid
                || self.rows(UnitBase::Party, 4).any(|g| g == guid)
                || self.rows(UnitBase::PartyPet, 4).any(|g| g == guid)
                || self.rows(UnitBase::Raid, raid).any(|g| g == guid)
                || self.rows(UnitBase::RaidPet, raid).any(|g| g == guid))
    }

    /// The name lookup (`0x60a630`) over these tables: the name in [`split_name`]'s buffer against
    /// ours, then the raid roster's, else the party's, else the counted arena opponents'; then our
    /// pet's, and the same tables' pets. The first match's guid and the hops after the name.
    fn by_name<'t>(
        &self,
        token: &'t str,
        name_of: impl Fn(NameAsk<'_>) -> Option<String>,
    ) -> Option<(u64, Hops<'t>)> {
        let (buffer, with_realm, hops) = split_name(token);
        let player = self.t.table_base(UnitBase::Player);
        let named = |unit: &str, guid: u64| {
            guid != 0
                && name_of(NameAsk {
                    token: unit,
                    guid,
                    ours: guid == player,
                    with_realm,
                })
                .is_some_and(|n| same_name(buffer, &n))
        };
        let first = |prefix: &str, base: Numbered, count: usize| {
            self.rows(base, count)
                .enumerate()
                .find(|&(i, g)| named(&format!("{prefix}{}", i + 1), g))
                .map_or(0, |(_, g)| g)
        };
        let counts = self.counts;
        let mut guid = if named("player", player) {
            player
        } else if counts.raid != 0 {
            first("raid", UnitBase::Raid, counts.raid)
        } else if counts.party != 0 {
            first("party", UnitBase::Party, counts.party)
        } else {
            first("arena", UnitBase::Arena, counts.arena)
        };
        if guid == 0 {
            let pet = self.t.table_base(UnitBase::Pet);
            guid = if named("pet", pet) {
                pet
            } else if counts.raid != 0 {
                first("raidpet", UnitBase::RaidPet, counts.raid)
            } else if counts.party != 0 {
                first("partypet", UnitBase::PartyPet, counts.party)
            } else {
                first("arenapet", UnitBase::ArenaPet, counts.arena)
            };
        }
        (guid != 0).then_some((guid, hops))
    }
}

/// The resolver over the client's state at the call (`0x60abf0`): the game's through the lent
/// view, or, where no view lends it, a fixture's. The focus `[0xbd07d0]` is the VM's own.
#[derive(Clone, Copy)]
pub(crate) struct Tokens<'a> {
    src: Src<'a>,
    focus: u64,
}

impl<'a> Tokens<'a> {
    /// A fixture's tables where a test or a harness set one, else the game's through the lent
    /// view, which name nobody where no view lends them.
    pub(crate) fn new(fixture: Option<&'a UnitGuids>, focus: u64) -> Self {
        let src = fixture.map_or(Src::Live, Src::Fixture);
        Self { src, focus }
    }

    /// The host's own bases and rows, for a name lookup made while the host holds its state.
    pub(crate) fn host(bases: HostBases<'a>, counts: UnitCounts, focus: u64) -> Self {
        Self {
            src: Src::Host(bases, counts),
            focus,
        }
    }

    /// `fixture`'s tables alone, whatever view is lent.
    #[cfg(test)]
    pub(crate) fn of(fixture: &'a UnitGuids, focus: u64) -> Self {
        Self {
            src: Src::Fixture(fixture),
            focus,
        }
    }

    /// Our guid, 0 without the player object.
    pub(crate) fn player(&self) -> u64 {
        self.base_guid(UnitBase::Player)
    }

    /// The target global.
    pub(crate) fn target(&self) -> u64 {
        self.base_guid(UnitBase::Target)
    }

    /// The tables, read once for the call that asks.
    fn tables(&self) -> Tables<'a> {
        match self.src {
            Src::Live => match read_live(|l| l.unit_tables()).flatten() {
                Some(t) => Tables::of(Cow::Owned(t), true),
                None => Tables::of(Cow::Owned(UnitGuids::default()), false),
            },
            Src::Fixture(g) => Tables::of(Cow::Borrowed(g), false),
            Src::Host(bases, counts) => Tables {
                t: Cow::Owned(UnitGuids::from_bases(bases, counts)),
                counts,
                live: false,
            },
            Src::Empty => Tables::of(Cow::Owned(UnitGuids::default()), false),
        }
    }

    /// The table rows the walks cover, read at the call.
    pub(crate) fn counts(&self) -> UnitCounts {
        self.tables().counts
    }

    /// A held unit's `UNIT_FIELD_TARGET`, `None` for a guid that is no held unit (`0x60aaa0`).
    fn held_target(&self, guid: u64) -> Option<u64> {
        match self.src {
            Src::Fixture(g) => g.held.get(&guid).copied(),
            Src::Live | Src::Host(..) | Src::Empty => None,
        }
    }

    /// The guid `token` names (`0x60abf0`, with no target default), `None` for nobody, `Err` where
    /// the reference answers false: the empty token, or a name lookup that finds no one. A base
    /// whose unit is there, or that names nobody with no text after it, takes the hops; a hop to
    /// nobody is nobody. Otherwise the whole token is a name, `name_of` the name a unit goes by
    /// (`0x60a360`), `"Name-Realm"` with the realm asked for and known.
    pub(crate) fn resolve_named(
        &self,
        token: &str,
        name_of: impl Fn(NameAsk<'_>) -> Option<String>,
    ) -> Result<Option<u64>, ()> {
        if let Some(resolved) = self.resolve_live(token, &name_of) {
            return resolved;
        }
        let tokens = match self.src {
            Src::Live => Self {
                src: Src::Empty,
                ..*self
            },
            _ => *self,
        };
        tokens.resolve_held(token, name_of)
    }

    /// [`Self::resolve_named`] over the game's state, a read for the base and its hops and, for a
    /// name, one for the tables and one for the hops after it; `None` where no view lends it.
    pub(crate) fn resolve_live(
        &self,
        token: &str,
        name_of: impl Fn(NameAsk<'_>) -> Option<String>,
    ) -> Option<Result<Option<u64>, ()>> {
        if !matches!(self.src, Src::Live) {
            return None;
        }
        let somebody = |guid: u64| Some(guid).filter(|&g| g != 0);
        match parse_unit_token(token) {
            UnitTokenParse::Empty => return Some(Err(())),
            UnitTokenParse::Nobody => return Some(Ok(None)),
            UnitTokenParse::Unit { base, hops } => {
                let end = read_live(|l| l.unit_token(base, hops, self.focus)).flatten()?;
                if let Some(end) = end {
                    return Some(Ok(somebody(end)));
                }
            }
            UnitTokenParse::Name => {}
        }
        let tables = Tables::of(Cow::Owned(read_live(|l| l.unit_tables()).flatten()?), true);
        let Some((guid, hops)) = tables.by_name(token, name_of) else {
            return Some(Err(()));
        };
        let end = if hops.is_empty() {
            Some(guid)
        } else {
            read_live(|l| l.unit_hops(guid, hops)).flatten()?
        };
        Some(end.map(somebody).ok_or(()))
    }

    /// [`Self::resolve_named`] over a fixture's or the host's tables.
    fn resolve_held(
        &self,
        token: &str,
        name_of: impl Fn(NameAsk<'_>) -> Option<String>,
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

    /// The guid `token` names with no name lookup: `None` for nobody, `Err` where the lookup would
    /// run.
    #[cfg(test)]
    pub(crate) fn resolve(&self, token: &str) -> Result<Option<u64>, ()> {
        self.resolve_named(token, |_| None)
    }

    /// The unit a base names, read at the call; 0 for nobody.
    pub(crate) fn base_guid(&self, base: UnitBase) -> u64 {
        match base {
            UnitBase::Focus => self.focus,
            // The interact target read at the call (`0x60af67`); with none held, nobody.
            UnitBase::Npc => self.npc_guid(),
            // The open quest window's giver, read live (`0x6081b0`).
            UnitBase::QuestNpc => read_live(|l| l.quest_npc()).flatten().unwrap_or(0),
            // The spectated players are listed in commentator mode alone, which benilla never
            // asks for (`CMSG_COMMENTATOR_ENABLE`), so the list is empty.
            UnitBase::Commentator(_) => 0,
            base => match self.src {
                Src::Live => read_live(|l| l.unit_base(base)).flatten().unwrap_or(0),
                Src::Fixture(g) => g.table_base(base),
                Src::Host(bases, _) => bases(base),
                Src::Empty => 0,
            },
        }
    }

    /// Whether `guid` holds one of the party's slots.
    pub(crate) fn in_party(&self, guid: u64) -> bool {
        self.tables().in_party(guid)
    }

    /// Whether `guid` is on the raid roster.
    pub(crate) fn in_raid(&self, guid: u64) -> bool {
        self.tables().in_raid(guid)
    }

    /// The group test a dead-looking unit and `UnitInRange` waive or ask (`0x512a30`): us, our pet,
    /// a party member or its pet, a raid member or its pet.
    pub(crate) fn is_member_or_pet(&self, guid: u64) -> bool {
        guid != 0 && self.tables().is_member_or_pet(guid)
    }

    /// The hops from `guid` (`0x60aaa0`): `None` where they fail, which sends the token to the name
    /// lookup; a hop to nobody goes on from nobody. The game's are read in one view.
    pub(crate) fn follow(&self, mut guid: u64, hops: Hops<'_>) -> Option<u64> {
        if matches!(self.src, Src::Live) {
            if let Some(end) = read_live(|l| l.unit_hops(guid, hops)).flatten() {
                return end;
            }
        }
        let me = Some(self.player()).filter(|&g| g != 0);
        for hop in hops {
            guid = match hop.ok()? {
                Hop::Target if Some(guid) == me => self.target(),
                Hop::Target => self.held_target(guid).unwrap_or(0),
                Hop::Pet => {
                    let open = Some(guid) == me
                        || {
                            let t = self.tables();
                            t.in_party(guid) || t.in_raid(guid)
                        }
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

    /// The name lookup (`0x60a630`) over the tables read once: the first match's guid and the
    /// hops after the name.
    pub(crate) fn by_name<'t>(
        &self,
        token: &'t str,
        name_of: impl Fn(NameAsk<'_>) -> Option<String>,
    ) -> Option<(u64, Hops<'t>)> {
        self.tables().by_name(token, name_of)
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
        let tables = self.tables();
        let t = &tables.t;
        let player = t.table_base(UnitBase::Player);
        if player == guid {
            out.push("player".into());
        }
        if t.table_base(UnitBase::Vehicle) == guid {
            out.push("vehicle".into());
        }
        if player != 0 && t.table_base(UnitBase::Pet) == guid {
            out.push("pet".into());
        }
        let counts = tables.counts;
        let rows: [(&str, Numbered, usize); 4] = [
            ("party", UnitBase::Party, 4),
            ("partypet", UnitBase::PartyPet, 4),
            ("raid", UnitBase::Raid, counts.raid),
            ("raidpet", UnitBase::RaidPet, counts.raid),
        ];
        for (prefix, base, count) in rows {
            for (i, g) in tables.rows(base, count).enumerate() {
                if g == guid {
                    out.push(format!("{prefix}{}", i + 1));
                }
            }
        }
        // Over the counted slots, an opponent tested before its pet, the first match ending it.
        for i in 0..counts.arena {
            if t.table_base(UnitBase::Arena(row(i))) == guid {
                out.push(format!("arena{}", i + 1));
                break;
            }
            if t.table_base(UnitBase::ArenaPet(row(i))) == guid {
                out.push(format!("arenapet{}", i + 1));
                break;
            }
        }
        // The first slot only, as the reference's loop breaks on a match.
        if let Some(i) = tables
            .rows(UnitBase::Boss, counts.boss)
            .position(|g| g == guid)
        {
            out.push(format!("boss{}", i + 1));
        }
        if t.table_base(UnitBase::Target) == guid {
            out.push("target".into());
        }
        if self.focus == guid {
            out.push("focus".into());
        }
        // The fan-out's compare is against the interact target itself (`0x60bece`).
        let npc = if tables.live { t.npc } else { self.npc_guid() };
        if npc == guid {
            out.push("npc".into());
        }
        out
    }

    /// The `"npc"` guid: the interact target read at the call (`0x60af67`), 0 with none held;
    /// with no view lent, a fixture's.
    pub(crate) fn npc_guid(&self) -> u64 {
        match read_live(|l| l.interact_target()) {
            Some(held) => held.unwrap_or(0),
            None => match self.src {
                Src::Fixture(g) => g.npc,
                _ => 0,
            },
        }
    }

    /// The guids a fixture's chains end on, each once: each held unit's target, the focus and the
    /// vehicle. The game's are walked by the app.
    #[cfg(any(test, feature = "fixtures"))]
    pub(crate) fn fixture_chain_ends(&self) -> Vec<u64> {
        let Src::Fixture(g) = self.src else {
            return Vec::new();
        };
        let mut ends: Vec<u64> = g
            .held
            .values()
            .copied()
            .chain([self.focus, g.vehicle])
            .filter(|&guid| guid != 0 && guid != crate::script::focus::NOBODY)
            .collect();
        ends.sort_unstable();
        ends.dedup();
        ends
    }
}

impl UnitGuids {
    /// The host's read of every base and row, as one table.
    fn from_bases(bases: HostBases<'_>, counts: UnitCounts) -> Self {
        let rows = |base: Numbered, n: usize| (0..n).map(|i| bases(base(row(i)))).collect();
        Self {
            player: bases(UnitBase::Player),
            pet: bases(UnitBase::Pet),
            vehicle: bases(UnitBase::Vehicle),
            target: bases(UnitBase::Target),
            party: std::array::from_fn(|i| bases(UnitBase::Party(row(i)))),
            party_pets: std::array::from_fn(|i| bases(UnitBase::PartyPet(row(i)))),
            raid: rows(UnitBase::Raid, counts.raid),
            raid_pets: rows(UnitBase::RaidPet, counts.raid),
            boss: rows(UnitBase::Boss, counts.boss),
            arena: std::array::from_fn(|i| bases(UnitBase::Arena(row(i)))),
            arena_pets: std::array::from_fn(|i| bases(UnitBase::ArenaPet(row(i)))),
            arena_count: counts.arena,
            ..Self::default()
        }
    }

    /// A fixture's unit for a base its tables hold; the VM's and the view's own bases are
    /// [`Tokens`]'.
    fn table_base(&self, base: UnitBase) -> u64 {
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
            UnitBase::PartyPet(row) => at(&self.party_pets, row),
            UnitBase::Party(row) => at(&self.party, row),
            UnitBase::RaidPet(row) => at(&self.raid_pets, row),
            UnitBase::Raid(row) => at(&self.raid, row),
            UnitBase::Boss(row) => at(&self.boss, row),
            UnitBase::Mouseover if !self.held.contains_key(&self.mouseover) => 0,
            UnitBase::Mouseover => self.mouseover,
            UnitBase::Vehicle => self.vehicle,
            UnitBase::Arena(row) => at(&self.arena, row),
            UnitBase::ArenaPet(row) => at(&self.arena_pets, row),
            UnitBase::Npc | UnitBase::Focus | UnitBase::QuestNpc | UnitBase::Commentator(_) => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture's tables with no focus, as the VM's tests read them.
    impl UnitGuids {
        fn resolve(&self, token: &str) -> Result<Option<u64>, ()> {
            Tokens::of(self, 0).resolve(token)
        }

        fn resolve_named(
            &self,
            token: &str,
            name_of: impl Fn(&str, bool) -> Option<String>,
        ) -> Result<Option<u64>, ()> {
            Tokens::of(self, 0).resolve_named(token, |a| name_of(a.token, a.with_realm))
        }

        fn guid_of(&self, token: &str) -> Option<u64> {
            Tokens::of(self, 0).guid_of(token)
        }

        fn tokens_naming(&self, guid: u64) -> Vec<String> {
            Tokens::of(self, 0).tokens_naming(guid)
        }
    }

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
        let g = guids();
        assert_eq!(Tokens::of(&g, P1).guid_of("focus"), Some(P1));
        assert_eq!(Tokens::of(&g, P1).guid_of("focustarget"), Some(ME));
        assert_eq!(g.guid_of("focus"), None, "no focus, nobody");
        assert_eq!(
            Tokens::of(&g, MOB).tokens_naming(MOB),
            ["target", "focus", "npc"]
        );
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
        assert!(Tokens::of(&g, 0).fixture_chain_ends().contains(&CART));
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
        let mut loan_v = view(0);
        crate::script::live::lending(&mut loan_v, || {
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
        });
        let mut loan_v = view(PET_OF_ANYONE);
        crate::script::live::lending(&mut loan_v, || {
            assert_eq!(at("targetpet"), Some(MOB_PET), "any unit's under the flag");
            assert_eq!(at("petpet"), None);
        });
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
