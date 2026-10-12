//! `RegisterStaticConstants(table)` (12340 `0x5ac320`), which stock `Constants.lua:469` calls at
//! load on its `STATIC_CONSTANTS` table: the table must be a table (`Usage:` otherwise), then the
//! seven fixed pairs at `0xacfac0` are set into it through `lua_settable` (`0x5ac36f`).

use mlua::{Lua, Value};

/// The pairs at `0xacfac0`, name and number, in the reference's order.
const STATIC_CONSTANTS: [(&str, i64); 7] = [
    ("STATIC_CONSTANTS", 0),
    ("Loot", 1),
    ("AuctionHouse", 2),
    ("Mail", 3),
    ("Chat", 4),
    ("Movement", 5),
    ("Spell", 6),
];

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "RegisterStaticConstants",
        lua.create_function(|_, table: Value| {
            let Value::Table(table) = table else {
                return Err(mlua::Error::runtime(
                    "Usage: RegisterStaticConstants(table)",
                ));
            };
            for (name, value) in STATIC_CONSTANTS {
                table.set(name, value)?;
            }
            Ok(())
        })?,
    )
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    #[test]
    fn the_seven_static_constants_fill_the_table() {
        let s = UiScript::new().unwrap();
        s.run("T = {} RegisterStaticConstants(T)").unwrap();
        for (name, want) in super::STATIC_CONSTANTS {
            assert_eq!(
                s.eval::<i64>(&format!("return T.{name}")).unwrap(),
                want,
                "{name}"
            );
        }
        let err = s.run("RegisterStaticConstants()").unwrap_err().to_string();
        assert!(
            err.contains("Usage: RegisterStaticConstants(table)"),
            "{err}"
        );
    }
}
