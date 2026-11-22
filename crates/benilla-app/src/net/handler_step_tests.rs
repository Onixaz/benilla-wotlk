//! A handler's raise at its step: a listener reads the state as of that step and its own writes reach the rest of the handler, as 12340's handler runs the listeners inside it (`0x703e50`).

use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::{dispatch, step, NetHandlerApp};
use crate::net::{ControlFlag, GuidIndex, ObjectStore, SelfGuid};
use crate::ui_script::live::UiEvents;

const ME: u64 = 0x7;

/// What the handler's last step read.
#[derive(Resource, Default)]
struct Read(Vec<String>);

/// A client with our body held, the control flag and the bank window, and `chunk` loaded.
fn client(chunk: &str) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<ControlFlag>()
        .init_resource::<Read>()
        .insert_non_send_resource(crate::ui_bank::BankOpen { banker: Some(0x99) })
        .insert_resource(SelfGuid(Some(ME)));
    let body = benilla_protocol::ObjectFields::from_pairs(&[
        (benilla_protocol::field::FIELD_OBJECT_TYPE, 0x19),
        (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
    ]);
    let me = app.world_mut().spawn(ObjectStore(body)).id();
    app.insert_resource(GuidIndex([(ME, me)].into()));
    let script = UiScript::new().unwrap();
    script.run(chunk).unwrap();
    app.insert_non_send_resource(script);
    app.world_mut().resource_mut::<ControlFlag>().0 = true;
    app
}

/// The control loss's write and its event, the handler's first step.
fn lose(mut flag: ResMut<ControlFlag>, mut ui: ResMut<UiEvents>) {
    flag.0 = false;
    ui.fire("PLAYER_CONTROL_LOST", Vec::new());
}

/// A later write of the same handler.
fn regain(mut flag: ResMut<ControlFlag>) {
    flag.0 = true;
}

/// The handler's last step: it reads the bank window a listener may have closed.
fn read_bank(bank: NonSend<crate::ui_bank::BankOpen>, mut read: ResMut<Read>) {
    read.0.push(format!("banker={:?}", bank.banker));
}

/// Raise, then write, as two steps.
fn on_split(In(ev): In<SessionEvent>, world: &mut World) {
    assert!(matches!(ev, SessionEvent::LoggedOut));
    step(world, lose, ());
    step(world, regain, ());
    step(world, read_bank, ());
}

/// Raise, then write, in one system: its raise fires once it returns.
fn on_whole(
    In(ev): In<SessionEvent>,
    mut flag: ResMut<ControlFlag>,
    mut ui: ResMut<UiEvents>,
    bank: NonSend<crate::ui_bank::BankOpen>,
    mut read: ResMut<Read>,
) {
    assert!(matches!(ev, SessionEvent::LoggedOut));
    flag.0 = false;
    ui.fire("PLAYER_CONTROL_LOST", Vec::new());
    flag.0 = true;
    read.0.push(format!("banker={:?}", bank.banker));
}

const LISTENER: &str = r#"
    SEEN = {}
    local f = CreateFrame("Frame")
    f:RegisterEvent("PLAYER_CONTROL_LOST")
    f:SetScript("OnEvent", function(_, e)
        SEEN[#SEEN + 1] = e .. " " .. tostring(HasFullControl())
        CloseBankFrame()
    end)
"#;

fn seen(app: &App) -> String {
    app.world()
        .non_send_resource::<UiScript>()
        .eval("return table.concat(SEEN, ',')")
        .unwrap()
}

/// A listener on an event raised mid-handler reads the flag as that step left it, not as the handler's later write leaves it; on the queue it read the later write.
#[test]
fn a_listener_on_a_mid_handler_event_reads_the_state_at_its_step() {
    let mut app = client(LISTENER);
    app.net_handler(SessionEventKind::LoggedOut, on_split);
    dispatch(app.world_mut(), vec![SessionEvent::LoggedOut]);
    assert_eq!(seen(&app), "PLAYER_CONTROL_LOST nil");
    assert!(
        app.world().resource::<ControlFlag>().0,
        "the later step ran"
    );

    let mut app = client(LISTENER);
    app.net_handler(SessionEventKind::LoggedOut, on_whole);
    dispatch(app.world_mut(), vec![SessionEvent::LoggedOut]);
    assert_eq!(
        seen(&app),
        "PLAYER_CONTROL_LOST 1",
        "the queue fires after the handler"
    );
}

/// A listener's write, the bank window `CloseBankFrame` closes, is what the handler's next step reads; on the queue the handler had read past it.
#[test]
fn a_listeners_write_reaches_the_rest_of_the_handler() {
    let mut app = client(LISTENER);
    app.net_handler(SessionEventKind::LoggedOut, on_split);
    dispatch(app.world_mut(), vec![SessionEvent::LoggedOut]);
    assert_eq!(app.world().resource::<Read>().0, ["banker=None"]);
    assert_eq!(
        app.world()
            .non_send_resource::<crate::ui_bank::BankOpen>()
            .banker,
        None
    );

    let mut app = client(LISTENER);
    app.net_handler(SessionEventKind::LoggedOut, on_whole);
    dispatch(app.world_mut(), vec![SessionEvent::LoggedOut]);
    assert_eq!(app.world().resource::<Read>().0, ["banker=Some(153)"]);
}

/// What a handler queued before a step, through `in_turn`, fires ahead of that step's own raise.
#[test]
fn a_step_fires_what_the_handler_queued_before_it_first() {
    fn on_queued_then_step(In(ev): In<SessionEvent>, world: &mut World) {
        assert!(matches!(ev, SessionEvent::LoggedOut));
        world
            .resource_mut::<UiEvents>()
            .fire("PLAYER_CONTROL_GAINED", Vec::new());
        step(world, lose, ());
    }
    let mut app = client(
        r#"SEEN = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("PLAYER_CONTROL_LOST")
        f:RegisterEvent("PLAYER_CONTROL_GAINED")
        f:SetScript("OnEvent", function(_, e) SEEN[#SEEN + 1] = e end)"#,
    );
    app.net_handler(SessionEventKind::LoggedOut, on_queued_then_step);
    dispatch(app.world_mut(), vec![SessionEvent::LoggedOut]);
    assert_eq!(seen(&app), "PLAYER_CONTROL_GAINED,PLAYER_CONTROL_LOST");
}

/// A step nested in a handler's world step hands its system lines on after the lines the handler queued before it, in wire and step order.
#[test]
fn a_nested_steps_lines_follow_the_handlers_earlier_lines() {
    fn later(mut ui: ResMut<UiEvents>) {
        ui.system_line("LINE_B", Vec::new());
    }
    fn on_lines(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
        assert!(matches!(ev, SessionEvent::LoggedOut));
        ui.system_line("LINE_A", Vec::new());
        ui.world(|world| {
            step(world, later, ());
        });
    }
    let mut app = client(r#"LINE_A = "first" LINE_B = "second""#);
    app.init_resource::<crate::ui_chat::ChatLog>();
    app.net_handler(SessionEventKind::LoggedOut, on_lines);
    dispatch(app.world_mut(), vec![SessionEvent::LoggedOut]);
    assert_eq!(
        app.world()
            .resource::<crate::ui_chat::ChatLog>()
            .pending_lines(),
        ["first", "second"]
    );
}

/// A step that raised both in turn on the VM and on the handler queue loses their order: a hard failure in a debug build.
#[test]
#[cfg_attr(debug_assertions, should_panic(expected = "their order is lost"))]
fn a_step_never_raises_on_both_lists() {
    fn both(mut script: NonSendMut<UiScript>, mut ui: ResMut<UiEvents>) {
        script.fire_event("PLAYER_CONTROL_LOST", Vec::new());
        ui.fire("PLAYER_CONTROL_GAINED", Vec::new());
    }
    let mut app = client(LISTENER);
    app.init_resource::<UiEvents>();
    step(app.world_mut(), both, ());
}
