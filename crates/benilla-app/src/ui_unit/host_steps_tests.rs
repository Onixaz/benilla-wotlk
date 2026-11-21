//! A typed line's token reads with no unit fixture: each token resolves against the `World` the
//! drain lends at the line (`0x60abf0`), never against an empty table.

use benilla_protocol::messages::GroupMemberEntry;
use benilla_protocol::ObjectFields;
use benilla_ui::script::{DuelRequest, UiScript};
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfGuid, SelfPlayer};
use crate::target::Selection;
use crate::ui_party::GroupState;

const ME: u64 = 0x10;
/// A player we target.
const FOE: u64 = 0x21;
/// `party1`.
const MATE: u64 = 0x22;

struct Client {
    app: App,
    rx: Receiver<ClientCommand>,
}

/// The whole client, headless: us, `FOE` selected and `MATE` as `party1`, all players, and a VM
/// with no unit fixture.
fn client() -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    let player = |guid: u64| {
        (
            Guid(guid),
            Transform::default(),
            GlobalTransform::default(),
            ObjectStore(ObjectFields::from_pairs(&[(
                benilla_protocol::field::FIELD_OBJECT_TYPE,
                0x19,
            )])),
        )
    };
    let me = world.spawn((SelfPlayer, player(ME))).id();
    let foe = world.spawn(player(FOE)).id();
    let mate = world.spawn(player(MATE)).id();
    let index = &mut world.resource_mut::<GuidIndex>().0;
    index.insert(ME, me);
    index.insert(FOE, foe);
    index.insert(MATE, mate);
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    world
        .resource_mut::<GroupState>()
        .members
        .push(GroupMemberEntry {
            name: "Bob".into(),
            guid: MATE,
            status: 1,
            flags: 0,
            roles: 0,
        });
    *world.resource_mut::<Selection>() = Selection {
        target: Some(foe),
        guid: Some(FOE),
        ..Default::default()
    };
    world.insert_non_send_resource(UiScript::new().expect("a VM"));
    Client { app, rx }
}

/// Type `lines` through the chat drain, with `/duel` and `/wave` (`WAVE`, 101) in the table.
fn type_lines(c: &mut Client, lines: &[&str]) {
    let table = crate::ui_chat::commands::SlashCommands::build(
        |key| match key {
            "SLASH_DUEL1" => Some("/duel".into()),
            "EMOTE1_CMD1" => Some("/wave".into()),
            "EMOTE1_TOKEN" => Some("WAVE".into()),
            _ => None,
        },
        |token| (token == "WAVE").then_some(101),
    );
    let world = c.app.world_mut();
    // The chat arm's probes read the point under us.
    benilla_world::world_point::init_world_point_resources(world);
    world.insert_resource(table);
    for line in lines {
        world
            .non_send_resource_mut::<UiScript>()
            .push_chat_input((*line).into());
    }
    world
        .run_system_once(crate::ui_chat::drain_chat_input)
        .expect("the drain runs as a one-shot system");
}

/// `/duel` reads the target and `/duel party1` the party slot at the line (`0x60abf0`).
#[test]
fn a_typed_duel_resolves_its_token_against_the_world() {
    let mut c = client();
    type_lines(&mut c, &["/duel", "/duel party1"]);
    assert_eq!(
        c.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .take_duel_requests(),
        vec![
            DuelRequest::ChallengeGuid(FOE),
            DuelRequest::ChallengeGuid(MATE)
        ]
    );
}

/// `/wave party1` emotes at the party slot's unit (`0x520190`), and a bare `/wave` at the
/// selection.
#[test]
fn a_typed_emote_at_a_token_targets_the_unit_it_names() {
    let mut c = client();
    type_lines(&mut c, &["/wave party1", "/wave"]);
    let targets: Vec<u64> =
        c.rx.try_iter()
            .filter_map(|cmd| match cmd {
                ClientCommand::TextEmote {
                    text_id: 101,
                    target,
                    ..
                } => Some(target),
                _ => None,
            })
            .collect();
    assert_eq!(targets, [MATE, FOE]);
}
