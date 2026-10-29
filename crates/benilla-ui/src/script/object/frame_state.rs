//! Frame methods: visibility, hierarchy, strata, level, scale, alpha, backdrop and input flags.

use mlua::{Lua, Table, Value};

use crate::script::region_map::{set_shared, Side};

use crate::order::Strata;
use crate::script::{event, Backdrop, Insets};
use crate::widget::FrameKind;

use super::{decode_id, draw_layer_from_str, frame_handle_of, frame_wrapper, strata_from_str};

/// Populate `m`'s visibility, hierarchy, strata, backdrop and input methods.
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // Show / Hide / visibility. No `SetShown`: no 1.12 method table registers it.
    m.set(
        "Show",
        lua.create_function(|lua, this: Table| set_shown(lua, &this, true))?,
    )?;
    m.set(
        "Hide",
        lua.create_function(|lua, this: Table| set_shown(lua, &this, false))?,
    )?;
    m.set(
        "IsShown",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            Ok(crate::script::binding_abi::flag(
                model.arena.frame(h).map(|f| f.shown).unwrap_or(false),
            ))
        })?,
    )?;
    m.set(
        "IsVisible",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            Ok(crate::script::binding_abi::flag(
                model
                    .arena
                    .frame(h)
                    .map(|f| f.effective_visible)
                    .unwrap_or(false),
            ))
        })?,
    )?;
    // Identity / hierarchy
    set_shared(lua, m, Side::Frame, "GetName", |lua, this: Table| {
        let h = frame_handle_of(lua, &this)?;
        let name = {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            model.arena.frame(h).and_then(|f| f.name.clone())
        };
        match name {
            Some(n) => Ok(Value::String(lua.create_string(&n)?)),
            None => Ok(Value::Nil),
        }
    })?;
    // GetID/SetID: a plain numeric label at `+0xb0` (XML `id=`), default 0.
    m.set(
        "GetID",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            Ok(model.arena.frame(h).map(|f| f.wow_id).unwrap_or(0))
        })?,
    )?;
    m.set(
        "SetID",
        lua.create_function(|lua, (this, id): (Table, i64)| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = crate::script::model_mut_of(lua);
            if let Some(f) = model.arena.frame_mut(h) {
                f.wow_id = id;
            }
            Ok(())
        })?,
    )?;
    // ── GetObjectType / IsObjectType, frame side ──
    //
    // Each class's chain is a fixed list in the reference, so a table here. `ScrollingMessageFrame`
    // derives from `Frame`, not `MessageFrame`; `SimpleHTML` keeps its capitals, unlike the
    // `SimpleHtml` variant; `Cooldown` is its own frame class (`0x5ebe70`).
    fn type_chain(kind: FrameKind) -> &'static [&'static str] {
        match kind {
            FrameKind::Frame => &["Frame", "Region"],
            FrameKind::WorldFrame => &["Frame", "Region"],
            FrameKind::Button => &["Button", "Frame", "Region"],
            FrameKind::CheckButton => &["CheckButton", "Button", "Frame", "Region"],
            FrameKind::EditBox => &["EditBox", "Frame", "Region"],
            FrameKind::StatusBar => &["StatusBar", "Frame", "Region"],
            FrameKind::Slider => &["Slider", "Frame", "Region"],
            FrameKind::ScrollFrame => &["ScrollFrame", "Frame", "Region"],
            FrameKind::Model => &["Model", "Frame", "Region"],
            FrameKind::PlayerModel => &["PlayerModel", "Model", "Frame", "Region"],
            FrameKind::DressUpModel => &["DressUpModel", "PlayerModel", "Model", "Frame", "Region"],
            FrameKind::TabardModel => &["TabardModel", "PlayerModel", "Model", "Frame", "Region"],
            FrameKind::MessageFrame => &["MessageFrame", "Frame", "Region"],
            FrameKind::ScrollingMessageFrame => &["ScrollingMessageFrame", "Frame", "Region"],
            FrameKind::ColorSelect => &["ColorSelect", "Frame", "Region"],
            FrameKind::SimpleHtml => &["SimpleHTML", "Frame", "Region"],
            FrameKind::MovieFrame => &["MovieFrame", "Frame", "Region"],
            FrameKind::GameTooltip => &["GameTooltip", "Frame", "Region"],
            FrameKind::Minimap => &["Minimap", "Frame", "Region"],
            // `IsObjectType` (`0x5ebe70`) answers its own id, then the frame chain's.
            FrameKind::Cooldown => &["Cooldown", "Frame", "Region"],
            // `IsObjectType` (`0x58fb00`) answers its own name, then the frame chain's.
            FrameKind::QuestPoiFrame => &["QuestPOIFrame", "Frame", "Region"],
        }
    }
    fn chain_of(lua: &Lua, this: &Table) -> mlua::Result<&'static [&'static str]> {
        let h = frame_handle_of(lua, this)?;
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model");
        Ok(model
            .arena
            .frame(h)
            .map_or(&["Frame", "Region"][..], |f| type_chain(f.kind)))
    }

    set_shared(lua, m, Side::Frame, "GetObjectType", |lua, this: Table| {
        Ok(Value::String(lua.create_string(chain_of(lua, &this)?[0])?))
    })?;

    // A case-insensitive whole-string match answering 1 or nil; a non-string, non-number argument
    // raises the reference's `Usage:`.
    set_shared(
        lua,
        m,
        Side::Frame,
        "IsObjectType",
        |lua, (this, want): (Table, Value)| {
            let chain = chain_of(lua, &this)?;
            let want = match &want {
                Value::String(s) => s.to_str()?.to_string(),
                Value::Number(n) => n.to_string(),
                Value::Integer(i) => i.to_string(),
                _ => {
                    let h = frame_handle_of(lua, &this)?;
                    let model = lua
                        .app_data_ref::<crate::script::ModelCell>()
                        .expect("model");
                    let who = model
                        .arena
                        .frame(h)
                        .and_then(|f| f.name.clone())
                        .unwrap_or_else(|| "<unnamed>".to_string());
                    return Err(mlua::Error::runtime(format!(
                        "Usage: {who}:IsObjectType(\"TYPE\")"
                    )));
                }
            };
            Ok(if chain.iter().any(|t| want.eq_ignore_ascii_case(t)) {
                Value::Number(1.0)
            } else {
                Value::Nil
            })
        },
    )?;

    // ── GetFrameType / IsFrameType ──
    //
    // The frame script's own spellings of the pair above (`GetFrameType`, `IsFrameType`), reading
    // the same per-class type slot: one name, or 1 or nil. An absent argument raises `Usage:` like
    // `IsObjectType`; the reference's absent-argument branch is untraced.
    m.set(
        "GetFrameType",
        lua.create_function(|lua, this: Table| {
            Ok(Value::String(lua.create_string(chain_of(lua, &this)?[0])?))
        })?,
    )?;
    m.set(
        "IsFrameType",
        lua.create_function(|lua, (this, want): (Table, Value)| {
            let chain = chain_of(lua, &this)?;
            let want = match &want {
                Value::String(s) => s.to_str()?.to_string(),
                Value::Number(n) => n.to_string(),
                Value::Integer(i) => i.to_string(),
                _ => {
                    let h = frame_handle_of(lua, &this)?;
                    let model = lua
                        .app_data_ref::<crate::script::ModelCell>()
                        .expect("model");
                    let who = model
                        .arena
                        .frame(h)
                        .and_then(|f| f.name.clone())
                        .unwrap_or_else(|| "<unnamed>".to_string());
                    return Err(mlua::Error::runtime(format!(
                        "Usage: {who}:IsFrameType(\"TYPE\")"
                    )));
                }
            };
            Ok(if chain.iter().any(|t| want.eq_ignore_ascii_case(t)) {
                Value::Number(1.0)
            } else {
                Value::Nil
            })
        })?,
    )?;

    set_shared(lua, m, Side::Frame, "GetParent", |lua, this: Table| {
        let h = frame_handle_of(lua, &this)?;
        let parent_id = {
            let mut model = crate::script::model_mut_of(lua);
            model
                .arena
                .frame(h)
                .and_then(|f| f.parent)
                .map(|p| model.frame_id(p))
        };
        match parent_id {
            Some(pid) => Ok(Value::Table(frame_wrapper(lua, pid)?)),
            None => Ok(Value::Nil),
        }
    })?;
    // ── GetChildren / GetNumChildren / GetRegions / GetNumRegions ──
    //
    // Link order, oldest first (both linkers, append at the tail), hidden ones included; the list
    // as separate values, none when empty, the count as one number. Each count shares its list's
    // walk.
    m.set(
        "GetChildren",
        lua.create_function(|lua, this: Table| {
            let ids = children_of(lua, &this)?;
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                out.push(Value::Table(frame_wrapper(lua, id)?));
            }
            Ok(mlua::MultiValue::from_vec(out))
        })?,
    )?;
    m.set(
        "GetNumChildren",
        lua.create_function(|lua, this: Table| Ok(children_of(lua, &this)?.len()))?,
    )?;
    m.set(
        "GetRegions",
        lua.create_function(|lua, this: Table| {
            let ids = regions_of(lua, &this)?;
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                out.push(Value::Table(crate::script::region::region_wrapper(
                    lua, id,
                )?));
            }
            Ok(mlua::MultiValue::from_vec(out))
        })?,
    )?;
    m.set(
        "GetNumRegions",
        lua.create_function(|lua, this: Table| Ok(regions_of(lua, &this)?.len()))?,
    )?;
    // `SetParent`: a frame, a name or an explicit nil; an absent argument raises like an unknown
    // name, and so does a cycle. As in the reference, the hide cascade runs before the move and the
    // show cascade after it.
    set_shared(
        lua,
        m,
        Side::Frame,
        "SetParent",
        |lua, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let this = match it.next() {
                Some(Value::Table(t)) => t,
                _ => return Err(mlua::Error::runtime("expected a frame")),
            };
            let h = frame_handle_of(lua, &this)?;
            let who = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model");
                model
                    .arena
                    .frame(h)
                    .and_then(|f| f.name.clone())
                    .unwrap_or_else(|| "<unnamed>".to_string())
            };
            let parent_arg = it.next();
            // `_G[name]` is read with no guard alive and no `$parent` expansion: the reference's
            // binding calls the name lookup directly, and only the layout path's lookup expands the
            // token.
            let named = match &parent_arg {
                Some(Value::String(s)) => Some(match s.to_str() {
                    Ok(n) => super::prefetch_named_target(lua, n.as_ref(), None),
                    Err(_) => super::NamedTarget::unreadable(),
                }),
                _ => None,
            };
            let new_parent = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model");
                match &parent_arg {
                    // An explicit nil reparents to the screen root, resetting strata and level.
                    Some(Value::Nil) => None,
                    Some(Value::Table(t)) => Some(
                        decode_id(t)
                            .ok()
                            .and_then(|id| model.id_to_frame.get(&id).copied())
                            .ok_or_else(|| {
                                mlua::Error::runtime(format!(
                                    "{who}:SetParent(): Couldn't find region named ''"
                                ))
                            })?,
                    ),
                    Some(Value::String(_)) => {
                        // The reference accepts only a frame, not any region, so a region's name
                        // fails like an absent one.
                        let nt = named.as_ref().expect("a String argument is prefetched");
                        let hit = super::resolve_named_target(&model, nt)
                            .and_then(|id| model.id_to_frame.get(&id).copied());
                        Some(hit.ok_or_else(|| {
                            mlua::Error::runtime(format!(
                                "{who}:SetParent(): Couldn't find region named '{}'",
                                nt.name
                            ))
                        })?)
                    }
                    // Absent, or any other type: the reference raises first.
                    _ => {
                        return Err(mlua::Error::runtime(format!(
                            "{who}:SetParent(): Couldn't find region named ''"
                        )))
                    }
                }
            };
            // The cycle guard raises (the reference's ancestor walk, the frame itself included).
            if let Some(np) = new_parent {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model");
                if np == h || model.arena.is_ancestor(h, np) {
                    let pname = model
                        .arena
                        .frame(np)
                        .and_then(|f| f.name.clone())
                        .unwrap_or_else(|| "<unnamed>".to_string());
                    return Err(mlua::Error::runtime(format!(
                        "{who}:SetParent(): Would create a loop parenting to {pname}"
                    )));
                }
            }
            // The same parent is a total no-op (`0x4911b0`), not even a layout touch.
            let moves = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model");
                model.arena.reparent_moves(h, new_parent)
            };
            if !moves {
                return Ok(());
            }
            event::hide_this(lua, h);
            let relinked = {
                let mut model = crate::script::model_mut_of(lua);
                let relinked = model.arena.reparent_relink(h, new_parent);
                if relinked {
                    // A reparent moves the subtree's effective scale, a layout input and part of
                    // every descendant FontString's measure key.
                    model.touch_layout_reparent(h);
                    model.touch_measure_all();
                }
                relinked
            };
            if relinked {
                event::show_this(lua, h);
            }
            Ok(())
        },
    )?;
    // Strata / level / scale / alpha
    m.set(
        "SetFrameStrata",
        lua.create_function(|lua, (this, strata): (Table, String)| {
            let h = frame_handle_of(lua, &this)?;
            // A name off the table raises, naming the frame or `<unnamed>`, and the stratum stays.
            let Some(s) = strata_from_str(&strata) else {
                let name = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model")
                    .arena
                    .frame(h)
                    .and_then(|f| f.name.clone())
                    .unwrap_or_else(|| "<unnamed>".to_string());
                return Err(mlua::Error::runtime(format!(
                    "{name}:SetFrameStrata(): Unknown frame strata: {strata}"
                )));
            };
            crate::script::model_mut_of(lua)
                .arena
                .set_frame_strata(h, s);
            Ok(())
        })?,
    )?;
    m.set(
        "GetFrameStrata",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            let s = model.arena.frame(h).map(|f| f.strata).unwrap_or_default();
            Ok(strata_name(s).to_string())
        })?,
    )?;
    m.set(
        "SetFrameLevel",
        lua.create_function(|lua, (this, level): (Table, i64)| {
            let h = frame_handle_of(lua, &this)?;
            let lvl = level.clamp(0, i64::from(u16::MAX)) as u16;
            // A script level change carries the same-strata children along: `0x49e9d0` calls
            // the frame-level set with `propagate = 1` (`0x49ea50`).
            crate::script::model_mut_of(lua)
                .arena
                .set_frame_level(h, lvl, true);
            Ok(())
        })?,
    )?;
    m.set(
        "GetFrameLevel",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            Ok(i64::from(
                model.arena.frame(h).map(|f| f.level).unwrap_or(0),
            ))
        })?,
    )?;
    m.set(
        "SetScale",
        lua.create_function(|lua, (this, scale): (Table, f32)| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = crate::script::model_mut_of(lua);
            let changed = model.arena.frame(h).is_some_and(|f| f.scale != scale);
            model.arena.set_scale(h, scale);
            if changed {
                // Only the subtree's effective scale moves, as for a reparent.
                model.touch_layout_reparent(h);
                // The owner's effective scale is in every descendant FontString's measure key.
                model.touch_measure_all();
            }
            Ok(())
        })?,
    )?;
    m.set(
        "GetScale",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            Ok(model.arena.frame(h).map(|f| f.scale).unwrap_or(1.0))
        })?,
    )?;
    m.set(
        "SetAlpha",
        lua.create_function(|lua, (this, alpha): (Table, f32)| {
            let h = frame_handle_of(lua, &this)?;
            // The reference clamps to 0..1; stock fade code passing a 0..255 alpha relies on it.
            let alpha = alpha.clamp(0.0, 1.0);
            let mut model = crate::script::model_mut_of(lua);
            // Paint only: the alpha carries down the subtree, whose targets the extract re-reads.
            let targets = model.arena.subtree_targets(h);
            model.paint_mut(&targets).arena.set_alpha(h, alpha);
            Ok(())
        })?,
    )?;
    m.set(
        "GetAlpha",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            Ok(model.arena.frame(h).map(|f| f.alpha).unwrap_or(1.0))
        })?,
    )?;
    // `SetBackdrop(table|nil)` installs or removes the tiled background and 8-piece border, both
    // white until tinted; the two colour setters tint the background and the border respectively.
    m.set(
        "SetBackdrop",
        lua.create_function(|lua, (this, arg): (Table, Value)| {
            let h = frame_handle_of(lua, &this)?;
            let bd = match arg {
                Value::Nil => None,
                Value::Table(t) => Some(backdrop_from_table(&t)?),
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "SetBackdrop: expected a table or nil, got {}",
                        other.type_name()
                    )))
                }
            };
            let mut model = crate::script::model_mut_of(lua);
            match bd {
                Some(b) => {
                    model.backdrops.insert(h, b);
                }
                None => {
                    model.backdrops.remove(&h);
                }
            }
            Ok(())
        })?,
    )?;
    // `GetBackdrop([table])` rebuilds the table from the stored struct: no backdrop is zero values,
    // not nil; an omitted key reads its ctor default (files `""`, `tileSize` 0, `edgeSize` 32);
    // `tile` is 1 or absent, never a boolean; a table argument is filled in place, its `insets`
    // reused. No colour keys.
    m.set(
        "GetBackdrop",
        lua.create_function(|lua, (this, target): (Table, Value)| {
            let h = frame_handle_of(lua, &this)?;
            // Copied out before any Lua write: filling a caller's table can run a `__newindex` that
            // re-enters and would panic on the app-data borrow.
            let bd = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model");
                model.backdrops.get(&h).cloned()
            };
            let Some(bd) = bd else {
                return Ok(mlua::MultiValue::new());
            };
            let t = match target {
                Value::Table(t) => t,
                _ => lua.create_table()?,
            };
            t.set("bgFile", bd.bg_file.as_deref().unwrap_or(""))?;
            t.set("edgeFile", bd.edge_file.as_deref().unwrap_or(""))?;
            t.set(
                "tile",
                if bd.tile {
                    Value::Number(1.0)
                } else {
                    Value::Nil // erases the key from a reused table
                },
            )?;
            t.set("tileSize", f64::from(bd.tile_size))?;
            t.set("edgeSize", f64::from(bd.edge_size))?;
            let insets = match t.get::<Value>("insets") {
                Ok(Value::Table(existing)) => existing,
                _ => {
                    let fresh = lua.create_table()?;
                    t.set("insets", &fresh)?;
                    fresh
                }
            };
            insets.set("left", f64::from(bd.insets.left))?;
            insets.set("right", f64::from(bd.insets.right))?;
            insets.set("top", f64::from(bd.insets.top))?;
            insets.set("bottom", f64::from(bd.insets.bottom))?;
            Ok(mlua::MultiValue::from_vec(vec![Value::Table(t)]))
        })?,
    )?;
    m.set(
        "SetBackdropColor",
        lua.create_function(|lua, (this, args): (Table, mlua::MultiValue)| {
            let color = backdrop_color(lua, args);
            let h = frame_handle_of(lua, &this)?;
            let mut model = crate::script::model_mut_of(lua);
            if let Some(bd) = model.backdrops.get_mut(&h) {
                bd.bg_color = color;
            }
            Ok(())
        })?,
    )?;
    m.set(
        "SetBackdropBorderColor",
        lua.create_function(|lua, (this, args): (Table, mlua::MultiValue)| {
            let color = backdrop_color(lua, args);
            let h = frame_handle_of(lua, &this)?;
            let mut model = crate::script::model_mut_of(lua);
            if let Some(bd) = model.backdrops.get_mut(&h) {
                bd.border_color = color;
            }
            Ok(())
        })?,
    )?;
    m.set(
        "GetBackdropColor",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            match model.backdrops.get(&h) {
                Some(bd) => Ok((
                    bd.bg_color[0],
                    bd.bg_color[1],
                    bd.bg_color[2],
                    bd.bg_color[3],
                )),
                None => Ok((1.0, 1.0, 1.0, 1.0)),
            }
        })?,
    )?;
    m.set(
        "GetBackdropBorderColor",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            match model.backdrops.get(&h) {
                Some(bd) => Ok((
                    bd.border_color[0],
                    bd.border_color[1],
                    bd.border_color[2],
                    bd.border_color[3],
                )),
                None => Ok((1.0, 1.0, 1.0, 1.0)),
            }
        })?,
    )?;
    // Mouse interaction: `EnableMouse` gates hit-testing.
    m.set(
        "EnableMouse",
        lua.create_function(|lua, (this, enable): (Table, bool)| {
            let h = frame_handle_of(lua, &this)?;
            crate::script::model_mut_of(lua)
                .arena
                .set_mouse_enabled(h, enable);
            Ok(())
        })?,
    )?;
    m.set(
        "IsMouseEnabled",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            Ok(crate::script::binding_abi::flag(
                model.arena.is_mouse_enabled(h),
            ))
        })?,
    )?;
    // EnableKeyboard / IsKeyboardEnabled: the flag key delivery gates on (`script::keyboard`),
    // separate from having a handler.
    m.set(
        "EnableKeyboard",
        lua.create_function(|lua, (this, enable): (Table, bool)| {
            let h = frame_handle_of(lua, &this)?;
            crate::script::model_mut_of(lua)
                .arena
                .set_keyboard_enabled(h, enable);
            Ok(())
        })?,
    )?;
    m.set(
        "IsKeyboardEnabled",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            Ok(crate::script::binding_abi::flag(
                model.arena.is_keyboard_enabled(h),
            ))
        })?,
    )?;
    // EnableDrawLayer / DisableDrawLayer: the frame's layer mask. An unknown layer name is a no-op
    // here; the reference's handling of one is untraced.
    for (name, disable) in [("EnableDrawLayer", false), ("DisableDrawLayer", true)] {
        m.set(
            name,
            lua.create_function(move |lua, (this, layer): (Table, Value)| {
                let Some(l) = layer
                    .as_string()
                    .and_then(|s| s.to_str().ok().and_then(|s| draw_layer_from_str(&s)))
                else {
                    return Ok(());
                };
                let h = frame_handle_of(lua, &this)?;
                let mut model = crate::script::model_mut_of(lua);
                if let Some(frame) = model.arena.frame_mut(h) {
                    let bit = 1u8 << l.index();
                    if disable {
                        frame.disabled_layers |= bit;
                    } else {
                        frame.disabled_layers &= !bit;
                    }
                }
                Ok(())
            })?,
        )?;
    }
    // EnableMouseWheel / IsMouseWheelEnabled: the wheel's own flag, separate from `EnableMouse` as
    // in the reference, and the one the wheel hit test gates on.
    m.set(
        "EnableMouseWheel",
        lua.create_function(|lua, (this, enable): (Table, bool)| {
            let h = frame_handle_of(lua, &this)?;
            crate::script::model_mut_of(lua)
                .arena
                .set_mouse_wheel_enabled(h, enable);
            Ok(())
        })?,
    )?;
    m.set(
        "IsMouseWheelEnabled",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            Ok(crate::script::binding_abi::flag(
                model.arena.is_mouse_wheel_enabled(h),
            ))
        })?,
    )?;
    // Clamp-to-screen (geometry flags bit 4): the layout resolve keeps the frame's rect inside the
    // window, size preserved.
    m.set(
        "SetClampedToScreen",
        lua.create_function(|lua, (this, clamp): (Table, bool)| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = crate::script::model_mut_of(lua);
            let changed = model.arena.is_clamped_to_screen(h) != clamp;
            model.arena.set_clamped_to_screen(h, clamp);
            if changed {
                model.sync_layout_input(h);
                model.touch_layout_frame(h);
            }
            Ok(())
        })?,
    )?;
    m.set(
        "IsClampedToScreen",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            Ok(crate::script::binding_abi::flag(
                model.arena.is_clamped_to_screen(h),
            ))
        })?,
    )?;
    // IgnoreDepth(ignore) (`0x4a1d80`): a boolean alone, else the named Usage.
    m.set(
        "IgnoreDepth",
        lua.create_function(|lua, (this, ignore): (Table, Value)| {
            let Value::Boolean(ignore) = ignore else {
                let who: Option<String> =
                    this.get::<mlua::Function>("GetName")?.call(this.clone())?;
                return Err(mlua::Error::runtime(format!(
                    "Usage: {}:IgnoreDepth(ignore)",
                    who.as_deref().unwrap_or(super::anchor_args::UNNAMED)
                )));
            };
            let h = frame_handle_of(lua, &this)?;
            if let Some(f) = crate::script::model_mut_of(lua).arena.frame_mut(h) {
                f.ignore_depth = ignore;
            }
            Ok(())
        })?,
    )?;
    // IsIgnoringDepth (`0x4a1e00`): 1 or nil.
    m.set(
        "IsIgnoringDepth",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            Ok(crate::script::binding_abi::flag(
                model.arena.frame(h).is_some_and(|f| f.ignore_depth),
            ))
        })?,
    )?;
    // SetClampRectInsets(left, right, top, bottom) (`0x4a0310`): four numbers, else the named
    // Usage (`0x9ecf00`); the clamp lets the frame that far past each screen edge (`0x48944e`).
    m.set(
        "SetClampRectInsets",
        lua.create_function(
            |lua, (this, left, right, top, bottom): (Table, Value, Value, Value, Value)| {
                let num = |v: Value| lua.coerce_number(v).ok().flatten();
                let (Some(l), Some(r), Some(t), Some(b)) =
                    (num(left), num(right), num(top), num(bottom))
                else {
                    let who: Option<String> =
                        this.get::<mlua::Function>("GetName")?.call(this.clone())?;
                    return Err(mlua::Error::runtime(format!(
                        "Usage: {}:SetClampRectInsets(left, right, top, bottom)",
                        who.as_deref().unwrap_or(super::anchor_args::UNNAMED)
                    )));
                };
                let h = frame_handle_of(lua, &this)?;
                let mut model = crate::script::model_mut_of(lua);
                let insets = [l as f32, r as f32, t as f32, b as f32];
                if model.arena.clamp_rect_insets(h) != insets {
                    model.arena.set_clamp_rect_insets(h, insets);
                    model.sync_layout_input(h);
                    model.touch_layout_frame(h);
                }
                Ok(())
            },
        )?,
    )?;
    // GetClampRectInsets() → left, right, top, bottom (`0x4a0230`).
    m.set(
        "GetClampRectInsets",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            let i = model.arena.clamp_rect_insets(h);
            Ok((
                f64::from(i[0]),
                f64::from(i[1]),
                f64::from(i[2]),
                f64::from(i[3]),
            ))
        })?,
    )?;
    // Hit-rect insets shrink only the mouse hit rect; geometry, drawing and anchors ignore them.
    m.set(
        "SetHitRectInsets",
        lua.create_function(
            |lua, (this, left, right, top, bottom): (Table, f32, f32, f32, f32)| {
                let h = frame_handle_of(lua, &this)?;
                crate::script::model_mut_of(lua)
                    .arena
                    .set_hit_rect_insets(h, [left, right, top, bottom]);
                Ok(())
            },
        )?,
    )?;
    m.set(
        "GetHitRectInsets",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            let i = model.arena.hit_rect_insets(h);
            Ok((i[0], i[1], i[2], i[3]))
        })?,
    )?;
    Ok(())
}

/// Parse a `SetBackdrop` table: exactly the reference's keys, a wrong-typed one read as absent and
/// a missing one left at the ctor default (`edgeSize` 32); no colour keys.
fn backdrop_from_table(t: &Table) -> mlua::Result<Backdrop> {
    let mut bd = Backdrop::default();
    if let Ok(Value::String(s)) = t.get::<Value>("bgFile") {
        bd.bg_file = Some(s.to_str()?.to_string());
    }
    if let Ok(Value::String(s)) = t.get::<Value>("edgeFile") {
        bd.edge_file = Some(s.to_str()?.to_string());
    }
    bd.tile = !matches!(
        t.get::<Value>("tile").unwrap_or(Value::Nil),
        Value::Nil | Value::Boolean(false)
    );
    if let Ok(Value::Number(n)) = t.get::<Value>("tileSize") {
        bd.tile_size = n as f32;
    } else if let Ok(Value::Integer(n)) = t.get::<Value>("tileSize") {
        bd.tile_size = n as f32;
    }
    if let Ok(Value::Number(n)) = t.get::<Value>("edgeSize") {
        bd.edge_size = n as f32;
    } else if let Ok(Value::Integer(n)) = t.get::<Value>("edgeSize") {
        bd.edge_size = n as f32;
    }
    if let Ok(Value::Table(ins)) = t.get::<Value>("insets") {
        let num = |k: &str| -> f32 {
            match ins.get::<Value>(k) {
                Ok(Value::Number(n)) => n as f32,
                Ok(Value::Integer(n)) => n as f32,
                _ => 0.0,
            }
        };
        bd.insets = Insets {
            left: num("left"),
            right: num("right"),
            top: num("top"),
            bottom: num("bottom"),
        };
    }
    Ok(bd)
}

fn strata_name(s: Strata) -> &'static str {
    match s {
        Strata::World => "WORLD",
        Strata::Background => "BACKGROUND",
        Strata::Low => "LOW",
        Strata::Medium => "MEDIUM",
        Strata::High => "HIGH",
        Strata::Dialog => "DIALOG",
        Strata::Fullscreen => "FULLSCREEN",
        Strata::FullscreenDialog => "FULLSCREEN_DIALOG",
        Strata::Tooltip => "TOOLTIP",
    }
}

fn set_shown(lua: &Lua, this: &Table, shown: bool) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    event::set_shown(lua, h, shown);
    Ok(())
}

/// The frame's child ids in link order, the one walk `GetChildren` and `GetNumChildren` share.
fn children_of(lua: &Lua, this: &Table) -> mlua::Result<Vec<u32>> {
    let h = frame_handle_of(lua, this)?;
    let mut model = crate::script::model_mut_of(lua);
    let Some(children) = model.arena.frame(h).map(|f| f.children.clone()) else {
        return Ok(Vec::new());
    };
    Ok(children.into_iter().map(|c| model.frame_id(c)).collect())
}

/// The frame's region ids in link order, the one walk `GetRegions` and `GetNumRegions` share. A
/// detached region is left out (`SetParent(nil)` unlinks it), and so is the title region, which
/// never reaches the linker; a Button's label and state textures are in.
fn regions_of(lua: &Lua, this: &Table) -> mlua::Result<Vec<u32>> {
    let h = frame_handle_of(lua, this)?;
    let mut model = crate::script::model_mut_of(lua);
    let Some((regions, title)) = model
        .arena
        .frame(h)
        .map(|f| (f.regions.clone(), f.title_region))
    else {
        return Ok(Vec::new());
    };
    let live: Vec<_> = regions
        .into_iter()
        .filter(|r| Some(*r) != title)
        .filter(|r| model.arena.region(*r).is_some_and(|reg| !reg.detached))
        .collect();
    Ok(live.into_iter().map(|r| model.region_id(r)).collect())
}

/// One backdrop colour channel as `SetBackdropColor` and `SetBackdropBorderColor` convert it: r, g,
/// b through a bare `lua_tonumber` (nil is 0), alpha gated by `lua_isnumber` with 1.0 staged;
/// clamped to 0..1 with NaN at 1, then stored as a byte, rounded half up (`×255 + 0.5`, `__ftol`).
fn backdrop_channel(lua: &Lua, v: Option<Value>, gated: bool) -> f32 {
    let x = if gated {
        v.and_then(|v| lua.coerce_number(v).ok().flatten())
            .unwrap_or(1.0)
    } else {
        crate::script::binding_abi::coerced_number(lua, v)
    };
    let clamped = if x.is_nan() { 1.0 } else { x.clamp(0.0, 1.0) };
    let byte = (clamped * 255.0 + 0.5) as u8;
    f32::from(byte) / 255.0
}

/// A backdrop colour setter's four channels; a missing argument and an explicit nil read alike.
fn backdrop_color(lua: &Lua, args: mlua::MultiValue) -> [f32; 4] {
    let a: Vec<Value> = args.into_iter().collect();
    [
        backdrop_channel(lua, a.first().cloned(), false),
        backdrop_channel(lua, a.get(1).cloned(), false),
        backdrop_channel(lua, a.get(2).cloned(), false),
        backdrop_channel(lua, a.get(3).cloned(), true),
    ]
}
