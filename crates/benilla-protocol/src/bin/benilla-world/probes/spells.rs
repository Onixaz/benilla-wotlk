//! `--spells`: require `SMSG_INITIAL_SPELLS` and `SMSG_ACTION_BUTTONS` at login, then a
//! `SMSG_CAST_RESULT` for a self cast, a ground cast and a packed-guid targeted cast.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use benilla_protocol::{decode, guid, EntityKind, SessionEvent};

use crate::probes::{Ctx, Probe};

/// "Rough Dynamite": dest-targeted (`Targets = 0x40`), no mana, reagent, equipment or aura state,
/// so any class can cast it once GM-learnt.
const DEST_SPELL: u32 = 4054;

#[derive(Default)]
pub(crate) struct Spells {
    cast_sent: Option<u32>,
    targeted_cast_sent: Option<(u32, u64)>,
    dest_cast_sent: Option<[f32; 3]>,
    dest_learn_sent: bool,
    /// Set by `SMSG_LEARNED_SPELL` for [`DEST_SPELL`]. The server runs `.learn` after the
    /// session's opcode batch, so a cast in the same batch is dropped as unknown.
    dest_spell_known: bool,
}

impl Probe for Spells {
    fn poll(&mut self, cx: &mut Ctx) -> Result<()> {
        // `HandleCastSpellOpcode` drops a passive with no CAST_RESULT, so pick a bar spell,
        // skipping auto-attack 6603 (a toggle, not a cast), else Battle Shout 6673.
        if self.cast_sent.is_none() {
            if let (Some(book), Some(bar)) = (&cx.world.spell_book, &cx.world.bar_spells) {
                let spell = bar
                    .iter()
                    .find(|&&s| s != 6603 && book.contains(&s))
                    .copied()
                    .or_else(|| book.contains(&6673).then_some(6673));
                if let Some(spell) = spell {
                    cx.session.cast_spell(spell, None)?;
                    println!("sent CMSG_CAST_SPELL for spell {spell} (self)");
                    self.cast_sent = Some(spell);
                } else {
                    bail!("no castable spell to probe with (bar has no known active spell)");
                }
            }
        }
        // Phase 3, the ground cast: mask 0x40 and three f32 coords, the targeting cursor's world
        // click body, cast at our own feet so it is always in range and line of sight.
        if self.dest_cast_sent.is_none() && cx.world.cast_verdict.is_some() {
            // A prior run's learn is already in the login book; no SMSG_LEARNED_SPELL will come.
            if !self.dest_spell_known
                && cx
                    .world
                    .spell_book
                    .as_ref()
                    .is_some_and(|b| b.contains(&DEST_SPELL))
            {
                self.dest_spell_known = true;
            }
            if !self.dest_spell_known && !self.dest_learn_sent {
                cx.session.send_chat(&format!(".learn {DEST_SPELL}"))?;
                println!("sent .learn {DEST_SPELL} (Rough Dynamite — the dest-cast phase's spell)");
                self.dest_learn_sent = true;
            }
            if self.dest_spell_known {
                let (pos, _) = cx.world.self_pose();
                cx.world.dest_spell = Some(DEST_SPELL);
                cx.session.cast_spell_at_dest(DEST_SPELL, pos)?;
                println!(
                    "sent CMSG_CAST_SPELL for spell {DEST_SPELL} at dest ({:.2}, {:.2}, {:.2}) — mask 0x40",
                    pos[0], pos[1], pos[2]
                );
                self.dest_cast_sent = Some(pos);
            }
        }
        // Phase 2, mask 2 with a packed guid. It runs after the ground cast: the deferred `.learn`
        // acts on the selection and fails with "Player not found!" on a creature.
        if self.targeted_cast_sent.is_none() && cx.world.dest_verdict.is_some() {
            if let Some(spell) = self.cast_sent {
                if let Some((&guid, _)) = cx
                    .world
                    .tracked
                    .iter()
                    .find(|(g, t)| t.kind == EntityKind::Unit && guid::is_creature_or_pet(**g))
                {
                    cx.session.set_selection(guid)?;
                    cx.session.cast_spell(spell, Some(guid))?;
                    println!("sent CMSG_CAST_SPELL for spell {spell} at {guid:#x} (packed target)");
                    self.targeted_cast_sent = Some((spell, guid));
                }
            }
        }
        Ok(())
    }

    fn on_event(&mut self, ev: &SessionEvent, _cx: &mut Ctx) -> Result<()> {
        // The dest phase's learn ack; `poll` checks a login book that already has the spell.
        if let SessionEvent::SpellLearned { spell_id } = ev {
            if *spell_id == DEST_SPELL {
                self.dest_spell_known = true;
            }
        }
        Ok(())
    }

    fn verify(&mut self, cx: &mut Ctx) -> Result<()> {
        let world = &mut *cx.world;
        let session = &mut *cx.session;

        // Inventory readout: what the server says the character holds.
        if let Some(sf) = &world.self_fields {
            let slot_entry = |guid: Option<u64>| {
                guid.filter(|g| *g != 0)
                    .and_then(|g| world.item_entries.get(&g).copied())
            };
            let mut wanted: Vec<u32> = (0..23)
                .filter_map(|i| slot_entry(sf.player_inv_slot(i)))
                .chain((0..16).filter_map(|i| slot_entry(sf.player_pack_slot(i))))
                .collect();
            wanted.sort_unstable();
            wanted.dedup();
            for e in &wanted {
                if !world.item_names.contains_key(e) {
                    session.item_query(*e, 0)?;
                }
            }
            let drain_until = Instant::now() + Duration::from_secs(3);
            while Instant::now() < drain_until
                && wanted.iter().any(|e| !world.item_names.contains_key(e))
            {
                if let Ok(msg) = session.recv() {
                    for ev in decode(msg) {
                        if let SessionEvent::ItemTemplate {
                            entry,
                            info: Some(i),
                            ..
                        } = ev
                        {
                            world.item_names.insert(
                                entry,
                                format!("{} [class {} subclass {}]", i.name, i.class, i.subclass),
                            );
                        }
                    }
                }
            }
            let show_equipped = |label: &str, slot: u8| {
                let text = match (sf.player_inv_slot(slot), sf.player_visible_item_entry(slot)) {
                    (_, Some(e)) => world
                        .item_names
                        .get(&e)
                        .cloned()
                        .unwrap_or_else(|| format!("entry {e}")),
                    (Some(g), None) if g != 0 => format!("guid {g:#x} (no visible entry)"),
                    _ => "EMPTY".to_string(),
                };
                println!("  {label:<10} {text}");
            };
            let show = |label: &str, guid: Option<u64>| {
                let text = match guid {
                    None => "(not sent)".to_string(),
                    Some(0) => "EMPTY".to_string(),
                    Some(g) => match world.item_entries.get(&g) {
                        Some(e) => world
                            .item_names
                            .get(e)
                            .cloned()
                            .unwrap_or_else(|| format!("entry {e}")),
                        None => format!("guid {g:#x} (no item object streamed)"),
                    },
                };
                println!("  {label:<10} {text}");
            };
            // Names come from the public visible-item entries; private INV guids show presence.
            for slot in [15u8, 16, 17] {
                if let Some(e) = sf.player_visible_item_entry(slot) {
                    if !world.item_names.contains_key(&e) {
                        session.item_query(e, 0)?;
                    }
                }
            }
            println!(
                "
--- One's hands + pack (server truth) ---"
            );
            show_equipped("mainhand", 15);
            show_equipped("offhand", 16);
            show_equipped("ranged", 17);
            for i in 0..16 {
                if let Some(g) = sf.player_pack_slot(i).filter(|g| *g != 0) {
                    show(&format!("pack {i}"), Some(g));
                }
            }
        } else {
            println!("(no self descriptor captured — inventory readout skipped)");
        }

        let book = world
            .spell_book
            .clone()
            .context("no SMSG_INITIAL_SPELLS arrived")?;
        if book.is_empty() {
            bail!("SMSG_INITIAL_SPELLS parsed to an empty spell book");
        }
        let bar = world
            .bar_spells
            .clone()
            .context("no SMSG_ACTION_BUTTONS arrived")?
            .len();
        let sent = self.cast_sent.context("cast never sent (empty book?)")?;
        let (spell, success, reason) = world
            .cast_verdict
            .context("no SMSG_CAST_RESULT for our CMSG_CAST_SPELL")?;
        if spell != sent {
            bail!("cast result names spell {spell}, we cast {sent}");
        }
        match (world.item_asked, &world.item_answer) {
            (Some(e), Some((entry, Some(name)))) if *entry == e => {
                println!("✅ item query: entry {e} → '{name}'.");
            }
            (Some(e), Some((entry, None))) if *entry == e => {
                println!("✅ item query: entry {e} → unknown (miss shape parsed).");
            }
            (Some(e), _) => bail!("no SMSG_ITEM_QUERY_SINGLE_RESPONSE for entry {e}"),
            (None, _) => println!("(no item-kind button on the bar to item-query)"),
        }
        // The ground cast must be accepted: no verdict means the body desynced the server's
        // reader, and a refusal carries the `CheckCast` reason.
        match (self.dest_cast_sent, &world.dest_verdict) {
            (Some(pos), Some((_, true, _))) => {
                println!(
                    "✅ ground cast (mask 0x40 + dest): spell {DEST_SPELL} at ({:.2}, {:.2}, {:.2}) → ok.",
                    pos[0], pos[1], pos[2]
                );
            }
            (Some(_), Some((_, false, r))) => {
                bail!(
                    "ground cast {DEST_SPELL} REFUSED (reason {:#04x}) — the dest body parsed but CheckCast said no",
                    r.unwrap_or(0)
                )
            }
            (Some(_), None) => {
                bail!("no SMSG_CAST_RESULT for the GROUND cast of {DEST_SPELL} — the mask-0x40 dest body desyncs the server")
            }
            (None, _) => bail!("ground cast never sent (phase 1 never resolved?)"),
        }
        match (self.targeted_cast_sent, &world.targeted_verdict) {
            (Some((spell, guid)), Some((s2, ok2, r2))) if *s2 == spell => {
                println!(
                    "✅ targeted cast (packed guid): spell {spell} at {guid:#x} → {}.",
                    if *ok2 {
                        "ok".to_string()
                    } else {
                        format!("failed (reason {:#04x})", r2.unwrap_or(0))
                    }
                );
            }
            (Some((spell, _)), _) => {
                bail!("no SMSG_CAST_RESULT for the TARGETED cast of {spell} — the mask-2 packed-guid body desyncs the server")
            }
            (None, _) => println!("(no creature in range for the targeted-cast phase)"),
        }
        println!(
            "\n✅ spells: {} known, {bar} bar slot(s), cast {sent} → {}.",
            book.len(),
            if success {
                "ok".to_string()
            } else {
                format!(
                    "failed (reason {:#04x}) — round trip proven",
                    reason.unwrap_or(0)
                )
            }
        );
        Ok(())
    }
}
