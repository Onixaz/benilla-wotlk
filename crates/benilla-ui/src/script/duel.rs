//! The three duel globals (registrar table `0xad0ac4`). What the UI reads arrives as event
//! arguments, so each call only queues a [`DuelRequest`] for the app to send. `AcceptDuel`
//! (`0x5cfdb0`) and `CancelDuel` (`0x5cfdc0`) send the held arbiter; `StartDuel` (`0x5cf950`) sends
//! no duel opcode but casts the duel spell at the player `CGGameUI::ClosestObjectMatch`
//! (`0x520190`) resolves.

use mlua::{Lua, Value};

/// A duel intent queued by the globals, drained by `UiScript::take_duel_requests`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DuelRequest {
    /// `AcceptDuel()`, the `DUEL_REQUESTED` popup's Accept: `CMSG_DUEL_ACCEPTED`.
    Accept,
    /// `CancelDuel()`: `CMSG_DUEL_CANCELLED`, which the server reads by duel state as a decline,
    /// a cancel or a forfeit.
    Cancel,
    /// `StartDuel` named a unit token, already resolved to this player's guid.
    ChallengeGuid(u64),
    /// `StartDuel` named no token: the app walks the players for the name, whole names only when
    /// `exact`.
    ChallengeName { name: String, exact: bool },
}

impl super::UiScript {
    /// Drain the duel intents queued since the last call.
    pub fn take_duel_requests(&mut self) -> Vec<DuelRequest> {
        drain_field!(self, duel_requests)
    }

    /// Queue an intent from the app's Rust slash parser: `/forfeit`, which the reference's
    /// `SlashCmdList` handler sends through `CancelDuel`.
    pub fn queue_duel_request(&mut self, request: DuelRequest) {
        self.model_mut().duel_requests.push(request);
    }

    /// `StartDuel(name)` from the app's Rust slash parser, the stock `/duel` handler's one call.
    pub fn start_duel(&mut self, name: &str) {
        start_duel(&self.lua, name, false);
    }
}

/// `StartDuel`'s resolution: `ClosestObjectMatch` with typemask `0x10` and filter mode 0 tries the
/// token first (`0x60abf0`, which reads no token as the target), else the name.
fn start_duel(lua: &Lua, name: &str, exact: bool) {
    let mut model = crate::script::model_mut_of(lua);
    let guids = &model.unit_guids;
    let token = if name.is_empty() {
        Some(Some(guids.target))
    } else {
        guids.resolve(name).ok()
    };
    let request = match token {
        // A token naming no player answers nothing, and nothing is cast (`0x5cf992`).
        Some(guid) => match guid.filter(|&g| is_player_guid(g)) {
            Some(guid) => DuelRequest::ChallengeGuid(guid),
            None => return,
        },
        None => DuelRequest::ChallengeName {
            name: name.to_string(),
            exact,
        },
    };
    model.duel_requests.push(request);
}

/// A player guid, high word 0 (`HIGHGUID_PLAYER`).
fn is_player_guid(guid: u64) -> bool {
    guid != 0 && guid >> 48 == 0
}

/// Register the duel globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "AcceptDuel",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.duel_requests.push(DuelRequest::Accept);
            Ok(())
        })?,
    )?;

    // `CancelDuel()`: the popup's Decline, and `/forfeit` (`/concede`, `/yield`) once under way.
    g.set(
        "CancelDuel",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.duel_requests.push(DuelRequest::Cancel);
            Ok(())
        })?,
    )?;

    // `StartDuel([name [, exactMatch]])`: the unit menu's Duel row passes its token and 1.
    g.set(
        "StartDuel",
        lua.create_function(|lua, (name, exact): (Value, Value)| {
            let name = super::binding_abi::optional_string(lua, &name).unwrap_or_default();
            start_duel(lua, &name, super::binding_abi::bool_or_default(Some(&exact), false));
            Ok(())
        })?,
    )?;

    Ok(())
}
