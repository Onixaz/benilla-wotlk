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

/// Installs `CharzLiveTaxi(guid)`, a test verb reading the unit's taxi bit through the view lent to
/// the call that runs it: `"1"` or `"nil"`, and `"unlent"` with no view lent.
pub(crate) fn install_live_taxi(script: &UiScript) {
    let f = script
        .lua()
        .create_function(|_, guid: u64| {
            Ok(
                benilla_ui::script::read_live(|l| l.unit_flags(guid)).map_or(
                    "unlent",
                    |f| match f {
                        Some(f) if f & TAXI != 0 => "1",
                        _ => "nil",
                    },
                ),
            )
        })
        .unwrap();
    script.lua().globals().set("CharzLiveTaxi", f).unwrap();
}

/// The app with the handler, our player held, and a VM whose observer logs each event with our
/// taxi bit read through the lent view at that moment.
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
    install_live_taxi(&script);
    script
        .run(
            r#"
            LOG = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("CHARZ_VALUES")
            f:SetScript("OnEvent", function(_, e, flags)
                LOG[#LOG + 1] = e .. " " .. flags .. " taxi=" .. CharzLiveTaxi(0x10)
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
/// packet's, and its handler's verb reads the `World` through the lent view.
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

/// A call into Lua that unwinds puts the VM and the view's state back, and ends the loan.
#[test]
fn an_unwinding_call_puts_the_vm_and_the_view_back() {
    let mut app = app();
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        super::live::with_live_world(app.world_mut(), |_| panic!("a call that unwinds"))
    }));
    assert!(unwound.is_err());
    let world = app.world_mut();
    assert!(world.contains_resource::<super::live::LiveViewState>());
    let script = world.non_send_resource::<UiScript>();
    assert!(!script.has_live());
    assert_eq!(
        super::live::with_live_world(world, |s| s
            .eval::<String>("return CharzLiveTaxi(0x10)")
            .unwrap()),
        Some("nil".to_string()),
        "the next call lends again"
    );
}

/// Whether a parameter list takes the VM by value, the way a system does (a helper borrowing a
/// system's `&mut Option<NonSendMut<UiScript>>` does not count).
fn takes_vm(params: &str) -> bool {
    let flat: String = params.split_whitespace().collect();
    flat.match_indices("UiScript>").any(|(at, _)| {
        let head = &flat[..at];
        let ty = &head[head.rfind(':').map_or(0, |c| c + 1)..];
        let ty = ty.trim_start_matches('(');
        !ty.starts_with('&') && ty.contains("NonSend")
    })
}

/// Holders that call no Lua, argued: (file, item, why).
const CALLS_NO_LUA: &[(&str, &str, &str)] = &[(
    "spell/net.rs",
    "SpellGoLoot",
    "reads the modified-click table on the Rust side, and its handler writes the object stores      the view would read",
)];

/// Whether `CALLS_NO_LUA` argues `item` in `rel`.
fn argued(rel: &str, item: &str) -> bool {
    CALLS_NO_LUA
        .iter()
        .any(|&(f, i, _)| f == rel && item.contains(i))
}

/// Every system parameter list or `SystemParam` struct that holds the VM also takes
/// [`super::LendLive`], so every call into Lua from it reads the `World`: scheduled systems, packet
/// handlers and observers alike. Read off the source, as no schedule lists handlers.
#[test]
fn every_holder_of_the_vm_lends_the_world() {
    use crate::test_support::{fn_items, rel_path, rust_files, src_dir};
    // An inline test module, wherever it sits; `mod tests;` is only a declaration.
    let test_module =
        regex::Regex::new(r"\r?\n#\[cfg\(test\)\]\r?\n(?:#\[[^\n]*\]\r?\n)*mod \w+ \{")
            .expect("a valid pattern");
    let mut checked = 0;
    let mut offenders = Vec::new();
    for file in rust_files(&src_dir()) {
        let rel = rel_path(&file);
        if rel.ends_with("tests.rs") || rel.starts_with("ui_script/live") {
            continue;
        }
        let source = std::fs::read_to_string(&file).expect("readable source");
        // Production code only: every inline test module cut out, to its closing brace.
        let mut text = String::new();
        let mut from = 0;
        while let Some(m) = test_module.find_at(&source, from) {
            text.push_str(&source[from..m.start()]);
            let mut depth = 0usize;
            from = source.len();
            for (i, c) in source[m.end() - 1..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => continue,
                }
                if depth == 0 {
                    from = m.end() + i;
                    break;
                }
            }
        }
        text.push_str(&source[from..]);
        let text = text.as_str();
        if !text.contains("UiScript") {
            continue;
        }
        for f in fn_items(text) {
            if takes_vm(f.params) {
                checked += 1;
                if !f.params.contains("LendLive") {
                    offenders.push(format!("{rel}: fn {}", f.name));
                }
            }
        }
        for (at, _) in text.match_indices("#[derive(SystemParam)]") {
            let rest = &text[at..];
            let (Some(open), Some(close)) = (rest.find('{'), rest.find("\n}")) else {
                continue;
            };
            let body = &rest[open..close];
            let name = rest[..open].trim();
            if takes_vm(body) {
                checked += 1;
                if !body.contains("LendLive") && !argued(&rel, name) {
                    offenders.push(format!("{rel}: {name}"));
                }
            }
        }
    }
    assert!(checked > 200, "the scan found only {checked} holders");
    assert!(
        offenders.is_empty(),
        "these hold the VM and lend nothing, so a verb they reach reads no client state; take \
         `crate::ui_script::LendLive` beside the VM:\n{}",
        offenders.join("\n")
    );
}

/// The same off the built schedules, which also sees a VM held inside a tuple alias: every
/// non-exclusive system whose access holds the VM reads [`super::live::LendsLive`], the mark only
/// [`super::live::LiveView`] reads.
#[test]
fn every_scheduled_holder_of_the_vm_lends_the_world() {
    use std::any::TypeId;
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let world = app.world_mut();
    let labels: Vec<_> = world
        .resource::<bevy::ecs::schedule::Schedules>()
        .iter()
        .map(|(_, schedule)| schedule.label())
        .collect();
    let mut holders = 0;
    let mut offenders = Vec::new();
    for label in labels {
        world.schedule_scope(label, |world, schedule| {
            schedule.graph_mut().systems.initialize(world);
            let components = world.components();
            let (Some(vm), Some(mark)) = (
                components.get_resource_id(TypeId::of::<UiScript>()),
                components.get_resource_id(TypeId::of::<super::live::LendsLive>()),
            ) else {
                return;
            };
            let graph = schedule.graph();
            for (key, system, _) in graph.systems.iter() {
                if system.is_exclusive() {
                    continue;
                }
                let Some(with_access) = graph.systems.get(key) else {
                    continue;
                };
                let access = with_access.access.combined_access();
                if access.has_resource_read(vm) || access.has_resource_write(vm) {
                    holders += 1;
                    if !access.has_resource_read(mark) {
                        offenders.push(format!("{label:?}: {}", system.name()));
                    }
                }
            }
        });
    }
    assert!(holders > 200, "the census found only {holders} holders");
    assert!(
        offenders.is_empty(),
        "these scheduled systems hold the VM and lend nothing:\n{}",
        offenders.join("\n")
    );
}
