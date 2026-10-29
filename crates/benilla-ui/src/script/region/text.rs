//! The FontString-only methods: the string, its measured width, justification, non-space wrap and
//! text height. The font block it shares with EditBox comes from [`crate::script::font_block`].

use mlua::{Lua, Table, Value};

use crate::justify;

use super::region_handle_of;

/// Whether FontString `rh` holds a font handle (`[fs+0xe8]`), which the FontString's font set makes
/// only from a face and a non-zero height. A Button's label also counts its button's state fonts
/// and `Button:SetFont`, which extract paints it with.
fn has_font(model: &crate::script::Model, rh: crate::widget::RegionHandle) -> bool {
    let own = model
        .region_data
        .get(&rh)
        .is_some_and(|d| d.font_path.is_some() && d.font_height.is_some_and(|h| h != 0.0));
    own || model
        .arena
        .region(rh)
        .and_then(|r| model.arena.frame(r.owner))
        .is_some_and(|f| match &f.kind_state {
            crate::widget::KindState::Button(bs) if bs.text == Some(rh) => {
                bs.font.is_some()
                    || [&bs.normal_font, &bs.highlight_font, &bs.disabled_font]
                        .into_iter()
                        .flatten()
                        .filter_map(|n| model.font_object(n))
                        .any(|fo| fo.font.is_some())
            }
            _ => false,
        })
}

/// `<verb>(): Font not set`, raised by `SetText` (`0x48d7ae`) and `SetFormattedText` (`0x48d838`)
/// before storing anything when the FontString has no font handle.
fn font_not_set(this: &Table, verb: &str) -> mlua::Error {
    let who = this
        .get::<mlua::Function>("GetName")
        .and_then(|f| f.call::<Option<String>>(this.clone()));
    match who {
        Ok(who) => mlua::Error::runtime(format!(
            "{}:{verb}(): Font not set",
            who.as_deref()
                .unwrap_or(super::super::object::anchor_args::UNNAMED)
        )),
        Err(e) => e,
    }
}

/// [`font_not_set`] unless the FontString has a font, for `SetFormattedText`, which checks before
/// it formats.
fn require_font(lua: &Lua, this: &Table, verb: &str) -> mlua::Result<()> {
    let rh = region_handle_of(lua, this)?;
    let set = {
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model app_data set");
        has_font(&model, rh)
    };
    if set {
        Ok(())
    } else {
        Err(font_not_set(this, verb))
    }
}

/// The XML `text=` store: the loader calls the FontString's text set (`0x483910`) itself, not the
/// binding, so no font check applies.
pub(crate) fn store_text(lua: &Lua, this: &Table, text: Option<String>) -> mlua::Result<()> {
    set_text(lua, this, text, None)
}

/// The string `SetText` (`0x483910`) stores, after its grammar pass ([`crate::language`]), with the
/// binding's font check under `check`'s verb, in the same model read.
fn set_text(
    lua: &Lua,
    this: &Table,
    text: Option<String>,
    check: Option<&str>,
) -> mlua::Result<()> {
    let rh = region_handle_of(lua, this)?;
    let text = text.map(crate::language::process_owned);
    // The string it holds, or empty over empty, returns untouched (a string compare at `0x483969`,
    // the empty arm before it): no string clear, so an armed gradient stays, and no model write.
    let (missing, same) = {
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model app_data set");
        let missing = check.filter(|_| !has_font(&model, rh));
        let held = model.region_data.get(&rh).and_then(|d| d.text.as_deref());
        let same = held.filter(|t| !t.is_empty()) == text.as_deref().filter(|t| !t.is_empty());
        (missing, same)
    };
    if let Some(verb) = missing {
        return Err(font_not_set(this, verb));
    }
    if same {
        return Ok(());
    }
    let mut model = crate::script::model_mut_of(lua);
    // The layout is touched only when the string empties or refills: a new string's extent has
    // not moved until its measure lands.
    model.set_region_text(rh, text);
    // Fresh text draws whole: an armed write-on gradient belongs to the old string.
    model.region_data.entry(rh).or_default().alpha_gradient = None;
    Ok(())
}

/// `FontString:SetFormattedText(fmt, ...)` (`0x48d800`) into the FontString `leaf`: the font check,
/// then the formatted text through `SetText`'s own store (`0x483910`), so never nil.
pub(super) fn install_formatted(lua: &Lua, leaf: &Table) -> mlua::Result<()> {
    leaf.set(
        "SetFormattedText",
        lua.create_function(|lua, (this, args): (Table, mlua::MultiValue)| {
            require_font(lua, &this, "SetFormattedText")?;
            let text = crate::script::formatted_text::format(lua, &args)?;
            set_text(lua, &this, Some(text), None)
        })?,
    )
}

/// Install the FontString methods into `m`.
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.set(
        "SetText",
        lua.create_function(|lua, (this, text): (Table, Option<mlua::Value>)| {
            // `lua_tolstring` (`0x48d7dd`): a string or number, anything else clears the text.
            let text = text
                .as_ref()
                .and_then(|v| crate::script::binding_abi::optional_string(lua, v));
            set_text(lua, &this, text, Some("SetText"))
        })?,
    )?;

    // GetText (`0x48d730`) answers nil for a NULL or "" string (its first-byte test at
    // `0x48d765`). The rule is per binding: `EditBox:GetText` keeps "", and stock
    // `MailFrame.lua:521` compares that one with "".
    m.set(
        "GetText",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let text = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model");
                model
                    .region_data
                    .get(&rh)
                    .and_then(|d| d.text.clone())
                    .filter(|t| !t.is_empty())
            };
            match text {
                Some(t) => Ok(Value::String(lua.create_string(&t)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetStringWidth (`0x48de90`): the natural, unwrapped width of the current text, measured on
    // demand. Never the declared width, which `PanelTemplates_TabResize` sets from this; on an axis
    // sized 0, `GetWidth` (`0x483f30`) reads the same width.
    fn natural_w(lua: &Lua, this: &Table) -> mlua::Result<f32> {
        let rh = region_handle_of(lua, this)?;
        // Measure now when a host font engine is installed, as the reference measures inline, so
        // `SetText` then `GetStringWidth` works in one tick; without one it is 0 until the host
        // answers.
        crate::script::measure::ensure_measured(lua, rh);
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model");
        let Some(d) = model.region_data.get(&rh) else {
            return Ok(0.0);
        };
        let scale = model
            .arena
            .region(rh)
            .and_then(|r| model.arena.frame(r.owner))
            .map(|f| f.effective_scale)
            .unwrap_or(1.0);
        Ok(d.measured
            .filter(|m| m.key == d.measure_key(scale))
            .map(|m| m.natural_w)
            .unwrap_or(0.0))
    }
    m.set(
        "GetStringWidth",
        lua.create_function(|lua, this: Table| natural_w(lua, &this))?,
    )?;

    // SetSpacing(spacing) (`0x48dd00`): a number, else the named Usage; clamped at 0
    // (`0x4870d0`) and kept against the font object, as the setter clears the inherit bit.
    m.set(
        "SetSpacing",
        lua.create_function(|lua, (this, sp): (Table, Value)| {
            let Some(sp) = lua.coerce_number(sp)? else {
                let who: Option<String> =
                    this.get::<mlua::Function>("GetName")?.call(this.clone())?;
                return Err(mlua::Error::runtime(format!(
                    "Usage: {}:SetSpacing(spacing)",
                    who.as_deref()
                        .unwrap_or(super::super::object::anchor_args::UNNAMED)
                )));
            };
            let rh = region_handle_of(lua, &this)?;
            let mut model = crate::script::model_mut_of(lua);
            let d = model.region_data.entry(rh).or_default();
            d.font_spacing = (sp as f32).max(0.0);
            d.font_explicit.spacing = true;
            model.touch_layout_region(rh);
            Ok(())
        })?,
    )?;
    // GetSpacing() (`0x48dca0`): the gap in logical px.
    m.set(
        "GetSpacing",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            Ok(f64::from(
                model.region_data.get(&rh).map_or(0.0, |d| d.font_spacing),
            ))
        })?,
    )?;

    // SetJustifyH (XML `justifyH`): the parse, the raise on a miss and the cross-axis clear are
    // `crate::justify`'s, shared with the `<Font>` object.
    m.set(
        "SetJustifyH",
        lua.create_function(|lua, (this, j): (Table, String)| {
            let rh = region_handle_of(lua, &this)?;
            let parsed = justify::parse_h(&j);
            if parsed == justify::Set::NoMatch {
                return Err(justify::usage_h("FontString"));
            }
            let mut model = crate::script::model_mut_of(lua);
            let d = model.region_data.entry(rh).or_default();
            match parsed {
                justify::Set::To(jh) => d.justify.set_h(jh),
                // A cross-axis token erases the axis: `GetJustifyH()` answers "UNKNOWN" while the
                // glyphs draw centred.
                justify::Set::Clears => d.justify.clear_h(),
                justify::Set::NoMatch => unreachable!("returned above"),
            }
            // Every successful parse overrides the font object, the erasing one too: the reference
            // sets the per-axis mask (`+0x124`), before the branch.
            d.font_explicit.justify_h = true;
            Ok(())
        })?,
    )?;

    // SetJustifyV (XML `justifyV`), as SetJustifyH.
    m.set(
        "SetJustifyV",
        lua.create_function(|lua, (this, j): (Table, String)| {
            let rh = region_handle_of(lua, &this)?;
            let parsed = justify::parse_v(&j);
            if parsed == justify::Set::NoMatch {
                return Err(justify::usage_v("FontString"));
            }
            let mut model = crate::script::model_mut_of(lua);
            let d = model.region_data.entry(rh).or_default();
            match parsed {
                justify::Set::To(jv) => d.justify.set_v(jv),
                // Addons reach this arm with `SetJustifyV("CENTER")`, meaning middle.
                justify::Set::Clears => d.justify.clear_v(),
                justify::Set::NoMatch => unreachable!("returned above"),
            }
            d.font_explicit.justify_v = true;
            Ok(())
        })?,
    )?;

    // GetJustifyH/GetJustifyV: one string; CENTER and MIDDLE when untouched, the ctor's `0x212`
    // read through each axis mask.
    fn justify_of(lua: &Lua, this: &Table) -> mlua::Result<justify::Justify> {
        let rh = region_handle_of(lua, this)?;
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model");
        Ok(model
            .region_data
            .get(&rh)
            .map(|d| d.justify)
            .unwrap_or_default())
    }
    m.set(
        "GetJustifyH",
        lua.create_function(|lua, this: Table| Ok(justify_of(lua, &this)?.name_h()))?,
    )?;
    m.set(
        "GetJustifyV",
        lua.create_function(|lua, this: Table| Ok(justify_of(lua, &this)?.name_v()))?,
    )?;

    // SetNonSpaceWrap/CanNonSpaceWrap, FontString only: the getter answers 1 or nil, and a call
    // with no argument enables.
    m.set(
        "SetNonSpaceWrap",
        lua.create_function(|lua, (this, enable): (Table, Value)| {
            let on = match &enable {
                Value::Nil => true,
                Value::Boolean(b) => *b,
                _ => true,
            };
            let rh = region_handle_of(lua, &this)?;
            let mut model = crate::script::model_mut_of(lua);
            model.region_data.entry(rh).or_default().non_space_wrap = Some(on);
            Ok(())
        })?,
    )?;

    m.set(
        "CanNonSpaceWrap",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let model = crate::script::model_mut_of(lua);
            let on = model
                .region_data
                .get(&rh)
                .and_then(|d| d.non_space_wrap)
                .unwrap_or(true);
            Ok(if on { Some(1i64) } else { None })
        })?,
    )?;

    // SetTextHeight: the scaled-string mode (alone clears the one-to-one bit `0x200`), the size
    // magnified from the raster, uncapped. The font is untouched, so GetFont keeps the face's own
    // height.
    m.set(
        "SetTextHeight",
        lua.create_function(|lua, (this, height): (Table, f32)| {
            let rh = region_handle_of(lua, &this)?;
            let mut model = crate::script::model_mut_of(lua);
            model.region_data.entry(rh).or_default().text_height = Some(height);
            model.touch_measure(rh);
            Ok(())
        })?,
    )?;

    // ── the shared font block ───────────────────────────────────────────────────────────────
    //
    // The font object, font, text colour and shadow getters and setters are type-guard shims over
    // one implementation each, shared with EditBox.
    super::super::font_block::install(lua, m, region_handle_of, "FontString", None)
}
