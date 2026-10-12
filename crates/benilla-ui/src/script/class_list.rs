//! `FillLocalizedClassList(classTable[, isFemale])` (12340 `0x60a510`), which stock
//! `Constants.lua:87-88` calls at load to build `LOCALIZED_CLASS_NAMES_MALE` and `_FEMALE`: every
//! `ChrClasses.dbc` row in record order (`[0xad340c]` rows of `0x30` bytes at `[0xad3420]`) sets
//! `classTable[Filename] = name` (`0x60a58b`-`0x60a5a3`). The name is `0x7159e0`'s: the asked
//! gender's column, else the other gender's, else the plain `Name_lang`.

use mlua::{Lua, Value};

use super::Model;

/// One `ChrClasses.dbc` row as `FillLocalizedClassList` reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClassNames {
    /// `Filename`, the class token (`"WARRIOR"`), the table key.
    pub file: String,
    /// `Name_lang`, the fallback when neither gendered column carries text (`0x715a41`).
    pub name: String,
    /// `Name_female_lang`; empty in enUS.
    pub female: String,
    /// `Name_male_lang`; empty in enUS.
    pub male: String,
}

impl ClassNames {
    /// `0x7159e0(row, female)`: the asked gender's column when it has text, else the other's,
    /// else `Name_lang`.
    fn for_sex(&self, female: bool) -> &str {
        let (first, second) = if female {
            (&self.female, &self.male)
        } else {
            (&self.male, &self.female)
        };
        [first, second]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or(&self.name)
    }
}

impl super::UiScript {
    /// Push `ChrClasses.dbc`'s rows in record order, which `FillLocalizedClassList` walks. The app
    /// seeds each VM before its first file runs, since `Constants.lua` asks at load.
    pub fn set_class_names(&mut self, rows: Vec<ClassNames>) {
        self.model_mut().class_names = rows;
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "FillLocalizedClassList",
        lua.create_function(|lua, (table, female): (Value, Value)| {
            // `lua_type(1) == LUA_TTABLE` or the usage error (`0x60a527`).
            let Value::Table(table) = table else {
                return Err(mlua::Error::runtime(
                    "Usage: FillLocalizedClassList(classTable[, isFemale])",
                ));
            };
            let female = !matches!(female, Value::Nil | Value::Boolean(false));
            let rows = lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .class_names
                .clone();
            for row in &rows {
                table.raw_set(row.file.as_str(), row.for_sex(female))?;
            }
            Ok(())
        })?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn row(file: &str, name: &str, female: &str, male: &str) -> ClassNames {
        ClassNames {
            file: file.into(),
            name: name.into(),
            female: female.into(),
            male: male.into(),
        }
    }

    /// enUS ships no gendered names, so both tables take `Name_lang`; a locale that does picks
    /// the asked gender, then the other, then the plain name.
    #[test]
    fn the_class_tables_fill_by_token_and_gender() {
        let mut s = UiScript::new().unwrap();
        s.set_class_names(vec![
            row("WARRIOR", "Warrior", "", ""),
            row("MAGE", "Mage", "Magierin", "Magier"),
            row("DRUID", "Druid", "Druidin", ""),
        ]);
        s.run(
            "M, F = {}, {} FillLocalizedClassList(M, false) FillLocalizedClassList(F, true) \
             D = {} FillLocalizedClassList(D)",
        )
        .unwrap();
        for (expr, want) in [
            ("M.WARRIOR", "Warrior"),
            ("F.WARRIOR", "Warrior"),
            ("M.MAGE", "Magier"),
            ("F.MAGE", "Magierin"),
            ("M.DRUID", "Druidin"),
            ("F.DRUID", "Druidin"),
            ("D.MAGE", "Magier"),
        ] {
            assert_eq!(
                s.eval::<String>(&format!("return {expr}")).unwrap(),
                want,
                "{expr}"
            );
        }
        let err = s
            .run("FillLocalizedClassList('x')")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Usage: FillLocalizedClassList(classTable[, isFemale])"),
            "{err}"
        );
    }
}
