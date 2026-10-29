//! The focus `[0xbd07d0]`: `FocusUnit` and `ClearFocus` set it through the game UI's focus setter
//! (`0x51ff20`), which signals `PLAYER_FOCUS_CHANGED` on a change. It rides
//! [`super::UnitGuids::focus`], which the app's pushes keep, so `focus` resolves like any base.

use mlua::{Lua, Value};

/// The guid a token naming nobody resolves to (`0x60b00a`): stored as a focus, it names no unit.
pub const NOBODY: u64 = 0xFFFF_FFFF_FFFF_FFFE;

impl super::UiScript {
    /// The focus guid, 0 for none.
    pub fn focus_guid(&self) -> u64 {
        self.model_ref().unit_guids.focus
    }

    /// The focus setter from the engine: the target break, the world leave and the pet-change
    /// handler clear the focus with it.
    pub fn set_focus_guid(&mut self, guid: u64) {
        set_focus_target(&self.lua, guid);
    }
}

/// The game UI's focus setter (`0x51ff20`): a new guid is stored and signals
/// `PLAYER_FOCUS_CHANGED`. Its blocked arm, for tainted code, never runs here: benilla runs every
/// chunk untainted.
fn set_focus_target(lua: &Lua, guid: u64) {
    {
        let mut model = super::model_mut_of(lua);
        if model.unit_guids.focus == guid {
            return;
        }
        model.unit_guids.focus = guid;
    }
    super::tick::fire_event_into(lua, "PLAYER_FOCUS_CHANGED", Vec::new());
}

/// Register `FocusUnit` and `ClearFocus`.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // FocusUnit([unit]) (`0x522480`): the token's guid through `0x60abf0` with its target default,
    // so no token focuses the target; a token naming nobody focuses [`NOBODY`] and an unknown one
    // does nothing.
    g.set(
        "FocusUnit",
        lua.create_function(|lua, token: Value| {
            let token = super::binding_abi::optional_string(lua, &token).unwrap_or_default();
            let guid = {
                let model = super::model_mut_of(lua);
                let guids = &model.unit_guids;
                if token.is_empty() {
                    Some(guids.target)
                } else {
                    guids.resolve(&token).ok().map(|g| g.unwrap_or(NOBODY))
                }
            };
            if let Some(guid) = guid {
                set_focus_target(lua, guid);
            }
            Ok(())
        })?,
    )?;

    // ClearFocus() (`0x5225e0`): the setter with guid 0.
    g.set(
        "ClearFocus",
        lua.create_function(|lua, _: mlua::MultiValue| {
            set_focus_target(lua, 0);
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::{UiScript, UnitGuids};

    const MOB: u64 = 0xF130_0000_0000_0001;

    fn with_target() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit_guids(&UnitGuids {
            player: 0x10,
            target: MOB,
            ..Default::default()
        });
        s.run(
            "FOCUS_SEEN = 0 \
             local f = CreateFrame('Frame') f:RegisterEvent('PLAYER_FOCUS_CHANGED') \
             f:SetScript('OnEvent', function() FOCUS_SEEN = FOCUS_SEEN + 1 end)",
        )
        .unwrap();
        s
    }

    fn seen(s: &UiScript) -> i64 {
        s.eval::<i64>("return FOCUS_SEEN").unwrap()
    }

    /// `PLAYER_FOCUS_CHANGED` fires once per change; the same guid again is a no-op.
    #[test]
    fn the_event_fires_once_per_change() {
        let s = with_target();
        s.run("FocusUnit('target')").unwrap();
        assert_eq!(s.focus_guid(), MOB);
        assert_eq!(seen(&s), 1);
        s.run("FocusUnit('target')").unwrap();
        assert_eq!(seen(&s), 1, "the same guid fires nothing");
        s.run("ClearFocus()").unwrap();
        assert_eq!((s.focus_guid(), seen(&s)), (0, 2));
        s.run("ClearFocus()").unwrap();
        assert_eq!(seen(&s), 2);
    }

    /// No token is the target; nobody is the sentinel; an unknown token leaves the focus alone.
    #[test]
    fn the_token_defaults_to_the_target() {
        let s = with_target();
        s.run("FocusUnit()").unwrap();
        assert_eq!(s.focus_guid(), MOB);
        s.run("FocusUnit('party1')").unwrap();
        assert_eq!(s.focus_guid(), super::NOBODY);
        s.run("FocusUnit('nonsense')").unwrap();
        assert_eq!(s.focus_guid(), super::NOBODY);
    }

    /// The app's guid pushes keep the focus, and `focus` resolves through it.
    #[test]
    fn a_guid_push_keeps_the_focus() {
        let mut s = with_target();
        s.run("FocusUnit('target')").unwrap();
        s.set_unit_guids(&UnitGuids {
            player: 0x10,
            ..Default::default()
        });
        assert_eq!(s.focus_guid(), MOB);
        s.set_focus_guid(0);
        assert_eq!((s.focus_guid(), seen(&s)), (0, 2));
    }
}
