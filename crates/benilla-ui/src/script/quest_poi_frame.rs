//! The `QuestPOIFrame` widget's Lua face (method table `0xacf180`, fourteen methods over the frame
//! table); the state is [`QuestPoiState`]. Stock `WorldMapFrame.xml` declares `WorldMapBlobFrame`
//! as one. No quest POI data is read yet (#134), so no quest has a blob to draw or hover.

use mlua::{Lua, MultiValue, Table, Value};

use super::object::frame_handle_of;
use crate::widget::{KindState, QuestPoiState};

pub(super) const REG_QUEST_POI_METHODS: &str = "__benilla_quest_poi_methods";

/// Run `f` over a frame's blob settings; errors on any other receiver.
fn with_poi<T>(
    lua: &Lua,
    this: &Table,
    f: impl FnOnce(&mut QuestPoiState) -> T,
) -> mlua::Result<T> {
    let h = frame_handle_of(lua, this)?;
    let mut model = crate::script::model_mut_of(lua);
    match model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        Some(KindState::QuestPoi(p)) => Ok(f(p)),
        Some(_) => Err(mlua::Error::runtime("not a QuestPOIFrame")),
        None => Err(mlua::Error::runtime("stale frame handle")),
    }
}

/// `lua_tonumber` (`0x84e030`): 0 for anything but a number or numeric string.
fn number(lua: &Lua, v: Value) -> f64 {
    lua.coerce_number(v).ok().flatten().unwrap_or(0.0)
}

/// An alpha byte (`0x58e660`): clamped to `[0, 255]`, then rounded.
fn alpha(v: f64) -> u8 {
    let v = v as f32;
    let v = if v <= 255.0 {
        if v < 0.0 {
            0.0
        } else {
            v
        }
    } else {
        255.0
    };
    v.round_ties_even() as u8
}

fn truthy(v: &Value) -> bool {
    !matches!(v, Value::Nil | Value::Boolean(false))
}

fn texture(lua: &Lua, v: Value) -> Option<String> {
    lua.coerce_string(v)
        .ok()
        .flatten()
        .map(|s| s.to_string_lossy())
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    m.set(
        "SetFillTexture",
        lua.create_function(|lua, (this, path): (Table, Value)| {
            let path = texture(lua, path);
            with_poi(lua, &this, |p| p.fill_texture = path)
        })?,
    )?;
    m.set(
        "SetBorderTexture",
        lua.create_function(|lua, (this, path): (Table, Value)| {
            let path = texture(lua, path);
            with_poi(lua, &this, |p| p.border_texture = path)
        })?,
    )?;
    m.set(
        "SetFillAlpha",
        lua.create_function(|lua, (this, v): (Table, Value)| {
            let a = alpha(number(lua, v));
            with_poi(lua, &this, |p| p.fill_alpha = a)
        })?,
    )?;
    m.set(
        "SetBorderAlpha",
        lua.create_function(|lua, (this, v): (Table, Value)| {
            let a = alpha(number(lua, v));
            with_poi(lua, &this, |p| p.border_alpha = a)
        })?,
    )?;
    // SetBorderScalar(s) (`0x58e780`): `[0, 10]`, a NaN reading as 10.
    m.set(
        "SetBorderScalar",
        lua.create_function(|lua, (this, v): (Table, Value)| {
            let v = number(lua, v);
            let s = if v <= 10.0 { v.max(0.0) } else { 10.0 } as f32;
            with_poi(lua, &this, |p| p.border_scalar = s)
        })?,
    )?;
    m.set(
        "EnableSmoothing",
        lua.create_function(|lua, (this, on): (Table, Value)| {
            with_poi(lua, &this, |p| p.smoothing = truthy(&on))
        })?,
    )?;
    m.set(
        "EnableMerging",
        lua.create_function(|lua, (this, on): (Table, Value)| {
            with_poi(lua, &this, |p| p.merging = truthy(&on))
        })?,
    )?;
    // SetMergeThreshold(t) (`0x58e8a0`): `[0.1, 0.5]`, a NaN reading as 0.1.
    m.set(
        "SetMergeThreshold",
        lua.create_function(|lua, (this, v): (Table, Value)| {
            let v = number(lua, v);
            let t = if v >= 0.1 { v.min(0.5) } else { 0.1 } as f32;
            with_poi(lua, &this, |p| p.merge_threshold = t)
        })?,
    )?;
    // SetNumSplinePoints(n) (`0x58e920`): `lua_tointeger`, `[8, 30]`.
    m.set(
        "SetNumSplinePoints",
        lua.create_function(|lua, (this, v): (Table, Value)| {
            let n = (number(lua, v) as i64).clamp(8, 30) as i32;
            with_poi(lua, &this, |p| p.spline_points = n)
        })?,
    )?;
    // DrawQuestBlob(questId, draw) (`0x5905f0`): adds the quest's blob from its POI data
    // (`0x5e3ac0`) or removes it (`0x58fb40`); with no POI data neither has a blob to touch.
    m.set(
        "DrawQuestBlob",
        lua.create_function(|lua, (this, _quest, _draw): (Table, Value, Value)| {
            with_poi(lua, &this, |_| ())
        })?,
    )?;
    // UpdateQuestPOI() (`0x58e990`): resolves the receiver, nothing else.
    m.set(
        "UpdateQuestPOI",
        lua.create_function(|lua, this: Table| with_poi(lua, &this, |_| ()))?,
    )?;
    // UpdateMouseOverTooltip(x, y) (`0x58e9c0`): the blob under the cursor's quest log index and
    // objective count, nothing when none is (`0x58e0d0` clears the hits first).
    m.set(
        "UpdateMouseOverTooltip",
        lua.create_function(|lua, (this, _x, _y): (Table, Value, Value)| {
            with_poi(lua, &this, |_| MultiValue::new())
        })?,
    )?;
    // GetTooltipIndex(i) (`0x58ea50`): hit `i` of four, 0 when unset.
    m.set(
        "GetTooltipIndex",
        lua.create_function(|lua, (this, _i): (Table, Value)| with_poi(lua, &this, |_| 0i64))?,
    )?;
    m.set(
        "GetNumTooltips",
        lua.create_function(|lua, this: Table| with_poi(lua, &this, |_| 0i64))?,
    )?;

    lua.set_named_registry_value(REG_QUEST_POI_METHODS, m)?;
    Ok(())
}
