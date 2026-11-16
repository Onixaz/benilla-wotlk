//! The live view and handler-fired events over a minimal app and a bare VM.

use benilla_protocol::field::FIELD_UNIT_FLAGS;
use benilla_protocol::messages::ObjectType;
use benilla_protocol::{ObjectFields, SessionEvent, SessionEventKind};

use benilla_ui::script::{ScriptValue, UiScript};
use bevy::prelude::*;

use super::live::UiEvents;
use crate::net::handlers::{dispatch, NetHandlerApp};
use crate::net::{GuidIndex, ObjectStore, SelfPlayer};

const ME: u64 = 0x10;
const TAXI: u32 = 1 << 20;
/// `OBJECT_FIELD_TYPE`: object, unit and player.
const PLAYER_TYPE: (u16, u32) = (2, 0x19);

/// Our player's flags word, as each delta sets it: the change goes through `Commands`, so it
/// lands only once the handler's commands apply.
fn on_values(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    mut commands: Commands,
    mut ui: ResMut<UiEvents>,
) {
    let SessionEvent::ObjectValues { guid, fields } = ev else {
        return;
    };
    let &e = index.0.get(&guid).expect("streamed");
    let flags = fields.unit_flags();
    let mut all = ObjectFields::from_pairs(&[PLAYER_TYPE]).into_created(ObjectType::Player);
    all.merge(fields);
    commands.entity(e).insert(ObjectStore(all));
    ui.fire("CHARZ_VALUES", vec![ScriptValue::Int(i64::from(flags))]);
}

/// The app with the handler, our player held, and a VM whose observer logs each event with
/// `UnitOnTaxi("player")` at that moment.
fn app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<UiEvents>()
        .net_handler(SessionEventKind::ObjectValues, on_values);
    let store = ObjectFields::from_pairs(&[PLAYER_TYPE]).into_created(ObjectType::Player);
    let me = app
        .world_mut()
        .spawn((crate::net::Guid(ME), ObjectStore(store), SelfPlayer))
        .id();
    app.insert_resource(GuidIndex([(ME, me)].into()));
    let mut script = UiScript::new().unwrap();
    script.set_unit_guids(&benilla_ui::script::UnitGuids {
        player: ME,
        held: [(ME, 0)].into(),
        ..Default::default()
    });
    script
        .run(
            r#"
            LOG = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("CHARZ_VALUES")
            f:SetScript("OnEvent", function(_, e, flags)
                LOG[#LOG + 1] = e .. " " .. flags .. " taxi=" .. tostring(UnitOnTaxi("player"))
            end)
            "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    app
}

fn values(flags: u32) -> SessionEvent {
    SessionEvent::ObjectValues {
        guid: ME,
        fields: ObjectFields::from_pairs(&[(FIELD_UNIT_FLAGS, flags)]),
    }
}

/// Each handler-fired event reaches Lua after its own packet's changes and before the next
/// packet's, and its handler's verb reads the `World`, not the pushed model.
#[test]
fn a_handler_event_fires_after_its_changes_and_before_the_next_packet() {
    let mut app = app();
    dispatch(app.world_mut(), vec![values(TAXI), values(0), values(TAXI)]);
    let log: Vec<String> = app
        .world()
        .non_send_resource::<UiScript>()
        .eval("return LOG")
        .unwrap();
    assert_eq!(
        log,
        vec![
            format!("CHARZ_VALUES {TAXI} taxi=1"),
            "CHARZ_VALUES 0 taxi=nil".to_string(),
            format!("CHARZ_VALUES {TAXI} taxi=1"),
        ]
    );
    assert!(app.world().resource::<UiEvents>().is_empty());
    // The loan ended with the call.
    assert!(!app.world().non_send_resource::<UiScript>().has_live());
}

/// With no VM a handler's events are dropped, not held for a later one.
#[test]
fn with_no_vm_the_events_are_dropped() {
    let mut app = app();
    let script = app
        .world_mut()
        .remove_non_send_resource::<UiScript>()
        .unwrap();
    dispatch(app.world_mut(), vec![values(TAXI)]);
    assert!(app.world().resource::<UiEvents>().is_empty());
    app.insert_non_send_resource(script);
    dispatch(app.world_mut(), vec![values(0)]);
    let log: Vec<String> = app
        .world()
        .non_send_resource::<UiScript>()
        .eval("return LOG")
        .unwrap();
    assert_eq!(log, vec!["CHARZ_VALUES 0 taxi=nil"]);
}
