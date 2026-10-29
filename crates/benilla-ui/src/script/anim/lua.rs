//! The Lua face of [`super`]: the object wrappers (`T[0]` the id, as frames'), the script slots,
//! and the AnimationGroup and Animation method tables, plus the region verbs
//! `CreateAnimationGroup`, `GetAnimationGroups` and `StopAnimating` (`0x49e350`).

use mlua::{Function, Lua, MultiValue, Table, Value};

use super::run;
use super::{
    set_max_framerate, set_smoothing, Kind, OriginPoint, Owner, LOOP_STATES, LOOP_TYPES, NONE,
    PAUSED, PENDING_FINISH, SMOOTHING,
};
use crate::script::object::{decode_id, frame_wrapper, id_to_lud};

const REG_WRAPPERS: &str = "__benilla_anim_wrappers";
const REG_SCRIPTS: &str = "__benilla_anim_scripts";
const REG_GROUP_META: &str = "__benilla_anim_group_meta";
const REG_ANIM_META: &str = "__benilla_anim_meta";

/// The scripts an AnimationGroup holds (`0x497800`) and an Animation (`0x497500`).
const GROUP_SCRIPTS: [&str; 7] = [
    "OnLoad",
    "OnPlay",
    "OnPause",
    "OnStop",
    "OnFinished",
    "OnUpdate",
    "OnLoop",
];
const ANIM_SCRIPTS: [&str; 6] = [
    "OnLoad",
    "OnPlay",
    "OnPause",
    "OnStop",
    "OnFinished",
    "OnUpdate",
];

/// The compiled parameter list of an XML handler (the reference's per-handler wrappers): an
/// Animation's `OnFinished` takes `self` alone, a group's `self,requested`.
pub(crate) fn script_parameters(script: &str, group: bool) -> &'static str {
    match script {
        "OnStop" => "self,requested",
        "OnFinished" if group => "self,requested",
        "OnUpdate" => "self,elapsed",
        "OnLoop" => "self,loopState",
        _ => "self",
    }
}

fn is_group(lua: &Lua, id: u32) -> bool {
    crate::script::model_mut_of(lua)
        .anims
        .groups
        .contains_key(&id)
}

/// The wrapper table for a group or animation id, created on first use.
pub(crate) fn wrapper(lua: &Lua, id: u32) -> mlua::Result<Table> {
    let wrappers: Table = lua.named_registry_value(REG_WRAPPERS)?;
    if let Value::Table(t) = wrappers.get::<Value>(id)? {
        return Ok(t);
    }
    let t = lua.create_table()?;
    t.raw_set(0, Value::LightUserData(id_to_lud(id)))?;
    let meta: Table = if is_group(lua, id) {
        lua.named_registry_value(REG_GROUP_META)?
    } else {
        let kind = crate::script::model_mut_of(lua)
            .anims
            .anims
            .get(&id)
            .map(|a| a.kind.type_name());
        let metas: Table = lua.named_registry_value(REG_ANIM_META)?;
        metas.get(kind.unwrap_or("Animation"))?
    };
    t.set_metatable(Some(meta))?;
    wrappers.set(id, t.clone())?;
    Ok(t)
}

/// A named object's global, non-overwriting, as frames publish theirs.
fn publish(lua: &Lua, name: Option<&str>, t: &Table) -> mlua::Result<()> {
    if let Some(name) = name.filter(|n| !n.is_empty()) {
        if lua.globals().raw_get::<Value>(name)?.is_nil() {
            lua.globals().raw_set(name, t.clone())?;
        }
    }
    Ok(())
}

/// The owner's wrapper.
pub(crate) fn owner_wrapper(lua: &Lua, owner: Owner) -> mlua::Result<Table> {
    match owner {
        Owner::Frame(h) => {
            let id = crate::script::model_mut_of(lua).frame_id(h);
            frame_wrapper(lua, id)
        }
        Owner::Region(r) => {
            let id = crate::script::model_mut_of(lua).region_id(r);
            crate::script::region::region_wrapper(lua, id)
        }
    }
}

/// The owner a frame or region wrapper names.
pub(crate) fn owner_of(lua: &Lua, this: &Table) -> mlua::Result<Owner> {
    let id = decode_id(this)?;
    let model = crate::script::model_mut_of(lua);
    if let Some(&h) = model.id_to_frame.get(&id) {
        return Ok(Owner::Frame(h));
    }
    if let Some(&r) = model.id_to_region.get(&id) {
        return Ok(Owner::Region(r));
    }
    Err(mlua::Error::runtime("stale or invalid region handle"))
}

fn scripts_of(lua: &Lua, id: u32) -> mlua::Result<Option<Table>> {
    let all: Table = lua.named_registry_value(REG_SCRIPTS)?;
    match all.get::<Value>(id)? {
        Value::Table(t) => Ok(Some(t)),
        _ => Ok(None),
    }
}

/// Set one script slot, `None` clearing it.
pub(crate) fn set_script(
    lua: &Lua,
    id: u32,
    name: &str,
    func: Option<Function>,
) -> mlua::Result<()> {
    let all: Table = lua.named_registry_value(REG_SCRIPTS)?;
    let per = match scripts_of(lua, id)? {
        Some(t) => t,
        None => {
            let t = lua.create_table()?;
            all.set(id, t.clone())?;
            t
        }
    };
    per.set(name, func)
}

/// Run a script with `self` and `args`, `this` set around it; an error is recorded.
pub(crate) fn fire(lua: &Lua, id: u32, name: &str, args: Vec<Value>) {
    let func = match scripts_of(lua, id).and_then(|s| match s {
        Some(t) => t.get::<Option<Function>>(name),
        None => Ok(None),
    }) {
        Ok(Some(f)) => f,
        _ => return,
    };
    let outcome = wrapper(lua, id)
        .and_then(|w| crate::script::event::invoke_with_globals(lua, w, &func, None, args));
    if let Err(e) = outcome {
        crate::script::model_mut_of(lua).record_script_error(e.to_string());
    }
}

/// The receiver's id, checked to be a live group or animation.
fn this_id(lua: &Lua, this: &Table, group: bool) -> mlua::Result<u32> {
    let id = decode_id(this)?;
    let model = crate::script::model_mut_of(lua);
    let live = if group {
        model.anims.groups.contains_key(&id)
    } else {
        model.anims.anims.contains_key(&id)
    };
    if live {
        Ok(id)
    } else {
        Err(mlua::Error::runtime("not an animation object"))
    }
}

fn name_of(lua: &Lua, id: u32) -> String {
    let model = crate::script::model_mut_of(lua);
    model
        .anims
        .groups
        .get(&id)
        .and_then(|g| g.name.clone())
        .or_else(|| model.anims.anims.get(&id).and_then(|a| a.name.clone()))
        .unwrap_or_else(|| "<unnamed>".into())
}

fn number(lua: &Lua, v: Value) -> mlua::Result<Option<f64>> {
    lua.coerce_number(v)
}

fn usage(lua: &Lua, id: u32, text: &str) -> mlua::Error {
    mlua::Error::RuntimeError(format!("Usage: {}:{text}", name_of(lua, id)))
}

/// `Region:CreateAnimationGroup([name [, inherits]])` (`0x49e350`).
pub(crate) fn create_group(lua: &Lua, owner: Owner, name: Option<String>) -> mlua::Result<Table> {
    let id = crate::script::model_mut_of(lua)
        .anims
        .new_group(owner, name.clone());
    let t = wrapper(lua, id)?;
    publish(lua, name.as_deref(), &t)?;
    Ok(t)
}

/// `CreateAnimation([type [, name [, inherits]]])` (`0x4a7e00`): an unordered animation joins the
/// last order (`0x4a7f8b`).
pub(crate) fn create_anim(
    lua: &Lua,
    group: u32,
    kind: Kind,
    name: Option<String>,
) -> mlua::Result<(u32, Table)> {
    let id = {
        let mut model = crate::script::model_mut_of(lua);
        let s = &mut model.anims;
        let id = s.new_anim(group, kind, name.clone());
        let max = s.max_order(group);
        if let Some(a) = s.anims.get_mut(&id) {
            a.order = if (0..=99).contains(&max) { max } else { 0 };
        }
        s.notify_added(group, id);
        id
    };
    let t = wrapper(lua, id)?;
    publish(lua, name.as_deref(), &t)?;
    Ok((id, t))
}

/// An XML-built animation, not yet in an order list: the animation sort places it at the group's
/// post-load (`0x49c2a0`).
pub(crate) fn xml_anim(
    lua: &Lua,
    group: u32,
    kind: Kind,
    name: Option<String>,
) -> mlua::Result<(u32, Table)> {
    let id = crate::script::model_mut_of(lua)
        .anims
        .new_anim(group, kind, name.clone());
    let t = wrapper(lua, id)?;
    publish(lua, name.as_deref(), &t)?;
    Ok((id, t))
}

/// The animation sort (`0x49b1c0`): stop, then rebuild the order lists.
pub(crate) fn sort(lua: &Lua, group: u32) {
    run::group_stop(lua, group, false);
    crate::script::model_mut_of(lua).anims.sort(group);
}

/// The scripts trio every object kind shares: Has/Get/Set/HookScript.
fn install_script_methods(lua: &Lua, m: &Table, group: bool) -> mlua::Result<()> {
    let known = move |n: &str| -> Option<&'static str> {
        let names: &[&'static str] = if group { &GROUP_SCRIPTS } else { &ANIM_SCRIPTS };
        names.iter().copied().find(|k| k.eq_ignore_ascii_case(n))
    };
    m.set(
        "HasScript",
        lua.create_function(move |lua, (this, n): (Table, Value)| {
            let id = this_id(lua, &this, group)?;
            let Value::String(n) = n else {
                return Err(usage(lua, id, "HasScript(\"type\")"));
            };
            Ok(known(&n.to_str()?).map(|_| 1))
        })?,
    )?;
    m.set(
        "GetScript",
        lua.create_function(move |lua, (this, n): (Table, Value)| {
            let id = this_id(lua, &this, group)?;
            let Value::String(n) = n else {
                return Err(usage(lua, id, "GetScript(\"type\")"));
            };
            let n = n.to_str()?.to_string();
            let Some(k) = known(&n) else {
                return Err(mlua::Error::RuntimeError(format!(
                    "{} doesn't have a \"{n}\" script",
                    name_of(lua, id)
                )));
            };
            Ok(match scripts_of(lua, id)? {
                Some(t) => t.get::<Option<Function>>(k)?,
                None => None,
            })
        })?,
    )?;
    m.set(
        "SetScript",
        lua.create_function(move |lua, (this, n, f): (Table, Value, Value)| {
            let id = this_id(lua, &this, group)?;
            let (Value::String(n), func) = (n, f) else {
                return Err(usage(lua, id, "SetScript(\"type\", function)"));
            };
            let func = match func {
                Value::Function(f) => Some(f),
                Value::Nil => None,
                _ => return Err(usage(lua, id, "SetScript(\"type\", function)")),
            };
            let n = n.to_str()?.to_string();
            let Some(k) = known(&n) else {
                return Err(mlua::Error::RuntimeError(format!(
                    "{} doesn't have a \"{n}\" script",
                    name_of(lua, id)
                )));
            };
            set_script(lua, id, k, func)
        })?,
    )?;
    m.set(
        "HookScript",
        lua.create_function(move |lua, (this, n, f): (Table, Value, Value)| {
            let id = this_id(lua, &this, group)?;
            let (Value::String(n), Value::Function(hook)) = (n, f) else {
                return Err(usage(lua, id, "HookScript(\"type\", function)"));
            };
            let n = n.to_str()?.to_string();
            let Some(k) = known(&n) else {
                return Err(mlua::Error::RuntimeError(format!(
                    "{} doesn't have a \"{n}\" script",
                    name_of(lua, id)
                )));
            };
            let current = match scripts_of(lua, id)? {
                Some(t) => t.get::<Option<Function>>(k)?,
                None => None,
            };
            // The script hook wrapper: the original, then the hook, with the same arguments.
            let func = match current {
                None => hook,
                Some(orig) => lua
                    .load("local a, b = ... return function(...) a(...) return b(...) end")
                    .set_name("=HookScript")
                    .call::<Function>((orig, hook))?,
            };
            set_script(lua, id, k, Some(func))
        })?,
    )?;
    Ok(())
}

/// `GetObjectType`, `IsObjectType`, `GetName`, `GetParent` (the script object base, `0xac1520`'s).
fn install_object_methods(lua: &Lua, m: &Table, type_name: &'static str) -> mlua::Result<()> {
    let group = type_name == "AnimationGroup";
    let chain: &'static [&'static str] = match type_name {
        "AnimationGroup" => &["AnimationGroup"],
        "Animation" => &["Animation"],
        "Alpha" => &["Alpha", "Animation"],
        "Translation" => &["Translation", "Animation"],
        "Rotation" => &["Rotation", "Animation"],
        "Scale" => &["Scale", "Animation"],
        _ => &["Path", "Animation"],
    };
    m.set(
        "GetObjectType",
        lua.create_function(move |_, _this: Table| Ok(type_name))?,
    )?;
    m.set(
        "IsObjectType",
        lua.create_function(move |_, (_this, t): (Table, String)| {
            Ok(chain.iter().any(|c| c.eq_ignore_ascii_case(&t))
                || t.eq_ignore_ascii_case("ScriptObject"))
        })?,
    )?;
    m.set(
        "GetName",
        lua.create_function(move |lua, this: Table| {
            let id = this_id(lua, &this, group)?;
            let model = crate::script::model_mut_of(lua);
            Ok(if group {
                model.anims.groups.get(&id).and_then(|g| g.name.clone())
            } else {
                model.anims.anims.get(&id).and_then(|a| a.name.clone())
            })
        })?,
    )?;
    m.set(
        "GetParent",
        lua.create_function(move |lua, this: Table| {
            let id = this_id(lua, &this, group)?;
            if group {
                let owner = crate::script::model_mut_of(lua)
                    .anims
                    .groups
                    .get(&id)
                    .map(|g| g.owner);
                return owner.map(|o| owner_wrapper(lua, o)).transpose();
            }
            let g = crate::script::model_mut_of(lua)
                .anims
                .anims
                .get(&id)
                .map(|a| a.group);
            g.map(|g| wrapper(lua, g)).transpose()
        })?,
    )?;
    Ok(())
}

fn install_group_methods(lua: &Lua) -> mlua::Result<Table> {
    let m = lua.create_table()?;
    install_object_methods(lua, &m, "AnimationGroup")?;
    install_script_methods(lua, &m, true)?;
    let g = |lua: &Lua, this: &Table| this_id(lua, this, true);
    m.set(
        "Play",
        lua.create_function(move |lua, this: Table| {
            run::group_play(lua, g(lua, &this)?);
            Ok(())
        })?,
    )?;
    m.set(
        "Pause",
        lua.create_function(move |lua, this: Table| {
            run::group_pause(lua, g(lua, &this)?);
            Ok(())
        })?,
    )?;
    m.set(
        "Stop",
        lua.create_function(move |lua, this: Table| {
            run::group_stop(lua, g(lua, &this)?, true);
            Ok(())
        })?,
    )?;
    m.set(
        "Finish",
        lua.create_function(move |lua, this: Table| {
            run::group_finish(lua, g(lua, &this)?);
            Ok(())
        })?,
    )?;
    let read = |f: fn(&super::Group) -> Value| {
        move |lua: &Lua, this: Table| -> mlua::Result<Value> {
            let id = this_id(lua, &this, true)?;
            let model = crate::script::model_mut_of(lua);
            Ok(model.anims.groups.get(&id).map_or(Value::Nil, f))
        }
    };
    // GetProgress (`0x4a6fa0`): elapsed over duration, unclamped.
    m.set(
        "GetProgress",
        lua.create_function(read(|x| {
            Value::Number(f64::from(x.elapsed) / f64::from(x.duration))
        }))?,
    )?;
    m.set(
        "IsPlaying",
        lua.create_function(read(|x| {
            Value::Boolean(x.loop_state != NONE && x.flags & PAUSED == 0)
        }))?,
    )?;
    m.set(
        "IsPaused",
        lua.create_function(read(|x| Value::Boolean(x.flags & PAUSED != 0)))?,
    )?;
    m.set(
        "IsPendingFinish",
        lua.create_function(read(|x| Value::Boolean(x.flags & PENDING_FINISH != 0)))?,
    )?;
    m.set(
        "GetDuration",
        lua.create_function(read(|x| Value::Number(f64::from(x.duration))))?,
    )?;
    m.set(
        "IsDone",
        lua.create_function(|lua, this: Table| {
            let id = this_id(lua, &this, true)?;
            Ok(crate::script::model_mut_of(lua).anims.is_done(id))
        })?,
    )?;
    m.set(
        "SetLooping",
        lua.create_function(|lua, (this, v): (Table, Value)| {
            let id = this_id(lua, &this, true)?;
            let Value::String(s) = v else {
                return Err(usage(lua, id, "SetLooping(\"loopType\")"));
            };
            let s = s.to_str()?.to_string();
            let Some(t) = LOOP_TYPES.iter().position(|k| k.eq_ignore_ascii_case(&s)) else {
                return Err(mlua::Error::RuntimeError(format!(
                    "{}:SetLooping(): Unknown loop type specified",
                    name_of(lua, id)
                )));
            };
            if let Some(x) = crate::script::model_mut_of(lua).anims.groups.get_mut(&id) {
                x.loop_type = t as u8;
            }
            Ok(())
        })?,
    )?;
    // GetLooping/GetLoopState (`0x4a71f0`, `0x4a7240`): the names, "UNKNOWN" off the table.
    let names = |state: bool| {
        move |lua: &Lua, this: Table| -> mlua::Result<&'static str> {
            let id = this_id(lua, &this, true)?;
            let model = crate::script::model_mut_of(lua);
            let (v, table) = match model.anims.groups.get(&id) {
                Some(x) if state => (x.loop_state, &LOOP_STATES),
                Some(x) => (x.loop_type, &LOOP_TYPES),
                None => (u8::MAX, &LOOP_TYPES),
            };
            Ok(table.get(usize::from(v)).copied().unwrap_or("UNKNOWN"))
        }
    };
    m.set("GetLooping", lua.create_function(names(false))?)?;
    m.set("GetLoopState", lua.create_function(names(true))?)?;
    m.set(
        "GetMaxOrder",
        lua.create_function(|lua, this: Table| {
            let id = this_id(lua, &this, true)?;
            Ok(crate::script::model_mut_of(lua).anims.max_order(id) + 1)
        })?,
    )?;
    m.set(
        "SetInitialOffset",
        lua.create_function(|lua, (this, x, y): (Table, Value, Value)| {
            let id = this_id(lua, &this, true)?;
            let (Some(x), Some(y)) = (number(lua, x)?, number(lua, y)?) else {
                return Err(usage(lua, id, "SetInitialOffset(x, y)"));
            };
            let (x, y) = (x as f32, y as f32);
            if let Some(g) = crate::script::model_mut_of(lua).anims.groups.get_mut(&id) {
                g.initial_offset = if y * y + x * x <= 0.000_000_24 {
                    (0.0, 0.0)
                } else {
                    (x, y)
                };
            }
            Ok(())
        })?,
    )?;
    m.set(
        "GetInitialOffset",
        lua.create_function(|lua, this: Table| {
            let id = this_id(lua, &this, true)?;
            let o = crate::script::model_mut_of(lua)
                .anims
                .groups
                .get(&id)
                .map_or((0.0, 0.0), |g| g.initial_offset);
            Ok((f64::from(o.0), f64::from(o.1)))
        })?,
    )?;
    m.set(
        "GetAnimations",
        lua.create_function(|lua, this: Table| {
            let id = this_id(lua, &this, true)?;
            let anims = crate::script::model_mut_of(lua)
                .anims
                .groups
                .get(&id)
                .map(|g| g.anims.clone())
                .unwrap_or_default();
            anims
                .into_iter()
                .map(|a| wrapper(lua, a).map(Value::Table))
                .collect::<mlua::Result<MultiValue>>()
        })?,
    )?;
    m.set(
        "CreateAnimation",
        lua.create_function(|lua, (this, kind, name): (Table, Value, Value)| {
            let id = this_id(lua, &this, true)?;
            let kind = match &kind {
                Value::String(s) => Kind::from_name(&s.to_str()?),
                _ => Kind::Plain,
            };
            let name = match name {
                Value::String(s) => Some(s.to_str()?.to_string()),
                _ => None,
            };
            create_anim(lua, id, kind, name).map(|(_, t)| t)
        })?,
    )?;
    Ok(m)
}

fn install_anim_methods(lua: &Lua, type_name: &'static str) -> mlua::Result<Table> {
    let m = lua.create_table()?;
    install_object_methods(lua, &m, type_name)?;
    install_script_methods(lua, &m, false)?;
    let a = |lua: &Lua, this: &Table| this_id(lua, this, false);
    m.set(
        "Play",
        lua.create_function(move |lua, this: Table| {
            run::anim_play(lua, a(lua, &this)?);
            Ok(())
        })?,
    )?;
    m.set(
        "Pause",
        lua.create_function(move |lua, this: Table| {
            run::anim_pause(lua, a(lua, &this)?);
            Ok(())
        })?,
    )?;
    m.set(
        "Stop",
        lua.create_function(move |lua, this: Table| {
            run::anim_stop(lua, a(lua, &this)?, true);
            Ok(())
        })?,
    )?;
    let read = |f: fn(&super::Anim) -> f64| {
        move |lua: &Lua, this: Table| -> mlua::Result<f64> {
            let id = this_id(lua, &this, false)?;
            let model = crate::script::model_mut_of(lua);
            Ok(model.anims.anims.get(&id).map_or(0.0, f))
        }
    };
    let flag = |f: fn(&super::Anim) -> bool| {
        move |lua: &Lua, this: Table| -> mlua::Result<bool> {
            let id = this_id(lua, &this, false)?;
            let model = crate::script::model_mut_of(lua);
            Ok(model.anims.anims.get(&id).is_some_and(f))
        }
    };
    m.set(
        "IsDone",
        lua.create_function(flag(|x| x.progress_with_delay >= 1.0))?,
    )?;
    m.set(
        "IsPlaying",
        lua.create_function(flag(|x| x.play_state == 1))?,
    )?;
    m.set(
        "IsPaused",
        lua.create_function(flag(|x| x.play_state == 2))?,
    )?;
    m.set(
        "IsStopped",
        lua.create_function(flag(|x| x.play_state == 0))?,
    )?;
    m.set(
        "IsDelaying",
        lua.create_function(|lua, this: Table| {
            let id = this_id(lua, &this, false)?;
            Ok(run::delaying(lua, id))
        })?,
    )?;
    m.set(
        "GetElapsed",
        lua.create_function(read(|x| f64::from(x.elapsed)))?,
    )?;
    m.set(
        "GetStartDelay",
        lua.create_function(read(|x| f64::from(x.start_delay)))?,
    )?;
    m.set(
        "GetEndDelay",
        lua.create_function(read(|x| f64::from(x.end_delay)))?,
    )?;
    m.set(
        "GetDuration",
        lua.create_function(read(|x| f64::from(x.duration)))?,
    )?;
    m.set(
        "GetSmoothProgress",
        lua.create_function(read(|x| f64::from(x.smooth_progress)))?,
    )?;
    m.set(
        "GetProgress",
        lua.create_function(read(|x| f64::from(x.progress)))?,
    )?;
    m.set(
        "GetProgressWithDelay",
        lua.create_function(read(|x| f64::from(x.progress_with_delay)))?,
    )?;
    m.set(
        "GetMaxFramerate",
        lua.create_function(read(|x| f64::from(x.max_framerate)))?,
    )?;
    m.set(
        "GetOrder",
        lua.create_function(|lua, this: Table| {
            let id = this_id(lua, &this, false)?;
            let model = crate::script::model_mut_of(lua);
            Ok(model.anims.anims.get(&id).map_or(0, |x| x.order + 1))
        })?,
    )?;
    // The numeric setters, each re-totalling the group (`0x49ae20`, `0x49ae60`, `0x49aea0`).
    type Setter = fn(&mut super::Anim, f32);
    let setters: [(&str, &str, Setter); 5] = [
        ("SetStartDelay", "SetStartDelay(delaySec)", |x, v| {
            x.start_delay = v.max(0.0)
        }),
        ("SetEndDelay", "SetEndDelay(delaySec)", |x, v| {
            x.end_delay = v.max(0.0)
        }),
        ("SetDuration", "SetDuration(durationSec)", |x, v| {
            x.duration = v
        }),
        (
            "SetSmoothProgress",
            "SetSmoothProgress(smoothProgress)",
            |x, v| x.smooth_progress = v,
        ),
        (
            "SetMaxFramerate",
            "SetMaxFramerate(framesPerSec)",
            |x, v| set_max_framerate(x, v),
        ),
    ];
    for (method, text, set) in setters {
        m.set(
            method,
            lua.create_function(move |lua, (this, v): (Table, Value)| {
                let id = this_id(lua, &this, false)?;
                let Some(v) = number(lua, v)? else {
                    return Err(usage(lua, id, text));
                };
                let mut model = crate::script::model_mut_of(lua);
                let s = &mut model.anims;
                let group = s.anims.get_mut(&id).map(|x| {
                    set(x, v as f32);
                    x.group
                });
                if let Some(g) = group {
                    s.compute_duration_pub(g);
                }
                Ok(())
            })?,
        )?;
    }
    // SetOrder(order) (`0x4a5660`): 1-based, clamped to 1-100, re-listed on a change.
    m.set(
        "SetOrder",
        lua.create_function(|lua, (this, v): (Table, Value)| {
            let id = this_id(lua, &this, false)?;
            let Some(v) = number(lua, v)? else {
                return Err(usage(lua, id, "SetOrder(order)"));
            };
            let order = v as i64 as i32 - 1;
            let emptied = {
                let mut model = crate::script::model_mut_of(lua);
                let s = &mut model.anims;
                let Some(anim) = s.anims.get_mut(&id) else {
                    return Ok(());
                };
                let old = anim.order;
                anim.order = order.clamp(0, 99);
                let g = anim.group;
                (old != order).then(|| (g, s.relist(g, id)))
            };
            if let Some((g, true)) = emptied {
                sort(lua, g);
            }
            Ok(())
        })?,
    )?;
    m.set(
        "SetSmoothing",
        lua.create_function(|lua, (this, v): (Table, Value)| {
            let id = this_id(lua, &this, false)?;
            let Value::String(s) = v else {
                return Err(usage(lua, id, "SetSmoothing(\"smoothingType\")"));
            };
            let s = s.to_str()?.to_string();
            let Some(&(_, i, o)) = SMOOTHING.iter().find(|k| k.0.eq_ignore_ascii_case(&s)) else {
                return Err(mlua::Error::RuntimeError(format!(
                    "{}:SetSmoothing(): Unknown smoothing type specified",
                    name_of(lua, id)
                )));
            };
            if let Some(x) = crate::script::model_mut_of(lua).anims.anims.get_mut(&id) {
                set_smoothing(x, (i, o));
            }
            Ok(())
        })?,
    )?;
    m.set(
        "GetSmoothing",
        lua.create_function(|lua, this: Table| {
            let id = this_id(lua, &this, false)?;
            let w = crate::script::model_mut_of(lua)
                .anims
                .anims
                .get(&id)
                .and_then(|x| x.smoothing)
                .unwrap_or((0.0, 0.0));
            let name = SMOOTHING
                .iter()
                .find(|k| (w.0 - k.1).abs() < 0.001 && (w.1 - k.2).abs() < 0.001)
                .map_or("UNKNOWN", |k| k.0);
            Ok(name)
        })?,
    )?;
    m.set(
        "GetRegionParent",
        lua.create_function(|lua, this: Table| {
            let id = this_id(lua, &this, false)?;
            let owner = {
                let model = crate::script::model_mut_of(lua);
                model
                    .anims
                    .anims
                    .get(&id)
                    .and_then(|a| model.anims.groups.get(&a.group))
                    .map(|g| g.owner)
            };
            owner.map(|o| owner_wrapper(lua, o)).transpose()
        })?,
    )?;
    match type_name {
        "Alpha" => install_alpha(lua, &m)?,
        "Translation" => install_translation(lua, &m)?,
        "Rotation" => install_rotation(lua, &m)?,
        "Scale" => install_scale(lua, &m)?,
        _ => {}
    }
    Ok(m)
}

fn with_kind<R>(lua: &Lua, this: &Table, f: impl FnOnce(&mut Kind) -> R) -> mlua::Result<R> {
    let id = this_id(lua, this, false)?;
    let mut model = crate::script::model_mut_of(lua);
    let a = model
        .anims
        .anims
        .get_mut(&id)
        .ok_or_else(|| mlua::Error::runtime("not an animation object"))?;
    Ok(f(&mut a.kind))
}

/// `SetChange(change)`/`GetChange()` (`0x4982e0`): stored as a byte step, read back over 255.
fn install_alpha(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.set(
        "SetChange",
        lua.create_function(|lua, (this, v): (Table, Value)| {
            let id = this_id(lua, &this, false)?;
            let Some(v) = number(lua, v)? else {
                return Err(usage(lua, id, "SetChange(change)"));
            };
            let step = ((v as f32).clamp(-1.0, 1.0) * 255.0) as i16;
            with_kind(lua, &this, |k| *k = Kind::Alpha(step))
        })?,
    )?;
    m.set(
        "GetChange",
        lua.create_function(|lua, this: Table| {
            with_kind(lua, &this, |k| match k {
                Kind::Alpha(s) => f64::from(*s) / 255.0,
                _ => 0.0,
            })
        })?,
    )?;
    Ok(())
}

/// `SetOffset(x, y)`/`GetOffset()` (`0x497ff0`): a near-zero offset is zero.
fn install_translation(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.set(
        "SetOffset",
        lua.create_function(|lua, (this, x, y): (Table, Value, Value)| {
            let id = this_id(lua, &this, false)?;
            let (Some(x), Some(y)) = (number(lua, x)?, number(lua, y)?) else {
                return Err(usage(lua, id, "SetOffset(x, y)"));
            };
            let (x, y) = (x as f32, y as f32);
            let o = if y * y + x * x <= 0.000_000_24 {
                (0.0, 0.0)
            } else {
                (x, y)
            };
            with_kind(lua, &this, |k| *k = Kind::Translation(o.0, o.1))
        })?,
    )?;
    m.set(
        "GetOffset",
        lua.create_function(|lua, this: Table| {
            with_kind(lua, &this, |k| match k {
                Kind::Translation(x, y) => (f64::from(*x), f64::from(*y)),
                _ => (0.0, 0.0),
            })
        })?,
    )?;
    Ok(())
}

fn origin_methods(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.set(
        "SetOrigin",
        lua.create_function(|lua, (this, p, x, y): (Table, String, Value, Value)| {
            let point = crate::script::object::point_from_str(&p)
                .map(crate::script::object::point_name)
                .unwrap_or("CENTER");
            let x = crate::script::binding_abi::coerced_number(lua, Some(x)) as f32;
            let y = crate::script::binding_abi::coerced_number(lua, Some(y)) as f32;
            let origin = OriginPoint { point, x, y };
            with_kind(lua, &this, |k| match k {
                Kind::Rotation(_, o) | Kind::Scale(_, _, o) => *o = origin,
                _ => {}
            })
        })?,
    )?;
    m.set(
        "GetOrigin",
        lua.create_function(|lua, this: Table| {
            with_kind(lua, &this, |k| match k {
                Kind::Rotation(_, o) | Kind::Scale(_, _, o) => {
                    (o.point, f64::from(o.x), f64::from(o.y))
                }
                _ => ("CENTER", 0.0, 0.0),
            })
        })?,
    )?;
    Ok(())
}

fn install_rotation(lua: &Lua, m: &Table) -> mlua::Result<()> {
    origin_methods(lua, m)?;
    let set = |to_rad: fn(f64) -> f64| {
        move |lua: &Lua, (this, v): (Table, Value)| -> mlua::Result<()> {
            let v = crate::script::binding_abi::coerced_number(lua, Some(v));
            with_kind(lua, &this, |k| {
                if let Kind::Rotation(r, _) = k {
                    *r = to_rad(v) as f32;
                }
            })
        }
    };
    let get = |from_rad: fn(f64) -> f64| {
        move |lua: &Lua, this: Table| -> mlua::Result<f64> {
            with_kind(lua, &this, |k| match k {
                Kind::Rotation(r, _) => from_rad(f64::from(*r)),
                _ => 0.0,
            })
        }
    };
    m.set("SetDegrees", lua.create_function(set(f64::to_radians))?)?;
    m.set("SetRadians", lua.create_function(set(|v| v))?)?;
    m.set("GetDegrees", lua.create_function(get(f64::to_degrees))?)?;
    m.set("GetRadians", lua.create_function(get(|v| v))?)?;
    Ok(())
}

fn install_scale(lua: &Lua, m: &Table) -> mlua::Result<()> {
    origin_methods(lua, m)?;
    m.set(
        "SetScale",
        lua.create_function(|lua, (this, x, y): (Table, Value, Value)| {
            let x = crate::script::binding_abi::coerced_number(lua, Some(x)) as f32;
            let y = crate::script::binding_abi::coerced_number(lua, Some(y)) as f32;
            with_kind(lua, &this, |k| {
                if let Kind::Scale(sx, sy, _) = k {
                    (*sx, *sy) = (x, y);
                }
            })
        })?,
    )?;
    m.set(
        "GetScale",
        lua.create_function(|lua, this: Table| {
            with_kind(lua, &this, |k| match k {
                Kind::Scale(x, y, _) => (f64::from(*x), f64::from(*y)),
                _ => (1.0, 1.0),
            })
        })?,
    )?;
    Ok(())
}

/// The Region-map animation verbs (`0xac1520`), one arm per side of the shared map.
pub(in crate::script) fn install_region_verbs(
    lua: &Lua,
    m: &Table,
    side: crate::script::region_map::Side,
) -> mlua::Result<()> {
    use crate::script::region_map::set_shared;
    set_shared(
        lua,
        m,
        side,
        "CreateAnimationGroup",
        |lua, (this, name): (Table, Value)| {
            let owner = owner_of(lua, &this)?;
            let name = match name {
                Value::String(s) => Some(s.to_str()?.to_string()),
                _ => None,
            };
            create_group(lua, owner, name)
        },
    )?;
    set_shared(lua, m, side, "GetAnimationGroups", |lua, this: Table| {
        let owner = owner_of(lua, &this)?;
        let groups = crate::script::model_mut_of(lua).anims.groups_of(owner);
        groups
            .into_iter()
            .map(|g| wrapper(lua, g).map(Value::Table))
            .collect::<mlua::Result<MultiValue>>()
    })?;
    set_shared(lua, m, side, "StopAnimating", |lua, this: Table| {
        let owner = owner_of(lua, &this)?;
        run::stop_animating(lua, owner);
        Ok(())
    })?;
    Ok(())
}

/// Register the registry tables and the method tables (`__index` the table itself).
pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.set_named_registry_value(REG_WRAPPERS, lua.create_table()?)?;
    lua.set_named_registry_value(REG_SCRIPTS, lua.create_table()?)?;
    let meta = |methods: Table| -> mlua::Result<Table> {
        let t = lua.create_table()?;
        t.set("__index", methods)?;
        Ok(t)
    };
    lua.set_named_registry_value(REG_GROUP_META, meta(install_group_methods(lua)?)?)?;
    let metas = lua.create_table()?;
    for kind in [
        "Animation",
        "Alpha",
        "Translation",
        "Rotation",
        "Scale",
        "Path",
    ] {
        metas.set(kind, meta(install_anim_methods(lua, kind)?)?)?;
    }
    lua.set_named_registry_value(REG_ANIM_META, metas)?;
    let _ = (LOOP_STATES, NONE);
    Ok(())
}
