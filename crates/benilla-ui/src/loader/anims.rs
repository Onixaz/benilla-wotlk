use mlua::{Function, ObjectLike, Table};

use crate::framexml::{self, Element};
use crate::script::anim::{self, lua as anim_lua, Kind, OriginPoint};

use super::{children_named, Loader};

impl Loader<'_> {
    /// `<Animations>` (`0x4883f0`), right after the region's `parentKey`: each `<AnimationGroup>`
    /// built, loaded and posted at once (`0x816000`), so its `OnLoad` runs inside the owner's load.
    /// `$parent` in a group's name is the owner's name, and in an animation's the group's (named
    /// over the script parent).
    pub(super) fn apply_animations(
        &mut self,
        el: &Element,
        owner: &Table,
        owner_name: &str,
        dbg: &str,
    ) {
        for animations in children_named(el, "Animations") {
            for node in &animations.children {
                if !node.tag.eq_ignore_ascii_case("AnimationGroup") {
                    self.report.warnings.push(format!(
                        "{dbg}: Unknown child node in Animations element: {}",
                        node.tag
                    ));
                    continue;
                }
                if let Err(e) = self.load_group(node, owner, owner_name, dbg) {
                    self.report
                        .errors
                        .push(format!("{dbg}: <AnimationGroup>: {e}"));
                }
            }
        }
    }

    fn load_group(
        &mut self,
        node: &Element,
        owner: &Table,
        owner_name: &str,
        dbg: &str,
    ) -> mlua::Result<()> {
        let lua = self.lua;
        let name = node
            .name()
            .map(|raw| framexml::resolve_name(raw, owner_name));
        let who = anim_lua::owner_of(lua, owner)?;
        let group = anim_lua::create_group(lua, who, name.clone())?;
        let gid = crate::script::object::decode_id(&group)?;
        if node.attr("inherits").is_some_and(|i| !i.is_empty()) {
            self.report.warnings.push(format!(
                "{dbg}: AnimationGroup inherits= is not built; ignored"
            ));
        }
        self.bind_parent_key(node, owner, &group);
        if let Some(l) = node.attr("looping").filter(|l| !l.is_empty()) {
            if let Err(e) = group.call_method::<()>("SetLooping", l) {
                self.report
                    .warnings
                    .push(format!("{dbg}: Invalid looping value: {l} ({e})"));
            }
        }
        let num = |k: &str| {
            node.attr(k)
                .and_then(|v| v.trim().parse::<f64>().ok())
                .unwrap_or(0.0)
        };
        let (x, y) = (num("initialOffsetX"), num("initialOffsetY"));
        group.call_method::<()>("SetInitialOffset", (x, y))?;
        let base = name.clone().unwrap_or_else(|| owner_name.to_string());
        let mut anims = Vec::new();
        for child in &node.children {
            let tag = child.tag.as_str();
            if tag.eq_ignore_ascii_case("Scripts") {
                self.load_anim_scripts(child, gid, true, &base, dbg);
                continue;
            }
            let kind = match tag.to_ascii_lowercase().as_str() {
                "animation" | "alpha" | "translation" | "rotation" | "scale" | "path" => {
                    Kind::from_name(tag)
                }
                _ => continue,
            };
            let aname = child.name().map(|raw| framexml::resolve_name(raw, &base));
            let (aid, anim) = anim_lua::xml_anim(lua, gid, kind, aname.clone())?;
            self.bind_parent_key(child, &group, &anim);
            self.load_anim(child, aid, &anim, aname.as_deref().unwrap_or(&base), dbg);
            anims.push(aid);
        }
        // The group's post-load (`0x49c2a0`): each animation's `OnLoad`, the sort, the group's
        // `OnLoad`.
        for aid in anims {
            anim_lua::fire(lua, aid, "OnLoad", Vec::new());
        }
        anim_lua::sort(lua, gid);
        anim_lua::fire(lua, gid, "OnLoad", Vec::new());
        Ok(())
    }

    /// An animation's load (`0x49b810`) and its kinds' (`0x49c170` Alpha, `0x49bb00` Translation).
    fn load_anim(&mut self, node: &Element, aid: u32, anim: &Table, base: &str, dbg: &str) {
        let num = |k: &str| node.attr(k).and_then(|v| v.trim().parse::<f64>().ok());
        let lua = self.lua;
        let mut warnings = Vec::new();
        {
            let mut model = crate::script::model_mut_of(lua);
            let Some(a) = model.anims.anims.get_mut(&aid) else {
                return;
            };
            if let Some(v) = num("startDelay") {
                a.start_delay = (v as f32).max(0.0);
            }
            if let Some(v) = num("endDelay") {
                a.end_delay = (v as f32).max(0.0);
            }
            if let Some(v) = num("duration") {
                a.duration = v as f32;
            }
            if let Some(v) = num("maxFramerate") {
                anim::set_max_framerate(a, v as f32);
            }
            if let Some(s) = node.attr("smoothing").filter(|s| !s.is_empty()) {
                match anim::SMOOTHING.iter().find(|k| k.0.eq_ignore_ascii_case(s)) {
                    Some(&(_, i, o)) => anim::set_smoothing(a, (i, o)),
                    None => warnings.push(format!("{dbg}: Invalid smoothing value: {s}")),
                }
            }
            if let Some(v) = node
                .attr("order")
                .and_then(|v| v.trim().parse::<i32>().ok())
            {
                let order = v - 1;
                if !(0..=99).contains(&order) {
                    warnings.push(format!(
                        "{dbg}: Invalid order value: {v}. Order must be between 1 and 100."
                    ));
                }
                a.order = order.clamp(0, 99);
            }
            a.kind = match a.kind {
                Kind::Alpha(_) => {
                    let v = num("change").unwrap_or(0.0) as f32;
                    if !(-1.0..=1.0).contains(&v) {
                        warnings.push(format!("{dbg}: Invalid change value: {v}"));
                    }
                    Kind::Alpha((v.clamp(-1.0, 1.0) * 255.0) as i16)
                }
                Kind::Translation(..) => {
                    let (x, y) = (
                        num("offsetX").unwrap_or(0.0) as f32,
                        num("offsetY").unwrap_or(0.0) as f32,
                    );
                    if y * y + x * x <= 0.000_000_24 {
                        Kind::Translation(0.0, 0.0)
                    } else {
                        Kind::Translation(x, y)
                    }
                }
                Kind::Rotation(_, o) => {
                    let r = num("radians")
                        .or_else(|| num("degrees").map(f64::to_radians))
                        .unwrap_or(0.0);
                    Kind::Rotation(r as f32, origin(node).unwrap_or(o))
                }
                Kind::Scale(..) => Kind::Scale(
                    num("scaleX").unwrap_or(1.0) as f32,
                    num("scaleY").unwrap_or(1.0) as f32,
                    origin(node).unwrap_or_default(),
                ),
                k => k,
            };
        }
        let _ = anim;
        self.report.warnings.extend(warnings);
        for scripts in children_named(node, "Scripts") {
            self.load_anim_scripts(scripts, aid, false, base, dbg);
        }
    }

    /// An animation object's `<Scripts>` (`0x498370`, `0x497c30`): each handler compiled under its
    /// own wrapper, or read from `function=`.
    fn load_anim_scripts(
        &mut self,
        scripts: &Element,
        id: u32,
        group: bool,
        owner: &str,
        dbg: &str,
    ) {
        for handler in &scripts.children {
            let name = handler.tag.clone();
            let func: Option<Function> =
                if let Some(global) = handler.attr("function").filter(|f| !f.is_empty()) {
                    self.named_handler(global, &name, owner)
                } else if handler.body.is_empty() {
                    None
                } else {
                    let params = anim_lua::script_parameters(&name, group);
                    let source = format!("return function({params}) {} end", handler.body);
                    match self
                        .lua
                        .load(source.as_str())
                        .set_name(format!("*:{name}"))
                        .set_mode(mlua::ChunkMode::Text)
                        .into_function()
                        .and_then(|c| c.call::<Function>(()))
                    {
                        Ok(f) => Some(f),
                        Err(e) => {
                            self.report
                                .errors
                                .push(format!("{dbg}: compiling <{name}>: {e}"));
                            continue;
                        }
                    }
                };
            let target = match anim_lua::wrapper(self.lua, id) {
                Ok(t) => t,
                Err(e) => {
                    self.report.errors.push(format!("{dbg}: {e}"));
                    continue;
                }
            };
            if let Err(e) = target.call_method::<()>("SetScript", (name.clone(), func)) {
                self.report
                    .warnings
                    .push(format!("{dbg}: Unknown script element {name}: {e}"));
            }
        }
    }
}

/// `<Origin point="..."><Offset x y/></Origin>` of a Rotation or Scale.
fn origin(node: &Element) -> Option<OriginPoint> {
    let o = children_named(node, "Origin").next()?;
    let point = o
        .attr("point")
        .and_then(crate::script::object::point_from_str)
        .map_or("CENTER", crate::script::object::point_name);
    let (x, y) = children_named(o, "Offset")
        .next()
        .map(|off| {
            let f = |k: &str| {
                off.attr(k)
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(0.0)
            };
            (f("x"), f("y"))
        })
        .unwrap_or((0.0, 0.0));
    Some(OriginPoint { point, x, y })
}
