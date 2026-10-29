//! The melee blood spurt: a self-terminating particle model on the victim, front or back by where
//! the attacker stands, large on a crushing blow, colored by the victim's blood type.

use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;
use benilla_formats::BloodCatalog;

use crate::net::NetEntity;
use benilla_assets::{LockRecover, WorldAssets};

use super::spell_visual::FxSlot;
use super::spell_visual::SpellVisuals;
use super::{SpellKitFx, SwingImpact, SwingMessage};

/// The gore level's ceiling, the reference's per-locale table (`0xaf4e14`): 2 for enUS, 1 for koKR
/// and zhCN, whose blood turns green. `violenceLevel` (0 none, 1 green, 2 true colors) registers at
/// it (`0x7f3a10`).
const MAX_VIOLENCE_LEVEL: usize = 2;

/// `violenceLevel` as its change handler (`0x7f39b0`) holds it: a value outside `[0, max]` reads as
/// the ceiling.
pub(super) fn violence_level(cvars: Option<&crate::cvars::Cvars>) -> usize {
    match cvars.and_then(|c| c.num("violenceLevel")).map(|v| v as i64) {
        Some(level) if (0..=MAX_VIOLENCE_LEVEL as i64).contains(&level) => level as usize,
        _ => MAX_VIOLENCE_LEVEL,
    }
}

/// The victim attachments the spurt hangs on, `0xf` front and `0x10` back.
const ATTACH_FRONT: u16 = 15;
const ATTACH_BACK: u16 = 16;

/// The `UnitBlood` and `UnitBloodLevels` tables; absent, no spurts.
#[derive(Resource)]
pub(super) struct BloodTables(pub(super) BloodCatalog);

/// Load the blood tables off the patch chain at startup.
pub(super) fn load_blood_tables(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_blood_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            let (levels, rows) = cat.len();
            info!("anim: {levels} UnitBloodLevels / {rows} UnitBlood rows");
            commands.insert_resource(BloodTables(cat));
        }
        Err(e) => warn!("anim: blood tables failed to load (no melee spurts): {e:#}"),
    }
}

/// One spurt per landed swing, at the clip's impact key ([`SwingImpact`]). A missing link in the
/// blood tables drops it, as the client skips a null record, and logs why at `info`.
pub(super) fn blood_spurts(
    mut swings: MessageReader<SwingImpact>,
    transforms: Query<&Transform>,
    net: Query<&NetEntity>,
    creatures: Option<Res<crate::entities::Creatures>>,
    blood: Option<Res<BloodTables>>,
    visuals: Option<Res<SpellVisuals>>,
    cvars: Option<Res<crate::cvars::Cvars>>,
    mut fx: MessageWriter<SpellKitFx>,
) {
    let (Some(creatures), Some(blood), Some(visuals)) = (creatures, blood, visuals) else {
        for _ in swings.read() {} // tables not loaded: drain, do not backlog
        return;
    };
    for SwingImpact {
        swing, text_only, ..
    } in swings.read()
    {
        if *text_only {
            continue; // a flush carries only the floating text
        }
        // `0x755e40`: `AFFECTS_VICTIM`, damage or `FAKE_DAMAGE`, and victim state 1 or 4.
        let fake = swing.hit_info & benilla_protocol::messages::hit_info::FAKE_DAMAGE != 0;
        if swing.hit_info & benilla_protocol::messages::hit_info::AFFECTS_VICTIM == 0
            || (swing.damage == 0 && !fake)
            || !matches!(swing.victim_state, 1 | 4)
        {
            if swing.damage > 0 {
                info!(
                    "blood: gate dropped a damaging swing (hit_info {:#x}, victim_state {}, damage {})",
                    swing.hit_info, swing.victim_state, swing.damage
                );
            }
            continue;
        }
        let Some(victim) = swing.victim else {
            info!("blood: dropped — victim entity unresolved");
            continue;
        };
        let Some(display_id) = net.get(victim).ok().and_then(|n| n.display_id) else {
            info!("blood: dropped — victim carries no net display id");
            continue;
        };
        let Some((disp_blood, model_blood)) = creatures.blood_candidates(display_id) else {
            info!("blood: dropped — display {display_id} unknown to the creature catalog");
            continue;
        };
        // The three-tier row resolve. Its tier-3 records-base fallback is real blood, never "no
        // blood": Quilboar, crocolisks and gnolls land there.
        let Some(blood_id) = blood.0.level_key(disp_blood, model_blood) else {
            info!("blood: dropped — UnitBloodLevels is empty");
            continue;
        };
        let blood_id = blood_id as i32;
        // Front or back: the sign of `victimForward · (attackerPos − victimPos)` in WoW space,
        // where a unit's Y rotation is its WoW yaw. An unresolved attacker counts as front.
        let front = match (transforms.get(victim), swing_attacker(&transforms, swing)) {
            (Ok(vt), Some(at)) => {
                let yaw = vt.rotation.to_euler(EulerRot::YXZ).0;
                let (v, a) = (bevy_to_wow(vt.translation), bevy_to_wow(at.translation));
                yaw.cos() * (a[0] - v[0]) + yaw.sin() * (a[1] - v[1]) >= 0.0
            }
            _ => true,
        };
        // `LARGE_BLOOD` picks the large row (`0x755380`).
        let large = swing.hit_info & benilla_protocol::messages::hit_info::LARGE_BLOOD != 0;
        let Some((effect, path)) = blood
            .0
            .effect_id(blood_id, violence_level(cvars.as_deref()), front, large)
            .and_then(|id| visuals.0.effect_path(id).map(|path| (id, path)))
        else {
            info!("blood: dropped — no effect for blood {blood_id} (front {front}, large {large})");
            continue;
        };
        debug!("blood: spurt {path} (blood {blood_id}, front {front}, large {large})");
        fx.write(SpellKitFx::Begin {
            entity: victim,
            spell_id: 0, // no spell: a self-terminating effect is never reaped by id
            persistent: false,
            class: super::FxClass::Hold,
            // Not a kit stage (an effect added straight off the melee path), but it too plays once.
            stage: super::FxStage::OneShot,
            // One spurt per (record, tag) on a body: a new one replaces the old (a replace
            // `resolve_spell_fx` runs for every slot), so a busy fight does not stack them.
            effects: vec![FxSlot {
                tag: if front { ATTACH_FRONT } else { ATTACH_BACK },
                effect,
                path: path.to_string(),
            }],
        });
    }
}

fn swing_attacker<'a>(
    transforms: &'a Query<&Transform>,
    swing: &SwingMessage,
) -> Option<&'a Transform> {
    transforms.get(swing.attacker).ok()
}

#[cfg(test)]
mod tests {
    use super::{violence_level, MAX_VIOLENCE_LEVEL};
    use crate::cvars::Cvars;

    /// The `violenceLevel` change handler (`0x7f39b0`): a level inside `[0, max]` holds, anything
    /// else reads as the ceiling, as does a missing registry.
    #[test]
    fn the_violence_level_holds_inside_its_ceiling() {
        assert_eq!(violence_level(None), MAX_VIOLENCE_LEVEL);
        let mut cvars = Cvars::default();
        assert_eq!(violence_level(Some(&cvars)), MAX_VIOLENCE_LEVEL);
        for (value, level) in [("0", 0), ("1", 1), ("2", 2), ("3", 2), ("-1", 2)] {
            cvars.set("violenceLevel", value);
            assert_eq!(violence_level(Some(&cvars)), level, "violenceLevel {value}");
        }
    }
}
