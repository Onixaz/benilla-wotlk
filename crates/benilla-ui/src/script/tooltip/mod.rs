//! The `GameTooltip` method surface and engine behaviour: the line stack, owner and anchor,
//! auto-size and the fade.
//!
//! Lines are named FontString regions published as Lua globals (`<name>TextLeft1` …), since
//! FrameXML addresses them by name (`GameTooltipTextLeft1:SetTextColor`, `UnitFrame.lua:76`). The
//! template declares 30 pairs, which are adopted; the reference grows past them through
//! `AddFontStrings`, and this also creates pairs on demand.
//!
//! Layout is a pre-pass of every `resolve` ([`layout_tooltips`]): the width is the widest line (a
//! double line adds the column gap) plus padding, floored by `SetMinimumWidth`; the height is the
//! summed line heights and gaps; each right column sits flush with the text inset.

use mlua::{Lua, Table, Value};

use super::event;
use super::object::{frame_handle_of, publish_global};
use super::region::region_wrapper;
use super::{FontObject, Model, RegionData};
use crate::layout::{Anchor, Point};
use crate::order::DrawLayer;
use crate::widget::{
    FrameHandle, KindState, RegionKind, TooltipAnchor, TooltipState, TOOLTIP_DOUBLE_GAP,
    TOOLTIP_FADE_SECS, TOOLTIP_LINE_GAP, TOOLTIP_PAD,
};

/// Registry key of the GameTooltip method table, a named registry root (the MAXCSTACK discipline).
pub(super) const REG_TOOLTIP_METHODS: &str = "__benilla_tooltip_methods";

impl super::UiScript {
    /// The name of the frame that owns the GameTooltip named `tooltip`; `None` for no such
    /// tooltip, no owner or an unnamed one. 1.12 answers only `IsOwned(frame)`, so the host's
    /// recorders read the owner here.
    pub fn tooltip_owner_name(&self, tooltip: &str) -> Option<String> {
        let model = self.model_ref();
        let h = model.arena.lookup(tooltip)?;
        let owner = match &model.arena.frame(h)?.kind_state {
            KindState::Tooltip(t) => t.owner?,
            _ => return None,
        };
        model.arena.frame(owner)?.name.clone()
    }
}

/// Runs `f` over a GameTooltip's state under one short write borrow.
fn with_tip<T>(lua: &Lua, this: &Table, f: impl FnOnce(&mut TooltipState) -> T) -> mlua::Result<T> {
    let h = frame_handle_of(lua, this)?;
    let mut model = crate::script::model_mut_of(lua);
    tip_mut(&mut model, h).map(f)
}

pub(super) fn tip_mut(model: &mut Model, h: FrameHandle) -> mlua::Result<&mut TooltipState> {
    match model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        Some(KindState::Tooltip(t)) => Ok(t),
        _ => Err(mlua::Error::runtime("not a GameTooltip")),
    }
}

/// `SetFontObject` onto a line cell, except that a missing font object keeps the default face
/// rather than raising (a VM loaded without Fonts.xml).
fn apply_font(d: &mut RegionData, name: &str, fo: Option<&FontObject>) {
    d.font_object = Some(name.to_string());
    // The inherit mask (`+0x2c`) stays: a re-point does not restore inheritance in the reference.
    // From here the line follows the font object live, through `font::propagate`.
    if let Some(f) = fo {
        d.font_path = f.font.clone();
        d.font_height = f.height;
        d.outline = f.outline;
        d.font_shadow = f.shadow;
        if let Some(c) = f.color {
            d.vertex_color = Some(c);
        }
    }
}

/// Grows the line-pair pool to `n` pairs: XML-declared pairs (`<name>TextLeft<i>`) are adopted
/// with their fonts, and created pairs clone the previous pair's fonts past a declared ladder, or
/// else wear the template's `GameTooltipHeaderText` (line 1) and `GameTooltipText`.
fn ensure_lines(lua: &Lua, this: &Table, n: usize) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    // (id, global name) pairs to publish once the model borrow is dropped.
    let mut publish: Vec<(u32, String)> = Vec::new();
    {
        let mut model = crate::script::model_mut_of(lua);
        let frame_name = model
            .arena
            .frame(h)
            .and_then(|f| f.name.clone())
            .unwrap_or_default();
        let frame_id = model.frame_id(h);
        loop {
            let (have, prev_left, prev_right, clone_prev) =
                match model.arena.frame(h).map(|f| &f.kind_state) {
                    Some(KindState::Tooltip(t)) => (
                        t.left_lines.len(),
                        t.left_lines.last().copied(),
                        t.right_lines.last().copied(),
                        t.xml_declared_lines,
                    ),
                    _ => return Err(mlua::Error::runtime("not a GameTooltip")),
                };
            if have >= n {
                break;
            }
            let i = have + 1;

            // The template's declared pair for this index (ShoppingTooltipTemplate's ladder).
            let declared = |model: &Model, cell: &str| -> Option<crate::widget::RegionHandle> {
                if frame_name.is_empty() {
                    return None;
                }
                let id = model
                    .region_names
                    .get(&format!("{frame_name}Text{cell}{i}"))
                    .copied()?;
                let rh = model.id_to_region.get(&id).copied()?;
                (model.arena.region(rh)?.owner == h).then_some(rh)
            };
            // A created cell's fonts: the previous line's under an XML ladder, else the defaults.
            let seed_font =
                |model: &mut Model,
                 d: &mut RegionData,
                 prev: Option<crate::widget::RegionHandle>| {
                    let cloned = clone_prev
                        .then(|| prev.and_then(|p| model.region_data.get(&p)).cloned())
                        .flatten();
                    match cloned {
                        Some(src) => {
                            d.font_object = src.font_object.clone();
                            d.font_path = src.font_path.clone();
                            d.font_height = src.font_height;
                            d.outline = src.outline;
                            d.font_shadow = src.font_shadow;
                            d.vertex_color = src.vertex_color;
                        }
                        None => {
                            let font = if i == 1 {
                                "GameTooltipHeaderText"
                            } else {
                                "GameTooltipText"
                            };
                            let fo = model.font_object(font).cloned();
                            apply_font(d, font, fo.as_ref());
                        }
                    }
                };

            let adopted_left = declared(&model, "Left");
            let left = match adopted_left {
                Some(rh) => rh,
                None => model
                    .arena
                    .create_region(h, RegionKind::FontString, DrawLayer::Artwork, 0)
                    .ok_or_else(|| mlua::Error::runtime("dead tooltip frame"))?,
            };
            {
                let mut d = if adopted_left.is_some() {
                    // Keep the declared faces; the engine owns justify/visibility/anchors.
                    model.region_data.remove(&left).unwrap_or_default()
                } else {
                    let mut d = RegionData {
                        hidden: true,
                        ..RegionData::default()
                    };
                    seed_font(&mut model, &mut d, prev_left);
                    d
                };
                d.hidden = true;
                // Left-justified as in the template; it shows once a wrap line pins its width.
                d.justify.set_h(super::JustifyH::Left);
                d.anchors = vec![match prev_left {
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
                }];
                model.region_data.insert(left, d);
                model.touch_measure(left); // an adopted cell can arrive text-in-hand
                model.touch_layout(); // a line row entered the layout graph
            }
            let left_id = model.region_id(left);

            let adopted_right = declared(&model, "Right");
            let right = match adopted_right {
                Some(rh) => rh,
                None => model
                    .arena
                    .create_region(h, RegionKind::FontString, DrawLayer::Artwork, 0)
                    .ok_or_else(|| mlua::Error::runtime("dead tooltip frame"))?,
            };
            {
                let mut d = if adopted_right.is_some() {
                    model.region_data.remove(&right).unwrap_or_default()
                } else {
                    let mut d = RegionData {
                        hidden: true,
                        ..RegionData::default()
                    };
                    seed_font(&mut model, &mut d, prev_right);
                    d
                };
                d.hidden = true;
                d.justify.set_h(super::JustifyH::Right);
                d.anchors = vec![Anchor::new(Point::Right, left_id, Point::Right, 0.0, 0.0)];
                model.region_data.insert(right, d);
                model.touch_measure(right); // an adopted cell can arrive text-in-hand
                model.touch_layout(); // a line row entered the layout graph
            }
            let right_id = model.region_id(right);

            if !frame_name.is_empty() {
                // Adopted pairs are already named and published by the loader.
                if adopted_left.is_none() {
                    let ln = format!("{frame_name}TextLeft{i}");
                    model.name_region(ln.clone(), left_id);
                    publish.push((left_id, ln));
                }
                if adopted_right.is_none() {
                    let rn = format!("{frame_name}TextRight{i}");
                    model.name_region(rn.clone(), right_id);
                    publish.push((right_id, rn));
                }
            }

            let t = tip_mut(&mut model, h)?;
            t.left_lines.push(left);
            t.right_lines.push(right);
            if i == 1 && adopted_left.is_some() {
                t.xml_declared_lines = true;
            }
        }
    }
    for (id, name) in publish {
        let wrapper = region_wrapper(lua, id)?;
        publish_global(lua, &name, &wrapper)?;
    }
    Ok(())
}

/// Writes a line cell's text and colour and shows it; the cell's `SetText` runs the grammar pass.
pub(super) fn write_cell(
    model: &mut Model,
    rh: crate::widget::RegionHandle,
    text: &str,
    color: [f32; 4],
) {
    model.set_region_text(rh, Some(crate::language::process(text).into_owned()));
    let d = model.region_data.entry(rh).or_default();
    d.vertex_color = Some(color);
    // The line's own colour, as if set by `SetTextColor`: a later
    // `GameTooltipText:SetTextColor` must not repaint it.
    d.font_explicit.color = true;
    d.hidden = false;
}

/// Texture slot `i` (0-based), the template's `$parentTexture<i+1>` owned by this tooltip; `None`
/// for an unnamed tooltip or a template without it, where `AddTexture` does nothing (`0x61c8c5`).
fn texture_slot(model: &Model, h: FrameHandle, i: usize) -> Option<crate::widget::RegionHandle> {
    let name = model.arena.frame(h)?.name.as_deref()?;
    let id = *model.region_names.get(&format!("{name}Texture{}", i + 1))?;
    let rh = *model.id_to_region.get(&id)?;
    (model.arena.region(rh)?.owner == h).then_some(rh)
}

/// A line's first anchor's x offset, the indent `AddTexture` writes; touched only on a change.
fn set_line_indent(model: &mut Model, rh: crate::widget::RegionHandle, x: f32) {
    let Some(a) = model
        .region_data
        .get_mut(&rh)
        .and_then(|d| d.anchors.first_mut())
    else {
        return;
    };
    if a.x_off.to_bits() != x.to_bits() {
        a.x_off = x;
        model.touch_layout_region(rh);
    }
}

/// A shown slot's width plus the text gap, the room it takes on its line (`0x61ca00`).
fn texture_room(model: &Model, slot: crate::widget::RegionHandle) -> f32 {
    let w = model
        .region_data
        .get(&slot)
        .and_then(|d| d.size)
        .map_or(0.0, |(w, _)| w);
    w + crate::widget::TOOLTIP_TEXTURE_GAP
}

/// `CGTooltip::AddTexture` (`0x61c8b0`): the next free slot goes on the last line, TOPRIGHT to its
/// TOPLEFT less the gap, the line indents by the slot's room, or by none under a slot on the line
/// above, and the line after it is pulled back by the same room. Nothing with no slot left or
/// fewer than 2 lines. `tint` is the slot's vertex colour, white from Lua.
pub(super) fn add_texture(
    lua: &Lua,
    this: &Table,
    texture: Option<String>,
    coords: Option<super::TexCoords>,
    tint: [f32; 4],
) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let (index, lines) = with_tip(lua, this, |t| (t.texture_lines.len(), t.num_lines))?;
    if index >= crate::widget::TOOLTIP_TEXTURE_SLOTS || lines < 2 {
        return Ok(());
    }
    let Some(slot) = texture_slot(&crate::script::model_mut_of(lua), h, index) else {
        return Ok(());
    };
    // The reference's pool holds the next line already; this one grows on demand.
    ensure_lines(lua, this, lines + 1)?;
    let mut model = crate::script::model_mut_of(lua);
    let line = lines - 1;
    let (left, next, shares_indent) = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Tooltip(t)) => (
            t.left_lines[line],
            t.left_lines.get(line + 1).copied(),
            t.texture_lines.last() == Some(&(line - 1)),
        ),
        _ => return Err(mlua::Error::runtime("not a GameTooltip")),
    };
    let room = texture_room(&model, slot);
    set_line_indent(&mut model, left, if shares_indent { 0.0 } else { room });
    if let Some(next) = next {
        set_line_indent(&mut model, next, -room);
    }
    let left_id = model.region_id(left);
    let anchor = Anchor::new(
        Point::TopRight,
        left_id,
        Point::TopLeft,
        -crate::widget::TOOLTIP_TEXTURE_GAP,
        0.0,
    );
    let d = model.region_data.entry(slot).or_default();
    let old: Vec<u32> = d.anchors.iter().map(|a| a.relative_to).collect();
    d.anchors = vec![anchor];
    d.texture = texture;
    d.fill = None;
    d.tex_coords = coords;
    d.vertex_color = Some(tint);
    let shown = std::mem::replace(&mut d.hidden, false);
    if shown {
        model.arena.note_visibility_change();
    }
    model.touch_layout_retarget_region(slot, &old, &[left_id]);
    tip_mut(&mut model, h)?.texture_lines.push(line);
    Ok(())
}

/// Hides and blanks every line cell and resets the counters; the caller fires `OnTooltipCleared`
/// outside the model borrow.
pub(super) fn clear_content(model: &mut Model, h: FrameHandle) {
    let (lefts, rights, texture_lines) = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Tooltip(t)) => (
            t.left_lines.clone(),
            t.right_lines.clone(),
            t.texture_lines.clone(),
        ),
        _ => return,
    };
    // Each texture's line and the one after it go back to the column and the slot hides
    // (`0x61c6f0`). The reference re-seats the line after at a 0 gap (`0x61c772`), so it rides 2
    // units high from then on; that quirk is not kept.
    for (i, &line) in texture_lines.iter().enumerate() {
        for rh in lefts.iter().skip(line).take(2) {
            set_line_indent(model, *rh, 0.0);
        }
        if let Some(slot) = texture_slot(model, h, i) {
            let d = model.region_data.entry(slot).or_default();
            if !std::mem::replace(&mut d.hidden, true) {
                model.arena.note_visibility_change();
            }
        }
    }
    for rh in lefts.into_iter().chain(rights) {
        let Some(d) = model.region_data.get_mut(&rh) else {
            continue;
        };
        d.hidden = true;
        // `measured` and the wrap pin (`size`) stay: the measure cache is keyed by content
        // ([`RegionData::measure_key`]) and `append_line` rewrites the pin, so the hover loop's
        // clear and rebuild (`GameTooltip_OnUpdate` → `UpdateTooltip`) re-measures nothing. The
        // blank still names the row: a resolve before the rebuild (`OnTooltipCleared`'s money
        // reset reads widths) lays it at the one-unit floor, and the refill must lay it back.
        model.set_region_text(rh, None);
    }
    if let Ok(t) = tip_mut(model, h) {
        t.num_lines = 0;
        t.texture_lines.clear();
        t.min_width = 0.0;
        t.min_width_forced = false;
        // Clear drops the subject and unregisters its health watcher (`0x5300ad`, `0x53007c`).
        t.unit_guid = None;
        t.item_id = 0;
        t.world_owned = false;
    }
    // New content replaces a spell or item render still waiting on its record.
    model.spell_tooltip_waits.remove(&h);
    model.item_tooltip_waits.remove(&h);
    model.achievement.tooltips.remove(&h);
    // The `<name>StatusBar` health bar is unit content: it hides with the lines, and the next
    // unit render re-shows it.
    let bar = model
        .arena
        .frame(h)
        .and_then(|f| f.name.clone())
        .and_then(|n| model.arena.lookup(&format!("{n}StatusBar")));
    if let Some(bar) = bar {
        model.arena.set_shown(bar, false);
    }
}

/// Fires `OnTooltipCleared`; a script error is recorded, never propagated.
pub(super) fn fire_cleared(lua: &Lua, h: FrameHandle) {
    let id = {
        let mut model = crate::script::model_mut_of(lua);
        model.frame_id(h)
    };
    if let Err(e) = event::fire_widget_handler(lua, id, "OnTooltipCleared", Vec::new()) {
        crate::script::model_mut_of(lua).record_script_error(e.to_string());
    }
}

/// A content `Set*`'s tail: `Show` with lines, hidden without.
///
/// Deviation: an empty render hides without the `HideThis` override (`0x61eb20`), keeping the
/// owner, where the reference's `Hide` clears it. benilla answers some content lazily, so a
/// render the reference fills at once can come up empty and replay when answered, under the same
/// owner, which the hover loop's `IsOwned` also reads.
pub(super) fn show_or_hide_empty(lua: &Lua, h: FrameHandle) {
    let lines = {
        let mut model = crate::script::model_mut_of(lua);
        match tip_mut(&mut model, h) {
            Ok(t) => t.num_lines,
            Err(_) => 0,
        }
    };
    if lines > 0 {
        set_shown(lua, h, true);
    } else {
        event::hide_without_override(lua, h);
    }
}

/// New content cancels a running fade at full alpha; with no fade running, an alpha the addon set
/// stays, since only the paths in [`full_alpha`] reset it.
fn cancel_fade(model: &mut Model, h: FrameHandle) {
    if let Ok(t) = tip_mut(model, h) {
        if t.fade_start.take().is_some() {
            model.arena.set_alpha(h, 1.0);
        }
    }
}

/// `SetAlpha(255)`, unconditional: the SetOwner core `0x52ffe0` does it first (`0x52fff4`), and
/// Show (`0x530a80`) and its self-hide (`0x530a60`, the core with no owner) reach it too.
fn full_alpha(model: &mut Model, h: FrameHandle) {
    if let Ok(t) = tip_mut(model, h) {
        t.fade_start = None;
    }
    model.arena.set_alpha(h, 1.0);
}

/// The engine's `GetTime` clock, which [`super::UiScript::tick`] advances.
fn now(lua: &Lua) -> f64 {
    crate::script::clock::now(lua)
}

/// Shows or hides through the live visibility cascade ([`event::set_shown`]).
pub(super) fn set_shown(lua: &Lua, h: FrameHandle, shown: bool) {
    event::set_shown(lua, h, shown);
}

/// The default line colour, gold `0xffffd200` (stored at `0xc0d3e8` by `0x528e50`). The zone
/// tooltip's `AddLine(text, "", 1.0, 1.0, 1.0)` (`Minimap.lua:40`) renders in it: the `""` r-slot
/// drops the whole colour, and the last `1.0` lands in the wrap slot.
pub(in crate::script) const DEFAULT_TEXT_GOLD: [f32; 4] = [1.0, 210.0 / 255.0, 0.0, 1.0];

/// The client's duration formatter (`0x52fa50`): days, hours, minutes or seconds by threshold,
/// each dividing by its own unit. `round_up` ceils every arm but seconds, which truncates, so 61 s
/// reads "2 minutes" and 3 599 999 ms "60 minutes". The text is the player's own
/// `GlobalStrings.lua` through `get`; a missing key renders no line.
pub(in crate::script) fn duration_text(
    ms: u32,
    key_prefix: &str,
    round_up: bool,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    const DAY_MS: u32 = 86_400_000;
    const HOUR_MS: u32 = 3_600_000;
    const MIN_MS: u32 = 60_000;
    const SEC_MS: u32 = 1_000;

    let (suffix, divisor) = if ms >= DAY_MS {
        ("DAYS", DAY_MS)
    } else if ms >= HOUR_MS {
        ("HOURS", HOUR_MS)
    } else if ms >= MIN_MS {
        ("MIN", MIN_MS)
    } else {
        ("SEC", SEC_MS)
    };
    let n = if round_up && divisor != SEC_MS {
        ms.div_ceil(divisor)
    } else {
        ms / divisor
    };
    let template = plural_template(&format!("{key_prefix}_{suffix}"), n, get)?;
    Some(template.replacen("%d", &n.to_string(), 1))
}

/// [`crate::strings::plural`], the `_P1` pick of `GetText(token, gender, n)`. There is no gender:
/// the formatter passes a literal 0 (`0x52fa50`), which `0x703bf0` reads as nil. With neither key
/// present the lookup yields "" (`0x882748`), which the line core drops, so no line either way.
pub(in crate::script) fn plural_template(
    token: &str,
    n: u32,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    crate::strings::plural(token, Some(n), get)
}

/// A colour component by `lua_tonumber`'s coercion: a numeric string counts, other strings do not.
fn color_num(v: Option<&Value>) -> Option<f32> {
    match v {
        Some(Value::Number(n)) => Some(*n as f32),
        Some(Value::Integer(i)) => Some(*i as f32),
        Some(Value::String(s)) => s.to_str().ok().and_then(|s| s.trim().parse::<f32>().ok()),
        _ => None,
    }
}

/// A wrap flag: nil, false, 0 and the strings "0", "off" and "disabled" are false, anything else
/// true. The reference reads it with `GetBoolOrDefault` (`0x6f1c10`,
/// `binding_abi::bool_or_default`), where "false", "no" and 0.5 are false too.
fn bool_arg(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Nil) | Some(Value::Boolean(false)) => false,
        Some(Value::Integer(i)) => *i != 0,
        Some(Value::Number(n)) => *n != 0.0,
        Some(Value::String(s)) => !s.to_str().ok().is_some_and(|s| {
            s == "0" || s.eq_ignore_ascii_case("off") || s.eq_ignore_ascii_case("disabled")
        }),
        _ => true,
    }
}

/// An `AddLine`-family colour, gated on the r-slot alone (`lua_isnumber`, `0x6f34d0`), else
/// [`DEFAULT_TEXT_GOLD`]; past the gate a non-number g or b reads 0, and alpha is always opaque.
fn parse_line_color(r: Option<&Value>, g: Option<&Value>, b: Option<&Value>) -> [f32; 4] {
    match color_num(r) {
        Some(r) => [
            r,
            color_num(g).unwrap_or(0.0),
            color_num(b).unwrap_or(0.0),
            1.0,
        ],
        None => DEFAULT_TEXT_GOLD,
    }
}

/// An `AddLine` tail, positional `r, g, b, wrap` (`0x531630`); the wrap flag is read whatever the
/// colour gate decides.
fn parse_line_tail(args: &[Value]) -> ([f32; 4], bool) {
    (
        parse_line_color(args.first(), args.get(1), args.get(2)),
        bool_arg(args.get(3)),
    )
}

/// A text argument as a line's string: numbers stringify as Lua does, and nil is empty.
fn text_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.to_str().map(|s| s.to_string()).unwrap_or_default(),
        Some(Value::Number(n)) => {
            let n = *n;
            if n == n.trunc() && n.abs() < 1e15 {
                format!("{}", n as i64)
            } else {
                format!("{n}")
            }
        }
        Some(Value::Integer(i)) => format!("{i}"),
        _ => String::new(),
    }
}

/// Appends one line and cancels any fade, without showing the tooltip: `AddLine` callers call
/// `Show`. The reference's line core drops a line whose sides are both empty (`0x530270`); this
/// appends it.
pub(super) fn append_line(
    lua: &Lua,
    this: &Table,
    left: (String, [f32; 4]),
    right: Option<(String, [f32; 4])>,
    wrap: bool,
) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let n = with_tip(lua, this, |t| t.num_lines + 1)?;
    ensure_lines(lua, this, n)?;
    let mut model = crate::script::model_mut_of(lua);
    let (lh, rh) = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Tooltip(t)) => (t.left_lines[n - 1], t.right_lines[n - 1]),
        _ => return Err(mlua::Error::runtime("not a GameTooltip")),
    };
    write_cell(&mut model, lh, &left.0, left.1);
    // The wrap width is pinned at append, so the first measure already comes back wrapped. It is
    // written from this line's own flag every time, and layout is touched only on a change.
    let pin = wrap.then_some((crate::widget::TOOLTIP_WRAP_WIDTH, 0.0));
    if let Some(d) = model.region_data.get_mut(&lh) {
        if d.size != pin {
            d.size = pin;
            // A size-only write takes the named touch: a conservative one would re-derive the
            // whole layout graph on every hover that turns a line between wrapped and plain.
            model.touch_layout_region(lh);
            // The wrap pin is the measure key's wrap-width input.
            model.touch_measure(lh);
        }
    }
    if let Some((text, color)) = right {
        write_cell(&mut model, rh, &text, color);
    }
    if let Ok(t) = tip_mut(&mut model, h) {
        t.num_lines = n;
    }
    cancel_fade(&mut model, h);
    Ok(())
}

mod verbs;
pub(super) use verbs::install;

/// `Hide` (`0x48f620`), whose `HideThis` is the override below. `SetOwner`'s first step, a `Show`
/// with no owner or no lines and the end of a fade all take it.
pub(super) fn hide_tooltip(lua: &Lua, h: FrameHandle) {
    set_shown(lua, h, false);
}

/// `CGTooltip::HideThis` (`0x61eb20`), before the base step on every hide that reaches the
/// tooltip, its own or an ancestor's: the `SetOwner` core with no owner.
pub(in crate::script) fn hide_this_override(lua: &Lua, h: FrameHandle) {
    set_owner_core(lua, h, None, TooltipAnchor::Left, (0.0, 0.0));
}

/// `CGTooltip::ShowThis` (`0x61cff0`), before the base step on every show that reaches the
/// tooltip: with no owner or no lines it takes `Hide` instead and answers false; otherwise full
/// alpha with the fade disarmed, and true.
pub(in crate::script) fn show_this_override(lua: &Lua, h: FrameHandle) -> bool {
    let live = {
        let mut model = crate::script::model_mut_of(lua);
        tip_mut(&mut model, h)
            .map(|t| t.owner.is_some() && t.num_lines > 0)
            .unwrap_or(false)
    };
    if !live {
        hide_tooltip(lua, h);
        return false;
    }
    full_alpha(&mut crate::script::model_mut_of(lua), h);
    true
}

/// `SetOwner`'s skip gate (`0x61ea47`): an offset this close to the stored one is unchanged.
const OFFSET_EPSILON: f32 = 2.0 * f32::EPSILON;

/// The `SetOwner` core (`0x61ea00`): the clear and its OnTooltipCleared (`0x61ea06`), full alpha
/// with the fade disarmed (`0x61ea12`), then, unless owner, mode and offsets all match the stored
/// ones (`0x61ea20`), the store (no owner stores mode 9, `0x61ea8a`) and the placement.
pub(super) fn set_owner_core(
    lua: &Lua,
    h: FrameHandle,
    owner: Option<FrameHandle>,
    mode: TooltipAnchor,
    offset: (f32, f32),
) {
    {
        let mut model = crate::script::model_mut_of(lua);
        clear_content(&mut model, h);
    }
    fire_cleared(lua, h);
    let mut model = crate::script::model_mut_of(lua);
    full_alpha(&mut model, h);
    let Ok(t) = tip_mut(&mut model, h) else {
        return;
    };
    if t.owner == owner
        && t.anchor == mode
        && (offset.0 - t.offset.0).abs() < OFFSET_EPSILON
        && (offset.1 - t.offset.1).abs() < OFFSET_EPSILON
    {
        return;
    }
    t.offset = offset;
    t.owner = owner;
    t.anchor = if owner.is_some() {
        mode
    } else {
        TooltipAnchor::None
    };
    reset_position(&mut model, h);
}

/// `ResetPosition` (`0x61b040`, always called with 1): nothing without an owner or in PRESERVE;
/// otherwise the points clear (`0x61b070`), and modes 0..7 set one on the owner at the offsets.
/// The clear lets `GameTooltip_SetDefaultAnchor` (GameTooltip.lua:76-77) set one point with no
/// stale owner anchor. An unchanged point is left alone, so the bag hover's per-frame `SetOwner`
/// does not dirty the layout; a change is a named retarget.
pub(super) fn reset_position(model: &mut Model, h: FrameHandle) {
    let (owner, mode, (x, y)) = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Tooltip(t)) => (t.owner, t.anchor, t.offset),
        _ => return,
    };
    let Some(owner) = owner else {
        return;
    };
    if mode == TooltipAnchor::Preserve {
        return;
    }
    let Some((own, rel)) = mode.owner_points() else {
        let dropped = match model.layout_inputs.get_mut(&h) {
            Some(input) if !input.anchors.is_empty() => Some(
                input
                    .anchors
                    .drain(..)
                    .map(|a| a.relative_to)
                    .collect::<Vec<u32>>(),
            ),
            _ => None,
        };
        if let Some(old_targets) = dropped {
            model.touch_layout_retarget_frame(h, &old_targets, &[]);
        }
        return;
    };
    let owner_id = model.frame_id(owner);
    let new = Anchor::new(own, owner_id, rel, x, y);
    let input = model.layout_inputs.entry(h).or_default();
    let same =
        input.anchors.len() == 1 && crate::script::object::anchor_bits_eq(&input.anchors[0], &new);
    if !same {
        let old_targets: Vec<u32> = input.anchors.iter().map(|a| a.relative_to).collect();
        input.anchors = vec![new];
        model.touch_layout_retarget_frame(h, &old_targets, &[owner_id]);
    }
}

/// The name a binding's error gives the tooltip: `GetName`, or `<unnamed>` (`0x9ea280`).
pub(super) fn display_name(lua: &Lua, h: FrameHandle) -> String {
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    model
        .arena
        .frame(h)
        .and_then(|f| f.name.clone())
        .unwrap_or_else(|| "<unnamed>".to_string())
}

/// A line cell's measured extent; `None` for a hidden or unmeasured cell, which adds no size and
/// no gap, so a fresh tooltip keeps its declared size until its lines are measured.
type Cell = Option<(f32, f32)>;

fn cell(model: &Model, rh: crate::widget::RegionHandle) -> Cell {
    let d = model.region_data.get(&rh)?;
    let text = d.text.as_deref()?;
    if d.hidden {
        return None;
    }
    if text.is_empty() {
        // One unit each way, the floor of the line's own rect: the reference's `GetHeight`
        // (`0x772a60`) clamps to one unit, so no FontString reads back 0.
        return Some((
            super::layout::FONTSTRING_MIN_SPAN,
            super::layout::FONTSTRING_MIN_SPAN,
        ));
    }
    d.measured.map(|m| (m.w, m.h))
}

/// The cursor modes, re-anchored every frame by `OnLayerUpdate` (`0x61b2e0`): one `SetPoint` on
/// the root's BOTTOMLEFT (`[0xb499a8]`) at the cursor over the tooltip's own scale. CURSOR (8,
/// `0x61b2fd`) pins the plate's BOTTOM there, centred above the cursor, offsets unused;
/// CURSOR_RIGHT (11, `0x61b363`) pins its BOTTOMLEFT there plus the offsets. There is no clear,
/// and `SetPoint`'s no-op gate keeps a still cursor free.
fn cursor_anchor(model: &mut Model, h: FrameHandle) {
    let (mode, (ox, oy)) = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Tooltip(t)) => (t.anchor, t.offset),
        _ => return,
    };
    let scale = crate::script::object::eff_scale(model, h);
    let (cx, cy) = model.cursor_pos;
    let new = match mode {
        TooltipAnchor::CursorRight => Anchor::new(
            Point::BottomLeft,
            super::SCREEN,
            Point::BottomLeft,
            cx / scale + ox,
            cy / scale + oy,
        ),
        _ => Anchor::new(
            Point::Bottom,
            super::SCREEN,
            Point::BottomLeft,
            cx / scale,
            cy / scale,
        ),
    };
    let input = model.layout_inputs.entry(h).or_default();
    // `0x767c70`'s change detect: the one slot is replaced only on a real change.
    let same =
        input.anchors.len() == 1 && crate::script::object::anchor_bits_eq(&input.anchors[0], &new);
    if same {
        return;
    }
    let old_targets: Vec<u32> = input.anchors.iter().map(|a| a.relative_to).collect();
    input.anchors = vec![new];
    model.touch_layout_retarget_frame(h, &old_targets, &[super::SCREEN]);
}

/// The auto-size (`CGTooltip::CalculateSize` `0x61caf0`), run before every resolve's gate: the
/// reference sizes at each Show, line add and item build, so the floor `SetTooltipMoney` writes
/// after a mid-call `GetWidth` still lands. Its inputs (lines, floor, padding) name no node, so
/// it touches only what it moves, and an unchanged tooltip leaves the gate shut.
pub(super) fn layout_tooltips(model: &mut Model) {
    // Only tooltips in the resolve's roster: one outside it would mint an id here and take the
    // layout ledger's conservative branch.
    for i in 0..model.arena.tooltip_kinds().len() {
        let h = model.arena.tooltip_kinds()[i];
        if !model.frame_to_id.contains_key(&h) {
            continue;
        }
        // The cursor modes run before the line gate: the reference re-anchors any shown plate,
        // lines or not.
        let cursor_mode = match model.arena.frame(h).map(|f| (&f.kind_state, f.shown)) {
            Some((KindState::Tooltip(t), shown)) => {
                shown && matches!(t.anchor, TooltipAnchor::Cursor | TooltipAnchor::CursorRight)
            }
            _ => false,
        };
        if cursor_mode {
            cursor_anchor(model, h);
        }
        // The line gate first: this runs at every resolve, and an empty tooltip clones nothing.
        let (num, lefts, rights, min_w, pad_w, texture_lines) =
            match model.arena.frame(h).map(|f| &f.kind_state) {
                Some(KindState::Tooltip(t)) if t.num_lines > 0 => (
                    t.num_lines,
                    t.left_lines.clone(),
                    t.right_lines.clone(),
                    t.min_width,
                    t.padding,
                    t.texture_lines.clone(),
                ),
                _ => continue,
            };
        let n = num.min(lefts.len()).min(rights.len());
        // An `AddTexture` row is wider by its slot's room (`0x61cba0`).
        let mut rooms = vec![0.0f32; n];
        for (i, &line) in texture_lines.iter().enumerate() {
            if let (Some(room), Some(slot)) = (rooms.get_mut(line), texture_slot(model, h, i)) {
                *room = texture_room(model, slot);
            }
        }
        let rows: Vec<(Cell, Cell)> = (0..n)
            .map(|i| (cell(model, lefts[i]), cell(model, rights[i])))
            .collect();
        let mut maxw: f32 = 0.0;
        let mut totalh: f32 = 0.0;
        let mut counted = 0usize;
        for ((l, r), room) in rows.iter().zip(&rooms) {
            if l.is_none() && r.is_none() {
                continue;
            }
            let (lw, lh) = l.unwrap_or((0.0, 0.0));
            let (rw, rh_h) = r.map_or((0.0, 0.0), |(w, hgt)| (TOOLTIP_DOUBLE_GAP + w, hgt));
            if counted > 0 {
                totalh += TOOLTIP_LINE_GAP;
            }
            totalh += lh.max(rh_h);
            counted += 1;
            maxw = maxw.max(lw + rw + room);
        }
        if counted == 0 {
            continue; // nothing measured yet: the declared size holds
        }
        maxw = maxw.max(min_w);
        if maxw <= 0.0 || totalh <= 0.0 {
            continue;
        }
        // `pad_w` is `SetPadding`'s extra width (ItemRefTooltip's room for its close button).
        let width = maxw + 2.0 * TOOLTIP_PAD + pad_w;
        let height = totalh + 2.0 * TOOLTIP_PAD;
        let input = model.layout_inputs.entry(h).or_default();
        if input.width.to_bits() != width.to_bits() || input.height.to_bits() != height.to_bits() {
            input.width = width;
            input.height = height;
            model.touch_layout_frame(h);
        }
        // Right-flush each double line: left.right + (maxw - left.width) is the text inset, less
        // an indented row's room (`0x61cc3c`).
        for (i, (l, r)) in rows.iter().enumerate() {
            if r.is_none() {
                continue;
            }
            let x_off = maxw - l.map_or(0.0, |(w, _)| w) - rooms[i];
            let mut moved = false;
            if let Some(a) = model
                .region_data
                .get_mut(&rights[i])
                .and_then(|d| d.anchors.first_mut())
            {
                if a.x_off.to_bits() != x_off.to_bits() {
                    a.x_off = x_off;
                    moved = true;
                }
            }
            if moved {
                model.touch_layout_region(rights[i]);
            }
        }
    }
}

/// The fade step of `CGTooltip::OnLayerUpdate` (`0x61b2e0`), in the tooltip's walk slot: alpha
/// ramps from 1 to 0 over [`TOOLTIP_FADE_SECS`], then the full hide.
pub(super) fn update(lua: &Lua, h: FrameHandle) {
    let t = now(lua);
    let step = {
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model app_data");
        // Roster-gated as in `layout_tooltips`, since `hide_tooltip` below writes layout.
        if !model.frame_to_id.contains_key(&h) {
            return;
        }
        let Some(KindState::Tooltip(tip)) = model.arena.frame(h).map(|f| &f.kind_state) else {
            return;
        };
        let Some(start) = tip.fade_start else {
            return;
        };
        1.0 - ((t - start) / TOOLTIP_FADE_SECS) as f32
    };
    if step <= 0.0 {
        hide_tooltip(lua, h);
    } else {
        crate::script::model_mut_of(lua).arena.set_alpha(h, step);
    }
}
