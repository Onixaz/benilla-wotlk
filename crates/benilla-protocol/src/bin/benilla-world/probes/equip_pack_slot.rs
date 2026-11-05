//! `--equip-pack-slot`: `CMSG_AUTOEQUIP_ITEM` on a 1-based backpack slot must land the item in an
//! equipment slot or draw a refusal; the item is then swapped home.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use benilla_protocol::{decode, SessionEvent};

use crate::probes::{Ctx, Probe};

pub(crate) struct EquipPackSlot {
    pub(crate) n: u8,
}

impl Probe for EquipPackSlot {
    fn verify(&mut self, cx: &mut Ctx) -> Result<()> {
        let world = &mut *cx.world;
        let session = &mut *cx.session;
        let n = self.n;

        let sf = world
            .self_fields
            .as_mut()
            .context("no self descriptor — can't find the pack slot")?;
        let n0 = n.checked_sub(1).context("pack slots are 1-based")?;
        let guid = sf
            .player_pack_slot(n0)
            .filter(|g| *g != 0)
            .with_context(|| format!("pack slot {n} is empty/unsent"))?;
        let entry = world.item_entries.get(&guid).copied().unwrap_or(0);
        println!(
            "\nequipping pack slot {n} (guid {guid:#x}, entry {entry}) → CMSG_AUTOEQUIP_ITEM bag 255 slot {}",
            23 + n0
        );
        session.auto_equip_item(255, 23 + n0)?;
        let drain_until = Instant::now() + Duration::from_secs(5);
        let mut verdict: Option<String> = None;
        // The equipment slot the item landed in, for the restore.
        let mut equipped_slot: Option<u8> = None;
        while Instant::now() < drain_until && verdict.is_none() {
            let Ok(msg) = session.recv() else { continue };
            for ev in decode(msg) {
                match ev {
                    SessionEvent::ObjectValues { guid: g, fields } if g == world.self_guid => {
                        sf.merge(fields);
                        if let Some(slot) = (0..19).find(|&i| sf.player_inv_slot(i) == Some(guid)) {
                            equipped_slot = Some(slot);
                            verdict = Some(format!(
                                "equipped — the guid landed in equipment INV slot {slot}"
                            ));
                        }
                    }
                    SessionEvent::InventoryFailure(f) => {
                        verdict = Some(format!(
                            "server refused (InventoryResult {:#04x}, tail {:?})",
                            f.reason, f.tail
                        ));
                    }
                    _ => {}
                }
            }
        }
        let v = verdict.context("no reaction to CMSG_AUTOEQUIP_ITEM within 5s")?;
        println!("✅ auto-equip: {v}.");

        // Swap the equipment slot back to the backpack slot, which also returns anything it
        // displaced. Player slots: equipment 0-18, backpack 23-38.
        if let Some(slot) = equipped_slot {
            println!(
                "restoring: CMSG_SWAP_INV_ITEM equipment slot {slot} → backpack slot {}",
                23 + n0
            );
            session.swap_inv_item(slot, 23 + n0)?;
            let drain_until = Instant::now() + Duration::from_secs(5);
            let mut restored = false;
            while Instant::now() < drain_until && !restored {
                let Ok(msg) = session.recv() else { continue };
                for ev in decode(msg) {
                    if let SessionEvent::ObjectValues { guid: g, fields } = ev {
                        if g == world.self_guid {
                            sf.merge(fields);
                        }
                    }
                }
                restored = sf.player_pack_slot(n0) == Some(guid);
            }
            if !restored {
                bail!("equip restore failed: the item didn't return to backpack slot {n}");
            }
            println!("✅ equip restore: the item is back in backpack slot {n} (character left as found).");
        }
        Ok(())
    }
}
