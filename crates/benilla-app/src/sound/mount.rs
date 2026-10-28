//! The dismount sound, the one mount-transition sound the client plays: the dismount handler
//! `0x607ce0` always plays a fixed kit, resolved once at startup by name-match to
//! `"SpiritWolf (DONOTRENAME)"` (`0x623110` → `0x8627bc`), at the dismounting unit. It is not
//! CreatureSoundData, and the attach path `0x607a00` plays nothing, so mounting is silent. A
//! remount (id → id′) is not a dismount.

use bevy::prelude::*;

use crate::net::FieldChanged;
use benilla_assets::WorldAssets;
use benilla_world::schedule::WorldStage;

use super::kit::{play_kit, KitRef, SoundCategory, SoundKits};
use super::{AudioListener, SoundConfig, SoundOutput};

/// The client's fixed dismount kit by SoundEntries `Name`, the constant at `0x8627bc`.
const DISMOUNT_KIT: &str = "SpiritWolf (DONOTRENAME)";

/// Play the dismount kit on any streamed unit's `UNIT_FIELD_MOUNTDISPLAYID` edge to zero, or at an
/// `SMSG_DISMOUNT` drop, whose field edge then plays nothing: the drop already destroyed the mount.
/// The edge stream skips creates, so streaming in unmounted is not a dismount.
#[allow(clippy::too_many_arguments)] // a Bevy system's full input set
fn dismount_sounds(
    mut drops: MessageReader<crate::entities::MountDropped>,
    mut dropped: Local<std::collections::HashSet<Entity>>,
    mut edges: MessageReader<FieldChanged>,
    poses: Query<&Transform>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    let (Some(mut kits), Some(assets)) = (kits, assets) else {
        return;
    };
    let listener = listener.pos;
    let mut plays = Vec::new();
    for d in drops.read() {
        dropped.insert(d.entity);
        plays.push((d.entity, d.display));
    }
    for e in edges.read() {
        if !e.unit_field(benilla_protocol::field::FIELD_UNIT_MOUNTDISPLAYID) {
            continue;
        }
        // Any move of the field ends the drop's stand-in, a remount included.
        if !dropped.remove(&e.entity) && e.new == 0 {
            plays.push((e.entity, e.old));
        }
    }
    for (entity, old) in plays {
        let Ok(transform) = poses.get(entity) else {
            continue;
        };
        debug!("dismount kit on {entity:?} (was mount {old})");
        if let Err(err) = play_kit(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            KitRef::Name(DISMOUNT_KIT),
            Some(transform.translation),
            SoundCategory::Sfx,
        ) {
            warn!("dismount kit ({DISMOUNT_KIT}): {err:#}");
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_message::<crate::entities::MountDropped>()
        .add_systems(Update, dismount_sounds.in_set(WorldStage::Present));
}
