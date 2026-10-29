//! The functions the stock `Bindings.xml` bodies call, from the reference's three registrar tables:
//! the movement table, the camera table and the targeting and posture verbs of the main table. A
//! key and its Lua function share one path: a held command's Start and Stop are the key's press and
//! release ([`HeldInput`]), a one-shot command fires as its key does ([`FiredInput`]), and the
//! targeting and attack calls go through the call queue ([`ScriptCall`]), so a macro's
//! `TargetNearestEnemy()` then `/cast` casts at the new target.
//!
//! Every movement function but the mouselook three opens on the hardware-event gate with tag 0.
//! Untainted, it passes only inside a real input event, which for a binding body is the binding
//! executor on the stack; tainted code (a macro, `RunScript`, an addon's own) is refused. So a
//! key's body moves and a `/script MoveForwardStart()` does not. benilla has no taint model, so
//! here the gate is "a binding body is running" ([`UiScript::execute_binding`]), which refuses the
//! same callers, silently where the reference fires `MACRO_ACTION_FORBIDDEN` (431) or
//! `ADDON_ACTION_FORBIDDEN` (432). The camera table and the main-table verbs have no gate.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{bool_or_default, flag};
use super::calls::{NearestMode, ScriptCall};
use super::UiScript;

/// A held movement command: one bit of the reference's `[InputControl+4]` word, set by its Start
/// and cleared by its Stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeldInput {
    /// `0x10`, `MoveForwardStart`/`Stop`.
    MoveForward,
    /// `0x20`, `MoveBackwardStart`/`Stop`.
    MoveBackward,
    /// `0x100`, `TurnLeftStart`/`Stop`.
    TurnLeft,
    /// `0x200`, `TurnRightStart`/`Stop`.
    TurnRight,
    /// `0x40`, `StrafeLeftStart`/`Stop`.
    StrafeLeft,
    /// `0x80`, `StrafeRightStart`/`Stop`.
    StrafeRight,
    /// `0x1`, `TurnOrActionStart`/`Stop`, the right mouse button's bit.
    TurnOrAction,
    /// `0x2`, `CameraOrSelectOrMoveStart`/`Stop`, the left button's.
    CameraOrSelectOrMove,
    /// 3.3.5a's `0x2000`, set by `JumpOrAscendStart` (`0x5fbf80`, `0x5fa170(0x2000)`) and cleared
    /// by `AscendStop` (`0x5fc0a0`, `0x5fa450(0x2000)`): the up half of the vertical channel.
    Ascend,
    /// 3.3.5a's `0x4000`, set by `SitStandOrDescendStart` (`0x51b1d0` → `0x5fc0f0`) and cleared
    /// by `DescendStop` (`0x5fc140`): the down half of the vertical channel.
    Descend,
}

/// A one-shot command, fired as its key fires it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FiredInput {
    /// The ground arm of `JumpOrAscendStart()` (`0x5fbf80`): a jump, which the engine takes only
    /// off a mover neither swimming nor flying (`0x5fbfed` tests `0x2200000`).
    Jump,
    /// `ToggleRun()`, the walk toggle.
    ToggleRun,
    /// `ToggleAutoRun()`, which toggles `0x1000`.
    ToggleAutoRun,
    /// The posture arm of `SitStandOrDescendStart()` (`0x51b1d0`): sit when standing, else stand
    /// (`0x6dcb40(1)` or `(0)` off the stand byte `[+0xd0]+0x110`), skipped while flying.
    SitOrStand,
    /// `ToggleSheath()`.
    ToggleSheath,
    /// `CameraZoomIn([yards])`, 1.0 when absent.
    CameraZoomIn(f32),
    /// `CameraZoomOut([yards])`, 1.0 when absent.
    CameraZoomOut(f32),
}

/// One call for the app's binding state, in call order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BindingInput {
    Start(HeldInput),
    /// A Stop and the control-bit clear's keep-smoothing argument (`0x5fa450`), set only by
    /// `CameraOrSelectOrMoveStop`'s sticky flag: the camera then skips its return behind the
    /// character until a look ends without it (`0x604850` sets the camera's `0x20`).
    Stop(HeldInput, bool),
    Fire(FiredInput),
}

/// The VM's half of the binding functions.
#[derive(Default)]
pub(crate) struct InputState {
    /// The calls made since the app last drained them.
    queue: Vec<BindingInput>,
    /// How many binding bodies are running: the hardware-event gate's.
    binding_depth: u32,
    /// `[InputControl+4] & 1`, the TurnOrAction channel `IsMouselooking` tests, as the app pushed
    /// it or a `MouselookStart`/`Stop` since moved it.
    turn_or_action: bool,
    /// The `MouselookStart` (`true`) and `MouselookStop` calls since the app last drained them.
    mouselook: Vec<bool>,
}

impl UiScript {
    /// Take the binding calls queued since the last call, in call order.
    pub fn take_binding_input(&mut self) -> Vec<BindingInput> {
        drain_field!(self, input.queue)
    }

    /// Run a binding body as the binding executor's command run does: the movement functions'
    /// hardware-event gate passes while it runs.
    pub fn run_binding(&self, chunk: &str) -> mlua::Result<()> {
        self.model_mut().input.binding_depth += 1;
        let result = self.run(chunk);
        self.model_mut().input.binding_depth -= 1;
        result
    }

    /// The binding executor once the chord has resolved to `command`: its command run with the
    /// hardware-event gate raised, a press when `down`, else the release, which runs only a
    /// `runOnUp` body. `Ok(false)` when nothing ran: no such command, or a release of one that is
    /// not `runOnUp`.
    pub fn execute_binding(&self, command: &str, down: bool) -> mlua::Result<bool> {
        self.model_mut().input.binding_depth += 1;
        let result = super::keybind::run_command(&self.lua, command, down);
        self.model_mut().input.binding_depth -= 1;
        result
    }

    /// Push whether the TurnOrAction channel is held: the world holds the right mouse button, or
    /// a `MouselookStart` holds it with none.
    pub fn set_turn_or_action_held(&mut self, held: bool) {
        set_field!(self, input.turn_or_action, held);
    }

    /// Take the `MouselookStart` (`true`) and `MouselookStop` calls made since the last call, in
    /// call order, for the app's look session.
    pub fn take_mouselook_calls(&mut self) -> Vec<bool> {
        drain_field!(self, input.mouselook)
    }
}

/// The hardware-event gate: inside a binding body.
pub(super) fn in_binding(lua: &Lua) -> bool {
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    model.input.binding_depth > 0
}

fn push(lua: &Lua, input: BindingInput) {
    let mut model = crate::script::model_mut_of(lua);
    model.input.queue.push(input);
}

fn push_call(lua: &Lua, call: ScriptCall) {
    let mut model = crate::script::model_mut_of(lua);
    model.script_calls.push(call);
}

/// A zoom's optional amount: `is-number` (a numeric string passes) then `tonumber`, else the stored
/// `1.0f`. A NaN or infinite amount is dropped: what the reference's tick conversion makes of it is
/// not established.
fn zoom_amount(lua: &Lua, v: Option<Value>) -> Option<f32> {
    let n = match v {
        Some(v) => lua.coerce_number(v).ok().flatten().unwrap_or(1.0),
        None => 1.0,
    } as f32;
    n.is_finite().then_some(n)
}

/// Register a function that takes any arguments and does nothing: the mechanism behind it is not
/// built, so the call is answered with the reference's zero results and no effect.
fn inert(lua: &Lua, names: &[&str]) -> mlua::Result<()> {
    let g = lua.globals();
    for name in names {
        g.set(*name, lua.create_function(|_, _: MultiValue| Ok(()))?)?;
    }
    Ok(())
}

/// Register the binding functions: the three tables' and `TargetNearestFriend`, with 3.3.5a's
/// vertical four in place of 1.12's `Jump` and `SitOrStand`.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ── The movement table ────────────────────────────────────────────────────── Each Start/Stop,
    // then `(bit, set, time, 0)`; nothing is returned, and no argument is read but
    // `CameraOrSelectOrMoveStop`'s sticky flag (`0x5fc730` reads argument 1 as a boolean with
    // default 0, at `0x5fc757`), which rides its Stop.
    let held: [(&str, &str, HeldInput); 8] = [
        (
            "MoveForwardStart",
            "MoveForwardStop",
            HeldInput::MoveForward,
        ),
        (
            "MoveBackwardStart",
            "MoveBackwardStop",
            HeldInput::MoveBackward,
        ),
        ("TurnLeftStart", "TurnLeftStop", HeldInput::TurnLeft),
        ("TurnRightStart", "TurnRightStop", HeldInput::TurnRight),
        ("StrafeLeftStart", "StrafeLeftStop", HeldInput::StrafeLeft),
        (
            "StrafeRightStart",
            "StrafeRightStop",
            HeldInput::StrafeRight,
        ),
        // The mouse pair also arms the world click; that, and each bit alone, the look session
        // reads off the physical buttons. The two held together are the both-button run, which
        // MOVEANDSTEER's body reaches through them.
        (
            "TurnOrActionStart",
            "TurnOrActionStop",
            HeldInput::TurnOrAction,
        ),
        (
            "CameraOrSelectOrMoveStart",
            "CameraOrSelectOrMoveStop",
            HeldInput::CameraOrSelectOrMove,
        ),
    ];
    for (start, stop, input) in held {
        g.set(
            start,
            lua.create_function(move |lua, _: MultiValue| {
                if in_binding(lua) {
                    push(lua, BindingInput::Start(input));
                }
                Ok(())
            })?,
        )?;
        g.set(
            stop,
            lua.create_function(move |lua, args: MultiValue| {
                if in_binding(lua) {
                    let sticky = input == HeldInput::CameraOrSelectOrMove
                        && bool_or_default(args.front(), false);
                    push(lua, BindingInput::Stop(input, sticky));
                }
                Ok(())
            })?,
        )?;
    }
    // `ToggleRun` and `ToggleAutoRun` open on the same gate; their further refusals (a dead or
    // rooted player) are the engine's, as for the key.
    let fired: [(&str, FiredInput); 2] = [
        ("ToggleRun", FiredInput::ToggleRun),
        ("ToggleAutoRun", FiredInput::ToggleAutoRun),
    ];
    for (name, input) in fired {
        g.set(
            name,
            lua.create_function(move |lua, _: MultiValue| {
                if in_binding(lua) {
                    push(lua, BindingInput::Fire(input));
                }
                Ok(())
            })?,
        )?;
    }
    // 3.3.5a's vertical pair, which replaced 1.12's one-shot `Jump`, each a zero-result call
    // behind the hardware-event gate (tag 0). `JumpOrAscendStart` (`0x5fbf80`)
    // sets the ascend bit, then jumps off a mover neither swimming nor flying; `AscendStop`
    // (`0x5fc0a0`) and `DescendStop` (`0x5fc140`) clear their bit. Each bit change resyncs the
    // vertical channel (`0x5fbbc0` → `0x5face0`), which only a swimming or flying mover reads.
    g.set(
        "JumpOrAscendStart",
        lua.create_function(|lua, _: MultiValue| {
            if in_binding(lua) {
                push(lua, BindingInput::Start(HeldInput::Ascend));
                push(lua, BindingInput::Fire(FiredInput::Jump));
            }
            Ok(())
        })?,
    )?;
    for (name, input) in [
        ("AscendStop", HeldInput::Ascend),
        ("DescendStop", HeldInput::Descend),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, _: MultiValue| {
                if in_binding(lua) {
                    push(lua, BindingInput::Stop(input, false));
                }
                Ok(())
            })?,
        )?;
    }
    // `IsMouselooking()`: `[InputControl+4] & 1`, the TurnOrAction channel, not the camera's
    // freelook bit; 1 or nil.
    g.set(
        "IsMouselooking",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(model.input.turn_or_action))
        })?,
    )?;
    // `MouselookStart`/`MouselookStop` pass no gate: each disarms the pending world click and sets
    // or clears the TurnOrAction channel (`(1, set, now, 0)`), the bit the held right button
    // drives. The bit moves at the call, so `IsMouselooking` answers it at once; the app's look
    // session applies the call.
    for (name, on) in [("MouselookStart", true), ("MouselookStop", false)] {
        g.set(
            name,
            lua.create_function(move |lua, _: MultiValue| {
                let mut model = crate::script::model_mut_of(lua);
                model.input.turn_or_action = on;
                model.input.mouselook.push(on);
                Ok(())
            })?,
        )?;
    }
    // Not built, so these answer and do nothing: the pitch pair (`0x400`/`0x800`), since the
    // mover has no keyboard pitch axis, so PITCHUP and PITCHDOWN (INSERT, DELETE) do nothing.
    inert(
        lua,
        &[
            "PitchUpStart",
            "PitchUpStop",
            "PitchDownStart",
            "PitchDownStop",
        ],
    )?;

    // ── The camera table ────────────────────────────────────────────────────────
    g.set(
        "CameraZoomIn",
        lua.create_function(|lua, v: Option<Value>| {
            if let Some(yards) = zoom_amount(lua, v) {
                push(lua, BindingInput::Fire(FiredInput::CameraZoomIn(yards)));
            }
            Ok(())
        })?,
    )?;
    g.set(
        "CameraZoomOut",
        lua.create_function(|lua, v: Option<Value>| {
            if let Some(yards) = zoom_amount(lua, v) {
                push(lua, BindingInput::Fire(FiredInput::CameraZoomOut(yards)));
            }
            Ok(())
        })?,
    )?;
    // Not built: the `MoveView*` pairs (`(direction, time, 0)` and `(direction, time)` for In, Out,
    // Right, Left, Up, Down = 0..5), whose effect on the camera is not established; 1.12's own
    // `MOVEVIEW*` bindings are commented out. And `ToggleMouseMove`, which toggles freelook with no
    // button held.
    inert(
        lua,
        &[
            "MoveViewInStart",
            "MoveViewInStop",
            "MoveViewOutStart",
            "MoveViewOutStop",
            "MoveViewRightStart",
            "MoveViewRightStop",
            "MoveViewLeftStart",
            "MoveViewLeftStop",
            "MoveViewUpStart",
            "MoveViewUpStop",
            "MoveViewDownStart",
            "MoveViewDownStop",
            "ToggleMouseMove",
        ],
    )?;

    // ── The main table ──────────────────────────────────────────────────────────────────────
    // The four `TargetNearest*` shims are byte-identical but for the mode: argument 1, read as a
    // boolean with default 0, is the reverse flag, then the nearest-target call takes
    // `(reverse, mode)`.
    let nearest: [(&str, NearestMode); 4] = [
        ("TargetNearestEnemy", NearestMode::Enemy),
        ("TargetNearestFriend", NearestMode::Friend),
        ("TargetNearestPartyMember", NearestMode::PartyMember),
        ("TargetNearestRaidMember", NearestMode::RaidMember),
    ];
    for (name, mode) in nearest {
        g.set(
            name,
            lua.create_function(move |lua, reverse: Option<Value>| {
                let reverse = bool_or_default(reverse.as_ref(), false);
                push_call(lua, ScriptCall::TargetNearest { mode, reverse });
                Ok(())
            })?,
        )?;
    }
    // `TargetLastTarget()` and `AttackTarget()` read no argument.
    g.set(
        "TargetLastTarget",
        lua.create_function(|lua, _: MultiValue| {
            push_call(lua, ScriptCall::TargetLastTarget);
            Ok(())
        })?,
    )?;
    g.set(
        "AttackTarget",
        lua.create_function(|lua, _: MultiValue| {
            push_call(lua, ScriptCall::AttackTarget);
            Ok(())
        })?,
    )?;
    // `ToggleSheath()` resolves the active player and acts on it, ungated.
    g.set(
        "ToggleSheath",
        lua.create_function(|lua, _: MultiValue| {
            push(lua, BindingInput::Fire(FiredInput::ToggleSheath));
            Ok(())
        })?,
    )?;
    // `SitStandOrDescendStart()` (`0x51b1d0`): the descend bit through `0x5fc0f0`, behind the
    // hardware-event gate, then the stand toggle on the active player, which has none.
    g.set(
        "SitStandOrDescendStart",
        lua.create_function(|lua, _: MultiValue| {
            if in_binding(lua) {
                push(lua, BindingInput::Start(HeldInput::Descend));
            }
            push(lua, BindingInput::Fire(FiredInput::SitOrStand));
            Ok(())
        })?,
    )?;
    // `DetectWowMouse` (`0x5fa050`) probes for a WowMouse device and returns nothing; benilla
    // drives none.
    inert(lua, &["DetectWowMouse"])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The binding functions, each a callable global that answers without raising.
    const NAMES: [&str; 50] = [
        "JumpOrAscendStart",
        "AscendStop",
        "SitStandOrDescendStart",
        "DescendStop",
        "ToggleRun",
        "ToggleAutoRun",
        "MoveForwardStart",
        "MoveForwardStop",
        "MoveBackwardStart",
        "MoveBackwardStop",
        "TurnLeftStart",
        "TurnLeftStop",
        "TurnRightStart",
        "TurnRightStop",
        "StrafeLeftStart",
        "StrafeLeftStop",
        "StrafeRightStart",
        "StrafeRightStop",
        "PitchUpStart",
        "PitchUpStop",
        "PitchDownStart",
        "PitchDownStop",
        "TurnOrActionStart",
        "TurnOrActionStop",
        "CameraOrSelectOrMoveStart",
        "CameraOrSelectOrMoveStop",
        "MouselookStart",
        "MouselookStop",
        "IsMouselooking",
        "CameraZoomIn",
        "CameraZoomOut",
        "MoveViewInStart",
        "MoveViewInStop",
        "MoveViewOutStart",
        "MoveViewOutStop",
        "MoveViewRightStart",
        "MoveViewRightStop",
        "MoveViewLeftStart",
        "MoveViewLeftStop",
        "MoveViewUpStart",
        "MoveViewUpStop",
        "MoveViewDownStart",
        "MoveViewDownStop",
        "ToggleMouseMove",
        "TargetNearestEnemy",
        "TargetNearestPartyMember",
        "TargetNearestRaidMember",
        "TargetLastTarget",
        "AttackTarget",
        "ToggleSheath",
    ];

    #[test]
    fn every_binding_function_is_a_global_that_answers() {
        let s = UiScript::new().unwrap();
        for name in NAMES {
            let kind: String = s.eval(&format!("return type({name})")).unwrap();
            assert_eq!(kind, "function", "{name}");
            s.run(&format!("{name}()"))
                .unwrap_or_else(|e| panic!("{name}() raised: {e}"));
            s.run_binding(&format!("{name}(1)"))
                .unwrap_or_else(|e| panic!("{name}(1) in a binding raised: {e}"));
        }
    }

    /// `CameraOrSelectOrMoveStop(sticky)` hands its flag through the boolean coercion with default
    /// 0 (`0x5fc757`); every other Stop passes 0, whatever it is given.
    #[test]
    fn the_camera_stop_carries_its_sticky_flag() {
        let mut s = UiScript::new().unwrap();
        s.run_binding(
            "CameraOrSelectOrMoveStop() CameraOrSelectOrMoveStop(1) \
             CameraOrSelectOrMoveStop(nil) TurnOrActionStop(1)",
        )
        .unwrap();
        assert_eq!(
            s.take_binding_input(),
            vec![
                BindingInput::Stop(HeldInput::CameraOrSelectOrMove, false),
                BindingInput::Stop(HeldInput::CameraOrSelectOrMove, true),
                BindingInput::Stop(HeldInput::CameraOrSelectOrMove, false),
                BindingInput::Stop(HeldInput::TurnOrAction, false),
            ]
        );
    }

    /// The gate: a movement call queues only from inside a binding body, while the camera and
    /// posture verbs, which have no gate, queue from anywhere.
    #[test]
    fn the_movement_calls_act_only_inside_a_binding_body() {
        let mut s = UiScript::new().unwrap();
        s.run("MoveForwardStart() StrafeLeftStop() JumpOrAscendStart() ToggleRun()")
            .unwrap();
        assert!(s.take_binding_input().is_empty());
        s.run_binding("MoveForwardStart() StrafeLeftStop() JumpOrAscendStart() ToggleRun()")
            .unwrap();
        assert_eq!(
            s.take_binding_input(),
            vec![
                BindingInput::Start(HeldInput::MoveForward),
                BindingInput::Stop(HeldInput::StrafeLeft, false),
                BindingInput::Start(HeldInput::Ascend),
                BindingInput::Fire(FiredInput::Jump),
                BindingInput::Fire(FiredInput::ToggleRun),
            ]
        );
        // The gate closes again after the body, and after a body that raised.
        assert!(s.run_binding("error('boom')").is_err());
        s.run("TurnLeftStart()").unwrap();
        assert!(s.take_binding_input().is_empty());
        s.run("SitStandOrDescendStart() ToggleSheath() CameraZoomIn()")
            .unwrap();
        assert_eq!(
            s.take_binding_input(),
            vec![
                BindingInput::Fire(FiredInput::SitOrStand),
                BindingInput::Fire(FiredInput::ToggleSheath),
                BindingInput::Fire(FiredInput::CameraZoomIn(1.0)),
            ]
        );
    }

    /// The stock 3.3.5a `JUMP` and `SITORSTAND` bodies (`runOnUp`) on both key edges: the press
    /// starts its half of the vertical channel (the jump and the stand toggle beside it), the
    /// release stops it and fires nothing; 1.12's one-shot `Jump` and `SitOrStand` are gone.
    #[test]
    fn the_vertical_pair_starts_on_the_press_and_stops_on_the_release() {
        const JUMP: &str = r#"if ( keystate == "down" ) then
                JumpOrAscendStart();
            else
                AscendStop();
            end"#;
        const SITORSTAND: &str = r#"if ( keystate == "down" ) then
                SitStandOrDescendStart();
            else
                DescendStop();
            end"#;
        let mut s = UiScript::new().unwrap();
        for name in [
            "JumpOrAscendStart()",
            "AscendStop()",
            "SitStandOrDescendStart()",
            "DescendStop()",
        ] {
            assert_eq!(s.arity(name).unwrap(), 0, "{name} returns nothing");
        }
        // The arity probe ran each call once; its queued inputs are not the edges under test.
        s.take_binding_input();
        let edge = |s: &mut UiScript, body: &str, keystate: &str| {
            s.run_binding(&format!("local keystate = {keystate:?} {body}"))
                .unwrap();
            s.take_binding_input()
        };
        assert_eq!(
            edge(&mut s, JUMP, "down"),
            vec![
                BindingInput::Start(HeldInput::Ascend),
                BindingInput::Fire(FiredInput::Jump),
            ]
        );
        assert_eq!(
            edge(&mut s, JUMP, "up"),
            vec![BindingInput::Stop(HeldInput::Ascend, false)],
            "the release stops the ascent and jumps no second time"
        );
        assert_eq!(
            edge(&mut s, SITORSTAND, "down"),
            vec![
                BindingInput::Start(HeldInput::Descend),
                BindingInput::Fire(FiredInput::SitOrStand),
            ]
        );
        assert_eq!(
            edge(&mut s, SITORSTAND, "up"),
            vec![BindingInput::Stop(HeldInput::Descend, false)]
        );
        for gone in ["Jump", "SitOrStand"] {
            assert_eq!(
                s.eval::<String>(&format!("return type({gone})")).unwrap(),
                "nil",
                "{gone} is 1.12.1's, absent from 3.3.5a"
            );
        }
    }

    /// The zoom's amount: a number or numeric string, else 1.0; a non-finite amount is dropped.
    #[test]
    fn a_zoom_takes_its_yards_or_one() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"CameraZoomIn(2.5) CameraZoomOut("4") CameraZoomIn("far") CameraZoomOut(0/0)"#)
            .unwrap();
        assert_eq!(
            s.take_binding_input(),
            vec![
                BindingInput::Fire(FiredInput::CameraZoomIn(2.5)),
                BindingInput::Fire(FiredInput::CameraZoomOut(4.0)),
                BindingInput::Fire(FiredInput::CameraZoomIn(1.0)),
            ]
        );
    }

    /// `IsMouselooking` answers the pushed TurnOrAction channel as 1 or nil.
    #[test]
    fn is_mouselooking_answers_one_or_nil() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.arity("IsMouselooking()").unwrap(), 1);
        assert!(s
            .eval::<Option<i64>>("return IsMouselooking()")
            .unwrap()
            .is_none());
        s.set_turn_or_action_held(true);
        assert_eq!(
            s.eval::<Option<i64>>("return IsMouselooking()").unwrap(),
            Some(1)
        );
    }

    /// `MouselookStart`/`Stop` pass no gate, queue for the look session in call order, and move the
    /// channel at the call, so `IsMouselooking` answers them at once.
    #[test]
    fn mouselook_start_and_stop_move_the_channel_from_any_caller() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.arity("MouselookStart()").unwrap(), 0);
        s.run("MouselookStop()").unwrap();
        s.take_mouselook_calls();
        let looking = |s: &UiScript| s.eval::<Option<i64>>("return IsMouselooking()").unwrap();
        s.run("MouselookStart()").unwrap();
        assert_eq!(
            looking(&s),
            Some(1),
            "an addon's own call, outside any binding"
        );
        s.run("MouselookStop() MouselookStart() MouselookStop()")
            .unwrap();
        assert_eq!(looking(&s), None);
        assert_eq!(s.take_mouselook_calls(), vec![true, false, true, false]);
        assert!(s.take_mouselook_calls().is_empty(), "drained");
        assert!(
            s.take_binding_input().is_empty(),
            "not a movement-binding call"
        );
        // The app's push is the channel's truth from then on: a right release ended it.
        s.run("MouselookStart()").unwrap();
        s.set_turn_or_action_held(false);
        assert_eq!(looking(&s), None);
    }

    /// `TargetLastTarget` and `AttackTarget` join the call queue, in call order with the rest.
    #[test]
    fn the_targeting_calls_join_the_call_queue_in_order() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"TargetNearestEnemy(1) TargetLastTarget() AttackTarget() ClearTarget()"#)
            .unwrap();
        assert_eq!(
            s.take_script_calls(),
            vec![
                ScriptCall::TargetNearest {
                    mode: NearestMode::Enemy,
                    reverse: true
                },
                ScriptCall::TargetLastTarget,
                ScriptCall::AttackTarget,
                ScriptCall::ClearTarget,
            ]
        );
    }
}
