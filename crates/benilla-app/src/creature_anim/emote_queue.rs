//! A unit's emote queue, which the questgiver panels fill from their packets' emote pairs: the
//! setter keeps up to four pairs whose `Emotes.dbc` row is a one-shot (`EmoteSpecProc` 0) and
//! arms the first at its delay (`0x71a260`); the per-frame pass plays the due head's `AnimID` and
//! arms the next at its own delay past the played sequence's length plus 500 ms (`0x73adc0`).

use std::time::{Duration, Instant};

use benilla_assets::ModelAnimations;
use bevy::prelude::*;

use crate::net::{GuidIndex, NetEntity};
use crate::sound::EmoteSounds;

use super::{AnimData, EmoteAnim, PlaySeq};

/// The queue's slot count (`[unit+0x9fc]`, four `{delay, emote}` pairs).
const SLOTS: usize = 4;

/// The gap the pass adds after a played sequence before the next pair's delay (`0x73aec7`).
const SEQUENCE_GAP_MS: u64 = 500;

/// The setter's ask: `(delay ms, Emotes.dbc id)` pairs for the unit `guid`, in wire order.
#[derive(Message, Clone, Debug, PartialEq, Eq)]
pub(crate) struct QueueNpcEmotes {
    pub(crate) guid: u64,
    pub(crate) emotes: Vec<(u32, u32)>,
}

/// One unit's queue: the kept emote ids in order, the head due at `due`, and the later pairs'
/// raw delays.
#[derive(Component, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct EmoteQueue {
    /// `(delay ms, emote)`, the head first; empty is the idle queue.
    pending: Vec<(u32, u32)>,
    due: Option<Instant>,
}

impl EmoteQueue {
    /// The setter: a pair is kept when its emote is non-zero and its `Emotes.dbc` row is a
    /// one-shot (`row+0x10 == 0`, `0x71a2a2`-`0x71a2cd`), into the four slots; the head is due its
    /// delay from now.
    fn set(&mut self, emotes: &[(u32, u32)], spec_proc: impl Fn(u32) -> Option<u32>, now: Instant) {
        self.pending = emotes
            .iter()
            .copied()
            .filter(|&(_, emote)| emote != 0 && spec_proc(emote) == Some(0))
            .take(SLOTS)
            .collect();
        self.due = self
            .pending
            .first()
            .map(|&(delay, _)| now + Duration::from_millis(u64::from(delay)));
    }

    /// The pass: once the head is due, its emote is returned to play and the next pair is armed at
    /// its delay past `now`, the played sequence's `length_ms` and the 500 ms gap.
    fn tick(&mut self, now: Instant, length_ms: impl FnOnce(u32) -> u64) -> Option<u32> {
        let due = self.due?;
        if now < due {
            return None;
        }
        let (_, emote) = self.pending.remove(0);
        self.due = self.pending.first().map(|&(delay, _)| {
            now + Duration::from_millis(u64::from(delay) + length_ms(emote) + SEQUENCE_GAP_MS)
        });
        Some(emote)
    }
}

/// Each [`QueueNpcEmotes`] onto its unit, replacing what was queued; a guid not streamed as a unit
/// or a player (the readers' type-mask 8 lookup) is dropped.
pub(super) fn set_emote_queues(
    mut asks: MessageReader<QueueNpcEmotes>,
    mut commands: Commands,
    index: Res<GuidIndex>,
    kinds: Query<&NetEntity>,
    mut queues: Query<&mut EmoteQueue>,
    emotes: Option<Res<EmoteSounds>>,
) {
    let Some(emotes) = emotes else {
        asks.clear();
        return;
    };
    let now = Instant::now();
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
        let spec_proc = |id| emotes.0.spec_proc(id);
        if let Ok(mut queue) = queues.get_mut(entity) {
            queue.set(&ask.emotes, spec_proc, now);
        } else {
            let mut queue = EmoteQueue::default();
            queue.set(&ask.emotes, spec_proc, now);
            commands.entity(entity).try_insert(queue);
        }
    }
}

/// The per-frame pass over every queued unit: the due head's `AnimID` plays as a one-shot (the
/// pass calls the anim player directly, `0x73ae52`), and the next waits on that sequence's length.
/// Deviation: the setter's weapon-sheath hand-off (`0x71a30c`-`0x71a31c`) is not modelled.
pub(super) fn drive_emote_queues(
    mut queues: Query<(Entity, &mut EmoteQueue, Option<&ModelAnimations>)>,
    mut out: MessageWriter<EmoteAnim>,
    mut play_seq: ResMut<PlaySeq>,
    emotes: Option<Res<EmoteSounds>>,
    anim_data: Option<Res<AnimData>>,
) {
    let Some(emotes) = emotes else { return };
    let now = Instant::now();
    let catalog = anim_data.as_deref().map(|d| &d.0);
    for (entity, mut queue, anims) in &mut queues {
        if queue.due.is_none_or(|due| now < due) {
            continue;
        }
        let anim_of = |emote| emotes.0.state_anim(emote).unwrap_or(0);
        let Some(emote) = queue.tick(now, |emote| {
            anims
                .and_then(|a| super::find_resolved(a, anim_of(emote) as u16, catalog))
                .map_or(0, |c| (c.duration * 1000.0) as u64)
        }) else {
            continue;
        };
        out.write(EmoteAnim {
            entity,
            anim_id: anim_of(emote) as u16,
            seq: play_seq.next(),
            via_player: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Emote 10 and 11 are one-shots, 12 a looping state emote, 13 has no row.
    fn spec_proc(id: u32) -> Option<u32> {
        match id {
            10 | 11 => Some(0),
            12 => Some(2),
            _ => None,
        }
    }

    #[test]
    fn the_setter_keeps_one_shots_only_and_four_at_most() {
        let t0 = Instant::now();
        let mut q = EmoteQueue::default();
        q.set(&[(0, 12), (5, 10), (0, 0), (0, 13), (7, 11)], spec_proc, t0);
        assert_eq!(q.pending, [(5, 10), (7, 11)], "the kept pairs, compacted");
        assert_eq!(q.due, Some(t0 + Duration::from_millis(5)));
        q.set(&[(0, 10); 6], spec_proc, t0);
        assert_eq!(q.pending.len(), 4, "four slots");
        q.set(&[(0, 12)], spec_proc, t0);
        assert!(
            q.pending.is_empty() && q.due.is_none(),
            "nothing kept: idle"
        );
    }

    #[test]
    fn each_pair_waits_its_delay_past_the_last_sequence_and_the_gap() {
        let t0 = Instant::now();
        let ms = |n| Duration::from_millis(n);
        let mut q = EmoteQueue::default();
        q.set(&[(100, 10), (200, 11)], spec_proc, t0);
        assert_eq!(q.tick(t0 + ms(99), |_| 1000), None, "the head's delay");
        assert_eq!(q.tick(t0 + ms(100), |_| 1000), Some(10));
        // 200 delay + 1000 sequence + 500 gap past the play.
        assert_eq!(q.tick(t0 + ms(1799), |_| 0), None);
        assert_eq!(q.tick(t0 + ms(1800), |_| 0), Some(11));
        assert_eq!(q.tick(t0 + ms(10_000), |_| 0), None, "drained");
    }

    #[test]
    fn a_new_queue_replaces_the_old() {
        let t0 = Instant::now();
        let mut q = EmoteQueue::default();
        q.set(&[(0, 10), (0, 11)], spec_proc, t0);
        q.set(&[(0, 11)], spec_proc, t0);
        assert_eq!(q.tick(t0, |_| 0), Some(11));
        assert_eq!(q.tick(t0 + Duration::from_secs(5), |_| 0), None);
    }
}
