//! `--death`: the death wire. [`crate::world::DeathArc`] runs die, release and the ghost
//! transition; this probe adds the corpse query, a GM revive and a post-revive re-query that must
//! find nothing, then checks the whole arc in wire order.

use anyhow::{bail, Context, Result};
use benilla_protocol::{EntityKind, SessionEvent};

use crate::probes::{Ctx, Probe};

#[derive(Default)]
pub(crate) struct Death {
    corpse_query_sent: bool,
    corpse_answer: Option<(bool, i32, [f32; 3], u32)>,
    revive_sent: bool,
    /// The post-revive corpse re-query: the server's "corpse gone" push goes only to looters
    /// (`Map.cpp:3617-3629`), so after a PvE res the client must re-ask and hear not-found.
    post_revive_query_sent: bool,
    corpse_gone: Option<bool>,
    corpse_create: Option<(u64, [f32; 3])>,
    /// The corpse descriptor at create: `(CORPSE_FIELD_FLAGS, owner, bones, lootable, insignia)`.
    corpse_flags: Option<(u32, u64, bool, bool, bool)>,
}

impl Probe for Death {
    fn poll(&mut self, cx: &mut Ctx) -> Result<()> {
        // A released ghost at the graveyard asks where its corpse is (the corpse-run marker).
        if !self.corpse_query_sent {
            if let Some(arc) = &cx.world.death_arc {
                if arc.ghost_seen && arc.graveyard_pos.is_some() {
                    cx.session.corpse_query()?;
                    println!("sent MSG_CORPSE_QUERY");
                    self.corpse_query_sent = true;
                }
            }
        }
        // Then GM-revive as cleanup; a real player corpse-runs and sends `CMSG_RECLAIM_CORPSE`.
        if !self.revive_sent && self.corpse_answer.is_some() {
            cx.session.send_chat(".revive")?;
            println!("sent GM: .revive (self-revive)");
            self.revive_sent = true;
            if let Some(arc) = &mut cx.world.death_arc {
                arc.revive_initiated = true;
            }
        }
        // Once revived, the re-query must find nothing: the corpse unbinds at `SpawnCorpseBones`.
        if !self.post_revive_query_sent {
            if let Some(arc) = &cx.world.death_arc {
                if arc.revived_seen {
                    cx.session.corpse_query()?;
                    println!("sent MSG_CORPSE_QUERY (post-revive — expecting not-found)");
                    self.post_revive_query_sent = true;
                }
            }
        }
        Ok(())
    }

    fn on_event(&mut self, ev: &SessionEvent, cx: &mut Ctx) -> Result<()> {
        match ev {
            SessionEvent::ObjectCreate {
                guid,
                kind,
                position,
                fields,
                ..
            } => {
                // The corpse streams in after release as `EntityKind::Corpse`; an `==` here, unlike
                // a `match`, gets no compiler help if the variant changes.
                let repop_sent = cx.world.death_arc.as_ref().is_some_and(|a| a.repop_sent);
                if repop_sent && self.corpse_create.is_none() && *kind == EntityKind::Corpse {
                    self.corpse_create = Some((*guid, *position));
                    // BONES picks the model, DYNAMIC_FLAGS bit 0 alone opens `CMSG_LOOT`, FLAGS
                    // bit 5 is the PvP insignia. Printed, not asserted.
                    self.corpse_flags = Some((
                        fields.corpse_flags(),
                        fields.corpse_owner().unwrap_or(0),
                        fields.corpse_is_bones(),
                        fields.corpse_lootable(),
                        fields.corpse_pvp_insignia(),
                    ));
                    println!(
                        "corpse object streamed: guid {guid:#x} pos ({:.1}, {:.1}, {:.1})",
                        position[0], position[1], position[2]
                    );
                }
            }
            SessionEvent::CorpseQuery {
                found,
                display_map,
                position,
                corpse_map,
                ..
            } => {
                if self.post_revive_query_sent {
                    self.corpse_gone = Some(!*found);
                    println!("MSG_CORPSE_QUERY (post-revive): found={found}");
                } else {
                    self.corpse_answer = Some((*found, *display_map, *position, *corpse_map));
                    println!(
                        "MSG_CORPSE_QUERY: found={found} display_map={display_map} pos=({:.1}, {:.1}, {:.1}) corpse_map={corpse_map}",
                        position[0], position[1], position[2]
                    );
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn verify(&mut self, cx: &mut Ctx) -> Result<()> {
        let self_map = cx.world.self_map;
        let self_guid = cx.world.self_guid;
        let arc = cx
            .world
            .death_arc
            .as_ref()
            .expect("death_arc present when --death is set");

        // Every signal of the arc in wire order, bailing on the first one missing.
        let death_pos = arc.death_pos.context(
            "--death: `.die` never dropped our health to 0 — is the account gmlevel ≥ 2?",
        )?;
        if !arc.rooted_seen {
            bail!("--death: no SMSG_FORCE_MOVE_ROOT at death");
        }
        if !arc.unroot_seen {
            bail!("--death: no SMSG_FORCE_MOVE_UNROOT after CMSG_REPOP_REQUEST");
        }
        if !arc.water_walk_seen {
            bail!("--death: no SMSG_MOVE_WATER_WALK — the ghost's walk-on-water grant");
        }
        let reclaim_delay_ms = arc
            .reclaim_delay_ms
            .context("--death: no SMSG_CORPSE_RECLAIM_DELAY at release")?;
        if !arc.ghost_seen {
            bail!("--death: PLAYER_FLAGS_GHOST (bit 0x10, field 190) never set");
        }
        // 3D distance from where we died: a wrong z is as much a wire bug as a wrong x or y.
        let dist3 = |a: [f32; 3], b: [f32; 3]| {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        let (corpse_guid, corpse_pos) = self
            .corpse_create
            .context("--death: the corpse object never streamed / streamed at the wrong spot")?;
        let corpse_dist = dist3(corpse_pos, death_pos);
        if corpse_dist > 10.0 {
            bail!(
                "--death: the corpse object never streamed / streamed at the wrong spot ({corpse_dist:.1} yd from where we died)"
            );
        }
        let graveyard_pos = arc
            .graveyard_pos
            .context("--death: no graveyard teleport after release")?;
        let graveyard_dist = dist3(graveyard_pos, death_pos);
        if graveyard_dist <= 20.0 {
            bail!(
                "--death: no graveyard teleport after release (landed only {graveyard_dist:.1} yd from where we died)"
            );
        }
        let (found, _display_map, query_pos, query_map) = self
            .corpse_answer
            .context("--death: MSG_CORPSE_QUERY answered not-found / wrong spot")?;
        let query_dist = dist3(query_pos, death_pos);
        if !found || query_dist > 10.0 || query_map != self_map {
            bail!(
                "--death: MSG_CORPSE_QUERY answered not-found / wrong spot (found={found}, {query_dist:.1} yd from death, map {query_map} vs {})",
                self_map
            );
        }
        if !arc.revived_seen {
            bail!(
                "--death: `.revive` never cleared the ghost flag — the character may be left dead: run with --say \".revive\" to clean up"
            );
        }
        let corpse_gone = self.corpse_gone;
        if corpse_gone != Some(true) {
            bail!(
                "--death: the post-revive corpse query still finds a corpse (got {corpse_gone:?}) — the app's unghost-edge re-query would leave the map tombstone standing"
            );
        }

        println!("\n✅ DEATH ARC SLICE-1 WIRE VERIFIED:");
        println!(
            "  died at        ({:.1}, {:.1}, {:.1})",
            death_pos[0], death_pos[1], death_pos[2]
        );
        println!("  root/unroot    SMSG_FORCE_MOVE_ROOT + SMSG_FORCE_MOVE_UNROOT acked");
        println!("  water-walk     SMSG_MOVE_WATER_WALK acked");
        println!("  reclaim delay  {reclaim_delay_ms} ms");
        println!("  ghost flag     PLAYER_FLAGS_GHOST set at release, cleared at revive");
        println!(
            "  corpse         guid {corpse_guid:#x} pos ({:.1}, {:.1}, {:.1})  ({corpse_dist:.1} yd from death)",
            corpse_pos[0], corpse_pos[1], corpse_pos[2]
        );
        if let Some((flags, owner, bones, lootable, insignia)) = self.corpse_flags {
            println!(
                "  corpse fields  FLAGS {flags:#06x} (bones={bones} insignia={insignia})  DYNFLAGS lootable={lootable}  owner {owner:#x}{}",
                if owner == self_guid { " = us" } else { " ≠ US — the reclaim send would carry the wrong guid" }
            );
        }
        println!(
            "  graveyard      ({:.1}, {:.1}, {:.1})  ({graveyard_dist:.1} yd from death)",
            graveyard_pos[0], graveyard_pos[1], graveyard_pos[2]
        );
        println!("  corpse query   found, map {query_map}  ({query_dist:.1} yd from death)");
        println!("  revive         PLAYER_FLAGS_GHOST cleared — character alive");
        Ok(())
    }
}
