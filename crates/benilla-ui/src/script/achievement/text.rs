//! The criteria and statistic text (`0x5b0ea0`), the money strings it formats with (the coin
//! texture string `0x7e7e10`, the short coin text `0x7e7ee0`), and the `|Hachievement:` link
//! (`0x61bff0`).

use mlua::Lua;

use super::{flag, AchievementCriteriaRow, AchievementDb, AchievementRow, CriteriaState, WowDate};
use crate::strings::{fill, global, Arg};

/// What the money and per-day arms read off the VM: the string table, `colorblindMode` and the
/// clock.
pub(super) struct TextCtx<'a> {
    pub(super) lua: &'a Lua,
    pub(super) colorblind: bool,
    pub(super) now: u32,
}

impl TextCtx<'_> {
    /// Copper as gold, silver and copper, largest first and the empty units left out; textured,
    /// or as `1g 2s` symbols in colorblind mode (`0x5b0ea0`).
    fn money(&self, amount: i64) -> String {
        let amount = amount as i32;
        let keys = if self.colorblind {
            [
                "COPPER_AMOUNT_SYMBOL",
                "SILVER_AMOUNT_SYMBOL",
                "GOLD_AMOUNT_SYMBOL",
            ]
        } else {
            [
                "COPPER_AMOUNT_TEXTURE",
                "SILVER_AMOUNT_TEXTURE",
                "GOLD_AMOUNT_TEXTURE",
            ]
        };
        let unit = |i: usize, v: i32| {
            let tmpl = global(self.lua, keys[i]).unwrap_or_default();
            if self.colorblind {
                format!("{v}{tmpl}")
            } else {
                fill(&tmpl, &[Arg::D(v.into()), Arg::D(0), Arg::D(0)])
            }
        };
        if amount == 0 {
            return unit(0, 0);
        }
        let parts = [amount % 100, amount % 10000 / 100, amount / 10000];
        (0..3)
            .rev()
            .filter(|&i| parts[i] != 0)
            .map(|i| unit(i, parts[i]))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Whole days since a criterion's first update, `(now - timer2) / 86400` (`0x5b0ea0`).
    fn days(&self, c: &CriteriaState) -> u32 {
        self.now.wrapping_sub(c.timer2) / 86400
    }
}

/// The output string build's inputs: the walk over the source's criteria and the one row shown.
pub(super) struct Output<'a> {
    pub(super) sum: u64,
    pub(super) highest: u64,
    pub(super) highest_id: i64,
    pub(super) count: u64,
    pub(super) value: u64,
    pub(super) progress: Option<&'a CriteriaState>,
    pub(super) flags: u32,
    pub(super) criteria_flags: u32,
    pub(super) completed: bool,
    pub(super) achievement: &'a AchievementRow,
    pub(super) criterion: Option<&'a AchievementCriteriaRow>,
    pub(super) required: u32,
}

/// The output string build (`0x5b0ea0`): the shown text and the number it shows.
pub(super) fn build_output(ctx: &TextCtx, db: &AchievementDb, o: &Output) -> (String, i64) {
    let dashes = |o: &Output| {
        if o.flags & flag::STATISTIC != 0 {
            "--".to_string()
        } else {
            "0".to_string()
        }
    };
    let mut per_day = 0.0f32;
    let (mut text, result): (String, i64);
    if o.flags & flag::SUM != 0 {
        let mut r = o.sum as i64;
        if let Some(c) = o.progress.filter(|_| o.flags & flag::PER_DAY != 0) {
            let days = ctx.days(c);
            if days != 0 {
                per_day = o.sum as f32 / days as f32;
                r = per_day as i64;
            }
        }
        text = if o.criteria_flags & 0x20 != 0 {
            ctx.money(r)
        } else if per_day != 0.0 {
            format!("{per_day:.2}")
        } else if r != 0 {
            r.to_string()
        } else {
            dashes(o)
        };
        result = r;
    } else if o.flags & flag::MAX != 0 {
        let r = o.highest as i64;
        let best = u32::try_from(o.highest_id)
            .ok()
            .and_then(|id| db.criterion(id));
        text = match best {
            Some(b) if o.flags & flag::STATISTIC != 0 => format!("{r} ({})", b.description),
            Some(_) => r.to_string(),
            None if r != 0 => r.to_string(),
            None => dashes(o),
        };
        result = r;
    } else if o.flags & flag::COUNT != 0 {
        let r = o.count as i64;
        text = if r != 0 { r.to_string() } else { dashes(o) };
        result = r;
    } else if o.value != 0 {
        let mut r = o.value as i64;
        if let Some(c) = o.criterion {
            if o.flags & flag::STATISTIC == 0 && r >= i64::from(c.quantity) {
                r = i64::from(c.quantity);
            }
        }
        if let Some(p) = o.progress.filter(|_| o.flags & flag::PER_DAY != 0) {
            let days = ctx.days(p);
            if days != 0 {
                per_day = r as f32 / days as f32;
                r = per_day as i64;
            }
        }
        let rec = o.progress.and_then(|p| db.criterion(p.id));
        text = if rec.is_some_and(|c| c.flags & 0x10 != 0) {
            let d = WowDate(r as u32);
            format!(
                "{:02}/{:02}/{}",
                d.month() + 1,
                d.month_day() + 1,
                (d.year() + 2000) as i16
            )
        } else if rec.is_some_and(|c| c.flags & 0x20 != 0) {
            ctx.money(r)
        } else if o.flags & flag::STATISTIC == 0 {
            r.to_string()
        } else if per_day != 0.0 {
            format!("{per_day:.2}")
        } else if r != 0 {
            r.to_string()
        } else {
            "--".to_string()
        };
        result = r;
    } else if o.completed && o.achievement.minimum_criteria == 0 {
        let r = o.criterion.map_or(0, |c| i64::from(c.quantity));
        text = if r == 0 {
            dashes(o)
        } else if o.criteria_flags & 0x20 != 0 {
            ctx.money(r)
        } else {
            r.to_string()
        };
        result = r;
    } else {
        text = if o.criteria_flags & 0x21 == 0x21 {
            ctx.money(0)
        } else {
            dashes(o)
        };
        result = 0;
    }
    if let Some(c) = o.criterion.filter(|_| o.criteria_flags & 1 != 0) {
        let total = if o.required != 0 {
            o.required
        } else {
            c.quantity
        };
        let total = if o.criteria_flags & 0x20 != 0 {
            ctx.money(total.into())
        } else {
            total.to_string()
        };
        text = format!("{text} / {total}");
    }
    (text, result)
}

/// The achievement hyperlink build (`0x61bff0`): the yellow `|Hachievement:` link, `""` for an id
/// the table lacks. `date` fills month, day and year (1-based month and day, years since 2000); the
/// four words are the met-criteria bits.
pub fn achievement_hyperlink(
    db: &AchievementDb,
    id: u32,
    guid: u64,
    completed: bool,
    date: WowDate,
    bits: [u32; 4],
) -> String {
    let Some(a) = db.get(id) else {
        return String::new();
    };
    format!(
        "|cffffff00|Hachievement:{id}:{guid:016X}:{}:{}:{}:{}:{}:{}:{}:{}|h[{}]|h|r",
        u8::from(completed),
        date.month() + 1,
        date.month_day() + 1,
        date.year(),
        bits[0],
        bits[1],
        bits[2],
        bits[3],
        a.title
    )
}
