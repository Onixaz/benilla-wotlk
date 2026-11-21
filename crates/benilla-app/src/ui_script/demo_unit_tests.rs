//! A server-less capture's synthetic units read through the token resolver with the `World` lent
//! and no selection, hover or self player in it: the stand-in tables name the snapshots' guids.

use benilla_ui::script::{UiScript, UnitState};
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;

/// The whole client, headless, with a bare VM and nothing selected or hovered.
fn client() -> App {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.world_mut()
        .insert_non_send_resource(UiScript::new().expect("a VM"));
    app
}

fn eval(app: &mut App, chunk: &'static str) -> String {
    super::safe_world::lend(app.world_mut(), |s| {
        s.eval::<String>(chunk).expect("the chunk runs")
    })
    .expect("a VM")
}

#[test]
fn the_demo_player_and_target_read_with_the_world_lent() {
    let mut app = client();
    app.world_mut()
        .run_system_once(super::demo_unit_feed)
        .expect("the feed runs");
    assert_eq!(
        eval(
            &mut app,
            "return tostring(UnitName('player'))..'|'..tostring(UnitName('target'))..'|'\
             ..tostring(UnitIsUnit('playertarget', 'target'))"
        ),
        "Benilla|Young Wolf|1"
    );
}

#[test]
fn the_demo_player_keeps_a_spawned_self_players_guid() {
    let mut app = client();
    let world = app.world_mut();
    world.spawn((crate::net::SelfPlayer, crate::net::Guid(0x77)));
    world
        .run_system_once(super::demo_unit_feed)
        .expect("the feed runs");
    assert_eq!(
        eval(&mut app, "return UnitGUID('player')"),
        "0x0000000000000077"
    );
    assert_eq!(
        eval(&mut app, "return tostring(UnitName('player'))"),
        "Benilla"
    );
}

#[test]
fn a_tooltip_fixtures_mouseover_reads_with_the_world_lent() {
    let mut app = client();
    let world = app.world_mut();
    world
        .run_system_once(super::demo_unit_feed)
        .expect("the feed runs");
    super::safe_world::lend(world, |s| {
        s.set_unit(
            "mouseover",
            Some(UnitState {
                exists: true,
                name: Some("Stormwind Guard".into()),
                guid: super::DEMO_MOUSEOVER_GUID,
                ..Default::default()
            }),
        );
    });
    assert_eq!(
        eval(
            &mut app,
            "return tostring(UnitExists('mouseover'))..'|'..tostring(UnitName('mouseover'))"
        ),
        "1|Stormwind Guard"
    );
}
