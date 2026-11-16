//! A press, its script calls and what the frame publishes, run as the frame runs them: the real
//! systems, whole client headless, each frame in the order the real `Update` graph declares
//! between them. 12340 handles a press in its input pass (`0x480410`, from the scheduler at
//! `0x47f12d`) before the frame's world idle and the paint, whose layer update (`0x495810` →
//! `0x495320`) runs the `OnUpdate` scripts and the cooldown widgets; the GCD arm (`0x805d70`)
//! signals the cooldown update (`0x805ec3` → `0x53bac0`) in the same call.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use bevy::ecs::schedule::{ExecutorKind, Schedule, SystemKey};
use bevy::input::keyboard::{Key, KeyboardInput, NativeKey};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowResolution};
use crossbeam_channel::Receiver;

use benilla_formats::{SpellCatalog, SpellDisplay};
use benilla_protocol::messages::{
    ActionButton, GroupMemberEntry, PetActionEntry, ACTION_KIND_MACRO, ACTION_KIND_SPELL,
    PET_ACT_ENABLED,
};
use benilla_ui::script::{SpellBookState, SpellSlotView, SpellTabView, UiScript};

use crate::game_plugins::schedule_tests::{census, headless_client, SyncPoints};
use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfGuid, SelfPlayer};
use crate::target::Selection;

const ME: u64 = 0x10;
/// The unit selected when the frame starts.
const OLD: u64 = 0x21;
/// `party1`, the unit a click or a script selects.
const NEW: u64 = 0x22;
const PET: u64 = 0x2A;
/// A spell at the selection with the standard GCD (category 133, 1500 ms), on action slot 1.
const BOLT: u32 = 133;
/// A spell with a 30 s recovery of its own and no GCD, on action slot 2, running when a test
/// starts.
const NOVA: u32 = 122;
/// A pet spell with the pet's GCD, on the pet bar's slot 1.
const CLAW: u32 = 16829;
/// Water in the backpack's first slot when a test puts it there, and its on-use spell, which
/// arms the standard GCD.
const WATER: u32 = 159;
const DRINK: u32 = 430;
const WATER_GUID: u64 = 0x4000_0000_0000_0042;
/// The `GetTime()` the rigs' VM stands at; no frame moves it, as `Time<Real>` never steps here.
const NOW: f64 = 100.0;
/// How long the running recovery has run when a test starts.
const RAN: Duration = Duration::from_secs(10);

/// The real `Update` graph, by system name: who is declared to run before whom.
struct Graph {
    names: Vec<String>,
    succ: Vec<Vec<usize>>,
}

impl Graph {
    /// The one system whose name ends with `suffix`.
    fn one(&self, suffix: &str) -> usize {
        let hits: Vec<usize> = (0..self.names.len())
            .filter(|&i| self.names[i].ends_with(suffix))
            .collect();
        assert_eq!(hits.len(), 1, "exactly one system named `…{suffix}`");
        hits[0]
    }

    /// Whether a declared path runs `from` before `to`.
    fn reaches(&self, from: usize, to: usize) -> bool {
        let mut seen = vec![false; self.names.len()];
        let mut stack = vec![from];
        while let Some(n) = stack.pop() {
            for &m in &self.succ[n] {
                if m == to {
                    return true;
                }
                if !std::mem::replace(&mut seen[m], true) {
                    stack.push(m);
                }
            }
        }
        false
    }

    /// Whether the system named `a` runs before the one named `b`.
    fn before(&self, a: &str, b: &str) -> bool {
        self.reaches(self.one(a), self.one(b))
    }
}

/// The real graph, read once per test binary; `None` in a build without system names.
fn graph() -> Option<&'static Graph> {
    static GRAPH: OnceLock<Option<Graph>> = OnceLock::new();
    GRAPH
        .get_or_init(|| {
            let mut app = headless_client();
            let c = census(&mut app, Update, SyncPoints::Declared);
            if !c.systems.values().any(|s| s.name.contains("benilla_app::")) {
                return None;
            }
            let keys: Vec<SystemKey> = c.systems.keys().copied().collect();
            let index: HashMap<SystemKey, usize> =
                keys.iter().enumerate().map(|(i, k)| (*k, i)).collect();
            let mut succ = vec![Vec::new(); keys.len()];
            for (a, b) in &c.dependencies {
                if let (Some(&a), Some(&b)) = (index.get(a), index.get(b)) {
                    succ[a].push(b);
                }
            }
            Some(Graph {
                names: keys.iter().map(|k| c.systems[k].name.clone()).collect(),
                succ,
            })
        })
        .as_ref()
}

/// The real graph, or a skip line for a build that carries no system names.
macro_rules! graph_or_skip {
    () => {
        match graph() {
            Some(g) => g,
            None => {
                eprintln!("skipped: this build carries no system names");
                return;
            }
        }
    };
}

/// One real system a rig's frame runs, named by its path's tail in the real graph.
#[derive(bevy::ecs::schedule::SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Step {
    /// The pointer and key feed into the UI: a button's `OnClick`.
    UiInput,
    /// The binding table's derivation, then the key dispatch: a binding's body.
    BindingSync,
    BindingDispatch,
    /// The script calls in call order: where a press's cast goes out and its GCD is armed.
    ScriptCalls,
    /// The world click's select.
    WorldSelect,
    /// The action bar's state feed: `GetActionCooldown` and the cooldown events.
    ActionState,
    /// The spellbook's feed: `GetSpellCooldown`.
    SpellBook,
    /// The pet bar's feed and its cooldown flush: `GetPetActionCooldown`.
    PetBar,
    PetFlush,
    /// The VM's tick: the `OnUpdate` scripts and the cooldown widgets' update.
    Tick,
}

impl Step {
    fn suffix(self) -> &'static str {
        match self {
            Step::UiInput => "::input::feed_ui_input",
            Step::BindingSync => "::bindings::sync_dispatch",
            Step::BindingDispatch => "::bindings::latch_and_dispatch",
            Step::ScriptCalls => "::script_calls::apply_script_calls",
            Step::WorldSelect => "::click::select_on_click",
            Step::ActionState => "::state::feed_action_state",
            Step::SpellBook => "::ui_spellbook::feed_spellbook",
            Step::PetBar => "::bar::feed_pet_bar",
            Step::PetFlush => "::bar::fire_pet_cooldown_events",
            Step::Tick => "::extract::tick_script",
        }
    }

    fn add(self, schedule: &mut Schedule) {
        match self {
            Step::UiInput => schedule.add_systems(crate::ui_script::feed_ui_input.in_set(self)),
            Step::BindingSync => schedule.add_systems(crate::bindings::sync_dispatch.in_set(self)),
            Step::BindingDispatch => {
                schedule.add_systems(crate::bindings::latch_and_dispatch.in_set(self))
            }
            Step::ScriptCalls => schedule.add_systems(super::apply_script_calls.in_set(self)),
            Step::WorldSelect => {
                schedule.add_systems(crate::target::click::select_on_click.in_set(self))
            }
            Step::ActionState => {
                schedule.add_systems(crate::ui_action::feed_action_state.in_set(self))
            }
            Step::SpellBook => {
                schedule.add_systems(crate::ui_spellbook::feed_spellbook.in_set(self))
            }
            Step::PetBar => schedule.add_systems(crate::ui_pet::feed_pet_bar.in_set(self)),
            Step::PetFlush => {
                schedule.add_systems(crate::ui_pet::fire_pet_cooldown_events.in_set(self))
            }
            Step::Tick => schedule.add_systems(crate::ui_script::extract::tick_script.in_set(self)),
        };
    }
}

/// `steps` in the real graph's order; every pair must be ordered there, or the frame is not one
/// the client runs.
fn real_order(g: &Graph, steps: &[Step]) -> Vec<Step> {
    let mut order: Vec<Step> = Vec::new();
    for &step in steps {
        let at = order
            .iter()
            .position(|&o| g.before(step.suffix(), o.suffix()))
            .unwrap_or(order.len());
        order.insert(at, step);
    }
    for (i, &a) in order.iter().enumerate() {
        for &b in &order[i + 1..] {
            assert!(
                g.before(a.suffix(), b.suffix()),
                "the real graph does not order {a:?} before {b:?}"
            );
        }
    }
    order
}

/// The stock binding's shape: a body that calls whatever the test put in `PROBE_PRESS`, on `J`.
const PRESS_BINDING: &str =
    r#"<Bindings><Binding name="PROBEPRESS">PROBE_PRESS()</Binding></Bindings>"#;

/// The instrument: every cooldown event and every `OnUpdate`, stamped with the frame, and the two
/// slots' cooldown frames redrawn on `ACTIONBAR_UPDATE_COOLDOWN` the way the stock
/// `ActionButton_UpdateCooldown` does it, through `CooldownFrame_SetTimer` (`Cooldown.lua`). An
/// armed `TICK_CAST` is cast by name from the next `OnUpdate`.
const INSTRUMENT: &str = r#"
    FRAME = 0
    LOG = {}
    local function note(what) LOG[#LOG + 1] = FRAME .. " " .. what end
    for i = 1, 2 do
        local cd = CreateFrame("Cooldown", "ProbeCooldown" .. i)
        cd:SetWidth(36) cd:SetHeight(36)
        cd:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100 * i, 100)
        cd:Hide()
    end
    local function set_timer(cd, start, duration, enable)
        if start > 0 and duration > 0 and enable > 0 then
            cd:SetCooldown(start, duration)
        else
            cd:Hide()
        end
    end
    local f = CreateFrame("Frame", "ProbeWatch")
    for _, e in ipairs({ "ACTIONBAR_UPDATE_COOLDOWN", "SPELL_UPDATE_COOLDOWN",
                         "PET_BAR_UPDATE_COOLDOWN" }) do
        f:RegisterEvent(e)
    end
    f:SetScript("OnEvent", function(self, event)
        note(event)
        if event == "ACTIONBAR_UPDATE_COOLDOWN" then
            for i = 1, 2 do set_timer(_G["ProbeCooldown" .. i], GetActionCooldown(i)) end
        end
    end)
    f:SetScript("OnUpdate", function()
        note("OnUpdate action " .. (GetActionCooldown(1) > 0 and "swept" or "cold")
            .. " pet " .. (GetPetActionCooldown(1) > 0 and "swept" or "cold")
            .. " book " .. (GetSpellCooldown(BOLT_NAME) and GetSpellCooldown(BOLT_NAME) > 0
                and "swept" or "cold"))
        if TICK_CAST then
            local name = TICK_CAST
            TICK_CAST = nil
            CastSpellByName(name)
        end
    end)
    BOLT_NAME = "Bolt"
"#;

struct Rig {
    app: App,
    rx: Receiver<ClientCommand>,
    frame: Schedule,
    frames: u32,
}

impl Rig {
    /// The whole client headless: us, `OLD` selected, `NEW` as `party1`, Bolt and Nova on the
    /// first two action slots with Nova's recovery [`RAN`] in, our pet with Claw on its bar, a
    /// 1024×768 window, and a VM at [`NOW`] carrying the [`INSTRUMENT`] and the `J` binding. The
    /// frame runs `steps` in the real graph's order; `None` where the build has no system names.
    fn new(steps: &[Step]) -> Option<Rig> {
        let order = real_order(graph()?, steps);
        let mut app = headless_client();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let world = app.world_mut();
        let unit = |guid: u64, x: f32, pairs: &[(u16, u32)]| {
            let mut fields = vec![(22, 100), (28, 100)];
            fields.extend_from_slice(pairs);
            (
                Guid(guid),
                Transform::from_xyz(x, 0.0, 0.0),
                GlobalTransform::from_translation(Vec3::new(x, 0.0, 0.0)),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&fields)),
            )
        };
        // A player (`OBJECT_FIELD_TYPE` 0x19, `UNIT_FLAG_PVP_ATTACKABLE`): with no faction table
        // every plain unit is attackable to it.
        let me = world
            .spawn((SelfPlayer, unit(ME, 0.0, &[(2, 0x19), (46, 0x8)])))
            .id();
        let old = world.spawn(unit(OLD, 5.0, &[])).id();
        let new = world.spawn(unit(NEW, 6.0, &[])).id();
        // Our pet: `SUMMONEDBY` and `CREATEDBY` us.
        let pet = world
            .spawn((
                Guid(PET),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (12, ME as u32),
                    (13, 0),
                    (14, ME as u32),
                    (15, 0),
                ])),
            ))
            .id();
        let index = &mut world.resource_mut::<GuidIndex>().0;
        index.insert(ME, me);
        index.insert(OLD, old);
        index.insert(NEW, new);
        index.insert(PET, pet);
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        world
            .resource_mut::<crate::ui_party::GroupState>()
            .members
            .push(GroupMemberEntry {
                name: "Bob".into(),
                guid: NEW,
                status: 1,
                flags: 0,
                roles: 0,
            });
        *world.resource_mut::<Selection>() = Selection {
            target: Some(old),
            guid: Some(OLD),
            ..Default::default()
        };
        let gcd = |name: &str| SpellDisplay {
            name: name.into(),
            start_recovery_category: 133,
            start_recovery_ms: 1500,
            ..Default::default()
        };
        world.insert_resource(crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(
                [
                    (
                        BOLT,
                        SpellDisplay {
                            implicit_target_a1: 6,
                            ..gcd("Bolt")
                        },
                    ),
                    (
                        NOVA,
                        SpellDisplay {
                            name: "Nova".into(),
                            ..Default::default()
                        },
                    ),
                    (CLAW, gcd("Claw")),
                    (DRINK, gcd("Drink")),
                ]
                .into(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        {
            let mut actions = world.resource_mut::<crate::ui_action::PlayerActions>();
            for (slot, spell) in [(0, BOLT), (1, NOVA)] {
                actions.buttons.insert(
                    slot,
                    ActionButton {
                        slot,
                        action: spell,
                        kind: ACTION_KIND_SPELL,
                    },
                );
            }
            actions.spells = [BOLT, NOVA].into();
        }
        world
            .resource_mut::<crate::spell::Cooldowns>()
            .start_recovery(NOVA, 30_000, Instant::now() - RAN);
        {
            let mut bar = world.resource_mut::<crate::ui_pet::PetBar>();
            bar.spells.pet_guid = PET;
            bar.spells.bar[0] = PetActionEntry::from(CLAW | (u32::from(PET_ACT_ENABLED) << 24));
        }
        world.spawn((
            Window {
                resolution: WindowResolution::new(1024, 768),
                ..default()
            },
            PrimaryWindow,
        ));
        let mut script = UiScript::new().expect("a VM");
        script.set_now(NOW);
        script.set_spellbook(SpellBookState {
            tabs: vec![SpellTabView {
                name: "General".into(),
                texture: None,
                offset: 0,
                num_spells: 1,
            }],
            slots: vec![SpellSlotView {
                spell_id: BOLT,
                name: "Bolt".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        script.set_default_bindings(benilla_ui::script::keybind::parse_bindings_wtf(
            "bind J PROBEPRESS\r\n",
        ));
        script.load_binding_set(1);
        script.register_bindings(
            &benilla_ui::bindings_xml::parse(PRESS_BINDING).expect("well-formed"),
        );
        script.run(INSTRUMENT).expect("the instrument loads");
        world.insert_non_send_resource(script);
        world.insert_resource(crate::ui_script::UiClock {
            anchor: Instant::now(),
            ui_now: NOW,
        });
        let mut frame = Schedule::default();
        frame.set_executor_kind(ExecutorKind::SingleThreaded);
        for &step in &order {
            step.add(&mut frame);
        }
        for pair in order.windows(2) {
            frame.configure_sets(pair[0].before(pair[1]));
        }
        Some(Rig {
            app,
            rx,
            frame,
            frames: 0,
        })
    }

    fn vm(&mut self) -> Mut<'_, UiScript> {
        self.app.world_mut().non_send_resource_mut::<UiScript>()
    }

    fn lua(&mut self, chunk: &str) {
        self.vm().run(chunk).expect("the chunk runs");
    }

    /// One frame: the steps in order, then the input planes' frame edges cleared.
    fn frame(&mut self) {
        self.frames += 1;
        let n = self.frames;
        self.lua(&format!("FRAME = {n}"));
        self.frame.run(self.app.world_mut());
        let world = self.app.world_mut();
        world.resource_mut::<ButtonInput<KeyCode>>().clear();
        world.resource_mut::<ButtonInput<MouseButton>>().clear();
    }

    /// `J`, down or up, as the OS delivers it: the button plane and the message.
    fn key(&mut self, down: bool) {
        let world = self.app.world_mut();
        let mut keys = world.resource_mut::<ButtonInput<KeyCode>>();
        if down {
            keys.press(KeyCode::KeyJ);
        } else {
            keys.release(KeyCode::KeyJ);
        }
        world.write_message(KeyboardInput {
            key_code: KeyCode::KeyJ,
            logical_key: Key::Unidentified(NativeKey::Unidentified),
            state: if down {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            },
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    /// A full key press: down in one frame, up in the next.
    fn press(&mut self) {
        self.key(true);
        self.frame();
        self.key(false);
        self.frame();
    }

    /// The cast attempt's in-flight id and the GCD cleared, as a server's answer would: the next
    /// press is free to send.
    fn settle(&mut self) {
        let world = self.app.world_mut();
        world.insert_resource(crate::spell::PendingCast::default());
        world.resource_mut::<crate::spell::Cooldowns>().wipe();
    }

    /// A left click on the world at `NEW`, the press pick the click selects from.
    fn world_click(&mut self) {
        let world = self.app.world_mut();
        let new = world.resource::<GuidIndex>().0[&NEW];
        world.insert_resource(crate::target::PressPick {
            hovered: crate::target::Hovered {
                target: Some(new),
                guid: Some(NEW),
                distance: 6.0,
                ..Default::default()
            },
            ..Default::default()
        });
        world.write_message(benilla_world::interact::WorldClick);
    }

    /// The instrument's lines since the last read.
    fn log(&mut self) -> Vec<String> {
        let vm = self.vm();
        let log = vm.eval::<Vec<String>>("return LOG").expect("the log");
        vm.run("LOG = {}").expect("the log resets");
        log
    }

    /// The `CMSG_CAST_SPELL`s sent since the last read, as `(spell, target)`.
    fn casts(&self) -> Vec<(u32, Option<u64>)> {
        self.sent()
            .into_iter()
            .filter_map(|c| match c {
                ClientCommand::CastSpell { spell_id, target } => Some((spell_id, target)),
                _ => None,
            })
            .collect()
    }

    fn sent(&self) -> Vec<ClientCommand> {
        self.rx.try_iter().collect()
    }

    fn selected(&self) -> Option<u64> {
        self.app.world().resource::<Selection>().guid
    }

    /// Five waters in the backpack's first slot (`PLAYER_FIELD_PACK_SLOT_1`, field 532), with
    /// Drink as their on-use spell.
    fn water_in_the_backpack(&mut self) {
        let world = self.app.world_mut();
        let water = world
            .spawn((
                Guid(WATER_GUID),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (3, WATER),
                    (14, 5),
                ])),
            ))
            .id();
        let me = {
            let mut index = world.resource_mut::<GuidIndex>();
            index.0.insert(WATER_GUID, water);
            index.0[&ME]
        };
        world
            .entity_mut(me)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x19),
                (22, 100),
                (28, 100),
                (46, 0x8),
                (532, WATER_GUID as u32),
                (533, (WATER_GUID >> 32) as u32),
            ])));
        world.resource_mut::<crate::items::Items>().insert_template(
            WATER,
            Some(benilla_protocol::ItemInfo {
                spells: vec![benilla_protocol::messages::ItemSpellEntry {
                    index: 0,
                    spell_id: DRINK,
                    trigger: 0,
                    charges: -1,
                    cooldown_ms: -1,
                    category: 59,
                    category_cooldown_ms: 1000,
                }],
                use_spell: Some(benilla_protocol::messages::ItemUseSpell {
                    spell_id: DRINK,
                    cooldown_ms: -1,
                    category: 59,
                    category_cooldown_ms: 1000,
                }),
                ..crate::items::test_template("Water")
            }),
        );
    }
}

/// The rig, or a skip line for a build that carries no system names.
macro_rules! rig_or_skip {
    ($steps:expr) => {
        match Rig::new($steps) {
            Some(rig) => rig,
            None => {
                eprintln!("skipped: this build carries no system names");
                return;
            }
        }
    };
}

/// The frame's lines that `frame` stamped, without the stamp.
fn in_frame(log: &[String], frame: u32) -> Vec<String> {
    let stamp = format!("{frame} ");
    log.iter()
        .filter_map(|l| l.strip_prefix(&stamp).map(str::to_string))
        .collect()
}

// ── A: what a press already does, which no reorder of the frame may change ──────────────────────

/// A focused box's key never reaches its binding: the UI's key feed fills the capture gate the
/// dispatch reads, so it runs first.
#[test]
fn a_key_a_focused_box_ate_never_reaches_its_binding() {
    let g = graph_or_skip!();
    assert!(g.before(Step::UiInput.suffix(), Step::BindingDispatch.suffix()));
    assert!(g.before(Step::BindingSync.suffix(), Step::BindingDispatch.suffix()));
}

/// A click's and a key's script calls are applied in the frame the press is made.
#[test]
fn a_press_is_applied_in_the_frame_it_is_made() {
    let g = graph_or_skip!();
    for press in [Step::UiInput, Step::BindingDispatch] {
        assert!(
            g.before(press.suffix(), Step::ScriptCalls.suffix()),
            "{press:?} runs before the script calls apply"
        );
    }
}

/// What reads the script calls' work still follows them: the pet bar's and the pet book's drains,
/// the two item legs, and the rest of the target chain behind them.
#[test]
fn what_follows_the_script_calls_still_follows_them() {
    let g = graph_or_skip!();
    for after in [
        "::drain_pet_actions",
        "::drain_pet_book",
        "::drop_item_on_unit",
        "::drop_on_world_click",
        "::target_by_name_requests",
        "::assist_requests",
        "::follow_requests",
        "::auto_acquire_attacker",
        "::drive_flash",
        "::update_ring",
    ] {
        assert!(
            g.before(Step::ScriptCalls.suffix(), after),
            "{after} runs after the script calls"
        );
    }
}

/// The pointer arbiter reads the UI's hover after the input feed wrote it, and player control
/// reads the arbiter's answer: a press over the UI never also turns the camera or clicks the world.
#[test]
fn the_pointer_arbiter_sits_between_the_ui_input_and_player_control() {
    let g = graph_or_skip!();
    let arbiter = "::arbitrate_pointer_over_ui";
    assert!(g.before(Step::UiInput.suffix(), arbiter));
    assert!(g.before(arbiter, "::controller::control"));
}

/// The raw input stamp, which keeps the AFK clock, precedes the dispatch, so a key a box eats
/// still counts.
#[test]
fn raw_input_is_stamped_before_the_ui_input_feed() {
    let g = graph_or_skip!();
    assert!(g.before("::stamp_input", Step::UiInput.suffix()));
    assert!(g.before("::stamp_input", Step::BindingDispatch.suffix()));
}

/// The world clicks resolve against this frame's pick: player control settles the click, the press
/// latch and the hover pick follow it, and every click leg (the select, the cursor commits, Click
/// to Move's arrival, the right click and a crate's interaction) follows them.
#[test]
fn the_world_clicks_resolve_against_this_frames_pick() {
    let g = graph_or_skip!();
    let control = "::controller::control";
    for pick in ["::latch_press_pick", "::update_hover"] {
        assert!(g.before(control, pick), "{pick} follows player control");
        for leg in [
            Step::WorldSelect.suffix(),
            "::commit_ground_cast_on_click",
            "::commit_object_cast_on_click",
            "::act_on_arrival",
            "::act_on_right_click",
            "::act_on_interact",
        ] {
            assert!(g.before(pick, leg), "{leg} follows {pick}");
        }
    }
}

/// A bound key sends its cast once per press: never twice for the held key or the release, and a
/// second press after the first one settled sends again.
#[test]
fn a_bound_key_sends_one_cast_per_press() {
    let mut rig = rig_or_skip!(&[
        Step::BindingSync,
        Step::BindingDispatch,
        Step::ScriptCalls,
        Step::ActionState,
        Step::Tick,
    ]);
    rig.lua("function PROBE_PRESS() UseAction(1) end");
    rig.frame();
    rig.press();
    rig.frame();
    assert_eq!(rig.casts(), [(BOLT, Some(OLD))], "one cast for one press");
    rig.settle();
    rig.press();
    rig.frame();
    assert_eq!(rig.casts(), [(BOLT, Some(OLD))], "and one for the next");
}

/// A button's click sends its cast once: the `OnClick` the release fires, through the UI's own
/// pointer feed.
#[test]
fn a_button_click_sends_one_cast_per_press() {
    let mut rig = rig_or_skip!(&[Step::UiInput, Step::ScriptCalls, Step::Tick]);
    rig.lua(
        r#"local b = CreateFrame("Button", "ProbeButton")
        b:SetWidth(40) b:SetHeight(40)
        b:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 500, 300)
        b:SetScript("OnClick", function() UseAction(1) end)
        b:Show()"#,
    );
    // The button's centre at (520, 320) in the root, 768 high like the window: y flips.
    {
        let world = rig.app.world_mut();
        let mut windows = world.query_filtered::<&mut Window, With<PrimaryWindow>>();
        windows
            .single_mut(world)
            .expect("the window")
            .set_cursor_position(Some(Vec2::new(520.0, 768.0 - 320.0)));
    }
    rig.frame();
    rig.frame();
    rig.app
        .world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    rig.frame();
    rig.app
        .world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    rig.frame();
    rig.frame();
    assert_eq!(rig.casts(), [(BOLT, Some(OLD))], "one cast for one click");
}

/// Us on a taxi (`UNIT_FIELD_FLAGS` bit 20) and the test verb `CharzLiveTaxi` installed, which
/// reads that bit through the view lent to the call that runs it.
fn ride_and_install_live_taxi(rig: &mut Rig) {
    let world = rig.app.world_mut();
    let me = world.resource::<GuidIndex>().0[&ME];
    world
        .entity_mut(me)
        .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (2, 0x19),
            (22, 100),
            (28, 100),
            (benilla_protocol::field::FIELD_UNIT_FLAGS, 0x8 | 1 << 20),
        ])));
    crate::ui_script::live_tests::install_live_taxi(&rig.vm());
    rig.lua("LIVE = {}");
}

/// The lines the probes in [`ride_and_install_live_taxi`]'s tests left.
fn live_lines(rig: &mut Rig) -> Vec<String> {
    rig.vm().eval("return LIVE").expect("the probe lines")
}

/// A button's mouse handlers run inside the input pass with the `World` lent: a verb they call
/// reads the client's state, as 12340's verbs read the live object when called (`0x60f396`).
#[test]
fn a_click_handler_reads_the_world_through_the_input_pass() {
    let mut rig = rig_or_skip!(&[Step::UiInput, Step::Tick]);
    ride_and_install_live_taxi(&mut rig);
    rig.lua(
        r#"local b = CreateFrame("Button", "ProbeLiveButton")
        b:SetWidth(40) b:SetHeight(40)
        b:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 500, 300)
        b:SetScript("OnMouseDown", function() LIVE[#LIVE + 1] = "down " .. CharzLiveTaxi(0x10) end)
        b:SetScript("OnClick", function() LIVE[#LIVE + 1] = "click " .. CharzLiveTaxi(0x10) end)
        b:Show()"#,
    );
    {
        let world = rig.app.world_mut();
        let mut windows = world.query_filtered::<&mut Window, With<PrimaryWindow>>();
        windows
            .single_mut(world)
            .expect("the window")
            .set_cursor_position(Some(Vec2::new(520.0, 768.0 - 320.0)));
    }
    rig.frame();
    rig.frame();
    rig.app
        .world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    rig.frame();
    rig.app
        .world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    rig.frame();
    assert_eq!(
        live_lines(&mut rig),
        ["down 1", "click 1"],
        "each handler read the lent `World`"
    );
}

/// A `runOnUp` binding's body runs on the press and on the release with the `World` lent: a verb
/// it calls reads the client's state both times.
#[test]
fn a_binding_reads_the_world_on_its_press_and_its_release() {
    let mut rig = rig_or_skip!(&[Step::UiInput, Step::BindingSync, Step::BindingDispatch]);
    ride_and_install_live_taxi(&mut rig);
    rig.vm().register_addon_bindings(
        "ProbeAddon",
        &benilla_ui::bindings_xml::parse(
            r#"<Bindings><Binding name="PROBELIVE" runOnUp="true">
                LIVE[#LIVE + 1] = keystate .. " " .. CharzLiveTaxi(0x10)
            </Binding></Bindings>"#,
        )
        .expect("well-formed"),
    );
    rig.lua(r#"SetBinding("J", "PROBELIVE")"#);
    rig.frame();
    rig.press();
    assert_eq!(
        live_lines(&mut rig),
        ["down 1", "up 1"],
        "the press and the release each read the lent `World`"
    );
}

/// A world click and a cast the script makes after it in the same frame (an `OnUpdate`'s): the
/// cast goes to the unit the click selected, once.
#[test]
fn a_world_click_selects_before_a_cast_a_script_makes_after_it() {
    let mut rig = rig_or_skip!(&[Step::WorldSelect, Step::ScriptCalls, Step::Tick]);
    rig.frame();
    rig.world_click();
    rig.lua(r#"TICK_CAST = "Bolt""#);
    rig.frame();
    rig.frame();
    assert_eq!(rig.selected(), Some(NEW), "the click selects");
    assert_eq!(rig.casts(), [(BOLT, Some(NEW))], "and the cast follows it");
}

/// A bound key pressed the frame after a world click casts at the unit the click selected.
#[test]
fn a_key_pressed_after_a_world_click_casts_at_the_clicked_unit() {
    let mut rig = rig_or_skip!(&[
        Step::BindingSync,
        Step::BindingDispatch,
        Step::WorldSelect,
        Step::ScriptCalls,
    ]);
    rig.lua("function PROBE_PRESS() UseAction(1) end");
    rig.frame();
    rig.world_click();
    rig.frame();
    rig.press();
    rig.frame();
    assert_eq!(rig.selected(), Some(NEW));
    assert_eq!(rig.casts(), [(BOLT, Some(NEW))]);
}

/// `/target` then `/cast` in one macro, pressed through a binding: the cast goes to the macro's new
/// target, once.
#[test]
fn a_macros_target_then_cast_casts_at_the_new_target() {
    let mut rig = rig_or_skip!(&[Step::BindingSync, Step::BindingDispatch, Step::ScriptCalls]);
    // `ChatFrame1`'s `EXECUTE_CHAT_LINE` handler (`ChatFrame.lua:1343`) sends each line through the
    // edit box; this one runs it as Lua, which is all the order needs.
    rig.lua(
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("EXECUTE_CHAT_LINE")
        f:SetScript("OnEvent", function(self, event, arg1) RunScript(arg1) end)
        MACRO_INDEX = CreateMacro("probe", 1, 'TargetUnit("party1")\nCastSpellByName("Bolt")')
        function PROBE_PRESS() UseAction(3) end"#,
    );
    let index: u32 = rig.vm().eval("return MACRO_INDEX").expect("an index");
    rig.app
        .world_mut()
        .resource_mut::<crate::ui_action::PlayerActions>()
        .buttons
        .insert(
            2,
            ActionButton {
                slot: 2,
                action: index,
                kind: ACTION_KIND_MACRO,
            },
        );
    rig.frame();
    rig.press();
    rig.frame();
    assert_eq!(rig.selected(), Some(NEW));
    assert_eq!(rig.casts(), [(BOLT, Some(NEW))]);
}

/// A pet bar press reaches the pet: one `CMSG_PET_ACTION`, the pet's GCD on its list, and the bar
/// sweeping it by the frame after the press's at the latest.
#[test]
fn a_pet_bar_press_reaches_the_pet_and_its_bar() {
    let mut rig = rig_or_skip!(&[
        Step::BindingSync,
        Step::BindingDispatch,
        Step::ScriptCalls,
        Step::PetBar,
        Step::PetFlush,
        Step::Tick,
    ]);
    rig.lua("function PROBE_PRESS() CastPetAction(1) end");
    rig.frame();
    rig.key(true);
    rig.frame();
    rig.key(false);
    rig.frame();
    let pet_actions = rig
        .sent()
        .into_iter()
        .filter(|c| matches!(c, ClientCommand::PetAction { .. }))
        .count();
    assert_eq!(pet_actions, 1, "one send for one press");
    assert!(
        rig.app
            .world()
            .resource::<crate::ui_pet::PetBar>()
            .cooldowns
            .live_count()
            > 0,
        "the pet's GCD is on its list"
    );
    let log = rig.log();
    assert!(
        in_frame(&log, 3).contains(&"OnUpdate action cold pet swept book cold".to_string()),
        "the bar sweeps by the frame after the press: {log:?}"
    );
}

/// A bag item used through a binding goes out once per press, and the GCD its on-use spell arms
/// reaches the action bar by the frame after the press at the latest.
#[test]
fn a_bag_item_use_is_sent_once_and_its_gcd_reaches_the_bar() {
    let mut rig = rig_or_skip!(&[
        Step::BindingSync,
        Step::BindingDispatch,
        Step::ScriptCalls,
        Step::ActionState,
        Step::Tick,
    ]);
    rig.water_in_the_backpack();
    rig.lua("function PROBE_PRESS() UseContainerItem(0, 1) end");
    rig.frame();
    rig.press();
    rig.frame();
    let uses: Vec<u32> = rig
        .sent()
        .into_iter()
        .filter_map(|c| match c {
            ClientCommand::UseItem {
                spell_id,
                item_guid,
                ..
            } if item_guid == WATER_GUID => Some(spell_id),
            _ => None,
        })
        .collect();
    assert_eq!(uses, [DRINK], "one use for one press");
    let log = rig.log();
    assert!(
        in_frame(&log, 3).contains(&"OnUpdate action swept pet cold book cold".to_string()),
        "the GCD reaches the bar by the frame after the press: {log:?}"
    );
}

// ── B: #1215, the press before the frame's cooldown publication and its tick ───────────────────

/// Every feed that publishes cooldowns runs after the press and before the tick, so a GCD a press
/// arms reaches the frame's `OnUpdate` and its cooldown widgets: the action bar's, the spellbook's,
/// the bags', the pet bar's and book's, the runes', the forms' and the companions', and the pet's
/// flush.
#[test]
fn the_press_runs_before_every_cooldown_feed_and_each_feed_before_the_tick() {
    let g = graph_or_skip!();
    for feed in [
        Step::ActionState.suffix(),
        Step::SpellBook.suffix(),
        "::feed_containers",
        Step::PetBar.suffix(),
        "::feed_pet_book",
        "::feed_runes",
        "::feed_shapeshift_bar",
        "::feed_companions",
        Step::PetFlush.suffix(),
    ] {
        for press in [Step::UiInput, Step::BindingDispatch, Step::ScriptCalls] {
            assert!(
                g.before(press.suffix(), feed),
                "{press:?} runs before {feed}"
            );
        }
        assert!(
            g.before(feed, Step::Tick.suffix()),
            "{feed} runs before the tick"
        );
    }
}

/// A cast pressed in a frame shows its GCD in that frame: `ACTIONBAR_UPDATE_COOLDOWN` and
/// `SPELL_UPDATE_COOLDOWN` fire before the frame's `OnUpdate`, which reads the sweep from
/// `GetActionCooldown`.
#[test]
fn a_pressed_cast_shows_its_gcd_in_the_frame_of_the_press() {
    let mut rig = rig_or_skip!(&[
        Step::BindingSync,
        Step::BindingDispatch,
        Step::ScriptCalls,
        Step::ActionState,
        Step::Tick,
    ]);
    rig.lua("function PROBE_PRESS() UseAction(1) end");
    rig.frame();
    rig.log();
    rig.key(true);
    rig.frame();
    assert_eq!(rig.casts(), [(BOLT, Some(OLD))], "the press casts");
    let log = rig.log();
    assert_eq!(
        in_frame(&log, 2),
        [
            "ACTIONBAR_UPDATE_COOLDOWN",
            "SPELL_UPDATE_COOLDOWN",
            "OnUpdate action swept pet cold book cold",
        ],
        "{log:?}"
    );
}

/// The spellbook's sweep of the same press, in the same frame: `GetSpellCooldown` reads the GCD
/// from the frame's `OnUpdate`.
#[test]
fn a_pressed_cast_shows_its_gcd_on_the_spellbook_in_the_frame_of_the_press() {
    let mut rig = rig_or_skip!(&[
        Step::BindingSync,
        Step::BindingDispatch,
        Step::ScriptCalls,
        Step::SpellBook,
        Step::Tick,
    ]);
    rig.lua("function PROBE_PRESS() UseAction(1) end");
    rig.frame();
    rig.log();
    rig.key(true);
    rig.frame();
    let log = rig.log();
    assert_eq!(
        in_frame(&log, 2),
        ["OnUpdate action cold pet cold book swept"],
        "{log:?}"
    );
}

/// A pet bar press shows the pet's GCD in that frame: the flush fires before the frame's
/// `OnUpdate`, which reads the sweep from `GetPetActionCooldown`.
#[test]
fn a_pet_bar_press_shows_the_pets_gcd_in_the_frame_of_the_press() {
    let mut rig = rig_or_skip!(&[
        Step::BindingSync,
        Step::BindingDispatch,
        Step::ScriptCalls,
        Step::PetBar,
        Step::PetFlush,
        Step::Tick,
    ]);
    rig.lua("function PROBE_PRESS() CastPetAction(1) end");
    rig.frame();
    rig.log();
    rig.key(true);
    rig.frame();
    let log = rig.log();
    assert_eq!(
        in_frame(&log, 2),
        [
            "ACTIONBAR_UPDATE_COOLDOWN",
            "SPELL_UPDATE_COOLDOWN",
            "PET_BAR_UPDATE_COOLDOWN",
            "OnUpdate action cold pet swept book cold",
        ],
        "{log:?}"
    );
}

/// A running cooldown that the press frame's handler sets again (`SetCooldown`, `0x5ecd70`, zeroes
/// its elapsed time) is updated (`0x5ec8f0`) before the frame draws, so it draws at its real
/// elapsed time and never fully dark.
#[test]
fn a_running_cooldown_set_again_in_the_press_frame_draws_at_its_elapsed_time() {
    let mut rig = rig_or_skip!(&[
        Step::BindingSync,
        Step::BindingDispatch,
        Step::ScriptCalls,
        Step::ActionState,
        Step::Tick,
    ]);
    rig.lua("function PROBE_PRESS() UseAction(1) end");
    rig.frame();
    rig.log();
    rig.key(true);
    rig.frame();
    let log = rig.log();
    assert!(
        in_frame(&log, 2).contains(&"ACTIONBAR_UPDATE_COOLDOWN".to_string()),
        "the press frame's handler sets the running cooldown again: {log:?}"
    );
    let (start, duration): (f64, f64) = rig
        .vm()
        .eval("local s, d = GetActionCooldown(2) return s, d")
        .expect("Nova's triple");
    assert!(duration == 30.0, "Nova is running: {start} {duration}");
    let elapsed_ms = (NOW * 1000.0) as u32 - (start * 1000.0) as u32;
    let expected = benilla_ui::widget::CooldownState {
        start_ms: (start * 1000.0) as u32,
        duration_ms: 30_000,
        elapsed_ms,
        armed: true,
        ..Default::default()
    }
    .paint(1.0)
    .expect("an armed sweep paints");
    let vm = rig.vm();
    let drawn = vm
        .extract()
        .into_iter()
        .find_map(|q| match q.content {
            benilla_ui::script::QuadContent::Cooldown(paint)
                if vm.target_owner_name(q.target).as_deref() == Some("ProbeCooldown2") =>
            {
                Some(paint)
            }
            _ => None,
        })
        .expect("Nova's cooldown draws");
    assert!(
        elapsed_ms >= 9_000,
        "the elapsed time is the recovery's: {elapsed_ms}"
    );
    assert_eq!(drawn, expected, "drawn at {elapsed_ms} ms, not at 0");
}

/// A key and a world click landing in one frame: the press pass runs before player control settles
/// the click, so the key's cast goes to the selection the frame began with and the click selects
/// after it. 12340 takes the two in the order the OS queued them (`0x480410`).
#[test]
fn a_key_and_a_world_click_in_one_frame_cast_before_the_click_selects() {
    let mut rig = rig_or_skip!(&[
        Step::BindingSync,
        Step::BindingDispatch,
        Step::ScriptCalls,
        Step::WorldSelect,
    ]);
    rig.lua("function PROBE_PRESS() UseAction(1) end");
    rig.frame();
    rig.key(true);
    rig.world_click();
    rig.frame();
    assert_eq!(rig.casts(), [(BOLT, Some(OLD))]);
    assert_eq!(rig.selected(), Some(NEW));
}
