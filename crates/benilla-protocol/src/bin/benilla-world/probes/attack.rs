//! `--attack`: the melee-swing wire. Teleports onto a Northshire Kobold Vermin, swings at the
//! nearest creature and requires an `SMSG_ATTACKERSTATEUPDATE`.

use anyhow::{bail, Context, Result};
use benilla_protocol::{guid, EntityKind};

use crate::probes::{Ctx, Probe};
use crate::world::{ATTACK_TP, ATTACK_TP_POS};

#[derive(Default)]
pub(crate) struct Attack {
    attack_target: Option<u64>,
}

impl Probe for Attack {
    fn stage(&mut self, cx: &mut Ctx) -> Result<()> {
        // --loot and the death arc share this teleport; `attack_tp_staged` sends it once.
        if !cx.world.attack_tp_staged {
            cx.session.send_chat(ATTACK_TP)?;
            cx.world.attack_tp_staged = true;
            println!("sent GM teleport: {ATTACK_TP}");
        }
        Ok(())
    }

    fn poll(&mut self, cx: &mut Ctx) -> Result<()> {
        // A login already at the spot gets no teleport ack: the login position is the landing.
        if cx.world.attack_pos.is_none() {
            if let Some(me) = cx.world.tracked.get(&cx.world.self_guid) {
                let p = me.position;
                let d = (p[0] - ATTACK_TP_POS[0]).hypot(p[1] - ATTACK_TP_POS[1]);
                if d < 10.0 {
                    println!("already at the attack spot ({d:.1} yd off): no teleport needed");
                    cx.world.attack_pos = Some(p);
                }
            }
        }
        // Once landed, swing at the nearest creature within 20 yd: the kobold we stand on.
        if self.attack_target.is_none() {
            if let Some(pos) = cx.world.attack_pos {
                let nearest = cx
                    .world
                    .tracked
                    .iter()
                    .filter(|(g, t)| t.kind == EntityKind::Unit && guid::is_creature_or_pet(**g))
                    .map(|(&g, t)| {
                        let d = (t.position[0] - pos[0]).hypot(t.position[1] - pos[1]);
                        (g, d)
                    })
                    .filter(|(_, d)| *d < 20.0)
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((guid, dist)) = nearest {
                    cx.session.set_selection(guid)?;
                    cx.session.attack_swing(guid)?;
                    println!("sent CMSG_ATTACKSWING at {guid:#x} ({dist:.1} yd)");
                    self.attack_target = Some(guid);
                }
            }
        }
        Ok(())
    }

    fn verify(&mut self, cx: &mut Ctx) -> Result<()> {
        if cx.world.attack_pos.is_none() {
            bail!("--attack: the GM teleport never arrived (is the account gmlevel ≥ 2?)");
        }
        let target = self
            .attack_target
            .context("--attack: no creature within 20 yd of the landing")?;
        let swings_seen = cx.world.swings_seen;
        if swings_seen == 0 {
            bail!("--attack: swung at {target:#x} but no SMSG_ATTACKERSTATEUPDATE decoded");
        }
        println!("✅ attack: {swings_seen} SMSG_ATTACKERSTATEUPDATE swing(s) decoded (target {target:#x}).");
        // Refusals are reported, never required: the probe stands on its kobold, so the server has
        // no cause to send one.
        let refusals = &cx.world.swing_refusals;
        if refusals.is_empty() {
            println!(
                "   (no SMSG_ATTACKSWING refusal seen — expected, we swing from on top of it)"
            );
        } else {
            println!(
                "   {} swing refusal(s) decoded: {refusals:?}",
                refusals.len()
            );
        }
        Ok(())
    }
}
