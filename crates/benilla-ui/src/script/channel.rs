//! The chat-channel globals: the joined-channel lookups, the join, leave and management verbs the
//! app sends, and the guild-recruitment auto-join latch. `GetChannelName` looks up by 1-based slot
//! or by name and returns three values: the slot (`CHAT_MSG_*` arg8), the name and `instanceID`
//! (arg10).

use mlua::{IntoLuaMulti, Lua, MultiValue, Value};

/// One `ChatChannels.dbc` row for the channel verbs: the id, the Shortcut a typed name is matched
/// against case-folded, the name composed for the player's zone (`None` while the zone text is
/// empty, the verbs' nil leg), and whether it is listed (a `flags & 0x10` city row only in a city).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZoneChannelRow {
    pub id: u32,
    pub shortcut: String,
    pub resolved: Option<String>,
    pub listed: bool,
    /// `flags & 1`, a channel `ResetChatWindows` joins back.
    pub auto_join: bool,
    /// `flags & 0x20000`, the `GUILD_REQ` row whose typed Shortcut resets the guild-recruitment
    /// latch in a manual join or leave (`0x507bd9`, `0x509513`).
    pub guild_recruitment: bool,
}

/// A channel verb's ask, drained by the app into its `CMSG_*`: the stock `ChatFrame.lua` slash
/// handlers' calls, one variant each.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelCommand {
    /// `JoinChannelByName(name, password)`, sent on both non-nil legs; `voice` is the `hasVoice`
    /// argument, kept on the record (`+0xa4`, `0x5077c8`), and `permanent` is
    /// `JoinPermanentChannel`'s flag (`+0xa5`, `0x5077d9`), which the chat cache's save reads
    /// (`0x4fd432`). `id` is the matched row's ChannelID (`0x507baa`), 0 for a custom channel.
    Join {
        id: u32,
        name: String,
        password: String,
        voice: bool,
        permanent: bool,
    },
    /// `LeaveChannelByName(name)`; `id` is the matched row's ChannelID (`0x509502`), 0 for a
    /// custom channel.
    Leave {
        id: u32,
        name: String,
    },
    /// `ListChannelByName(name)`: `CMSG_CHANNEL_LIST`.
    List {
        name: String,
    },
    /// `ListChannels()`: the roster of every joined channel.
    ListAll,
    /// `DisplayChannelOwner(name)`: `CMSG_CHANNEL_OWNER`.
    DisplayOwner {
        name: String,
    },
    /// `SetChannelOwner(name, player)`.
    SetOwner {
        name: String,
        player: String,
    },
    /// `SetChannelPassword(name, password)`.
    SetPassword {
        name: String,
        password: String,
    },
    Ban {
        name: String,
        player: String,
    },
    Invite {
        name: String,
        player: String,
    },
    Kick {
        name: String,
        player: String,
    },
    Moderator {
        name: String,
        player: String,
    },
    Unmoderator {
        name: String,
        player: String,
    },
    Mute {
        name: String,
        player: String,
    },
    Unmute {
        name: String,
        player: String,
    },
    Unban {
        name: String,
        player: String,
    },
    /// `ChannelModerate(name)`: toggles moderation.
    Moderate {
        name: String,
    },
    /// `DeclineInvite(channel)` (`0x4ffcc0`): `CMSG_CHAT_CHANNEL_DECLINE_INVITE`.
    DeclineInvite {
        name: String,
    },
    /// `ChannelToggleAnnouncements(name)`.
    ToggleAnnouncements {
        name: String,
    },
    /// The channel pane's new selection: `CMSG_SET_CHANNEL_WATCH`.
    Watch {
        name: String,
    },
    /// The channel pane's roster ask (`0x4fe530`): `CMSG_CHANNEL_DISPLAY_LIST`.
    DisplayList {
        name: String,
    },
}

impl super::UiScript {
    /// Channel verbs called since the last drain, in call order.
    pub fn take_channel_commands(&mut self) -> Vec<ChannelCommand> {
        drain_field!(self, channel_commands)
    }

    /// The guild-recruitment latch (`[0xac7a3c]`): `1` AUTO, any other value STANDARD.
    pub fn guild_recruitment_mode(&self) -> i32 {
        self.model_ref().guild_recruitment_mode
    }

    /// Seat the latch from the chat cache at login. A host write is not a player gesture, so it
    /// does not arm [`Self::take_guild_recruitment_change`]: a login must not save what it read.
    pub fn set_guild_recruitment_mode(&mut self, mode: i32) {
        let mut model = self.model_mut();
        model.guild_recruitment_mode = mode;
        model.guild_recruitment_changed = false;
    }

    /// Whether the latch moved since the last drain: the chat cache's dirty signal.
    pub fn take_guild_recruitment_change(&mut self) -> bool {
        drain_field!(self, guild_recruitment_changed)
    }

    /// The mode setter's store (`0x508260`): an unmoved latch is a no-op (`0x50826c`), a moved one
    /// is stored (`0x508275`). Returns whether it moved; the caller runs what follows the store.
    pub fn set_guild_recruitment_mode_from_cvar(&mut self, mode: i32) -> bool {
        let mut model = self.model_mut();
        if model.guild_recruitment_mode == mode {
            return false;
        }
        model.guild_recruitment_mode = mode;
        model.guild_recruitment_changed = true;
        true
    }

    /// A manual join or leave of `GuildRecruitment` forces the latch to 0 and, as a player gesture,
    /// arms the save; mode 0 has no cascade, and the cascade's own join passes a flag that skips
    /// this reset. Returns whether the latch moved.
    pub fn reset_guild_recruitment_mode(&mut self) -> bool {
        let mut model = self.model_mut();
        if model.guild_recruitment_mode == 0 {
            return false;
        }
        model.guild_recruitment_mode = 0;
        model.guild_recruitment_changed = true;
        true
    }
}

/// The pane's selection asks (`0x4fe530`), queued under its model borrow, sent on the host inside
/// the same call once it lets go; with no host lent they stay recorded.
pub(super) fn send_queued(lua: &Lua) {
    if !super::world_loan::lent() {
        return;
    }
    let queued = std::mem::take(&mut crate::script::model_mut_of(lua).channel_commands);
    for cmd in queued {
        push(lua, cmd);
    }
}

/// A channel verb's send, made on the host inside the call; with no host lent the VM records it,
/// which only its tests read. The caller holds no model borrow. `false` when the verb returned
/// before its work.
fn push(lua: &Lua, cmd: ChannelCommand) -> bool {
    match super::calls::ask_host(lua, &super::HostAsk::Channel(cmd.clone())) {
        Some(super::HostAnswer::Declined) => false,
        Some(_) => true,
        None => {
            crate::script::model_mut_of(lua).channel_commands.push(cmd);
            true
        }
    }
}

/// The verbs' channel argument (`0x4fe160`): an empty name is nothing (`0x4fe16b`); a non-zero
/// integer parse names the record whose own number it is and whose state is 0 (`0x4fe197`,
/// `0x4fe19f`) and answers that record's name, else nothing; any other name passes as typed.
fn channel_string(key: &str) -> Option<String> {
    if key.is_empty() {
        return None;
    }
    match super::cvars::sstr_to_int(key) {
        0 => Some(key.to_string()),
        n => usize::try_from(n)
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|i| super::channel_pane::live_slots().get(i).cloned().flatten())
            .filter(|s| s.active)
            .map(|s| s.name),
    }
}

/// A channel verb's string argument as `lua_isstring` and `lua_tolstring` read it: a string, or a
/// number in its string form; anything else is `None`.
fn lua_string(lua: &Lua, v: &Value) -> mlua::Result<Option<String>> {
    match v {
        Value::String(_) | Value::Integer(_) | Value::Number(_) => {
            Ok(lua.coerce_string(v.clone())?.map(|s| s.to_string_lossy()))
        }
        _ => Ok(None),
    }
}

/// The joined channels' names at the call, in slot order with holes, read live off the channel
/// records (`0xbcf094`); none with nothing lent.
pub(crate) fn joined_names() -> Vec<Option<String>> {
    super::read_live(|l| l.channel_slots())
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.map(|s| s.name))
        .collect()
}

/// `ChatChannels.dbc` composed for the zone at the call; none with nothing lent.
pub(crate) fn zone_catalog() -> Vec<ZoneChannelRow> {
    super::read_live(|l| l.zone_channel_catalog()).unwrap_or_default()
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetChannelName(n | name) (`0x4fe850`): a number, a numeric string included (`lua_isnumber`),
    // names the record whose own number it is and whose state is 0 (`0x4fe895`, `0x4fe8a3`); a
    // name finds its record whatever the state (`0x4fe8e4`). Each answers (number, name, `+0x98`),
    // a miss (0, nil, 0); any other argument is the usage error (`0x4fe938`).
    g.set(
        "GetChannelName",
        lua.create_function(|lua, key: Value| {
            let slots = super::channel_pane::live_slots();
            let found = match &key {
                Value::Integer(_) | Value::Number(_) | Value::String(_)
                    if lua.coerce_number(key.clone())?.is_some() =>
                {
                    let n = lua.coerce_number(key.clone())?.unwrap_or_default().trunc() as i64;
                    usize::try_from(n)
                        .ok()
                        .and_then(|n| n.checked_sub(1))
                        .and_then(|i| Some((i + 1, slots.get(i)?.as_ref()?)))
                        .filter(|(_, s)| s.active)
                }
                Value::String(s) => {
                    let name = s.to_str()?;
                    slots.iter().enumerate().find_map(|(i, s)| {
                        s.as_ref()
                            .filter(|s| s.name.eq_ignore_ascii_case(&name))
                            .map(|s| (i + 1, s))
                    })
                }
                _ => {
                    return Err(mlua::Error::RuntimeError(
                        "Usage: GetChannelName([channelIndex] or [channelName])".into(),
                    ))
                }
            };
            let Some((number, slot)) = found else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Integer(0),
                    Value::Nil,
                    Value::Integer(0),
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::Integer(number as i64),
                Value::String(lua.create_string(&slot.name)?),
                Value::Integer(i64::from(slot.instance)),
            ]))
        })?,
    )?;

    // GetChannelList() (`0x4fe650`): a flat vararg of number, name pairs over the records in state
    // 0 (`0x4fe67a`, `0x4fe681`), a zone channel by its row's Shortcut and a custom one by its name
    // (`0x4fe6a0`-`0x4fe6dc`), as `FCFDropDown_LoadChannels` steps it; none is zero returns.
    g.set(
        "GetChannelList",
        lua.create_function(|lua, ()| {
            let slots = super::channel_pane::live_slots();
            let mut out = Vec::with_capacity(slots.len() * 2);
            for (i, slot) in slots.iter().enumerate() {
                let Some(slot) = slot.as_ref().filter(|s| s.active) else {
                    continue;
                };
                out.push(Value::Integer(i as i64 + 1));
                let name = slot.shortcut.as_deref().unwrap_or(&slot.name);
                out.push(Value::String(lua.create_string(name)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // The three joins: `JoinTemporaryChannel` and `JoinChannelByName` are one binding (`0x50daa0`),
    // `JoinPermanentChannel` (`0x50dac0`) the same with the permanent flag, each
    // `(name [, password [, frameId [, hasVoice]]])`. A DBC row answers `(ChannelID, Shortcut)`,
    // the row's `+0x10` the join writes out (`0x507ba0`) and the verb pushes (`0x50da72`), and a
    // custom channel `(0, nil)` (`0x50da86`), both sending; a `frameId` that truncates to 1..=10 registers
    // the channel in that window's list, deduplicated by name. A non-string name raises the usage;
    // a space, a row whose zone text is empty, or a name or password of 128 bytes or more, answers
    // nothing (`0x50d974`-`0x50d99e`), as does a profane name, after its `ERR_PROFANE_CHAT_NAME`
    // (`0x50d9c7`, `0x50d9d8`). A matched row's password is dropped.
    for (verb, permanent) in [
        ("JoinChannelByName", false),
        ("JoinTemporaryChannel", false),
        ("JoinPermanentChannel", true),
    ] {
        g.set(
            verb,
            lua.create_function(
                move |lua, (name, password, frame, voice): (Value, Value, Value, Value)| {
                    let text = |v: &Value| match v {
                        Value::String(s) => Some(s.to_string_lossy()),
                        Value::Integer(n) => Some(n.to_string()),
                        Value::Number(n) => Some(n.to_string()),
                        _ => None,
                    };
                    let Some(name) = text(&name) else {
                        return Err(mlua::Error::runtime(format!(
                            "Usage: {verb}(\"name\" [,\"password\"] [,index] [,hasVoice])"
                        )));
                    };
                    let password = text(&password).unwrap_or_default();
                    if name.is_empty() || name.len() >= 0x80 || password.len() >= 0x80 {
                        return Ok(MultiValue::new());
                    }
                    let voice = !matches!(voice, Value::Nil | Value::Boolean(false));
                    match join_by_name(lua, name, password, frame, voice, permanent)? {
                        Some(answer) => answer.into_lua_multi(lua),
                        None => Ok(MultiValue::new()),
                    }
                },
            )?,
        )?;
    }

    // EnumerateServerChannels(): the `ChatChannels.dbc` Shortcuts in row order, never the composed
    // names, a `flags & 0x10` row only in a city (`AreaTable.Flags & 0x8`); zero values while the
    // zone is unresolved.
    g.set(
        "EnumerateServerChannels",
        lua.create_function(|lua, _ignored: MultiValue| {
            let mut out = Vec::new();
            for row in zone_catalog().iter().filter(|r| r.listed) {
                out.push(Value::String(lua.create_string(&row.shortcut)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // LeaveChannelByName(name) (`0x5096d0` -> `0x5093f0`) returns nothing and raises `Usage:` on a
    // non-string, non-number. A name whose integer parse is non-zero names a confirmed record, else
    // the call does nothing (`0x509403` -> `0x4fe160`). A Shortcut composes its name for this zone
    // exactly as the join does, bailing on an empty subject (`0x5094b1`-`0x509505`), and strips
    // the windows by the Shortcut; any other name strips by itself (`0x50952f`-`0x509560`). The
    // record lookup and the send are the host's (`0x509562`-`0x509613`).
    g.set(
        "LeaveChannelByName",
        lua.create_function(|lua, name: Option<Value>| {
            let key = match name {
                Some(Value::String(s)) => s.to_str().map(|s| s.to_string())?,
                Some(Value::Integer(i)) => i.to_string(),
                Some(Value::Number(n)) => format!("{n:.14}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string(),
                _ => return Err(mlua::Error::runtime(r#"Usage: LeaveChannelByName("name")"#)),
            };
            // The Shortcut compare reads the typed key, never the number leg's name (`0x50948e`).
            let row = zone_catalog()
                .into_iter()
                .find(|r| r.shortcut.eq_ignore_ascii_case(&key));
            // The channel string: an empty name, or a number naming no record in state 0, is
            // nothing (`0x509403` -> `0x4fe160`, `0x509410`).
            let Some(key) = channel_string(&key) else {
                return Ok(());
            };
            // A matched row composes its name and takes its ChannelID (`0x5094e9`, `0x509502`);
            // a custom channel's id is 0, its record found by name (`0x50959f`).
            let (id, wire, strip_key, guild_row) = match row {
                Some(ZoneChannelRow { resolved: None, .. }) => return Ok(()),
                Some(ZoneChannelRow {
                    id,
                    resolved: Some(r),
                    shortcut,
                    guild_recruitment,
                    ..
                }) => (id, r, shortcut, guild_recruitment),
                None => (0, key.clone(), key, false),
            };
            let mut model = crate::script::model_mut_of(lua);
            // The verb's manual leave on the `GUILD_REQ` row stores STANDARD (`0x50950d`-`0x509525`).
            if guild_row {
                manual_reset(&mut model);
            }
            model.strip_window_channel(&strip_key);
            drop(model);
            push(lua, ChannelCommand::Leave { id, name: wire });
            Ok(())
        })?,
    )?;

    // The one-channel verbs (`0x4fe460`): a non-string channel is the `Usage: %s("channel")` error
    // (`0x4fe47f`); the channel string (`0x4fe4b7` -> `0x4fe160`) refused sends nothing, else the
    // packet carries what it answered (`0x4fe4eb`). `DeclineInvite` (`0x4ffcc0`),
    // `ListChannelByName` (`0x4fe630`), `DisplayChannelOwner` (`0x4fe830`) and
    // `ChannelToggleAnnouncements` (`0x4fea50`) are this body.
    for (verb, make) in [
        (
            "DeclineInvite",
            (|name| ChannelCommand::DeclineInvite { name }) as fn(String) -> ChannelCommand,
        ),
        ("ListChannelByName", |name| ChannelCommand::List { name }),
        ("DisplayChannelOwner", |name| ChannelCommand::DisplayOwner {
            name,
        }),
        ("ChannelToggleAnnouncements", |name| {
            ChannelCommand::ToggleAnnouncements { name }
        }),
    ] {
        g.set(
            verb,
            lua.create_function(move |lua, name: Value| {
                let Some(name) = lua_string(lua, &name)? else {
                    return Err(mlua::Error::runtime(format!("Usage: {verb}(\"channel\")")));
                };
                if let Some(name) = channel_string(&name) {
                    push(lua, make(name));
                }
                Ok(())
            })?,
        )?;
    }

    // ChannelModerate is a 1.12.1 global 3.3.5a does not have (#52): its 1.12 body, kept until
    // that removal.
    g.set(
        "ChannelModerate",
        lua.create_function(|lua, name: Option<String>| {
            if let Some(name) = name.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
                push(lua, ChannelCommand::Moderate { name });
            }
            Ok(())
        })?,
    )?;

    g.set(
        "ListChannels",
        lua.create_function(|lua, _ignored: MultiValue| {
            push(lua, ChannelCommand::ListAll);
            Ok(())
        })?,
    )?;

    // SetChannelPassword(name, password) (`0x4fe720`): either argument not a string is its usage
    // error (`0x4fe7ef`); the channel string refused sends nothing (`0x4fe76a`), else the packet
    // carries it and the password as typed, empty included (`0x4fe797`, `0x4fe7b0`).
    g.set(
        "SetChannelPassword",
        lua.create_function(|lua, (name, password): (Value, Value)| {
            let (Some(name), Some(password)) =
                (lua_string(lua, &name)?, lua_string(lua, &password)?)
            else {
                return Err(mlua::Error::runtime(
                    r#"Usage: SetChannelPassword("name", "password")"#,
                ));
            };
            if let Some(name) = channel_string(&name) {
                push(lua, ChannelCommand::SetPassword { name, password });
            }
            Ok(())
        })?,
    )?;

    // The channel-and-player verbs (`0x4fe330`): either argument not a string is the
    // `Usage: %s("channel", "name")` error (`0x4fe426`); the channel string refused sends nothing
    // (`0x4fe37c`); a player name over 0x30 bytes is the `Name too long` error (`0x4fe397`,
    // `0x4fe39c`); else the packet carries both, an empty name included (`0x4fe3d6`, `0x4fe3e2`).
    for (verb, make) in [
        (
            "SetChannelOwner",
            (|name, player| ChannelCommand::SetOwner { name, player })
                as fn(String, String) -> ChannelCommand,
        ),
        ("ChannelBan", |name, player| ChannelCommand::Ban {
            name,
            player,
        }),
        ("ChannelInvite", |name, player| ChannelCommand::Invite {
            name,
            player,
        }),
        ("ChannelKick", |name, player| ChannelCommand::Kick {
            name,
            player,
        }),
        ("ChannelModerator", |name, player| {
            ChannelCommand::Moderator { name, player }
        }),
        ("ChannelUnmoderator", |name, player| {
            ChannelCommand::Unmoderator { name, player }
        }),
        ("ChannelMute", |name, player| ChannelCommand::Mute {
            name,
            player,
        }),
        ("ChannelUnmute", |name, player| ChannelCommand::Unmute {
            name,
            player,
        }),
        ("ChannelUnban", |name, player| ChannelCommand::Unban {
            name,
            player,
        }),
    ] {
        g.set(
            verb,
            lua.create_function(move |lua, (name, player): (Value, Value)| {
                let (Some(name), Some(player)) =
                    (lua_string(lua, &name)?, lua_string(lua, &player)?)
                else {
                    return Err(mlua::Error::runtime(format!(
                        "Usage: {verb}(\"channel\", \"name\")"
                    )));
                };
                let Some(name) = channel_string(&name) else {
                    return Ok(());
                };
                if player.len() > 0x30 {
                    return Err(mlua::Error::runtime("Name too long"));
                }
                push(lua, make(name, player));
                Ok(())
            })?,
        )?;
    }

    Ok(())
}

/// A manual join or leave typed as the `GUILD_REQ` row's Shortcut: the latch to STANDARD.
fn manual_reset(model: &mut super::Model) {
    if model.guild_recruitment_mode != 0 {
        model.guild_recruitment_mode = 0;
        model.guild_recruitment_changed = true;
    }
}

/// The joins' shared body: `name` resolved against the zone catalog by Shortcut, the window
/// registration and the queued [`ChannelCommand::Join`], a matched row's password dropped. `None`
/// is a refusal that sends nothing: a space in the name, or a row with no zone text.
fn join_by_name(
    lua: &Lua,
    name: String,
    password: String,
    frame: Value,
    voice: bool,
    permanent: bool,
) -> mlua::Result<Option<(i64, Option<String>)>> {
    if name.contains(' ') {
        return Ok(None);
    }
    // The typed name through the profanity filter, forced on (`0x50d9c7` -> `0x509a50`): a match
    // shows its line and answers nothing (`0x50d9d8`).
    if super::calls::ask_host(lua, &super::HostAsk::ProfaneChannelName(name.clone()))
        == Some(super::HostAnswer::Declined)
    {
        return Ok(None);
    }
    let row = zone_catalog()
        .into_iter()
        .find(|r| r.shortcut.eq_ignore_ascii_case(&name));
    let (id, resolved, shortcut) = match row {
        None => (0, None, None),
        Some(ZoneChannelRow { resolved: None, .. }) => return Ok(None),
        Some(ZoneChannelRow {
            id,
            resolved,
            shortcut,
            guild_recruitment,
            ..
        }) => {
            // The verb's manual join on the `GUILD_REQ` row stores STANDARD through the mode
            // setter's join leg, which leaves nothing and signals nothing (`0x507bd9`-`0x507bea`).
            if guild_recruitment {
                manual_reset(&mut crate::script::model_mut_of(lua));
            }
            (id, resolved, Some(shortcut))
        }
    };
    // The reference takes the frame number less one (a missing one reads -1) and skips a window
    // registration for anything outside the ten chat windows.
    let window = match frame {
        Value::Integer(n) => Some(n as f64),
        Value::Number(n) => Some(n),
        _ => None,
    }
    .map(|n| n.trunc() as i64 - 1)
    .and_then(|i| usize::try_from(i).ok())
    .filter(|i| *i < 10);
    let password = if id != 0 { String::new() } else { password };
    // The join finds or claims its record and sends; a failed claim answers nothing and registers
    // no window (`0x507c51`, `0x50da57`).
    if !push(
        lua,
        ChannelCommand::Join {
            id,
            name: resolved.unwrap_or_else(|| name.clone()),
            password,
            voice,
            permanent,
        },
    ) {
        return Ok(None);
    }
    // The window registration comes after the send, held record or new, under the Shortcut, a
    // custom channel under its name (`0x507cea`-`0x507d11`).
    if let Some(i) = window {
        let listed_as = shortcut.clone().unwrap_or(name);
        crate::script::model_mut_of(lua).register_window_channel(i, listed_as, id);
    }
    Ok(Some((i64::from(id), shortcut)))
}

#[cfg(test)]
mod command_tests {
    use super::{ChannelCommand, ZoneChannelRow};
    use crate::script::UiScript;

    fn catalog() -> Vec<ZoneChannelRow> {
        vec![
            ZoneChannelRow {
                id: 1,
                shortcut: "General".into(),
                resolved: Some("General - Elwynn Forest".into()),
                listed: true,
                auto_join: false,
                guild_recruitment: false,
            },
            ZoneChannelRow {
                id: 2,
                shortcut: "Trade".into(),
                resolved: None,
                listed: false,
                auto_join: false,
                guild_recruitment: false,
            },
        ]
    }

    #[test]
    fn enumerate_server_channels_lists_the_shortcuts_the_zone_admits() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(
            s.arity("EnumerateServerChannels()").unwrap(),
            0,
            "no zone yet — 0 values"
        );
        let mut view = crate::script::live::TestView {
            zone_channels: catalog(),
            ..Default::default()
        };
        s.with_live(&mut view, |s| {
            assert_eq!(
                s.eval::<Vec<String>>("return {EnumerateServerChannels()}")
                    .unwrap(),
                vec!["General".to_string()],
                "the shortcut, never the composed name; a city row only in a city"
            );
        });
    }

    #[test]
    fn the_management_verbs_queue_in_call_order_and_drop_empty_channels() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "LeaveChannelByName('General') ListChannelByName('') ListChannels() \
             SetChannelPassword('General', '') ChannelKick('General', 'Bob') \
             ChannelKick('General', '') ChannelModerate('General') \
             ChannelToggleAnnouncements('General') SetChannelOwner('General', 'Ann')",
        )
        .unwrap();
        assert_eq!(
            s.take_channel_commands(),
            vec![
                ChannelCommand::Leave {
                    id: 0,
                    name: "General".into()
                },
                ChannelCommand::ListAll,
                ChannelCommand::SetPassword {
                    name: "General".into(),
                    password: String::new()
                },
                ChannelCommand::Kick {
                    name: "General".into(),
                    player: "Bob".into()
                },
                ChannelCommand::Kick {
                    name: "General".into(),
                    player: String::new()
                },
                ChannelCommand::Moderate {
                    name: "General".into()
                },
                ChannelCommand::ToggleAnnouncements {
                    name: "General".into()
                },
                ChannelCommand::SetOwner {
                    name: "General".into(),
                    player: "Ann".into()
                },
            ]
        );
    }

    /// `frameId - 1` indexes the ten window records; a missing, non-numeric or out-of-range frame
    /// skips the registration and nothing else.
    #[test]
    fn join_channel_by_name_registers_the_channel_in_the_named_window() {
        let mut s = UiScript::new().unwrap();
        let mut view = crate::script::live::TestView {
            zone_channels: catalog(),
            ..Default::default()
        };
        s.with_live(&mut view, |s| {
            s.run(
                "JoinChannelByName('General', nil, 1) \
                 JoinChannelByName('general', nil, 1) \
                 JoinChannelByName('MyChan', nil, 2) \
                 JoinChannelByName('Trade', nil, nil) \
                 JoinChannelByName('LocalDefense', nil, 11) \
                 JoinChannelByName('WorldDefense', nil, 0)",
            )
            .unwrap();
            let looks = s.chat_window_looks();
            assert_eq!(
                looks[0].channels,
                vec![("General".to_string(), 1)],
                "the DBC Shortcut and its id, once — the second join deduplicated by name"
            );
            assert_eq!(
                looks[1].channels,
                vec![("MyChan".to_string(), 0)],
                "a custom channel: the name and id 0"
            );
            assert!(
                looks[2..].iter().all(|l| l.channels.is_empty()),
                "nil, 11 and 0 register nowhere"
            );
            assert_eq!(
                s.take_chat_window_changes(),
                vec![0, 1],
                "…and both windows are owed a save"
            );
            // The joins went out whatever the frame argument: five, as this catalog's Trade is the
            // nil leg, which registers nothing either.
            assert_eq!(s.take_channel_commands().len(), 5);
        });
    }

    /// The manual verbs reset the guild-recruitment latch only when the typed key is the
    /// `GUILD_REQ` row's Shortcut (`0x509494`, `0x50950d`-`0x509525`; `0x507b19`,
    /// `0x507bd9`-`0x507bea`): a number naming its record, or the composed name, leaves it AUTO.
    #[test]
    fn only_the_typed_shortcut_resets_the_guild_recruitment_latch() {
        let mut s = UiScript::new().unwrap();
        let recruitment = ZoneChannelRow {
            id: 25,
            shortcut: "GuildRecruitment".into(),
            resolved: Some("GuildRecruitment - City".into()),
            listed: true,
            auto_join: false,
            guild_recruitment: true,
        };
        let mut view = crate::script::live::TestView {
            zone_channels: vec![recruitment],
            channel_slots: vec![Some(crate::script::ChannelSlotInfo {
                name: "GuildRecruitment - City".into(),
                shortcut: None,
                active: true,
                flags: 0,
                instance: 0,
            })],
            ..Default::default()
        };
        s.with_live(&mut view, |s| {
            s.set_guild_recruitment_mode(1);
            s.run("LeaveChannelByName('1') LeaveChannelByName(1)")
                .unwrap();
            assert_eq!(
                s.take_channel_commands(),
                vec![
                    ChannelCommand::Leave {
                        id: 0,
                        name: "GuildRecruitment - City".into()
                    };
                    2
                ]
            );
            assert_eq!(s.guild_recruitment_mode(), 1, "the number leg: AUTO kept");
            s.run("LeaveChannelByName('GuildRecruitment - City')")
                .unwrap();
            assert_eq!(
                s.guild_recruitment_mode(),
                1,
                "the composed name: AUTO kept"
            );
            s.run("JoinChannelByName('GuildRecruitment - City')")
                .unwrap();
            assert_eq!(
                s.guild_recruitment_mode(),
                1,
                "a custom-name join: AUTO kept"
            );
            s.run("JoinChannelByName('guildrecruitment')").unwrap();
            assert_eq!(s.guild_recruitment_mode(), 0, "the typed Shortcut's join");
            s.set_guild_recruitment_mode(1);
            s.run("LeaveChannelByName('GUILDRECRUITMENT')").unwrap();
            assert_eq!(s.guild_recruitment_mode(), 0, "the typed Shortcut's leave");
        });
    }

    /// `LeaveChannelByName`'s four legs and its raise; a Shortcut strips its entry from every
    /// window, and a number names a confirmed record (`0x4fe160`).
    #[test]
    fn leave_channel_by_name_composes_strips_and_raises_the_way_the_reference_does() {
        let mut s = UiScript::new().unwrap();
        let slot = |name: &str, active: bool| {
            Some(crate::script::ChannelSlotInfo {
                name: name.into(),
                shortcut: None,
                active,
                flags: 0,
                instance: 0,
            })
        };
        let mut view = crate::script::live::TestView {
            zone_channels: catalog(),
            channel_slots: vec![slot("Other", true), slot("Waiting", false)],
            ..Default::default()
        };
        s.with_live(&mut view, |s| {
            s.run(
                "JoinChannelByName('General', nil, 1) JoinChannelByName('MyChan', nil, 1) \
                 JoinChannelByName('General', nil, 2)",
            )
            .unwrap();
            s.take_channel_commands();
            s.take_chat_window_changes();

            s.run("LeaveChannelByName('1')").unwrap();
            assert_eq!(
                s.take_channel_commands(),
                vec![ChannelCommand::Leave {
                    id: 0,
                    name: "Other".into()
                }],
                "a number names its confirmed record"
            );
            assert!(
                s.take_chat_window_changes().is_empty(),
                "…and strips the windows by that name, which none lists"
            );
            s.run("LeaveChannelByName('2') LeaveChannelByName('3') LeaveChannelByName(-1)")
                .unwrap();
            assert!(
                s.take_channel_commands().is_empty(),
                "a pending record, no record or a negative number: nothing (`0x4fe19f`)"
            );
            s.run("LeaveChannelByName('1abc') LeaveChannelByName('4294967297')")
                .unwrap();
            assert_eq!(
                s.take_channel_commands(),
                vec![
                    ChannelCommand::Leave {
                        id: 0,
                        name: "Other".into()
                    };
                    2
                ],
                "the integer parse takes the leading digits and wraps: 2^32 + 1 reads 1"
            );
            s.run("LeaveChannelByName('+1') LeaveChannelByName('0')")
                .unwrap();
            assert_eq!(
                s.take_channel_commands(),
                vec![
                    ChannelCommand::Leave {
                        id: 0,
                        name: "+1".into()
                    },
                    ChannelCommand::Leave {
                        id: 0,
                        name: "0".into()
                    },
                ],
                "no `+`, and 0 is a name"
            );

            s.run("LeaveChannelByName('general')").unwrap();
            assert_eq!(
                s.take_channel_commands(),
                vec![ChannelCommand::Leave {
                    id: 1,
                    name: "General - Elwynn Forest".into()
                }],
                "a shortcut composes for the zone"
            );
            let looks = s.chat_window_looks();
            assert_eq!(
                looks[0].channels,
                vec![("MyChan".to_string(), 0)],
                "General stripped from window 1, MyChan kept"
            );
            assert!(
                looks[1].channels.is_empty(),
                "…and from window 2 — all ten are walked"
            );
            assert_eq!(s.take_chat_window_changes(), vec![0, 1]);

            s.run("LeaveChannelByName('Trade')").unwrap();
            assert!(
                s.take_channel_commands().is_empty(),
                "unresolvable here (no city word in this catalog): a complete no-op"
            );

            s.run("LeaveChannelByName('mychan')").unwrap();
            assert_eq!(
                s.take_channel_commands(),
                vec![ChannelCommand::Leave {
                    id: 0,
                    name: "mychan".into()
                }],
                "custom: verbatim"
            );
            assert!(
                s.chat_window_looks()[0].channels.is_empty(),
                "…and stripped verbatim, case-folded"
            );

            let e = s.run("LeaveChannelByName()").unwrap_err();
            assert!(e.to_string().contains("Usage: LeaveChannelByName"), "{e}");
            assert_eq!(s.arity("LeaveChannelByName('x')").unwrap(), 0);
        });
    }

    /// The joins: the same row and custom legs, `hasVoice` carried, a row's password
    /// dropped, a refusal answering nothing and a non-string name raising.
    #[test]
    fn the_joins_carry_voice_and_answer_nothing_on_a_refusal() {
        let mut s = UiScript::new().unwrap();
        let mut view = crate::script::live::TestView {
            zone_channels: catalog(),
            ..Default::default()
        };
        s.with_live(&mut view, |s| {
            s.run(
                "A = {JoinPermanentChannel('General', 'pw', 1, 1)} \
                 B = {JoinTemporaryChannel('MyChan', 'secret')} \
                 N = select('#', JoinChannelByName('two words'))",
            )
            .unwrap();
            assert_eq!(
                s.eval::<(i64, String)>("return A[1], A[2]").unwrap(),
                (1, "General".to_string()),
                "the row's Shortcut, written out at `0x507ba0` and pushed at `0x50da72`"
            );
            assert!(s.eval::<bool>("return B[1] == 0").unwrap());
            assert_eq!(s.eval::<i64>("return N").unwrap(), 0);
            assert!(s.run("JoinPermanentChannel(nil)").is_err());
            assert_eq!(
                s.take_channel_commands(),
                vec![
                    ChannelCommand::Join {
                        id: 1,
                        name: "General - Elwynn Forest".into(),
                        password: String::new(),
                        voice: true,
                        permanent: true,
                    },
                    ChannelCommand::Join {
                        id: 0,
                        name: "MyChan".into(),
                        password: "secret".into(),
                        voice: false,
                        permanent: false,
                    },
                ]
            );
        });
    }

    /// The channel verbs take their channel through the channel string (`0x4fe160`): a number
    /// names the record whose own number it is in state 0 and the packet carries its name
    /// (`0x4fe197`, `0x4fe19f`), any other number or an empty name sends nothing. A non-string
    /// argument is the usage error (`0x4fe47f`, `0x4fe426`, `0x4fe7ef`), a player name over 0x30
    /// bytes `Name too long` (`0x4fe397`), and an empty one is sent (`0x4fe3e2`).
    #[test]
    fn the_channel_verbs_read_the_records_at_the_call() {
        let mut s = UiScript::new().unwrap();
        let slot = |name: &str, active: bool| {
            Some(crate::script::ChannelSlotInfo {
                name: name.into(),
                shortcut: None,
                active,
                flags: 0,
                instance: 0,
            })
        };
        let mut view = crate::script::live::TestView {
            zone_channels: catalog(),
            channel_slots: vec![
                slot("General - Elwynn Forest", true),
                slot("Waiting", false),
            ],
            ..Default::default()
        };
        s.with_live(&mut view, |s| {
            s.run(
                "ChannelKick('1', 'Bob') ChannelKick(1, '') SetChannelPassword('1', 'pw')                  ListChannelByName(1) DisplayChannelOwner('1') ChannelToggleAnnouncements('1')                  DeclineInvite('1') ChannelBan('Mine', 'Ann')",
            )
            .unwrap();
            let general = || "General - Elwynn Forest".to_string();
            assert_eq!(
                s.take_channel_commands(),
                vec![
                    ChannelCommand::Kick {
                        name: general(),
                        player: "Bob".into()
                    },
                    ChannelCommand::Kick {
                        name: general(),
                        player: String::new()
                    },
                    ChannelCommand::SetPassword {
                        name: general(),
                        password: "pw".into()
                    },
                    ChannelCommand::List { name: general() },
                    ChannelCommand::DisplayOwner { name: general() },
                    ChannelCommand::ToggleAnnouncements { name: general() },
                    ChannelCommand::DeclineInvite { name: general() },
                    ChannelCommand::Ban {
                        name: "Mine".into(),
                        player: "Ann".into()
                    },
                ],
                "a number sends its record's name, a name passes as typed"
            );
            s.run(
                "ChannelKick('2', 'Bob') ChannelKick('3', 'Bob') ChannelKick('', 'Bob')                  DeclineInvite('2') ListChannelByName('') SetChannelPassword('7', 'pw')                  ChannelKick('2', string.rep('x', 49))",
            )
            .unwrap();
            assert!(
                s.take_channel_commands().is_empty(),
                "a pending record, no record or an empty name: nothing, ahead of the name check"
            );
            for (call, usage) in [
                ("ChannelKick('General')", r#"Usage: ChannelKick("channel", "name")"#),
                ("ChannelKick(nil, 'Bob')", r#"Usage: ChannelKick("channel", "name")"#),
                ("DeclineInvite()", r#"Usage: DeclineInvite("channel")"#),
                ("ListChannelByName({})", r#"Usage: ListChannelByName("channel")"#),
                (
                    "SetChannelPassword('General')",
                    r#"Usage: SetChannelPassword("name", "password")"#,
                ),
                ("ChannelKick('General', string.rep('x', 49))", "Name too long"),
            ] {
                let e = s.run(call).unwrap_err();
                assert!(e.to_string().contains(usage), "{call}: {e}");
            }
            s.run("ChannelKick('General', string.rep('x', 48))").unwrap();
            assert_eq!(s.take_channel_commands().len(), 1, "0x30 bytes is in range");
        });
    }
}
