//! `--questlog`: accept quest 7 at McBride, query its template, GM-complete it and require the
//! slot's complete bit, then abandon it and require the slot's id field to clear.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use benilla_protocol::{decode, guid, EntityKind, SessionEvent};

use crate::probes::{
    Ctx, Probe, FIELD_PLAYER_QUEST_LOG_1_1, QUESTLOG_ID, QUEST_LOG_SLOT_FIELDS, QUEST_TURNIN_ENTRY,
    QUEST_TURNIN_TP,
};

const QUESTLOG_TITLE: &str = "Kobold Camp Cleanup";

pub(crate) struct QuestLog;

impl Probe for QuestLog {
    fn stage(&mut self, cx: &mut Ctx) -> Result<()> {
        // Shared with --giverstatus: `mcbride_staged` keeps these GM lines to one send per run.
        if !cx.world.mcbride_staged {
            cx.session
                .send_chat(&format!(".quest remove {QUESTLOG_ID}"))?;
            cx.session.send_chat(QUEST_TURNIN_TP)?;
            cx.world.mcbride_staged = true;
            println!(
                "sent GM: .quest remove {QUESTLOG_ID}; teleport onto McBride {QUEST_TURNIN_TP}"
            );
        }
        Ok(())
    }

    fn verify(&mut self, cx: &mut Ctx) -> Result<()> {
        let world = &mut *cx.world;
        let session = &mut *cx.session;
        let self_guid = world.self_guid;

        // The staging teleport lands on McBride, so he is streamed and in interaction range.
        let mcbride = world
            .tracked
            .iter()
            .filter(|(g, t)| t.kind == EntityKind::Unit && guid::is_creature_or_pet(**g))
            .find(|(g, _)| guid::entry(**g) == Some(QUEST_TURNIN_ENTRY))
            .map(|(&g, _)| g)
            .context(
                "--questlog: Marshal McBride (entry 197) didn't stream — did the GM teleport \
                 land? (needs gmlevel >= 2; try a longer --seconds)",
            )?;
        println!("\nMarshal McBride: guid {mcbride:#x}");

        // 1) Hello, query and accept quest 7 at McBride, who also ends it.
        println!("giver: CMSG_QUESTGIVER_HELLO + CMSG_QUESTGIVER_QUERY_QUEST({QUESTLOG_ID})");
        session.questgiver_hello(mcbride)?;
        session.questgiver_query_quest(mcbride, QUESTLOG_ID)?;
        let mut giver_title: Option<String> = None;
        let drain_until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < drain_until && giver_title.is_none() {
            let Ok(msg) = session.recv() else { continue };
            for ev in decode(msg) {
                if let SessionEvent::QuestDetail(d) = ev {
                    if d.quest_id == QUESTLOG_ID {
                        giver_title = Some(d.title);
                    }
                }
            }
        }
        let giver_title = giver_title
            .context("--questlog: no SMSG_QUESTGIVER_QUEST_DETAILS for quest 7 within 5s")?;
        println!("✅ giver details: \"{giver_title}\"");

        println!("giver: CMSG_QUESTGIVER_ACCEPT_QUEST");
        session.questgiver_accept_quest(mcbride, QUESTLOG_ID, 0)?;
        // The accept's values delta can trail the GOSSIP_COMPLETE that closes the interaction.
        let drain_until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < drain_until {
            let Ok(msg) = session.recv() else { continue };
            for ev in decode(msg) {
                if let SessionEvent::ObjectValues { guid: g, fields } = ev {
                    if g == self_guid {
                        if let Some(sf) = &mut world.self_fields {
                            sf.merge(fields);
                        }
                    }
                }
            }
        }

        // 2) CMSG_QUEST_QUERY: the template parser against the live server's bytes.
        println!("CMSG_QUEST_QUERY({QUESTLOG_ID})");
        session.quest_query(QUESTLOG_ID)?;
        let mut template: Option<benilla_protocol::messages::QuestTemplate> = None;
        let drain_until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < drain_until && template.is_none() {
            let Ok(msg) = session.recv() else { continue };
            for ev in decode(msg) {
                if let SessionEvent::ObjectValues { guid: g, fields } = &ev {
                    if *g == self_guid {
                        if let Some(sf) = &mut world.self_fields {
                            sf.merge(fields.clone());
                        }
                    }
                }
                if let SessionEvent::QuestTemplate(t) = ev {
                    if t.quest_id == QUESTLOG_ID {
                        template = Some(*t);
                    }
                }
            }
        }
        let template =
            template.context("--questlog: no SMSG_QUEST_QUERY_RESPONSE for quest 7 within 5s")?;
        println!(
            "SMSG_QUEST_QUERY_RESPONSE: \"{}\" (level {}), details {} char(s), {} reward(s) / \
             {} choice(s) non-zero, {}c money",
            template.title,
            template.level,
            template.details.len(),
            template.rewards.iter().filter(|(id, _)| *id != 0).count(),
            template.choices.iter().filter(|(id, _)| *id != 0).count(),
            template.money,
        );
        for (i, o) in template.objectives.iter().enumerate() {
            if o.required_count > 0 {
                println!(
                    "  objective {i}: creature_or_go {} required_count {} — \"{}\"",
                    o.creature_or_go, o.required_count, o.text
                );
            }
        }
        for (i, (id, n)) in template.required_items.iter().enumerate() {
            if *n > 0 {
                println!("  required item {i}: {id}x{n}");
            }
        }
        if template.title != QUESTLOG_TITLE {
            bail!(
                "--questlog: SMSG_QUEST_QUERY_RESPONSE title '{}', expected '{QUESTLOG_TITLE}'",
                template.title
            );
        }
        if !template.objectives.iter().any(|o| o.required_count > 0) {
            bail!("--questlog: no objective with required_count > 0 in the parsed template");
        }
        println!(
            "✅ template: title matches '{QUESTLOG_TITLE}', ≥1 objective with required_count > 0."
        );

        // 3) Find the accept's PLAYER_QUEST_LOG slot, GM-complete it, and require the slot's
        // state field (id field + 1) to gain the complete bit, 0x01.
        let find_slot = |sf: &Option<benilla_protocol::messages::ObjectFields>| {
            sf.as_ref().and_then(|sf| {
                (0..benilla_protocol::messages::PLAYER_QUEST_LOG_SLOTS)
                    .find(|&i| sf.player_quest_log(i).map(|s| s.quest_id) == Some(QUESTLOG_ID))
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
                    if let SessionEvent::ObjectValues { guid: g, fields } = ev {
                        if g == self_guid {
                            if let Some(sf) = &mut world.self_fields {
                                sf.merge(fields);
                            }
                        }
                    }
                }
            }
            slot = find_slot(&world.self_fields);
        }
        let slot = slot.context(
            "--questlog: quest 7 never landed in a PLAYER_QUEST_LOG slot within the poll window",
        )?;
        println!("quest {QUESTLOG_ID} occupies PLAYER_QUEST_LOG slot {slot}");

        // Raw rather than decoded, so the exact before and after bytes print.
        let state_index = FIELD_PLAYER_QUEST_LOG_1_1 + QUEST_LOG_SLOT_FIELDS * u16::from(slot) + 1;
        let state_word = |sf: &Option<benilla_protocol::messages::ObjectFields>| {
            sf.as_ref().and_then(|sf| {
                sf.raw_fields()
                    .find(|&(idx, _)| idx == state_index)
                    .map(|(_, v)| v)
            })
        };
        let before_word = state_word(&world.self_fields).unwrap_or(0);
        println!("state word before: {before_word:#010x}");

        session.send_chat(&format!(".quest complete {QUESTLOG_ID}"))?;
        println!("GM: .quest complete {QUESTLOG_ID} (letting it apply…)");
        let mut after_word = None;
        for _ in 0..8 {
            if after_word.is_some() {
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
                        SessionEvent::Chat(m) => println!("   server: {}", m.text),
                        _ => {}
                    }
                }
            }
            if let Some(w) = state_word(&world.self_fields) {
                if w & 0x01 != 0 {
                    after_word = Some(w);
                }
            }
        }
        let after_word = after_word.context(
            "--questlog: the slot's state field never gained the COMPLETE bit (0x01) after GM \
             .quest complete",
        )?;
        println!("state word after:  {after_word:#010x}");
        println!(
            "✅ slot state: quest {QUESTLOG_ID} in slot {slot}; COMPLETE bit set after GM \
             complete ({before_word:#010x} → {after_word:#010x})."
        );

        // 4) CMSG_QUESTLOG_REMOVE_QUEST has no reply; the slot's id field clearing is the ack.
        println!("CMSG_QUESTLOG_REMOVE_QUEST(slot {slot})");
        session.questlog_remove_quest(slot)?;
        let mut cleared = false;
        let drain_until = Instant::now() + Duration::from_secs(8);
        while Instant::now() < drain_until && !cleared {
            let Ok(msg) = session.recv() else { continue };
            for ev in decode(msg) {
                if let SessionEvent::ObjectValues { guid: g, fields } = ev {
                    if g == self_guid {
                        if let Some(sf) = &mut world.self_fields {
                            sf.merge(fields);
                        }
                    }
                }
            }
            cleared = world
                .self_fields
                .as_ref()
                .and_then(|sf| sf.player_quest_log(slot))
                .map(|qs| qs.quest_id)
                == Some(0);
        }
        if !cleared {
            bail!(
                "--questlog: slot {slot}'s PLAYER_QUEST_LOG id field never cleared to 0 within \
                 8s after CMSG_QUESTLOG_REMOVE_QUEST (no ack SMSG exists on this wire — the field \
                 clear IS the confirmation)"
            );
        }
        println!(
            "✅ abandon: slot {slot}'s PLAYER_QUEST_LOG id field cleared to 0 (no ack SMSG on \
             this wire — the field update is the confirmation)."
        );

        println!(
            "\n✅ --questlog PASS: template parsed, slot {slot} tracked → COMPLETE → abandoned \
             (id field cleared) — the quest-log wire verified end to end."
        );
        Ok(())
    }
}
