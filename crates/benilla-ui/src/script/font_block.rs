//! The font block: the ten font methods a text-bearing widget's own method table declares,
//! implemented once over the [`RegionData`](super::RegionData) its glyphs paint from.
//!
//! The 1.12 client has no `FontInstance` class: each text type re-declares the font names in its
//! own flat method table, and each binding is a shim that tail-calls one shared implementation.
//! This module is that shared layer, installed by the FontString, EditBox, MessageFrame and
//! ScrollingMessageFrame tables; the `<Font>` object and `SimpleHTML` write their own records and
//! share only [`set_font_args`].
//!
//! `EditBox`'s table (48 entries) opens with these ten, then `Set/GetSpacing` and the four justify
//! methods. Its shims leave `eax` alone, so each returns what the shared implementation pushes, and
//! act on `[this+0x324]`, the box's implicit FontString
//! ([`EditBoxState::text_region`](crate::widget::EditBoxState::text_region)). The justify four are
//! installed by `script::editbox::methods`; the spacing pair by [`install_spacing`].

use mlua::{Lua, Table, Value};

use super::object::as_f32;
use super::{FontShadow, Outline};
use crate::widget::RegionHandle;

/// How a widget's method table finds the region its glyphs paint from: a `FontString` is one, an
/// `EditBox` creates its own on demand. An error means the wrong receiver.
pub(super) type ResolveRegion = fn(&Lua, &Table) -> mlua::Result<RegionHandle>;

/// A widget's listener on its font instance, run after `SetFont` loads a new face or size and after
/// `SetFontObject` links another object: the notify `[vt+0x14]` that both paths call, which the
/// EditBox answers.
pub(super) type FontChanged = fn(&Lua, &Table) -> mlua::Result<()>;

/// `SetFont`'s shared argument gate: arg 2 must pass `lua_isstring` and arg 3 `lua_isnumber`, both
/// coercing, else it raises `Usage: %s:SetFont("font", fontHeight [, flags])`. An empty path is not
/// an argument error but a failed load, which the caller answers with nil.
pub(super) fn set_font_args(
    file: &Value,
    height: &Value,
    widget: &str,
) -> mlua::Result<(String, f32)> {
    let usage = || {
        mlua::Error::runtime(format!(
            "Usage: <{widget}>:SetFont(\"font\", fontHeight [, flags])"
        ))
    };
    let path = match file {
        Value::String(s) => s.to_str()?.to_string(),
        Value::Number(_) | Value::Integer(_) => as_f32(file).to_string(),
        _ => return Err(usage()),
    };
    let height = match height {
        Value::Number(_) | Value::Integer(_) => as_f32(height),
        Value::String(s) => s.to_str()?.parse::<f32>().map_err(|_| usage())?,
        _ => return Err(usage()),
    };
    Ok((path, height))
}

/// Install the ten font-block methods onto `m`, reading `this` through `resolve`; only for a
/// widget whose reference method table carries all ten.
pub(super) fn install(
    lua: &Lua,
    m: &Table,
    resolve: ResolveRegion,
    widget: &'static str,
    on_font_change: Option<FontChanged>,
) -> mlua::Result<()> {
    // ── the font object ── SetFontObject(font | "font" | nil) → nothing. Any other argument or an
    // unknown name raises: every rejection is `luaL_error`.
    m.set(
        "SetFontObject",
        lua.create_function(move |lua, (this, font): (Table, Value)| {
            let name = super::font::resolve("SetFontObject", &font)?;
            let rh = resolve(lua, &this)?;
            let relinked = {
                let mut model = crate::script::model_mut_of(lua);
                // nil severs the link and keeps the paint; the reference only nulls the parent.
                let Some(name) = name else {
                    model.region_data.entry(rh).or_default().font_object = None;
                    return Ok(());
                };
                let Some(fo) = model.font_object(&name).cloned() else {
                    return Err(mlua::Error::runtime(format!(
                        "SetFontObject: no font object named '{name}' is registered"
                    )));
                };
                let d = model.region_data.entry(rh).or_default();
                // The same object again changes nothing.
                let relinked = d
                    .font_object
                    .as_deref()
                    .is_none_or(|old| !old.eq_ignore_ascii_case(&name));
                d.font_object = Some(name);
                // The inherit mask stays: each local setter clears its bit (`FONTINSTANCE+0x2c`)
                // and nothing restores it, so a property set locally survives a later
                // `SetFontObject`.
                super::font::repaint(d, &fo);
                model.touch_measure(rh);
                relinked
            };
            if relinked {
                if let Some(changed) = on_font_change {
                    changed(lua, &this)?;
                }
            }
            Ok(())
        })?,
    )?;
    // GetFontObject() → the font object last set, never its name, or nil.
    m.set(
        "GetFontObject",
        lua.create_function(move |lua, this: Table| {
            let rh = resolve(lua, &this)?;
            let name = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model");
                model
                    .region_data
                    .get(&rh)
                    .and_then(|d| d.font_object.clone())
                    .filter(|n| model.font_object(n).is_some())
            };
            match name {
                Some(n) => Ok(Value::Table(super::font::wrapper(lua, &n)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // ── the face ── SetFont(path, height [, flags]) → the number 1, or nil on a failed load, never
    // a boolean. The EditBox shim passes it through (the reference leaves `eax` alone), unlike
    // `Button:SetFont`, which returns nothing.
    m.set(
        "SetFont",
        lua.create_function(
            move |lua, (this, file, height, flags): (Table, Value, Value, Option<String>)| {
                let (path, height) = set_font_args(&file, &height, widget)?;
                let rh = resolve(lua, &this)?;
                let mut model = crate::script::model_mut_of(lua);
                let before = model
                    .region_data
                    .get(&rh)
                    .map(|d| (d.font_path.clone(), d.font_height, d.outline));
                // A failed load is the host's probe to judge, as only the store knows the files;
                // with no probe, a non-empty path loads.
                let ok =
                    !path.is_empty() && model.font_probe.as_ref().is_none_or(|probe| probe(&path));
                let d = model.region_data.entry(rh).or_default();
                // Each supplied argument is an explicit set, kept through a later font-object
                // change; the height applies even when the face fails to load.
                if ok {
                    d.font_path = Some(path);
                    d.font_explicit.face = true;
                }
                d.font_height = Some(height);
                d.font_explicit.height = true;
                if let Some(f) = flags {
                    // The Lua spelling (`OUTLINE`, `THICKOUTLINE`), not the XML one, parsed as the
                    // reference parses it: a case-insensitive substring scan, so `THICKOUTLINE`
                    // sets both.
                    d.outline = Outline::flags(&f);
                    d.font_explicit.outline = true;
                }
                let after = (d.font_path.clone(), d.font_height, d.outline);
                model.touch_measure(rh);
                drop(model);
                // A loaded face that changed the path, height or flags notifies; a failed load does
                // not.
                if ok && before.as_ref() != Some(&after) {
                    if let Some(changed) = on_font_change {
                        changed(lua, &this)?;
                    }
                }
                Ok(if ok { Value::Number(1.0) } else { Value::Nil })
            },
        )?,
    )?;
    // GetFont() → path, height, flags (`mov eax,3`); flags is "" when none, never nil. The height
    // is a number even with no font: the reference's getter loads `[esi+0xe4]` unconditionally (it
    // skips its `+0xe0` test). That field has no constructor writer, so an unset height reads 0
    // here, like the Font object's `+0x48`.
    m.set(
        "GetFont",
        lua.create_function(move |lua, this: Table| {
            let rh = resolve(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            let d = model.region_data.get(&rh);
            let path = match d.and_then(|d| d.font_path.clone()) {
                Some(p) => Value::String(lua.create_string(&p)?),
                None => Value::Nil,
            };
            let height = d.and_then(|d| d.font_height).unwrap_or(0.0);
            let flags = d.map(|d| d.outline).unwrap_or_default().as_str();
            Ok((path, height, flags))
        })?,
    )?;

    // ── the text colour ── SetTextColor(r, g, b [, a]) → nothing. r, g and b are a bare
    // `lua_tonumber`, so nil reads 0.0 and the call never raises; alpha defaults to 1.0
    // (`lua_isnumber`-gated). The colour is the region's vertex colour, the `+0xb8` slot
    // `SetVertexColor` writes.
    m.set(
        "SetTextColor",
        lua.create_function(
            move |lua, (this, r, g, b, a): (Table, Value, Value, Value, Option<f32>)| {
                let (r, g, b) = (
                    super::object::as_f32(&r),
                    super::object::as_f32(&g),
                    super::object::as_f32(&b),
                );
                let rh = resolve(lua, &this)?;
                let mut model = crate::script::model_mut_of(lua);
                let d = model.region_data.entry(rh).or_default();
                d.vertex_color = Some([r, g, b, a.unwrap_or(1.0)]);
                // Clears the colour inherit bit: it survives a font-object repaint.
                d.font_explicit.color = true;
                Ok(())
            },
        )?,
    )?;
    // GetTextColor() → r, g, b, a; white when never set.
    m.set(
        "GetTextColor",
        lua.create_function(move |lua, this: Table| {
            let rh = resolve(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            let c = model
                .region_data
                .get(&rh)
                .and_then(|d| d.vertex_color)
                .unwrap_or([1.0, 1.0, 1.0, 1.0]);
            Ok((c[0], c[1], c[2], c[3]))
        })?,
    )?;

    // ── the shadow ── `SetShadowColor`, `GetShadowColor` (four values, not three: `mov eax,0x4`),
    // `SetShadowOffset`, `GetShadowOffset` (two, in UI units). Colour and offset share one inherit
    // slot, so each setter keeps the other half.
    m.set(
        "SetShadowColor",
        lua.create_function(
            // r, g and b are a bare `lua_tonumber`, as in `SetTextColor`.
            move |lua, (this, r, g, b, a): (Table, Value, Value, Value, Option<f32>)| {
                let (r, g, b) = (
                    super::object::as_f32(&r),
                    super::object::as_f32(&g),
                    super::object::as_f32(&b),
                );
                let rh = resolve(lua, &this)?;
                let mut model = crate::script::model_mut_of(lua);
                let d = model.region_data.entry(rh).or_default();
                let offset = d.font_shadow.map_or([0.0, 0.0], |s| s.offset);
                d.font_shadow = Some(FontShadow {
                    offset,
                    color: [r, g, b, a.unwrap_or(1.0)],
                });
                d.font_explicit.shadow = true;
                Ok(())
            },
        )?,
    )?;
    m.set(
        "GetShadowColor",
        lua.create_function(move |lua, this: Table| {
            let rh = resolve(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            let c = model
                .region_data
                .get(&rh)
                .and_then(|d| d.font_shadow)
                .map_or([0.0, 0.0, 0.0, 1.0], |s| s.color);
            Ok((c[0], c[1], c[2], c[3]))
        })?,
    )?;
    // SetShadowOffset(x, y): both required, else `Usage: %s:SetShadowOffset(x, y)`.
    m.set(
        "SetShadowOffset",
        lua.create_function(move |lua, (this, x, y): (Table, f32, f32)| {
            let rh = resolve(lua, &this)?;
            let mut model = crate::script::model_mut_of(lua);
            let d = model.region_data.entry(rh).or_default();
            let color = d.font_shadow.map_or([0.0, 0.0, 0.0, 1.0], |s| s.color);
            d.font_shadow = Some(FontShadow {
                offset: [x, y],
                color,
            });
            d.font_explicit.shadow = true;
            Ok(())
        })?,
    )?;
    m.set(
        "GetShadowOffset",
        lua.create_function(move |lua, this: Table| {
            let rh = resolve(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            let o = model
                .region_data
                .get(&rh)
                .and_then(|d| d.font_shadow)
                .map_or([0.0, 0.0], |s| s.offset);
            Ok((o[0], o[1]))
        })?,
    )?;

    Ok(())
}

/// Install `SetSpacing`/`GetSpacing` onto a text widget's table: the EditBox (`0x9756d0`),
/// MessageFrame (`0x974140`) and ScrollingMessageFrame (`0x972c20`) shims over the font's own
/// pair (`0x4a3f90`/`0x4a4040`). A FontString's pair is its own (`region::text`).
pub(super) fn install_spacing(
    lua: &Lua,
    m: &Table,
    resolve: ResolveRegion,
    on_font_change: Option<FontChanged>,
) -> mlua::Result<()> {
    // SetSpacing(spacing): a number, else `Usage: <name>:SetSpacing(spacing)` with the widget's
    // name; stored unclamped, it clears the inherit bit (`0x10`) and notifies the widget.
    m.set(
        "SetSpacing",
        lua.create_function(move |lua, (this, sp): (Table, Value)| {
            let Some(sp) = lua.coerce_number(sp)? else {
                let who: Option<String> =
                    this.get::<mlua::Function>("GetName")?.call(this.clone())?;
                return Err(mlua::Error::runtime(format!(
                    "Usage: {}:SetSpacing(spacing)",
                    who.as_deref()
                        .unwrap_or(super::object::anchor_args::UNNAMED)
                )));
            };
            let rh = resolve(lua, &this)?;
            {
                let mut model = crate::script::model_mut_of(lua);
                let d = model.region_data.entry(rh).or_default();
                d.font_spacing = sp as f32;
                d.font_explicit.spacing = true;
                model.touch_measure(rh);
                model.touch_layout_region(rh);
            }
            if let Some(changed) = on_font_change {
                changed(lua, &this)?;
            }
            Ok(())
        })?,
    )?;
    // GetSpacing() → the font's spacing, 0 when never set.
    m.set(
        "GetSpacing",
        lua.create_function(move |lua, this: Table| {
            let rh = resolve(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            Ok(f64::from(
                model.region_data.get(&rh).map_or(0.0, |d| d.font_spacing),
            ))
        })?,
    )
}
