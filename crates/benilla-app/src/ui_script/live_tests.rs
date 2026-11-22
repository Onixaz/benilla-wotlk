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

/// A call into Lua that unwinds leaves the VM in the `World` and ends the loan.
#[test]
fn an_unwinding_call_puts_the_vm_and_the_view_back() {
    let mut app = app();
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        super::live::with_live_world(app.world_mut(), |_| panic!("a call that unwinds"))
    }));
    assert!(unwound.is_err());
    let world = app.world_mut();
    assert!(!benilla_ui::script::world_loan::lent());
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
    // `NonSend[Mut]<UiScript>`, its path written out or not; a reference to one is a helper's.
    flat.match_indices("UiScript>").any(|(at, _)| {
        let head = &flat[..at];
        let Some(open) = head.rfind('<') else {
            return false;
        };
        let path = &head[open + 1..];
        // A lifetime first: `NonSendMut<'w, UiScript>`.
        let path = match path.strip_prefix('\'') {
            Some(rest) => rest.split_once(',').map_or("", |(_, p)| p),
            None => path,
        };
        let wrapper = &head[..open];
        let ty = &wrapper[wrapper.rfind([':', ',', '(']).map_or(0, |c| c + 1)..];
        path.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == ':')
            && (ty.ends_with("NonSend") || ty.ends_with("NonSendMut"))
            && !ty.starts_with('&')
    })
}

/// Holders that call no Lua, argued: (file, item, why).
const CALLS_NO_LUA: &[(&str, &str, &str)] = &[
    (
        "ui_lfg.rs",
        "show_step",
        "reads the money line's global strings off the VM",
    ),
    (
        "target/click.rs",
        "act_on_right_click",
        "reads and takes the cursor's sale off the model; its talks run as steps with the `World`",
    ),
    (
        "target/click.rs",
        "act_on_interact",
        "reads and takes the cursor's sale off the model; its talks run as steps with the `World`",
    ),
    (
        "target/click.rs",
        "act_on_arrival",
        "reads and takes the cursor's sale off the model; its talks run as steps with the `World`",
    ),
    (
        "target/by_name.rs",
        "follow_requests",
        "reads the focus and the names a token resolves through off the model",
    ),
    (
        "death/mod.rs",
        "death_action",
        "one death intent as a press, its raises queued in turn; `press_step` fires them with the          `World` lent",
    ),
    (
        "spell/targeting/item.rs",
        "item_leg",
        "one item-pick leg as a press, its raises queued in turn; `press_step` fires them with the          `World` lent",
    ),
    (
        "ui_items/drain.rs",
        "direct_use_step",
        "the glyph arm's direct use as a press, its raises queued in turn; `press_step` fires them          with the `World` lent",
    ),
    (
        "ui_items/mod.rs",
        "run_item_use_rest",
        "the item use's rest past its form leave, a cast edge's step: the bind deferral queues its event on the VM, no call into Lua; its edges fire with the `World` lent",
    ),
    (
        "ui_action/drain.rs",
        "go_opener",
        "one GameObject opener as a press, its raises queued in turn; `press_step` fires them with          the `World` lent",
    ),
    (
        "ui_action/drop_item.rs",
        "feed_leg",
        "reads the cursor payload off the model; `press_step` fires the press's raises with the          `World` lent",
    ),
    (
        "script_calls.rs",
        "apply_one",
        "applies one script call with its raises queued in turn; its caller fires them with the \
         `World` lent",
    ),
    (
        "spell/net.rs",
        "SpellGoLoot",
        "reads the modified-click table on the Rust side, and its handler writes the object stores      the view would read",
    ),
    (
        "ui_unit/tokens.rs",
        "UnitVm",
        "reads the focus and the name lookup off the VM's model for the app's resolver, and calls no Lua",
    ),
    (
        "ui_unit.rs",
        "TargetSetTail",
        "the selection setter's writes into the VM's model; its raises run after it with the `World` lent",
    ),
    (
        "ui_summon.rs",
        "drain_summon",
        "takes the answers `ConfirmSummon` and `CancelSummon` queued, a field drain",
    ),
    (
        "ui_tabard.rs",
        "sync_tabard_booth",
        "reads the designer's shown flag and the pane's facing off the model",
    ),
    (
        "ui_net.rs",
        "feed_net_stats",
        "writes the latency field `GetNetStats` reads",
    ),
    (
        "ui_layout.rs",
        "load_layout",
        "writes the placed frames' anchors and sizes into the model",
    ),
    (
        "ui_quest_share.rs",
        "drain_quest_share",
        "takes the confirms `ConfirmAcceptQuest` queued, a field drain",
    ),
    (
        "ui_socket.rs",
        "drain_item_sends",
        "takes the socket and refund sends the verbs queued, field drains",
    ),
    (
        "ui_battlefield_score.rs",
        "drain_battlefield_score",
        "takes the score requests `RequestBattlefieldScoreData` queued, a field drain",
    ),
    (
        "ui_follow.rs",
        "drain_follow",
        "takes the follow requests the verbs queued, a field drain",
    ),
    (
        "ui_merchant/mod.rs",
        "end_repair_mode_on_right_press",
        "clears the repair-mode field",
    ),
    (
        "combat_log/producers.rs",
        "LogCtx",
        "reads the combat-text unit and the focus off the model",
    ),
    (
        "minimap/mod.rs",
        "emit_minimap",
        "takes the ping click and reads the town filter off the model",
    ),
    (
        "ui_dressup.rs",
        "sync_dressup_booth",
        "reads the shown pane and its facing off the model",
    ),
    (
        "ui_stable/mod.rs",
        "feed_stable_booth",
        "reads the pane's facing and the stable selection off the model",
    ),
    (
        "ui_loot/net.rs",
        "on_money_notify",
        "reads the money line's global strings off the VM",
    ),
    (
        "ui_macro/mod.rs",
        "rebind_macro_spells",
        "reads the macro bodies and writes their bound spells into the model",
    ),
    (
        "combat_log/mod.rs",
        "log_deaths",
        "reads the combat-text unit and the focus off the model through `LogCtx`",
    ),
    (
        "combat_log/mod.rs",
        "log_missile_misses",
        "reads the combat-text unit and the focus off the model through `LogCtx`",
    ),
    (
        "combat_log/mod.rs",
        "log_cast_failures",
        "reads the combat-text unit and the focus off the model through `LogCtx`",
    ),
    (
        "cursor.rs",
        "drive",
        "reads the cursor payload and mode off the model and takes its cursor writes",
    ),
    (
        "cursor.rs",
        "drive_displayed_cursor",
        "reads the cursor mode off the model and takes its cursor writes",
    ),
    (
        "cursor/other.rs",
        "drive",
        "the inline module's cursor driver: reads the payload's icon off the model",
    ),
    (
        "cursor/macos.rs",
        "drive",
        "the inline module's cursor driver: reads the payload's icon off the model",
    ),
    (
        "ui_script/extract/mod.rs",
        "paint_measure",
        "measures the strings written since the tick; the resolve runs with the `World` lent",
    ),
    (
        "ui_script/extract/mod.rs",
        "paint_remeasure",
        "measures the strings the resolve rewrote",
    ),
    (
        "ui_script/extract/mod.rs",
        "paint_extract",
        "walks the resolved frames into the draw list",
    ),
];

/// Whether `CALLS_NO_LUA` argues the scheduled system `name` (`crate::module::item`).
fn argued_system(name: &str) -> bool {
    CALLS_NO_LUA.iter().any(|&(file, item, _)| {
        let module = file
            .trim_end_matches(".rs")
            .trim_end_matches("/mod")
            .replace('/', "::");
        name.ends_with(&format!("{module}::{item}"))
    })
}

/// Whether `CALLS_NO_LUA` argues `item` in `rel`.
fn argued(rel: &str, item: &str) -> bool {
    CALLS_NO_LUA
        .iter()
        .any(|&(f, i, _)| f == rel && item.contains(i))
}

/// A system that calls into Lua is exclusive and lends the `World` ([`super::safe_world`]), so no
/// parameter list or `SystemParam` struct holds the VM but one argued in [`CALLS_NO_LUA`] or the
/// raise queue's own. Read off the source, as no schedule lists handlers.
#[test]
fn every_holder_of_the_vm_lends_the_world() {
    use crate::test_support::{fn_items, rel_path, rust_files, src_dir};
    // An inline test module, wherever it sits; `mod tests;` is only a declaration.
    let test_module =
        regex::Regex::new(r"\r?\n#\[cfg\(test\)\]\r?\n(?:#\[[^\n]*\]\r?\n)*mod \w+ \{")
            .expect("a valid pattern");
    let mut offenders = Vec::new();
    for file in rust_files(&src_dir()) {
        let rel = rel_path(&file);
        // The raise queue's own param holds the VM to queue the raises it opens.
        if rel.ends_with("tests.rs")
            || rel.starts_with("ui_script/live")
            || rel == "ui_script/raises.rs"
        {
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
            if takes_vm(f.params) && !argued(&rel, f.name) {
                offenders.push(format!("{rel}: fn {}", f.name));
            }
        }
        for (at, _) in text.match_indices("#[derive(SystemParam)]") {
            let rest = &text[at..];
            let (Some(open), Some(close)) = (rest.find('{'), rest.find("\n}")) else {
                continue;
            };
            let body = &rest[open..close];
            let name = rest[..open].trim();
            if takes_vm(body) && !argued(&rel, name) {
                offenders.push(format!("{rel}: {name}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these hold the VM with no view and are not argued in `CALLS_NO_LUA`; a system that calls \
         into Lua is exclusive and lends the `World` (`safe_world::lend`):\n{}",
        offenders.join("\n")
    );
}

/// No borrowed view is left anywhere in the app or the VM, tests included: every call into Lua
/// reaches the state through the `World` (or a test's view) lent to it for the call alone
/// ([`super::safe_world`]), never a view, a write handle, a read handle or a standing loan.
#[test]
fn no_borrowed_view_is_left_in_the_tree() {
    use crate::test_support::{rel_path, rust_files, src_dir};
    let names = [
        ["Lend", "Live"].concat(),
        ["Owned", "Mut"].concat(),
        ["Lent", "Ref"].concat(),
        ["Owns", "ScriptCalls"].concat(),
        ["Live", "Loan"].concat(),
        ["push", "_view"].concat(),
        ["pop", "_view"].concat(),
    ];
    let word =
        regex::Regex::new(&format!(r"\b(?:{})\b", names.join("|"))).expect("a valid pattern");
    let vm_src = src_dir().join("../../benilla-ui/src");
    let mut offenders = Vec::new();
    let mut scanned = 0;
    for file in rust_files(&src_dir())
        .into_iter()
        .chain(rust_files(&vm_src))
    {
        let source = std::fs::read_to_string(&file).expect("readable source");
        scanned += usize::from(file.starts_with(&vm_src));
        for (n, line) in source.lines().enumerate() {
            if word.is_match(line) {
                offenders.push(format!("{}:{}: {}", rel_path(&file), n + 1, line.trim()));
            }
        }
    }
    assert!(scanned > 0, "the VM's sources are read");
    assert!(
        offenders.is_empty(),
        "a borrowed view is back; lend the `World` instead:\n{}",
        offenders.join("\n")
    );
}

/// The same off the built schedules, which also sees a VM held inside a tuple alias: every
/// non-exclusive system whose access holds the VM raises in turn ([`super::raises::RaisesInTurn`])
/// or is argued in [`CALLS_NO_LUA`].
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
            let Some(vm) = components.get_resource_id(TypeId::of::<UiScript>()) else {
                return;
            };
            let in_turn = components.get_resource_id(TypeId::of::<super::raises::RaisesInTurn>());
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
                    let raises_in_turn = in_turn.is_some_and(|t| access.has_resource_read(t));
                    if !raises_in_turn && !argued_system(&system.name()) {
                        offenders.push(format!("{label:?}: {}", system.name()));
                    }
                }
            }
        });
    }
    // A floor that proves the census reads the schedule; the #1245 parts shrink the count.
    assert!(holders > 150, "the census found only {holders} holders");
    assert!(
        offenders.is_empty(),
        "these scheduled systems hold the VM and lend nothing:\n{}",
        offenders.join("\n")
    );
}

/// Every scheduled [`super::raises::InTurn`] holder has a raise step right after it: it sits in a
/// UI phase a step follows ([`super::raises::phases`]), or is ordered directly before a step of
/// its own schedule. Otherwise its raises wait for whichever step or handler fires next.
#[test]
fn every_scheduled_holder_in_turn_has_its_raise_step() {
    use bevy::ecs::schedule::NodeId;
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
            let Some(mark) = world
                .components()
                .get_resource_id(TypeId::of::<super::raises::RaisesInTurn>())
            else {
                return;
            };
            let graph = schedule.graph();
            let phases: Vec<NodeId> = super::raises::phases()
                .into_iter()
                .filter_map(|set| graph.system_sets.get_key(set).map(NodeId::Set))
                .collect();
            let steps: Vec<NodeId> = graph
                .systems
                .iter()
                .filter(|(_, system, _)| system.name().to_string().contains("raises::fire_raises"))
                .map(|(key, _, _)| NodeId::System(key))
                .collect();
            let hierarchy = graph.hierarchy().graph();
            // The sets a node sits in, directly or through a parent set.
            let in_phase = |node: NodeId| {
                let mut stack = vec![node];
                let mut seen = std::collections::HashSet::new();
                while let Some(n) = stack.pop() {
                    if phases.contains(&n) {
                        return true;
                    }
                    if seen.insert(n) {
                        stack.extend(hierarchy.neighbors_directed(
                            n,
                            bevy::ecs::schedule::graph::Direction::Incoming,
                        ));
                    }
                }
                false
            };
            let dependency = graph.dependency().graph();
            let in_turn: Vec<(NodeId, String)> = graph
                .systems
                .iter()
                .filter(|(key, _, _)| {
                    graph
                        .systems
                        .get(*key)
                        .is_some_and(|s| s.access.combined_access().has_resource_read(mark))
                })
                .map(|(key, system, _)| (NodeId::System(key), system.name().to_string()))
                .collect();
            holders += in_turn.len();
            // Covered: in a phase, or ordered directly before a step or a covered holder in turn,
            // which runs no Lua between.
            let mut covered: Vec<NodeId> = Vec::new();
            loop {
                let before = covered.len();
                for (node, _) in &in_turn {
                    if covered.contains(node) {
                        continue;
                    }
                    let next = dependency
                        .neighbors_directed(*node, bevy::ecs::schedule::graph::Direction::Outgoing)
                        .any(|n| steps.contains(&n) || covered.contains(&n));
                    if in_phase(*node) || next {
                        covered.push(*node);
                    }
                }
                if covered.len() == before {
                    break;
                }
            }
            offenders.extend(
                in_turn
                    .iter()
                    .filter(|(node, _)| !covered.contains(node))
                    .map(|(_, name)| format!("{label:?}: {name}")),
            );
        });
    }
    assert!(
        holders > 100,
        "the census found only {holders} holders in turn"
    );
    assert!(
        offenders.is_empty(),
        "these hold the VM in turn with no raise step after them; put them in a UI phase or chain \
         `raises::then_fire` after them:\n{}",
        offenders.join("\n")
    );
}

/// A unique signal waits once however often it is queued (`0x6143f0`), in its first queueing's place, and the flush fires the queue in that order and empties it (`0x614760`).
#[test]
fn a_unique_signal_waits_once_and_the_flush_fires_the_queue_in_order() {
    use super::live::{flush_unique_signals, UniqueSignals};
    let mut app = App::new();
    app.init_non_send_resource::<UniqueSignals>()
        .add_systems(Update, flush_unique_signals);
    let s = UiScript::new().unwrap();
    s.run(
        "SEEN = {} local f = CreateFrame('Frame') \
         f:RegisterEvent('QUEST_POI_UPDATE') f:RegisterEvent('QUEST_LOG_UPDATE') \
         f:SetScript('OnEvent', function(_, e, ...) SEEN[#SEEN + 1] = e .. select('#', ...) end)",
    )
    .unwrap();
    app.insert_non_send_resource(s);
    {
        let mut q = app.world_mut().non_send_resource_mut::<UniqueSignals>();
        q.unique_signal("QUEST_POI_UPDATE");
        q.unique_signal("QUEST_LOG_UPDATE");
        q.unique_signal("QUEST_POI_UPDATE");
        assert_eq!(q.names(), ["QUEST_POI_UPDATE", "QUEST_LOG_UPDATE"]);
    }
    app.update();
    assert!(app
        .world()
        .non_send_resource::<UniqueSignals>()
        .names()
        .is_empty());
    let seen = app
        .world()
        .non_send_resource::<UiScript>()
        .eval::<String>("return table.concat(SEEN, ' ')")
        .unwrap();
    assert_eq!(seen, "QUEST_POI_UPDATE0 QUEST_LOG_UPDATE0");
}

/// A signal queued while the flush fires matches the entry in flight (`0x6143f0`) or is dropped when the count is zeroed (`0x6147a3`): a `QUEST_LOG_UPDATE` handler that folds a header fires its event once, and nothing waits for the next frame.
#[test]
fn a_signal_queued_during_the_flush_is_dropped() {
    use super::live::{flush_unique_signals, UniqueSignals};
    let mut app = App::new();
    app.init_non_send_resource::<UniqueSignals>()
        .add_systems(Update, flush_unique_signals);
    let s = UiScript::new().unwrap();
    s.run(
        "SEEN = {} local f = CreateFrame('Frame') f:RegisterEvent('QUEST_LOG_UPDATE') \
         f:SetScript('OnEvent', function(_, e) SEEN[#SEEN + 1] = e \
           ExpandQuestHeader(0) CollapseQuestHeader(0) end)",
    )
    .unwrap();
    app.insert_non_send_resource(s);
    let seen = |app: &App| {
        app.world()
            .non_send_resource::<UiScript>()
            .eval::<String>("local t = table.concat(SEEN, ' ') SEEN = {} return t")
            .unwrap()
    };
    app.world_mut()
        .non_send_resource_mut::<UniqueSignals>()
        .unique_signal("QUEST_LOG_UPDATE");
    app.update();
    assert_eq!(seen(&app), "QUEST_LOG_UPDATE", "one fire for the frame");
    assert!(
        app.world()
            .non_send_resource::<UniqueSignals>()
            .names()
            .is_empty(),
        "the handler's re-queue went with the count"
    );
    app.update();
    assert_eq!(seen(&app), "", "nothing fires the next frame");
}

/// A verb's unique signal joins the handlers' queue through the lent view, so the frame fires the log's update once however many sites queue it (`0x5e0c7c`, `0x5e6cb9`, `0x614760`); the game UI's shutdown at a disconnect empties the queue (`0x52902f`), so nothing fires at the next login.
#[test]
fn a_verbs_unique_signal_shares_the_queue_and_a_disconnect_empties_it() {
    use super::live::{clear_unique_signals, flush_unique_signals, UniqueSignals};
    let mut app = App::new();
    app.init_non_send_resource::<UniqueSignals>()
        .net_handler(SessionEventKind::Disconnected, clear_unique_signals)
        .add_systems(Update, flush_unique_signals);
    let s = UiScript::new().unwrap();
    s.run(
        "SEEN = {} local f = CreateFrame('Frame') f:RegisterEvent('QUEST_LOG_UPDATE') \
         f:SetScript('OnEvent', function(_, e) SEEN[#SEEN + 1] = e end)",
    )
    .unwrap();
    app.insert_non_send_resource(s);
    let seen = |app: &App| {
        app.world()
            .non_send_resource::<UiScript>()
            .eval::<String>("local t = table.concat(SEEN, ' ') SEEN = {} return t")
            .unwrap()
    };
    super::live::with_live_world(app.world_mut(), |s| s.run("ExpandQuestHeader(0)").unwrap());
    assert_eq!(
        app.world().non_send_resource::<UniqueSignals>().names(),
        ["QUEST_LOG_UPDATE"],
        "the verb's signal waits in the handlers' queue"
    );
    app.world_mut()
        .non_send_resource_mut::<UniqueSignals>()
        .unique_signal("QUEST_LOG_UPDATE");
    app.update();
    assert_eq!(seen(&app), "QUEST_LOG_UPDATE");

    super::live::with_live_world(app.world_mut(), |s| {
        s.run("CollapseQuestHeader(0)").unwrap()
    });
    dispatch(
        app.world_mut(),
        vec![SessionEvent::Disconnected {
            reason: String::new(),
            end: benilla_protocol::SessionEnd::LoggedOut,
        }],
    );
    app.update();
    assert_eq!(seen(&app), "", "the shutdown emptied the queue");
}

/// A verb's write through the `World` lent to its call lands in the world's state and shows as a
/// change to the next system; with nothing lent the write finds no state.
#[test]
fn a_verbs_write_through_the_world_loan_lands_and_shows_as_a_change() {
    use super::live::UniqueSignals;
    use benilla_ui::script::write_live;

    #[derive(Resource, Default)]
    struct Changed(Vec<bool>);

    fn verb(world: &mut World) {
        assert_eq!(write_live(|l| l.unique_signal("OUTSIDE")), None);
        let wrote = super::live::with_live_world(world, |_| {
            write_live(|l| l.unique_signal("VERB")) == Some(true)
        });
        assert_eq!(wrote, Some(true));
    }

    fn reader(signals: NonSend<UniqueSignals>, mut changed: ResMut<Changed>) {
        changed.0.push(signals.is_changed());
    }

    let mut app = App::new();
    app.init_non_send_resource::<UniqueSignals>()
        .init_resource::<Changed>()
        .add_systems(Update, (verb, reader).chain());
    app.insert_non_send_resource(UiScript::new().unwrap());
    app.update();
    assert_eq!(
        app.world().non_send_resource::<UniqueSignals>().names(),
        ["VERB"]
    );
    assert_eq!(app.world().resource::<Changed>().0, [true]);
}
