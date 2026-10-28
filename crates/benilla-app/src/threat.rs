//! Threat lists: each held unit's view of who it hates (`SMSG_THREAT_UPDATE`,
//! `SMSG_HIGHEST_THREAT_UPDATE`, `SMSG_THREAT_REMOVE`, `SMSG_THREAT_CLEAR`), kept by the reference's
//! rules, pushed to the VM for `UnitThreatSituation` and `UnitDetailedThreatSituation`, and
//! signalled as `UNIT_THREAT_LIST_UPDATE` and `UNIT_THREAT_SITUATION_UPDATE`.
//!
//! The rules are the unit's (`0x7417a0` the update, `0x71c3b0` one row, `0x737620` the top's
//! status, `0x737750` a removal, `0x7345c0` a clear): a row's status is 1 below the top's threat
//! and 2 at or above it, while the top reads 4, or 3 once any other row reaches 2. A row's raw
//! percent of the top's threat is capped at 250 and starts at 255 until first computed.
//!
//! Not built: the floating `COMBAT_THREAT_*` text over the unit (`0x719220`), the nameplate's
//! threat flash (`0x98e9f0` reads our status; benilla's plate is still the 1.12 widget) and the
//! unit tooltip's `THREAT_TOOLTIP` line with its icon (`0x621070`; the tooltip has no texture
//! rows yet).

use std::collections::HashMap;
use std::time::Instant;

use bevy::prelude::*;

use benilla_protocol::messages::ThreatEntry;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{MobThreat, ThreatRow, UiScript};

use crate::net::{GuidIndex, NetHandlerApp, ObjectStore, SelfGuid};
use crate::ui_script::UiFeed;

/// The aggro sounds, `SoundEntries` ids: we pass the top's threat while not tanking
/// (`0x71c3b0`), and we lose the secure hold on the unit as its top (`0x737620`).
const SOUND_PULLED_AGGRO: u32 = 15262;
const SOUND_LOSING_AGGRO: u32 = 15263;
/// The threat tutorial, triggered when we join a list at level 14 or more with the threat
/// warning on (`0x52b370`).
const TUTORIAL_THREAT: u32 = 53;
const TUTORIAL_THREAT_LEVEL: u32 = 14;

/// One row of a unit's list (`0x7416f0` seeds a new one at status 1 and percent 255).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Row {
    status: u8,
    percent: u8,
    threat: i32,
}

impl Row {
    fn new() -> Self {
        Self {
            status: 1,
            percent: 0xff,
            threat: 0,
        }
    }
}

/// One unit's list.
#[derive(Clone, Debug, Default)]
struct List {
    highest: u64,
    /// The rows in the order they joined, as the reference's list walks them.
    rows: Vec<(u64, Row)>,
    /// We joined the list and no status of ours has been judged since: the reference's
    /// `m_clientFlags & 8` (`0x7416f0` sets it), which holds back the losing-aggro cue once.
    fresh_player: bool,
}

/// What a packet asks of the frame: unit signals in first-raised order, sounds, a tutorial.
#[derive(Debug, Default, PartialEq, Eq)]
struct Effects {
    signals: Vec<(u64, &'static str)>,
    sounds: Vec<u32>,
    tutorial: bool,
}

impl Effects {
    /// `Script_SendUniqueSignal`: one per unit and event a frame, at its first raise.
    fn signal(&mut self, guid: u64, event: &'static str) {
        if !self.signals.contains(&(guid, event)) {
            self.signals.push((guid, event));
        }
    }
}

const LIST_UPDATE: &str = "UNIT_THREAT_LIST_UPDATE";
const SITUATION_UPDATE: &str = "UNIT_THREAT_SITUATION_UPDATE";

impl List {
    fn index(&self, guid: u64) -> Option<usize> {
        self.rows.iter().position(|(g, _)| *g == guid)
    }

    /// `0x7416f0`: the row for `guid`, made at status 1 if new. Our own new row marks the list
    /// fresh and is the tutorial's candidate (`0x52b370`).
    fn entry(&mut self, guid: u64, me: Option<u64>, fx: &mut Effects) -> usize {
        if let Some(i) = self.index(guid) {
            return i;
        }
        self.rows.push((guid, Row::new()));
        if Some(guid) == me {
            self.fresh_player = true;
            fx.tutorial = true;
        }
        self.rows.len() - 1
    }

    /// `0x7417a0`: the highest variant sets the top first; each row's threat is written; then
    /// every row but the top is judged against the top's threat, all of them when the top is in
    /// this packet and only this packet's rows otherwise, and the top's status after a highest
    /// update or any change.
    fn update(
        &mut self,
        unit: u64,
        highest: Option<u64>,
        entries: &[ThreatEntry],
        me: Option<u64>,
        fx: &mut Effects,
    ) {
        let mut top_in_packet = highest.is_some();
        if let Some(h) = highest {
            self.highest = h;
            if h != 0 {
                self.entry(h, me, fx);
            }
        }
        let mut touched = Vec::with_capacity(entries.len());
        for e in entries {
            let i = self.entry(e.target, me, fx);
            // The wire's `u32` lands in the row's signed field, as `0x741814` stores it.
            self.rows[i].1.threat = e.threat as i32;
            if e.target == self.highest {
                top_in_packet = true;
            }
            touched.push(i);
        }
        let mut changed = false;
        if let Some(top) = self.index(self.highest).filter(|_| self.highest != 0) {
            let top_threat = self.rows[top].1.threat;
            let judged: Vec<usize> = if top_in_packet {
                (0..self.rows.len()).collect()
            } else {
                touched
            };
            for i in judged {
                if i != top && self.judge(i, top_threat, me, fx) {
                    changed = true;
                }
            }
        }
        if highest.is_some() || changed {
            self.judge_top(me, fx);
        }
        fx.signal(unit, LIST_UPDATE);
    }

    /// `0x71c3b0`: one row against the top's threat; true when its status moved.
    fn judge(&mut self, i: usize, top_threat: i32, me: Option<u64>, fx: &mut Effects) -> bool {
        let (guid, row) = &mut self.rows[i];
        let ours = Some(*guid) == me;
        let percent = if top_threat == 0 {
            100
        } else if row.threat <= 0 {
            0
        } else {
            let scaled = 100 * i64::from(row.threat) / i64::from(top_threat);
            // `static_cast<uint8_t>`: a negative top wraps as the reference's does.
            if scaled >= 250 {
                250
            } else {
                scaled as u8
            }
        };
        if percent == row.percent {
            return false;
        }
        row.percent = percent;
        let status = if percent < 100 { 1 } else { 2 };
        let mut changed = false;
        if status != row.status {
            if ours && status == 2 && row.status == 1 {
                fx.sounds.push(SOUND_PULLED_AGGRO);
            }
            row.status = status;
            fx.signal(*guid, SITUATION_UPDATE);
            changed = true;
        }
        if ours {
            self.fresh_player = false;
        }
        changed
    }

    /// `0x737620`: the top reads 4, or 3 once another row stands at 2.
    fn judge_top(&mut self, me: Option<u64>, fx: &mut Effects) {
        let Some(top) = self.index(self.highest) else {
            return;
        };
        let status = if self
            .rows
            .iter()
            .enumerate()
            .any(|(i, (_, r))| i != top && r.status == 2)
        {
            3
        } else {
            4
        };
        let ours = Some(self.highest) == me;
        if status != self.rows[top].1.status {
            // The losing-aggro cue sits inside the floating text's gate, held while fresh.
            if ours && !self.fresh_player {
                fx.sounds.push(SOUND_LOSING_AGGRO);
            }
            self.rows[top].1.status = status;
            fx.signal(self.highest, SITUATION_UPDATE);
        }
        if ours {
            self.fresh_player = false;
        }
    }

    /// `0x737750`: the row goes; the top's status is judged again when a row at 2 or more left,
    /// and losing the top clears it instead.
    fn remove(&mut self, unit: u64, guid: u64, me: Option<u64>, fx: &mut Effects) {
        let Some(i) = self.index(guid) else {
            return;
        };
        let mut rejudge = false;
        if guid == self.highest {
            self.highest = 0;
        } else if self.rows[i].1.status > 1 {
            rejudge = true;
        }
        self.rows.remove(i);
        fx.signal(guid, SITUATION_UPDATE);
        if rejudge {
            self.judge_top(me, fx);
        }
        fx.signal(unit, LIST_UPDATE);
    }

    /// `0x7345c0`: every row goes, each signalled; an empty list signals nothing.
    fn clear(&mut self, unit: u64, fx: &mut Effects) {
        if self.rows.is_empty() {
            return;
        }
        for (guid, _) in self.rows.drain(..) {
            fx.signal(guid, SITUATION_UPDATE);
        }
        self.highest = 0;
        fx.signal(unit, LIST_UPDATE);
    }
}

/// Every held unit's list, and what this frame's packets asked for.
#[derive(Resource, Default)]
pub(crate) struct ThreatLists {
    lists: HashMap<u64, List>,
    effects: Effects,
}

/// `threatPlaySounds`, the aggro sounds' gate.
#[derive(Resource)]
struct ThreatConfig {
    play_sounds: bool,
}

impl Default for ThreatConfig {
    fn default() -> Self {
        Self { play_sounds: true }
    }
}

fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut cfg: ResMut<ThreatConfig>) {
    if ev.key() == "threatplaysounds" {
        cfg.play_sounds = ev.flag();
    }
}

/// The handlers act only on a unit the object manager holds (`0x741c90`, `0x737b20` and
/// `0x734b00` resolve it first).
fn on_update(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    me: Res<SelfGuid>,
    mut threat: ResMut<ThreatLists>,
) {
    let SessionEvent::ThreatUpdate(u) = ev else {
        return;
    };
    if !index.0.contains_key(&u.unit) {
        return;
    }
    let ThreatLists { lists, effects } = &mut *threat;
    lists
        .entry(u.unit)
        .or_default()
        .update(u.unit, u.highest, &u.entries, me.0, effects);
}

fn on_remove(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    me: Res<SelfGuid>,
    mut threat: ResMut<ThreatLists>,
) {
    let SessionEvent::ThreatRemove { unit, target } = ev else {
        return;
    };
    if !index.0.contains_key(&unit) {
        return;
    }
    let ThreatLists { lists, effects } = &mut *threat;
    if let Some(list) = lists.get_mut(&unit) {
        list.remove(unit, target, me.0, effects);
    }
}

fn on_clear(In(ev): In<SessionEvent>, index: Res<GuidIndex>, mut threat: ResMut<ThreatLists>) {
    let SessionEvent::ThreatClear { unit } = ev else {
        return;
    };
    if !index.0.contains_key(&unit) {
        return;
    }
    let ThreatLists { lists, effects } = &mut *threat;
    if let Some(list) = lists.get_mut(&unit) {
        list.clear(unit, effects);
    }
}

fn on_session_end(In(_): In<SessionEvent>, mut threat: ResMut<ThreatLists>) {
    *threat = ThreatLists::default();
}

/// Each frame: drop the lists of units no longer held, push the lists to the VM with each row's
/// melee test (`0x7374c0`'s range scaling), then raise the frame's signals, sounds and tutorial.
#[allow(clippy::too_many_arguments)]
fn feed_threat(
    script: Option<NonSendMut<UiScript>>,
    mut threat: ResMut<ThreatLists>,
    index: Res<GuidIndex>,
    units: Query<(&ObjectStore, &Transform)>,
    self_store: Query<&ObjectStore, With<crate::net::SelfPlayer>>,
    cfg: Res<ThreatConfig>,
    mut tutorials: ResMut<crate::tutorial::Tutorials>,
    mut sounds: MessageWriter<crate::sound::UiKitSound>,
) {
    // A unit that left the world takes its list with it.
    threat.lists.retain(|guid, _| index.0.contains_key(guid));
    let Some(mut script) = script else {
        threat.effects = Effects::default();
        return;
    };
    let unit = |guid: u64| index.0.get(&guid).and_then(|&e| units.get(e).ok());
    let lists: HashMap<u64, MobThreat> = threat
        .lists
        .iter()
        .filter(|(_, l)| !l.rows.is_empty() || l.highest != 0)
        .map(|(&mob, list)| {
            let mob_unit = unit(mob);
            let rows = list
                .rows
                .iter()
                .map(|&(guid, r)| {
                    let in_melee = mob_unit
                        .zip(unit(guid))
                        .is_some_and(|((ms, mt), (us, ut))| {
                            let reach = crate::target::melee_reach(ms, us);
                            mt.translation.distance_squared(ut.translation) <= reach * reach
                        });
                    (
                        guid,
                        ThreatRow {
                            status: r.status,
                            percent: r.percent,
                            threat: r.threat,
                            in_melee,
                        },
                    )
                })
                .collect();
            (
                mob,
                MobThreat {
                    highest: list.highest,
                    rows,
                },
            )
        })
        .collect();
    script.set_threat(lists);
    let fx = std::mem::take(&mut threat.effects);
    for (guid, event) in fx.signals {
        script.fire_unit_event(guid, event, Vec::new());
    }
    if cfg.play_sounds {
        for kit in fx.sounds {
            sounds.write(crate::sound::UiKitSound(kit));
        }
    }
    let level = self_store
        .iter()
        .next()
        .and_then(|s| s.0.unit_level())
        .unwrap_or(0);
    if fx.tutorial && level >= TUTORIAL_THREAT_LEVEL && script.threat_warning_enabled() {
        tutorials.trigger(TUTORIAL_THREAT, 0, Instant::now());
    }
}

pub(crate) struct ThreatPlugin;

impl Plugin for ThreatPlugin {
    fn build(&self, app: &mut App) {
        use SessionEventKind as K;
        app.init_resource::<ThreatLists>()
            .init_resource::<ThreatConfig>()
            .add_observer(on_cvar)
            .net_handler(K::ThreatUpdate, on_update)
            .net_handler(K::ThreatRemove, on_remove)
            .net_handler(K::ThreatClear, on_clear)
            .net_handler(K::Disconnected, on_session_end)
            .add_systems(Update, feed_threat.in_set(UiFeed));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOB: u64 = 0xF130_0000_0000_00AB;
    const ME: u64 = 0x10;
    const P1: u64 = 0x20;
    const P2: u64 = 0x30;

    fn rows(rows: &[(u64, u32)]) -> Vec<ThreatEntry> {
        rows.iter()
            .map(|&(target, threat)| ThreatEntry { target, threat })
            .collect()
    }

    fn row(list: &List, guid: u64) -> Row {
        list.rows[list.index(guid).unwrap()].1
    }

    /// A highest update: the top reads 4, the rest by share, our new row marked fresh.
    #[test]
    fn a_highest_update_judges_every_row_against_the_top() {
        let (mut list, mut fx) = (List::default(), Effects::default());
        list.update(
            MOB,
            Some(P1),
            &rows(&[(P1, 1000), (ME, 400), (P2, 1000)]),
            Some(ME),
            &mut fx,
        );
        assert_eq!(
            row(&list, ME),
            Row {
                status: 1,
                percent: 40,
                threat: 400
            }
        );
        assert_eq!(row(&list, P2).status, 2, "equal to the top: 100 %");
        assert_eq!(row(&list, P1).status, 3, "another row at 2: insecure");
        assert_eq!(
            row(&list, P1).percent,
            0xff,
            "the top's percent is never judged"
        );
        assert!(fx.tutorial, "we joined the list");
        assert!(!list.fresh_player, "judging our row spent the fresh mark");
        assert_eq!(
            fx.signals,
            [
                (P2, SITUATION_UPDATE),
                (P1, SITUATION_UPDATE),
                (MOB, LIST_UPDATE)
            ],
            "our row stays at 1, so it raises nothing"
        );
        assert!(fx.sounds.is_empty());
    }

    /// Passing the top while not tanking plays the pulled-aggro cue, once.
    #[test]
    fn passing_the_top_plays_the_pulled_aggro_sound() {
        let (mut list, mut fx) = (List::default(), Effects::default());
        list.update(
            MOB,
            Some(P1),
            &rows(&[(P1, 1000), (ME, 900)]),
            Some(ME),
            &mut fx,
        );
        let mut fx = Effects::default();
        list.update(MOB, None, &rows(&[(ME, 1100)]), Some(ME), &mut fx);
        assert_eq!(
            row(&list, ME),
            Row {
                status: 2,
                percent: 110,
                threat: 1100
            }
        );
        assert_eq!(fx.sounds, [SOUND_PULLED_AGGRO]);
        assert_eq!(
            row(&list, P1).status,
            3,
            "the changed row re-judges the top"
        );
    }

    /// As the top, a challenger reaching 100 % costs us the secure hold: the losing cue, but
    /// not while our first status is unjudged.
    #[test]
    fn losing_the_secure_hold_plays_the_losing_aggro_sound_after_the_first_judgement() {
        let (mut list, mut fx) = (List::default(), Effects::default());
        list.update(MOB, Some(ME), &rows(&[(ME, 1000)]), Some(ME), &mut fx);
        assert_eq!(row(&list, ME).status, 4);
        assert!(
            fx.sounds.is_empty(),
            "fresh: the first judgement of our top row is silent"
        );
        let mut fx = Effects::default();
        list.update(MOB, None, &rows(&[(P1, 1000)]), Some(ME), &mut fx);
        assert_eq!(row(&list, ME).status, 3);
        assert_eq!(fx.sounds, [SOUND_LOSING_AGGRO]);
    }

    /// A plain update with the top absent judges only its own rows.
    #[test]
    fn a_plain_update_without_the_top_judges_only_its_rows() {
        let (mut list, mut fx) = (List::default(), Effects::default());
        list.update(
            MOB,
            Some(P1),
            &rows(&[(P1, 1000), (ME, 500), (P2, 500)]),
            Some(ME),
            &mut fx,
        );
        // P1's threat moves without a packet saying so: only a row in a packet is re-judged.
        list.rows[list.index(P1).unwrap()].1.threat = 2000;
        let mut fx = Effects::default();
        list.update(MOB, None, &rows(&[(ME, 600)]), Some(ME), &mut fx);
        assert_eq!(row(&list, ME).percent, 30, "600 of 2000");
        assert_eq!(row(&list, P2).percent, 50, "untouched: still 500 of 1000");
    }

    /// The share caps at 250 %, and a top of 0 threat makes every share 100 %.
    #[test]
    fn the_share_caps_at_250_and_a_zero_top_reads_100() {
        let (mut list, mut fx) = (List::default(), Effects::default());
        list.update(
            MOB,
            Some(P1),
            &rows(&[(P1, 100), (ME, 1000)]),
            Some(ME),
            &mut fx,
        );
        assert_eq!(row(&list, ME).percent, 250);
        let (mut list, mut fx) = (List::default(), Effects::default());
        list.update(MOB, Some(P1), &rows(&[(P1, 0), (ME, 0)]), Some(ME), &mut fx);
        assert_eq!(row(&list, ME).percent, 100);
    }

    /// `0x737750`: removing the top clears it; removing a row at 2 re-judges the top.
    #[test]
    fn removals_clear_the_top_or_rejudge_it() {
        let (mut list, mut fx) = (List::default(), Effects::default());
        list.update(
            MOB,
            Some(ME),
            &rows(&[(ME, 1000), (P1, 1000)]),
            Some(ME),
            &mut fx,
        );
        assert_eq!(row(&list, ME).status, 3);
        let mut fx = Effects::default();
        list.remove(MOB, P1, Some(ME), &mut fx);
        assert_eq!(
            row(&list, ME).status,
            4,
            "the challenger left: secure again"
        );
        assert_eq!(
            fx.signals,
            [
                (P1, SITUATION_UPDATE),
                (ME, SITUATION_UPDATE),
                (MOB, LIST_UPDATE)
            ]
        );
        let mut fx = Effects::default();
        list.remove(MOB, ME, Some(ME), &mut fx);
        assert_eq!(list.highest, 0);
        assert!(list.rows.is_empty());
        let mut fx = Effects::default();
        list.remove(MOB, P2, Some(ME), &mut fx);
        assert!(fx.signals.is_empty(), "no row: nothing");
    }

    /// `0x7345c0`: every row signalled once; an empty list signals nothing.
    #[test]
    fn a_clear_signals_every_row_then_the_unit() {
        let (mut list, mut fx) = (List::default(), Effects::default());
        list.update(
            MOB,
            Some(ME),
            &rows(&[(ME, 10), (P1, 5)]),
            Some(ME),
            &mut fx,
        );
        let mut fx = Effects::default();
        list.clear(MOB, &mut fx);
        assert_eq!(
            fx.signals,
            [
                (ME, SITUATION_UPDATE),
                (P1, SITUATION_UPDATE),
                (MOB, LIST_UPDATE)
            ]
        );
        assert_eq!(list.highest, 0);
        let mut fx = Effects::default();
        list.clear(MOB, &mut fx);
        assert!(fx.signals.is_empty());
    }

    /// The whole path: a held unit's packets reach the VM, a unit not held is ignored, and the
    /// verbs answer from the pushed lists.
    #[test]
    fn packets_reach_the_threat_verbs_for_held_units_only() {
        use benilla_protocol::messages::ThreatUpdate;
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        app.add_message::<crate::sound::UiKitSound>()
            .init_resource::<ThreatLists>()
            .init_resource::<ThreatConfig>()
            .init_resource::<GuidIndex>()
            .init_resource::<crate::tutorial::Tutorials>()
            .insert_resource(SelfGuid(Some(ME)));
        let world = app.world_mut();
        let mob = world
            .spawn((ObjectStore::default(), Transform::default()))
            .id();
        let me = world
            .spawn((
                ObjectStore::default(),
                Transform::from_xyz(3.0, 0.0, 0.0),
                crate::net::SelfPlayer,
            ))
            .id();
        world
            .resource_mut::<GuidIndex>()
            .0
            .extend([(MOB, mob), (ME, me)]);
        let mut script = UiScript::new().unwrap();
        script.set_unit_guids(&benilla_ui::script::UnitGuids {
            player: ME,
            target: MOB,
            held: HashMap::from([(ME, 0), (MOB, 0)]),
            ..Default::default()
        });
        script
            .run(
                "SEEN = {}\n\
                 local f = CreateFrame(\"Frame\")\n\
                 f:RegisterEvent(\"UNIT_THREAT_LIST_UPDATE\")\n\
                 f:RegisterEvent(\"UNIT_THREAT_SITUATION_UPDATE\")\n\
                 f:SetScript(\"OnEvent\", function(self, event, unit) tinsert(SEEN, event .. \":\" .. unit) end)",
            )
            .unwrap();
        world.insert_non_send_resource(script);

        for unit in [MOB, 0xF130_0000_0000_0FFF] {
            world
                .run_system_once_with(
                    on_update,
                    SessionEvent::ThreatUpdate(ThreatUpdate {
                        unit,
                        highest: Some(ME),
                        entries: rows(&[(ME, 900)]),
                    }),
                )
                .unwrap();
        }
        world.run_system_once(feed_threat).unwrap();
        assert_eq!(
            world.resource::<ThreatLists>().lists.len(),
            1,
            "the unheld unit is ignored"
        );
        let mut script = world.non_send_resource_mut::<UiScript>();
        assert_eq!(
            script
                .eval::<f64>(r#"return UnitThreatSituation("player", "target")"#)
                .unwrap(),
            3.0,
            "tanking securely"
        );
        assert_eq!(
            script
                .eval::<f64>(r#"return select(3, UnitDetailedThreatSituation("player", "target"))"#)
                .unwrap(),
            100.0
        );
        let seen = script
            .eval::<String>("return table.concat(SEEN, ',')")
            .unwrap();
        assert_eq!(
            seen,
            "UNIT_THREAT_SITUATION_UPDATE:player,UNIT_THREAT_LIST_UPDATE:target"
        );
    }
}
