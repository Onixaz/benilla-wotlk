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
