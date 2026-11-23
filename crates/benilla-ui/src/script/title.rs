//! 3.3.5a's title verbs over `CharTitles.dbc` and the player's title fields: the character sheet's
//! picker (`PlayerTitleFrame_UpdateTitles`) and `SetCurrentTitle`'s `CMSG_SET_TITLE`, and the
//! chosen-title name `UnitPVPName`, the unit tooltip and the overhead name share (`0x72a290`).

use mlua::{Lua, MultiValue, Value};

/// One `CharTitles.dbc` row as the verbs read it, in file order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TitleRow {
    /// The known-titles bit and `PLAYER_CHOSEN_TITLE` value (`+0x10`).
    pub mask_id: u32,
    /// The male form (`+0x8`), a format with one `%s` for the name.
    pub name: String,
    /// The female form (`+0xc`).
    pub name1: String,
}

/// The active player's title fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlayerTitles {
    /// `PLAYER_CHOSEN_TITLE`, a mask id; 0 none, and -1 is what "None" in the picker sends.
    pub chosen: i32,
    /// `PLAYER__FIELD_KNOWN_TITLES`, 192 bits by mask id.
    pub known: [u32; 6],
    /// `PLAYER_BYTES_3` byte 0 is 1 (`0x51b546`).
    pub female: bool,
}

/// `SetCurrentTitle`'s throttle (`0x522e30`): the count starts at 10, so the first call opens a
/// window; more than 10 sends inside 60 s are refused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TitleThrottle {
    count: u32,
    window_start: Option<f64>,
}

impl Default for TitleThrottle {
    fn default() -> Self {
        Self {
            count: 10,
            window_start: None,
        }
    }
}

impl TitleThrottle {
    /// Whether a send at `now` passes.
    fn admit(&mut self, now: f64) -> bool {
        self.count += 1;
        if self.count <= 10 {
            return true;
        }
        if self.window_start.is_none_or(|t| now - t >= 60.0) {
            self.window_start = Some(now);
            self.count = 0;
            return true;
        }
        false
    }
}

/// The row whose mask id is `mask` (`0x719900`, a walk in file order).
pub fn title_row(rows: &[TitleRow], mask: i64) -> Option<&TitleRow> {
    rows.iter().find(|r| i64::from(r.mask_id) == mask)
}

/// A row's form for the gender: female takes `name1` and male `name`, each falling back to the
/// other when empty (`0x51b58f`).
fn gendered(row: &TitleRow, female: bool) -> &str {
    let (first, second) = if female {
        (&row.name1, &row.name)
    } else {
        (&row.name, &row.name1)
    };
    if first.is_empty() {
        second
    } else {
        first
    }
}

/// `GetTitleName`'s strip (`0x51b62b`): a `%` starts a skip that runs to the next space, which is
/// kept, so `"%s the Explorer"` reads `" the Explorer"` and `"%s, Champion"` reads `" Champion"`.
fn strip_name_slot(format: &str) -> String {
    let mut out = String::with_capacity(format.len());
    let mut skipping = false;
    for c in format.chars() {
        if c == '%' {
            skipping = true;
        } else if !skipping {
            out.push(c);
        } else if c == ' ' {
            skipping = false;
            out.push(c);
        }
    }
    out
}

/// The chosen-title name's title leg (`0x72a290`): a player's chosen title (above 0) with the name
/// in its `%s`, and the `PVP_MEDAL<n>` line under it for a set medal byte (`PLAYER_BYTES_3` byte
/// 2), its text from `medal_text`; `row` finds the title's row (`0x719900`). `None` for no title,
/// where the caller shows the plain name.
///
/// Deviation: a chosen title no row names answers `None`, where the reference returns its
/// unwritten buffer.
pub fn chosen_title_name(
    row: impl FnOnce(i64) -> Option<TitleRow>,
    chosen: i32,
    female: bool,
    medal: u8,
    name: &str,
    medal_text: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    if chosen <= 0 {
        return None;
    }
    let row = row(i64::from(chosen))?;
    let mut out = super::pvp::format_two_strings(gendered(&row, female), name, "");
    if medal != 0 {
        out.push('\n');
        out.push_str(&medal_text(&format!("PVP_MEDAL{medal}")).unwrap_or_default());
    }
    Some(out)
}

/// The `CharTitles.dbc` row for `mask` in the view lent now (`0x719900`).
pub(crate) fn live_title_row(mask: i64) -> Option<TitleRow> {
    super::live::read_live(|l| l.char_title(mask)).flatten()
}

/// The active player's title fields in the view lent now, `None` with no active player held.
fn live_player_titles() -> Option<PlayerTitles> {
    super::live::read_live(|l| l.player_titles()).flatten()
}

/// `lua_isnumber`, then `fistp` under a chop control word: a truncating integer.
fn mask_arg(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<i64> {
    match lua.coerce_number(v)? {
        Some(n) => Ok(n.trunc() as i64),
        None => Err(mlua::Error::runtime(usage)),
    }
}

/// Register the title globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumTitles() (`0x5165e0`): the row count plus 1 (`[0xad337c]`); the picker walks mask ids
    // 1 through it.
    g.set(
        "GetNumTitles",
        lua.create_function(|_, ()| {
            let rows = super::live::read_live(|l| l.char_title_count()).unwrap_or(0);
            Ok(rows as f64 + 1.0)
        })?,
    )?;

    // GetCurrentTitle() (`0x51b3b0`): the active player's `PLAYER_CHOSEN_TITLE` signed, 0 with no
    // player held.
    g.set(
        "GetCurrentTitle",
        lua.create_function(|_, ()| Ok(live_player_titles().map_or(0, |t| t.chosen)))?,
    )?;

    // IsTitleKnown(mask) (`0x51b410`): 1 for a set known-titles bit of the active player
    // (`0x51b4a6`), else 0; mask 0 is unknown (`0x51b489`).
    g.set(
        "IsTitleKnown",
        lua.create_function(|lua, mask: Value| {
            let mask = mask_arg(lua, mask, "Usage: IsTitleKnown(titleMaskID)")? as u32;
            let known = live_player_titles().is_some_and(|t| {
                mask != 0
                    && t.known
                        .get((mask / 32) as usize)
                        .is_some_and(|w| w & (1 << (mask % 32)) != 0)
            });
            Ok(if known { 1 } else { 0 })
        })?,
    )?;

    // GetTitleName(mask) (`0x51b4e0`): the row's gendered form with the name slot stripped, or
    // nothing at all for a mask no row has; with no player the form is the male one.
    g.set(
        "GetTitleName",
        lua.create_function(|lua, mask: Value| {
            let mask = mask_arg(lua, mask, "Usage: GetTitleName(titleMaskID)")?;
            let female = live_player_titles().is_some_and(|t| t.female);
            let text = live_title_row(mask).map(|r| strip_name_slot(gendered(&r, female)));
            let mut out = MultiValue::new();
            if let Some(text) = text {
                out.push_back(Value::String(lua.create_string(&text)?));
            }
            Ok(out)
        })?,
    )?;

    // SetCurrentTitle(mask) (`0x522e30`): nothing with no player held (`0x522e90`),
    // `ERR_CLIENT_LOCKED_OUT` past the throttle (`0x522ec3`), else `CMSG_SET_TITLE` with the mask
    // sent in the call (`0x522efb`).
    g.set(
        "SetCurrentTitle",
        lua.create_function(|lua, mask: Value| {
            let mask = mask_arg(lua, mask, "Usage: SetCurrentTitle(titleMaskID)")? as i32;
            if live_player_titles().is_none() {
                return Ok(());
            }
            // Not built yet (#1450): tainted code outside a hardware event gets the blocked feedback and no send (`0x522e96`).
            let now = super::clock::now(lua);
            let admitted = super::model_mut_of(lua).title_throttle.admit(now);
            if !admitted {
                crate::script::ui_errors::show_now(lua, "ERR_CLIENT_LOCKED_OUT");
                return Ok(());
            }
            super::live::write_live(|l| l.send_set_title(mask));
            Ok(())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::live::TestView;
    use crate::script::UiScript;

    fn script() -> UiScript {
        UiScript::new().unwrap()
    }

    fn rows() -> Vec<TitleRow> {
        vec![
            TitleRow {
                mask_id: 1,
                name: "Private %s".into(),
                name1: String::new(),
            },
            TitleRow {
                mask_id: 46,
                name: "%s the Explorer".into(),
                name1: "%s the Exploress".into(),
            },
            TitleRow {
                mask_id: 64,
                name: "%s, Champion of the Naaru".into(),
                name1: String::new(),
            },
        ]
    }

    /// The picker's reads at each call: the count is rows + 1, a known bit by mask id, the
    /// gendered and stripped name, and nothing for a mask no row has.
    #[test]
    fn the_picker_verbs_read_the_rows_and_the_fields_at_the_call() {
        let mut s = script();
        let mut v = TestView {
            char_titles: rows(),
            ..Default::default()
        };
        s.with_live(&mut v, |s| {
            assert_eq!(s.eval::<i64>("return GetNumTitles()").unwrap(), 4);
            assert_eq!(s.eval::<i64>("return GetCurrentTitle()").unwrap(), 0);
            assert_eq!(s.eval::<i64>("return IsTitleKnown(46)").unwrap(), 0);
            assert_eq!(
                s.eval::<String>("return GetTitleName(46)").unwrap(),
                " the Explorer",
                "no player reads the male form"
            );
        });
        let mut known = [0u32; 6];
        known[46 / 32] |= 1 << (46 % 32);
        known[2] |= 1;
        v.player_titles = Some(PlayerTitles {
            chosen: 46,
            known,
            female: true,
        });
        s.with_live(&mut v, |s| {
            assert_eq!(s.eval::<i64>("return GetCurrentTitle()").unwrap(), 46);
            assert_eq!(s.eval::<i64>("return IsTitleKnown(46)").unwrap(), 1);
            assert_eq!(s.eval::<i64>("return IsTitleKnown(64.9)").unwrap(), 1);
            assert_eq!(s.eval::<i64>("return IsTitleKnown(1)").unwrap(), 0);
            assert_eq!(s.eval::<i64>("return IsTitleKnown(0)").unwrap(), 0);
            assert_eq!(
                s.eval::<String>("return GetTitleName(46)").unwrap(),
                " the Exploress"
            );
            assert_eq!(
                s.eval::<String>("return GetTitleName(64)").unwrap(),
                " Champion of the Naaru"
            );
            assert_eq!(
                s.eval::<String>("return GetTitleName(1)").unwrap(),
                "Private "
            );
            assert_eq!(
                s.eval::<i64>("return select('#', GetTitleName(7))")
                    .unwrap(),
                0
            );
            assert!(s.run("GetTitleName()").is_err());
        });
        assert_eq!(
            s.eval::<i64>("return GetNumTitles()").unwrap(),
            1,
            "no view lent reads an unloaded table"
        );
    }

    /// `SetCurrentTitle` sends the mask inside the call, "None" as -1; the eleventh send inside the
    /// window is refused with `ERR_CLIENT_LOCKED_OUT`.
    #[test]
    fn set_current_title_sends_in_the_call_and_throttles() {
        let mut s = script();
        let mut v = TestView::default();
        s.with_live(&mut v, |s| s.run("SetCurrentTitle(46)").unwrap());
        assert!(v.title_sends.is_empty(), "no player, no send");
        v.player_titles = Some(PlayerTitles::default());
        s.with_live(&mut v, |s| {
            s.run("SetCurrentTitle(46) SetCurrentTitle(-1)").unwrap();
            assert_eq!(
                crate::script::live::test_view(|v| v.title_sends.clone()),
                vec![46, -1],
                "both sent before the chunk returned"
            );
            s.run("for i = 1, 10 do SetCurrentTitle(i) end").unwrap();
        });
        assert_eq!(v.title_sends.len(), 11);
        assert_eq!(s.take_shown(), vec!["ERR_CLIENT_LOCKED_OUT"]);
        assert!(s.run("SetCurrentTitle()").is_err());
    }

    /// The chosen-title name: the format filled, the female form, a medal line, and none at 0.
    #[test]
    fn the_chosen_title_name_fills_the_format() {
        let medal = |k: &str| (k == "PVP_MEDAL2").then(|| "Protector".to_string());
        let row = |m: i64| title_row(&rows(), m).cloned();
        assert_eq!(
            chosen_title_name(row, 46, true, 0, "Elza", medal).as_deref(),
            Some("Elza the Exploress")
        );
        assert_eq!(
            chosen_title_name(row, 1, false, 2, "Elza", medal).as_deref(),
            Some("Private Elza\nProtector")
        );
        assert_eq!(chosen_title_name(row, 0, false, 2, "Elza", medal), None);
        assert_eq!(chosen_title_name(row, 9, false, 0, "Elza", medal), None);
    }
}
