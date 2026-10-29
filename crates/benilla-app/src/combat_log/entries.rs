//! The 3.3.5a combat-log entries, built per producer and fired once named.

use std::collections::VecDeque;

use benilla_protocol::messages::{
    hit_info, AttackerState, GROUP_MEMBER_MAIN_ASSIST, GROUP_MEMBER_MAIN_TANK,
};
use benilla_ui::script::{
    CombatLogEntry, CombatLogEvent, CombatLogInfo, CombatLogSpell, CombatLogUnit, Damage, Miss,
    MissType,
};
use bevy::prelude::*;

use crate::names::{NameCache, NameKey};
use crate::net::{NetCommands, ObjectStore, Reputations};
use crate::target::ring::Factions;
use crate::ui_party::GroupState;

/// `time()` and the client clock at world entry, the timestamp base (`0x7538a0`).
#[derive(Resource, Default, Clone, Copy)]
pub(crate) struct CombatLogClock {
    epoch_secs: i64,
    at_ms: u64,
}

impl CombatLogClock {
    fn timestamp(self, now_ms: u64) -> f64 {
        self.epoch_secs as f64 + now_ms.saturating_sub(self.at_ms) as f64 * 0.001
    }
}

/// How an endpoint's name is looked up (`0x74fd40`): a unit's cache, or a game object's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NameRef {
    Unit(NameKey),
    GameObject,
}

/// One entry waiting on its names; each key empties as its name lands.
struct Pending {
    entry: CombatLogEntry,
    keys: [Option<NameRef>; 2],
    /// The entry's item, named off the item cache (`0x750210`).
    item: Option<u32>,
}

/// Entries not yet fired, oldest first; the head holds the rest (`0x74f910`).
#[derive(Resource, Default)]
pub(crate) struct PendingEntries(VecDeque<Pending>);

/// Deviates: 12340 waits on a name unbounded (`0x74f720`); an unanswered pet name would stall all.
const NAME_WAIT_MS: u64 = 2_000;

/// What an endpoint's flags read (`0x74dcb0`).
pub(crate) struct FlagInputs<'a> {
    pub me: Option<u64>,
    pub group: &'a GroupState,
    pub target: Option<u64>,
    pub focus: Option<u64>,
    pub factions: Option<&'a Factions>,
    pub reputations: &'a Reputations,
}

/// The 12340 player-guid test: high nibble clear and some bit set (`0x74dcb0`).
fn is_player_guid(guid: u64) -> bool {
    let hi = (guid >> 32) as u32;
    hi & 0xf000_0000 == 0 && (guid as u32 != 0 || hi & 0xf07f_ffff != 0)
}

/// The guid family the reference tests with `hi & 0xf0f00000`.
fn family(guid: u64) -> u32 {
    (guid >> 32) as u32 & 0xf0f0_0000
}

const GAME_OBJECT: u32 = 0xf010_0000;
const CREATURE: u32 = 0xf030_0000;
const PET: u32 = 0xf040_0000;
const VEHICLE: u32 = 0xf050_0000;

/// A unit's owner: `UNIT_FIELD_CHARMEDBY`, else `UNIT_FIELD_CREATEDBY` (`0x4f5f20`); a game
/// object's creator.
fn owner_of(store: &ObjectStore) -> Option<u64> {
    let set = |g: Option<u64>| g.filter(|&g| g != 0);
    if store.is_unit() {
        set(store.0.unit_charmed_by()).or_else(|| set(store.0.unit_created_by()))
    } else {
        set(store.0.gameobject_created_by())
    }
}

/// The main-tank or main-assist assignment of a group member (`0x74dcb0`'s party/raid read).
fn assignment(group: &GroupState, guid: u64, me: Option<u64>) -> u8 {
    if me == Some(guid) {
        return group.own_flags;
    }
    group
        .members
        .iter()
        .find(|m| m.guid == guid)
        .map_or(0, |m| m.flags)
}

/// `COMBATLOG_OBJECT_*` for an endpoint (`0x74dcb0`).
pub(crate) fn unit_flags<'s>(
    guid: u64,
    inputs: &FlagInputs,
    store_of: impl Fn(u64) -> Option<&'s ObjectStore>,
) -> u32 {
    if guid == 0 {
        return benilla_ui::script::OBJECT_NONE;
    }
    let group = inputs.group;
    let in_raid = group.in_group && group.group_type == 1;
    let grouped = group.in_group;
    let unit = store_of(guid);
    let mut controller = guid;
    if let Some(owner) = unit.and_then(owner_of) {
        controller = owner;
        // An NPC owner hands control to its own owner, one step up.
        if !is_player_guid(owner) {
            if let Some(up) = store_of(owner).and_then(owner_of) {
                controller = up;
            }
        }
    }

    let fam = family(guid);
    let mut flags = if is_player_guid(guid) || matches!(fam, CREATURE | PET | VEHICLE) {
        if controller == guid {
            if is_player_guid(guid) {
                0x400
            } else if fam == PET {
                0x1000
            } else {
                0x800
            }
        } else {
            let summoned = unit.and_then(|u| {
                let set = |g: Option<u64>| g.filter(|&g| g != 0);
                set(u.0.unit_charmed_by()).or_else(|| set(u.0.unit_summoned_by()))
            });
            if summoned.is_some() {
                0x1000
            } else {
                0x2000
            }
        }
    } else {
        0x4000
    };
    flags |= if is_player_guid(controller) {
        0x100
    } else {
        0x200
    };

    if inputs.me == Some(controller) {
        flags |= 0x11;
    } else {
        let mut hostile = false;
        if let (Some(unit), Some(me)) =
            (unit.filter(|u| u.is_unit()), inputs.me.and_then(&store_of))
        {
            let rank = crate::target::ring::ring_reaction(
                inputs.factions,
                inputs.reputations,
                inputs.group,
                Some(unit),
                Some(me),
            );
            if rank >= 4 {
                flags |= 0x10;
            } else if rank <= 1 {
                flags |= 0x40;
                hostile = true;
            }
        }
        if !hostile && grouped {
            let member = group.members.iter().find(|m| m.guid == controller);
            let same_party = member.is_some_and(|m| m.flags & 0x7 == group.own_flags & 0x7);
            if same_party {
                flags |= 0x12;
            } else if in_raid && member.is_some() {
                flags |= 0x14;
            }
        }
        if flags & 0xf == 0 {
            flags |= 0x8;
        }
        if flags & 0xf0 == 0 {
            flags |= 0x20;
        }
    }
    if inputs.target == Some(guid) {
        flags |= 0x10000;
    }
    if inputs.focus == Some(guid) {
        flags |= 0x20000;
    }
    // A player in the group, or ours while grouped, carries its tank or assist assignment.
    if flags & 0x400 != 0 && (flags & 0x6 != 0 || (inputs.me == Some(controller) && grouped)) {
        let assigned = assignment(group, guid, inputs.me);
        if assigned & GROUP_MEMBER_MAIN_TANK != 0 {
            flags |= 0x40000;
        } else if assigned & GROUP_MEMBER_MAIN_ASSIST != 0 {
            flags |= 0x80000;
        }
    }
    if grouped {
        if let Some(icon) = group.raid_targets.iter().position(|&g| g == guid) {
            flags |= 0x100000 << icon;
        }
    }
    flags
}

/// The miss info `0x74d980` records: only a resist, block or absorb carries its amount.
pub(crate) fn miss(kind: MissType, amount: i32) -> Miss {
    Miss {
        kind,
        amount: matches!(kind, MissType::Resist | MissType::Block | MissType::Absorb)
            .then_some(amount),
    }
}

/// A swing's entry (`0x7512b0`), or none for a hidden swing or a state that logs nothing.
pub(crate) fn swing_entry(s: &AttackerState) -> Option<(CombatLogEvent, CombatLogInfo)> {
    if !s.displayed() {
        return None;
    }
    if matches!(s.victim_state, 1 | 5) && s.damage != 0 {
        let info = CombatLogInfo {
            damage: Some(Damage {
                amount: s.damage as i32,
                overkill: s.overkill as i32,
                school: s.school_mask,
                resisted: s.resist,
                blocked: s.blocked as i32,
                absorbed: s.absorb as i32,
                critical: s.hit_info & hit_info::CRIT != 0,
                glancing: s.hit_info & hit_info::GLANCING != 0,
                crushing: s.hit_info & hit_info::CRUSHING != 0,
            }),
            ..CombatLogInfo::default()
        };
        return Some((CombatLogEvent::SwingDamage, info));
    }
    let kind = if s.hit_info & hit_info::MISS != 0 {
        miss(MissType::Miss, 0)
    } else if s.hit_info & hit_info::FULL_ABSORB != 0 {
        miss(MissType::Absorb, s.absorb as i32)
    } else if s.hit_info & hit_info::FULL_RESIST != 0 {
        miss(MissType::Resist, s.resist)
    } else {
        let kind = match s.victim_state {
            2 => MissType::Dodge,
            3 => MissType::Parry,
            5 => MissType::Block,
            6 => MissType::Evade,
            7 => MissType::Immune,
            8 => MissType::Deflect,
            _ => return None,
        };
        miss(kind, s.blocked as i32)
    };
    Some((
        CombatLogEvent::SwingMissed,
        CombatLogInfo {
            miss: Some(kind),
            ..CombatLogInfo::default()
        },
    ))
}

/// One entry before its endpoints resolve: the producer's event, guids, spell and suffix.
pub(crate) struct NewEntry {
    pub event: CombatLogEvent,
    pub source: u64,
    pub dest: u64,
    pub spell: Option<CombatLogSpell>,
    /// `AttributesExD & 0x2000`: the spell hides its caster (`0x750400`).
    pub hide_source: bool,
    pub info: CombatLogInfo,
}

impl PendingEntries {
    /// Queue an entry (`0x750400`, `0x74ff20`), its flags and name keys taken now; `now_ms` is the
    /// event's own client time.
    pub(crate) fn queue<'s>(
        &mut self,
        new: NewEntry,
        (now_ms, clock): (u64, CombatLogClock),
        inputs: &FlagInputs,
        store_of: impl Fn(u64) -> Option<&'s ObjectStore> + Copy,
    ) {
        let end = |guid: u64| {
            if guid == 0 {
                return (CombatLogUnit::none(), None);
            }
            let unit = CombatLogUnit {
                guid,
                name: None,
                flags: unit_flags(guid, inputs, store_of),
            };
            let key = if family(guid) == GAME_OBJECT {
                Some(NameRef::GameObject)
            } else {
                NameKey::of(guid, store_of(guid)).map(NameRef::Unit)
            };
            (unit, key)
        };
        let (source, source_key) = end(if new.hide_source { 0 } else { new.source });
        let (dest, dest_key) = end(new.dest);
        let item = new.info.item.as_ref().map(|i| i.id);
        self.0.push_back(Pending {
            entry: CombatLogEntry {
                time_ms: now_ms,
                timestamp: clock.timestamp(now_ms),
                event: new.event,
                source,
                dest,
                spell: new.spell,
                info: new.info,
            },
            keys: [source_key, dest_key],
            item,
        });
    }
}

#[cfg(test)]
impl PendingEntries {
    /// The queued entries, oldest first.
    pub(crate) fn entries(&self) -> Vec<&CombatLogEntry> {
        self.0.iter().map(|p| &p.entry).collect()
    }
}

/// `0x74d210`: a creature with template type flag `0x4000` shows no spawn counter, as in
/// `UnitGUID` (`0x60e6e1`).
pub(crate) fn shown_guid(guid: u64, names: &NameCache) -> u64 {
    if !matches!(family(guid), CREATURE | VEHICLE) {
        return guid;
    }
    let entry = ((guid >> 24) & 0xff_ffff) as u32;
    let hidden = names
        .creature_record(entry)
        .is_some_and(|r| r.type_flags & 0x4000 != 0);
    if hidden {
        guid & !0xff_ffff
    } else {
        guid
    }
}

/// An entry fired with its names in, for the feed to hand to the UI.
#[derive(Message)]
pub(crate) struct CombatLogFired(pub(crate) CombatLogEntry);

/// The caches an entry's names come from.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct NameSources<'w> {
    names: Res<'w, NameCache>,
    net: Res<'w, NetCommands>,
    items: Option<Res<'w, crate::items::Items>>,
    objects: Option<Res<'w, crate::go_templates::GameObjectTemplates>>,
}

impl NameSources<'_> {
    /// The answer for one endpoint: `None` in flight, `Some(None)` answered empty.
    fn answer(&self, key: NameRef, guid: u64) -> Option<Option<String>> {
        match key {
            NameRef::Unit(k) => self
                .names
                .answer(k, guid, &self.net)
                .map(|n| n.map(str::to_owned)),
            NameRef::GameObject => {
                let objects = self.objects.as_deref()?;
                match objects.get(guid) {
                    Some(t) => Some(Some(t.name.clone())),
                    None => {
                        objects.request(guid, &self.net);
                        None
                    }
                }
            }
        }
    }

    /// The item's name: `None` in flight, `Some(None)` answered unknown.
    fn item(&self, entry: u32) -> Option<Option<String>> {
        let items = self.items.as_deref()?;
        match items.template(entry, 0, &self.net) {
            Some(t) => Some(Some(t.name.clone())),
            None if items.template_answered_unknown(entry) => Some(None),
            None => None,
        }
    }
}

/// Fire the queued entries whose names are in, oldest first (`0x74f910`).
pub(crate) fn fire_pending(
    mut fired: MessageWriter<CombatLogFired>,
    mut pending: ResMut<PendingEntries>,
    sources: NameSources,
    time: Option<Res<Time<bevy::time::Real>>>,
) {
    let Some(time) = time else {
        return;
    };
    let now_ms = time.elapsed().as_millis() as u64;
    while let Some(head) = pending.0.front_mut() {
        let mut ready = true;
        let Pending { entry, keys, item } = head;
        let [source_key, dest_key] = keys;
        for (unit, key) in [(&mut entry.source, source_key), (&mut entry.dest, dest_key)] {
            let Some(k) = *key else { continue };
            match sources.answer(k, unit.guid) {
                Some(name) => {
                    unit.name = name;
                    *key = None;
                }
                None => ready = false,
            }
        }
        if let Some(id) = *item {
            match sources.item(id) {
                Some(name) => {
                    if let Some(i) = entry.info.item.as_mut() {
                        i.name = name;
                    }
                    *item = None;
                }
                None => ready = false,
            }
        }
        if !ready && now_ms.saturating_sub(entry.time_ms) < NAME_WAIT_MS {
            break;
        }
        let Some(Pending { mut entry, .. }) = pending.0.pop_front() else {
            break;
        };
        entry.source.guid = shown_guid(entry.source.guid, &sources.names);
        entry.dest.guid = shown_guid(entry.dest.guid, &sources.names);
        fired.write(CombatLogFired(entry));
    }
}

/// `0x7538a0` at world entry: the timestamp base taken now, nothing carried from the last world.
pub(crate) fn on_world_entry(
    mut entered: MessageReader<crate::net::EnteredWorldMessage>,
    mut clock: ResMut<CombatLogClock>,
    mut pending: ResMut<PendingEntries>,
    time: Option<Res<Time<bevy::time::Real>>>,
) {
    if entered.read().count() == 0 {
        return;
    }
    let at_ms = time.map_or(0, |t| t.elapsed().as_millis() as u64);
    let epoch_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    *clock = CombatLogClock { epoch_secs, at_ms };
    pending.0.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn swing(hit: u32, state: u32, damage: u32) -> AttackerState {
        AttackerState {
            attacker: 0x3ee,
            victim: 0xf130_0000_060c_281e,
            hit_info: hit,
            damage,
            overkill: 0,
            victim_state: state,
            absorb: 0,
            resist: 0,
            blocked: 0,
            melee_spell_id: 0,
            school_mask: 1,
        }
    }

    fn damage_of(s: &AttackerState) -> Option<Damage> {
        match swing_entry(s) {
            Some((CombatLogEvent::SwingDamage, info)) => info.damage,
            _ => None,
        }
    }

    fn miss_of(s: AttackerState) -> Option<(MissType, Option<i32>)> {
        match swing_entry(&s) {
            Some((CombatLogEvent::SwingMissed, info)) => info.miss.map(|m| (m.kind, m.amount)),
            _ => None,
        }
    }

    #[test]
    fn a_landed_swing_is_swing_damage_with_its_bits() {
        let d = damage_of(&AttackerState {
            overkill: 7,
            ..swing(
                hit_info::AFFECTS_VICTIM | hit_info::CRIT | hit_info::CRUSHING,
                1,
                15,
            )
        })
        .expect("a hit logs damage");
        assert_eq!((d.amount, d.overkill, d.school), (15, 7, 1));
        assert!(d.critical && d.crushing && !d.glancing);
    }

    #[test]
    fn a_partial_block_still_logs_damage() {
        let s = AttackerState {
            blocked: 4,
            ..swing(hit_info::AFFECTS_VICTIM | hit_info::BLOCK, 5, 11)
        };
        let d = damage_of(&s).expect("damage through a block logs damage");
        assert_eq!((d.amount, d.blocked), (11, 4));
    }

    #[test]
    fn a_swing_that_does_not_land_logs_its_miss_word() {
        assert_eq!(
            miss_of(swing(hit_info::MISS, 0, 0)),
            Some((MissType::Miss, None))
        );
        assert_eq!(miss_of(swing(0, 2, 0)), Some((MissType::Dodge, None)));
        assert_eq!(miss_of(swing(0, 3, 0)), Some((MissType::Parry, None)));
        assert_eq!(miss_of(swing(0, 8, 0)), Some((MissType::Deflect, None)));
        let absorbed = AttackerState {
            absorb: 30,
            ..swing(hit_info::FULL_ABSORB, 1, 0)
        };
        assert_eq!(miss_of(absorbed), Some((MissType::Absorb, Some(30))));
        let resisted = AttackerState {
            resist: 12,
            ..swing(hit_info::FULL_RESIST, 1, 0)
        };
        assert_eq!(miss_of(resisted), Some((MissType::Resist, Some(12))));
        let blocked = AttackerState {
            blocked: 9,
            ..swing(hit_info::BLOCK, 5, 0)
        };
        assert_eq!(miss_of(blocked), Some((MissType::Block, Some(9))));
    }

    /// A zero-damage hit with no miss bit, and a hidden on-next-swing miss, log nothing.
    #[test]
    fn some_swings_log_nothing() {
        assert_eq!(swing_entry(&swing(hit_info::AFFECTS_VICTIM, 1, 0)), None);
        let hidden = AttackerState {
            melee_spell_id: 78,
            ..swing(0, 2, 0)
        };
        assert_eq!(swing_entry(&hidden), None);
    }

    #[test]
    fn the_player_guid_test_is_the_12340_one() {
        assert!(is_player_guid(0x3ee));
        assert!(!is_player_guid(0));
        assert!(!is_player_guid(0xf130_0000_060c_281e));
        assert_eq!(family(0xf130_0000_060c_281e), CREATURE);
        assert_eq!(family(0xf140_0012_3400_0001), PET);
        assert_eq!(family(0xf110_0000_0100_0001), GAME_OBJECT);
    }

    #[test]
    fn the_timestamp_is_wall_seconds_plus_elapsed() {
        let clock = CombatLogClock {
            epoch_secs: 1_000,
            at_ms: 500,
        };
        assert_eq!(clock.timestamp(2_750), 1_002.25);
    }

    /// The focus and a main-tank assignment set their bits; another's tank flag needs grouping.
    #[test]
    fn focus_and_main_tank_set_their_bits() {
        let group = GroupState {
            in_group: true,
            group_type: 1,
            own_flags: GROUP_MEMBER_MAIN_TANK,
            ..GroupState::default()
        };
        let reputations = Reputations::default();
        let inputs = FlagInputs {
            me: Some(0x3ee),
            group: &group,
            target: None,
            focus: Some(0x3ee),
            factions: None,
            reputations: &reputations,
        };
        let flags = unit_flags(0x3ee, &inputs, |_| None);
        assert_eq!(
            flags & 0x511,
            0x511,
            "mine, friendly, player-controlled player"
        );
        assert_eq!(flags & 0x20000, 0x20000, "the focus");
        assert_eq!(flags & 0x40000, 0x40000, "the main tank");
    }
}
