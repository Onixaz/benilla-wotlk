//! The release, reclaim, resurrect and self-res verbs and the getters the `DEATH`,
//! `RECOVER_CORPSE`, `RESURRECT` and `XP_LOSS` dialogs call, each read through the lent view at
//! call time, as 12340's read the client's own state (`0x516210`, `0x516280`).

use mlua::{Lua, Value};

use super::read_live;

/// One drained death intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeathAction {
    /// `RepopMe()`: `CMSG_REPOP_REQUEST`.
    Repop,
    /// `RetrieveCorpse()`: `CMSG_RECLAIM_CORPSE`.
    RetrieveCorpse,
    /// `AcceptResurrect()`: `CMSG_RESURRECT_RESPONSE`, accepting.
    AcceptResurrect,
    /// `DeclineResurrect()`: `CMSG_RESURRECT_RESPONSE`, declining.
    DeclineResurrect,
    /// `AcceptXPLoss()`: `CMSG_SPIRIT_HEALER_ACTIVATE`.
    AcceptXpLoss,
    /// `UseSoulstone()`: `CMSG_SELF_RES` when `PLAYER_SELF_RES_SPELL` is non-zero, else a use of
    /// the carried item, decided at drain time as the reference decides it at call time.
    UseSoulstone,
}

impl super::UiScript {
    /// Drain the queued death intents.
    pub fn take_death_actions(&mut self) -> Vec<DeathAction> {
        drain_field!(self, death_actions)
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    fn install_action(lua: &Lua, name: &str, action: DeathAction) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, ()| {
                crate::script::model_mut_of(lua).death_actions.push(action);
                super::calls::press(lua, super::VerbPress::Death);
                Ok(())
            })?,
        )
    }
    install_action(lua, "RepopMe", DeathAction::Repop)?;
    install_action(lua, "RetrieveCorpse", DeathAction::RetrieveCorpse)?;
    install_action(lua, "AcceptResurrect", DeathAction::AcceptResurrect)?;
    install_action(lua, "DeclineResurrect", DeathAction::DeclineResurrect)?;
    install_action(lua, "AcceptXPLoss", DeathAction::AcceptXpLoss)?;
    install_action(lua, "UseSoulstone", DeathAction::UseSoulstone)?;

    // Whole seconds, truncated; -1 while the no-timer flag is set (`0x516210`).
    g.set(
        "GetReleaseTimeRemaining",
        lua.create_function(|_, ()| {
            let left = read_live(|l| l.release_time_left_ms()).unwrap_or(Some(0));
            Ok(left.map_or(-1, |ms| i64::from(ms / 1000)))
        })?,
    )?;

    // ResurrectGetOfferer() (`0x515950`): the caster's name, or nil.
    g.set(
        "ResurrectGetOfferer",
        lua.create_function(|_, _: mlua::MultiValue| {
            Ok(read_live(|l| l.resurrect_offerer()).flatten())
        })?,
    )?;

    // Whole seconds, truncated (`0x516280`).
    g.set(
        "GetCorpseRecoveryDelay",
        lua.create_function(|_, ()| {
            let ms = read_live(|l| l.corpse_recovery_delay_ms()).unwrap_or(0);
            Ok(i64::from(ms / 1000))
        })?,
    )?;

    // The predicates answer `1` or nil, as the reference pushes them.
    fn install_flag(
        lua: &Lua,
        name: &str,
        read: fn(&dyn super::LiveState) -> bool,
    ) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |_, ()| {
                Ok(super::binding_abi::flag(read_live(read).unwrap_or(false)))
            })?,
        )
    }
    install_flag(lua, "ResurrectHasSickness", |l| l.resurrect_sickness())?;
    install_flag(lua, "ResurrectHasTimer", |l| l.resurrect_has_timer())?;
    install_flag(lua, "CheckSpiritHealerDist", |l| l.spirit_healer_in_range())?;
    install_flag(lua, "IsFalling", |l| l.falling())?;
    install_flag(lua, "IsOutOfBounds", |l| l.out_of_bounds())?;
    install_flag(lua, "CannotBeResurrected", |l| l.cannot_be_resurrected())?;

    g.set(
        "GetResSicknessDuration",
        lua.create_function(|lua, ()| {
            let Some(minutes) = read_live(|l| l.res_sickness_minutes()).flatten() else {
                return Ok(Value::Nil);
            };
            // `GetText(token, nil, ordinal)`'s plural pick, through the shared primitive.
            let get = |key: &str| {
                lua.globals()
                    .get::<String>(key)
                    .ok()
                    .filter(|t| !t.is_empty())
            };
            Ok(
                match crate::strings::plural("GENERIC_MIN", Some(minutes), &get) {
                    Some(template) => Value::String(lua.create_string(crate::strings::fill(
                        &template,
                        &[crate::strings::Arg::D(i64::from(minutes))],
                    ))?),
                    None => Value::Nil,
                },
            )
        })?,
    )?;

    // Nil, never `0`, when there is none: the DEATH dialog shows its second button on any truthy
    // answer, stamps the answer on it and picks soulstone over release by it.
    g.set(
        "HasSoulstone",
        lua.create_function(|lua, ()| {
            Ok(match read_live(|l| l.self_res_label()).flatten() {
                Some(s) => Value::String(lua.create_string(&s)?),
                None => Value::Nil,
            })
        })?,
    )?;

    Ok(())
}
