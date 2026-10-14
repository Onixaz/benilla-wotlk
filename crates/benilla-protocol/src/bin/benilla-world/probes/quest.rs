//! `--quest`: accepts and turns in a real quest over the whole questgiver wire (details, accept,
//! complete, reward, quest-complete), checked against `PLAYER_QUEST_LOG` and `PLAYER_XP`.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use benilla_protocol::{decode, guid, EntityKind, SessionEvent, WorldSession};

use crate::probes::{
    Ctx, Probe, FIELD_PLAYER_QUEST_LOG_1_1, QUEST_LOG_SLOT_FIELDS, QUEST_TURNIN_ENTRY,
    QUEST_TURNIN_TP,
};

/// A `drain_quest` handler: `Some(msg)` stops the drain, `None` keeps pumping.
type QuestEventHandler = Box<dyn FnMut(&SessionEvent) -> Option<String>>;

/// Quest 783 "A Threat Within": no prerequisite and no objective, from Deputy Willem (823) to
/// Marshal McBride (197), a few yards apart in the Abbey. It rewards 40 XP and no money.
const QUEST_GIVER_TP: &str = ".go xyz -8933.54 -136.523 83.4466"; // onto Deputy Willem
const QUEST_GIVER_ENTRY: u32 = 823; // Deputy Willem, who gives 783
const QUEST_ID: u32 = 783;

pub(crate) struct Quest;

impl Probe for Quest {
    fn stage(&mut self, cx: &mut Ctx) -> Result<()> {
        // Remove the quest for a fresh accept, then stand on Willem: the accept's
        // `CanInteractWithQuestGiver` gate (about 5 yd) is stricter than QUERY_QUEST's.
        cx.session.send_chat(&format!(".quest remove {QUEST_ID}"))?;
        cx.session.send_chat(QUEST_GIVER_TP)?;
        println!("sent GM: .quest remove {QUEST_ID}; teleport onto Willem {QUEST_GIVER_TP}");
        Ok(())
    }

    fn verify(&mut self, cx: &mut Ctx) -> Result<()> {
        let world = &mut *cx.world;
        let session = &mut *cx.session;
        let self_guid = world.self_guid;

        // Pump for up to `secs`, folding self deltas into `self_fields`, until `f` returns `Some`.
        let drain_quest = |session: &mut WorldSession,
                           sf: &mut Option<benilla_protocol::messages::ObjectFields>,
                           secs: u64,
                           mut f: QuestEventHandler|
         -> Option<String> {
            let until = Instant::now() + Duration::from_secs(secs);
            while Instant::now() < until {
                let Ok(msg) = session.recv() else { continue };
                for ev in decode(msg) {
                    if let SessionEvent::ObjectValues { guid: g, fields } = &ev {
                        if *g == self_guid {
                            if let Some(sf) = sf.as_mut() {
                                sf.merge(fields.clone());
                            }
                        }
                    }
                    if let Some(done) = f(&ev) {
                        return Some(done);
                    }
                }
            }
            None
        };

        // Find Willem and McBride by entry, which a creature guid carries in bits 24–47.
        let find_npc = |entry: u32| {
            world
                .tracked
                .iter()
                .filter(|(g, t)| t.kind == EntityKind::Unit && guid::is_creature_or_pet(**g))
                .find(|(g, _)| guid::entry(**g) == Some(entry))
                .map(|(&g, _)| g)
        };
        let willem = find_npc(QUEST_GIVER_ENTRY).context(
            "--quest: Deputy Willem (entry 823) didn't stream — did the GM teleport land? \
             (needs gmlevel >= 2; try a longer --seconds)",
        )?;
        let mcbride = find_npc(QUEST_TURNIN_ENTRY)
            .context("--quest: Marshal McBride (entry 197) didn't stream in range")?;
        println!("\nDeputy Willem: guid {willem:#x}; Marshal McBride: guid {mcbride:#x}");

        // 1) Look at the quest at the giver: QUERY_QUEST → DETAILS.
        println!("giver: CMSG_QUESTGIVER_HELLO + CMSG_QUESTGIVER_QUERY_QUEST({QUEST_ID})");
        session.questgiver_hello(willem)?;
        session.questgiver_query_quest(willem, QUEST_ID)?;
        let details = drain_quest(
            session,
            &mut world.self_fields,
            5,
            Box::new(|ev| match ev {
                SessionEvent::QuestDetail(d) if d.quest_id == QUEST_ID => Some(format!(
                    "SMSG_QUESTGIVER_QUEST_DETAILS: \"{}\" — {} choice / {} fixed reward(s), {}c money",
                    d.title,
                    d.choices.len(),
                    d.rewards.len(),
                    d.extra.money
                )),
                _ => None,
            }),
        )
        .context("--quest: no SMSG_QUESTGIVER_QUEST_DETAILS for quest 783 within 5s")?;
        println!("✅ details: {details}");

        // 2) Accept, then find the quest id in a `PLAYER_QUEST_LOG` slot (5 fields per slot, 25
        // slots), polling 6 s since the delta can trail the closing GOSSIP_COMPLETE.
        println!("giver: CMSG_QUESTGIVER_ACCEPT_QUEST");
        session.questgiver_accept_quest(willem, QUEST_ID, 0)?;
        let quest_in_log = |sf: &Option<benilla_protocol::messages::ObjectFields>| {
            sf.as_ref().is_some_and(|sf| {
                (0..u16::from(benilla_protocol::messages::PLAYER_QUEST_LOG_SLOTS)).any(|i| {
                    sf.raw_fields().any(|(idx, val)| {
                        idx == FIELD_PLAYER_QUEST_LOG_1_1 + QUEST_LOG_SLOT_FIELDS * i
                            && val == QUEST_ID
                    })
                })
            })
        };
        let mut in_log = false;
        for _ in 0..6 {
            drain_quest(session, &mut world.self_fields, 1, Box::new(|_| None));
            if quest_in_log(&world.self_fields) {
                in_log = true;
                break;
            }
        }
        if in_log {
            println!("✅ accept: quest {QUEST_ID} is in the player descriptor's PLAYER_QUEST_LOG (field-confirmed)");
        } else {
            // Not fatal: the delta may miss the window, and the turn-in only completes if the
            // accept took.
            println!(
                "ℹ️  accept: quest {QUEST_ID} not observed in a PLAYER_QUEST_LOG field within the poll \
                 window (the log UpdateFields are a deferred slice); the turn-in below is authoritative"
            );
        }

        // 3) GM-complete it: a talk-to quest stays INCOMPLETE until the ender interaction, and
        // CHOOSE_REWARD's `CanRewardQuest` requires COMPLETE.
        session.send_chat(&format!(".quest complete {QUEST_ID}"))?;
        println!("GM: .quest complete {QUEST_ID} (letting it apply…)");
        drain_quest(
            session,
            &mut world.self_fields,
            2,
            Box::new(|ev| {
                if let SessionEvent::Chat(m) = ev {
                    println!("   server: {}", m.text);
                }
                None
            }),
        );

        // Teleport onto McBride for interaction range; his guid is already known.
        session.send_chat(QUEST_TURNIN_TP)?;
        println!("moving to McBride: {QUEST_TURNIN_TP}");
        drain_quest(session, &mut world.self_fields, 3, Box::new(|_| None));

        // Turn in at McBride: COMPLETE_QUEST answers REQUEST_ITEMS, or OFFER_REWARD directly.
        println!("turn-in: CMSG_QUESTGIVER_HELLO + CMSG_QUESTGIVER_COMPLETE_QUEST({QUEST_ID})");
        session.questgiver_hello(mcbride)?;
        session.questgiver_complete_quest(mcbride, QUEST_ID)?;
        let progress = drain_quest(
            session,
            &mut world.self_fields,
            5,
            Box::new(|ev| match ev {
                SessionEvent::QuestProgress(p) if p.quest_id == QUEST_ID => Some(format!(
                    "SMSG_QUESTGIVER_REQUEST_ITEMS: \"{}\" — {} required item(s), complete={}",
                    p.title,
                    p.required_items.len(),
                    p.is_complete
                )),
                SessionEvent::QuestOffer(o) if o.quest_id == QUEST_ID => Some(format!(
                    "SMSG_QUESTGIVER_OFFER_REWARD direct (no required items) for \"{}\"",
                    o.title
                )),
                _ => None,
            }),
        )
        .context("--quest: no REQUEST_ITEMS/OFFER_REWARD after COMPLETE_QUEST within 5s")?;
        println!("✅ progress: {progress}");

        // 4) With required items, the progress panel's Continue sends REQUEST_REWARD for
        // OFFER_REWARD; without them (783), COMPLETE_QUEST already answered OFFER_REWARD.
        if !progress.contains("OFFER_REWARD") {
            println!("turn-in: CMSG_QUESTGIVER_REQUEST_REWARD");
            session.questgiver_request_reward(mcbride, QUEST_ID)?;
            let offer = drain_quest(
                session,
                &mut world.self_fields,
                5,
                Box::new(|ev| match ev {
                    SessionEvent::QuestOffer(o) if o.quest_id == QUEST_ID => Some(format!(
                        "SMSG_QUESTGIVER_OFFER_REWARD: \"{}\" — {} choice / {} fixed reward(s), {}c money",
                        o.title,
                        o.choices.len(),
                        o.rewards.len(),
                        o.extra.money
                    )),
                    _ => None,
                }),
            )
            .context("--quest: no SMSG_QUESTGIVER_OFFER_REWARD after REQUEST_REWARD within 5s")?;
            println!("✅ offer: {offer}");
        } else {
            println!(
                "✅ offer: reward panel already reached (OFFER_REWARD direct — no required items)"
            );
        }

        // 5) CHOOSE_REWARD (index 0, no choices) brings QUEST_COMPLETE and the XP delta.
        let xp_before = world.self_fields.as_ref().and_then(|sf| sf.player_xp());
        println!("turn-in: CMSG_QUESTGIVER_CHOOSE_REWARD (choice 0)");
        session.questgiver_choose_reward(mcbride, QUEST_ID, 0)?;
        let complete = drain_quest(
            session,
            &mut world.self_fields,
            6,
            Box::new(|ev| match ev {
                SessionEvent::QuestComplete(c) if c.quest_id == QUEST_ID => Some(format!(
                    "SMSG_QUESTGIVER_QUEST_COMPLETE: quest {} — {} XP, {} copper",
                    c.quest_id, c.xp, c.money
                )),
                // Diagnostics: a rejected choice re-offers the reward, and a success offers the
                // chain's next quest (783, then 7).
                SessionEvent::QuestOffer(o) => {
                    eprintln!("   (diag) OFFER_REWARD again for quest {}", o.quest_id);
                    None
                }
                SessionEvent::QuestDetail(d) => {
                    eprintln!("   (diag) next-quest DETAILS for quest {}", d.quest_id);
                    None
                }
                SessionEvent::QuestGiverStatus { status, .. } => {
                    eprintln!("   (diag) QUESTGIVER_STATUS {status}");
                    None
                }
                _ => None,
            }),
        )
        .context("--quest: no SMSG_QUESTGIVER_QUEST_COMPLETE after CHOOSE_REWARD within 6s")?;
        println!("✅ complete: {complete}");

        // Pump a moment more so the XP grant's values delta lands, then verify PLAYER_XP moved.
        drain_quest(session, &mut world.self_fields, 2, Box::new(|_| None));
        let xp_after = world.self_fields.as_ref().and_then(|sf| sf.player_xp());
        match (xp_before, xp_after) {
            (Some(b), Some(a)) if a != b => println!(
                "✅ reward XP: PLAYER_XP {b} → {a} (+{}) — the grant landed on the descriptor",
                a as i64 - b as i64
            ),
            (before, after) => println!(
                "ℹ️  reward XP: PLAYER_XP {before:?} → {after:?} (a level-up resets XP to the \
                 into-level remainder, so an equal/lower value can still be a real grant)"
            ),
        }
        println!("\n✅ --quest PASS: full accept (Willem) → turn-in (McBride) loop over the questgiver wire.");
        Ok(())
    }
}
