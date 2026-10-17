//! Draw and stow sounds, as 12340's `SI2::PlaySheatheSound` (`0x4d07b0`) picks them: the moved
//! item's `Material.dbc` row names the kit, `SheatheSoundID` to stow and `UnsheatheSoundID` to draw,
//! played 2 yd above the unit. The material rides the wire (`SMSG_ITEM_QUERY_SINGLE_RESPONSE` for
//! players, `UNIT_VIRTUAL_ITEM_INFO` for creatures) into `Wielded`.
//!
//! The trigger is [`SheathSwapMessage`], fired once per arm as that arm's sheath one-shot crosses
//! its swap point. Snap transitions (attack auto-draw, reactive stows, remote units) are silent,
//! as the reference's instant paths play no clip. It is not the `$SHL`/`$SHR` anim tags: HipSheath
//! (90, every one-hand draw) carries no sound events.

use bevy::prelude::*;

use crate::creature_anim::{SheathSwapMessage, Wielded};
use crate::net::NetEntity;
use benilla_assets::WorldAssets;
use benilla_world::schedule::WorldStage;

use super::kit::{play_kit, KitRef, SoundCategory, SoundKits};
use super::{AudioListener, Materials, SoundConfig, SoundOutput};

/// The lift over the unit's position the sound plays at (yd, `[0xa4040c]` at `0x4d07ff`).
const SOUND_LIFT: f32 = 2.0;

fn sheathe_sounds(
    mut swaps: MessageReader<SheathSwapMessage>,
    units: Query<(&Transform, &Wielded), With<NetEntity>>,
    materials: Option<Res<Materials>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    if swaps.is_empty() {
        return;
    }
    let (Some(materials), Some(mut kits), Some(assets)) = (materials, kits, assets) else {
        return;
    };
    let listener = listener.pos;
    for swap in swaps.read() {
        let Ok((transform, wielded)) = units.get(swap.entity) else {
            continue;
        };
        // One message per arm, naming the slot that moved: a sword-and-board draw is two sounds,
        // and a melee → ranged toggle's second movement rings the bow.
        let item = match swap.slot {
            0 => wielded.main,
            1 => wielded.off,
            _ => wielded.ranged,
        };
        if item.is_none() {
            continue; // an empty slot is silent
        }
        let material = u32::from(wielded.materials[usize::from(swap.slot).min(2)]);
        let Some(kit) = materials.0.sheathe_kit(material, !swap.drawing) else {
            continue;
        };
        debug!(
            "sheathe: slot {} material {material} drawing {} kit {kit}",
            swap.slot, swap.drawing
        );
        if let Err(e) = play_kit(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            KitRef::Id(kit),
            Some(transform.translation + Vec3::Y * SOUND_LIFT),
            SoundCategory::Sfx,
        ) {
            warn!("sheathe (kit {kit}): {e:#}");
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, sheathe_sounds.in_set(WorldStage::Present));
}
