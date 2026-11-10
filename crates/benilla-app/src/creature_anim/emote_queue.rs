//! A unit's emote queue, fed by the quest panels and the gossip text (`0x71a260`, `0x73adc0`).

use benilla_assets::ModelAnimations;
use bevy::prelude::*;

use crate::net::{GuidIndex, NetEntity};
use crate::sound::EmoteSounds;

use super::{AnimData, AnimDriver, EmoteAnim, PlaySeq};

/// The queue's slot count (`[unit+0x9fc]`, four `{delay, emote}` pairs).
const SLOTS: usize = 4;

/// The gap the pass adds after a played sequence before the next pair's delay (`0x73aec7`).
const SEQUENCE_GAP_MS: u32 = 500;

/// Slot 0's emote while the queue is idle (`[unit+0xa00] = -1`).
const IDLE: u32 = u32::MAX;

/// The setter's ask: `(delay ms, Emotes.dbc id)` pairs for the unit `guid`, in wire order.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub(crate) struct QueueNpcEmotes {
    pub(crate) guid: u64,
    pub(crate) emotes: Vec<(u32, u32)>,
}

/// One unit's `(delay, emote)` slots; an armed slot's delay is a clock time in ms.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub(crate) struct EmoteQueue {
    slots: [(u32, u32); SLOTS],
}

impl Default for EmoteQueue {
    fn default() -> Self {
        let mut slots = [(0, 0); SLOTS];
        slots[0].1 = IDLE;
        Self { slots }
    }
}

/// One pass: the emote it plays, and whether it left the queue idle (`0x73adf5`, `0x73aed7`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Pass {
    emote: Option<u32>,
    drained: bool,
}

impl EmoteQueue {
    /// The setter over `row` (`EmoteSpecProc`, `AnimID`), `true` when pair i's slot i has an anim.
    fn set(
        &mut self,
        pairs: &[(u32, u32)],
        row: impl Fn(u32) -> Option<(u32, u32)>,
        now_ms: u32,
    ) -> bool {
        self.slots[0].1 = IDLE;
        if pairs.is_empty() {
            return false;
        }
        let mut kept = 0;
        let mut hand_off = false;
        for (i, &(delay, emote)) in pairs.iter().enumerate() {
            if kept < SLOTS && emote != 0 && row(emote).is_some_and(|(spec, _)| spec == 0) {
                self.slots[kept] = (delay, emote);
                kept += 1;
            }
            let slot = self.slots.get(i).and_then(|&(_, id)| row(id));
            hand_off |= slot.is_some_and(|(_, anim)| anim != 0);
        }
        if kept < SLOTS {
            self.slots[kept].1 = 0;
        }
        self.slots[0].0 = self.slots[0].0.wrapping_add(now_ms);
        hand_off
    }

    fn is_idle(&self) -> bool {
        self.slots[0].1 == IDLE
    }

    /// The pass: the first live slot once due; the next waits `length_ms` and the gap past it.
    fn tick(&mut self, now_ms: u32, length_ms: impl FnOnce(u32) -> u32) -> Pass {
        if self.is_idle() {
            return Pass::default();
        }
        let Some(i) = self.slots.iter().position(|&(_, emote)| emote != 0) else {
            self.slots[0].1 = IDLE;
            return Pass {
                emote: None,
                drained: true,
            };
        };
        let (due, emote) = self.slots[i];
        if due > now_ms {
            return Pass::default();
        }
        self.slots[i].1 = 0;
        let drained = match self.slots.get_mut(i + 1) {
            Some(next) if next.1 != 0 => {
                let wait = length_ms(emote).saturating_add(SEQUENCE_GAP_MS);
                next.0 = next.0.wrapping_add(now_ms.wrapping_add(wait));
                false
            }
            _ => {
                self.slots[0].1 = IDLE;
                true
            }
        };
        Pass {
            emote: Some(emote),
            drained,
        }
    }
}

/// The async clock the setter and the pass share (`0x86ae20`).
fn clock_ms(time: &Time<Real>) -> u32 {
    time.elapsed().as_millis() as u32
}

/// Each [`QueueNpcEmotes`] onto its unit, replacing what was queued; a guid not streamed as a unit
/// or a player (the readers' type-mask 8 lookup) is dropped.
pub(super) fn set_emote_queues(
    mut asks: MessageReader<QueueNpcEmotes>,
    mut commands: Commands,
    index: Res<GuidIndex>,
    kinds: Query<&NetEntity>,
    mut queues: Query<&mut EmoteQueue>,
    mut drivers: Query<&mut AnimDriver>,
    emotes: Option<Res<EmoteSounds>>,
    time: Res<Time<Real>>,
) {
    let Some(emotes) = emotes else {
        asks.clear();
        return;
    };
    let now = clock_ms(&time);
    for ask in asks.read() {
        let Some(&entity) = index.0.get(&ask.guid) else {
            continue;
        };
        let unit = kinds.get(entity).is_ok_and(|n| {
            matches!(
                n.kind,
                benilla_protocol::EntityKind::Unit | benilla_protocol::EntityKind::Player
            )
        });
        if !unit {
            continue;
        }
        let row = |id| Some((emotes.0.spec_proc(id)?, emotes.0.state_anim(id)?));
        let hand_off = if let Ok(mut queue) = queues.get_mut(entity) {
            queue.set(&ask.emotes, row, now)
        } else {
            let mut queue = EmoteQueue::default();
            let hand_off = queue.set(&ask.emotes, row, now);
            commands.entity(entity).try_insert(queue);
            hand_off
        };
        if hand_off {
            if let Ok(mut drv) = drivers.get_mut(entity) {
                drv.hand_off_sheath();
            }
        }
    }
}

/// The per-frame pass: the due `AnimID` plays as a one-shot (`0x73ae52`); a drain arms the restore.
pub(super) fn drive_emote_queues(
    mut queues: Query<(
        Entity,
        &mut EmoteQueue,
        Option<&ModelAnimations>,
        Option<&mut AnimDriver>,
    )>,
    mut out: MessageWriter<EmoteAnim>,
    mut play_seq: ResMut<PlaySeq>,
    emotes: Option<Res<EmoteSounds>>,
    anim_data: Option<Res<AnimData>>,
    time: Res<Time<Real>>,
) {
    let Some(emotes) = emotes else { return };
    let now = clock_ms(&time);
    let catalog = anim_data.as_deref().map(|d| &d.0);
    for (entity, mut queue, anims, drv) in &mut queues {
        if queue.is_idle() {
            continue;
        }
        let anim_of = |emote| emotes.0.state_anim(emote).unwrap_or(0);
        let pass = queue.tick(now, |emote| {
            anims
                .and_then(|a| super::find_resolved(a, anim_of(emote) as u16, catalog))
                .map_or(0, |c| (c.duration * 1000.0) as u32)
        });
        if let Some(emote) = pass.emote {
            if benilla_assets::trace::enabled() {
                benilla_assets::trace::line(
                    "fct",
                    &format!("anim emote-queue unit={entity} emote={emote}"),
                );
            }
            out.write(EmoteAnim {
                entity,
                anim_id: anim_of(emote) as u16,
                seq: play_seq.next(),
                via_player: false,
                play: Default::default(),
            });
        }
        if pass.drained {
            if let Some(mut drv) = drv {
                drv.sheath_restore = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10 and 11 are one-shots, 12 a state emote, 13 has no row, 14 a one-shot with no anim.
    fn row(id: u32) -> Option<(u32, u32)> {
        match id {
            10 | 11 => Some((0, 60)),
            12 => Some((2, 60)),
            14 => Some((0, 0)),
            _ => None,
        }
    }

    #[test]
    fn the_setter_keeps_one_shots_only_and_four_at_most() {
        let mut q = EmoteQueue::default();
        q.set(&[(0, 12), (5, 10), (0, 0), (0, 13), (7, 11)], row, 1000);
        assert_eq!(q.slots[..3], [(1005, 10), (7, 11), (0, 0)], "compacted");
        q.set(&[(0, 10); 6], row, 0);
        assert_eq!(
            q.slots.iter().filter(|s| s.1 == 10).count(),
            4,
            "four slots"
        );
    }

    #[test]
    fn each_pair_waits_its_delay_past_the_last_sequence_and_the_gap() {
        let mut q = EmoteQueue::default();
        q.set(&[(100, 10), (200, 11)], row, 0);
        assert_eq!(q.tick(99, |_| 1000), Pass::default(), "the head's delay");
        let first = Pass {
            emote: Some(10),
            drained: false,
        };
        assert_eq!(q.tick(100, |_| 1000), first);
        // 200 delay + 1000 sequence + 500 gap past the play.
        assert_eq!(q.tick(1799, |_| 0), Pass::default());
        let last = Pass {
            emote: Some(11),
            drained: true,
        };
        assert_eq!(q.tick(1800, |_| 0), last);
        assert!(q.is_idle());
        assert_eq!(q.tick(10_000, |_| 0), Pass::default(), "idle");
    }

    #[test]
    fn a_new_queue_replaces_the_old() {
        let mut q = EmoteQueue::default();
        q.set(&[(0, 10), (0, 11)], row, 0);
        q.set(&[(0, 11)], row, 0);
        assert_eq!(q.tick(0, |_| 0).emote, Some(11));
        assert!(q.is_idle());
    }

    /// Pair i tests slot i after the keep, so a pair kept into an earlier slot does not count.
    #[test]
    fn the_hand_off_runs_for_an_animated_slot_at_the_pairs_own_index() {
        let hand_off = |pairs: &[(u32, u32)]| EmoteQueue::default().set(pairs, row, 0);
        assert!(hand_off(&[(0, 10)]), "a kept pair with an anim");
        assert!(!hand_off(&[(0, 14)]), "a kept pair with AnimID 0");
        assert!(!hand_off(&[(0, 12)]), "a dropped pair, slot 0 idle");
        assert!(
            !hand_off(&[(0, 12), (0, 10)]),
            "kept into slot 0, while pair 1 reads the empty slot 1"
        );
        assert!(!hand_off(&[]), "no pairs: the setter leaves at once");

        let mut q = EmoteQueue::default();
        q.set(&[(0, 10), (0, 11)], row, 0);
        assert!(
            q.set(&[(0, 12), (0, 10)], row, 0),
            "pair 1 reads slot 1's stale row"
        );
    }

    /// A pair with nothing kept still drains on the next pass (`0x73adf5`).
    #[test]
    fn a_queue_that_kept_nothing_drains_on_its_next_pass() {
        let mut q = EmoteQueue::default();
        q.set(&[(0, 12)], row, 0);
        let drained = Pass {
            emote: None,
            drained: true,
        };
        assert_eq!(q.tick(0, |_| 0), drained);
        assert_eq!(q.tick(0, |_| 0), Pass::default());

        q.set(&[], row, 0);
        assert_eq!(q.tick(0, |_| 0), Pass::default(), "no pairs, no drain");
    }

    /// The real setter system: a unit takes the pairs and the hand-off, a game object neither.
    #[test]
    fn an_ask_lands_on_the_units_queue_and_hands_off_the_sheath() {
        let mut app = App::new();
        app.add_message::<QueueNpcEmotes>()
            .init_resource::<GuidIndex>()
            .init_resource::<Time<Real>>()
            .insert_resource(EmoteSounds(
                benilla_formats::EmoteSoundCatalog::default()
                    .with_row(10, 60, 0)
                    .with_spec_proc(10, 0),
            ))
            .add_systems(Update, set_emote_queues);
        let mut driver = AnimDriver::default();
        driver.commit_sheath(1);
        driver.commit_sheath(0);
        driver.sheath_restore = true;
        let unit = app
            .world_mut()
            .spawn((
                NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                driver,
            ))
            .id();
        let chest = app
            .world_mut()
            .spawn(NetEntity {
                kind: benilla_protocol::EntityKind::GameObject,
                display_id: None,
                scale: 1.0,
            })
            .id();
        let mut index = app.world_mut().resource_mut::<GuidIndex>();
        index.0.insert(0x42, unit);
        index.0.insert(0x43, chest);

        for guid in [0x42, 0x43, 0x44] {
            app.world_mut().write_message(QueueNpcEmotes {
                guid,
                emotes: vec![(250, 10)],
            });
        }
        app.update();

        let world = app.world();
        let queue = world.entity(unit).get::<EmoteQueue>().expect("queued");
        assert_eq!(queue.slots[0].1, 10);
        let drv = world.entity(unit).get::<AnimDriver>().unwrap();
        assert!(!drv.sheath_restore, "the hand-off drops a pending restore");
        assert_eq!(drv.sheath_prev, 0, "and re-bases PREV on the current state");
        assert!(
            world.entity(chest).get::<EmoteQueue>().is_none(),
            "a game object takes no emotes"
        );
    }
}
