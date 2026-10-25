//! 3.3.5a's title verbs over `CharTitles.dbc` and the player's title fields: the character sheet's
//! picker (`PlayerTitleFrame_UpdateTitles`) and `SetCurrentTitle`'s `CMSG_SET_TITLE`, and the
//! chosen-title name `UnitPVPName`, the unit tooltip and the overhead name share
//! (`CGUnit_C::GetUnitChosenTitle` `0x72a290`).

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

/// The row whose mask id is `mask` (`GetRecordForTitle` `0x719900`, a walk in file order).
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

/// `GetUnitChosenTitle`'s title leg (`0x72a290`): a player's chosen title (above 0) with the name
/// in its `%s`, and the `PVP_MEDAL<n>` line under it for a set medal byte (`PLAYER_BYTES_3` byte
/// 2), its text from `medal_text`. `None` for no title, where the caller shows the plain name.
///
/// Deviation: a chosen title no row names answers `None`, where the reference returns its
/// unwritten buffer.
pub fn chosen_title_name(
    rows: &[TitleRow],
    chosen: i32,
    female: bool,
    medal: u8,
    name: &str,
    medal_text: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    if chosen <= 0 {
        return None;
    }
    let row = title_row(rows, i64::from(chosen))?;
    let mut out = super::pvp::format_two_strings(gendered(row, female), name, "");
    if medal != 0 {
        out.push('\n');
        out.push_str(&medal_text(&format!("PVP_MEDAL{medal}")).unwrap_or_default());
    }
    Some(out)
}

impl super::UiScript {
    /// Push `CharTitles.dbc`'s rows in file order.
    pub fn set_title_rows(&mut self, rows: Vec<TitleRow>) {
        set_field!(self, title_rows, rows);
    }

    /// Push the active player's title fields, `None` with no player.
    pub fn set_player_titles(&mut self, titles: Option<PlayerTitles>) {
        set_field!(self, player_titles, titles);
    }

    /// Take `SetCurrentTitle`'s sends, each a `CMSG_SET_TITLE` body, in call order.
    pub fn take_title_sets(&mut self) -> Vec<i32> {
        drain_field!(self, title_sets)
    }
}

fn read(lua: &Lua) -> mlua::AppDataRef<'_, super::ModelCell> {
    lua.app_data_ref::<super::ModelCell>()
        .expect("model app_data")
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
        lua.create_function(|lua, ()| {
            let model = read(lua);
            Ok(model.title_rows.len() as f64 + 1.0)
        })?,
    )?;

    // GetCurrentTitle() (`0x51b3b0`): `PLAYER_CHOSEN_TITLE` signed, 0 with no player.
    g.set(
        "GetCurrentTitle",
        lua.create_function(|lua, ()| {
            let model = read(lua);
            Ok(model.player_titles.as_ref().map_or(0, |t| t.chosen))
        })?,
    )?;

    // IsTitleKnown(mask) (`0x51b410`): 1 for a set known-titles bit, else 0; mask 0 is unknown.
    g.set(
        "IsTitleKnown",
        lua.create_function(|lua, mask: Value| {
            let mask = mask_arg(lua, mask, "Usage: IsTitleKnown(titleMaskID)")? as u32;
            let model = read(lua);
            let known = model.player_titles.as_ref().is_some_and(|t| {
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
            let text = {
                let model = read(lua);
                let female = model.player_titles.as_ref().is_some_and(|t| t.female);
                title_row(&model.title_rows, mask).map(|r| strip_name_slot(gendered(r, female)))
            };
            let mut out = MultiValue::new();
            if let Some(text) = text {
                out.push_back(Value::String(lua.create_string(&text)?));
            }
            Ok(out)
        })?,
    )?;

    // SetCurrentTitle(mask) (`0x522e30`): `CMSG_SET_TITLE` with the mask, nothing with no player,
    // and `ERR_CLIENT_LOCKED_OUT` past the throttle.
    g.set(
        "SetCurrentTitle",
        lua.create_function(|lua, mask: Value| {
            let mask = mask_arg(lua, mask, "Usage: SetCurrentTitle(titleMaskID)")? as i32;
            let now = super::clock::now(lua);
            let mut model = super::model_mut_of(lua);
            if model.player_titles.is_none() {
                return Ok(());
            }
            if model.title_throttle.admit(now) {
                model.title_sets.push(mask);
            } else {
                model.ui_errors.push("ERR_CLIENT_LOCKED_OUT");
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// The picker's reads: the count is rows + 1, a known bit by mask id, the gendered and
    /// stripped name, and nothing for a mask no row has.
    #[test]
    fn the_picker_verbs_read_the_rows_and_the_fields() {
        let mut s = script();
        s.set_title_rows(rows());
        assert_eq!(s.eval::<i64>("return GetCurrentTitle()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return IsTitleKnown(46)").unwrap(), 0);
        let mut known = [0u32; 6];
        known[46 / 32] |= 1 << (46 % 32);
        known[2] |= 1;
        s.set_player_titles(Some(PlayerTitles {
            chosen: 46,
            known,
            female: true,
        }));
        assert_eq!(s.eval::<i64>("return GetNumTitles()").unwrap(), 4);
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
    }

    /// `SetCurrentTitle` queues the mask, "None" as -1; the eleventh send inside the window is
    /// refused with `ERR_CLIENT_LOCKED_OUT`.
    #[test]
    fn set_current_title_queues_and_throttles() {
        let mut s = script();
        s.run("SetCurrentTitle(46)").unwrap();
        assert!(s.take_title_sets().is_empty(), "no player, no send");
        s.set_player_titles(Some(PlayerTitles::default()));
        s.run("SetCurrentTitle(46) SetCurrentTitle(-1)").unwrap();
        assert_eq!(s.take_title_sets(), vec![46, -1]);
        s.run("for i = 1, 10 do SetCurrentTitle(i) end").unwrap();
        assert_eq!(s.take_title_sets().len(), 9);
        assert_eq!(s.take_ui_errors(), vec!["ERR_CLIENT_LOCKED_OUT"]);
    }

    /// The chosen-title name: the format filled, the female form, a medal line, and none at 0.
    #[test]
    fn the_chosen_title_name_fills_the_format() {
        let medal = |k: &str| (k == "PVP_MEDAL2").then(|| "Protector".to_string());
        assert_eq!(
            chosen_title_name(&rows(), 46, true, 0, "Elza", medal).as_deref(),
            Some("Elza the Exploress")
        );
        assert_eq!(
            chosen_title_name(&rows(), 1, false, 2, "Elza", medal).as_deref(),
            Some("Private Elza\nProtector")
        );
        assert_eq!(chosen_title_name(&rows(), 0, false, 2, "Elza", medal), None);
        assert_eq!(chosen_title_name(&rows(), 9, false, 0, "Elza", medal), None);
    }
}
