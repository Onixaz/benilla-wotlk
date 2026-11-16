//! The Path animation's control points (`ControlPoint`, methods `0xac1a34`) and the two
//! `SetParent`s that move an animation between groups (`0x4a5880`) and a control point between Paths
//! (`0x4a6790`). A point keeps its offset and order; a Path draws nothing, so nothing flies them.

use mlua::{Lua, MultiValue, Table, Value};

use super::lua::{create_point, this_id, usage, wrapper};
use super::{Kind, CURVES};
use crate::script::object::decode_id;

/// The registry key of the ControlPoint metatable.
pub(super) const REG_POINT_META: &str = "__benilla_anim_point_meta";

/// The receiver's id, checked to be a live control point.
fn point_id(lua: &Lua, this: &Table) -> mlua::Result<u32> {
    let id = decode_id(this)?;
    if crate::script::model_mut_of(lua)
        .anims
        .points
        .contains_key(&id)
    {
        Ok(id)
    } else {
        Err(mlua::Error::runtime("not a control point"))
    }
}

/// The 0-based order an optional 1-based number names, clamped to 0..=99 (`SetOrder`'s
/// no-wrap leg); `None` for anything but a number.
fn order_arg(lua: &Lua, v: Value) -> mlua::Result<Option<i32>> {
    Ok(lua
        .coerce_number(v)?
        .map(|n| (n as i64 as i32 - 1).clamp(0, 99)))
}

/// An unset order takes `next` (`SetOrder`'s wrapping leg): one outside 0..=99 becomes 0.
fn wrapped(next: i32) -> i32 {
    if (0..=99).contains(&next) {
        next
    } else {
        0
    }
}

/// `SetParent`'s parent argument (`0x4a5880`, `0x4a6790`): a name, looked up among the objects of
/// the wanted kind, or the object's own table. `kind` and `noun` fill the reference's error texts.
fn parent_arg(
    lua: &Lua,
    who: &str,
    v: &Value,
    find: impl Fn(&str) -> Option<u32>,
    is_kind: impl Fn(u32) -> bool,
    (kind, noun, plural): (&str, &str, &str),
) -> mlua::Result<u32> {
    match v {
        Value::Nil => Err(mlua::Error::runtime(format!(
            "{who}:SetParent(): Cannot set a 'nil' parent for {plural}"
        ))),
        Value::Table(t) => {
            let Ok(id) = decode_id(t) else {
                return Err(mlua::Error::runtime(format!(
                    "{who}:SetParent(): Couldn't find 'this' in parent object"
                )));
            };
            // Frame and animation ids come from separate counters, so the id must also name
            // this very table.
            if is_kind(id) && wrapper(lua, id)? == *t {
                Ok(id)
            } else {
                Err(mlua::Error::runtime(format!(
                    "{who}:SetParent(): Wrong parent object type, expected {kind}"
                )))
            }
        }
        other => {
            let name = lua
                .coerce_string(other.clone())?
                .map(|s| s.to_string_lossy())
                .unwrap_or_default();
            find(&name).ok_or_else(|| {
                mlua::Error::runtime(format!(
                    "{who}:SetParent(): Couldn't find {noun} named '{name}'"
                ))
            })
        }
    }
}

/// `Animation:SetParent(group [, order])` (`0x4a5880`), on every animation type: the group by name
/// or table; a given order is set first, and an unset one becomes the new group's last order plus
/// one. Moving re-lists the animation in the new group; staying with a new order re-lists it in
/// place.
pub(super) fn install_anim_set_parent(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.set(
        "SetParent",
        lua.create_function(|lua, (this, parent, order): (Table, Value, Value)| {
            let id = this_id(lua, &this, false)?;
            let who = super::lua::name_of(lua, id);
            let group = parent_arg(
                lua,
                &who,
                &parent,
                |name| {
                    let model = crate::script::model_mut_of(lua);
                    model
                        .anims
                        .groups
                        .iter()
                        .filter(|(_, g)| g.name.as_deref() == Some(name))
                        .map(|(&g, _)| g)
                        .min()
                },
                |g| {
                    crate::script::model_mut_of(lua)
                        .anims
                        .groups
                        .contains_key(&g)
                },
                ("AnimationGroup", "animation group", "animations"),
            )?;
            let order = order_arg(lua, order)?;
            let emptied = {
                let mut model = crate::script::model_mut_of(lua);
                let s = &mut model.anims;
                let Some(old) = s.anims.get(&id).map(|a| a.order) else {
                    return Ok(());
                };
                let new = match order {
                    Some(o) => o,
                    None if old == -1 => wrapped(s.max_order(group) + 1),
                    None => old,
                };
                if let Some(a) = s.anims.get_mut(&id) {
                    a.order = new;
                }
                let moved = s.change_group(id, group);
                (!moved && new != old).then(|| s.relist(group, id))
            };
            if emptied == Some(true) {
                super::lua::sort(lua, group);
            }
            Ok(())
        })?,
    )
}

/// The Path's own methods (`0xac1a48`).
pub(super) fn install_path_methods(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // CreateControlPoint([name [, inherits [, order]]]) (`0x4a7bf0`): a number order is set, else
    // the Path's highest, wrapped, with no step past it. The template argument is not read, as
    // `CreateAnimation`'s is not.
    m.set(
        "CreateControlPoint",
        lua.create_function(
            |lua, (this, name, _inherits, order): (Table, Value, Value, Value)| {
                let path = this_id(lua, &this, false)?;
                let name = match name {
                    Value::String(s) if !s.as_bytes().is_empty() => Some(s.to_str()?.to_string()),
                    _ => None,
                };
                let order = order_arg(lua, order)?;
                create_point(lua, path, name, order)
            },
        )?,
    )?;
    // GetControlPoints (`0x4a79d0`): every point, in the sorted array's order.
    m.set(
        "GetControlPoints",
        lua.create_function(|lua, this: Table| {
            let path = this_id(lua, &this, false)?;
            let ids = crate::script::model_mut_of(lua).anims.path_points(path);
            ids.into_iter()
                .map(|p| wrapper(lua, p).map(Value::Table))
                .collect::<mlua::Result<MultiValue>>()
        })?,
    )?;
    // GetMaxOrder (`0x4a6d20`): the highest point order plus one, 0 with none.
    m.set(
        "GetMaxOrder",
        lua.create_function(|lua, this: Table| {
            let path = this_id(lua, &this, false)?;
            Ok(f64::from(
                crate::script::model_mut_of(lua).anims.max_point_order(path) + 1,
            ))
        })?,
    )?;
    // SetCurve("curveType") (`0x4a6c10`): NONE or SMOOTH, case-insensitive; GetCurve (`0x4a6cd0`).
    m.set(
        "SetCurve",
        lua.create_function(|lua, (this, curve): (Table, Value)| {
            let path = this_id(lua, &this, false)?;
            let curve = match &curve {
                Value::String(_) | Value::Integer(_) | Value::Number(_) => lua
                    .coerce_string(curve)?
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default(),
                _ => return Err(usage(lua, path, "SetCurve(\"curveType\")")),
            };
            let Some(byte) = CURVES.iter().position(|c| c.eq_ignore_ascii_case(&curve)) else {
                return Err(mlua::Error::runtime(format!(
                    "{}:SetCurve(): Unknown curve type specified",
                    super::lua::name_of(lua, path)
                )));
            };
            crate::script::model_mut_of(lua)
                .anims
                .curves
                .insert(path, byte as u8);
            Ok(())
        })?,
    )?;
    m.set(
        "GetCurve",
        lua.create_function(|lua, this: Table| {
            let path = this_id(lua, &this, false)?;
            let byte = crate::script::model_mut_of(lua)
                .anims
                .curves
                .get(&path)
                .copied()
                .unwrap_or(0);
            Ok(CURVES.get(usize::from(byte)).copied().unwrap_or("UNKNOWN"))
        })?,
    )?;
    Ok(())
}

/// The ControlPoint metatable: the script object's four methods and the point's own five.
pub(super) fn install_point_meta(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;
    m.set(
        "GetObjectType",
        lua.create_function(|lua, this: Table| {
            point_id(lua, &this)?;
            Ok("ControlPoint")
        })?,
    )?;
    m.set(
        "IsObjectType",
        lua.create_function(|lua, (this, t): (Table, String)| {
            point_id(lua, &this)?;
            Ok(crate::script::binding_abi::flag(
                t.eq_ignore_ascii_case("ControlPoint") || t.eq_ignore_ascii_case("ScriptObject"),
            ))
        })?,
    )?;
    m.set(
        "GetName",
        lua.create_function(|lua, this: Table| {
            let id = point_id(lua, &this)?;
            Ok(crate::script::model_mut_of(lua)
                .anims
                .points
                .get(&id)
                .and_then(|p| p.name.clone()))
        })?,
    )?;
    m.set(
        "GetParent",
        lua.create_function(|lua, this: Table| {
            let id = point_id(lua, &this)?;
            let path = crate::script::model_mut_of(lua)
                .anims
                .points
                .get(&id)
                .map(|p| p.path);
            path.map(|p| wrapper(lua, p)).transpose()
        })?,
    )?;
    // SetOffset(x, y) (`0x4a6980`): two numbers, else the named Usage; GetOffset (`0x4a6a70`).
    m.set(
        "SetOffset",
        lua.create_function(|lua, (this, x, y): (Table, Value, Value)| {
            let id = point_id(lua, &this)?;
            let (Some(x), Some(y)) = (lua.coerce_number(x)?, lua.coerce_number(y)?) else {
                return Err(usage(lua, id, "SetOffset(x, y)"));
            };
            let (x, y) = (x as f32, y as f32);
            if let Some(p) = crate::script::model_mut_of(lua).anims.points.get_mut(&id) {
                p.offset = if x * x + y * y <= 2.384_185_8e-7 {
                    (0.0, 0.0)
                } else {
                    (x, y)
                };
            }
            Ok(())
        })?,
    )?;
    m.set(
        "GetOffset",
        lua.create_function(|lua, this: Table| {
            let id = point_id(lua, &this)?;
            let o = crate::script::model_mut_of(lua)
                .anims
                .points
                .get(&id)
                .map_or((0.0, 0.0), |p| p.offset);
            Ok((f64::from(o.0), f64::from(o.1)))
        })?,
    )?;
    // SetOrder(order) (`0x4a6b10`): 1-based, clamped to 1..=100, else the named Usage; GetOrder
    // (`0x4a6ba0`) pushes the integer order plus one, 0 while unset.
    m.set(
        "SetOrder",
        lua.create_function(|lua, (this, v): (Table, Value)| {
            let id = point_id(lua, &this)?;
            let Some(order) = order_arg(lua, v)? else {
                return Err(usage(lua, id, "SetOrder(order)"));
            };
            if let Some(p) = crate::script::model_mut_of(lua).anims.points.get_mut(&id) {
                p.order = order;
            }
            Ok(())
        })?,
    )?;
    m.set(
        "GetOrder",
        lua.create_function(|lua, this: Table| {
            let id = point_id(lua, &this)?;
            Ok(crate::script::model_mut_of(lua)
                .anims
                .points
                .get(&id)
                .map_or(0, |p| i64::from(p.order) + 1))
        })?,
    )?;
    // SetParent(path [, order]) (`0x4a6790`): `Animation:SetParent`'s rules with a Path for the
    // group; an unset order becomes the new Path's highest plus one.
    m.set(
        "SetParent",
        lua.create_function(|lua, (this, parent, order): (Table, Value, Value)| {
            let id = point_id(lua, &this)?;
            let who = super::lua::name_of(lua, id);
            let is_path = |a: u32| {
                crate::script::model_mut_of(lua)
                    .anims
                    .anims
                    .get(&a)
                    .is_some_and(|x| x.kind == Kind::Path)
            };
            let path = parent_arg(
                lua,
                &who,
                &parent,
                |name| {
                    let model = crate::script::model_mut_of(lua);
                    model
                        .anims
                        .anims
                        .iter()
                        .filter(|(_, a)| a.kind == Kind::Path && a.name.as_deref() == Some(name))
                        .map(|(&a, _)| a)
                        .min()
                },
                is_path,
                ("Path", "Path", "control points"),
            )?;
            let order = order_arg(lua, order)?;
            let mut model = crate::script::model_mut_of(lua);
            let s = &mut model.anims;
            let Some(old) = s.points.get(&id).map(|p| p.order) else {
                return Ok(());
            };
            let new = match order {
                Some(o) => o,
                None if old == -1 => wrapped(s.max_point_order(path) + 1),
                None => old,
            };
            if let Some(p) = s.points.get_mut(&id) {
                p.order = new;
                p.path = path;
            }
            Ok(())
        })?,
    )?;
    let meta = lua.create_table()?;
    meta.set("__index", m)?;
    lua.set_named_registry_value(REG_POINT_META, meta)
}

/// The order a new point takes (`0x4a7bf0`): the given one, else the Path's highest, wrapped.
pub(super) fn new_point_order(s: &super::AnimStore, path: u32, order: Option<i32>) -> i32 {
    order.unwrap_or_else(|| wrapped(s.max_point_order(path)))
}
