//! A group member or pet selected by guid alone: the reference's target is a guid, so a member out
//! of view stays selected and comes back as a unit when it streams in again.

use bevy::prelude::*;

use crate::net::{ClientCommand, Guid, GuidIndex, ObjectStore, SelfPlayer};

use super::Selection;

/// What [`RosterTarget::follow`] reads besides the selection, the group and the send.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct RosterTarget<'w, 's> {
    index: Option<Res<'w, GuidIndex>>,
    stores: Query<'w, 's, &'static ObjectStore>,
    me: Query<'w, 's, (&'static Guid, &'static ObjectStore), With<SelfPlayer>>,
}

impl RosterTarget<'_, '_> {
    /// Follow the selection's entity across the member's streaming: a held target that leaves the
    /// object manager stays selected by guid while the group names it and it is not dueling us
    /// (`0x734fd0`'s gate at `0x735127`); a guid-only target that streams in is bound to its
    /// entity; one the group no longer names is cleared with its send, as the roster's removal and
    /// offline legs do (`0x52d7a7`, `0x52d626`). A stranger's teardown is left to the ring.
    /// `held` is whether an entity is still a held object.
    pub(super) fn follow(
        &self,
        selection: &mut ResMut<Selection>,
        group: Option<&crate::ui_party::GroupState>,
        held: impl Fn(Entity) -> bool,
        net: &crate::net::NetCommands,
    ) {
        let Some(guid) = selection.guid else {
            return;
        };
        if selection.target.is_some_and(&held) {
            return;
        }
        let own = self.me.single().ok().map(|(g, s)| (g.0, &s.0));
        let names = |guid: u64| {
            let live = |member: u64| {
                let &entity = self.index.as_deref()?.0.get(&member)?;
                self.stores.get(entity).ok().map(|s| &s.0)
            };
            group.is_some_and(|g| g.names_member_or_pet(guid, own, live))
        };
        match selection.target {
            Some(gone) => {
                // A duel opponent's teardown breaks the target even in the group (`0x735151`).
                let dueling = self
                    .stores
                    .get(gone)
                    .ok()
                    .zip(own)
                    .is_some_and(|(unit, (_, me))| {
                        matches!(
                            super::ring::duel_rung(&unit.0, me),
                            super::ring::DuelRung::Engaged { .. }
                        )
                    });
                if names(guid) && !dueling {
                    debug!("target: {guid:#x} left view; kept by guid as a group member");
                    selection.target = None;
                }
            }
            None => {
                if let Some(&entity) = self.index.as_deref().and_then(|i| i.0.get(&guid)) {
                    debug!("target: {guid:#x} streamed in; bound to its unit");
                    selection.target = Some(entity);
                } else if !names(guid) {
                    debug!("target: {guid:#x} left the group out of view; cleared");
                    selection.guid = None;
                    let _ = net.0.send(ClientCommand::SetSelection { guid: 0 });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{member_status, GroupMemberEntry};
    use bevy::ecs::system::RunSystemOnce;

    const ME: u64 = 1;
    const MEMBER: u64 = 0x1234;

    fn world() -> (World, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut world = World::new();
        world.insert_resource(crate::net::NetCommands(tx));
        world.init_resource::<GuidIndex>();
        world.init_resource::<Selection>();
        let mut group = crate::ui_party::GroupState::default();
        group.apply_list(
            0,
            0,
            vec![GroupMemberEntry {
                name: "Brisca".into(),
                guid: MEMBER,
                status: member_status::ONLINE,
                flags: 0,
                roles: 0,
            }],
            ME,
            None,
            Some(ME),
        );
        world.insert_resource(group);
        world.spawn((
            SelfPlayer,
            Guid(ME),
            ObjectStore(benilla_protocol::ObjectFields::default()),
        ));
        (world, rx)
    }

    fn step(world: &mut World) -> (Option<Entity>, Option<u64>) {
        world
            .run_system_once(
                |mut selection: ResMut<Selection>,
                 roster: RosterTarget,
                 group: Res<crate::ui_party::GroupState>,
                 held: Query<(), With<Guid>>,
                 net: Res<crate::net::NetCommands>| {
                    roster.follow(&mut selection, Some(&group), |e| held.contains(e), &net);
                },
            )
            .expect("the system runs");
        let sel = world.resource::<Selection>();
        (sel.target, sel.guid)
    }

    /// A selected member that streams out stays selected by guid, is bound again when it streams
    /// in, and is cleared, with `CMSG_SET_SELECTION` 0, once the group no longer names it.
    #[test]
    fn a_member_target_survives_leaving_view_and_clears_on_leaving_the_group() {
        let (mut world, rx) = world();
        let member = world
            .spawn((
                Guid(MEMBER),
                ObjectStore(benilla_protocol::ObjectFields::default()),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(MEMBER, member);
        {
            let mut sel = world.resource_mut::<Selection>();
            sel.target = Some(member);
            sel.guid = Some(MEMBER);
        }
        assert_eq!(step(&mut world), (Some(member), Some(MEMBER)), "held");

        world.resource_mut::<GuidIndex>().0.remove(&MEMBER);
        world.despawn(member);
        assert_eq!(step(&mut world), (None, Some(MEMBER)), "kept by guid");
        assert!(rx.try_recv().is_err(), "nothing sent");

        let back = world.spawn((Guid(MEMBER),)).id();
        world.resource_mut::<GuidIndex>().0.insert(MEMBER, back);
        assert_eq!(step(&mut world), (Some(back), Some(MEMBER)), "bound again");

        world.resource_mut::<GuidIndex>().0.remove(&MEMBER);
        world.despawn(back);
        assert_eq!(step(&mut world), (None, Some(MEMBER)));
        world
            .resource_mut::<crate::ui_party::GroupState>()
            .apply_list(0, 0, Vec::new(), ME, None, Some(ME));
        assert_eq!(step(&mut world), (None, None), "left the group");
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::SetSelection { guid: 0 })
        ));
    }

    /// A stranger's teardown is not this system's: the ring clears it.
    #[test]
    fn a_strangers_teardown_is_left_to_the_ring() {
        let (mut world, _rx) = world();
        let stranger = world.spawn(Guid(0xC0DE)).id();
        {
            let mut sel = world.resource_mut::<Selection>();
            sel.target = Some(stranger);
            sel.guid = Some(0xC0DE);
        }
        world.despawn(stranger);
        assert_eq!(step(&mut world), (Some(stranger), Some(0xC0DE)));
    }
}
