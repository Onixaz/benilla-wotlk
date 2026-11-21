//! Each unit's running cast and channel, behind `UnitCastingInfo` and `UnitChannelInfo`, and the
//! `UNIT_SPELLCAST_*` fan-out. The verbs read the cast the unit holds through the lent view when
//! called (the reference keeps them on the unit: the cast at `+0xa6c`/`+0xa78`/`+0xa7c`, its
//! `castCount` at `+0xa5c`, the channel at `+0xa80`..`+0xa88`); the app fires each event through
//! [`super::UiScript::fire_unit_event`], once per token naming the caster (`0x60bb70`).

use mlua::{Lua, MultiValue, Value};

use super::ScriptValue;

/// One unit's cast or channel as `UnitCastingInfo` and `UnitChannelInfo` answer it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitCast {
    pub name: String,
    /// `NameSubtext`, `""` when the spell has none.
    pub rank: String,
    /// The bar's text: the name, `""` under `AttributesEx3 & 4` (`0x611e81`); a channel's is the
    /// name, which the `"player"` token reads as `CHANNELING` under [`Self::channeling_text`].
    pub text: String,
    /// A channel the `"player"` token names `CHANNELING`: no `AttributesEx` bit 29 (`0x612151`).
    pub channeling_text: bool,
    pub texture: Option<String>,
    /// On the `GetTime` clock, in milliseconds (`+0xa78`/`+0xa7c`, `+0xa84`/`+0xa88`).
    pub start_ms: f64,
    pub end_ms: f64,
    /// `Attributes & 0x20`, a trade skill (`0x611fa8`).
    pub trade_skill: bool,
    /// The cast's `castCount` (`+0xa5c`); a channel has none.
    pub cast_count: u8,
    /// The unit's not-interruptible test (`0x71ab20`), as the app last evaluated it.
    pub not_interruptible: bool,
}

impl super::UiScript {
    /// Fire `event` once for each token naming `guid`, the token first, then `args`
    /// (`0x60bf60`: `%s%s%s%d`, unit, name, rank, castCount).
    pub fn fire_unit_event(&mut self, guid: u64, event: &str, args: Vec<ScriptValue>) {
        // The tokens are read as the event fires, in a host step's turn too: `"npc"` is the
        // interact target's (`0x60bece`), read through the lent `World`.
        let event = event.to_string();
        super::world_loan::in_turn_or_now(self, move |script| {
            let tokens = script.model_ref().tokens().tokens_naming(guid);
            for token in tokens {
                let mut all = Vec::with_capacity(args.len() + 1);
                all.push(ScriptValue::Str(token));
                all.extend(args.iter().cloned());
                script.fire_event(&event, all);
            }
        });
    }
}

/// The unit a token names, through the shared resolver, and the token; `None` for nobody or an
/// unknown token.
fn unit_of(lua: &Lua, unit: Value, usage: &'static str) -> mlua::Result<(Option<u64>, String)> {
    let token = super::binding_abi::string_arg(lua, unit, usage)?;
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    Ok((model.resolve_token(&token).ok().flatten(), token))
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
            let (Some(guid), _) = unit_of(lua, unit, r#"Usage: UnitCastingInfo("unit")"#)? else {
                return Ok(MultiValue::new());
            };
            let now_ms = super::clock::now(lua) * 1000.0;
            let cast = super::live::read_live(|l| l.unit_cast(guid, false)).flatten();
            match cast.filter(|c| now_ms < c.end_ms) {
                Some(c) => info(lua, &c, true),
                None => Ok(MultiValue::new()),
            }
        })?,
    )?;

    // UnitChannelInfo("unit") (`0x612090`): name, nameSubtext, text, texture, startTime, endTime,
    // isTradeSkill, notInterruptible while the unit channels, else nothing. Only the literal
    // `"player"` token reads `CHANNELING` for a channel without its own text (`0x612151`).
    g.set(
        "UnitChannelInfo",
        lua.create_function(|lua, unit: Value| {
            let (Some(guid), token) = unit_of(lua, unit, r#"Usage: UnitChannelInfo("unit")"#)?
            else {
                return Ok(MultiValue::new());
            };
            let now_ms = super::clock::now(lua) * 1000.0;
            let cast = super::live::read_live(|l| l.unit_cast(guid, true)).flatten();
            match cast.filter(|c| now_ms < c.end_ms) {
                Some(c) if c.channeling_text && token.eq_ignore_ascii_case("player") => {
                    let text = lua
                        .globals()
                        .get::<String>("CHANNELING")
                        .unwrap_or_default();
                    info(lua, &UnitCast { text, ..c }, false)
                }
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
    use crate::script::live::TestView;
    use crate::script::{ScriptValue, UiScript, UnitGuids};

    /// A view answering `casts` as the units' held casts and channels, by `(guid, channel)`.
    fn lend(casts: Vec<((u64, bool), UnitCast)>) -> TestView {
        TestView {
            unit_casts: casts.into_iter().collect(),
            ..Default::default()
        }
    }

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
            channeling_text: false,
        }
    }

    #[test]
    fn casting_info_answers_nine_values_while_the_cast_runs() {
        let mut s = script();
        assert_eq!(s.arity(r#"UnitCastingInfo("player")"#).unwrap(), 0);
        let mut view_v = lend(vec![((ME, false), heal())]);
        crate::script::live::lending(&mut view_v, || {
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
        });
    }

    #[test]
    fn channel_info_answers_eight_values() {
        let s = script();
        let mut view_v = lend(vec![((MOB, true), heal())]);
        crate::script::live::lending(&mut view_v, || {
            assert_eq!(s.arity(r#"UnitChannelInfo("target")"#).unwrap(), 8);
        });
    }

    /// `0x612090` says `CHANNELING` only to the literal `"player"` token, in any case: the same
    /// channel read through `"target"` names the spell.
    #[test]
    fn only_the_player_token_reads_the_channeling_text() {
        let mut s = script();
        s.set_unit_guids(&UnitGuids {
            player: ME,
            target: ME,
            ..Default::default()
        });
        s.run("CHANNELING = 'Channeling'").unwrap();
        let mut view_v = lend(vec![(
            (ME, true),
            UnitCast {
                channeling_text: true,
                ..heal()
            },
        )]);
        crate::script::live::lending(&mut view_v, || {
            let text = |s: &UiScript, token: &str| {
                s.eval::<String>(&format!("return (select(3, UnitChannelInfo({token:?})))"))
                    .unwrap()
            };
            assert_eq!(text(&s, "player"), "Channeling");
            assert_eq!(text(&s, "PLAYER"), "Channeling");
            assert_eq!(text(&s, "target"), "Lesser Heal");
        });
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
