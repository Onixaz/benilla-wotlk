//! `FollowUnit` and the 1.12 `FollowByName`; each call queues a [`FollowRequest`] the app drains.
//! 12340's `FollowUnit(nameOrUnit, exactMatch)` (`0x5224c0`) takes a unit token or a name: the
//! stock `/follow` passes its text (`""` bare, which names the target) and the unit popup the
//! unit's name with `1` (`UnitPopup.lua:1325`). The app resolves it.

use mlua::Lua;

/// A follow intent queued by the globals: the first argument as given, and the second as the
/// resolver's exact-only flag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FollowRequest {
    pub name: String,
    pub exact: bool,
}

impl super::UiScript {
    /// Drain the follow intents queued since the last call.
    pub fn take_follow_requests(&mut self) -> Vec<FollowRequest> {
        drain_field!(self, follow_requests)
    }
}

/// The reference's boolean read with default false: nil, false and 0 are false, any other value
/// true.
fn flag(value: Option<mlua::Value>) -> bool {
    match value {
        None | Some(mlua::Value::Nil) | Some(mlua::Value::Boolean(false)) => false,
        Some(mlua::Value::Integer(n)) => n != 0,
        Some(mlua::Value::Number(n)) => n != 0.0,
        Some(_) => true,
    }
}

/// Register the follow globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    // A nil first argument reads as the empty string, the target.
    let queue = |lua: &Lua, (name, exact): (Option<String>, Option<mlua::Value>)| {
        let mut model = crate::script::model_mut_of(lua);
        model.follow_requests.push(FollowRequest {
            name: name.unwrap_or_default(),
            exact: flag(exact),
        });
        Ok(())
    };
    g.set("FollowUnit", lua.create_function(queue)?)?;
    g.set("FollowByName", lua.create_function(queue)?)?;
    Ok(())
}
