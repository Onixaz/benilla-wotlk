//! The control update's UI half: the flag and its events.

use super::*;
use crate::net::handlers::{dispatch, NetHandlerApp};
use benilla_ui::script::UiScript;

const ME: u64 = 0x7;
const CREATURE: u64 = 0xF130_000C_1A00_A2B4;

/// [`on_control_flag`] as this fixture registers it.
fn on_control(In(ev): In<SessionEvent>, world: &mut World) {
    assert!(matches!(ev, SessionEvent::ClientControl { .. }));
    world.run_system_cached_with(on_control_flag, ev).unwrap();
}

fn control(mover: u64, allow_move: bool) -> SessionEvent {
    SessionEvent::ClientControl { mover, allow_move }
}

/// Each event with `HasFullControl` as its handler reads it, the flag the view lends.
fn fired(app: &mut App, events: Vec<SessionEvent>) -> Vec<String> {
    dispatch(app.world_mut(), events);
    app.world()
        .non_send_resource::<UiScript>()
        .eval("local t = SEEN; SEEN = {}; return t")
        .unwrap()
}

/// The control update for our own unit writes the flag and fires on a change only, in wire
/// order with the flag already written (`0x520fe6`-`0x521021`); another unit's leaves both.
#[test]
fn our_own_control_update_writes_the_flag_and_fires_its_edge() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<ControlFlag>()
        .insert_resource(SelfGuid(Some(ME)))
        .net_handler(SessionEventKind::ClientControl, on_control);
    // Our body held, alive, for `HasFullControl`'s player test.
    let body = benilla_protocol::ObjectFields::from_pairs(&[
        (2, 0x19),
        (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
    ]);
    let me = app.world_mut().spawn(ObjectStore(body)).id();
    app.insert_resource(GuidIndex([(ME, me)].into()));
    let script = UiScript::new().unwrap();
    script
        .run(
            r#"
            SEEN = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("PLAYER_CONTROL_LOST")
            f:RegisterEvent("PLAYER_CONTROL_GAINED")
            f:SetScript("OnEvent", function(_, e)
                SEEN[#SEEN + 1] = e .. " " .. tostring(HasFullControl())
            end)
        "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);

    assert!(
        fired(&mut app, vec![control(ME, true)]).is_empty(),
        "boots in control"
    );
    assert_eq!(
        fired(
            &mut app,
            vec![control(ME, false), control(ME, false), control(ME, true)]
        ),
        vec!["PLAYER_CONTROL_LOST nil", "PLAYER_CONTROL_GAINED 1"]
    );
    assert!(
        fired(
            &mut app,
            vec![control(CREATURE, false), control(CREATURE, true)]
        )
        .is_empty(),
        "a unit we drive is not the active player"
    );
    assert!(app.world().resource::<ControlFlag>().0);
}

/// `HasFullControl` refuses while our `PLAYER_FARSIGHT` names a held object (`0x611622`), and
/// answers again once the field clears, the subject is not held, or an immediate clear ends it.
#[test]
fn far_sight_through_a_held_subject_takes_full_control() {
    use benilla_protocol::field::FIELD_PLAYER_FARSIGHT;
    const EYE: u64 = 0xF130_0000_0000_0001;
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<ControlFlag>()
        .init_resource::<crate::player::view_subject::FarSightClear>()
        .insert_resource(SelfGuid(Some(ME)));
    let me = app
        .world_mut()
        .spawn((
            crate::net::SelfPlayer,
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x19),
                (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
            ])),
        ))
        .id();
    let eye = app.world_mut().spawn_empty().id();
    app.insert_resource(GuidIndex([(ME, me)].into()));
    app.insert_non_send_resource(UiScript::new().unwrap());
    let look = |app: &mut App, subject: u64| {
        let world = app.world_mut();
        let mut store = world.get_mut::<ObjectStore>(me).unwrap();
        store.0.merge(benilla_protocol::ObjectFields::from_pairs(&[
            (FIELD_PLAYER_FARSIGHT, subject as u32),
            (FIELD_PLAYER_FARSIGHT + 1, (subject >> 32) as u32),
        ]));
    };
    let ask = |app: &mut App| {
        crate::ui_script::live::with_live_world(app.world_mut(), |s| {
            s.eval::<String>("return tostring(HasFullControl())")
                .unwrap()
        })
        .unwrap()
    };

    assert_eq!(ask(&mut app), "1");
    look(&mut app, EYE);
    assert_eq!(ask(&mut app), "1", "a subject not held sets no bit");
    app.world_mut()
        .resource_mut::<GuidIndex>()
        .0
        .insert(EYE, eye);
    assert_eq!(ask(&mut app), "nil");
    app.world_mut()
        .run_system_cached_with(
            crate::player::view_subject::on_clear_far_sight,
            SessionEvent::ClearFarSight,
        )
        .unwrap();
    assert_eq!(ask(&mut app), "1", "the immediate clear ends it");
    app.world_mut()
        .run_system_cached_with(
            crate::player::view_subject::on_far_sight_field,
            SessionEvent::ObjectValues {
                guid: ME,
                fields: benilla_protocol::ObjectFields::from_pairs(&[(FIELD_PLAYER_FARSIGHT, 0)]),
            },
        )
        .unwrap();
    look(&mut app, 0);
    assert_eq!(ask(&mut app), "1");
    look(&mut app, EYE);
    assert_eq!(ask(&mut app), "nil", "a fresh set looks again");
}

/// Our own create after a control loss gives control back with `PLAYER_CONTROL_GAINED`
/// (`0x6e80b6` -> `0x520fe0`); a create while in control, or another player's, fires nothing.
#[test]
fn our_own_create_regains_control_on_the_flags_edge() {
    use benilla_protocol::messages::ObjectType;
    use benilla_protocol::{EntityKind, ObjectFields};
    const OTHER: u64 = 0x8;
    const NEXT: u64 = 0x9;
    let create = |guid| SessionEvent::ObjectCreate {
        guid,
        kind: EntityKind::Player,
        display_id: None,
        position: [0.0; 3],
        orientation: 0.0,
        scale: 1.0,
        speeds: None,
        mover: None,
        transport_progress: None,
        transport: None,
        spline: None,
        vehicle: None,
        attack_target: None,
        spawned: false,
        fields: ObjectFields::from_pairs(&[(2, 0x19)]).into_created(ObjectType::Player),
    };
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let script = UiScript::new().unwrap();
    script
        .run(
            r#"
            SEEN = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("PLAYER_CONTROL_GAINED")
            f:SetScript("OnEvent", function(_, e) SEEN[#SEEN + 1] = e end)
        "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    let world = app.world_mut();
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    let seen = |world: &mut World, ev| -> Vec<String> {
        dispatch(world, vec![ev]);
        world
            .non_send_resource::<UiScript>()
            .eval("local t = SEEN; SEEN = {}; return t")
            .unwrap()
    };

    world.resource_mut::<ControlFlag>().0 = false;
    assert!(
        seen(world, create(OTHER)).is_empty(),
        "not the active player"
    );
    assert!(!world.resource::<ControlFlag>().0);
    assert_eq!(seen(world, create(ME)), vec!["PLAYER_CONTROL_GAINED"]);
    assert!(world.resource::<ControlFlag>().0);

    // In control already: the flag holds and nothing fires.
    world.resource_mut::<SelfGuid>().0 = Some(NEXT);
    assert!(seen(world, create(NEXT)).is_empty());
    assert!(world.resource::<ControlFlag>().0);
}

/// A game start gives control back (`0x52b8a0`), so a session that ended with control lost fires
/// no `PLAYER_CONTROL_GAINED` at the next character's own create.
#[test]
fn a_game_start_regains_control_silently() {
    use benilla_protocol::messages::ObjectType;
    use benilla_protocol::{EntityKind, ObjectFields};
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let script = UiScript::new().unwrap();
    script
        .run(
            r#"
            SEEN = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("PLAYER_CONTROL_GAINED")
            f:SetScript("OnEvent", function(_, e) SEEN[#SEEN + 1] = e end)
        "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    let world = app.world_mut();
    world.resource_mut::<ControlFlag>().0 = false;
    dispatch(
        world,
        vec![
            SessionEvent::Connected {
                self_guid: ME,
                name: "Me".into(),
                billing_time_rested: 0,
                expansion: 2,
                tutorial_flags: None,
                addon_info: None,
            },
            SessionEvent::ObjectCreate {
                guid: ME,
                kind: EntityKind::Player,
                display_id: None,
                position: [0.0; 3],
                orientation: 0.0,
                scale: 1.0,
                speeds: None,
                mover: None,
                transport_progress: None,
                transport: None,
                spline: None,
                vehicle: None,
                attack_target: None,
                spawned: false,
                fields: ObjectFields::from_pairs(&[(2, 0x19)]).into_created(ObjectType::Player),
            },
        ],
    );
    assert!(world.resource::<ControlFlag>().0);
    let seen: Vec<String> = world
        .non_send_resource::<UiScript>()
        .eval("return SEEN")
        .unwrap();
    assert!(seen.is_empty(), "{seen:?}");
}
