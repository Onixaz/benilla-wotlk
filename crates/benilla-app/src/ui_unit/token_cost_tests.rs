//! A token read costs a few views of the lent `World` whatever the raid's size: the resolver
//! (`0x60abf0`) reads each table once per call, never once per row.

use benilla_protocol::messages::{member_status, GroupMemberEntry};
use benilla_protocol::ObjectFields;
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use crate::net::{Guid, GuidIndex, ObjectStore, SelfGuid, SelfPlayer};
use crate::target::Selection;
use crate::ui_party::{GroupState, GROUPTYPE_RAID};
use crate::ui_script::live::{with_live_world, VIEW_BUILDS};

const ME: u64 = 0x10;

/// The 39 members after us on the roster, `raid2` to `raid40`.
fn member(i: u64) -> u64 {
    0x1000 + i
}

/// A held player at full health targeting `target`.
fn player(target: u64) -> ObjectStore {
    let target_field = benilla_protocol::field::FIELD_UNIT_TARGET;
    ObjectStore(ObjectFields::from_pairs(&[
        (benilla_protocol::field::FIELD_OBJECT_TYPE, 0x19),
        (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
        (target_field, target as u32),
        (target_field + 1, (target >> 32) as u32),
    ]))
}

/// A 40-man raid, every member held: we select `raid2`, who targets `raid3`.
fn raid() -> App {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let world = app.world_mut();
    let me = world.spawn((SelfPlayer, Guid(ME), player(0))).id();
    world.resource_mut::<GuidIndex>().0.insert(ME, me);
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    for i in 1..40 {
        let e = world.spawn((Guid(member(i)), player(member(i + 1)))).id();
        world.resource_mut::<GuidIndex>().0.insert(member(i), e);
        let mut group = world.resource_mut::<GroupState>();
        group.members.push(GroupMemberEntry {
            name: format!("M{i}"),
            guid: member(i),
            status: member_status::ONLINE,
            flags: 0,
            roles: 0,
        });
    }
    let mut group = world.resource_mut::<GroupState>();
    group.in_group = true;
    group.group_type = GROUPTYPE_RAID;
    let first = world.resource::<GuidIndex>().0[&member(1)];
    *world.resource_mut::<Selection>() = Selection {
        target: Some(first),
        guid: Some(member(1)),
        ..Default::default()
    };
    world.insert_non_send_resource(UiScript::new().expect("a VM"));
    app
}

/// `chunk`'s answer and the views it built.
fn cost(app: &mut App, chunk: &str) -> (Option<String>, u32) {
    VIEW_BUILDS.with(|n| n.set(0));
    let out = with_live_world(app.world_mut(), |s| {
        s.eval::<Option<String>>(chunk).expect("the chunk runs")
    })
    .expect("a VM");
    (out, VIEW_BUILDS.with(std::cell::Cell::get))
}

/// In a 40-man raid the token costs one view, the verb's own read or group test one more.
#[test]
fn a_token_read_in_a_full_raid_builds_a_few_views() {
    let mut app = raid();
    let (dead, builds) = cost(
        &mut app,
        "return UnitIsDeadOrGhost('raid40') and 'dead' or 'alive'",
    );
    assert_eq!(dead.as_deref(), Some("alive"));
    assert_eq!(builds, 2, "UnitIsDeadOrGhost('raid40') built {builds}");
    let (_, builds) = cost(&mut app, "return UnitInRange('party3') and 'near' or nil");
    assert_eq!(builds, 2, "UnitInRange('party3') built {builds}");
    let (guid, builds) = cost(&mut app, "return UnitGUID('targettarget')");
    assert_eq!(guid, Some(format!("0x{:016X}", member(2))));
    assert_eq!(builds, 1, "UnitGUID('targettarget') built {builds}");
    let (guid, builds) = cost(&mut app, "return UnitGUID('raid40')");
    assert_eq!(
        guid,
        Some(format!("0x{:016X}", member(39))),
        "the last row, by index"
    );
    assert_eq!(builds, 1, "UnitGUID('raid40') built {builds}");
}
