//! `--questtimer`: the timed-quest countdown. The server writes the deadline into the quest-log
//! slot's third field as an absolute unix time, `time(nullptr) + limitTime` (vmangos
//! `Player::AddQuest`), and the countdown is that minus the server clock from `CMSG_QUERY_TIME`.
//! The probe adds the quest itself, so the remaining time must sit just under the full limit.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use benilla_protocol::events::decode;
use benilla_protocol::SessionEvent;

use crate::probes::{Ctx, Probe, FIELD_PLAYER_QUEST_LOG_1_1, QUEST_LOG_SLOT_FIELDS};

/// "Iverron's Antidote" (Teldrassil): `LimitTime` 300, min level 2, and no prerequisite that a
/// GM `.quest add` checks.
const TIMED_QUEST: u32 = 3522;
/// [`TIMED_QUEST`]'s `quest_template.LimitTime`. No 1.12 packet carries it, not even
/// `SMSG_QUEST_QUERY_RESPONSE`; vmangos reads it only in `Player::AddQuest`.
const LIMIT_TIME_SECS: u32 = 300;
/// Seconds allowed between the `.quest add` and the read, generous for a slow login.
const ADD_TO_READ_SLACK: f64 = 120.0;

#[derive(Default)]
pub(crate) struct QuestTimer {
    /// The server's wall clock, and when we received it (`SMSG_QUERY_TIME_RESPONSE`).
    clock: Option<(u32, Instant)>,
}

impl Probe for QuestTimer {
    fn stage(&mut self, cx: &mut Ctx) -> Result<()> {
        cx.session
            .send_chat(&format!(".quest remove {TIMED_QUEST}"))?;
        cx.session.send_chat(&format!(".quest add {TIMED_QUEST}"))?;
        println!("questtimer: GM .quest remove/add {TIMED_QUEST}");
        Ok(())
    }

    fn on_event(&mut self, ev: &SessionEvent, _cx: &mut Ctx) -> Result<()> {
        if let SessionEvent::ServerUnixTime { unix_time, .. } = ev {
            self.clock = Some((*unix_time, Instant::now()));
        }
        Ok(())
    }

    fn verify(&mut self, cx: &mut Ctx) -> Result<()> {
        let Ctx { session, world } = cx;
        let self_guid = world.self_guid;

        // 1) The slot, polled: its field update can trail the GM command's chat ack.
        let find_slot = |sf: &Option<benilla_protocol::messages::ObjectFields>| {
            sf.as_ref().and_then(|sf| {
                (0..benilla_protocol::messages::PLAYER_QUEST_LOG_SLOTS)
                    .find(|&i| sf.player_quest_log(i).map(|s| s.quest_id) == Some(TIMED_QUEST))
            })
        };
        let mut slot = find_slot(&world.self_fields);
        for _ in 0..6 {
            if slot.is_some() {
                break;
            }
            let drain_until = Instant::now() + Duration::from_secs(1);
            while Instant::now() < drain_until {
                let Ok(msg) = session.recv() else { continue };
                for ev in decode(msg) {
                    match ev {
                        SessionEvent::ObjectValues { guid: g, fields } if g == self_guid => {
                            if let Some(sf) = &mut world.self_fields {
                                sf.merge(fields);
                            }
                        }
                        SessionEvent::ServerUnixTime { unix_time, .. } => {
                            self.clock = Some((unix_time, Instant::now()));
                        }
                        _ => {}
                    }
                }
            }
            slot = find_slot(&world.self_fields);
        }
        let slot = slot.context(
            "--questtimer: the timed quest never landed in a PLAYER_QUEST_LOG slot within the \
             poll window (is `.quest add` refused for this character?)",
        )?;

        // Raw timer field (id field + 2) beside the decoded one, so a wrong-field decode shows.
        let raw_timer = world
            .self_fields
            .as_ref()
            .and_then(|sf| {
                sf.raw_fields()
                    .find(|&(idx, _)| {
                        idx == FIELD_PLAYER_QUEST_LOG_1_1
                            + QUEST_LOG_SLOT_FIELDS * u16::from(slot)
                            + 4
                    })
                    .map(|(_, v)| v)
            })
            .unwrap_or(0);
        let decoded = world
            .self_fields
            .as_ref()
            .and_then(|sf| sf.player_quest_log(slot))
            .context("--questtimer: the slot vanished between the find and the read")?;
        println!(
            "quest {TIMED_QUEST} occupies slot {slot}; timer field raw {raw_timer} \
             (decoded {}), state byte {:#04x}",
            decoded.timer, decoded.state
        );
        if raw_timer == 0 || decoded.timer != raw_timer {
            bail!(
                "--questtimer: timer field raw {raw_timer}, decoded {} — a timed quest must carry \
                 a nonzero absolute deadline and the decode must agree with the raw word",
                decoded.timer
            );
        }

        // 2) The server's wall clock, asked for here so the round trip itself is under test.
        session.query_time()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let asked_at = Instant::now();
        while Instant::now() < deadline {
            let Ok(msg) = session.recv() else { continue };
            for ev in decode(msg) {
                if let SessionEvent::ServerUnixTime { unix_time, .. } = ev {
                    self.clock = Some((unix_time, Instant::now()));
                }
            }
            if self.clock.is_some_and(|(_, at)| at > asked_at) {
                break;
            }
        }
        let (base, at) = self
            .clock
            .context("--questtimer: no SMSG_QUERY_TIME_RESPONSE within 5s")?;
        let now = f64::from(base) + at.elapsed().as_secs_f64();
        println!(
            "server wall clock {base} (+{:.1}s since the answer)",
            at.elapsed().as_secs_f64()
        );

        // 3) A wrong epoch misses this window by decades, a seconds/milliseconds slip by a factor
        // of 1000, an absent or stale clock by more than the run's length.
        let remaining = f64::from(raw_timer) - now;
        let elapsed_since_add = f64::from(LIMIT_TIME_SECS) - remaining;
        if !(0.0..=ADD_TO_READ_SLACK).contains(&elapsed_since_add) {
            bail!(
                "--questtimer: {remaining:.1}s remaining of a {LIMIT_TIME_SECS}s limit implies \
                 {elapsed_since_add:.1}s between the `.quest add` and this read — expected \
                 0..{ADD_TO_READ_SLACK}s. The deadline and the clock disagree: wrong field, wrong \
                 epoch, wrong unit, or no clock at all."
            );
        }
        println!(
            "✅ countdown: {remaining:.1}s remaining of {LIMIT_TIME_SECS}s — deadline {raw_timer} \
             minus server now {now:.0}, i.e. {elapsed_since_add:.1}s since the `.quest add`."
        );
        Ok(())
    }
}
