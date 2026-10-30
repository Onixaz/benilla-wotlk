//! The world cursor's classifier: the hovered unit, GameObject or corpse resolves to a
//! [`CursorKind`] as the reference's world-frame classifier does: the NPC service ladder, else
//! loot, skin or attack by state, each grayed by its own range gate. Loot rights never gray; they
//! decide whether the loot cursor shows at all.

use bevy::prelude::*;

use crate::net::{ObjectStore, Reputations, SelfPlayer};

use super::ring::Factions;
use super::{go_is_nearest, Hovered, HoveredObject};

/// The world cursor modes benilla shows, named by the reference's mode table, whose strings are the
/// `Interface\Cursor\<Name>.blp` stems.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CursorKind {
    Point,
    Attack,
    Speak,
    /// Pickup(8): the vendor's pouch and the loot leg's base mode.
    Pickup,
    /// LootAll(16): the loot leg's triple pouch while the effective auto-loot is on; the vendor
    /// pouch never triples.
    LootAll,
    /// Interact(5): the gear, for a GameObject with no data-named cursor and for the innkeeper.
    Interact,
    Buy,
    /// Inspect(7): the UI's Ctrl-hover cursor (`ShowInspectCursor`) and the world cursor over a
    /// readable TEXT(9) GameObject.
    Inspect,
    Trainer,
    Taxi,
    Skin,
    /// SkinHorde(20) and SkinAlliance(21): the enemy faction's insignia over a player's body, picked
    /// by the player's own side ([`insignia_cursor`]).
    SkinHorde,
    SkinAlliance,
    /// Repair(17): never set by the world classifier; the UI's repair-mode base cursor
    /// (`ShowRepairCursor`).
    Repair,
    /// Mail(15): a MAILBOX(19) or type-28 GameObject.
    Mail,
    /// Mine(11): a GameObject whose lock's first `LockType` is Mining (3), named by LockType.dbc.
    Mine,
    /// GatherHerbs(13): the same, for Herbalism (2).
    GatherHerbs,
    /// PickLock(14): the same, for Pick Lock (1); never grayed, since the reference skips the
    /// usable gate for `LockType.Id == 1`.
    PickLock,
    /// Cast(2): the spell-targeting cursor, which pre-empts the whole object classifier while a
    /// spell awaits a target; the classifier here never sets it.
    Cast,
    /// RepairNPC(18): a repairer's (`0x4f7a50`).
    RepairNpc,
    /// Innkeeper(22).
    Innkeeper,
    /// Quest(23), QuestRepeatable(24) and QuestTurnIn(25): a questgiver by its status
    /// (`0x744640`).
    Quest,
    QuestRepeatable,
    QuestTurnIn,
    /// vehichleCursor(26), the table's own spelling: a player vehicle with a free seat.
    Vehicle,
    /// A creature cursor name outside the mode table, drawn from its own file (mode `0x35`,
    /// `0x715f20`).
    Named(&'static str),
}

/// The 3.3.5a mode table's names (`0xad280c`), 1-based modes, `Unable<Name>` at `+0x1a`.
const CURSOR_TABLE: [(&str, CursorKind); 26] = [
    ("Point", CursorKind::Point),
    ("Cast", CursorKind::Cast),
    ("Buy", CursorKind::Buy),
    ("Attack", CursorKind::Attack),
    ("Interact", CursorKind::Interact),
    ("Speak", CursorKind::Speak),
    ("Inspect", CursorKind::Inspect),
    ("Pickup", CursorKind::Pickup),
    ("Taxi", CursorKind::Taxi),
    ("Trainer", CursorKind::Trainer),
    ("Mine", CursorKind::Mine),
    ("Skin", CursorKind::Skin),
    ("GatherHerbs", CursorKind::GatherHerbs),
    ("PickLock", CursorKind::PickLock),
    ("Mail", CursorKind::Mail),
    ("LootAll", CursorKind::LootAll),
    ("Repair", CursorKind::Repair),
    ("RepairNPC", CursorKind::RepairNpc),
    // Item(19), the use-item cursor; no world unit names it.
    ("Item", CursorKind::Interact),
    ("SkinHorde", CursorKind::SkinHorde),
    ("SkinAlliance", CursorKind::SkinAlliance),
    ("Innkeeper", CursorKind::Innkeeper),
    ("Quest", CursorKind::Quest),
    ("QuestRepeatable", CursorKind::QuestRepeatable),
    ("QuestTurnIn", CursorKind::QuestTurnIn),
    ("vehichleCursor", CursorKind::Vehicle),
];

/// A creature's cursor name as the cursor-name lookup (`0x616290`) reads it: a table name to its
/// mode, any other name to its own file.
fn cursor_from_name(name: &str) -> CursorKind {
    CURSOR_TABLE
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map_or_else(|| CursorKind::Named(intern(name)), |(_, kind)| *kind)
}

/// A creature cursor name kept for the life of the process; the set is the templates' few names.
fn intern(name: &str) -> &'static str {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(n) = names.get(name) {
        return n;
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(leaked);
    leaked
}

impl CursorKind {
    /// The cursor's BLP stem in `Interface\Cursor\`.
    fn name(self) -> &'static str {
        match self {
            CursorKind::Point => "Point",
            CursorKind::Attack => "Attack",
            CursorKind::Speak => "Speak",
            CursorKind::Pickup => "Pickup",
            CursorKind::LootAll => "LootAll",
            CursorKind::Interact => "Interact",
            CursorKind::Buy => "Buy",
            CursorKind::Inspect => "Inspect",
            CursorKind::Trainer => "Trainer",
            CursorKind::Taxi => "Taxi",
            CursorKind::Skin => "Skin",
            CursorKind::SkinHorde => "SkinHorde",
            CursorKind::SkinAlliance => "SkinAlliance",
            CursorKind::Repair => "Repair",
            CursorKind::Mail => "Mail",
            CursorKind::Mine => "Mine",
            CursorKind::GatherHerbs => "GatherHerbs",
            CursorKind::PickLock => "PickLock",
            CursorKind::Cast => "Cast",
            CursorKind::RepairNpc => "RepairNPC",
            CursorKind::Innkeeper => "Innkeeper",
            CursorKind::Quest => "Quest",
            CursorKind::QuestRepeatable => "QuestRepeatable",
            CursorKind::QuestTurnIn => "QuestTurnIn",
            CursorKind::Vehicle => "vehichleCursor",
            CursorKind::Named(name) => name,
        }
    }
}

/// This frame's world cursor, written by [`classify_cursor`]; `unable` picks the grayed
/// `Unable<Name>` twin (the reference's `mode + 0x1a`).
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct WorldCursor {
    pub(crate) kind: CursorKind,
    pub(crate) unable: bool,
}

impl Default for WorldCursor {
    fn default() -> Self {
        Self {
            kind: CursorKind::Point,
            unable: false,
        }
    }
}

impl WorldCursor {
    /// The BLP stem (`Attack`, `UnableAttack`), the key the platform cursor caches use.
    pub(crate) fn stem(&self) -> String {
        if self.unable && self.kind != CursorKind::Point {
            format!("Unable{}", self.kind.name())
        } else {
            self.kind.name().to_string()
        }
    }
}

/// `UNIT_NPC_FLAGS` bits, as the 12340 right-click ladder (`0x6ddbb0`) and the cursor classifier
/// (`0x4f7a50`) test them.
pub(crate) mod npc_flags {
    pub const GOSSIP: u32 = 0x1;
    pub const QUESTGIVER: u32 = 0x2;
    pub const TRAINER: u32 = 0x10;
    /// The vendor bit the ladders test; the ammo, food, poison and reagent kinds ride beside it.
    pub const VENDOR: u32 = 0x80;
    pub const REPAIR: u32 = 0x1000;
    pub const FLIGHTMASTER: u32 = 0x2000;
    pub const SPIRITHEALER: u32 = 0x4000;
    pub const SPIRITGUIDE: u32 = 0x8000;
    pub const INNKEEPER: u32 = 0x1_0000;
    pub const BANKER: u32 = 0x2_0000;
    pub const PETITIONER: u32 = 0x4_0000;
    pub const TABARDDESIGNER: u32 = 0x8_0000;
    pub const BATTLEMASTER: u32 = 0x10_0000;
    pub const AUCTIONEER: u32 = 0x20_0000;
    pub const STABLEMASTER: u32 = 0x40_0000;
    pub const GUILD_BANKER: u32 = 0x80_0000;
    pub const SPELLCLICK: u32 = 0x100_0000;
    pub const MAILBOX: u32 = 0x400_0000;

    /// The guild master test (`0x4f5f80`): petitioner and tabard designer both.
    pub fn is_guild_master(flags: u32) -> bool {
        flags & PETITIONER != 0 && flags & TABARDDESIGNER != 0
    }
}

pub(super) const UNIT_FLAG_SKINNABLE: u32 = 0x0400_0000;
const UNIT_FLAG_PACIFIED: u32 = 0x0002_0000;

/// The spell the skin leg casts at a dead skinnable unit, shared by the cursor (`0x4f7e18`) and the
/// click (`0x7313a3`): the skinning spell lookup's (`0x53bce0`) slot for it, refused for a player
/// that the assist check passes (`0x7313c5`, `0x4f7e3a`), so a friendly body takes no skin leg.
pub(super) fn unit_skin_spell(
    learned: &crate::ui_action::LearnedAbilities,
    store: &ObjectStore,
    self_store: Option<&ObjectStore>,
    factions: Option<&Factions>,
    reputations: &Reputations,
    party: &dyn super::GroupMembers,
    names: Option<&crate::names::NameCache>,
) -> Option<u32> {
    skin_leg_spell(
        learned,
        crate::ui_action::SkinSubject::of(store, names),
        || super::can_assist(Some(store), factions, reputations, party, self_store, names),
    )
}

/// [`unit_skin_spell`] past the object reads: `can_assist` is asked of a player only.
fn skin_leg_spell(
    learned: &crate::ui_action::LearnedAbilities,
    subject: crate::ui_action::SkinSubject,
    can_assist: impl FnOnce() -> bool,
) -> Option<u32> {
    let spell = learned.skinning_spell(subject)?;
    (subject != crate::ui_action::SkinSubject::PlayerOrCorpse || !can_assist()).then_some(spell)
}

/// The skin leg's cursor: a player's body takes the insignia by the player's side (`0x4f7e6f`), a
/// creature its skinning type (`0x715e50`, `0x4f7ece`): Skin(12) for leather, GatherHerbs(13) for
/// herb, Mine(11) for rock and bolts.
fn skin_cursor(
    store: &ObjectStore,
    names: Option<&crate::names::NameCache>,
    side: Option<u8>,
) -> CursorKind {
    use crate::ui_action::SkinSubject;
    match SkinSubject::of(store, names) {
        SkinSubject::PlayerOrCorpse => insignia_cursor(side),
        SkinSubject::Creature { type_flags } => skin_cursor_kind(type_flags),
    }
}

/// The player's own PvP side (`0x6d6e90`): his race's template group `& 4` is 0 (Horde), else
/// `& 2` is 1 (Alliance), else none.
fn pvp_faction_index(self_store: &ObjectStore, factions: Option<&Factions>) -> Option<u8> {
    let template = factions?
        .catalog()
        .race_template(self_store.0.unit_race()?)?;
    if template.group_mask & 4 != 0 {
        Some(0)
    } else if template.group_mask & 2 != 0 {
        Some(1)
    } else {
        None
    }
}

/// The insignia cursor, the enemy side's emblem by the player's own (`0x9f9864`, its `Unable`
/// twins at `0x9f986c`): Horde (0) sees SkinAlliance(21), Alliance (1) SkinHorde(20). The
/// reference indexes the tables at -1 for no side; that reads Point here.
fn insignia_cursor(side: Option<u8>) -> CursorKind {
    match side {
        Some(0) => CursorKind::SkinAlliance,
        Some(1) => CursorKind::SkinHorde,
        _ => CursorKind::Point,
    }
}

/// The skin legs' reach: the skinning spell's max range squared against the centre distance
/// squared, unable only when strictly beyond it, so a NaN passes (`0x4f7e78`, `0x4f812e`).
fn in_skin_reach(max_range: f32, dist_sq: f32) -> bool {
    !matches!(
        (max_range * max_range).partial_cmp(&dist_sq),
        Some(std::cmp::Ordering::Less)
    )
}

fn skin_cursor_kind(type_flags: u32) -> CursorKind {
    match crate::ui_action::skinning_type(type_flags) {
        1 => CursorKind::GatherHerbs,
        2 | 3 => CursorKind::Mine,
        _ => CursorKind::Skin,
    }
}

/// The hovered unit is one the unit dispatcher forks to its attack arm for: alive or feigning,
/// refused by the interact check and passed by the attack check. The sword also needs the player's
/// own legs ([`player_attack_legs`]), which the arm tests again inside.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct AttackFork(pub(crate) bool);

/// The player's legs of the classifier's attack leg, in its order: not dead or a ghost, not
/// mounted, not pacified and in control. Any failure clears to Point.
pub(crate) fn player_attack_legs(own: &ObjectStore, control_lost: bool) -> bool {
    !own.0.is_dead_or_ghost()
        && own.0.unit_mount_display_id() == 0
        && own.0.unit_flags() & UNIT_FLAG_PACIFIED == 0
        && !control_lost
}

/// NPC-service range, squared: gray beyond 5.5556 yd. The NPC windows' walk-away close uses the
/// same constant.
pub(crate) const SERVICE_RANGE_SQ: f32 = 30.864;

/// A unit's interact reach, squared: its combat reach plus 4 (`0x4f7b0f`, the right-click's
/// `0x6ddcc6`), boundary-inclusive.
pub(crate) fn unit_interact_reach_sq(unit: &ObjectStore) -> f32 {
    let reach = unit.0.unit_combat_reach() + 4.0;
    reach * reach
}
/// Attack's range, squared: gray beyond a fixed 10.45 yd, not the melee reach.
const ATTACK_RANGE_SQ: f32 = 109.2025;
/// Melee interact reach `max(reachA + reachB + 1.333, 5.0)`: 5.0 is a floor, so large creatures
/// reach farther. Gates skin and loot, center to center, boundary-inclusive.
pub(super) const MELEE_OFFSET: f32 = 1.333_33;
pub(super) const MELEE_FLOOR: f32 = 5.0;

pub(super) fn melee_reach(target: &ObjectStore, me: &ObjectStore) -> f32 {
    (target.0.unit_combat_reach() + me.0.unit_combat_reach() + MELEE_OFFSET).max(MELEE_FLOOR)
}

/// A corpse's interact reach, squared: a flat 5 yd, since a corpse has no combat reach and the
/// melee formula falls to its floor. The loot check and the `CMSG_LOOT` sender both compare against
/// 25.0.
pub(super) const CORPSE_INTERACT_RANGE_SQ: f32 = 25.0;

/// `GAMEOBJECT_TYPE_GENERIC`: decoration whose highlightable slot is constant false, so it never
/// shows an interact cursor.
pub(crate) const GO_TYPE_GENERIC: i32 = 5;
/// The transport family, TRANSPORT(11), MAP_OBJECT(14) and MO_TRANSPORT(15): both strategy slots
/// are constant false (`+0x14`), so a boat or elevator never shows a cursor, a tooltip or a
/// right-click use.
const GO_TYPE_TRANSPORT: i32 = 11;
const GO_TYPE_MAP_OBJECT: i32 = 14;
const GO_TYPE_MO_TRANSPORT: i32 = 15;
/// The marker set, SPELL_FOCUS(8), DUEL_ARBITER(16), FISHINGHOLE(25) and AURA_GENERATOR(30):
/// mouseover slot `+0xc` constant true, highlightable slot `+0x14` constant false. An anvil or a
/// duel flag tooltips and brightens but shows the plain pointer and ignores the right-click
/// (`OnUse` asks `+0x14`).
const GO_TYPE_SPELL_FOCUS: i32 = 8;
const GO_TYPE_DUEL_ARBITER: i32 = 16;
const GO_TYPE_FISHINGHOLE: i32 = 25;
const GO_TYPE_AURA_GENERATOR: i32 = 30;
/// The highest type the type factory's jump table covers; 21 GUARDPOST has no case and takes
/// `default:` with every out-of-range type.
const GO_TYPE_MAX: i32 = 30;
const GO_TYPE_GUARDPOST: i32 = 21;

/// Whether the type factory gives this type its `default:` strategy: a static placeholder whose
/// `+0x14` and `+0xc` both answer false, so the object shows nothing. No 1.12 data sends such a
/// type; the flag predicate alone would pass it.
fn strategy_is_default(type_id: i32) -> bool {
    type_id != GO_TYPE_GUILD_BANK
        && (type_id == GO_TYPE_GUARDPOST || !(0..=GO_TYPE_MAX).contains(&type_id))
}
/// `GAMEOBJECT_TYPE_TEXT` (9), a readable book or plaque: the Inspect magnifier.
pub(crate) const GO_TYPE_TEXT: i32 = 9;
/// MAILBOX(19) and type 28 show Mail; no 1.12 data uses type 28. RITUAL(18) shows Mail here, where
/// the reference gives it the base cursor: its own vtable keeps.
const GO_TYPE_RITUAL: i32 = 18;
pub(super) const GO_TYPE_MAILBOX: i32 = 19;
const GO_TYPE_28: i32 = 28;
/// `GAMEOBJECT_TYPE_GUILD_BANK` (34), a guild vault: 3.3.5a gives it a strategy of its own
/// (`0x712ac0`), whose cursor is Buy, grayed out of reach (`0x70b910`).
pub(super) const GO_TYPE_GUILD_BANK: i32 = 34;
/// Every other type's interact range: the NPC-service reach, standing in for the reference's
/// per-type strategy constant (`[strat+0xc]`, compared squared by `usable`; base 5.0, overridden by
/// several types), which is not transcribed.
const GO_INTERACT_RANGE_SQ: f32 = SERVICE_RANGE_SQ;
/// `GAMEOBJECT_TYPE_FISHINGNODE` (17), the bobber: its strategy overrides only the highlightable
/// slot, and its interact range is 100 yd.
pub(crate) const GO_TYPE_FISHINGNODE: i32 = 17;
/// `GAMEOBJECT_TYPE_CHAIR` (7): its own range predicate accepts within 3.0 yd.
pub(crate) const GO_TYPE_CHAIR: i32 = 7;
/// `GAMEOBJECT_TYPE_BARBER_CHAIR` (32): a chair (`0x70c7f0` builds on the chair's `0x70c6a0`), so
/// its reach is the chair's 3.0 yd.
pub(crate) const GO_TYPE_BARBER_CHAIR: i32 = 32;
pub(super) fn go_interact_range_sq(type_id: i32) -> f32 {
    match type_id {
        GO_TYPE_FISHINGNODE => 100.0 * 100.0,
        // Measured to the object; the reference and vmangos (`GameObject.cpp:2231-2236`) measure to
        // the nearest seat, so only a multi-seat bench differs, by up to half its span.
        GO_TYPE_CHAIR | GO_TYPE_BARBER_CHAIR => 9.0,
        _ => GO_INTERACT_RANGE_SQ,
    }
}
/// `GameObjectFlags` IN_USE (`0x1`) and NO_INTERACT (`0x10`), which the reference rejects together.
const GO_FLAG_IN_USE_OR_NO_INTERACT: u32 = 0x11;
/// `GO_FLAG_INTERACT_COND`: usable only while the per-player activate bit is set, the quest gate.
const GO_FLAG_INTERACT_COND: u32 = 0x4;
/// `GO_DYNFLAG_LO_ACTIVATE` in `GAMEOBJECT_DYN_FLAGS`, set per player by the server
/// (`GameObject::ActivateToQuest`); read only under `INTERACT_COND`.
const GO_DYNFLAG_ACTIVATE: u32 = 0x1;

/// The types whose highlightable slot `+0x14` is constant false, so the shared gate never runs.
fn strategy_never_highlightable(type_id: i32) -> bool {
    strategy_is_default(type_id)
        || matches!(
            type_id,
            GO_TYPE_GENERIC
                | GO_TYPE_TRANSPORT
                | GO_TYPE_MAP_OBJECT
                | GO_TYPE_MO_TRANSPORT
                | GO_TYPE_SPELL_FOCUS
                | GO_TYPE_DUEL_ARBITER
                | GO_TYPE_FISHINGHOLE
                | GO_TYPE_AURA_GENERATOR
                | GO_TYPE_AUCTIONHOUSE
                | GO_TYPE_CAPTURE_POINT
        )
}

/// `GAMEOBJECT_TYPE_AUCTIONHOUSE` (20): both slots false, so it shows nothing and `OnUse` returns
/// unsent; the auctioneer players click is a unit.
const GO_TYPE_AUCTIONHOUSE: i32 = 20;
/// `GAMEOBJECT_TYPE_CAPTURE_POINT` (29): never a cursor, but a tooltip and brighten when its
/// highlight column `data[19]` is set, GENERIC's rule. No 1.12 data uses it.
const GO_TYPE_CAPTURE_POINT: i32 = 29;
/// Inputs of the two types whose `+0x14` is a predicate of its own rather than the reference or a
/// constant; the caller resolves them, and each is ignored for every other type.
#[derive(Clone, Copy, Default)]
pub(crate) struct GoOverrides {
    /// FISHINGNODE: the local player is channeling at this bobber.
    pub(crate) channel_owned: bool,
    /// MEETINGSTONE: this stone's area is the one we are queued at.
    pub(crate) meeting_stone_queued: bool,
}

/// `GAMEOBJECT_TYPE_MEETINGSTONE` (23): its `+0x14` only compares `template.data[2]` with the
/// queued area; it never calls the shared gate.
pub(super) const GO_TYPE_MEETINGSTONE: i32 = 23;

/// Whether a GameObject shows an interact cursor at all: the shared gate plus the per-type
/// overrides. An INTERACT_COND object passes only while the server has set its per-player activate
/// bit: the quest gate. The FISHINGNODE override compares our channel object, not `CREATED_BY`,
/// then runs the shared gate.
fn highlightable_flags(
    type_id: i32,
    flags: u32,
    dyn_flags: u32,
    reaction: Option<u8>,
    overrides: GoOverrides,
) -> bool {
    if strategy_never_highlightable(type_id) {
        return false;
    }
    if type_id == GO_TYPE_FISHINGNODE && !overrides.channel_owned {
        return false;
    }
    // MEETINGSTONE's slot replaces the shared gate, so none of the terms below apply to it.
    if type_id == GO_TYPE_MEETINGSTONE {
        return !overrides.meeting_stone_queued;
    }
    // The faction term: the GameObject's reaction toward us must beat hostile. An unresolved
    // reaction passes, so a data gap never blanks the world.
    let ordinary_faction_ok = |r: Option<u8>| r.is_none_or(|r| r > 1);
    if type_id == GO_TYPE_TRAP {
        // TRAP inverts it: highlightable only to whoever it is hostile to.
        if !reaction.is_none_or(|r| r == 1) {
            return false;
        }
    } else if !ordinary_faction_ok(reaction) {
        return false;
    }
    if flags & GO_FLAG_IN_USE_OR_NO_INTERACT != 0 {
        return false;
    }
    if flags & GO_FLAG_INTERACT_COND != 0 && dyn_flags & GO_DYNFLAG_ACTIVATE == 0 {
        return false;
    }
    true
}

/// `GAMEOBJECT_DYNFLAG` `0x4`, which the use check refuses.
const GO_DYNFLAG_NO_USE: u32 = 0x4;

/// The shared use check (`0x7112a0`) of an ordinary GameObject type: a reaction toward us above
/// hostile (an unresolved one passes), not in use or uninteractable, and an interact-condition
/// object only while activated for us. The trap arm and the creator-only rule are not applied.
pub(crate) fn go_can_use(flags: u32, dyn_flags: u32, reaction: Option<u8>) -> bool {
    reaction.is_none_or(|r| r > 1)
        && dyn_flags & GO_DYNFLAG_NO_USE == 0
        && flags & GO_FLAG_IN_USE_OR_NO_INTERACT == 0
        && (flags & GO_FLAG_INTERACT_COND == 0 || dyn_flags & GO_DYNFLAG_ACTIVATE != 0)
}

/// `GAMEOBJECT_TYPE_TRAP` (6), the one type whose faction term is inverted.
const GO_TYPE_TRAP: i32 = 6;

/// A GameObject's reaction toward us on the 1/3/4 scale: the reference's chain, the GameObject's
/// template toward the player's. No faction is neutral; `None` when unresolvable, which callers
/// pass. Only the template comparator runs: every shipped GameObject faction (114, 35, 14 and 1375)
/// resolves through it, so the reference's reputation arm and a `GAMEOBJECT_CREATED_BY` creator's
/// reaction are not applied.
pub(crate) fn go_reaction(
    factions: Option<&Factions>,
    go_faction: u32,
    self_store: Option<&ObjectStore>,
) -> Option<u8> {
    if go_faction == 0 {
        return Some(benilla_formats::Reaction::Neutral as u8);
    }
    let catalog = factions?.catalog();
    let go_tpl = catalog.template(go_faction)?;
    let self_tpl = catalog.template(self_store?.0.unit_faction_template()?)?;
    Some(go_tpl.reaction_toward(self_tpl) as u8)
}

/// Whether a picked object becomes the mouseover at all (`[vtbl+0x54]`): false publishes the null
/// mouseover, so no tooltip, brighten or cursor. For a GameObject it is the type's slot `+0xc`, not
/// the cursor's slot. `highlight_column` is GENERIC's `data[1]` or CAPTURE_POINT's `data[19]`;
/// `None`, a template not yet answered, reads eligible.
pub(crate) fn mouseover_eligible(
    type_id: i32,
    flags: u32,
    dyn_flags: u32,
    highlight_column: Option<bool>,
    reaction: Option<u8>,
    overrides: GoOverrides,
) -> bool {
    match type_id {
        GO_TYPE_TRANSPORT | GO_TYPE_MAP_OBJECT | GO_TYPE_MO_TRANSPORT => false,
        GO_TYPE_SPELL_FOCUS
        | GO_TYPE_DUEL_ARBITER
        | GO_TYPE_FISHINGHOLE
        | GO_TYPE_AURA_GENERATOR => true,
        GO_TYPE_GENERIC | GO_TYPE_CAPTURE_POINT => highlight_column.unwrap_or(true),
        // Every other type's `+0xc` forwards to its own `+0x14`.
        _ => highlightable_flags(type_id, flags, dyn_flags, reaction, overrides),
    }
}

/// [`highlightable_flags`] off a GameObject's descriptor; an absent `GAMEOBJECT_TYPE_ID` is the
/// wire default 0, DOOR. This slot gates the cursor, the right-click use (`OnUse`) and the pick's
/// pass-2 priority; the tooltip and the brighten follow the mouseover slot, [`mouseover_eligible`].
pub(crate) fn go_highlightable(
    store: &ObjectStore,
    reaction: Option<u8>,
    overrides: GoOverrides,
) -> bool {
    highlightable_flags(
        store.0.gameobject_type_id(),
        store.0.gameobject_flags(),
        store.0.gameobject_dynamic_flags(),
        reaction,
        overrides,
    )
}

/// MEETINGSTONE's override for one stone, `template.data[2] == queued_area`. `queued_area` is the
/// reference's global: 0 until the `SMSG_MEETINGSTONE_SETQUEUE` handler writes it. An unanswered
/// template reads not queued; a `data[2]` of 0 matches the unqueued 0, as in the reference.
pub(crate) fn meeting_stone_queued(area: Option<u32>, queued_area: u32) -> bool {
    area.is_some_and(|a| a == queued_area)
}

/// The FISHINGNODE override's input: our `UNIT_FIELD_CHANNEL_OBJECT` is this GameObject. False when
/// either is unknown, as the reference answers false with no active player.
pub(crate) fn fishing_channel_owned(
    self_store: Option<&ObjectStore>,
    go_guid: Option<u64>,
) -> bool {
    match (self_store, go_guid) {
        (Some(s), Some(g)) => s.0.unit_channel_object() == Some(g),
        _ => false,
    }
}

/// A `LockType.dbc` CursorName to its cursor, the reference's cursor-name lookup over the three
/// cursor-bearing lock types in 1.12.
fn cursor_kind_from_lock_name(name: &str) -> Option<CursorKind> {
    match name {
        "PickLock" => Some(CursorKind::PickLock),
        "GatherHerbs" => Some(CursorKind::GatherHerbs),
        "Mine" => Some(CursorKind::Mine),
        _ => None,
    }
}

/// The data-named cursor for a GameObject's lock: the `Lock.dbc` row's first slot only
/// (`[lockRow+0x24]`, no scan), then that `LockType.dbc` row's CursorName.
fn go_lock_cursor(
    lock_id: u32,
    locks: Option<&crate::go_templates::Locks>,
    lock_types: Option<&crate::go_templates::LockTypes>,
) -> Option<CursorKind> {
    if lock_id == 0 {
        return None;
    }
    let slots = locks?.0.slots(lock_id)?;
    let lock_type_id = slots[0].index;
    let name = lock_types?.0.cursor_name(lock_type_id)?;
    cursor_kind_from_lock_name(name)
}

/// A highlightable GameObject's cursor: TEXT and the Mail types by type, any other its lock's
/// data-named cursor or the gear.
fn go_cursor_kind(type_id: i32, lock_cursor: Option<CursorKind>) -> CursorKind {
    match type_id {
        GO_TYPE_TEXT => CursorKind::Inspect,
        GO_TYPE_RITUAL | GO_TYPE_MAILBOX | GO_TYPE_28 => CursorKind::Mail,
        GO_TYPE_GUILD_BANK => CursorKind::Buy,
        _ => lock_cursor.unwrap_or(CursorKind::Interact),
    }
}

/// The gossip test's (`0x6d1de0`) status half: the cached `SMSG_QUESTGIVER_STATUS` is 2 or above,
/// and a missing one reads as NONE since the server sends it unprompted.
pub(crate) fn questgiver_has_quest(quest_status: Option<u32>) -> bool {
    quest_status.is_some_and(|s| s >= 2)
}

/// The questgiver cursor `0x744640`, by the cached status alone, before any flag.
fn quest_cursor(quest_status: Option<u32>) -> Option<CursorKind> {
    match quest_status? {
        2..=5 | 7 | 8 => Some(CursorKind::Quest),
        6 => Some(CursorKind::QuestRepeatable),
        9 | 10 => Some(CursorKind::QuestTurnIn),
        _ => None,
    }
}

/// The unit cursor of a unit with any service bit (`0x4f7a50`): the quest cursor, then the
/// template's own cursor name (`0x715ea0`), then the flag ladder. `None` when nothing matches.
fn service_cursor(
    service: u32,
    quest_status: Option<u32>,
    cursor_name: Option<&str>,
) -> Option<CursorKind> {
    use npc_flags::*;
    if let Some(kind) = quest_cursor(quest_status) {
        return Some(kind);
    }
    if let Some(name) = cursor_name.filter(|n| !n.is_empty()) {
        return Some(cursor_from_name(name));
    }
    let bit = |m: u32| service & m != 0;
    Some(if bit(REPAIR) {
        CursorKind::RepairNpc
    } else if bit(INNKEEPER) {
        CursorKind::Innkeeper
    } else if bit(FLIGHTMASTER) {
        CursorKind::Taxi
    } else if bit(TRAINER) {
        CursorKind::Trainer
    } else if bit(SPIRITHEALER | SPIRITGUIDE) {
        CursorKind::Speak
    } else if bit(BANKER | GUILD_BANKER) {
        CursorKind::Buy
    } else if bit(PETITIONER | TABARDDESIGNER | BATTLEMASTER) || is_guild_master(service) {
        CursorKind::Speak
    } else if bit(AUCTIONEER) {
        CursorKind::Buy
    } else if bit(STABLEMASTER) {
        CursorKind::Speak
    } else if bit(VENDOR) {
        CursorKind::Pickup
    } else if bit(GOSSIP) {
        CursorKind::Speak
    } else if bit(SPELLCLICK) {
        CursorKind::Interact
    } else {
        return None;
    })
}

/// The loot leg's mode, `8 + (keyDown(0) ? 8 : 0)` in the reference: LootAll while the effective
/// auto-loot, `autoLootDefault` XOR shift, is on. `autoLootDefault` is not a 1.12 CVar; with it off
/// the held key alone decides, as in 1.12.
fn loot_cursor(auto_loot: bool, shift_held: bool) -> CursorKind {
    if auto_loot != shift_held {
        CursorKind::LootAll
    } else {
        CursorKind::Pickup
    }
}

/// Whether a corpse becomes the mouseover (the corpse object's `vtbl+0x54`): a body always, your
/// own included; a bone pile only while it has loot. A refused corpse is still picked but shows no
/// name and no brighten.
pub(crate) fn corpse_mouseover_eligible(store: &ObjectStore) -> bool {
    !store.0.corpse_is_bones() || store.0.corpse_lootable()
}

/// What the corpse classifier reads. It has two legs and no own-corpse leg: your own body sets
/// neither flag and reads Point.
#[derive(Debug, Clone, Copy, Default)]
struct CorpseFacts {
    /// `CORPSE_FIELD_DYNAMIC_FLAGS` bit 0.
    lootable: bool,
    /// `CORPSE_FIELD_FLAGS` bit 5, `CORPSE_FLAG_LOOTABLE`: the PvP insignia.
    insignia: bool,
    /// The insignia leg's skinning spell slot (`0x53bce0`) as its max range (`0x802c30`); `None`
    /// with no slot learned or when the corpse assist check (`0x71f890`) passes.
    insignia_range: Option<f32>,
    /// The player's own PvP side ([`pvp_faction_index`]).
    side: Option<u8>,
    /// `d2`, centre to centre.
    dist_sq: f32,
    /// The already-looting override: `[player+0x1d28/2c] == corpse GUID`.
    looting_this: bool,
    /// The auto-interact test, which the loot check asks first and then skips its range.
    can_auto_interact: bool,
    auto_loot: bool,
    /// Key 0, shift.
    shift_held: bool,
}

/// The corpse classifier (`0x4f8000`).
fn corpse_cursor(f: CorpseFacts) -> Option<(CursorKind, bool)> {
    // Leg 1, lootable: able within the loot check's reach or while this corpse's loot window is
    // open; Pickup or LootAll by key 0, grayed.
    if f.lootable {
        let able = f.can_auto_interact || f.dist_sq <= CORPSE_INTERACT_RANGE_SQ || f.looting_this;
        return Some((loot_cursor(f.auto_loot, f.shift_held), !able));
    }
    // Leg 2, the PvP insignia (`0x4f80dd`): the flag, a skinning slot and a body the player cannot
    // assist, then the side's insignia, grayed beyond the spell's range.
    if let Some(max_range) = f.insignia_range.filter(|_| f.insignia) {
        return Some((
            insignia_cursor(f.side),
            !in_skin_reach(max_range, f.dist_sq),
        ));
    }
    None // the reference's "else clear": Point
}

/// Resolve this frame's [`WorldCursor`] over the hover.
pub(super) fn classify_cursor(
    hovered: Res<Hovered>,
    hovered_object: Res<HoveredObject>,
    inputs: CursorInputs,
    (mut cursor, mut fork): (ResMut<WorldCursor>, ResMut<AttackFork>),
) {
    let (want, want_fork) = classify(&inputs, &hovered, &hovered_object);
    if *cursor != want {
        *cursor = want;
    }
    if *fork != want_fork {
        *fork = want_fork;
    }
}

#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct CursorInputs<'w, 's> {
    factions: Option<Res<'w, Factions>>,
    reputations: Res<'w, Reputations>,
    /// The group, for the reaction's party rung; none without the party plugin.
    group: Option<Res<'w, crate::ui_party::GroupState>>,
    // `control_lost` is the attack leg's clear.
    player: Res<'w, crate::player::Player>,
    approach: Res<'w, crate::player::Approach>,
    #[allow(clippy::type_complexity)]
    units: Query<
        'w,
        's,
        (
            &'static Transform,
            Option<&'static ObjectStore>,
            Option<&'static crate::go_anim::GoAnim>,
        ),
    >,
    self_q: Query<'w, 's, (&'static Transform, &'static ObjectStore), With<SelfPlayer>>,
    go_inputs: super::lock::GoLockInputs<'w, 's>,
    player_actions: Res<'w, crate::ui_action::PlayerActions>,
    // The skin legs' learned-spell slots.
    learned: Res<'w, crate::ui_action::LearnedAbilities>,
    // The skin legs' range lookup (`0x802c30`): the spell rows, their modifiers and the units.
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    spell_mods: Option<Res<'w, crate::spell::SpellModifiers>>,
    range_units: crate::spell::RangeUnits<'w, 's>,
    quest: Res<'w, crate::ui_quest::QuestGiver>,
    loot_cfg: Res<'w, crate::ui_loot::LootConfig>,
    keys: Res<'w, ButtonInput<KeyCode>>,
    // `[player+0x1d28/2c]`, the object whose loot window is open.
    loot_latch: Res<'w, crate::ui_loot::LootLatch>,
    // The queued area; absent in a headless build, which reads as not queued.
    stone: Option<Res<'w, crate::ui_dialog_verbs::MeetingStone>>,
    // The creature records, whose cursor name `0x715ea0` reads; absent reads no name.
    names: Option<Res<'w, crate::names::NameCache>>,
}

/// The cursor and the attack fork over a pick; no pick, or nothing resolvable, reads Point.
pub(super) fn classify(
    inputs: &CursorInputs,
    hovered: &Hovered,
    hovered_object: &HoveredObject,
) -> (WorldCursor, AttackFork) {
    let CursorInputs {
        factions,
        reputations,
        group,
        player,
        approach,
        units,
        self_q,
        go_inputs,
        player_actions,
        learned,
        quest,
        loot_cfg,
        keys,
        loot_latch,
        stone,
        names,
        spells,
        spell_mods,
        range_units,
    } = inputs;
    let party = super::party_of(group.as_deref());
    // The skinning spell's max range from the player toward `target` (`0x802c30`); a spell with no
    // row reads 0, as the lookup zeroes both bounds.
    let skin_max_range = |spell_id: u32, target: Option<benilla_formats::RangeUnit>| -> f32 {
        let range = (|| {
            let spells = spells.as_deref()?;
            let d = spells.catalog.get(spell_id)?;
            let row = spells.ranges.get(d.range_index);
            let caster = range_units.caster();
            let targets = benilla_formats::RangeTargets {
                target,
                attack_target: range_units.caster_attack_target(),
            };
            match spell_mods.as_deref() {
                Some(mods) => mods.min_max_range(d, row, caster, targets),
                None => benilla_formats::min_max_range(d, row, caster, targets),
            }
        })();
        range.map_or(0.0, |(_, max)| max)
    };
    let can_auto_interact = crate::player::can_auto_interact(
        approach.enabled,
        self_q.single().ok().map(|(_, s)| s),
        player,
    );
    // A GameObject that is not highlightable clears the cursor, as the reference's handler does.
    let resolve_go = || {
        let (go_tf, store, anim) = units.get(hovered_object.target?).ok()?;
        let store = store?;
        let (self_tf, self_store) = self_q.single().ok()?;
        let reaction = go_reaction(
            factions.as_deref(),
            store.0.gameobject_faction(),
            Some(self_store),
        );
        // Read before the gate: MEETINGSTONE's override is a template slot.
        let tmpl = hovered_object.guid.and_then(|g| go_inputs.templates.get(g));
        let overrides = GoOverrides {
            channel_owned: fishing_channel_owned(Some(self_store), hovered_object.guid),
            meeting_stone_queued: meeting_stone_queued(
                tmpl.and_then(|t| t.meeting_stone).map(|m| m.area),
                stone.as_deref().map_or(0, |s| s.area),
            ),
        };
        if !go_highlightable(store, reaction, overrides) {
            return None;
        }
        // Until its template answers, a locked GameObject reads the gear.
        let lock_id = tmpl.map_or(0, |t| t.lock_id);
        let lock_cursor = go_lock_cursor(
            lock_id,
            go_inputs.locks.as_deref(),
            go_inputs.lock_types.as_deref(),
        );
        let kind = go_cursor_kind(store.0.gameobject_type_id(), lock_cursor);
        let dist_sq = go_tf.translation.distance_squared(self_tf.translation);
        // `usable`'s lock arm then range arm; its player-state arm is not applied. The lock arm
        // needs `GO_FLAG_LOCKED`, so an herb the player cannot gather stays lit and refuses on the
        // click, while a padlocked door grays.
        let facts = super::lock::go_facts(Some((store, crate::go_anim::go_state(anim, store))));
        let lock_unmet = go_inputs
            .locks
            .as_deref()
            .and_then(|l| l.0.slots(lock_id).filter(|_| lock_id != 0))
            .is_some_and(|slots| {
                super::lock::resolve_lock(
                    slots,
                    &player_actions.spells,
                    go_inputs.spells.as_deref(),
                    go_inputs.skill_lines.as_ref().map(|s| &*s.catalog),
                    Some(self_store),
                    &go_inputs.objects,
                    facts,
                    &mut None,
                )
                .blocks_usable(facts.flag_locked)
            });
        let unable = kind != CursorKind::PickLock
            && (lock_unmet || dist_sq > go_interact_range_sq(store.0.gameobject_type_id()));
        Some((kind, unable))
    };
    // The reference picks once over all objects; benilla picks units and GameObjects apart and
    // classifies whichever is nearer under the cursor.
    let attack_fork = std::cell::Cell::new(false);
    let resolve_unit = || {
        let (unit_tf, store, _) = units.get(hovered.target?).ok()?;
        let store = store?;
        let (self_tf, self_store) = self_q.single().ok()?;
        let dist_sq = unit_tf.translation.distance_squared(self_tf.translation);
        let reach = melee_reach(store, self_store);
        let in_melee = dist_sq <= reach * reach;

        let dead = store.0.unit_is_dead();
        if dead {
            if store.0.unit_lootable() {
                // Grayed by the loot check's melee reach; the reference's mid-loot state block and
                // open-loot-window override are not applied to units.
                let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
                return Some((
                    loot_cursor(loot_cfg.auto_loot, shift),
                    !(can_auto_interact || in_melee),
                ));
            }
            // Skin also needs the corpse's learned slot ([`unit_skin_spell`]): a miner over a
            // beast, or anyone over a friendly player's body, sees Point. Grayed beyond the
            // spell's own range (`0x4f7e5b`).
            if store.0.unit_flags() & UNIT_FLAG_SKINNABLE != 0 {
                if let Some(spell) = unit_skin_spell(
                    learned,
                    store,
                    Some(self_store),
                    factions.as_deref(),
                    reputations,
                    party,
                    names.as_deref(),
                ) {
                    let max_range =
                        skin_max_range(spell, hovered.target.and_then(|e| range_units.unit(e)));
                    let side = pvp_faction_index(self_store, factions.as_deref());
                    return Some((
                        skin_cursor(store, names.as_deref(), side),
                        !in_skin_reach(max_range, dist_sq),
                    ));
                }
            }
            return None; // a plain corpse: Point
        }

        // The interact check, asked through its timed wrapper, picks the ladder; it is not a
        // reaction threshold.
        if super::can_interact(
            Some(store),
            factions.as_deref(),
            reputations,
            party,
            Some(self_store),
        ) {
            let status = hovered.guid.and_then(|g| quest.status(g));
            let cursor_name = store
                .0
                .object_entry()
                .and_then(|entry| names.as_deref()?.creature_record(entry))
                .and_then(|rec| rec.cursor_name.as_deref());
            // No matching bit clears the cursor without reaching the attack leg: Point, never the
            // sword.
            return service_cursor(store.0.unit_npc_flags(), status, cursor_name)
                .map(|kind| (kind, dist_sq > unit_interact_reach_sq(store)));
        }
        // The attack check, the predicate TAB, the combat flash and `UnitCanAttack` share; not
        // attackable clears the cursor: Point.
        if !super::can_attack(
            Some(store),
            factions.as_deref(),
            reputations,
            party,
            Some(self_store),
        ) {
            return None;
        }
        attack_fork.set(true);
        player_attack_legs(self_store, player.control_lost)
            .then_some((CursorKind::Attack, dist_sq > ATTACK_RANGE_SQ))
    };
    // The corpse classifier.
    let resolve_corpse = || {
        let (corpse_tf, store, _) = units.get(hovered.corpse?).ok()?;
        let store = store?;
        let (self_tf, self_store) = self_q.single().ok()?;
        let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let insignia = store.0.corpse_pvp_insignia();
        // The corpse leg passes no target to the range lookup (`0x4f810a`).
        let insignia_range = learned
            .skin_player_corpse
            .filter(|_| insignia)
            .filter(|_| !super::corpse_friendly(store, factions.as_deref(), Some(self_store)))
            .map(|spell| skin_max_range(spell, None));
        corpse_cursor(CorpseFacts {
            lootable: store.0.corpse_lootable(),
            insignia,
            insignia_range,
            side: pvp_faction_index(self_store, factions.as_deref()),
            dist_sq: corpse_tf.translation.distance_squared(self_tf.translation),
            looting_this: loot_latch.0.is_some() && loot_latch.0 == hovered.corpse_guid,
            can_auto_interact,
            auto_loot: loot_cfg.auto_loot,
            shift_held: shift,
        })
    };
    let resolved = if go_is_nearest(hovered, hovered_object) {
        resolve_go()
    } else if hovered.corpse.is_some() {
        resolve_corpse()
    } else {
        resolve_unit()
    };
    let (kind, unable) = resolved.unwrap_or((CursorKind::Point, false));
    (WorldCursor { kind, unable }, AttackFork(attack_fork.get()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::dialog_status;

    /// The skinning spell lookup (`0x53bce0`) and the click's player refusal (`0x7313c5`), for a
    /// character with Skinning 8613 and Mining 32606 learned as the learn switch files them
    /// (`0x5420c7`).
    #[test]
    fn the_skin_leg_reads_the_corpses_own_slot() {
        use crate::ui_action::{LearnedAbilities, SkinSubject};
        const SKINNING: u32 = 8613;
        const MINING: u32 = 32606;
        const REMOVE_INSIGNIA: u32 = 22027;
        let beast = SkinSubject::Creature { type_flags: 0 };
        let rock = SkinSubject::Creature { type_flags: 0x200 };
        let herb = SkinSubject::Creature { type_flags: 0x100 };
        let player = SkinSubject::PlayerOrCorpse;
        let miner = LearnedAbilities {
            skinning: [None, None, Some(MINING), None],
            ..Default::default()
        };
        let never = || -> bool { panic!("the assist check is asked of a player only") };
        assert_eq!(
            skin_leg_spell(&miner, beast, never),
            None,
            "a miner over a beast"
        );
        assert_eq!(skin_leg_spell(&miner, rock, never), Some(MINING));
        assert_eq!(skin_leg_spell(&miner, herb, never), None);
        let both = LearnedAbilities {
            skinning: [Some(SKINNING), None, Some(MINING), None],
            ..Default::default()
        };
        assert_eq!(skin_leg_spell(&both, beast, never), Some(SKINNING));
        assert_eq!(skin_leg_spell(&both, rock, never), Some(MINING));
        // A player's body reads the Remove Insignia slot, never a Skinning one, and only while the
        // assist check refuses it.
        assert_eq!(skin_leg_spell(&both, player, || false), None);
        let insignia = LearnedAbilities {
            skin_player_corpse: Some(REMOVE_INSIGNIA),
            ..both
        };
        assert_eq!(skin_leg_spell(&insignia, player, || true), None, "a friend");
        assert_eq!(
            skin_leg_spell(&insignia, player, || false),
            Some(REMOVE_INSIGNIA),
            "an enemy"
        );
    }

    /// The skin cursor by the skinning type (`0x715e50`, `0x4f7ece`).
    #[test]
    fn the_skin_cursor_follows_the_creature_kind() {
        assert_eq!(skin_cursor_kind(0), CursorKind::Skin);
        assert_eq!(skin_cursor_kind(0x100), CursorKind::GatherHerbs);
        assert_eq!(skin_cursor_kind(0x200), CursorKind::Mine);
        assert_eq!(skin_cursor_kind(0x8000), CursorKind::Mine);
        // Herb is tested first.
        assert_eq!(skin_cursor_kind(0x300), CursorKind::GatherHerbs);
    }

    /// The loot check skips its range under the auto-interact test, so the click that walks there
    /// finds a lit pouch.
    #[test]
    fn click_to_move_lights_a_far_corpses_pouch() {
        let far = CorpseFacts {
            lootable: true,
            dist_sq: 400.0,
            ..CorpseFacts::default()
        };
        assert_eq!(corpse_cursor(far), Some((CursorKind::Pickup, true)));
        assert_eq!(
            corpse_cursor(CorpseFacts {
                can_auto_interact: true,
                ..far
            }),
            Some((CursorKind::Pickup, false))
        );
    }

    #[test]
    fn the_corpse_classifier_has_two_legs_and_no_own_corpse_leg() {
        let near = 25.0; // exactly the gate, which is inclusive
        let far = 25.01;
        let bones = |dist_sq| CorpseFacts {
            lootable: true,
            dist_sq,
            ..CorpseFacts::default()
        };
        assert_eq!(
            corpse_cursor(bones(near)),
            Some((CursorKind::Pickup, false))
        );
        assert_eq!(
            corpse_cursor(CorpseFacts {
                auto_loot: true,
                ..bones(near)
            }),
            Some((CursorKind::LootAll, false))
        );
        assert_eq!(corpse_cursor(bones(far)), Some((CursorKind::Pickup, true)));
        assert_eq!(
            corpse_cursor(CorpseFacts {
                looting_this: true,
                ..bones(far)
            }),
            Some((CursorKind::Pickup, false))
        );
        // The reference tests lootable first.
        assert_eq!(
            corpse_cursor(CorpseFacts {
                insignia: true,
                insignia_range: Some(5.0),
                side: Some(0),
                ..bones(near)
            }),
            Some((CursorKind::Pickup, false))
        );
        // Your own corpse sets neither flag.
        let mine = |dist_sq| CorpseFacts {
            insignia_range: Some(5.0),
            side: Some(0),
            dist_sq,
            ..CorpseFacts::default()
        };
        assert_eq!(corpse_cursor(mine(near)), None);
        assert_eq!(
            corpse_cursor(CorpseFacts {
                auto_loot: true,
                shift_held: true,
                ..mine(far)
            }),
            None
        );
    }

    /// The insignia leg (`0x4f80dd`): the enemy side's emblem by the player's own side through
    /// `0x9f9864`, its `Unable` twin beyond the spell's range squared (`0x9f986c`).
    #[test]
    fn the_insignia_corpse_shows_the_enemy_sides_emblem() {
        const HORDE: Option<u8> = Some(0);
        const ALLIANCE: Option<u8> = Some(1);
        let insignia = |side, dist_sq| CorpseFacts {
            insignia: true,
            insignia_range: Some(5.0),
            side,
            dist_sq,
            ..CorpseFacts::default()
        };
        assert_eq!(
            corpse_cursor(insignia(HORDE, 25.0)),
            Some((CursorKind::SkinAlliance, false)),
            "a Horde player in reach, the boundary inclusive"
        );
        assert_eq!(
            corpse_cursor(insignia(ALLIANCE, 4.0)),
            Some((CursorKind::SkinHorde, false))
        );
        let far = corpse_cursor(insignia(HORDE, 25.01)).unwrap();
        assert_eq!(far, (CursorKind::SkinAlliance, true));
        assert_eq!(
            WorldCursor {
                kind: far.0,
                unable: far.1
            }
            .stem(),
            "UnableSkinAlliance"
        );
        let far = corpse_cursor(insignia(ALLIANCE, 30.0)).unwrap();
        assert_eq!(
            WorldCursor {
                kind: far.0,
                unable: far.1
            }
            .stem(),
            "UnableSkinHorde"
        );
        // No slot learned, or a friendly body (the assist check passes): Point.
        assert_eq!(
            corpse_cursor(CorpseFacts {
                insignia_range: None,
                ..insignia(HORDE, 1.0)
            }),
            None
        );
        // A slot but no insignia flag: Point.
        assert_eq!(
            corpse_cursor(CorpseFacts {
                insignia: false,
                ..insignia(HORDE, 1.0)
            }),
            None
        );
        // A spell with no row reads range 0: only the caster's own spot is in reach.
        assert_eq!(
            corpse_cursor(CorpseFacts {
                insignia_range: Some(0.0),
                ..insignia(HORDE, 0.01)
            }),
            Some((CursorKind::SkinAlliance, true))
        );
    }

    /// The side digit (`0x6d6e90`) and the two tables, and the unit leg's player arm (`0x4f7e6f`).
    #[test]
    fn the_insignia_follows_the_players_own_side() {
        assert_eq!(insignia_cursor(Some(0)), CursorKind::SkinAlliance);
        assert_eq!(insignia_cursor(Some(1)), CursorKind::SkinHorde);
        assert_eq!(insignia_cursor(None), CursorKind::Point);
        assert!(in_skin_reach(5.0, 25.0));
        assert!(!in_skin_reach(5.0, 25.01));
        assert!(
            in_skin_reach(f32::NAN, 25.0),
            "unordered takes the in-reach arm"
        );
        // The side reads the player's race template's group mask, `& 4` first.
        let fx_factions = crate::spell::targeting::corpse_fixture::factions();
        let of_race = |race: u8| {
            // `UNIT_FIELD_BYTES_0` (field 36) byte 0, the race.
            let store = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                36,
                u32::from(race),
            )]));
            pvp_faction_index(&store, Some(&fx_factions))
        };
        assert_eq!(of_race(1), Some(1), "a human is Alliance");
        assert_eq!(of_race(2), Some(0), "an orc is Horde");
        assert_eq!(of_race(9), None, "a race with no template row");
        // The name table now names the insignia kinds, so a creature cursor name does too.
        assert_eq!(cursor_from_name("SkinHorde"), CursorKind::SkinHorde);
        assert_eq!(cursor_from_name("skinalliance"), CursorKind::SkinAlliance);
    }

    #[test]
    fn the_loot_pouch_triples_with_the_effective_auto_loot() {
        assert_eq!(loot_cursor(false, false), CursorKind::Pickup);
        assert_eq!(loot_cursor(true, false), CursorKind::LootAll);
        // Shift inverts both ways; with the setting off it is 1.12's whole mechanism.
        assert_eq!(loot_cursor(false, true), CursorKind::LootAll);
        assert_eq!(loot_cursor(true, true), CursorKind::Pickup);
        let far = WorldCursor {
            kind: CursorKind::LootAll,
            unable: true,
        };
        assert_eq!(far.stem(), "UnableLootAll");
    }

    #[test]
    fn stems_name_the_shipped_blps() {
        let attack = WorldCursor {
            kind: CursorKind::Attack,
            unable: false,
        };
        assert_eq!(attack.stem(), "Attack");
        let far = WorldCursor {
            kind: CursorKind::Attack,
            unable: true,
        };
        assert_eq!(far.stem(), "UnableAttack");
        let point = WorldCursor {
            kind: CursorKind::Point,
            unable: true,
        };
        assert_eq!(point.stem(), "Point");
    }

    #[test]
    fn mouseover_eligibility_matches_the_per_type_slot_table() {
        for t in [
            GO_TYPE_SPELL_FOCUS,
            GO_TYPE_DUEL_ARBITER,
            GO_TYPE_FISHINGHOLE,
            GO_TYPE_AURA_GENERATOR,
        ] {
            assert!(mouseover_eligible(
                t,
                GO_FLAG_INTERACT_COND,
                0,
                None,
                None,
                GoOverrides::default()
            ));
            assert!(mouseover_eligible(
                t,
                0x10,
                0,
                None,
                None,
                GoOverrides::default()
            ));
        }
        for t in [GO_TYPE_TRANSPORT, GO_TYPE_MAP_OBJECT, GO_TYPE_MO_TRANSPORT] {
            assert!(!mouseover_eligible(
                t,
                0,
                GO_DYNFLAG_ACTIVATE,
                None,
                None,
                GoOverrides::default()
            ));
        }
        assert!(mouseover_eligible(
            GO_TYPE_GENERIC,
            0,
            0,
            Some(true),
            None,
            GoOverrides::default()
        ));
        assert!(!mouseover_eligible(
            GO_TYPE_GENERIC,
            0,
            0,
            Some(false),
            None,
            GoOverrides::default()
        ));
        assert!(
            mouseover_eligible(GO_TYPE_GENERIC, 0, 0, None, None, GoOverrides::default()),
            "template not answered yet reads eligible, so a signpost isn't blank while it queries"
        );
        assert!(!mouseover_eligible(
            0,
            GO_FLAG_INTERACT_COND,
            0,
            None,
            None,
            GoOverrides::default()
        ));
        assert!(mouseover_eligible(
            0,
            GO_FLAG_INTERACT_COND,
            GO_DYNFLAG_ACTIVATE,
            None,
            None,
            GoOverrides::default()
        ));
        assert!(!mouseover_eligible(
            3,
            0x10,
            0,
            None,
            None,
            GoOverrides::default()
        )); // NO_INTERACT chest
        assert!(!mouseover_eligible(
            3,
            0x1,
            0,
            None,
            None,
            GoOverrides::default()
        )); // IN_USE chest
            // A door hostile to us, like the Deadmines Factory Door (faction 114, flags 0x20).
        assert!(!mouseover_eligible(
            0,
            0x20,
            0,
            None,
            Some(1),
            GoOverrides::default()
        ));
        assert!(mouseover_eligible(
            0,
            0x20,
            0,
            None,
            Some(3),
            GoOverrides::default()
        ));
        assert!(mouseover_eligible(
            0,
            0x20,
            0,
            None,
            Some(4),
            GoOverrides::default()
        ));
        assert!(mouseover_eligible(
            GO_TYPE_TRAP,
            0,
            0,
            None,
            Some(1),
            GoOverrides::default()
        ));
        assert!(!mouseover_eligible(
            GO_TYPE_TRAP,
            0,
            0,
            None,
            Some(3),
            GoOverrides::default()
        ));
        assert!(mouseover_eligible(
            0,
            0x20,
            0,
            None,
            None,
            GoOverrides::default()
        ));
        assert!(mouseover_eligible(
            0,
            0,
            0,
            None,
            None,
            GoOverrides::default()
        ));
        assert!(mouseover_eligible(
            19,
            0,
            0,
            None,
            None,
            GoOverrides::default()
        )); // mailbox
    }

    #[test]
    fn highlightable_gates_the_quest_object_but_not_the_plain_door() {
        assert!(highlightable_flags(0, 0, 0, None, GoOverrides::default())); // DOOR, no flags
        assert!(!highlightable_flags(
            GO_TYPE_GENERIC,
            0,
            0,
            None,
            GoOverrides::default()
        ));
        for t in [GO_TYPE_TRANSPORT, GO_TYPE_MAP_OBJECT, GO_TYPE_MO_TRANSPORT] {
            assert!(!highlightable_flags(
                t,
                0,
                GO_DYNFLAG_ACTIVATE,
                None,
                GoOverrides::default()
            ));
        }
        for t in [
            GO_TYPE_SPELL_FOCUS,
            GO_TYPE_DUEL_ARBITER,
            GO_TYPE_FISHINGHOLE,
            GO_TYPE_AURA_GENERATOR,
        ] {
            assert!(!highlightable_flags(t, 0, 0, None, GoOverrides::default()));
            assert!(!highlightable_flags(
                t,
                0,
                GO_DYNFLAG_ACTIVATE,
                Some(3),
                GoOverrides::default()
            ));
        }
        // The marker set's two slots disagree: a tooltip, but no interact cursor.
        assert!(mouseover_eligible(
            GO_TYPE_SPELL_FOCUS,
            0,
            0,
            None,
            Some(3),
            GoOverrides::default()
        ));
        assert!(!highlightable_flags(
            GO_TYPE_SPELL_FOCUS,
            0,
            0,
            Some(3),
            GoOverrides::default()
        ));
        for t in [GO_TYPE_GUARDPOST, GO_TYPE_MAX + 1, 99, -1] {
            assert!(strategy_is_default(t), "type {t} takes the default arm");
            assert!(!highlightable_flags(t, 0, 0, None, GoOverrides::default()));
            assert!(!mouseover_eligible(
                t,
                0,
                0,
                None,
                None,
                GoOverrides::default()
            ));
        }
        for t in (0..=GO_TYPE_MAX).filter(|t| *t != GO_TYPE_GUARDPOST) {
            assert!(!strategy_is_default(t), "type {t} has its own strategy");
        }
        assert!(!highlightable_flags(
            GO_TYPE_AUCTIONHOUSE,
            0,
            GO_DYNFLAG_ACTIVATE,
            Some(3),
            GoOverrides::default()
        ));
        assert!(!mouseover_eligible(
            GO_TYPE_AUCTIONHOUSE,
            0,
            0,
            None,
            Some(3),
            GoOverrides::default()
        ));
        assert!(!highlightable_flags(
            GO_TYPE_CAPTURE_POINT,
            0,
            0,
            Some(3),
            GoOverrides::default()
        ));
        assert!(mouseover_eligible(
            GO_TYPE_CAPTURE_POINT,
            0,
            0,
            Some(true),
            Some(3),
            GoOverrides::default()
        ));
        assert!(!mouseover_eligible(
            GO_TYPE_CAPTURE_POINT,
            0,
            0,
            Some(false),
            Some(3),
            GoOverrides::default()
        ));
        assert!(!highlightable_flags(
            3,
            0x1,
            0,
            None,
            GoOverrides::default()
        )); // CHEST, IN_USE
        assert!(!highlightable_flags(
            3,
            0x10,
            0,
            None,
            GoOverrides::default()
        )); // CHEST, NO_INTERACT
        assert!(!highlightable_flags(
            3,
            GO_FLAG_INTERACT_COND,
            0,
            None,
            GoOverrides::default()
        )); // quest chest, no quest → clear
        assert!(highlightable_flags(
            3,
            GO_FLAG_INTERACT_COND,
            GO_DYNFLAG_ACTIVATE,
            None,
            GoOverrides::default()
        )); // quest chest, quest held → usable
    }

    #[test]
    fn the_meeting_stone_answers_only_the_queued_area() {
        let queued = GoOverrides {
            meeting_stone_queued: true,
            ..Default::default()
        };
        assert!(highlightable_flags(
            GO_TYPE_MEETINGSTONE,
            0,
            0,
            None,
            GoOverrides::default()
        ));
        assert!(!highlightable_flags(
            GO_TYPE_MEETINGSTONE,
            0,
            0,
            None,
            queued
        ));
        assert!(!mouseover_eligible(
            GO_TYPE_MEETINGSTONE,
            0,
            0,
            None,
            None,
            queued
        ));
        for (flags, dyn_flags, reaction) in [
            (0x10, 0, Some(1)),
            (0x1, 0, Some(1)),
            (GO_FLAG_INTERACT_COND, 0, Some(1)),
        ] {
            assert!(
                highlightable_flags(
                    GO_TYPE_MEETINGSTONE,
                    flags,
                    dyn_flags,
                    reaction,
                    GoOverrides::default()
                ),
                "flags {flags:#x} must not reach a meeting stone — its slot never calls"
            );
        }
        assert!(!meeting_stone_queued(None, 0));
        assert!(!meeting_stone_queued(Some(1519), 0));
        assert!(meeting_stone_queued(Some(1519), 1519));
        assert!(!meeting_stone_queued(Some(1519), 1517));
        assert!(!meeting_stone_queued(None, 1519));
        // The reference's own `0 != 0`: a stone whose `data[2]` is 0 matches while unqueued.
        assert!(meeting_stone_queued(Some(0), 0));
    }

    #[test]
    fn the_bobber_is_channel_gated_and_reaches_a_hundred_yards() {
        assert!(highlightable_flags(
            GO_TYPE_FISHINGNODE,
            0,
            0,
            None,
            GoOverrides {
                channel_owned: true,
                ..Default::default()
            }
        ));
        assert!(!highlightable_flags(
            GO_TYPE_FISHINGNODE,
            0,
            0,
            None,
            GoOverrides::default()
        ));
        // NO_INTERACT still applies after the channel check.
        assert!(!highlightable_flags(
            GO_TYPE_FISHINGNODE,
            0x10,
            0,
            None,
            GoOverrides {
                channel_owned: true,
                ..Default::default()
            }
        ));
        assert!(!mouseover_eligible(
            GO_TYPE_FISHINGNODE,
            0,
            0,
            None,
            None,
            GoOverrides::default()
        ));
        assert!(mouseover_eligible(
            GO_TYPE_FISHINGNODE,
            0,
            0,
            None,
            None,
            GoOverrides {
                channel_owned: true,
                ..Default::default()
            }
        ));
        assert!(highlightable_flags(0, 0, 0, None, GoOverrides::default()));
        assert_eq!(go_interact_range_sq(GO_TYPE_FISHINGNODE), 100.0 * 100.0);
        assert_eq!(go_interact_range_sq(0), GO_INTERACT_RANGE_SQ);
        assert_eq!(go_interact_range_sq(GO_TYPE_CHAIR), 9.0);
        assert_eq!(go_interact_range_sq(GO_TYPE_BARBER_CHAIR), 9.0);
        assert!(
            go_interact_range_sq(GO_TYPE_CHAIR) < GO_INTERACT_RANGE_SQ,
            "a chair's reach is shorter than the default GameObject reach"
        );
        assert!(!fishing_channel_owned(None, Some(7)));
        assert!(!fishing_channel_owned(None, None));
    }

    #[test]
    fn melee_reach_floors_at_five() {
        // A typical pair, 1.5 + 1.5 + 1.333, is lifted to the 5 yd floor.
        assert_eq!((1.5_f32 + 1.5 + MELEE_OFFSET).max(MELEE_FLOOR), 5.0);
        assert!(((3.0_f32 + 3.0 + MELEE_OFFSET).max(MELEE_FLOOR) - 7.333_33).abs() < 1e-4);
    }

    /// The flag ladder after the quest cursor and the cursor name (`0x4f7a50`): repair first,
    /// gossip near the bottom.
    #[test]
    fn service_ladder_walks_the_3_3_5a_order() {
        use npc_flags::*;
        let cursor = |flags| service_cursor(flags, None, None);
        for (flags, kind) in [
            (REPAIR, CursorKind::RepairNpc),
            (INNKEEPER, CursorKind::Innkeeper),
            (FLIGHTMASTER, CursorKind::Taxi),
            (TRAINER, CursorKind::Trainer),
            (SPIRITHEALER, CursorKind::Speak),
            (SPIRITGUIDE, CursorKind::Speak),
            (BANKER, CursorKind::Buy),
            (GUILD_BANKER, CursorKind::Buy),
            (PETITIONER, CursorKind::Speak),
            (TABARDDESIGNER, CursorKind::Speak),
            (BATTLEMASTER, CursorKind::Speak),
            (AUCTIONEER, CursorKind::Buy),
            (STABLEMASTER, CursorKind::Speak),
            (VENDOR, CursorKind::Pickup),
            (GOSSIP, CursorKind::Speak),
            (SPELLCLICK, CursorKind::Interact),
        ] {
            assert_eq!(cursor(flags), Some(kind), "flags {flags:#x}");
        }
        assert_eq!(
            cursor(GOSSIP | VENDOR),
            Some(CursorKind::Pickup),
            "vendor before gossip"
        );
        assert_eq!(cursor(VENDOR | REPAIR), Some(CursorKind::RepairNpc));
        assert_eq!(cursor(GOSSIP | INNKEEPER), Some(CursorKind::Innkeeper));
        // The questgiver bit and the vendor kinds are not on the ladder: Point, never the sword.
        assert_eq!(cursor(QUESTGIVER), None);
        assert_eq!(cursor(0x100), None);
        assert_eq!(cursor(0), None);
    }

    /// The questgiver cursor `0x744640` reads the status alone, before the cursor name and every
    /// flag, over all eleven statuses.
    #[test]
    fn the_quest_cursor_comes_first_by_status() {
        use npc_flags::*;
        for (status, want) in [
            (dialog_status::NONE, None),
            (dialog_status::UNAVAILABLE, None),
            (dialog_status::LOW_LEVEL_AVAILABLE, Some(CursorKind::Quest)),
            (dialog_status::LOW_LEVEL_REWARD_REP, Some(CursorKind::Quest)),
            (
                dialog_status::LOW_LEVEL_AVAILABLE_REP,
                Some(CursorKind::Quest),
            ),
            (dialog_status::INCOMPLETE, Some(CursorKind::Quest)),
            (dialog_status::REWARD_REP, Some(CursorKind::QuestRepeatable)),
            (dialog_status::AVAILABLE_REP, Some(CursorKind::Quest)),
            (dialog_status::AVAILABLE, Some(CursorKind::Quest)),
            (dialog_status::REWARD2, Some(CursorKind::QuestTurnIn)),
            (dialog_status::REWARD, Some(CursorKind::QuestTurnIn)),
        ] {
            assert_eq!(quest_cursor(Some(status)), want, "status {status}");
            assert_eq!(
                service_cursor(GOSSIP | QUESTGIVER, Some(status), Some("Buy")),
                want.or(Some(CursorKind::Buy)),
                "status {status} over a cursor name"
            );
        }
        assert_eq!(quest_cursor(None), None);
    }

    /// A template's cursor name comes before the flags (`0x715ea0`), by table name; any other name
    /// is drawn from its own file (mode `0x35`).
    #[test]
    fn the_cursor_name_comes_before_the_flags() {
        use npc_flags::*;
        assert_eq!(
            service_cursor(GOSSIP | VENDOR, None, Some("Speak")),
            Some(CursorKind::Speak)
        );
        assert_eq!(
            service_cursor(VENDOR, None, Some("repair")),
            Some(CursorKind::Repair),
            "case-insensitive"
        );
        assert_eq!(
            service_cursor(GOSSIP, None, Some("vehichleCursor")),
            Some(CursorKind::Vehicle)
        );
        let named = service_cursor(GOSSIP, None, Some("Directions")).unwrap();
        assert_eq!(named, CursorKind::Named("Directions"));
        let unable = WorldCursor {
            kind: named,
            unable: true,
        };
        assert_eq!(unable.stem(), "UnableDirections");
        assert_eq!(
            service_cursor(GOSSIP, None, Some("")),
            Some(CursorKind::Speak),
            "an empty name is none"
        );
    }

    /// The reach is the unit's combat reach plus 4 (`0x4f7b0f`).
    #[test]
    fn the_service_reach_follows_the_units_combat_reach() {
        let unit = |reach: f32| {
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                130, // UNIT_FIELD_COMBATREACH
                reach.to_bits(),
            )]))
        };
        assert_eq!(unit_interact_reach_sq(&unit(1.5)), 30.25);
        assert_eq!(unit_interact_reach_sq(&unit(6.0)), 100.0);
    }

    #[test]
    fn lock_names_resolve_to_the_three_data_cursors() {
        assert_eq!(
            cursor_kind_from_lock_name("PickLock"),
            Some(CursorKind::PickLock)
        );
        assert_eq!(
            cursor_kind_from_lock_name("GatherHerbs"),
            Some(CursorKind::GatherHerbs)
        );
        assert_eq!(cursor_kind_from_lock_name("Mine"), Some(CursorKind::Mine));
        assert_eq!(cursor_kind_from_lock_name(""), None);
        assert_eq!(cursor_kind_from_lock_name("Fishing"), None);
    }

    #[test]
    fn go_cursor_kind_maps_type_then_lock() {
        // MAILBOX(19), RITUAL(18) and type 28 take Mail whatever their lock.
        assert_eq!(go_cursor_kind(19, None), CursorKind::Mail);
        assert_eq!(go_cursor_kind(18, None), CursorKind::Mail);
        assert_eq!(go_cursor_kind(28, Some(CursorKind::Mine)), CursorKind::Mail);
        assert_eq!(go_cursor_kind(9, None), CursorKind::Inspect); // TEXT
        assert_eq!(go_cursor_kind(34, None), CursorKind::Buy); // GUILD_BANK
        assert!(!strategy_is_default(GO_TYPE_GUILD_BANK));
        for t in [0, 1, 3, 10, 6, 24] {
            assert_eq!(go_cursor_kind(t, None), CursorKind::Interact);
        }
        assert_eq!(go_cursor_kind(3, Some(CursorKind::Mine)), CursorKind::Mine);
        assert_eq!(
            go_cursor_kind(3, Some(CursorKind::GatherHerbs)),
            CursorKind::GatherHerbs
        );
        assert_eq!(
            go_cursor_kind(3, Some(CursorKind::PickLock)),
            CursorKind::PickLock
        );
    }

    #[test]
    fn go_cursor_stems_name_the_shipped_blps() {
        for (kind, stem) in [
            (CursorKind::Mail, "Mail"),
            (CursorKind::Mine, "Mine"),
            (CursorKind::GatherHerbs, "GatherHerbs"),
            (CursorKind::PickLock, "PickLock"),
        ] {
            assert_eq!(
                WorldCursor {
                    kind,
                    unable: false
                }
                .stem(),
                stem
            );
            assert_eq!(
                WorldCursor { kind, unable: true }.stem(),
                format!("Unable{stem}")
            );
        }
    }
}
