//! The in-game engine's debug, file and script-profiling verbs, which no stock interface file
//! calls: the empty debug toggles, the file verbs that refuse, the build flags, and the CPU
//! profiler's readers.
//!
//! 12340 profiles scripts only when the `scriptProfile` CVar was on at UI load (`0x81a2e0` passes
//! it to the VM's start, `0x819bb0`): the Lua state then times every function and the dispatcher
//! times every handler. With it off, every time and call count reads 0 (`0x8192e0`, and the Lua
//! function timer reads zeros for an unprofiled state). The one number kept regardless is each
//! event's signal count (`0x81aa00`). benilla registers no `scriptProfile` and so answers as
//! 12340 does with it off.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::bool_or_default;

/// The debug toggles and performance verbs 12340 binds to one empty body (`0x8e5250`): any
/// arguments, nothing returned.
const EMPTY: &[&str] = &[
    "QuestPOIUpdateTexture",
    "ResetPerformanceValues",
    "ToggleCollision",
    "ToggleCollisionDisplay",
    "TogglePerformanceDisplay",
    "TogglePerformancePause",
    "TogglePerformanceValues",
    "TogglePlayerBounds",
    "TogglePortals",
    "ToggleTris",
];

/// The file verbs and `GMRequestPlayerInfo`, which share one body (`0x510bb0`) that raises
/// `Access Denied`.
const DENIED: &[&str] = &[
    "AppendToFile",
    "DeleteFile",
    "GMRequestPlayerInfo",
    "ReadFile",
];

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    for name in EMPTY {
        g.set(*name, lua.create_function(|_, _: MultiValue| Ok(()))?)?;
    }
    for name in DENIED {
        g.set(
            *name,
            lua.create_function(|_, _: MultiValue| -> mlua::Result<()> {
                Err(mlua::Error::runtime("Access Denied"))
            })?,
        )?;
    }

    // `IsDebugBuild()` (`0x50fe80`): false in the shipped client.
    g.set(
        "IsDebugBuild",
        lua.create_function(|_, _: MultiValue| Ok(false))?,
    )?;
    // `IsDesaturateSupported()` (`0x511fb0`): 1 when the device has the desaturating pixel shader,
    // else nothing. benilla's texture shader always desaturates.
    g.set(
        "IsDesaturateSupported",
        lua.create_function(|_, _: MultiValue| Ok(1.0))?,
    )?;
    // `IsBNLogin()` (`0x530f20`): whether the login connection is a Battle.net one. A 3.3.5a
    // server's is the realm list's, so false.
    g.set(
        "IsBNLogin",
        lua.create_function(|_, _: MultiValue| Ok(false))?,
    )?;
    // `GetMovieResolution()`: the in-game binding of the glue's verb, the same body (`0x4dd610`).
    g.set(
        "GetMovieResolution",
        lua.create_function(|lua, _: MultiValue| Ok(super::glue_system::movie_resolution(lua)))?,
    )?;

    // `GetScriptCPUUsage()` (`0x511610`): the dispatcher's accrued milliseconds.
    g.set(
        "GetScriptCPUUsage",
        lua.create_function(|_, _: MultiValue| Ok(0.0))?,
    )?;
    // `ResetCPUUsage()` (`0x511830` → `0x81a4d0`): clears the script time, every event's time and
    // signal count, and the function timers.
    g.set(
        "ResetCPUUsage",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            model.event_signals.set(0);
            Ok(())
        })?,
    )?;
    // `GetEventCPUUsage(["event"])` (`0x5117c0` → `0x81a450`): milliseconds and signal count. The
    // reference reads its argument and then sums over every event, never the named one, so any
    // name, or none, answers the total.
    g.set(
        "GetEventCPUUsage",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok((0.0, f64::from(model.event_signals.get())))
        })?,
    )?;
    // `GetFrameCPUUsage(frame [, includeChildren])` (`0x517890`): milliseconds and calls of the
    // frame's script handlers, its children's too unless the flag reads false.
    g.set(
        "GetFrameCPUUsage",
        lua.create_function(|lua, args: MultiValue| {
            let Some(Value::Table(frame)) = args.front() else {
                return Err(mlua::Error::runtime(
                    "Usage: GetFrameCPUUsage(frame[, includeChildren])",
                ));
            };
            if super::object::decode_id(frame).is_err() {
                return Err(mlua::Error::runtime(
                    "GetFrameCPUUsage(): Couldn't find 'this' in frame object",
                ));
            }
            if super::object::frame_handle_of(lua, frame).is_err() {
                return Err(mlua::Error::runtime(
                    "GetFrameCPUUsage(): Wrong object type, expected frame",
                ));
            }
            let _children = bool_or_default(args.get(1), true);
            Ok((0.0, 0.0))
        })?,
    )?;
    // `GetFunctionCPUUsage(function [, includeSubroutines])` (`0x511720`): the function's own
    // or total milliseconds, and its calls.
    g.set(
        "GetFunctionCPUUsage",
        lua.create_function(|_, args: MultiValue| {
            if !matches!(args.front(), Some(Value::Function(_))) {
                return Err(mlua::Error::runtime(
                    "Usage: GetFunctionCPUUsage(function[, includeSubroutines])",
                ));
            }
            Ok((0.0, 0.0))
        })?,
    )?;

    // `GetFramesRegisteredForEvent("event")` (`0x81be70`): every frame registered for the event,
    // in registration order. Deviation: an event name the client does not know reads a null
    // listener list in the reference and crashes it; here it answers nothing.
    g.set(
        "GetFramesRegisteredForEvent",
        lua.create_function(|lua, args: MultiValue| {
            let Some(event) = args
                .front()
                .cloned()
                .and_then(|v| lua.coerce_string(v).ok().flatten())
            else {
                return Err(mlua::Error::runtime(
                    "Usage: GetFramesRegisteredForEvent(\"event\")",
                ));
            };
            let ids: Vec<u32> = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let name = event.to_string_lossy();
                let Some(key) = model.vm.event_key(&name) else {
                    return Ok(MultiValue::new());
                };
                model
                    .event_to_frames
                    .get(key)
                    .map(|l| {
                        l.iter()
                            .filter_map(|h| model.frame_to_id.get(h).copied())
                            .collect()
                    })
                    .unwrap_or_default()
            };
            ids.into_iter()
                .map(|id| super::object::frame_wrapper(lua, id).map(Value::Table))
                .collect::<mlua::Result<MultiValue>>()
        })?,
    )?;
    Ok(())
}
