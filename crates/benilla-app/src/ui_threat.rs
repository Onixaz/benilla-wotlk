//! Each held unit's threat list (`SMSG_HIGHEST_THREAT_UPDATE` and `SMSG_THREAT_UPDATE` at
//! `0x741c90`, `SMSG_THREAT_REMOVE` at `0x737b20`, `SMSG_THREAT_CLEAR` at `0x734b00`), fed to the
//! threat verbs, with what its updates owe: `UNIT_THREAT_SITUATION_UPDATE` and
//! `UNIT_THREAT_LIST_UPDATE` queued once per unit and event and fanned out each frame (`0x6143f0`,
//! `0x614760`), the aggro sounds, the threat tutorial and the unit tooltip's rebuild.
//!
//! Not built: the overhead `COMBAT_THREAT_*` text (`0x719220`), which needs the name plate's
//! world-text lines.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{ThreatEntry, ThreatList, ThreatView, UiScript};

use crate::net::{GuidIndex, NetHandlerApp, ObjectStore, SelfGuid};

/// `0x71c3b0`'s sound when the player reaches the highest's threat (`threatPlaySounds`).
const SOUND_AT_HIGHEST: u32 = 15262;
/// `0x737620`'s sound when the player's tanking status moves (`threatPlaySounds`).
const SOUND_TANK_STATUS: u32 = 15263;
/// The level `0x52b370` asks before the threat tutorial.
const TUTORIAL_LEVEL: u32 = 14;

/// One of the two signals the lists queue, by unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Signal {
    Situation,
    List,
}

impl Signal {
    fn event(self) -> &'static str {
        match self {
            Signal::Situation => "UNIT_THREAT_SITUATION_UPDATE",
            Signal::List => "UNIT_THREAT_LIST_UPDATE",
        }
    }
}

/// A held unit's list and its client flag `0x8`: the player was just added to it, which mutes
/// the player's first status edge.
#[derive(Default)]
struct Held {
    list: ThreatList,
    player_new: bool,
}

/// Every held unit's list, and what the frame owes the VM.
#[derive(Resource, Default)]
pub(crate) struct Threat {
    lists: HashMap<u64, Held>,
    /// `(unit, signal)`, unique, in queue order (`0x6143f0`).
    signals: Vec<(u64, Signal)>,
    /// `SoundEntries` ids, before the `threatPlaySounds` gate.
    sounds: Vec<u32>,
    /// Units whose tooltip rebuilds (`0x512ab0`).
    tooltips: Vec<u64>,
    /// The player joined a list (`0x52b370`), before the warning and level gates.
    tutorial: bool,
}

impl Threat {
    fn signal(&mut self, guid: u64, signal: Signal) {
        if !self.signals.contains(&(guid, signal)) {
            self.signals.push((guid, signal));
        }
    }

    fn tooltip(&mut self, guid: u64) {
        if !self.tooltips.contains(&guid) {
            self.tooltips.push(guid);
        }
    }

    /// `0x7416f0`: the entry for `guid`, made at status 1, percent 255 and threat 0 if new, which
    /// flags the list when it is the player and enters the target's list map (`0x52b370`).
    fn create_entry(&mut self, unit: u64, guid: u64, me: u64) -> usize {
        let held = self.lists.entry(unit).or_default();
        if let Some(i) = held.list.entries.iter().position(|e| e.guid == guid) {
            return i;
        }
        held.list.entries.push(ThreatEntry {
            guid,
            status: 1,
            percent: 0xff,
            threat: 0,
            in_melee: false,
        });
        if guid == me {
            held.player_new = true;
            self.tutorial = true;
        }
        self.lists[&unit].list.entries.len() - 1
    }

    /// `0x71c3b0`: the entry's percent of `highest` (100 for a zero highest, 0 for no threat, else
    /// capped at 250), its status from it, 1 under 100 and 2 at or over; a status edge signals.
    fn update_entry(&mut self, unit: u64, i: usize, highest: i32, me: u64) -> bool {
        let held = self.lists.get_mut(&unit).expect("held list");
        let e = &mut held.list.entries[i];
        let percent = if highest == 0 {
            100
        } else if e.threat <= 0 {
            0
        } else {
            // Truncated to a byte as the reference stores it, so a negative highest wraps.
            let scaled = 100 * i64::from(e.threat) / i64::from(highest);
            if scaled >= 250 {
                250
            } else {
                scaled as u8
            }
        };
        if percent == e.percent {
            return false;
        }
        e.percent = percent;
        let status = if percent < 100 { 1 } else { 2 };
        let (guid, old) = (e.guid, e.status);
        let active = guid == me;
        let mut changed = false;
        if status != old {
            e.status = status;
            if active && status == 2 && old == 1 {
                self.sounds.push(SOUND_AT_HIGHEST);
            }
            self.signal(guid, Signal::Situation);
            changed = true;
        }
        if active {
            self.lists.get_mut(&unit).expect("held list").player_new = false;
            self.tooltip(unit);
        }
        changed
    }

    /// `0x737620`: the highest's status, 3 with anyone else at 2, else 4; an edge signals, and
    /// sounds for the player unless just added.
    fn update_status(&mut self, unit: u64, me: u64) {
        let held = self.lists.get_mut(&unit).expect("held list");
        let highest = held.list.highest;
        let Some(top) = held.list.entries.iter().position(|e| e.guid == highest) else {
            return;
        };
        let insecure = held
            .list
            .entries
            .iter()
            .any(|e| e.guid != highest && e.status == 2);
        let status = if insecure { 3 } else { 4 };
        let mut edge = false;
        if status != held.list.entries[top].status {
            if highest == me && !held.player_new {
                self.sounds.push(SOUND_TANK_STATUS);
            }
            self.lists.get_mut(&unit).expect("held list").list.entries[top].status = status;
            edge = true;
        }
        if edge {
            self.signal(highest, Signal::Situation);
        }
        if highest == me {
            self.lists.get_mut(&unit).expect("held list").player_new = false;
            self.tooltip(unit);
        }
    }

    /// `0x7417a0`: a new highest first when the packet names one (entered when non-zero), then
    /// each entry's threat; the percents recompute against the highest's threat, over the whole
    /// list when the highest is in it or the packet named it, else over the packet's entries.
    fn update(&mut self, unit: u64, highest: Option<u64>, entries: &[(u64, u32)], me: u64) {
        self.lists.entry(unit).or_default();
        let mut highest_in_list = highest.is_some();
        if let Some(h) = highest {
            self.lists.get_mut(&unit).expect("held list").list.highest = h;
            if h != 0 {
                self.create_entry(unit, h, me);
            }
        }
        let top_guid = self.lists[&unit].list.highest;
        let mut touched = Vec::with_capacity(entries.len());
        for &(guid, threat) in entries {
            let i = self.create_entry(unit, guid, me);
            // The wire's dword, kept signed.
            self.lists.get_mut(&unit).expect("held list").list.entries[i].threat = threat as i32;
            if guid == top_guid {
                highest_in_list = true;
            }
            touched.push(i);
        }
        let mut changed = false;
        let top = self.lists[&unit].list.entry(top_guid).map(|e| e.threat);
        if let (true, Some(top_threat)) = (top_guid != 0, top) {
            let indices: Vec<usize> = if highest_in_list {
                (0..self.lists[&unit].list.entries.len()).collect()
            } else {
                touched
            };
            for i in indices {
                if self.lists[&unit].list.entries[i].guid != top_guid
                    && self.update_entry(unit, i, top_threat, me)
                {
                    changed = true;
                }
            }
        }
        if highest.is_some() || changed {
            self.update_status(unit, me);
        }
        self.signal(unit, Signal::List);
    }

    /// `0x737750`: `guid` off the list, the highest cleared if it was; an entry at 2 or over
    /// recomputes the highest's status.
    fn remove(&mut self, unit: u64, guid: u64, me: u64) {
        let Some(held) = self.lists.get_mut(&unit) else {
            return;
        };
        let Some(i) = held.list.entries.iter().position(|e| e.guid == guid) else {
            return;
        };
        let mut update = false;
        if guid == held.list.highest {
            held.list.highest = 0;
        } else if held.list.entries[i].status > 1 {
            update = true;
        }
        held.list.entries.remove(i);
        self.signal(guid, Signal::Situation);
        if update {
            self.update_status(unit, me);
        }
        self.signal(unit, Signal::List);
        if guid == me {
            self.tooltip(unit);
        }
    }

    /// `0x7345c0`: every entry off in order, each signalled, then the highest cleared; an empty
    /// list does nothing.
    fn clear(&mut self, unit: u64) {
        let Some(held) = self.lists.get_mut(&unit) else {
            return;
        };
        if held.list.entries.is_empty() {
            return;
        }
        let gone = std::mem::take(&mut held.list.entries);
        held.list.highest = 0;
        for e in gone {
            self.signal(e.guid, Signal::Situation);
        }
        self.signal(unit, Signal::List);
        self.tooltip(unit);
    }
}

pub(crate) struct UiThreatPlugin;

impl Plugin for UiThreatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Threat>()
            .init_resource::<crate::sound::MessageSounds>()
            .net_handler(SessionEventKind::ThreatUpdate, on_threat)
            .net_handler(SessionEventKind::ThreatRemove, on_threat)
            .net_handler(SessionEventKind::ThreatClear, on_threat)
            .add_systems(
                Update,
                feed_threat
                    .in_set(crate::ui_unit::UnitFeed)
                    // After the snapshots and the resolver's guids its fan-out and tooltip read.
                    .after(crate::ui_unit::EnterWorldFeed)
                    .after(crate::ui_aura::AuraEvents),
            );
    }
}

/// The three handlers: each reads its unit first and drops the packet for a unit not held.
fn on_threat(
    In(ev): In<SessionEvent>,
    mut threat: ResMut<Threat>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    me: Res<SelfGuid>,
) {
    let held = |guid: u64| {
        index
            .0
            .get(&guid)
            .and_then(|&e| stores.get(e).ok())
            .is_some_and(ObjectStore::is_unit)
    };
    let me = me.0.unwrap_or(0);
    match ev {
        SessionEvent::ThreatUpdate(u) if held(u.unit) => {
            threat.update(u.unit, u.highest, &u.entries, me)
        }
        SessionEvent::ThreatRemove { unit, victim } if held(unit) => {
            threat.remove(unit, victim, me)
        }
        SessionEvent::ThreatClear { unit } if held(unit) => threat.clear(unit),
        _ => {}
    }
}

/// The despawns' clears (`0x7345c0` from the unit's teardown), each entry's melee range, the view,
/// then the tooltips, sounds, tutorial and signals the frame owes.
#[allow(clippy::too_many_arguments)] // a Bevy system's full input set
fn feed_threat(
    script: Option<NonSendMut<UiScript>>,
    mut threat: ResMut<Threat>,
    index: Res<GuidIndex>,
    units: Query<(&ObjectStore, &Transform)>,
    me: Res<SelfGuid>,
    mut sounds: ResMut<crate::sound::MessageSounds>,
    mut tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
    mut pushed: Local<crate::ui_script::VmMemo<Option<ThreatView>>>,
) {
    let unit_of = |guid: u64| {
        index
            .0
            .get(&guid)
            .and_then(|&e| units.get(e).ok())
            .filter(|(s, _)| s.is_unit())
    };
    let gone: Vec<u64> = threat
        .lists
        .keys()
        .copied()
        .filter(|&g| unit_of(g).is_none())
        .collect();
    for unit in gone {
        threat.clear(unit);
        threat.lists.remove(&unit);
    }
    // `0x737560`: in range when `max(reachA + reachB + 4/3, 5)` squared reaches the squared
    // distance between the two units.
    for (&unit, held) in threat.lists.iter_mut() {
        let Some((owner, owner_tf)) = unit_of(unit) else {
            continue;
        };
        for e in &mut held.list.entries {
            e.in_melee = unit_of(e.guid).is_some_and(|(other, tf)| {
                let reach = (owner.0.unit_combat_reach() + other.0.unit_combat_reach() + 4.0 / 3.0)
                    .max(5.0);
                reach * reach >= owner_tf.translation.distance_squared(tf.translation)
            });
        }
    }

    let Some(mut script) = script else {
        return;
    };
    let view = ThreatView {
        lists: threat
            .lists
            .iter()
            .map(|(&g, held)| (g, held.list.clone()))
            .collect(),
    };
    let last = pushed.get(&script);
    if last.as_ref() != Some(&view) {
        script.set_threat(view.clone());
        *last = Some(view);
    }

    for unit in std::mem::take(&mut threat.tooltips) {
        script.refresh_unit_tooltip(unit);
    }
    let play = script
        .cvar("threatPlaySounds")
        .is_some_and(|v| v.trim().parse::<i64>().unwrap_or(0) != 0);
    for id in std::mem::take(&mut threat.sounds) {
        if play {
            sounds.push_kit(id);
        }
    }
    if std::mem::take(&mut threat.tutorial) && script.threat_warning_enabled() {
        let level =
            me.0.and_then(unit_of)
                .and_then(|(s, _)| s.0.unit_level())
                .unwrap_or(0);
        if level >= TUTORIAL_LEVEL {
            if let Some(t) = tutorials.as_mut() {
                t.write(crate::tutorial::TutorialEvent::trigger(
                    crate::tutorial::id::THREAT,
                ));
            }
        }
    }
    for (guid, signal) in std::mem::take(&mut threat.signals) {
        script.fire_unit_event(guid, signal.event(), vec![]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u64 = 0x10;
    const TANK: u64 = 0x21;
    const MOB: u64 = 0xF130_0000_0C35_0001;

    fn status(t: &Threat, guid: u64) -> (u8, u8) {
        let e = t.lists[&MOB].list.entry(guid).unwrap();
        (e.status, e.percent)
    }

    /// A pull: the highest update makes the tank the highest, alone at 4, and the player under it
    /// at 1 with its percent; the tank's own percent stays 255.
    #[test]
    fn a_highest_update_sets_statuses_and_percents() {
        let mut t = Threat::default();
        t.update(MOB, Some(TANK), &[(TANK, 20_000), (ME, 13_000)], ME);
        assert_eq!(t.lists[&MOB].list.highest, TANK);
        assert_eq!(status(&t, TANK), (4, 255));
        assert_eq!(status(&t, ME), (1, 65));
        assert_eq!(
            t.signals,
            [(TANK, Signal::Situation), (MOB, Signal::List)],
            "the player stays at 1, so only the tank's status moved"
        );
        assert!(t.tutorial, "the player joined a list");
        assert!(t.sounds.is_empty());
    }

    /// The player passes the tank: 2 with the at-highest sound, the tank to 3 with no sound,
    /// each signalled once however many packets land in a frame.
    #[test]
    fn passing_the_highest_makes_the_tank_insecure() {
        let mut t = Threat::default();
        t.update(MOB, Some(TANK), &[(TANK, 20_000), (ME, 13_000)], ME);
        t.signals.clear();
        t.update(MOB, None, &[(ME, 21_000)], ME);
        t.update(MOB, None, &[(ME, 22_000)], ME);
        assert_eq!(status(&t, ME), (2, 110));
        assert_eq!(status(&t, TANK), (3, 255));
        assert_eq!(t.sounds, [SOUND_AT_HIGHEST]);
        assert_eq!(
            t.signals,
            [
                (ME, Signal::Situation),
                (TANK, Signal::Situation),
                (MOB, Signal::List)
            ]
        );
    }

    /// The player as the highest: the first tanking edge is muted by the new-entry flag, a later
    /// one sounds.
    #[test]
    fn the_players_tank_edge_sounds_after_its_first() {
        let mut t = Threat::default();
        t.update(MOB, Some(ME), &[(ME, 500), (TANK, 100)], ME);
        assert_eq!(status(&t, ME), (4, 255));
        assert!(t.sounds.is_empty(), "muted: the player was just added");
        t.update(MOB, None, &[(TANK, 600)], ME);
        assert_eq!(status(&t, ME), (3, 255));
        assert_eq!(t.sounds, [SOUND_TANK_STATUS]);
    }

    /// Removing an entry at 2 recomputes the highest; removing the highest clears it; a clear
    /// signals each entry, then the list, and an empty list signals nothing.
    #[test]
    fn remove_and_clear_signal_as_the_reference_does() {
        let mut t = Threat::default();
        t.update(MOB, Some(TANK), &[(TANK, 100), (ME, 120)], ME);
        assert_eq!(status(&t, TANK), (3, 255));
        t.signals.clear();
        t.remove(MOB, ME, ME);
        assert_eq!(status(&t, TANK), (4, 255));
        assert_eq!(
            t.signals,
            [
                (ME, Signal::Situation),
                (TANK, Signal::Situation),
                (MOB, Signal::List)
            ]
        );
        t.remove(MOB, TANK, ME);
        assert_eq!(t.lists[&MOB].list.highest, 0);
        t.update(MOB, None, &[(ME, 5)], ME);
        t.signals.clear();
        t.clear(MOB);
        assert_eq!(t.signals, [(ME, Signal::Situation), (MOB, Signal::List)]);
        assert!(t.lists[&MOB].list.entries.is_empty());
        t.signals.clear();
        t.clear(MOB);
        assert!(t.signals.is_empty());
    }

    /// Percent laws: a zero highest reads 100, no threat 0, and the cap is 250.
    #[test]
    fn percents_follow_the_highests_threat() {
        let mut t = Threat::default();
        t.update(MOB, Some(TANK), &[(TANK, 0), (ME, 7)], ME);
        assert_eq!(status(&t, ME), (2, 100));
        t.update(MOB, None, &[(TANK, 10), (ME, 0)], ME);
        assert_eq!(status(&t, ME), (1, 0));
        t.update(MOB, None, &[(ME, 1_000)], ME);
        assert_eq!(status(&t, ME), (2, 250));
    }
}
