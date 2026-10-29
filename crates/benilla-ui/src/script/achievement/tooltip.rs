//! `GameTooltip:SetHyperlink` on an `achievement:` link (`0x62de04` arm → the achievement fill
//! `0x627220`): title, who earned it and when, the description, and the criteria two to a line,
//! green where the link's bits mark them met.

use mlua::{Lua, Table};

use crate::script::object::frame_handle_of;
use crate::script::tooltip::{append_line, clear_content, fire_cleared, hide_tooltip, set_shown};
use crate::strings::{fill, global, Arg};

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const GOLD: [f32; 4] = [1.0, 210.0 / 255.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
/// The reference's skill colours 1 and 0: a met and an unmet criterion.
const MET: [f32; 4] = [64.0 / 255.0, 192.0 / 255.0, 64.0 / 255.0, 1.0];
const UNMET: [f32; 4] = [128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0];

/// The fields after `achievement:` (`0x62de20`-`0x62df63`): id, owner guid in hex, completed,
/// month, day and year as written, and four criteria words; a missing field is 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AchievementLink {
    pub(crate) id: u32,
    pub(crate) guid: u64,
    pub(crate) completed: bool,
    pub(crate) month: i32,
    pub(crate) day: i32,
    pub(crate) year: i32,
    pub(crate) bits: [u32; 4],
}

/// The reference's string-to-int: an optional sign and leading digits, 0 for none.
fn leading_int(s: &str) -> i64 {
    let s = s.trim_start();
    let (neg, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let n: i64 = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i64, |n, b| {
            n.wrapping_mul(10).wrapping_add(i64::from(b - b'0'))
        });
    if neg {
        -n
    } else {
        n
    }
}

pub(crate) fn parse_achievement_link(link: &str) -> Option<AchievementLink> {
    let at = link.find("achievement:")?;
    let rest = &link[at + "achievement:".len()..];
    let mut fields = rest.split(':');
    let mut l = AchievementLink {
        id: leading_int(fields.next().unwrap_or("")) as u32,
        ..Default::default()
    };
    let hex = fields.next().unwrap_or("");
    let hex: String = hex.chars().take_while(char::is_ascii_hexdigit).collect();
    l.guid = u64::from_str_radix(&hex, 16).unwrap_or(0);
    let mut int = || leading_int(fields.next().unwrap_or(""));
    l.completed = int() != 0;
    l.month = int() as i32;
    l.day = int() as i32;
    l.year = int() as i32;
    for b in &mut l.bits {
        *b = int() as u32;
    }
    Some(l)
}

/// Render an `achievement:` link into `this`; `false` when `link` is no achievement link. A
/// second open of the shown achievement hides it; an owner whose name is not cached leaves the
/// tooltip as it was and asks the host for the name, as the reference waits on its name query.
pub(crate) fn set_achievement_link(lua: &Lua, this: &Table, link: &str) -> mlua::Result<bool> {
    let Some(l) = parse_achievement_link(link) else {
        return Ok(false);
    };
    let h = frame_handle_of(lua, this)?;
    let (db, name, shown_same) = {
        let mut model = crate::script::model_mut_of(lua);
        let Some(db) = model.achievement.db.clone() else {
            return Ok(true);
        };
        let shown = model.arena.frame(h).is_some_and(|f| f.shown);
        let shown_same = shown && model.achievement.tooltips.get(&h) == Some(&l.id);
        let name = model.achievement.names.get(&l.guid).cloned();
        if name.is_none() && !shown_same && db.get(l.id).is_some() {
            model.achievement.name_wants.push(l.guid);
        }
        (db, name, shown_same)
    };
    if shown_same {
        hide_tooltip(lua, h);
        return Ok(true);
    }
    let Some(a) = db.get(l.id) else {
        return Ok(true);
    };
    let Some(name) = name else {
        return Ok(true);
    };
    {
        let mut model = crate::script::model_mut_of(lua);
        clear_content(&mut model, h);
        model.achievement.tooltips.insert(h, l.id);
    }
    fire_cleared(lua, h);
    append_line(lua, this, (a.title.clone(), WHITE), None, false)?;
    append_line(lua, this, (" ".into(), GOLD), None, false)?;
    let status = if l.completed {
        global(lua, "ACHIEVEMENT_TOOLTIP_COMPLETE").map(|t| {
            fill(
                &t,
                &[
                    Arg::S(&name),
                    Arg::D(l.month.into()),
                    Arg::D(l.day.into()),
                    Arg::D(l.year.into()),
                ],
            )
        })
    } else {
        global(lua, "ACHIEVEMENT_TOOLTIP_IN_PROGRESS").map(|t| fill(&t, &[Arg::S(&name)]))
    };
    append_line(lua, this, (status.unwrap_or_default(), GREEN), None, false)?;
    append_line(lua, this, (" ".into(), GOLD), None, false)?;
    append_line(lua, this, (a.description.clone(), WHITE), None, true)?;
    append_line(lua, this, (" ".into(), GOLD), None, false)?;
    let source = db.source(l.id);
    let all_met = l.bits == [u32::MAX; 4];
    if let Some(source) = source.filter(|s| s.minimum_criteria == 0 || !all_met) {
        let mut left: Option<(String, [f32; 4])> = None;
        let visible = db.criteria_list(source.id).filter(|c| c.flags & 2 == 0);
        for (i, c) in visible.enumerate() {
            let met = i < 128 && l.bits[i / 32] & (1 << (i % 32)) != 0;
            let cell = (c.description.clone(), if met { MET } else { UNMET });
            match left.take() {
                Some(first) => append_line(lua, this, first, Some(cell), false)?,
                None => left = Some(cell),
            }
        }
        if let Some(first) = left {
            append_line(lua, this, first, None, false)?;
        }
    }
    let id = crate::script::model_mut_of(lua).frame_id(h);
    if let Err(e) =
        crate::script::event::fire_widget_handler(lua, id, "OnTooltipSetAchievement", Vec::new())
    {
        crate::script::model_mut_of(lua).record_script_error(e.to_string());
    }
    set_shown(lua, h, true);
    Ok(true)
}
