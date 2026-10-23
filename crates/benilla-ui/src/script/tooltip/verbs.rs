//! The GameTooltip Lua methods over the parent module's line pool, plus the content builders'.

use mlua::{Lua, Table, Value, Variadic};

use crate::layout::{Anchor, Point};
use crate::script::binding_abi::optional_string;
use crate::script::object::{decode_id, frame_handle_of};
use crate::script::region::region_handle_of;
use crate::widget::{KindState, TooltipAnchor, TOOLTIP_LINE_GAP, TOOLTIP_PAD};

use super::{
    append_line, bool_arg, clear_content, display_name, fire_cleared, full_alpha, hide_tooltip,
    now, parse_line_color, parse_line_tail, reset_position, set_owner_core, set_shown, text_of,
    tip_mut, with_tip, write_cell, REG_TOOLTIP_METHODS,
};

pub(in crate::script) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // SetOwner(owner [, anchorType [, x, y]]) (`0x61eb40`): the full hide first (`0x61eb76`, before
    // any argument check), so a hover whose content call then finds nothing leaves no plate. The
    // owner checks raise in the binding's order, each naming the tooltip (`0x61ee6e`, `0x61ebc9`,
    // `0x61ec1c`, `0x61ec4a`); an anchorType that is not a string or number, or names no mode, is
    // mode 0 (`0x61ec64`); an offset that is not a number is 0 (`0x61edc7`). Then the core.
    m.set(
        "SetOwner",
        lua.create_function(
            |lua, (this, owner, anchor, x, y): (Table, Value, Value, Value, Value)| {
                let h = frame_handle_of(lua, &this)?;
                hide_tooltip(lua, h);
                let who = || display_name(lua, h);
                let Value::Table(owner) = owner else {
                    return Err(mlua::Error::runtime(format!(
                        "Usage: {}:SetOwner(frame)",
                        who()
                    )));
                };
                let Ok(owner_id) = decode_id(&owner) else {
                    return Err(mlua::Error::runtime(format!(
                        "{}:SetOwner(): Couldn't find 'this' in frame object",
                        who()
                    )));
                };
                let owner_h = {
                    let model = lua
                        .app_data_ref::<crate::script::ModelCell>()
                        .expect("model app_data");
                    model.id_to_frame.get(&owner_id).copied()
                };
                let Some(owner_h) = owner_h else {
                    return Err(mlua::Error::runtime(format!(
                        "{}:SetOwner(): Wrong object type, expected frame",
                        who()
                    )));
                };
                if owner_h == h {
                    return Err(mlua::Error::runtime(format!(
                        "{}:SetOwner(): Can't set owner to self",
                        who()
                    )));
                }
                let mode = parse_anchor(lua, "SetOwner", &anchor);
                let offset = (number_or_zero(lua, &x), number_or_zero(lua, &y));
                set_owner_core(lua, h, Some(owner_h), mode, offset);
                Ok(())
            },
        )?,
    )?;
    // SetAnchorType(anchorType [, x, y]) (`0x61d3d0`): SetOwner's mode parse and offsets with no
    // clear, alpha or event. The offsets always store; the mode only with an owner (`0x61d617`),
    // and the placement re-runs (`0x61d634`), doing nothing un-owned.
    m.set(
        "SetAnchorType",
        lua.create_function(|lua, (this, anchor, x, y): (Table, Value, Value, Value)| {
            let h = frame_handle_of(lua, &this)?;
            if optional_string(lua, &anchor).is_none() {
                return Err(mlua::Error::runtime(format!(
                    "Usage: {}:SetAnchorType( anchorType [,Xoffset] [,Yoffset] )",
                    display_name(lua, h)
                )));
            }
            let mode = parse_anchor(lua, "SetAnchorType", &anchor);
            let offset = (number_or_zero(lua, &x), number_or_zero(lua, &y));
            let mut model = crate::script::model_mut_of(lua);
            let t = tip_mut(&mut model, h)?;
            t.offset = offset;
            if t.owner.is_some() {
                t.anchor = mode;
            }
            reset_position(&mut model, h);
            Ok(())
        })?,
    )?;
    // GetAnchorType(): the mode by name (`0x61d650`, table `0x61d79c`), never nil. Any hide stores
    // mode 9, so a hidden or never-owned tooltip answers "ANCHOR_NONE".
    m.set(
        "GetAnchorType",
        lua.create_function(|lua, this: Table| {
            let anchor = with_tip(lua, &this, |t| t.anchor)?;
            Ok(anchor.name())
        })?,
    )?;
    // IsOwned(frame): the hover loop's OnUpdate gate (`ContainerFrame.lua:657`); the owner drops
    // on hide, so a stale OnUpdate never brings a hidden tooltip back.
    m.set(
        "IsOwned",
        lua.create_function(|lua, (this, frame): (Table, Table)| {
            let h = frame_handle_of(lua, &this)?;
            let fh = frame_handle_of(lua, &frame)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            match model.arena.frame(h).map(|f| &f.kind_state) {
                Some(KindState::Tooltip(t)) => Ok(t.owner == Some(fh)),
                _ => Err(mlua::Error::runtime("not a GameTooltip")),
            }
        })?,
    )?;
    // GetOwner() (`0x61d350`): the owner's object (`+0x29c`), or nil with none; `ActionButton_Update`
    // asks it of every button (`ActionButton.lua:265`).
    m.set(
        "GetOwner",
        lua.create_function(|lua, this: Table| {
            let Some(owner) = with_tip(lua, &this, |t| t.owner)? else {
                return Ok(Value::Nil);
            };
            let id = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model
                    .arena
                    .frame(owner)
                    .and(model.frame_to_id.get(&owner).copied())
            };
            match id {
                Some(id) => Ok(Value::Table(crate::script::object::frame_wrapper(lua, id)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // SetText(text, r, g, b, alpha, wrap) (`0x531b90`): alpha has its own number gate, default 1,
    // and a text that is neither string nor number raises the binding's usage error. It clears,
    // writes line 1 and shows: the empty-slot hover calls no Show after it
    // (PaperDollFrame.lua:747).
    m.set(
        "SetText",
        lua.create_function(|lua, (this, args): (Table, Variadic<Value>)| {
            let h = frame_handle_of(lua, &this)?;
            if !matches!(
                args.first(),
                Some(Value::String(_)) | Some(Value::Number(_)) | Some(Value::Integer(_))
            ) {
                return Err(mlua::Error::RuntimeError(
                    "Usage: SetText(\"text\" [, color])".into(),
                ));
            }
            let text = text_of(args.first());
            let [r, g, b, _] = parse_line_color(args.get(1), args.get(2), args.get(3));
            let a = match args.get(4) {
                Some(Value::Number(n)) => *n as f32,
                Some(Value::Integer(i)) => *i as f32,
                _ => 1.0,
            };
            let wrap = bool_arg(args.get(5));
            {
                let mut model = crate::script::model_mut_of(lua);
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            append_line(lua, &this, (text, [r, g, b, a]), None, wrap)?;
            set_shown(lua, h, true);
            Ok(())
        })?,
    )?;
    // AppendText(text) (`0x531e30`): appends to line 1's left cell in place, keeping its colour,
    // and does nothing with no lines, as the reference's body does (`0x5305b0`, line 1 at
    // `0x5305cc`). The bag tooltips hang a keybinding on their SetText title with it
    // (ContainerFrame.xml:198, MainMenuBarBagButtons.lua:91).
    m.set(
        "AppendText",
        lua.create_function(|lua, (this, text): (Table, Value)| {
            let h = frame_handle_of(lua, &this)?;
            let extra = text_of(Some(&text));
            let mut model = crate::script::model_mut_of(lua);
            let lh = match model.arena.frame(h).map(|f| &f.kind_state) {
                Some(KindState::Tooltip(t)) if t.num_lines > 0 => t.left_lines[0],
                Some(KindState::Tooltip(_)) => return Ok(()),
                _ => return Err(mlua::Error::runtime("not a GameTooltip")),
            };
            let (current, color) = match model.region_data.get(&lh) {
                Some(d) => (
                    d.text.clone().unwrap_or_default(),
                    d.vertex_color.unwrap_or([1.0, 1.0, 1.0, 1.0]),
                ),
                None => (String::new(), [1.0, 1.0, 1.0, 1.0]),
            };
            write_cell(&mut model, lh, &(current + &extra), color);
            Ok(())
        })?,
    )?;
    // AddLine(text [, r, g, b [, wrap]]): positional, and appends without showing; the
    // `(text, "", r, g, b)` shape has a non-number r-slot, so it renders default gold.
    m.set(
        "AddLine",
        lua.create_function(|lua, (this, args): (Table, Variadic<Value>)| {
            let text = text_of(args.first());
            let (color, wrap) = parse_line_tail(&args[1.min(args.len())..]);
            append_line(lua, &this, (text, color), None, wrap)
        })?,
    )?;
    // AddDoubleLine(textL, textR, rL, gL, bL, rR, gR, bR [, wrap]) (`0x531840`): each side's
    // colour gates on its own r-slot. The core forces wrap off when the right text is non-empty;
    // this never wraps.
    m.set(
        "AddDoubleLine",
        lua.create_function(
            |lua, (this, tl, tr, cl): (Table, Value, Value, Variadic<Value>)| {
                let left_color = parse_line_color(cl.first(), cl.get(1), cl.get(2));
                let right_color = parse_line_color(cl.get(3), cl.get(4), cl.get(5));
                append_line(
                    lua,
                    &this,
                    (text_of(Some(&tl)), left_color),
                    Some((text_of(Some(&tr)), right_color)),
                    false,
                )
            },
        )?,
    )?;
    m.set(
        "ClearLines",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            {
                let mut model = crate::script::model_mut_of(lua);
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            Ok(())
        })?,
    )?;
    m.set(
        "NumLines",
        lua.create_function(|lua, this: Table| with_tip(lua, &this, |t| t.num_lines as i64))?,
    )?;
    // AddFontStrings(left, right) (`0x530c40`): adopts two caller-made FontStrings as the next
    // line pair, placed like an engine-grown pair but keeping the caller's fonts.
    m.set(
        "AddFontStrings",
        lua.create_function(|lua, (this, left, right): (Table, Table, Table)| {
            let h = frame_handle_of(lua, &this)?;
            let lh = region_handle_of(lua, &left)?;
            let rh = region_handle_of(lua, &right)?;
            let mut model = crate::script::model_mut_of(lua);
            let frame_id = model.frame_id(h);
            let prev_left = match model.arena.frame(h).map(|f| &f.kind_state) {
                Some(KindState::Tooltip(t)) => t.left_lines.last().copied(),
                _ => return Err(mlua::Error::runtime("not a GameTooltip")),
            };
            let anchor = match prev_left {
                None => Anchor::new(
                    Point::TopLeft,
                    frame_id,
                    Point::TopLeft,
                    TOOLTIP_PAD,
                    -TOOLTIP_PAD,
                ),
                Some(prev) => {
                    let prev_id = model.region_id(prev);
                    Anchor::new(
                        Point::TopLeft,
                        prev_id,
                        Point::BottomLeft,
                        0.0,
                        -TOOLTIP_LINE_GAP,
                    )
                }
            };
            let left_id = model.region_id(lh);
            {
                let d = model.region_data.entry(lh).or_default();
                d.hidden = true;
                d.justify.set_h(crate::script::JustifyH::Left);
                d.anchors = vec![anchor];
            }
            {
                let d = model.region_data.entry(rh).or_default();
                d.hidden = true;
                d.justify.set_h(crate::script::JustifyH::Right);
                d.anchors = vec![Anchor::new(Point::Right, left_id, Point::Right, 0.0, 0.0)];
            }
            model.touch_layout(); // two line rows entered the layout graph
            let t = tip_mut(&mut model, h)?;
            t.left_lines.push(lh);
            t.right_lines.push(rh);
            Ok(())
        })?,
    )?;
    // SetMinimumWidth(w [, force]): the auto-size floor `SetTooltipMoney` raises (`0x61d040`).
    m.set(
        "SetMinimumWidth",
        lua.create_function(|lua, (this, w, force): (Table, f32, Value)| {
            let force = !matches!(force, Value::Nil | Value::Boolean(false));
            with_tip(lua, &this, |t| {
                t.min_width = w.max(0.0);
                t.min_width_forced = force;
            })
        })?,
    )?;
    // GetMinimumWidth() -> width, 1 or nil: the floor and the force flag (`0x61d0d0`).
    m.set(
        "GetMinimumWidth",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = crate::script::model_mut_of(lua);
            let t = tip_mut(&mut model, h)?;
            let forced = if t.min_width_forced {
                Value::Integer(1)
            } else {
                Value::Nil
            };
            Ok((f64::from(t.min_width), forced))
        })?,
    )?;
    // GetPadding() -> width (`0x61d1c0`).
    m.set(
        "GetPadding",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = crate::script::model_mut_of(lua);
            Ok(f64::from(tip_mut(&mut model, h)?.padding))
        })?,
    )?;
    // SetPadding(w): extra auto-size width that survives a content clear (ItemRefTooltip's 16
    // keeps its close button off the text, ItemRef.xml:40).
    m.set(
        "SetPadding",
        lua.create_function(|lua, (this, w): (Table, f32)| {
            with_tip(lua, &this, |t| t.padding = w.max(0.0))
        })?,
    )?;
    // FadeOut(): alpha ramps to 0 over `TOOLTIP_FADE_SECS`, then the full hide; new content or
    // Show cancels it.
    m.set(
        "FadeOut",
        lua.create_function(|lua, this: Table| {
            let t = now(lua);
            with_tip(lua, &this, |tip| {
                if tip.fade_start.is_none() {
                    tip.fade_start = Some(t);
                }
            })
        })?,
    )?;
    // Show and Hide shadow the shared pair. Show (`ShowThis`, `0x61cff0`) shows only a plate with
    // an owner and at least one line, at full alpha with the fade disarmed (`0x61d005`), checked
    // at Show time only; otherwise it takes the full hide (`0x61d028`). Hide is the full hide.
    m.set(
        "Show",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let live = {
                let mut model = crate::script::model_mut_of(lua);
                full_alpha(&mut model, h);
                tip_mut(&mut model, h)
                    .map(|t| t.owner.is_some() && t.num_lines > 0)
                    .unwrap_or(false)
            };
            match live {
                true => set_shown(lua, h, true),
                false => hide_tooltip(lua, h),
            }
            Ok(())
        })?,
    )?;
    m.set(
        "Hide",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            hide_tooltip(lua, h);
            Ok(())
        })?,
    )?;

    // The content methods: items, spells and auras, talents, units.
    crate::script::tooltip_item::install_methods(lua, &m)?;
    crate::script::tooltip_spell::install_methods(lua, &m)?;
    crate::script::talent::install_tooltip_method(lua, &m)?;
    crate::script::glyph::install_tooltip_method(lua, &m)?;
    crate::script::tooltip_unit::install_methods(lua, &m)?;
    crate::script::tooltip_item::wrap_setters(lua, &m)?;

    lua.set_named_registry_value(REG_TOOLTIP_METHODS, m)?;
    Ok(())
}

/// The twelve-name compare chain SetOwner and SetAnchorType share (`SStrCmpI`, `0x61ec8f`): any
/// case, and a value that is not a string or number, or matches no name, is mode 0. The warning
/// on an unknown name is ours; Lua sees no difference.
fn parse_anchor(lua: &Lua, verb: &str, anchor: &Value) -> TooltipAnchor {
    let Some(name) = optional_string(lua, anchor) else {
        return TooltipAnchor::Left;
    };
    TooltipAnchor::from_name(&name).unwrap_or_else(|| {
        crate::script::model_mut_of(lua).record_warning(format!(
            "{verb}: unknown anchor '{name}' (mode 0, ANCHOR_LEFT)"
        ));
        TooltipAnchor::Left
    })
}

/// An offset read as `lua_isnumber` gates it: a number or numeric string, anything else 0.
fn number_or_zero(lua: &Lua, v: &Value) -> f32 {
    match v {
        Value::Number(_) | Value::Integer(_) | Value::String(_) => {
            lua.coerce_number(v.clone()).ok().flatten().unwrap_or(0.0) as f32
        }
        _ => 0.0,
    }
}
