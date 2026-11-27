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
    /// The spellbook's feed: `GetSpellCooldown`.
    /// The pet bar's feed and its cooldown flush: `GetPetActionCooldown`.
    /// The VM's tick: the `OnUpdate` scripts and the cooldown widgets' update.
    Tick,
    /// The units' feed: the `"target"` snapshot and `PLAYER_TARGET_CHANGED` for a selection a
    /// script did not move.
    UnitFeed,
}

impl Step {
    fn suffix(self) -> &'static str {
        match self {
            Step::UiInput => "::input::feed_ui_input",
            Step::BindingSync => "::bindings::sync_dispatch",
            Step::BindingDispatch => "::bindings::latch_and_dispatch",
            Step::ScriptCalls => "::script_calls::apply_script_calls",
            Step::WorldSelect => "::click::select_on_click",
            Step::Tick => "::extract::tick_script",
            Step::UnitFeed => "::ui_unit::feed_units",
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
            Step::Tick => schedule.add_systems(crate::ui_script::extract::tick_script.in_set(self)),
            Step::UnitFeed => {
                crate::ui_unit::feed_units_in(schedule, self);
                schedule
            }
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
    PROBE_START = {}
    local function set_timer(cd, start, duration, enable)
        PROBE_START[cd:GetName()] = start
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
            let mut fields = vec![
                (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
            ];
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
            .spawn((
                SelfPlayer,
                unit(
                    ME,
                    0.0,
                    &[
                        (benilla_protocol::field::FIELD_OBJECT_TYPE, 0x19),
                        (benilla_protocol::field::FIELD_UNIT_FLAGS, 0x8),
                    ],
                ),
            ))
            .id();
        let old = world.spawn(unit(OLD, 5.0, &[])).id();
        let new = world.spawn(unit(NEW, 6.0, &[])).id();
        // Our pet: `SUMMONEDBY` and `CREATEDBY` us.
        let pet = world
            .spawn((
                Guid(PET),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (benilla_protocol::field::FIELD_UNIT_SUMMONEDBY, ME as u32),
                    (benilla_protocol::field::FIELD_UNIT_SUMMONEDBY + 1, 0),
                    (benilla_protocol::field::FIELD_UNIT_CREATEDBY, ME as u32),
                    (benilla_protocol::field::FIELD_UNIT_CREATEDBY + 1, 0),
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
            let mut bar = world.resource_mut::<crate::ui_action::ActionSlots>();
            for (slot, spell) in [(0, BOLT), (1, NOVA)] {
                bar.buttons.insert(
                    slot,
                    ActionButton {
                        slot,
                        action: spell,
                        kind: ACTION_KIND_SPELL,
                    },
                );
            }
            world
                .resource_mut::<crate::ui_action::PlayerActions>()
                .spells = [BOLT, NOVA].into();
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
        world.insert_resource(crate::ui_spellbook::SpellBook::of(SpellBookState {
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
        }));
        script.set_default_bindings(benilla_ui::script::keybind::parse_bindings_wtf(
            "bind J PROBEPRESS\r\n",
        ));
        script.load_binding_set(1);
        script.register_bindings(
            &benilla_ui::bindings_xml::parse(PRESS_BINDING).expect("well-formed"),
        );
        script.run(INSTRUMENT).expect("the instrument loads");
        world.insert_non_send_resource(script);
        world.insert_resource(crate::ui_script::UiClock { ui_now: NOW });
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
                ClientCommand::CastSpell {
                    spell_id, target, ..
                } => Some((spell_id, target)),
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

    /// The water's template, changed by `f`.
    fn retemplate(&mut self, f: impl FnOnce(&mut benilla_protocol::ItemInfo)) {
        let mut items = self.app.world_mut().resource_mut::<crate::items::Items>();
        let mut t = items
            .template_cached(WATER)
            .cloned()
            .expect("the water's template");
        f(&mut t);
        items.insert_template(WATER, Some(t));
    }

    /// Five waters in the backpack's first slot (`PLAYER_FIELD_PACK_SLOT_1`), with
    /// Drink as their on-use spell.
    fn water_in_the_backpack(&mut self) {
        let world = self.app.world_mut();
        let water = world
            .spawn((
                Guid(WATER_GUID),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (benilla_protocol::field::FIELD_OBJECT_ENTRY, WATER),
                    (benilla_protocol::field::FIELD_UNIT_CREATEDBY, 5),
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
                (benilla_protocol::field::FIELD_OBJECT_TYPE, 0x19),
                (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_FLAGS, 0x8),
                (
                    benilla_protocol::field::FIELD_PLAYER_PACK_SLOT_1,
                    WATER_GUID as u32,
                ),
                (
                    benilla_protocol::field::FIELD_PLAYER_PACK_SLOT_1 + 1,
                    (WATER_GUID >> 32) as u32,
                ),
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

/// What reads the script calls' work still follows them: the two item legs and the target chain.
#[test]
fn what_follows_the_script_calls_still_follows_them() {
    let g = graph_or_skip!();
    for after in [
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

/// Feed Pet through `DropItemOnUnit("pet")` clears the cursor inside its press step (`0x80b556`), so the world click's payload leg after it raises no destroy confirm.
#[test]
fn a_fed_items_clear_lands_before_the_world_clicks_payload_leg() {
    use bevy::ecs::system::RunSystemOnce;
    const FEED_PET: u32 = 6991;
    const FOOD: u32 = 2287;
    const FOOD_GUID: u64 = 0x4000_0000_0000_0077;
    let g = graph_or_skip!();
    let drop = g.one("::drop_item_on_unit");
    let leg = g.one("::drop_on_world_click");
    assert!(g.reaches(drop, leg), "the drop runs before the payload leg");
    let mut rig = Rig::new(&[]).expect("names");
    let world = rig.app.world_mut();
    let index = world.resource::<GuidIndex>().0.clone();
    world
        .entity_mut(index[&ME])
        .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (benilla_protocol::field::FIELD_OBJECT_TYPE, 0x19),
            (benilla_protocol::field::FIELD_UNIT_FLAGS, 0x8),
            (
                benilla_protocol::field::FIELD_PLAYER_PACK_SLOT_1,
                FOOD_GUID as u32,
            ),
            (
                benilla_protocol::field::FIELD_PLAYER_PACK_SLOT_1 + 1,
                (FOOD_GUID >> 32) as u32,
            ),
        ])));
    world
        .entity_mut(index[&PET])
        .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (benilla_protocol::field::FIELD_UNIT_CREATED_BY_SPELL, 883),
            (benilla_protocol::field::FIELD_UNIT_CREATEDBY, ME as u32),
            (benilla_protocol::field::FIELD_UNIT_CREATEDBY + 1, 0),
        ])));
    world
        .resource_mut::<crate::ui_action::LearnedAbilities>()
        .feed_pet = Some(FEED_PET);
    rig.vm().set_container(
        0,
        Some(benilla_ui::script::ContainerState {
            bag_family: 0,
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: HashMap::from([(
                1,
                benilla_ui::script::ContainerSlot {
                    item_id: FOOD,
                    count: 1,
                    ..Default::default()
                },
            )]),
        }),
    );
    rig.lua(
        r#"CONFIRMS = 0
           local f = CreateFrame("Frame")
           f:RegisterEvent("DELETE_ITEM_CONFIRM")
           f:SetScript("OnEvent", function() CONFIRMS = CONFIRMS + 1 end)
           PickupContainerItem(0, 1)
           DropItemOnUnit("pet")"#,
    );
    assert!(
        rig.vm().eval::<bool>("return CursorHasItem()").unwrap(),
        "fixture: the food is held"
    );
    // The same press's click, claimed for a payload drop, on open ground.
    let world = rig.app.world_mut();
    world.write_message(crate::target::WorldDropClick);
    world
        .run_system_once(crate::ui_action::drop_item::drop_item_on_unit)
        .unwrap();
    world
        .run_system_once(crate::ui_action::drop_item::drop_on_world_click)
        .unwrap();
    rig.vm().tick(0.01);
    assert!(
        !rig.vm().eval::<bool>("return CursorHasItem()").unwrap(),
        "Feed Pet cleared the cursor"
    );
    assert_eq!(
        rig.vm().eval::<i64>("return CONFIRMS").unwrap(),
        0,
        "the fed food raises no destroy confirm"
    );
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

/// The world clicks resolve against the press's pick: the click tests settle in the press pass,
/// the press latch follows the settle and the select follows the latch, all before the tick, as
/// 12340's input pass selects (`0x480410`) before the paint's `OnUpdate` walk (`0x495320`); the
/// hover pick follows player control, and every later click leg (the cursor commits, Click to
/// Move's arrival, the right click and a crate's interaction) follows both picks.
#[test]
fn the_world_clicks_resolve_against_this_frames_pick() {
    let g = graph_or_skip!();
    let latch = "::latch_press_pick";
    assert!(g.before("::latch_world_mouse", latch));
    assert!(g.before(latch, Step::WorldSelect.suffix()));
    assert!(g.before(Step::WorldSelect.suffix(), Step::Tick.suffix()));
    assert!(g.before(Step::ScriptCalls.suffix(), Step::WorldSelect.suffix()));
    assert!(g.before("::controller::control", "::update_hover"));
    for pick in [latch, "::update_hover"] {
        for leg in [
            "::act_on_arrival",
            "::act_on_right_click",
            "::act_on_interact",
        ] {
            assert!(g.before(pick, leg), "{leg} follows {pick}");
        }
    }
    // The cursor commits read the press latch alone, after the gated select and before the tick, as 12340's input pass commits (`0x480410`) before the paint (`0x495320`).
    for leg in [
        "::commit_ground_cast_on_click",
        "::commit_object_cast_on_click",
    ] {
        assert!(
            g.before(Step::WorldSelect.suffix(), leg),
            "{leg} follows the select"
        );
        assert!(
            g.before(leg, Step::Tick.suffix()),
            "{leg} precedes the tick"
        );
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
            (benilla_protocol::field::FIELD_OBJECT_TYPE, 0x19),
            (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
            (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
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
/// cast goes to the unit the click selected, once, as 12340's input pass selects before the
/// paint runs the `OnUpdate` scripts (`0x480410`, `0x495320`).
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
        .resource_mut::<crate::ui_action::ActionSlots>()
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
            .record_count()
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
        in_frame(&log, 3).contains(&"OnUpdate action swept pet cold book swept".to_string()),
        "the GCD reaches the bar by the frame after the press: {log:?}"
    );
}

// ── B: #1215, the press before the frame's cooldown publication and its tick ───────────────────

/// No feed publishes cooldowns: the bar, the pet bar, the spellbook, the forms, the companions
/// and the bags read them at the call and the presses fire their signals inside the press; the
/// runes' regen tick runs before the UI's tick as the player's update does (`0x728aa6`).
#[test]
fn the_rune_regen_runs_before_the_tick() {
    let g = graph_or_skip!();
    assert!(g.before("::tick_runes", Step::Tick.suffix()));
}

/// The presses outside the script calls run in the press pass before the feed and tick (`0x480410` before `0x495320`), their GCD signals at their step (`0x805d70` -> `0x805ec3`): each shows its sweep in its press's frame.
#[test]
fn the_presses_outside_the_script_calls_cast_before_the_feed_and_tick() {
    let g = graph_or_skip!();
    for press in [
        "::ui_tradeskill::drain_trade_skill",
        "::targeting::item::commit_item_cast_on_pick",
        "::death::drain_death",
        "::commit_ground_cast_on_click",
        "::commit_object_cast_on_click",
    ] {
        assert!(
            g.before(Step::UiInput.suffix(), press),
            "{press} follows the clicks"
        );
        assert!(
            g.before(Step::ScriptCalls.suffix(), press),
            "{press} follows the script calls"
        );
        assert!(
            g.before(press, Step::UnitFeed.suffix()),
            "{press} precedes the feed"
        );
        assert!(
            g.before(press, Step::Tick.suffix()),
            "{press} precedes the tick"
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
            "OnUpdate action swept pet cold book swept",
        ],
        "{log:?}"
    );
}

/// The spellbook's sweep of the same press, in the same frame: `GetSpellCooldown` reads the GCD
/// from the history at the call in the frame's `OnUpdate` (`0x809000`).
#[test]
fn a_pressed_cast_shows_its_gcd_on_the_spellbook_in_the_frame_of_the_press() {
    let mut rig = rig_or_skip!(&[
        Step::BindingSync,
        Step::BindingDispatch,
        Step::ScriptCalls,
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
        [
            "ACTIONBAR_UPDATE_COOLDOWN",
            "SPELL_UPDATE_COOLDOWN",
            "OnUpdate action swept pet cold book swept"
        ],
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
    // The start the frame was handed: a later `GetActionCooldown` reads the recovery off the wall
    // clock against the rig's pinned UI clock, so it drifts by however long the frame took.
    let (start, duration): (f64, f64) =
        crate::ui_script::live::with_live_world(rig.app.world_mut(), |s| {
            s.eval("local _, d = GetActionCooldown(2) return PROBE_START.ProbeCooldown2, d")
                .expect("Nova's triple")
        })
        .expect("the VM");
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

// ── C: a script call applies inside the Lua call that made it ──────────────────────────────────

/// An `OnUpdate` that runs `TICK_DO` once, the next frame after it is set.
const TICK_DO: &str = r#"
    local f = CreateFrame("Frame")
    f:SetScript("OnUpdate", function()
        if TICK_DO then
            local run = TICK_DO
            TICK_DO = nil
            run()
        end
    end)
"#;

/// `UseAction` then `GetActionCooldown` in one `OnUpdate`: the cast goes out and its GCD is armed
/// inside the call (`0x80b583` → `0x805d70`), so the read after it sees the sweep.
#[test]
fn use_action_then_get_action_cooldown_reads_the_gcd_in_one_chunk() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.lua(TICK_DO);
    rig.lua("TICK_DO = function() UseAction(1) READ = GetActionCooldown(1) end");
    rig.frame();
    assert_eq!(
        rig.casts(),
        [(BOLT, Some(OLD))],
        "the cast went out in the call"
    );
    let start: f64 = rig.vm().eval("return READ").expect("a start");
    assert!(start > 0.0, "the read after the call sees the GCD: {start}");
}

/// `TargetUnit` then the getters in one `OnUpdate`, with no token's snapshot naming the new target:
/// the getters look the target global's object up (`0x524f38`), so they read it in the call.
#[test]
fn target_unit_then_unit_exists_reads_a_unit_no_snapshot_held() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.lua(TICK_DO);
    rig.lua(
        r#"TICK_DO = function()
            TargetUnit("party1")
            READ = { UnitExists("target") and true or false, UnitGUID("target") }
        end"#,
    );
    rig.frame();
    assert_eq!(rig.selected(), Some(NEW));
    let (exists, guid): (bool, String) =
        rig.vm().eval("return READ[1], READ[2]").expect("the reads");
    assert!(exists, "the new target exists in the call");
    assert_eq!(guid, format!("0x{NEW:016X}"));
}

/// The new target `party1` as a named player at 42 health, so its snapshot reads apart from `OLD`'s.
fn bob_at_42(rig: &mut Rig) {
    let world = rig.app.world_mut();
    let entity = world.resource::<GuidIndex>().0[&NEW];
    world
        .entity_mut(entity)
        .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (benilla_protocol::field::FIELD_OBJECT_TYPE, 0x19),
            (benilla_protocol::field::FIELD_UNIT_HEALTH, 42),
            (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
        ])));
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(NEW, "Bob".into(), None);
}

/// `TargetUnit` then the snapshot getters in one `OnUpdate`: the setter's tail writes the new
/// target's snapshot before it returns (`0x524f38`), so `UnitName` and `UnitHealth` read it in the
/// call.
#[test]
fn target_unit_then_unit_name_reads_the_new_target_in_one_chunk() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    bob_at_42(&mut rig);
    rig.lua(TICK_DO);
    rig.lua(
        r#"TICK_DO = function()
            TargetUnit("party1")
            READ = { UnitName("target"), UnitHealth("target") }
        end"#,
    );
    rig.frame();
    assert_eq!(rig.selected(), Some(NEW));
    let (name, health): (String, i64) =
        rig.vm().eval("return READ[1], READ[2]").expect("the reads");
    assert_eq!(
        (name.as_str(), health),
        ("Bob", 42),
        "the new target's snapshot in the call"
    );
}

/// A snapshot the setter's tail wrote is diffed by the feed against what the VM holds: the
/// selection moved back by a path that is not a script's leaves no stale `"target"` behind.
#[test]
fn a_target_the_tail_wrote_is_replaced_when_the_selection_moves_back() {
    let mut rig = rig_or_skip!(&[Step::UnitFeed, Step::Tick]);
    bob_at_42(&mut rig);
    rig.lua(TICK_DO);
    rig.lua(r#"TICK_DO = function() TargetUnit("party1") end"#);
    rig.frame();
    let health: i64 = rig
        .vm()
        .eval(r#"return UnitHealth("target")"#)
        .expect("a read");
    assert_eq!(health, 42, "the tail wrote the new target's snapshot");
    rig.lua("TICK_DO = nil");
    let old = rig.app.world().resource::<GuidIndex>().0[&OLD];
    *rig.app.world_mut().resource_mut::<Selection>() = Selection {
        target: Some(old),
        guid: Some(OLD),
        ..Default::default()
    };
    rig.frame();
    let health: i64 = rig
        .vm()
        .eval(r#"return UnitHealth("target")"#)
        .expect("a read");
    assert_eq!(
        health, 100,
        "the feed pushed the old target's snapshot over the tail's"
    );
}

/// The setter fires `PLAYER_TARGET_CHANGED` before it returns (`0x524f54`), after the usable
/// update (`0x524f48`): a listener runs inside the `TargetUnit` call, once.
#[test]
fn target_unit_fires_player_target_changed_inside_the_call() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.lua(TICK_DO);
    rig.lua(
        r#"ORDER = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("PLAYER_TARGET_CHANGED")
        f:SetScript("OnEvent", function(self, event)
            ORDER[#ORDER + 1] = event .. " " .. (UnitGUID("target") or "nil")
        end)
        TICK_DO = function()
            TargetUnit("party1")
            ORDER[#ORDER + 1] = "returned"
        end"#,
    );
    rig.frame();
    rig.frame();
    let order: Vec<String> = rig.vm().eval("return ORDER").expect("the order");
    assert_eq!(
        order,
        [
            format!("PLAYER_TARGET_CHANGED 0x{NEW:016X}"),
            "returned".into()
        ]
    );
}

/// A script call a listener makes while a press's raise runs applies inside that listener, nested,
/// as 12340's verbs re-enter from a `UNIT_SPELLCAST_SENT` listener (`0x80b556`): the select is
/// read back before the listener returns, and the outer press's cast kept its old target.
#[test]
fn a_select_from_a_sent_listener_applies_inside_it() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.lua(TICK_DO);
    rig.lua(
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("UNIT_SPELLCAST_SENT")
        f:SetScript("OnEvent", function()
            TargetUnit("party1")
            SEEN = UnitGUID("target")
        end)
        TICK_DO = function() UseAction(1) TICK_DO = nil end"#,
    );
    rig.frame();
    let seen: String = rig.vm().eval("return SEEN").expect("the listener ran");
    assert_eq!(
        seen,
        format!("0x{NEW:016X}"),
        "the listener's select applied in it"
    );
    assert_eq!(rig.selected(), Some(NEW));
    assert_eq!(rig.casts(), [(BOLT, Some(OLD))]);
}

/// A cast a `UNIT_SPELLCAST_SENT` listener makes applies inside it (`0x80b556` re-enters `0x80cce0`): the bail passes as the GCD arms after `SENT` (`0x80d36d`, `0x80b583`), so the nested press sends first.
#[test]
fn a_cast_from_a_sent_listener_applies_inside_it() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.lua(TICK_DO);
    rig.lua(
        r#"SENTS = 0
        local f = CreateFrame("Frame")
        f:RegisterEvent("UNIT_SPELLCAST_SENT")
        f:SetScript("OnEvent", function()
            SENTS = SENTS + 1
            if SENTS == 1 then
                UseAction(1)
                AFTER_NESTED = SENTS
            end
        end)
        TICK_DO = function() UseAction(1) TICK_DO = nil end"#,
    );
    rig.frame();
    let after: i64 = rig
        .vm()
        .eval("return AFTER_NESTED")
        .expect("the listener ran");
    assert_eq!(after, 2, "the nested SENT fired inside the listener");
    assert_eq!(rig.casts(), [(BOLT, Some(OLD)), (BOLT, Some(OLD))]);
}

/// `RepopMe` sends inside the call (`0x51aab3`): the frame runs only the tick, so no press pass could have sent it.
#[test]
fn a_death_verb_sends_inside_its_call() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.lua(TICK_DO);
    rig.lua("TICK_DO = function() RepopMe() end");
    rig.frame();
    assert!(
        rig.sent()
            .iter()
            .any(|c| matches!(c, ClientCommand::RepopRequest { automatic: false })),
        "the release went out inside the call"
    );
}

/// `DropItemOnUnit("pet")` feeds the pet inside the call (`0x51bf26` -> `0x80dcf0`): the caller's next line reads the cursor the feed cleared, and the cast is out.
#[test]
fn a_drop_on_the_pet_feeds_inside_its_call() {
    const FEED_PET: u32 = 6991;
    const FOOD: u32 = 2287;
    const FOOD_GUID: u64 = 0x4000_0000_0000_0077;
    let mut rig = rig_or_skip!(&[Step::Tick]);
    let world = rig.app.world_mut();
    let index = world.resource::<GuidIndex>().0.clone();
    world
        .entity_mut(index[&ME])
        .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (benilla_protocol::field::FIELD_OBJECT_TYPE, 0x19),
            (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
            (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
            (benilla_protocol::field::FIELD_UNIT_FLAGS, 0x8),
            (
                benilla_protocol::field::FIELD_PLAYER_PACK_SLOT_1,
                FOOD_GUID as u32,
            ),
            (
                benilla_protocol::field::FIELD_PLAYER_PACK_SLOT_1 + 1,
                (FOOD_GUID >> 32) as u32,
            ),
        ])));
    world
        .entity_mut(index[&PET])
        .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (benilla_protocol::field::FIELD_UNIT_CREATED_BY_SPELL, 883),
            (benilla_protocol::field::FIELD_UNIT_CREATEDBY, ME as u32),
            (benilla_protocol::field::FIELD_UNIT_CREATEDBY + 1, 0),
        ])));
    world
        .resource_mut::<crate::ui_action::LearnedAbilities>()
        .feed_pet = Some(FEED_PET);
    rig.vm().set_container(
        0,
        Some(benilla_ui::script::ContainerState {
            bag_family: 0,
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: HashMap::from([(
                1,
                benilla_ui::script::ContainerSlot {
                    item_id: FOOD,
                    count: 1,
                    ..Default::default()
                },
            )]),
        }),
    );
    rig.lua(TICK_DO);
    rig.lua(
        r#"PickupContainerItem(0, 1)
           TICK_DO = function() DropItemOnUnit("pet") HELD = CursorHasItem() end"#,
    );
    rig.frame();
    assert!(
        !rig.vm().eval::<bool>("return HELD").unwrap(),
        "the next line reads the cursor the feed cleared"
    );
    assert!(
        rig.sent().iter().any(|c| matches!(
            c,
            ClientCommand::CastSpellItem {
                spell_id: FEED_PET,
                item_guid: FOOD_GUID,
                ..
            }
        )),
        "Feed Pet went out at the food"
    );
}

/// `SpellTargetItem` casts the cursor's spell at the item inside the call (`0x8008a5` -> `0x5210d0`): the caller's next line reads the cursor down and the GCD the press armed.
#[test]
fn an_item_pick_casts_inside_its_call() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.water_in_the_backpack();
    rig.app
        .world_mut()
        .resource_mut::<crate::spell::SpellTargeting>()
        .enter(BOLT, crate::spell::CastCommit::Spell, 0x10);
    rig.vm().set_container(
        0,
        Some(benilla_ui::script::ContainerState {
            bag_family: 0,
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: HashMap::from([(
                1,
                benilla_ui::script::ContainerSlot {
                    item_id: WATER,
                    count: 5,
                    ..Default::default()
                },
            )]),
        }),
    );
    rig.lua(TICK_DO);
    rig.lua(&format!(
        "TICK_DO = function() SpellTargetItem({WATER}) UP = SpellIsTargeting() \
         GCD = select(2, GetActionCooldown(1)) end"
    ));
    rig.frame();
    assert!(
        rig.sent().iter().any(|c| matches!(
            c,
            ClientCommand::CastSpellItem {
                spell_id: BOLT,
                item_guid: WATER_GUID,
                ..
            }
        )),
        "the cursor's spell went out at the water"
    );
    assert!(
        !rig.vm().eval::<bool>("return UP == 1").unwrap(),
        "the cursor was down by the next line"
    );
    let gcd: f64 = rig.vm().eval("return GCD").unwrap();
    assert_eq!(gcd, 1.5, "the next line reads the press's GCD");
}

/// `UNIT_SPELLCAST_SENT` fires before the start visuals and the GCD arm (`0x80b556`, `0x80b573`, `0x80b583`): its listener reads no cooldown, the code after the call reads the GCD.
#[test]
fn a_sent_listener_reads_no_gcd_yet() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.lua(TICK_DO);
    rig.lua(
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("UNIT_SPELLCAST_SENT")
        f:SetScript("OnEvent", function() AT_SENT = GetActionCooldown(1) end)
        TICK_DO = function() UseAction(1) AFTER = GetActionCooldown(1) TICK_DO = nil end"#,
    );
    rig.frame();
    assert_eq!(rig.casts(), [(BOLT, Some(OLD))]);
    let at_sent: f64 = rig.vm().eval("return AT_SENT").expect("the listener ran");
    let after: f64 = rig.vm().eval("return AFTER").expect("the call returned");
    assert_eq!(at_sent, 0.0, "no GCD at SENT");
    assert!(after > 0.0, "the press armed the GCD after SENT: {after}");
}

/// A press that stops the targeting cursor runs its rungs below after the stop's selection update (`0x80d3ce`, `0x8054f0`): that listener reads the press neither filed nor sent.
#[test]
fn a_stopped_cursors_selection_listener_reads_the_press_unsent() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.app
        .world_mut()
        .resource_mut::<crate::spell::SpellTargeting>()
        .enter(NOVA, crate::spell::CastCommit::Spell, 0x40);
    rig.lua(TICK_DO);
    rig.lua(
        r#"ORDER = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("CURRENT_SPELL_CAST_CHANGED")
        f:RegisterEvent("UNIT_SPELLCAST_SENT")
        f:SetScript("OnEvent", function(_, e)
            if e == "CURRENT_SPELL_CAST_CHANGED" and #ORDER == 0 then
                AT_STOP = IsCurrentAction(1) and 1 or 0
            end
            ORDER[#ORDER + 1] = e
        end)
        TICK_DO = function() UseAction(1) TICK_DO = nil end"#,
    );
    rig.frame();
    assert_eq!(rig.casts(), [(BOLT, Some(OLD))]);
    let at_stop: i64 = rig.vm().eval("return AT_STOP").expect("the listener ran");
    assert_eq!(at_stop, 0, "the press is not filed at the stop's update");
    let order: Vec<String> = rig.vm().eval("return ORDER").expect("the order");
    let sent = order.iter().position(|e| e == "UNIT_SPELLCAST_SENT");
    assert_eq!(
        order.first().map(String::as_str),
        Some("CURRENT_SPELL_CAST_CHANGED")
    );
    assert!(
        sent.is_some_and(|at| at > 0),
        "SENT after the stop's update: {order:?}"
    );
}

/// A listener on `event`, raised by the call `call` makes inside an `OnUpdate`, that targets
/// `party1` and reads the target back inside the listener: what it read, and the selection after.
fn listener_selects(rig: &mut Rig, event: &str, call: &str) -> (Option<String>, Option<u64>) {
    rig.lua(TICK_DO);
    rig.lua(&format!(
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("{event}")
        f:SetScript("OnEvent", function()
            if SEEN then return end
            TargetUnit("party1")
            SEEN = UnitGUID("target")
        end)
        TICK_DO = function() {call} end"#
    ));
    rig.frame();
    let seen = rig
        .vm()
        .eval::<Option<String>>("return SEEN")
        .ok()
        .flatten();
    (seen, rig.selected())
}

/// The selection's refusal line (`0x525f87` -> `0x5216f0`) raises as `AssistUnit` returns, with no
/// applier borrow held: its listener's `TargetUnit` applies inside it.
#[test]
fn a_select_from_a_refusal_line_listener_applies_inside_it() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.lua(r#"ERR_UNIT_NOT_FOUND = "Unknown unit.""#);
    let (seen, selected) =
        listener_selects(&mut rig, "UI_ERROR_MESSAGE", r#"AssistUnit("nobody")"#);
    assert_eq!(seen, Some(format!("0x{NEW:016X}")));
    assert_eq!(selected, Some(NEW));
}

/// The item-use fork's `USE_GLYPH` (`0x5d8ad3`) raises as `UseContainerItem` returns: its
/// listener's `TargetUnit` applies inside it.
#[test]
fn a_select_from_a_use_glyph_listener_applies_inside_it() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.water_in_the_backpack();
    rig.retemplate(|t| t.class = 16);
    let (seen, selected) = listener_selects(&mut rig, "USE_GLYPH", "UseContainerItem(0, 1)");
    assert_eq!(seen, Some(format!("0x{NEW:016X}")));
    assert_eq!(selected, Some(NEW));
}

/// The direct use runs after `USE_GLYPH`'s listeners (`0x5d8ad3` before `0x5d8ae3` -> `0x708c20`): the listener's cast goes out first and the use meets its GCD.
#[test]
fn a_use_glyph_listeners_cast_sends_before_the_glyphs_use() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.water_in_the_backpack();
    rig.retemplate(|t| t.class = 16);
    rig.lua(TICK_DO);
    rig.lua(
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("USE_GLYPH")
        f:SetScript("OnEvent", function() UseAction(1) end)
        TICK_DO = function() UseContainerItem(0, 1) TICK_DO = nil end"#,
    );
    rig.frame();
    let sends: Vec<&str> = rig
        .sent()
        .into_iter()
        .filter_map(|c| match c {
            ClientCommand::CastSpell { .. } => Some("cast"),
            ClientCommand::UseItem { .. } => Some("use"),
            _ => None,
        })
        .collect();
    assert_eq!(sends, ["cast"]);
}

/// Our aura cancel drops the form ahead of the server (`0x80324d` -> `0x721cf0`, `0x721d89`): the listener and the chunk after `CancelShapeshiftForm` read no form, and the cancel goes out.
#[test]
fn a_form_cancel_drops_the_form_before_the_server_answers() {
    const BEAR: u32 = 5487;
    const FORM: u32 = 5;
    let mut rig = rig_or_skip!(&[Step::Tick]);
    {
        let world = rig.app.world_mut();
        let me = world.resource::<GuidIndex>().0[&ME];
        world.entity_mut(me).insert((
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (benilla_protocol::field::FIELD_OBJECT_TYPE, 0x19),
                (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_FLAGS, 0x8),
                (benilla_protocol::field::FIELD_UNIT_BYTES_2, FORM << 24),
            ])),
            crate::unit_auras::UnitAuras::of(
                ME,
                &[(
                    0,
                    BEAR,
                    benilla_protocol::messages::AURA_UPDATE_FLAG_CANCELABLE | 1,
                )],
            ),
        ));
        let mut spells = world.resource_mut::<crate::ui_action::Spells>();
        let bear = SpellDisplay {
            name: "Bear Form".into(),
            shapeshift_form: Some(FORM),
            ..Default::default()
        };
        spells.catalog = SpellCatalog::from_displays(HashMap::from([(BEAR, bear)]));
        spells
            .forms
            .insert(FORM, benilla_formats::ShapeshiftForm::default());
    }
    rig.lua(TICK_DO);
    rig.lua(
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("UPDATE_SHAPESHIFT_FORM")
        f:SetScript("OnEvent", function() IN_EVENT = GetShapeshiftForm() end)
        TICK_DO = function()
            BEFORE = GetShapeshiftForm()
            CancelShapeshiftForm()
            AFTER = GetShapeshiftForm()
            TICK_DO = nil
        end"#,
    );
    rig.frame();
    let read: (i64, Option<i64>, i64) = rig
        .vm()
        .eval("return BEFORE, IN_EVENT, AFTER")
        .expect("the chunk ran");
    assert_eq!(read, (1, Some(0), 0), "(before, in the signal, after)");
    let cancels: Vec<u32> = rig
        .sent()
        .into_iter()
        .filter_map(|c| match c {
            ClientCommand::CancelAura { spell_id } => Some(spell_id),
            _ => None,
        })
        .collect();
    assert_eq!(cancels, [BEAR]);
}

/// A cast's form drop raises before its send (`0x80af88` -> `0x80324d`, then `0x803458`, `0x80b4ee`): the listener reads the cast not current and its select goes out first.
#[test]
fn a_casts_form_drop_listener_runs_before_the_send() {
    const BEAR: u32 = 5487;
    const FORM: u32 = 5;
    let mut rig = rig_or_skip!(&[Step::Tick]);
    {
        let world = rig.app.world_mut();
        let me = world.resource::<GuidIndex>().0[&ME];
        world.entity_mut(me).insert((
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (benilla_protocol::field::FIELD_OBJECT_TYPE, 0x19),
                (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_FLAGS, 0x8),
                (benilla_protocol::field::FIELD_UNIT_BYTES_2, FORM << 24),
            ])),
            crate::unit_auras::UnitAuras::of(
                ME,
                &[(
                    0,
                    BEAR,
                    benilla_protocol::messages::AURA_UPDATE_FLAG_CANCELABLE | 1,
                )],
            ),
        ));
        let mut spells = world.resource_mut::<crate::ui_action::Spells>();
        // Bolt, `Attributes & 0x10000`: not while shapeshifted, so the send leaves the form.
        let bolt = SpellDisplay {
            name: "Bolt".into(),
            attributes: 0x0001_0000,
            implicit_target_a1: 6,
            start_recovery_category: 133,
            start_recovery_ms: 1500,
            ..Default::default()
        };
        let bear = SpellDisplay {
            name: "Bear Form".into(),
            shapeshift_form: Some(FORM),
            ..Default::default()
        };
        spells.catalog = SpellCatalog::from_displays(HashMap::from([(BOLT, bolt), (BEAR, bear)]));
        spells
            .forms
            .insert(FORM, benilla_formats::ShapeshiftForm::default());
    }
    rig.lua(TICK_DO);
    rig.lua(
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("UPDATE_SHAPESHIFT_FORM")
        f:SetScript("OnEvent", function()
            if AT_DROP then return end
            AT_DROP = IsCurrentAction(1) and 1 or 0
            TargetUnit("party1")
        end)
        TICK_DO = function() UseAction(1) TICK_DO = nil end"#,
    );
    rig.frame();
    let at_drop: i64 = rig.vm().eval("return AT_DROP").expect("the listener ran");
    assert_eq!(at_drop, 0, "the cast is not current at its form drop");
    let wire: Vec<&str> = rig
        .sent()
        .into_iter()
        .filter_map(|c| match c {
            ClientCommand::SetSelection { guid: NEW } => Some("select"),
            ClientCommand::CancelAura { spell_id: BEAR } => Some("cancel"),
            // The attempt bound its unit before the drop.
            ClientCommand::CastSpell {
                spell_id: BOLT,
                target: Some(OLD),
                ..
            } => Some("cast"),
            _ => None,
        })
        .collect();
    assert_eq!(wire, ["select", "cancel", "cast"]);
}

/// The not-ready refusal over a wand repeat cancels it (`0x8082cc`, `0x8082d3`) before its line (`0x808200`): a `UI_ERROR_MESSAGE` listener reads the repeat over, ahead of the code after the call.
#[test]
fn a_ui_error_listener_mid_press_reads_the_state_at_its_step() {
    const SHOOT: u32 = 5019;
    let mut rig = rig_or_skip!(&[Step::Tick]);
    {
        let world = rig.app.world_mut();
        let mut spells = world.resource_mut::<crate::ui_action::Spells>();
        spells.catalog = SpellCatalog::from_displays(HashMap::from([
            (
                NOVA,
                SpellDisplay {
                    name: "Nova".into(),
                    ..Default::default()
                },
            ),
            (
                SHOOT,
                SpellDisplay {
                    name: "Shoot".into(),
                    attributes_ex2: 0x20,
                    attributes_ex3: 0x0040_0000,
                    ..Default::default()
                },
            ),
        ]));
        world
            .resource_mut::<crate::ui_action::ActionSlots>()
            .buttons
            .insert(
                0,
                ActionButton {
                    slot: 0,
                    action: SHOOT,
                    kind: ACTION_KIND_SPELL,
                },
            );
        world
            .resource_mut::<crate::ui_action::PlayerActions>()
            .spells = [SHOOT, NOVA].into();
        world.resource_mut::<crate::spell::AutoRepeatActive>().0 = Some(SHOOT);
    }
    rig.lua(TICK_DO);
    rig.lua(
        r#"SEEN = {} ERR_SPELL_COOLDOWN = "Spell is not ready yet."
        local f = CreateFrame("Frame")
        f:RegisterEvent("UI_ERROR_MESSAGE")
        f:SetScript("OnEvent", function()
            SEEN[#SEEN + 1] = "line repeat=" .. (IsAutoRepeatAction(1) and 1 or 0)
        end)
        TICK_DO = function()
            SEEN[#SEEN + 1] = "before repeat=" .. (IsAutoRepeatAction(1) and 1 or 0)
            UseAction(2)
            SEEN[#SEEN + 1] = "after"
            TICK_DO = nil
        end"#,
    );
    rig.frame();
    let seen: String = rig
        .vm()
        .eval("return table.concat(SEEN, ' ')")
        .expect("the chunk ran");
    assert_eq!(seen, "before repeat=1 line repeat=0 after");
    let cancels: Vec<(u32, u8)> = rig
        .sent()
        .into_iter()
        .filter_map(|c| match c {
            ClientCommand::CancelCast { spell_id, count } => Some((spell_id, count)),
            _ => None,
        })
        .collect();
    assert_eq!(
        cancels,
        [(SHOOT, 0)],
        "the refusal's cancel, count 0 (`0x8082ad`)"
    );
}

/// Bear Form's spell, holding form 5 on us with its aura up and cancelable.
const BEAR: u32 = 5487;

/// Us in Bear Form (form 5, its aura up and cancelable), Bear on the first action slot, and an `UPDATE_SHAPESHIFT_FORM` listener that selects `party1` once.
fn in_bear_form(rig: &mut Rig) {
    const FORM: u32 = 5;
    let world = rig.app.world_mut();
    let me = world.resource::<GuidIndex>().0[&ME];
    world.entity_mut(me).insert((
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (benilla_protocol::field::FIELD_OBJECT_TYPE, 0x19),
            (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
            (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
            (benilla_protocol::field::FIELD_UNIT_FLAGS, 0x8),
            (benilla_protocol::field::FIELD_UNIT_BYTES_2, FORM << 24),
        ])),
        crate::unit_auras::UnitAuras::of(
            ME,
            &[(
                0,
                BEAR,
                benilla_protocol::messages::AURA_UPDATE_FLAG_CANCELABLE | 1,
            )],
        ),
    ));
    let mut spells = world.resource_mut::<crate::ui_action::Spells>();
    let bear = SpellDisplay {
        name: "Bear Form".into(),
        shapeshift_form: Some(FORM),
        effect_apply_aura: [36, 0, 0],
        ..Default::default()
    };
    spells.catalog = SpellCatalog::from_displays(HashMap::from([(BEAR, bear)]));
    spells
        .forms
        .insert(FORM, benilla_formats::ShapeshiftForm::default());
    world
        .resource_mut::<crate::ui_action::ActionSlots>()
        .buttons
        .insert(
            0,
            ActionButton {
                slot: 0,
                action: BEAR,
                kind: ACTION_KIND_SPELL,
            },
        );
    world
        .resource_mut::<crate::ui_action::PlayerActions>()
        .spells = [BEAR].into();
    world.insert_resource(crate::ui_spellbook::SpellBook::of(SpellBookState {
        tabs: vec![SpellTabView {
            name: "General".into(),
            texture: None,
            offset: 0,
            num_spells: 1,
        }],
        slots: vec![SpellSlotView {
            spell_id: BEAR,
            name: "Bear Form".into(),
            ..Default::default()
        }],
        ..Default::default()
    }));
    rig.lua(
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("UPDATE_SHAPESHIFT_FORM")
        f:SetScript("OnEvent", function()
            if AT_DROP then return end
            AT_DROP = 1
            TargetUnit("party1")
        end)"#,
    );
}

/// The selects, our Bear cancels and gameobject uses sent since, in order.
fn form_leave_wire(rig: &Rig) -> Vec<&'static str> {
    rig.sent()
        .into_iter()
        .filter_map(|c| match c {
            ClientCommand::SetSelection { guid: NEW } => Some("select"),
            ClientCommand::CancelAura { spell_id: BEAR } => Some("cancel"),
            ClientCommand::GameObjUse { .. } => Some("use"),
            _ => None,
        })
        .collect()
}

/// `CancelShapeshiftForm` (`0x726ce0` -> `0x802f80`): the drop's listeners run before `CMSG_CANCEL_AURA` (`0x80324d` before `0x803458`), so a select a listener makes goes out first.
#[test]
fn a_form_cancels_drop_listener_sends_before_the_cancel() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    in_bear_form(&mut rig);
    rig.lua(TICK_DO);
    rig.lua(r#"TICK_DO = function() CancelShapeshiftForm() TICK_DO = nil end"#);
    rig.frame();
    assert_eq!(form_leave_wire(&rig), ["select", "cancel"]);
}

/// The spellbook's press of the active form cancels it (`0x802f80`): the drop's listeners run before the cancel's send (`0x80324d` before `0x803458`).
#[test]
fn a_book_forms_drop_listener_sends_before_the_cancel() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    in_bear_form(&mut rig);
    rig.lua(TICK_DO);
    rig.lua(r#"TICK_DO = function() CastSpellByName("Bear Form") TICK_DO = nil end"#);
    rig.frame();
    assert_eq!(form_leave_wire(&rig), ["select", "cancel"]);
}

/// `CancelUnitBuff` on our form's aura (`0x802f80`): its drain sends after the drop's listeners (`0x80324d` before `0x803458`).
#[test]
fn a_buff_cancels_drop_listener_sends_before_the_cancel() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    in_bear_form(&mut rig);
    rig.vm()
        .set_player_auras(vec![benilla_ui::script::AuraState {
            spell_id: BEAR,
            name: Some("Bear Form".into()),
            count: 1,
            helpful: true,
            cancelable: true,
            ..Default::default()
        }]);
    rig.lua(TICK_DO);
    rig.lua(r#"TICK_DO = function() CancelUnitBuff("player", 1) TICK_DO = nil end"#);
    rig.frame();
    let mut drain = Schedule::default();
    drain.add_systems(crate::ui_script::raises::then_fire(
        crate::ui_aura::drain_aura_cancels,
    ));
    drain.run(rig.app.world_mut());
    assert_eq!(form_leave_wire(&rig), ["select", "cancel"]);
}

/// The world click's and the meeting stone's use leave the form first: each aura cancel's drop and its listeners, its send, then the use (`0x80324d`, `0x803458`, then `CMSG_GAMEOBJ_USE`).
#[test]
fn a_form_leave_then_use_sends_after_the_drops_listeners() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    in_bear_form(&mut rig);
    let world = rig.app.world_mut();
    crate::ui_shapeshift::cancel_aura_step(world, BEAR);
    crate::ui_shapeshift::send_after_raises(world, ClientCommand::GameObjUse { guid: 0xf1 });
    assert!(
        rig.sent().is_empty(),
        "nothing goes out before the listeners"
    );
    crate::ui_script::live::fire_handler_events(rig.app.world_mut());
    assert_eq!(form_leave_wire(&rig), ["select", "cancel", "use"]);
}

/// The selects, our Bear cancels, item uses and vendor lists sent since, in order.
fn form_leave_press_wire(rig: &Rig) -> Vec<&'static str> {
    rig.sent()
        .into_iter()
        .filter_map(|c| match c {
            ClientCommand::SetSelection { guid: NEW } => Some("select"),
            ClientCommand::CancelAura { spell_id: BEAR } => Some("cancel"),
            ClientCommand::UseItem { .. } => Some("use"),
            ClientCommand::ListInventory { .. } => Some("list"),
            _ => None,
        })
        .collect()
}

/// The item use leaves the form (`0x709338` -> `0x726ce0` -> `0x802f80`) before its cast (`0x70933d`): the drop's listener sends ahead of the cancel (`0x80324d` before `0x803458`) and of `CMSG_USE_ITEM`.
#[test]
fn an_item_uses_form_drop_listener_sends_before_the_cancel_and_the_use() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.water_in_the_backpack();
    in_bear_form(&mut rig);
    let world = rig.app.world_mut();
    let me = world.resource::<GuidIndex>().0[&ME];
    world
        .entity_mut(me)
        .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (benilla_protocol::field::FIELD_OBJECT_TYPE, 0x19),
            (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
            (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
            (benilla_protocol::field::FIELD_UNIT_FLAGS, 0x8),
            (benilla_protocol::field::FIELD_UNIT_BYTES_2, 5 << 24),
            (
                benilla_protocol::field::FIELD_PLAYER_PACK_SLOT_1,
                WATER_GUID as u32,
            ),
            (
                benilla_protocol::field::FIELD_PLAYER_PACK_SLOT_1 + 1,
                (WATER_GUID >> 32) as u32,
            ),
        ])));
    let mut spells = world.resource_mut::<crate::ui_action::Spells>();
    let bear = SpellDisplay {
        name: "Bear Form".into(),
        shapeshift_form: Some(5),
        effect_apply_aura: [36, 0, 0],
        ..Default::default()
    };
    let drink = SpellDisplay {
        name: "Drink".into(),
        ..Default::default()
    };
    spells.catalog = SpellCatalog::from_displays(HashMap::from([(BEAR, bear), (DRINK, drink)]));
    rig.lua(TICK_DO);
    rig.lua(r#"TICK_DO = function() UseContainerItem(0, 1) TICK_DO = nil end"#);
    rig.frame();
    assert_eq!(form_leave_press_wire(&rig), ["select", "cancel", "use"]);
}

/// The NPC-service ladder leaves the form (`0x6ddc01` -> `0x726ce0` -> `0x802f80`) before the service (`0x6ddc06`): the drop's listener sends ahead of the cancel (`0x80324d` before `0x803458`) and of the vendor's list.
#[test]
fn an_npc_talks_form_drop_listener_sends_before_the_cancel_and_the_service() {
    const VENDOR: u64 = 0xF130_0000_0000_0077;
    let mut rig = rig_or_skip!(&[Step::Tick]);
    in_bear_form(&mut rig);
    let world = rig.app.world_mut();
    let vendor = world
        .spawn((
            Guid(VENDOR),
            Transform::from_xyz(1.0, 0.0, 0.0),
            GlobalTransform::from_translation(Vec3::new(1.0, 0.0, 0.0)),
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_NPC_FLAGS, 0x80),
            ])),
            crate::net::NetEntity {
                kind: benilla_protocol::EntityKind::Unit,
                display_id: None,
                scale: 1.0,
            },
        ))
        .id();
    world.resource_mut::<GuidIndex>().0.insert(VENDOR, vendor);
    world.init_resource::<crate::player::Player>();
    crate::target::click::interact_now(world, vendor);
    assert_eq!(form_leave_press_wire(&rig), ["select", "cancel", "list"]);
}

/// A lootable item's lock edge (`0x6dcded` -> `0x706feb`) raises as `UseContainerItem` returns:
/// its listener's `TargetUnit` applies inside it.
#[test]
fn a_select_from_an_item_lock_listener_applies_inside_it() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.water_in_the_backpack();
    rig.retemplate(|t| t.flags |= benilla_protocol::messages::ITEM_FLAG_LOOTABLE);
    let (seen, selected) =
        listener_selects(&mut rig, "ITEM_LOCK_CHANGED", "UseContainerItem(0, 1)");
    assert_eq!(seen, Some(format!("0x{NEW:016X}")));
    assert_eq!(selected, Some(NEW));
}

/// `call` in an `OnUpdate`, with an `ITEM_LOCKED` listener that casts Bolt: the sends since, as
/// `"cast"`, `"equip"` or `"open"`, the others left out.
fn sends_around_the_lock(rig: &mut Rig, call: &str) -> Vec<&'static str> {
    rig.lua(TICK_DO);
    rig.lua(&format!(
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("ITEM_LOCKED")
        f:SetScript("OnEvent", function() UseAction(1) end)
        TICK_DO = function() {call} end"#
    ));
    rig.frame();
    rig.sent()
        .into_iter()
        .filter_map(|c| match c {
            ClientCommand::CastSpell { .. } => Some("cast"),
            ClientCommand::AutoEquipItemSlot { .. } => Some("equip"),
            ClientCommand::OpenItem { .. } => Some("open"),
            _ => None,
        })
        .collect()
}

/// The slotted equip locks and signals before it sends (`0x6e0136` before `0x6e016e`): a send the
/// lock's listener makes reaches the wire ahead of `CMSG_AUTOEQUIP_ITEM_SLOT`.
#[test]
fn a_slotted_equips_lock_listener_sends_before_the_equip() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.water_in_the_backpack();
    // `EquipItemByName` does nothing without the player (`0x51cdb0`).
    rig.vm().set_unit_guids(&benilla_ui::script::UnitGuids {
        player: ME,
        ..Default::default()
    });
    let sends = sends_around_the_lock(&mut rig, "EquipItemByName(159, 16)");
    assert_eq!(sends, ["cast", "equip"]);
}

/// The item open locks and signals before it sends (`0x6dcded` before `0x6dce3b`): a send the
/// lock's listener makes reaches the wire ahead of `CMSG_OPEN_ITEM`.
#[test]
fn an_item_opens_lock_listener_sends_before_the_open() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.water_in_the_backpack();
    rig.retemplate(|t| t.flags |= benilla_protocol::messages::ITEM_FLAG_LOOTABLE);
    let sends = sends_around_the_lock(&mut rig, "UseContainerItem(0, 1)");
    assert_eq!(sends, ["cast", "open"]);
}

/// The bind dialog's accept re-runs the slotted equip (`0x6e01a0`), which locks and signals before
/// it sends (`0x6e0136` before `0x6e016e`): the lock's listener's send goes out first.
#[test]
fn an_accepted_slotted_equips_lock_listener_sends_before_the_equip() {
    use bevy::ecs::system::RunSystemOnce;
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.water_in_the_backpack();
    let index = rig
        .app
        .world_mut()
        .resource_mut::<crate::ui_bind_confirm::PendingEquips>()
        .add(crate::ui_bind_confirm::PendingEquip::Slot {
            bag: 0,
            slot: 1,
            dst: 15,
        });
    rig.lua(&format!(
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("ITEM_LOCKED")
        f:SetScript("OnEvent", function() UseAction(1) end)
        EquipPendingItem({index})"#
    ));
    rig.app
        .world_mut()
        .run_system_once(crate::ui_items::drain_bind_confirm_answers)
        .expect("the answers drain");
    let sends: Vec<&str> = rig
        .sent()
        .into_iter()
        .filter_map(|c| match c {
            ClientCommand::CastSpell { .. } => Some("cast"),
            ClientCommand::AutoEquipItemSlot { .. } => Some("equip"),
            _ => None,
        })
        .collect();
    assert_eq!(sends, ["cast", "equip"]);
}

/// A call made where no hook is lent waits; the next call made under one applies it first, so the
/// two keep call order: the cast goes to the target the earlier call selected.
#[test]
fn a_queued_call_applies_before_the_in_call_one() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.lua(TICK_DO);
    rig.lua(r#"TargetUnit("party1")"#);
    assert_eq!(rig.selected(), Some(OLD), "no hook lent: the call waits");
    rig.lua(r#"TICK_DO = function() CastSpellByName("Bolt") end"#);
    rig.frame();
    assert_eq!(rig.casts(), [(BOLT, Some(NEW))]);
}

/// A macro's lines each apply their call as the line runs (`0x564e5d`), so a line after `/target`
/// reads the new target.
#[test]
fn a_macro_lines_cast_reads_back_in_the_next_line() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.lua(TICK_DO);
    rig.lua(
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("EXECUTE_CHAT_LINE")
        f:SetScript("OnEvent", function(self, event, arg1) RunScript(arg1) end)
        MACRO_INDEX = CreateMacro("probe", 1,
            'TargetUnit("party1")\nREAD = UnitGUID("target")\nUseAction(1)\nCD = GetActionCooldown(1)')"#,
    );
    let index: u32 = rig.vm().eval("return MACRO_INDEX").expect("an index");
    rig.app
        .world_mut()
        .resource_mut::<crate::ui_action::ActionSlots>()
        .buttons
        .insert(
            2,
            ActionButton {
                slot: 2,
                action: index,
                kind: ACTION_KIND_MACRO,
            },
        );
    rig.lua("TICK_DO = function() UseAction(3) end");
    rig.frame();
    let read: String = rig.vm().eval("return READ").expect("a guid");
    assert_eq!(
        read,
        format!("0x{NEW:016X}"),
        "the line after /target reads it"
    );
    let cd: f64 = rig.vm().eval("return CD").expect("a start");
    assert!(cd > 0.0, "the line after the cast reads its GCD: {cd}");
    assert_eq!(rig.casts(), [(BOLT, Some(NEW))]);
}

/// The self-pressing macro below run once, on a thread with `stack` bytes of stack: the depth it
/// reached, the depth one frame later, and the error the error handler saw.
fn self_pressing_macro(stack: usize) -> Option<(u32, u32, String)> {
    std::thread::Builder::new()
        .stack_size(stack)
        .spawn(|| {
            let mut rig = Rig::new(&[Step::Tick])?;
            rig.lua(TICK_DO);
            rig.lua(
                r#"local f = CreateFrame("Frame")
                f:RegisterEvent("EXECUTE_CHAT_LINE")
                f:SetScript("OnEvent", function(self, event, arg1) assert(loadstring(arg1))() end)
                seterrorhandler(function(msg) ERR = ERR or tostring(msg) end)
                DEPTH = 0
                MACRO_INDEX = CreateMacro("probe", 1, 'DEPTH = DEPTH + 1\nUseAction(3)')"#,
            );
            let index: u32 = rig.vm().eval("return MACRO_INDEX").expect("an index");
            rig.app
                .world_mut()
                .resource_mut::<crate::ui_action::ActionSlots>()
                .buttons
                .insert(
                    2,
                    ActionButton {
                        slot: 2,
                        action: index,
                        kind: ACTION_KIND_MACRO,
                    },
                );
            rig.lua("TICK_DO = function() UseAction(3) end");
            rig.frame();
            let depth: u32 = rig.vm().eval("return DEPTH").expect("a depth");
            rig.frame();
            let again: u32 = rig.vm().eval("return DEPTH").expect("a depth");
            let err: String = rig.vm().eval("return ERR or ''").expect("an error");
            Some((depth, again, err))
        })
        .expect("a thread")
        .join()
        .expect("no panic")
}

/// A macro that presses itself recurses as 12340's runner does, which keeps no depth of its own
/// (`0x564db0` saves and restores the running macro, nothing more): Lua's C-call limit ends it
/// with its stack error inside the call, on a stack the size of the client's main thread, and the
/// frame goes on. The handler compiles each line itself, so the recursion stays secure: a
/// `RunScript` line runs under the macro taint, whose nested run is refused (kind 3).
#[test]
fn a_self_pressing_macro_ends_in_the_lua_stack_error() {
    let Some((depth, again, err)) = self_pressing_macro(crate::MAIN_THREAD_STACK) else {
        eprintln!("skipped: this build carries no system names");
        return;
    };
    // Lua 5.1 counts C calls in `nCcalls` (`0x856760`); hitting `LUAI_MAXCCALLS` raises either
    // `"C stack overflow"` on a call or `"chunk has too many syntax levels"` inside the parser.
    assert!(
        err.ends_with("C stack overflow") || err.ends_with("chunk has too many syntax levels"),
        "Lua's C-call limit: {err:?}"
    );
    assert!(depth > 8, "past the old cap: {depth}");
    assert_eq!(again, depth, "nothing waits for the next frame");
}

/// The binary links the main thread's stack the self-pressing macro above runs on.
#[test]
fn the_binary_links_the_main_thread_stack() {
    let build = include_str!("../../../benilla/build.rs");
    let linked = build
        .lines()
        .find_map(|l| l.strip_prefix("const MAIN_THREAD_STACK: usize = "))
        .and_then(|v| {
            v.trim_end_matches(';')
                .replace('_', "")
                .parse::<usize>()
                .ok()
        });
    assert_eq!(linked, Some(crate::MAIN_THREAD_STACK));
}

/// `PetAttack` then `IsPetAttackActive` in one `OnUpdate`: the order raises the attacking global
/// inside the call (`0x5d4509`).
#[test]
fn pet_attack_then_is_pet_attack_active_in_one_chunk() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.lua(TICK_DO);
    rig.lua("TICK_DO = function() PetAttack() READ = IsPetAttackActive() end");
    rig.frame();
    let read: bool = rig.vm().eval("return READ == true").expect("a flag");
    assert!(read, "the attacking global is up before the verb returns");
}

/// `CastPetAction` then `GetPetActionCooldown` in one `OnUpdate`: the pet's cooldown starts and is
/// flushed inside the press (`0x5d4427` → `0x805d70`, `0x805eda`).
#[test]
fn cast_pet_action_then_get_pet_action_cooldown_in_one_chunk() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.lua(TICK_DO);
    rig.lua("TICK_DO = function() CastPetAction(1) READ = GetPetActionCooldown(1) end");
    rig.frame();
    let start: f64 = rig.vm().eval("return READ").expect("a start");
    assert!(start > 0.0, "the pet's GCD reads in the same call: {start}");
}

/// `SMSG_SPELL_FAILURE` for our sent cast: the end's listeners read the cast still pending, and the finish releases it after them (`0x809db3` -> `0x7fecc0`, then `0x809dc1` -> `0x8054f0`, `0x8055a2`).
#[test]
fn a_spell_failures_end_listener_reads_the_cast_still_pending() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.lua(TICK_DO);
    rig.lua(
        r#"SEEN = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("UNIT_SPELLCAST_FAILED")
        f:SetScript("OnEvent", function(_, e, unit)
            SEEN[#SEEN + 1] = e .. " " .. unit .. " current=" .. (IsCurrentAction(1) and 1 or 0)
        end)
        TICK_DO = function() UseAction(1) TICK_DO = nil end"#,
    );
    rig.frame();
    let count = rig
        .sent()
        .into_iter()
        .find_map(|c| match c {
            ClientCommand::CastSpell {
                spell_id: BOLT,
                count,
                ..
            } => Some(count),
            _ => None,
        })
        .expect("the press sent the cast");
    let before: String = crate::ui_script::live::with_live_world(rig.app.world_mut(), |s| {
        s.eval("return tostring(IsCurrentAction(1) and 1 or 0)")
    })
    .unwrap()
    .unwrap();
    assert_eq!(before, "1", "the sent cast is pending");
    crate::net::handlers::dispatch(
        rig.app.world_mut(),
        vec![benilla_protocol::SessionEvent::SpellFailed {
            other: false,
            caster: ME,
            cast_count: count,
            spell_id: BOLT,
            reason: 0x28,
            wire_reason: 0x28,
        }],
    );
    let seen: String = crate::ui_script::live::with_live_world(rig.app.world_mut(), |s| {
        s.eval("return table.concat(SEEN, ',') .. ' after=' .. (IsCurrentAction(1) and 1 or 0)")
    })
    .unwrap()
    .unwrap();
    assert_eq!(seen, "UNIT_SPELLCAST_FAILED player current=1 after=0");
}

/// Bolt chained to Nova (column 38), Nova ready, Bolt sent from action slot 1; returns the cast's count.
fn chained_bolt_rig(rig: &mut Rig) -> u8 {
    {
        let world = rig.app.world_mut();
        let mut spells = world.resource_mut::<crate::ui_action::Spells>();
        spells.catalog = SpellCatalog::from_displays(HashMap::from([
            (
                BOLT,
                SpellDisplay {
                    name: "Bolt".into(),
                    implicit_target_a1: 6,
                    start_recovery_category: 133,
                    start_recovery_ms: 1500,
                    modal_next_spell: NOVA,
                    ..Default::default()
                },
            ),
            (
                NOVA,
                SpellDisplay {
                    name: "Nova".into(),
                    ..Default::default()
                },
            ),
        ]));
        world.resource_mut::<crate::spell::Cooldowns>().wipe();
    }
    rig.lua(TICK_DO);
    rig.lua(
        r#"SEEN = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("UNIT_SPELLCAST_SENT")
        f:RegisterEvent("CURRENT_SPELL_CAST_CHANGED")
        f:SetScript("OnEvent", function(_, e, unit, spell)
            if not ARMED then return end
            SEEN[#SEEN + 1] = e .. " " .. (spell or "") .. " bolt=" .. (IsCurrentAction(1) and 1 or 0)
        end)
        TICK_DO = function() UseAction(1) TICK_DO = nil end"#,
    );
    rig.frame();
    rig.lua("ARMED = true");
    rig.sent()
        .into_iter()
        .find_map(|c| match c {
            ClientCommand::CastSpell {
                spell_id: BOLT,
                count,
                ..
            } => Some(count),
            _ => None,
        })
        .expect("the press sent the cast")
}

/// The finish's order on our `SMSG_SPELL_GO` (`0x80fe9d` -> `0x8054f0`): the chained cast's `UNIT_SPELLCAST_SENT` listener reads the record still held (`0x80559a`), the selection update's after the release (`0x8055a2`, `0x8055b5`).
#[test]
fn a_go_chains_before_it_releases_then_updates_the_selection() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    let count = chained_bolt_rig(&mut rig);
    crate::net::handlers::dispatch(
        rig.app.world_mut(),
        vec![benilla_protocol::SessionEvent::SpellGo {
            caster: ME,
            cast_count: count,
            spell_id: BOLT,
            cast_flags: 0,
            server_time: 0,
            hits: vec![],
            misses: vec![],
            reflects: vec![],
            target: None,
            go_target: None,
            target_mask: 0,
            src: None,
            dest: None,
            ammo: None,
            item_caster: None,
            trajectory: None,
            dest_counter: 0,
            points: vec![],
        }],
    );
    let seen: String = crate::ui_script::live::with_live_world(rig.app.world_mut(), |s| {
        s.eval("return table.concat(SEEN, ',')")
    })
    .unwrap()
    .unwrap();
    assert_eq!(
        seen,
        "UNIT_SPELLCAST_SENT Nova bolt=1,CURRENT_SPELL_CAST_CHANGED  bolt=1,CURRENT_SPELL_CAST_CHANGED  bolt=0"
    );
    assert!(
        rig.sent()
            .iter()
            .any(|c| matches!(c, ClientCommand::CastSpell { spell_id: NOVA, .. })),
        "the chain went out"
    );
}

/// The same finish on `SMSG_CAST_FAILED` (`0x808b7f` -> `0x8054f0`): chain, release, then the selection update.
#[test]
fn a_cast_failure_chains_before_it_releases_then_updates_the_selection() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    let count = chained_bolt_rig(&mut rig);
    crate::net::handlers::dispatch(
        rig.app.world_mut(),
        vec![benilla_protocol::SessionEvent::CastResult {
            cast_count: count,
            spell_id: BOLT,
            success: false,
            reason: Some(0x28),
            arg: None,
            arg2: None,
            wire_reason: Some(0x28),
        }],
    );
    let seen: String = crate::ui_script::live::with_live_world(rig.app.world_mut(), |s| {
        s.eval("return table.concat(SEEN, ',')")
    })
    .unwrap()
    .unwrap();
    assert_eq!(
        seen,
        "UNIT_SPELLCAST_SENT Nova bolt=1,CURRENT_SPELL_CAST_CHANGED  bolt=1,CURRENT_SPELL_CAST_CHANGED  bolt=0"
    );
}

/// `SMSG_CAST_FAILED` for our sent cast: the line's listener reads the cast still pending; the finish releases it after the line and the end (`0x808b00`, `0x808b75`, then `0x808b7f` -> `0x8054f0`, `0x8055a2`).
#[test]
fn a_cast_failures_line_listener_reads_the_cast_still_pending() {
    let mut rig = rig_or_skip!(&[Step::Tick]);
    rig.lua(TICK_DO);
    rig.lua(
        r#"SEEN = {} ERR_ITEM_COOLDOWN = "Item is not ready yet."
        local f = CreateFrame("Frame")
        f:RegisterEvent("UI_ERROR_MESSAGE")
        f:SetScript("OnEvent", function(_, e)
            SEEN[#SEEN + 1] = e .. " current=" .. (IsCurrentAction(1) and 1 or 0)
        end)
        TICK_DO = function() UseAction(1) TICK_DO = nil end"#,
    );
    rig.frame();
    let count = rig
        .sent()
        .into_iter()
        .find_map(|c| match c {
            ClientCommand::CastSpell {
                spell_id: BOLT,
                count,
                ..
            } => Some(count),
            _ => None,
        })
        .expect("the press sent the cast");
    crate::net::handlers::dispatch(
        rig.app.world_mut(),
        vec![benilla_protocol::SessionEvent::CastResult {
            cast_count: count,
            spell_id: BOLT,
            success: false,
            reason: Some(0x28),
            arg: None,
            arg2: None,
            wire_reason: Some(0x28),
        }],
    );
    let seen: String = crate::ui_script::live::with_live_world(rig.app.world_mut(), |s| {
        s.eval("return table.concat(SEEN, ',') .. ' after=' .. (IsCurrentAction(1) and 1 or 0)")
    })
    .unwrap()
    .unwrap();
    assert_eq!(seen, "UI_ERROR_MESSAGE current=1 after=0");
}
