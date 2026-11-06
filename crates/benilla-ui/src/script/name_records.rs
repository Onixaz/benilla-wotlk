//! The VM's read of the player name cache: the answered records `GetPlayerInfoByGUID` and
//! `UnitIsSameServer` look up, without asking the server.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::string_arg;
use super::model::Model;
use super::unit_menu::guid_of;

/// `PLAYER_FLAGS` 0x80000: `GetPlayerInfoByGUID` asks the server on a cache miss (`0x613f90`).
const PLAYER_FLAG_ASKS_NAMES: u32 = 0x80000;

/// One name cache record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameRecord {
    pub name: String,
    /// Empty for a player of this realm.
    pub realm: String,
    /// `(display, file)`, `None` with no answered class.
    pub class: Option<(&'static str, &'static str)>,
    /// `(display, file)`, `None` with no answered race.
    pub race: Option<(&'static str, &'static str)>,
    /// The wire's sex byte.
    pub gender: u8,
}

impl super::UiScript {
    /// Mirror one guid's record, or forget it.
    pub fn set_name_record(&mut self, guid: u64, record: Option<NameRecord>) {
        let mut model = self.model_mut();
        match record {
            Some(r) => model.name_records.insert(guid, r),
            None => model.name_records.remove(&guid),
        };
    }

    /// The world-session wipe of the name cache.
    pub fn clear_name_records(&mut self) {
        self.model_mut().name_records.clear();
    }

    /// Drain the guids `GetPlayerInfoByGUID` asked the name cache for.
    pub fn take_name_asks(&mut self) -> Vec<u64> {
        drain_field!(self, name_asks)
    }
}

/// `UnitIsSameServer`'s verdict (`0x612670`): one unit or guid is one server; otherwise a differing
/// realm needs a record, and a missing one needs the other record to name a realm.
fn same_server(model: &Model, first: &str, second: &str) -> bool {
    if first.eq_ignore_ascii_case(second) {
        return true;
    }
    let a = guid_of(model, first).unwrap_or(0);
    let b = guid_of(model, second).unwrap_or(0);
    if a == b {
        return true;
    }
    let ra = model.name_records.get(&a);
    let rb = model.name_records.get(&b);
    let names_realm = |r: Option<&NameRecord>| r.is_some_and(|r| !r.realm.is_empty());
    let differ = (ra.is_some() || names_realm(rb))
        && (rb.is_some() || names_realm(ra))
        && match (ra, rb) {
            (Some(x), Some(y)) => x.realm != y.realm,
            _ => true,
        };
    !differ
}

/// The sex byte as `UnitSex` answers it (`{2, 3, 1, 6}`).
fn sex_value(gender: u8) -> i64 {
    match gender {
        0 => 2,
        1 => 3,
        2 => 1,
        _ => 6,
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // UnitIsSameServer(unit, otherUnit) (`0x612670`): 1 or nil.
    g.set(
        "UnitIsSameServer",
        lua.create_function(|lua, (first, second): (Value, Value)| {
            const USAGE: &str = "Usage: UnitIsSameServer(\"unit\", \"otherUnit\")";
            let first = string_arg(lua, first, USAGE)?;
            let second = string_arg(lua, second, USAGE)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(super::binding_abi::flag(same_server(
                &model, &first, &second,
            )))
        })?,
    )?;

    // GetPlayerInfoByGUID(guid) (`0x613f90`): class, class file, race, race file, sex, name and
    // realm from the name cache; nothing for a non-player guid or a miss.
    g.set(
        "GetPlayerInfoByGUID",
        lua.create_function(|lua, guid: Value| {
            const USAGE: &str = "Usage: GetPlayerInfoByGUID(\"playerGUID\")";
            let guid = super::combat_log::parse_guid(&string_arg(lua, guid, USAGE)?);
            if guid == 0 {
                return Err(mlua::Error::runtime(USAGE));
            }
            if guid >> 60 != 0 || guid & 0xf07f_ffff_ffff_ffff == 0 {
                return Ok(MultiValue::new());
            }
            let mut model = crate::script::model_mut_of(lua);
            let me = model.unit("player").filter(|u| u.guid == guid).cloned();
            let Some(r) = model.name_records.get(&guid).cloned() else {
                let flags = model.unit("player").map_or(0, |u| u.player_flags);
                if flags & PLAYER_FLAG_ASKS_NAMES != 0 {
                    model.name_asks.push(guid);
                }
                return Ok(MultiValue::new());
            };
            drop(model);
            let text = |s: Option<&str>| -> mlua::Result<Value> {
                s.map_or(Ok(Value::Nil), |s| lua.create_string(s).map(Value::String))
            };
            // The own record is seeded at login without the name query's race and class.
            let own = me.filter(|_| r.class.is_none());
            let (class, class_file, race, race_file, sex) = match &own {
                Some(u) => (
                    text(u.class.as_deref())?,
                    text(u.class_file.as_deref())?,
                    text(u.race.as_deref())?,
                    text(u.race_file.as_deref())?,
                    i64::from(u.sex),
                ),
                None => (
                    text(r.class.map(|c| c.0))?,
                    text(r.class.map(|c| c.1))?,
                    text(r.race.map(|c| c.0))?,
                    text(r.race.map(|c| c.1))?,
                    sex_value(r.gender),
                ),
            };
            Ok(MultiValue::from_vec(vec![
                class,
                class_file,
                race,
                race_file,
                Value::Integer(sex),
                Value::String(lua.create_string(&r.name)?),
                Value::String(lua.create_string(&r.realm)?),
            ]))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn bob(realm: &str) -> Option<NameRecord> {
        Some(NameRecord {
            name: "Bob".into(),
            realm: realm.into(),
            class: Some(("Mage", "MAGE")),
            race: Some(("Gnome", "Gnome")),
            gender: 1,
        })
    }

    /// `0x613f90`: seven returns from the record; the non-player and missing guids answer none.
    #[test]
    fn player_info_reads_the_record() {
        let mut s = UiScript::new().unwrap();
        s.set_name_record(0x21, bob("Other"));
        let row: (String, String, String, String, i32, String, String) = s
            .eval("return GetPlayerInfoByGUID('0x0000000000000021')")
            .unwrap();
        assert_eq!(row.0, "Mage");
        assert_eq!(row.1, "MAGE");
        assert_eq!(row.2, "Gnome");
        assert_eq!(row.3, "Gnome");
        assert_eq!(row.4, 3);
        assert_eq!(row.5, "Bob");
        assert_eq!(row.6, "Other");
        let n: i32 = s
            .eval("return select('#', GetPlayerInfoByGUID('0x0000000000000022'))")
            .unwrap();
        assert_eq!(n, 0, "a miss");
        let n: i32 = s
            .eval("return select('#', GetPlayerInfoByGUID('0xF130000000000022'))")
            .unwrap();
        assert_eq!(n, 0, "a creature");
        assert!(s.eval::<()>("GetPlayerInfoByGUID('zz')").is_err());
    }

    /// `0x612670`: one name is one server, and with neither record there is nothing to differ.
    #[test]
    fn same_server_needs_a_differing_realm() {
        let s = UiScript::new().unwrap();
        let same: Option<i32> = s.eval("return UnitIsSameServer('Bob', 'bob')").unwrap();
        assert_eq!(same, Some(1));
        let same: Option<i32> = s.eval("return UnitIsSameServer('Bob', 'Ann')").unwrap();
        assert_eq!(same, Some(1), "two misses both read guid 0");
        assert!(s.eval::<()>("UnitIsSameServer('Bob')").is_err());
    }
}
