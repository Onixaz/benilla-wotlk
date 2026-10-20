//! The rune table and `GetRuneType` (`0x6131e0`).

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::number_arg;

/// The slots the rune verbs index.
pub const MAX_RUNES: usize = 8;

/// The player's runes: each slot's kind (0 blood, 1 unholy, 2 frost, 3 death) and how many slots
/// are live; 0 live makes every slot valid (`0x5edfc0`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuneState {
    pub kinds: [u8; MAX_RUNES],
    pub count: u8,
}

/// The table every game starts with, any class (`0x5edef0`): six runes, two each of blood,
/// unholy and frost.
impl Default for RuneState {
    fn default() -> Self {
        Self {
            kinds: [0, 0, 1, 1, 2, 2, 0, 0],
            count: 6,
        }
    }
}

impl RuneState {
    /// `0x5edfc0`: a slot below the live count, or any of the eight while none is set.
    fn valid(&self, index: usize) -> bool {
        index < MAX_RUNES && (self.count == 0 || index < usize::from(self.count))
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // GetRuneType(slot): nothing without the player object, then the 1-based kind of a valid slot.
    lua.globals().set(
        "GetRuneType",
        lua.create_function(|lua, slot: Value| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            if !model.unit("player").is_some_and(|u| u.exists) {
                return Ok(MultiValue::new());
            }
            let slot = number_arg(lua, slot, "Usage: Script_GetRuneType(slot)")?;
            let Some(index) = usize::try_from(slot - 1)
                .ok()
                .filter(|&i| model.runes.valid(i))
            else {
                return Ok(MultiValue::new());
            };
            Ok(MultiValue::from_vec(vec![Value::Integer(
                i64::from(model.runes.kinds[index]) + 1,
            )]))
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::{UiScript, UnitState};

    fn seated() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit(
            "player",
            Some(UnitState {
                exists: true,
                ..Default::default()
            }),
        );
        s
    }

    /// Every character starts on the reference's six runes, blood, unholy and frost in pairs.
    #[test]
    fn a_new_game_reads_two_runes_of_each_kind() {
        let s = seated();
        let kinds: Vec<i64> = (1..=6)
            .map(|i| s.eval(&format!("return GetRuneType({i})")).unwrap())
            .collect();
        assert_eq!(kinds, [1, 1, 2, 2, 3, 3]);
    }

    /// A slot past the live six or below 1 answers nothing, as does a call with no player.
    #[test]
    fn an_invalid_slot_or_no_player_answers_nothing() {
        let s = seated();
        for slot in [0, 7, 9, -1] {
            assert_eq!(
                s.arity(&format!("GetRuneType({slot})")).unwrap(),
                0,
                "{slot}"
            );
        }
        assert!(s.run("GetRuneType('x')").is_err());
        let bare = UiScript::new().unwrap();
        assert_eq!(bare.arity("GetRuneType('x')").unwrap(), 0);
    }
}
