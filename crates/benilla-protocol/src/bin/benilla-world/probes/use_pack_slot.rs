//! `--use-pack-slot`: `CMSG_USE_ITEM` on a 1-based backpack slot must draw a stack-count delta,
//! a destroy of the item, or a cast-result refusal.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use benilla_protocol::{decode, SessionEvent};

use crate::probes::{Ctx, Probe};

pub(crate) struct UsePackSlot {
    pub(crate) n: u8,
}

impl Probe for UsePackSlot {
    fn verify(&mut self, cx: &mut Ctx) -> Result<()> {
        let world = &mut *cx.world;
        let session = &mut *cx.session;
        let n = self.n;

        // Bag 255 (`INVENTORY_SLOT_BAG_0`) is the player's own array; the backpack is slots 23-38.
        let sf = world
            .self_fields
            .as_ref()
            .context("no self descriptor — can't find the pack slot")?;
        let n0 = n.checked_sub(1).context("pack slots are 1-based")?;
        let guid = sf
            .player_pack_slot(n0)
            .filter(|g| *g != 0)
            .with_context(|| format!("pack slot {n} is empty/unsent"))?;
        let entry = world.item_entries.get(&guid).copied().unwrap_or(0);
        let before = world.item_stacks.get(&guid).copied().unwrap_or(1);
        println!(
            "\nusing pack slot {n} (guid {guid:#x}, entry {entry}, stack {before}) → CMSG_USE_ITEM bag 255 slot {}",
            23 + n0
        );
        // The body names the on-use spell, so read it off the template first.
        session.item_query(entry, guid)?;
        let mut spell_id = None;
        let ask_until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < ask_until && spell_id.is_none() {
            let Ok(msg) = session.recv() else { continue };
            for ev in decode(msg) {
                if let SessionEvent::ItemTemplate {
                    entry: e,
                    info: Some(i),
                    ..
                } = ev
                {
                    if e == entry {
                        spell_id = Some(i.use_spell.map_or(0, |u| u.spell_id));
                    }
                }
            }
        }
        let spell_id = spell_id.context("no template answer for the pack item")?;
        session.use_item(255, 23 + n0, spell_id, guid)?;
        let drain_until = Instant::now() + Duration::from_secs(5);
        let mut verdict: Option<String> = None;
        while Instant::now() < drain_until && verdict.is_none() {
            let Ok(msg) = session.recv() else { continue };
            for ev in decode(msg) {
                match ev {
                    SessionEvent::ObjectValues { guid: g, fields } if g == guid => {
                        if let Some(after) = fields.item_stack_count() {
                            verdict = Some(format!(
                                "stack {before} → {after} (values delta on the guid)"
                            ));
                        }
                    }
                    SessionEvent::ObjectDestroyed(g) if g == guid => {
                        verdict = Some("item destroyed (last charge consumed)".into());
                    }
                    SessionEvent::CastResult {
                        success: false,
                        reason,
                        ..
                    } => {
                        verdict = Some(format!(
                            "server refused (cast result reason {:#04x})",
                            reason.unwrap_or(0)
                        ));
                    }
                    _ => {}
                }
            }
        }
        let v = verdict.context("no reaction to CMSG_USE_ITEM within 5s")?;
        println!("✅ use item: {v}.");
        Ok(())
    }
}
