//! Each unit's running cast and channel, behind `UnitCastingInfo` and `UnitChannelInfo`, and the
//! `UNIT_SPELLCAST_*` fan-out. The app pushes a unit's cast as its packets start, delay and end
//! it (the reference keeps them on the unit: the cast at `+0xa6c`/`+0xa78`/`+0xa7c`, its
//! `castCount` at `+0xa5c`, the channel at `+0xa80`..`+0xa88`), and fires each event through
//! [`super::UiScript::fire_unit_event`], once per token naming the caster (`0x60bb70`).

use std::collections::HashMap;

use mlua::{Lua, MultiValue, Value};

use super::ScriptValue;

/// One unit's cast or channel as `UnitCastingInfo` and `UnitChannelInfo` answer it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitCast {
    pub name: String,
    /// `NameSubtext`, `""` when the spell has none.
    pub rank: String,
    /// The bar's text: the name, `""` under `AttributesEx3 & 4` (`0x611e81`); a channel's is the
    /// name only with `AttributesEx` bit 29 (`0x612151`), else `CHANNELING`.
    pub text: String,
    pub texture: Option<String>,
    /// On the `GetTime` clock, in milliseconds (`+0xa78`/`+0xa7c`, `+0xa84`/`+0xa88`).
    pub start_ms: f64,
    pub end_ms: f64,
    /// `Attributes & 0x20`, a trade skill (`0x611fa8`).
    pub trade_skill: bool,
    /// The cast's `castCount` (`+0xa5c`); a channel has none.
    pub cast_count: u8,
    /// `CGUnit_C::IsCastNotInterruptible` (`0x71ab20`), as the app last evaluated it.
    pub not_interruptible: bool,
}

/// The pushed casts, by caster guid.
#[derive(Clone, Debug, Default)]
pub(crate) struct UnitCasts {
    casts: HashMap<u64, UnitCast>,
    channels: HashMap<u64, UnitCast>,
}

impl UnitCasts {
    /// The unit's pushed channel, running or not.
    pub(crate) fn channel_of(&self, guid: u64) -> Option<&UnitCast> {
        self.channels.get(&guid)
    }
}

impl super::UiScript {
    /// Push (or clear) a unit's cast, or with `channel` its channel.
    pub fn set_unit_cast(&mut self, guid: u64, channel: bool, cast: Option<UnitCast>) {
        let mut model = self.model_mut();
        let map = if channel {
            &mut model.unit_casts.channels
        } else {
            &mut model.unit_casts.casts
        };
        match cast {
            Some(c) => {
                map.insert(guid, c);
            }
            None => {
                map.remove(&guid);
            }
        }
    }

    /// Fire `event` once for each token naming `guid`, the token first, then `args`
    /// (`0x60bf60`: `%s%s%s%d`, unit, name, rank, castCount).
    pub fn fire_unit_event(&mut self, guid: u64, event: &str, args: Vec<ScriptValue>) {
        let tokens = self.model_ref().unit_guids.tokens_naming(guid);
        for token in tokens {
            let mut all = Vec::with_capacity(args.len() + 1);
            all.push(ScriptValue::Str(token));
            all.extend(args.iter().cloned());
            self.fire_event(event, all);
        }
    }
}

/// The unit a token names, through the shared resolver; `None` for nobody or an unknown token.
fn unit_of(lua: &Lua, unit: Value, usage: &'static str) -> mlua::Result<Option<u64>> {
    let token = super::binding_abi::string_arg(lua, unit, usage)?;
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    Ok(model.unit_guids.resolve(&token).ok().flatten())
}

/// The cast's values while it runs: `now` before its end (`0x611e75`), else none.
fn info(lua: &Lua, cast: &UnitCast, with_count: bool) -> mlua::Result<MultiValue> {
    let s = |t: &str| lua.create_string(t).map(Value::String);
    let mut v = vec![
        s(&cast.name)?,
        s(&cast.rank)?,
        s(&cast.text)?,
        match &cast.texture {
            Some(t) => s(t)?,
            None => s("")?,
        },
        Value::Number(cast.start_ms),
        Value::Number(cast.end_ms),
        Value::Boolean(cast.trade_skill),
    ];
    if with_count {
        v.push(Value::Integer(i64::from(cast.cast_count)));
    }
    v.push(Value::Boolean(cast.not_interruptible));
    Ok(MultiValue::from_vec(v))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // UnitCastingInfo("unit") (`0x611df0`): name, nameSubtext, text, texture, startTime, endTime,
    // isTradeSkill, castID, notInterruptible while the unit casts, else nothing. A non-string
    // raises.
    g.set(
        "UnitCastingInfo",
        lua.create_function(|lua, unit: Value| {
            let Some(guid) = unit_of(lua, unit, r#"Usage: UnitCastingInfo("unit")"#)? else {
                return Ok(MultiValue::new());
            };
            let now_ms = super::clock::now(lua) * 1000.0;
            let cast = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.unit_casts.casts.get(&guid).cloned()
            };
            match cast.filter(|c| now_ms < c.end_ms) {
                Some(c) => info(lua, &c, true),
                None => Ok(MultiValue::new()),
            }
        })?,
    )?;

    // UnitChannelInfo("unit") (`0x612090`): name, nameSubtext, text, texture, startTime, endTime,
    // isTradeSkill, notInterruptible while the unit channels, else nothing.
    g.set(
        "UnitChannelInfo",
        lua.create_function(|lua, unit: Value| {
            let Some(guid) = unit_of(lua, unit, r#"Usage: UnitChannelInfo("unit")"#)? else {
                return Ok(MultiValue::new());
            };
            let now_ms = super::clock::now(lua) * 1000.0;
            let cast = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.unit_casts.channels.get(&guid).cloned()
            };
            match cast.filter(|c| now_ms < c.end_ms) {
                Some(c) => info(lua, &c, false),
                None => Ok(MultiValue::new()),
            }
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::UnitCast;
    use crate::script::{ScriptValue, UiScript, UnitGuids};

    const ME: u64 = 0x10;
    const MOB: u64 = 0xF130_0000_0000_0001;

    fn script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit_guids(&UnitGuids {
            player: ME,
            target: MOB,
            ..Default::default()
        });
        s.set_now(10.0);
        s
    }

    fn heal() -> UnitCast {
        UnitCast {
            name: "Lesser Heal".into(),
            rank: "Rank 1".into(),
            text: "Lesser Heal".into(),
            texture: Some(r"Interface\Icons\Spell_Holy_LesserHeal".into()),
            start_ms: 10_000.0,
            end_ms: 11_500.0,
            trade_skill: false,
            cast_count: 7,
            not_interruptible: false,
        }
    }

    #[test]
    fn casting_info_answers_nine_values_while_the_cast_runs() {
        let mut s = script();
        assert_eq!(s.arity(r#"UnitCastingInfo("player")"#).unwrap(), 0);
        s.set_unit_cast(ME, false, Some(heal()));
        assert_eq!(s.arity(r#"UnitCastingInfo("player")"#).unwrap(), 9);
        assert_eq!(
            s.eval::<String>(
                r#"local n, r, t, i, a, b, ts, id, ni = UnitCastingInfo("player")
                   return table.concat({n, r, t, i, a, b, tostring(ts), id, tostring(ni)}, "|")"#
            )
            .unwrap(),
            r"Lesser Heal|Rank 1|Lesser Heal|Interface\Icons\Spell_Holy_LesserHeal|10000|11500|false|7|false"
        );
        assert_eq!(s.arity(r#"UnitChannelInfo("player")"#).unwrap(), 0);
        assert_eq!(s.arity(r#"UnitCastingInfo("target")"#).unwrap(), 0);
        s.set_now(11.5);
        assert_eq!(
            s.arity(r#"UnitCastingInfo("player")"#).unwrap(),
            0,
            "past its end"
        );
        let err = s.run("UnitCastingInfo()").unwrap_err().to_string();
        assert!(err.contains(r#"Usage: UnitCastingInfo("unit")"#), "{err}");
    }

    #[test]
    fn channel_info_answers_eight_values() {
        let mut s = script();
        s.set_unit_cast(MOB, true, Some(heal()));
        assert_eq!(s.arity(r#"UnitChannelInfo("target")"#).unwrap(), 8);
    }

    /// One fire per token naming the caster, the token first.
    #[test]
    fn a_unit_event_fans_out_over_its_tokens() {
        let mut s = script();
        s.set_unit_guids(&UnitGuids {
            player: ME,
            target: ME,
            ..Default::default()
        });
        s.run(
            "SEEN = {} local f = CreateFrame('Frame') f:RegisterEvent('UNIT_SPELLCAST_START')
             f:SetScript('OnEvent', function(self, e, u, n, r, c)
                 table.insert(SEEN, u .. ':' .. n .. ':' .. r .. ':' .. c) end)",
        )
        .unwrap();
        s.fire_unit_event(
            ME,
            "UNIT_SPELLCAST_START",
            vec![
                ScriptValue::Str("Lesser Heal".into()),
                ScriptValue::Str("Rank 1".into()),
                ScriptValue::Int(7),
            ],
        );
        assert_eq!(
            s.eval::<String>("return table.concat(SEEN, ' ')").unwrap(),
            "player:Lesser Heal:Rank 1:7 target:Lesser Heal:Rank 1:7"
        );
    }
}
