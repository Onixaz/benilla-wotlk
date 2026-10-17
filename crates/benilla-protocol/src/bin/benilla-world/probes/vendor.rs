//! `--vendor`: list the nearest vendor, buy its cheapest row and require a reaction; a coinage
//! delta confirms the `PLAYER_FIELD_COINAGE` accessor.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use benilla_protocol::{decode, SessionEvent};

use crate::probes::{Ctx, Probe};

/// vmangos `INTERACTION_DISTANCE` (`Objects/ObjectDefines.h:24`): every NPC service opcode past
/// it is silently ignored.
const INTERACTION_DISTANCE: f32 = 5.0;

pub(crate) struct Vendor;

impl Probe for Vendor {
    fn verify(&mut self, cx: &mut Ctx) -> Result<()> {
        let world = &mut *cx.world;
        let session = &mut *cx.session;
        let self_guid = world.self_guid;

        let sf = world
            .self_fields
            .as_mut()
            .context("no self descriptor — can't read coinage / find a vendor")?;
        let coinage_before = sf.player_money();
        println!(
            "\nPLAYER_FIELD_COINAGE at login = {} copper (money accessor)",
            coinage_before
                .map(|c| c.to_string())
                .unwrap_or_else(|| "unsent".into())
        );

        let self_pos = world.tracked.get(&self_guid).map(|t| t.position);
        // Nearest by 2D distance; Northshire is nearly flat.
        let vendor = self_pos.and_then(|p| {
            world
                .vendors
                .iter()
                .map(|(&g, vp)| (g, *vp, (vp[0] - p[0]).hypot(vp[1] - p[1])))
                .min_by(|a, b| a.2.total_cmp(&b.2))
        });
        let (vendor_guid, vendor_pos, dist) = vendor.context(
            "no UNIT_NPC_FLAG_VENDOR creature streamed in range (fresh Northshire spawn has none — \
             re-run with a longer --seconds after moving toward a vendor, or grant GM to teleport)",
        )?;
        // The server's check (`Player::CanInteractWithNPC`, `Objects/Player.cpp:2556`) is 3D and
        // allows for both bounding radii, so this 2D screen is only approximate.
        if dist > INTERACTION_DISTANCE {
            bail!(
                "nearest vendor {vendor_guid:#x} is {dist:.1} yd away; vmangos refuses \
                 CMSG_LIST_INVENTORY past INTERACTION_DISTANCE ({INTERACTION_DISTANCE:.0} yd) and \
                 answers nothing at all. Stand on it and re-run:\n    \
                 --say \".go xyz {:.1} {:.1} {:.1} 0\"",
                vendor_pos[0],
                vendor_pos[1],
                vendor_pos[2],
            );
        }
        println!(
            "nearest vendor: guid {vendor_guid:#x} ({dist:.1} yd) — sending CMSG_LIST_INVENTORY"
        );
        session.list_inventory(vendor_guid)?;

        let mut items: Option<Vec<benilla_protocol::messages::VendorItem>> = None;
        let drain_until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < drain_until && items.is_none() {
            let Ok(msg) = session.recv() else { continue };
            for ev in decode(msg) {
                if let SessionEvent::VendorInventory {
                    vendor,
                    items: rows,
                    ..
                } = ev
                {
                    if vendor == vendor_guid {
                        items = Some(rows);
                    }
                }
            }
        }
        let rows = items.context("no SMSG_LIST_INVENTORY arrived within 5s")?;
        println!("✅ list: SMSG_LIST_INVENTORY parsed {} row(s):", rows.len());
        for r in rows.iter().take(8) {
            let stock = if r.current_count == 0xFFFF_FFFF {
                "∞".to_string()
            } else {
                r.current_count.to_string()
            };
            println!(
                "  slot {:>2}  entry {:>6}  price {:>7}c  stock {stock}  buy_count {}",
                r.slot, r.entry, r.price, r.buy_count
            );
        }
        if rows.is_empty() {
            bail!("the vendor listed 0 rows — can't exercise a buy");
        }

        // `current_count` 0 is sold out and 0xFFFF_FFFF unlimited.
        let cheapest = rows
            .iter()
            .filter(|r| r.price > 0 && r.current_count != 0)
            .min_by_key(|r| r.price)
            .context("no priced, in-stock row to buy")?;
        println!(
            "buying cheapest: entry {} @ {}c → CMSG_BUY_ITEM",
            cheapest.entry, cheapest.price
        );
        session.buy_item(vendor_guid, cheapest.entry, cheapest.slot, 1)?;

        // Keep draining past the first reaction: the coinage delta can trail the item create.
        let mut round_trip: Option<String> = None;
        let mut coinage_delta: Option<String> = None;
        let drain_until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < drain_until && (round_trip.is_none() || coinage_delta.is_none()) {
            let Ok(msg) = session.recv() else { continue };
            for ev in decode(msg) {
                match ev {
                    SessionEvent::ObjectValues { guid: g, fields } if g == self_guid => {
                        sf.merge(fields);
                        if let (Some(before), Some(after)) = (coinage_before, sf.player_money()) {
                            if after != before && coinage_delta.is_none() {
                                coinage_delta = Some(format!(
                                    "coinage {before} → {after} copper (paid {}) — the delta landed on \
                                     PLAYER_FIELD_COINAGE (index-confirmed accessor)",
                                    before as i64 - after as i64
                                ));
                            }
                        }
                    }
                    SessionEvent::VendorBuyResult {
                        slot, new_count, ..
                    } => {
                        round_trip.get_or_insert(format!(
                            "SMSG_BUY_ITEM: vendor slot {slot} stock now {new_count} (purchase ok)"
                        ));
                    }
                    SessionEvent::ItemCreate {
                        guid: g, fields, ..
                    } => {
                        let entry = fields.object_entry().unwrap_or(0);
                        if entry == cheapest.entry {
                            round_trip.get_or_insert(format!(
                                "the bought item arrived (ItemCreate guid {g:#x}, entry {entry})"
                            ));
                        }
                    }
                    SessionEvent::VendorBuyFailed {
                        item_entry, reason, ..
                    } => {
                        round_trip.get_or_insert(format!(
                            "SMSG_BUY_FAILED for entry {item_entry} (reason {reason}) — round trip proven"
                        ));
                    }
                    _ => {}
                }
            }
        }
        let v = round_trip.context("no reaction to CMSG_BUY_ITEM within 5s")?;
        println!("✅ vendor buy: {v}.");
        if let Some(d) = coinage_delta {
            println!("✅ money accessor: {d}.");
        }
        Ok(())
    }
}
