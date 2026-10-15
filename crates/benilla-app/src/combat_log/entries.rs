//! The 3.3.5a combat-log entries (`UnitCombatLog_C.cpp`), built per packet and fired once named.

use std::collections::VecDeque;

use benilla_protocol::messages::{hit_info, AttackerState};
use benilla_ui::script::{
    CombatLogEntry, CombatLogPayload, CombatLogUnit, Damage, MissType, UiScript,
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

/// One entry waiting on its names; `keys` empties as each name lands.
struct Pending {
    entry: CombatLogEntry,
    keys: [Option<NameKey>; 2],
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

const CREATURE: u32 = 0xf030_0000;
const PET: u32 = 0xf040_0000;
const VEHICLE: u32 = 0xf050_0000;

/// A unit's owner: `UNIT_FIELD_CHARMEDBY`, else `UNIT_FIELD_CREATEDBY` (`0x4f5f20`).
fn owner_of(store: &ObjectStore) -> Option<u64> {
    let set = |g: Option<u64>| g.filter(|&g| g != 0);
    set(store.0.unit_charmed_by()).or_else(|| set(store.0.unit_created_by()))
}

/// `COMBATLOG_OBJECT_*` for an endpoint (`0x74dcb0`); focus, main tank and main assist unset.
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
        if let (Some(unit), Some(me)) = (unit, inputs.me.and_then(&store_of)) {
            let rank = crate::target::ring::ring_reaction(
                inputs.factions,
                inputs.reputations,
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
    if grouped {
        if let Some(icon) = group.raid_targets.iter().position(|&g| g == guid) {
            flags |= 0x100000 << icon;
        }
    }
    flags
}

/// A swing's payload (`0x7512b0`), or none for a hidden swing or a state that logs nothing.
pub(crate) fn swing_payload(s: &AttackerState) -> Option<CombatLogPayload> {
    if !s.displayed() {
        return None;
    }
    if matches!(s.victim_state, 1 | 5) && s.damage != 0 {
        return Some(CombatLogPayload::SwingDamage(Damage {
            amount: s.damage as i32,
            overkill: s.overkill as i32,
            school: s.school_mask,
            resisted: s.resist,
            blocked: s.blocked as i32,
            absorbed: s.absorb as i32,
            critical: s.hit_info & hit_info::CRIT != 0,
            glancing: s.hit_info & hit_info::GLANCING != 0,
            crushing: s.hit_info & hit_info::CRUSHING != 0,
        }));
    }
    let missed = |miss, amount| Some(CombatLogPayload::SwingMissed { miss, amount });
    if s.hit_info & hit_info::MISS != 0 {
        return missed(MissType::Miss, None);
    }
    if s.hit_info & hit_info::FULL_ABSORB != 0 {
        return missed(MissType::Absorb, Some(s.absorb as i32));
    }
    if s.hit_info & hit_info::FULL_RESIST != 0 {
        return missed(MissType::Resist, Some(s.resist));
    }
    match s.victim_state {
        2 => missed(MissType::Dodge, None),
        3 => missed(MissType::Parry, None),
        5 => missed(MissType::Block, Some(s.blocked as i32)),
        6 => missed(MissType::Evade, None),
        7 => missed(MissType::Immune, None),
        8 => missed(MissType::Deflect, None),
        _ => None,
    }
}

impl PendingEntries {
    /// Queue an entry from `source` to `dest` (`0x750400`), its flags and name keys taken now.
    pub(crate) fn queue<'s>(
        &mut self,
        (source, dest): (u64, u64),
        payload: CombatLogPayload,
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
            (unit, NameKey::of(guid, store_of(guid)))
        };
        let (source, source_key) = end(source);
        let (dest, dest_key) = end(dest);
        self.0.push_back(Pending {
            entry: CombatLogEntry {
                time_ms: now_ms,
                timestamp: clock.timestamp(now_ms),
                source,
                dest,
                payload,
            },
            keys: [source_key, dest_key],
        });
    }
}

/// `0x74d210`: a creature with template type flag `0x4000` shows no spawn counter.
fn shown_guid(guid: u64, names: &NameCache) -> u64 {
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

/// Fire the queued entries whose names are in, oldest first (`0x74f910`).
pub(crate) fn fire_pending(
    script: Option<NonSendMut<UiScript>>,
    mut pending: ResMut<PendingEntries>,
    names: Res<NameCache>,
    net: Res<NetCommands>,
    time: Option<Res<Time<bevy::time::Real>>>,
) {
    let (Some(mut script), Some(time)) = (script, time) else {
        return;
    };
    let now_ms = time.elapsed().as_millis() as u64;
    while let Some(head) = pending.0.front_mut() {
        let mut ready = true;
        let Pending { entry, keys } = head;
        let [source_key, dest_key] = keys;
        for (unit, key) in [(&mut entry.source, source_key), (&mut entry.dest, dest_key)] {
            let Some(k) = *key else { continue };
            match names.answer(k, unit.guid, &net) {
                Some(name) => {
                    unit.name = name.map(str::to_owned);
                    *key = None;
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
        entry.source.guid = shown_guid(entry.source.guid, &names);
        entry.dest.guid = shown_guid(entry.dest.guid, &names);
        script.push_combat_log(entry);
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

    #[test]
    fn a_landed_swing_is_swing_damage_with_its_bits() {
        let Some(CombatLogPayload::SwingDamage(d)) = swing_payload(&AttackerState {
            overkill: 7,
            ..swing(
                hit_info::AFFECTS_VICTIM | hit_info::CRIT | hit_info::CRUSHING,
                1,
                15,
            )
        }) else {
            panic!("a hit logs damage");
        };
        assert_eq!((d.amount, d.overkill, d.school), (15, 7, 1));
        assert!(d.critical && d.crushing && !d.glancing);
    }

    #[test]
    fn a_partial_block_still_logs_damage() {
        let s = AttackerState {
            blocked: 4,
            ..swing(hit_info::AFFECTS_VICTIM | hit_info::BLOCK, 5, 11)
        };
        let Some(CombatLogPayload::SwingDamage(d)) = swing_payload(&s) else {
            panic!("damage through a block logs damage");
        };
        assert_eq!((d.amount, d.blocked), (11, 4));
    }

    #[test]
    fn a_swing_that_does_not_land_logs_its_miss_word() {
        let miss = |s: AttackerState| match swing_payload(&s) {
            Some(CombatLogPayload::SwingMissed { miss, amount }) => Some((miss, amount)),
            _ => None,
        };
        assert_eq!(
            miss(swing(hit_info::MISS, 0, 0)),
            Some((MissType::Miss, None))
        );
        assert_eq!(miss(swing(0, 2, 0)), Some((MissType::Dodge, None)));
        assert_eq!(miss(swing(0, 3, 0)), Some((MissType::Parry, None)));
        assert_eq!(miss(swing(0, 8, 0)), Some((MissType::Deflect, None)));
        let absorbed = AttackerState {
            absorb: 30,
            ..swing(hit_info::FULL_ABSORB, 1, 0)
        };
        assert_eq!(miss(absorbed), Some((MissType::Absorb, Some(30))));
        let resisted = AttackerState {
            resist: 12,
            ..swing(hit_info::FULL_RESIST, 1, 0)
        };
        assert_eq!(miss(resisted), Some((MissType::Resist, Some(12))));
        let blocked = AttackerState {
            blocked: 9,
            ..swing(hit_info::BLOCK, 5, 0)
        };
        assert_eq!(miss(blocked), Some((MissType::Block, Some(9))));
    }

    /// A zero-damage hit with no miss bit, and a hidden on-next-swing miss, log nothing.
    #[test]
    fn some_swings_log_nothing() {
        assert_eq!(swing_payload(&swing(hit_info::AFFECTS_VICTIM, 1, 0)), None);
        let hidden = AttackerState {
            melee_spell_id: 78,
            ..swing(0, 2, 0)
        };
        assert_eq!(swing_payload(&hidden), None);
    }

    #[test]
    fn the_player_guid_test_is_the_12340_one() {
        assert!(is_player_guid(0x3ee));
        assert!(!is_player_guid(0));
        assert!(!is_player_guid(0xf130_0000_060c_281e));
        assert_eq!(family(0xf130_0000_060c_281e), CREATURE);
        assert_eq!(family(0xf140_0012_3400_0001), PET);
    }

    #[test]
    fn the_timestamp_is_wall_seconds_plus_elapsed() {
        let clock = CombatLogClock {
            epoch_secs: 1_000,
            at_ms: 500,
        };
        assert_eq!(clock.timestamp(2_750), 1_002.25);
    }
}
