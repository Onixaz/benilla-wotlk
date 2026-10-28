//! `IsInInstance`, `CanShowResetInstances` and `ResetInstances`, the whole Lua side of instance
//! lockouts (the rest arrives as `CHAT_MSG_SYSTEM` lines), and `IsThreatWarningEnabled`, which
//! keys on the map's instance type. The readers answer off state the app pushes from `Map.dbc`.

use mlua::{Lua, MultiValue, Value};

/// `IsInInstance()`'s type word for a `Map.dbc` `InstanceType`: the reference's table at
/// `0x83de58`, and `"none"` from 4 up.
pub fn instance_type_name(instance_type: u32) -> &'static str {
    match instance_type {
        1 => "party",
        2 => "raid",
        3 => "pvp",
        _ => "none",
    }
}

impl super::UiScript {
    /// Push the current map's `Map.dbc` `InstanceType`, `None` for a map with no row. Deviation:
    /// `IsInInstance()` then answers `nil, "none"`, because the reference returns two results
    /// without pushing either (`0x48a7a9`), a stack bug.
    pub fn set_instance_type(&mut self, instance_type: Option<u32>) {
        if self.model_ref().instance_type == instance_type {
            return;
        }
        let mut model = self.model_mut();
        model.instance_type = instance_type;
    }

    /// Push the current map's `MapName_lang` and whether its difficulty is dynamic (`Flags &
    /// 0x100`), `GetInstanceInfo`'s first and seventh returns; `None` for a map with no row.
    pub fn set_map_info(&mut self, info: Option<(String, bool)>) {
        if self.model_ref().map_info == info {
            return;
        }
        self.model_mut().map_info = info;
    }

    /// Push `CanShowResetInstances()`'s answer; the app owns its terms (the ownership latch, the
    /// last dungeon, its age and the current map).
    pub fn set_can_reset_instances(&mut self, can: bool) {
        if self.model_ref().can_reset_instances == can {
            return;
        }
        self.model_mut().can_reset_instances = can;
    }

    /// `ResetInstances()` calls since the last drain, each a `CMSG_RESET_INSTANCES`.
    pub fn take_reset_instance_asks(&mut self) -> u32 {
        drain_field!(self, reset_instance_asks)
    }
}

/// C's `atoi`, which `CVar` keeps as its integer: leading blanks, an optional sign, then digits up
/// to the first other byte; nothing parsed is 0.
fn atoi(s: &str) -> i64 {
    let s = s.trim_start();
    let (neg, rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let n = rest
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i64, |n, d| {
            n.saturating_mul(10).saturating_add(i64::from(d - b'0'))
        });
    if neg {
        -n
    } else {
        n
    }
}

/// The threat-warning flag `0x519df0` recomputes when the `threatWarning` CVar, the map or the
/// group changes, here computed on demand: mode 0 is never, 1 a dungeon or raid map
/// (`InstanceType` 1 or 2, `0x519e4b`), 2 in a party or raid (`0x519e08`), and any other value
/// always; the CVar is read as `atoi` reads it. `IsThreatWarningEnabled`, the threat tooltip and
/// the threat tutorial read it.
pub(crate) fn threat_warning_enabled(model: &super::model::Model) -> bool {
    let mode = model
        .cvars
        .get("threatwarning")
        .map_or(3, |c| atoi(&c.value));
    match mode {
        0 => false,
        1 => matches!(model.instance_type, Some(1 | 2)),
        2 => {
            let g = &model.unit_guids;
            g.party.iter().any(|&p| p != 0) || g.raid.iter().any(|&r| r != 0)
        }
        _ => true,
    }
}

impl super::UiScript {
    /// The threat-warning flag ([`threat_warning_enabled`]), for the app's engine-side readers.
    pub fn threat_warning_enabled(&self) -> bool {
        threat_warning_enabled(&self.model_ref())
    }
}

/// Register the lockout globals and `IsThreatWarningEnabled`.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `IsInInstance()` (`0x48a750`): 1 if `InstanceType` is nonzero, else nil; then the type word.
    g.set(
        "IsInInstance",
        lua.create_function(|lua, ()| {
            let ty = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.instance_type
            };
            let name = instance_type_name(ty.unwrap_or(0));
            let inside = match ty {
                Some(t) if t != 0 => Value::Number(1.0),
                _ => Value::Nil,
            };
            Ok(MultiValue::from_vec(vec![
                inside,
                Value::String(lua.create_string(name)?),
            ]))
        })?,
    )?;

    // `CanShowResetInstances()`: 1 or nil, the gate on the SELF menu's reset row.
    g.set(
        "CanShowResetInstances",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(if model.can_reset_instances {
                Value::Number(1.0)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // `IsThreatWarningEnabled()` (`0x517d30`): 1 or nil off the flag `0x519df0`
    // ([`threat_warning_enabled`]).
    g.set(
        "IsThreatWarningEnabled",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(super::binding_abi::flag(threat_warning_enabled(&model)))
        })?,
    )?;

    // `GetInstanceInfo()` (`0x51a8c0`): name, type word (`0xac8074`, "none" from 5 up),
    // `SMSG_INSTANCE_DIFFICULTY`'s difficulty + 1, its `MapDifficulty` row's name and size ("" and
    // 0 with no row), the dynamic difficulty, and whether the map's difficulty is dynamic.
    // Deviation: with no map row the reference pushes six values for its seven, a stack bug,
    // where this answers the six and a nil.
    g.set(
        "GetInstanceInfo",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let ty = model.instance_type.unwrap_or(0);
            let word = ["none", "party", "raid", "pvp", "arena"]
                .get(ty as usize)
                .copied()
                .unwrap_or("none");
            let (name, dynamic) = match &model.map_info {
                Some((name, dynamic)) => (
                    Value::String(lua.create_string(name)?),
                    Value::Boolean(*dynamic),
                ),
                None => (Value::Nil, Value::Nil),
            };
            let (map, difficulty, dynamic_difficulty) = model.lfg.world;
            let (size_name, size) = match model.lfg.catalog.map_difficulty(map, difficulty as i32) {
                Some(row) => (
                    crate::strings::global(lua, &row.difficulty_string)
                        .unwrap_or_else(|| row.difficulty_string.clone()),
                    row.max_players,
                ),
                None => (String::new(), 0),
            };
            Ok(MultiValue::from_vec(vec![
                name,
                Value::String(lua.create_string(word)?),
                Value::Number(f64::from(difficulty) + 1.0),
                Value::String(lua.create_string(&size_name)?),
                Value::Number(f64::from(size)),
                Value::Number(f64::from(dynamic_difficulty)),
                dynamic,
            ]))
        })?,
    )?;

    // `ResetInstances()`: the `CONFIRM_RESET_INSTANCES` dialog's Yes.
    g.set(
        "ResetInstances",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.reset_instance_asks += 1;
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{UiScript, UnitGuids};

    /// `0x519df0`'s four modes, off the `threatWarning` CVar, the map type and the group.
    #[test]
    fn the_threat_warning_follows_its_mode() {
        let mut s = UiScript::new().unwrap();
        s.register_cvars([("threatWarning", "3")]);
        let on = |s: &mut UiScript| {
            s.eval::<bool>("return IsThreatWarningEnabled() == 1")
                .unwrap()
        };
        assert!(on(&mut s), "3, the default, is always");

        s.set_cvar_host("threatWarning", "0");
        assert!(!on(&mut s), "0 is never");

        s.set_cvar_host("threatWarning", "1");
        s.set_instance_type(Some(0));
        assert!(!on(&mut s), "1 outside a dungeon");
        s.set_instance_type(Some(1));
        assert!(on(&mut s), "1 in a dungeon");
        s.set_instance_type(Some(3));
        assert!(!on(&mut s), "a battleground is neither dungeon nor raid");

        s.set_cvar_host("threatWarning", "2");
        assert!(!on(&mut s), "2 alone");
        s.set_unit_guids(&UnitGuids {
            player: 1,
            party: [2, 0, 0, 0],
            ..Default::default()
        });
        assert!(on(&mut s), "2 in a party");

        s.set_cvar_host("threatWarning", "7");
        assert!(on(&mut s), "any other value is always");
    }

    /// `0x51a8c0`'s seven returns off the pushed map; no row answers the six and a nil.
    #[test]
    fn get_instance_info_reads_the_current_map() {
        let mut s = UiScript::new().unwrap();
        let info = |s: &mut UiScript| {
            s.eval::<String>(
                "local n, t, d, dn, m, dd, dyn = GetInstanceInfo()                  return table.concat({tostring(n), t, d, '[' .. dn .. ']', m, dd, tostring(dyn)}, ',')",
            )
            .unwrap()
        };
        assert_eq!(info(&mut s), "nil,none,1,[],0,0,nil");
        s.set_instance_type(Some(2));
        s.set_map_info(Some(("Icecrown Citadel".into(), true)));
        assert_eq!(info(&mut s), "Icecrown Citadel,raid,1,[],0,0,true");
        s.set_instance_type(Some(0));
        s.set_map_info(Some(("Eastern Kingdoms".into(), false)));
        assert_eq!(info(&mut s), "Eastern Kingdoms,none,1,[],0,0,false");
    }
}
