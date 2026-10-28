//! Clones that wear their caster's look (Mirror Image): a unit with `UNIT_FIELD_FLAGS_2` bit `0x10`
//! asks for it once its model is in (`CGUnit_C::InitializeComponent` `0x730100` →
//! `RequestMirrorImageData` `0x716f10`), and `SMSG_MIRRORIMAGE_DATA` dresses its character model
//! with the caster's race, customization and worn displays (`0x730290`). The reference shows
//! nothing of the clone until the answer; benilla draws its bare display meanwhile.

use benilla_protocol::messages::{MirrorImage, MirrorLook};
use benilla_protocol::{EntityKind, SessionEvent};
use bevy::prelude::*;

use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, NetEntity, ObjectStore};

/// `UNIT_FLAG2_MIRROR_IMAGE`, the clone bit `0x73014a` tests.
const FLAG2_MIRROR_IMAGE: u32 = 0x10;

/// The caster's look a clone wears, for the display it was sent for.
#[derive(Component, Debug)]
pub(crate) struct MirrorLookOf {
    pub(crate) display_id: u32,
    pub(crate) look: MirrorLook,
}

impl MirrorLookOf {
    /// The look, while the unit still shows the display it was sent for.
    pub(crate) fn for_display(&self, display: Option<u32>) -> Option<&MirrorLook> {
        (display == Some(self.display_id)).then_some(&self.look)
    }
}

/// The display a clone's look was asked for, the reference's request bit `0x20000` in
/// `[unit+0xa30]`: a new display asks again, as a new component does.
#[derive(Component)]
pub(super) struct MirrorAsked(u32);

/// Ask once per display for every built clone (`0x730158`).
#[allow(clippy::type_complexity)] // a Bevy system's full input set
pub(super) fn request_mirror_images(
    mut commands: Commands,
    clones: Query<
        (
            Entity,
            &Guid,
            &NetEntity,
            &ObjectStore,
            Option<&MirrorAsked>,
        ),
        With<super::VisualAttached>,
    >,
    net: Res<NetCommands>,
) {
    for (entity, guid, net_entity, store, asked) in &clones {
        let Some(shown) = net_entity.display_id else {
            continue;
        };
        if net_entity.kind != EntityKind::Unit
            || store.0.unit_flags2() & FLAG2_MIRROR_IMAGE == 0
            || asked.is_some_and(|a| a.0 == shown)
        {
            continue;
        }
        debug!("net: asking {:#x}'s mirror image (display {shown})", guid.0);
        let _ = net
            .0
            .send(ClientCommand::GetMirrorImageData { guid: guid.0 });
        commands.entity(entity).insert(MirrorAsked(shown));
    }
}

/// Registers the `SMSG_MIRRORIMAGE_DATA` handler.
pub(super) fn register(app: &mut App) {
    use crate::net::NetHandlerApp;
    app.net_handler(
        benilla_protocol::SessionEventKind::MirrorImageData,
        on_mirror_image_data,
    );
}

/// `SMSG_MIRRORIMAGE_DATA` (`0x7324b0` → `0x730290`): a look for the clone's own display rebuilds
/// it as a character body; one for another display, or with no race, changes nothing.
fn on_mirror_image_data(
    In(ev): In<SessionEvent>,
    mut commands: Commands,
    index: Res<GuidIndex>,
    nets: Query<&NetEntity>,
) {
    let SessionEvent::MirrorImageData {
        guid,
        image: MirrorImage { display_id, look },
    } = ev
    else {
        return;
    };
    let Some(&entity) = index.0.get(&guid) else {
        return;
    };
    let Some(look) = look else {
        return;
    };
    if nets.get(entity).ok().and_then(|n| n.display_id) != Some(display_id) {
        debug!("net: mirror image for {guid:#x} names display {display_id}, not its own");
        return;
    }
    debug!(
        "net: mirror image for {guid:#x}: race {} sex {}",
        look.race, look.sex
    );
    // The worn displays land with the look, so the rebuild dresses the body: a unit is never
    // re-dressed in place.
    let worn = mirror_equipment(&look);
    commands
        .entity(entity)
        .insert((MirrorLookOf { display_id, look }, worn));
    super::live_display::rebuild_visual(&mut commands, entity);
}

/// The clone's worn displays as a player's [`super::Equipment`]: the geoset and composite slots,
/// cloak and helm from the eleven `AddItemBySlot` displays. The guild tabard's emblem
/// (`GuildGetGuildTabard`, `0x730716`) is not built.
pub(super) fn mirror_equipment(look: &MirrorLook) -> super::Equipment {
    let i = &look.items;
    super::Equipment {
        // Shirt, chest, belt, pants, boots, wrist, gloves, tabard.
        bodyslots: [i[2], i[3], i[4], i[5], i[6], i[7], i[8], i[10]],
        cloak: i[9],
        helm: i[0],
        emblem: None,
        tabard_preview: false,
        settled: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look() -> MirrorLook {
        MirrorLook {
            race: 1,
            sex: 0,
            class: 8,
            skin: 2,
            face: 3,
            hair_style: 4,
            hair_color: 5,
            facial_hair: 6,
            guild_id: 0,
            items: [10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20],
        }
    }

    #[test]
    fn the_wire_order_maps_onto_the_dress_slots() {
        let eq = mirror_equipment(&look());
        assert_eq!(eq.helm, 10);
        assert_eq!(eq.cloak, 19);
        assert_eq!(eq.bodyslots, [12, 13, 14, 15, 16, 17, 18, 20]);
        assert!(eq.settled);
    }

    #[test]
    fn a_look_reads_only_for_its_display() {
        let of = MirrorLookOf {
            display_id: 49,
            look: look(),
        };
        assert!(of.for_display(Some(49)).is_some());
        assert!(of.for_display(Some(50)).is_none());
        assert!(of.for_display(None).is_none());
    }
}
