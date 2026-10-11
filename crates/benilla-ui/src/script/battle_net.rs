//! 3.3.5a's Battle.net verbs, answered for a client with no Battle.net session.
//!
//! Each binding first asks the login client for a live Battle.net connection with its features on
//! (`0x6b0990`, then `0x533c40`); without one it answers zero or false. benilla only logs in to a
//! realm, never to Battle.net, so that is every answer it can give: `FriendsMicroButton`'s
//! `OnLoad` and `FriendsFrame` read the counts unguarded (`FriendsFrame.lua:346`, `:452`).

use mlua::Lua;

/// Install the verbs on a target whose Lua API has them.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    // `0x530ec0`: true only in connection states 3, 6 and 7.
    g.set("BNConnected", lua.create_function(|_, ()| Ok(false))?)?;
    // `0x537070` and `0x537010`.
    g.set("BNFeaturesEnabled", lua.create_function(|_, ()| Ok(false))?)?;
    g.set(
        "BNFeaturesEnabledAndConnected",
        lua.create_function(|_, ()| Ok(false))?,
    )?;
    // `0x534590`: total and online friends.
    g.set("BNGetNumFriends", lua.create_function(|_, ()| Ok((0, 0)))?)?;
    // `0x536400`, `0x535080`, `0x536890`.
    for name in [
        "BNGetNumBlocked",
        "BNGetNumFriendInvites",
        "BNGetNumBlockedToons",
    ] {
        g.set(name, lua.create_function(|_, ()| Ok(0))?)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    use crate::script::UiScript;

    #[test]
    fn a_3_3_5a_client_without_a_battle_net_session_counts_nothing() {
        let ui = UiScript::new().unwrap();
        let (total, online): (i64, i64) = ui.eval("return BNGetNumFriends()").unwrap();
        assert_eq!((total, online), (0, 0));
        assert_eq!(ui.eval::<i64>("return BNGetNumBlocked()").unwrap(), 0);
        assert!(!ui.eval::<bool>("return BNConnected()").unwrap());
        assert!(!ui
            .eval::<bool>("return BNFeaturesEnabledAndConnected()")
            .unwrap());
    }
}
