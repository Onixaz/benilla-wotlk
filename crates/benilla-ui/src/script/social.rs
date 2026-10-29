//! The social API: friends, ignores, voice mutes and `/who` (the friend list's script table,
//! registrar `0xad93d8`). The app pushes a display-ready [`SocialState`] (the reference resolves
//! names, class and zone engine-side too) and drains the queued [`SocialRequest`]s. Selection is
//! engine state in the reference (a guid per list, read back through `0x6b35e0`), so a set changes
//! the snapshot at once and queues the change for the app.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{bool_or_default, number_arg, optional_string, string_arg};
use super::get_text::global_text;
use super::who_sort::WhoSortChain;

/// One friend row as `GetFriendInfo` (`0x6b4130`) answers it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FriendInfo {
    /// Not a Lua return: the Recruit-a-Friend check's friend leg finds the friend by guid
    /// (`0x6b3510`).
    pub guid: u64,
    /// From the name cache; `None` while the name query is in flight, which answers nil.
    pub name: Option<String>,
    pub level: u32,
    /// The class name in the friend's gender; `None` with no `ChrClasses` row, which answers
    /// `UNKNOWN`.
    pub class: Option<String>,
    /// The zone's name, its parent zone's for a subzone; `None` with no row answers `UNKNOWN`.
    pub area: Option<String>,
    pub connected: bool,
    /// The away tag: `""`, `CHAT_FLAG_AFK` or `CHAT_FLAG_DND`'s text, DND first.
    pub status: String,
    /// The note; `None` for an empty one, which answers nil.
    pub note: Option<String>,
    /// Recruit-A-Friend (presence bit 0x8), the eighth return.
    pub raf: bool,
}

/// One `/who` row in `GetWhoInfo`'s order (`0x6b4a80`), names resolved; race comes before class,
/// where the wire has class first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WhoInfo {
    pub name: String,
    pub guild: String,
    pub level: u32,
    /// The race name in the row's gender (`0x715970`), `UNKNOWN`'s text with no `ChrRaces` row.
    pub race: String,
    /// The class name in the row's gender (`0x7159e0`), `UNKNOWN`'s text with no `ChrClasses` row.
    pub class: String,
    /// The `AreaTable` name, `UNKNOWN`'s text with no row.
    pub zone: String,
    /// `ChrClasses` `Filename` (`"WARRIOR"`), the seventh return; `None` with no row.
    pub class_file: Option<String>,
    /// The record names the sort orders by.
    pub records: WhoRecords,
}

/// The `Name_lang` of the record each id names, `None` with no row: the comparator's class, race
/// and zone arms order by these (`0x6b4d17`, `0x6b4d7e`, `0x6b4de2`), not by the gendered names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WhoRecords {
    pub race: Option<String>,
    pub class: Option<String>,
    pub zone: Option<String>,
}

/// The social snapshot the app pushes whole; the default is a fresh login's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SocialState {
    /// In display order: online first, then by name (`0x6b3960`).
    pub friends: Vec<FriendInfo>,
    /// The selected friend, 1-based, 0 for none (`GetSelectedFriend` `0x6b44b0`).
    pub selected_friend: u32,
    /// The ignored players' names, sorted (`0x6b3a60`); empty while a name query is out, which
    /// `GetIgnoreName` answers as `UNKNOWN`.
    pub ignores: Vec<String>,
    /// The selected ignore, on the same scale.
    pub selected_ignore: u32,
    /// The voice-muted players' names, as [`Self::ignores`].
    pub mutes: Vec<String>,
    pub selected_mute: u32,
    /// The last `/who` answer's rows, at most 50.
    pub who: Vec<WhoInfo>,
    /// The total match count, `GetNumWhoResults`'s second return; it can exceed `who.len()`.
    pub who_total: u32,
    /// The `/who` sort chain, pushed so `SortWho` can re-sort before its synchronous redraw.
    pub who_sort: WhoSortChain,
}

/// Outbound social intents, drained by the app. A row is 1-based in the order last pushed; the
/// app turns it into the guid the wire takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SocialRequest {
    /// `ShowFriends()`: `CMSG_CONTACT_LIST` for the friend list.
    RefreshFriends,
    /// `AddFriend(name[, note])`, or `AddOrRemoveFriend` for someone not listed.
    AddFriend { name: String, note: String },
    /// `RemoveFriend` by row or by name, or `AddOrRemoveFriend` for someone listed.
    RemoveFriend(u32),
    /// `SetFriendNotes`: the note, already stored and announced in the VM.
    SetFriendNotes { row: u32, note: String },
    /// `AddIgnore(name)`, or `AddOrDelIgnore` for someone not listed.
    AddIgnore(String),
    /// `DelIgnore(name)`, or `AddOrDelIgnore` for someone listed.
    DelIgnore(u32),
    /// `AddMute(name)`, or `AddOrDelMute` for someone not listed.
    AddMute(String),
    /// `DelMute(name)`, or `AddOrDelMute` for someone listed.
    DelMute(u32),
    /// `SetSelectedFriend(index)`, mirrored so the next push agrees.
    SelectFriend(u32),
    /// `SetSelectedIgnore(index)`.
    SelectIgnore(u32),
    /// `SetSelectedMute(index)`.
    SelectMute(u32),
    /// `SendWho(filter)`, the raw filter; parsing it needs the DBCs, so the app does it.
    Who(String),
    /// `SortWho(sortType)`, the raw argument; the binding has already sorted the snapshot.
    SortWho(String),
    /// `SetWhoToUI(flag)`: the next `/who` answer goes to the Who frame (true) or chat (false).
    SetWhoToUi(bool),
}

impl super::UiScript {
    /// Replace the social snapshot; firing `FRIENDLIST_UPDATE` and kin is the app's job.
    pub fn set_social(&mut self, state: SocialState) {
        set_field!(self, social, state);
    }

    /// Drain the social intents queued since the last call.
    pub fn take_social_requests(&mut self) -> Vec<SocialRequest> {
        drain_field!(self, social_requests)
    }
}

/// The friend array's slots (the friend list's `+0x0`, 100 entries).
const MAX_FRIENDS: usize = 100;
/// The ignore and mute arrays' slots (`+0xd490`, `+0xd7c0`, 50 each).
const MAX_IGNORES: usize = 50;

/// `(uint32)lua_tonumber(L, 1) - 1`: the 0-based slot of a 1-based Lua index; 0, a negative and a
/// fraction below 1 wrap past every list.
fn slot(n: f64) -> Option<usize> {
    let n = n.trunc();
    (n >= 1.0 && n <= u32::MAX as f64).then(|| n as usize - 1)
}

/// A name match as the reference makes it: a case-insensitive UTF-8 compare over the whole name.
fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// The first argument as `lua_isnumber` reads it, a numeric string included.
fn number_of(lua: &Lua, v: &Value) -> Option<f64> {
    lua.coerce_number(v.clone()).ok().flatten()
}

/// A list row's name as the `Get*Name` pair answers it (`0x6b4620`, `0x6b4760`): the cached name,
/// else `UNKNOWN`, for a slot past the list too.
fn listed_name(
    lua: &Lua,
    names: &[String],
    index: Value,
    usage: &'static str,
) -> mlua::Result<String> {
    let index = number_arg(lua, index, usage)?;
    let name = slot(f64::from(index))
        .filter(|&i| i <= MAX_IGNORES)
        .and_then(|i| names.get(i))
        .filter(|n| !n.is_empty());
    Ok(match name {
        Some(name) => name.clone(),
        None => global_text(lua, "UNKNOWN"),
    })
}

/// The 1-based row of the player named `name` in an ignore or mute list.
fn row_named(names: &[String], name: &str) -> Option<u32> {
    names
        .iter()
        .position(|n| !n.is_empty() && same_name(n, name))
        .map(|i| i as u32 + 1)
}

/// The 1-based row of the friend named `name` (`0x6b3570`).
fn friend_named(friends: &[FriendInfo], name: &str) -> Option<u32> {
    friends
        .iter()
        .position(|f| f.name.as_deref().is_some_and(|n| same_name(n, name)))
        .map(|i| i as u32 + 1)
}

/// Is the player a name or unit token names on `names`? The reference's token-to-guid lookup tries
/// the argument as a cached name, then as a unit token.
fn token_listed(model: &crate::script::Model, names: &[String], token: &str) -> bool {
    let listed = |name: &str| row_named(names, name).is_some();
    listed(token)
        || model
            .unit(token)
            .and_then(|u| u.name.as_deref())
            .is_some_and(listed)
}

/// The `Is*` trio's shape (`0x6b6c60`, `0x6b6cf0`, `0x6b6d80`): 1 when listed, else nil, always
/// one value.
fn token_verb(
    lua: &Lua,
    token: Value,
    lists: fn(&SocialState) -> [&[String]; 2],
) -> mlua::Result<Value> {
    let Some(token) = optional_string(lua, &token) else {
        return Ok(Value::Nil);
    };
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    let listed = lists(&model.social)
        .iter()
        .any(|names| token_listed(&model, names, &token));
    Ok(super::binding_abi::flag(listed))
}

/// Register the social globals against the snapshot store.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ── The friend list ──
    // GetNumFriends() (`0x6b4060`): the friend count, then how many are online.
    g.set(
        "GetNumFriends",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let friends = &model.social.friends;
            let online = friends.iter().filter(|f| f.connected).count();
            Ok((friends.len() as i64, online as i64))
        })?,
    )?;

    // GetFriendInfo(index or name) (`0x6b4130`): eight values; an index (a numeric string too)
    // past the list or an unlisted name answers nil, 0, UNKNOWN, UNKNOWN, nil, "", nil, nil.
    g.set(
        "GetFriendInfo",
        lua.create_function(|lua, who: Value| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let friends = &model.social.friends;
            let friend = if let Some(n) = number_of(lua, &who) {
                slot(n)
                    .filter(|&i| i <= MAX_FRIENDS)
                    .and_then(|i| friends.get(i))
            } else if let Value::String(name) = &who {
                friend_named(friends, &name.to_string_lossy())
                    .and_then(|row| friends.get(row as usize - 1))
            } else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: GetFriendInfo(index or name)".into(),
                ));
            };
            let text = |s: &str| lua.create_string(s).map(Value::String);
            let unknown = global_text(lua, "UNKNOWN");
            let Some(friend) = friend else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Integer(0),
                    text(&unknown)?,
                    text(&unknown)?,
                    Value::Nil,
                    text("")?,
                    Value::Nil,
                    Value::Nil,
                ]));
            };
            let optional = |s: &Option<String>| match s {
                Some(s) => text(s),
                None => Ok(Value::Nil),
            };
            Ok(MultiValue::from_vec(vec![
                optional(&friend.name)?,
                Value::Integer(i64::from(friend.level)),
                text(friend.class.as_deref().unwrap_or(&unknown))?,
                text(friend.area.as_deref().unwrap_or(&unknown))?,
                super::binding_abi::flag(friend.connected),
                text(&friend.status)?,
                optional(&friend.note)?,
                super::binding_abi::flag(friend.raf),
            ]))
        })?,
    )?;

    g.set(
        "GetSelectedFriend",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(model.social.selected_friend))
        })?,
    )?;

    // SetSelectedFriend(index) (`0x6b4400`): set now, since `FriendsList_Update` reads it back in
    // the same call, and queue it for the app; an empty slot selects nobody.
    g.set(
        "SetSelectedFriend",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: SetSelectedFriend(index)")?;
            let mut model = crate::script::model_mut_of(lua);
            let index = clamp_index(index, model.social.friends.len());
            model.social.selected_friend = index;
            model
                .social_requests
                .push(SocialRequest::SelectFriend(index));
            Ok(())
        })?,
    )?;

    g.set(
        "ShowFriends",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.social_requests.push(SocialRequest::RefreshFriends);
            Ok(())
        })?,
    )?;

    // AddFriend(name[, note]) (`0x6b6af0`): sent as given, a blank name too; the note is read only
    // when it passes `lua_isstring`.
    g.set(
        "AddFriend",
        lua.create_function(|lua, (name, note): (Value, Value)| {
            if let Some(name) = optional_string(lua, &name) {
                let note = optional_string(lua, &note).unwrap_or_default();
                crate::script::model_mut_of(lua)
                    .social_requests
                    .push(SocialRequest::AddFriend { name, note });
            }
            Ok(())
        })?,
    )?;

    // AddOrRemoveFriend(name[, note]) (`0x6b6e10`): removes a listed friend, else adds them.
    g.set(
        "AddOrRemoveFriend",
        lua.create_function(|lua, (name, note): (Value, Value)| {
            let Some(name) = optional_string(lua, &name) else {
                return Ok(());
            };
            let note = optional_string(lua, &note).unwrap_or_default();
            let mut model = crate::script::model_mut_of(lua);
            let request = match friend_named(&model.social.friends, &name) {
                Some(row) => SocialRequest::RemoveFriend(row),
                None => SocialRequest::AddFriend { name, note },
            };
            model.social_requests.push(request);
            Ok(())
        })?,
    )?;

    // RemoveFriend(index or name) (`0x6b7bd0`): a row in the list, else nothing; an unlisted name
    // prints `ERR_FRIEND_NOT_FOUND` locally (`0x6b6f00`).
    g.set(
        "RemoveFriend",
        lua.create_function(|lua, who: Value| {
            let mut model = crate::script::model_mut_of(lua);
            if let Some(n) = number_of(lua, &who) {
                let row = slot(n).filter(|&i| i < model.social.friends.len());
                if let Some(i) = row {
                    model
                        .social_requests
                        .push(SocialRequest::RemoveFriend(i as u32 + 1));
                }
                return Ok(());
            }
            let Value::String(name) = &who else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: RemoveFriend([\"name\"] or [index])".into(),
                ));
            };
            match friend_named(&model.social.friends, &name.to_string_lossy()) {
                Some(row) => model.social_requests.push(SocialRequest::RemoveFriend(row)),
                None => model.ui_errors.push("ERR_FRIEND_NOT_FOUND"),
            }
            Ok(())
        })?,
    )?;

    // SetFriendNotes(index or name, notes) (`0x6b7c90`): the note write (`0x6b5540`) stores the
    // note (511 bytes), fires `FRIENDLIST_UPDATE` at once, then sends it; an unlisted name prints
    // `ERR_FRIEND_NOT_FOUND`.
    g.set(
        "SetFriendNotes",
        lua.create_function(|lua, (who, notes): (Value, Value)| {
            const USAGE: &str = "Usage: SetFriendNotes([\"name\"] or [index], [\"notes\"])";
            let notes = string_arg(lua, notes, USAGE)?;
            {
                let mut model = crate::script::model_mut_of(lua);
                let row = if let Some(n) = number_of(lua, &who) {
                    slot(n)
                        .filter(|&i| i < model.social.friends.len())
                        .map(|i| i as u32 + 1)
                } else if let Value::String(name) = &who {
                    let row = friend_named(&model.social.friends, &name.to_string_lossy());
                    if row.is_none() {
                        model.ui_errors.push("ERR_FRIEND_NOT_FOUND");
                    }
                    row
                } else {
                    return Err(mlua::Error::RuntimeError(USAGE.into()));
                };
                let Some(row) = row else {
                    return Ok(());
                };
                let note = truncate_bytes(&notes, 511).to_string();
                model.social.friends[row as usize - 1].note =
                    (!note.is_empty()).then(|| note.clone());
                model
                    .social_requests
                    .push(SocialRequest::SetFriendNotes { row, note });
            }
            super::tick::fire_event_into(lua, "FRIENDLIST_UPDATE", Vec::new());
            Ok(())
        })?,
    )?;

    // ── The ignore and mute lists ──
    // GetNumIgnores() / GetNumMutes() (`0x6b4560`, `0x6b45c0`).
    g.set(
        "GetNumIgnores",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.social.ignores.len() as i64)
        })?,
    )?;
    g.set(
        "GetNumMutes",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.social.mutes.len() as i64)
        })?,
    )?;

    g.set(
        "GetIgnoreName",
        lua.create_function(|lua, index: Value| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            listed_name(
                lua,
                &model.social.ignores,
                index,
                "Usage: GetIgnoreName(index)",
            )
        })?,
    )?;
    g.set(
        "GetMuteName",
        lua.create_function(|lua, index: Value| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            listed_name(lua, &model.social.mutes, index, "Usage: GetMuteName(index)")
        })?,
    )?;

    // GetMuteStatus(nameOrUnit[, channel]) (`0x4fcdf0`): 1 for a player on the ignore or mute list
    // (`0x4fce5b`), the argument read as a cached name, else a unit token; no value, not an error,
    // without a string (`0x4fce0d`). A voice-channel squelch also answers 1, and benilla joins no
    // voice channel.
    g.set(
        "GetMuteStatus",
        lua.create_function(|lua, args: MultiValue| {
            let arg = match args.into_iter().next() {
                Some(v @ (Value::String(_) | Value::Integer(_) | Value::Number(_))) => v,
                _ => return Ok(MultiValue::new()),
            };
            let listed = token_verb(lua, arg, |s| [&s.ignores, &s.mutes])?;
            Ok(MultiValue::from_vec(vec![listed]))
        })?,
    )?;

    // IsIgnored / IsMuted / IsIgnoredOrMuted(token) (`0x6b6c60`, `0x6b6cf0`, `0x6b6d80`).
    g.set(
        "IsIgnored",
        lua.create_function(|lua, token: Value| token_verb(lua, token, |s| [&s.ignores, &[]]))?,
    )?;
    g.set(
        "IsMuted",
        lua.create_function(|lua, token: Value| token_verb(lua, token, |s| [&s.mutes, &[]]))?,
    )?;
    g.set(
        "IsIgnoredOrMuted",
        lua.create_function(|lua, token: Value| {
            token_verb(lua, token, |s| [&s.ignores, &s.mutes])
        })?,
    )?;

    // UnitIsTalking(nameOrUnit) (`0x7df0b0`): 1 for a player speaking in a voice channel; nil, as
    // benilla joins none, and no value without a string.
    g.set(
        "UnitIsTalking",
        lua.create_function(|_, args: MultiValue| {
            Ok(match args.into_iter().next() {
                Some(Value::String(_) | Value::Integer(_) | Value::Number(_)) => {
                    MultiValue::from_vec(vec![Value::Nil])
                }
                _ => MultiValue::new(),
            })
        })?,
    )?;

    // The selection pairs (`0x6b4a00`/`0x6b48a0`, `0x6b4a40`/`0x6b4950`), as the friend pair.
    g.set(
        "GetSelectedIgnore",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(model.social.selected_ignore))
        })?,
    )?;
    g.set(
        "SetSelectedIgnore",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: SetSelectedIgnore(index)")?;
            let mut model = crate::script::model_mut_of(lua);
            let index = clamp_index(index, model.social.ignores.len());
            model.social.selected_ignore = index;
            model
                .social_requests
                .push(SocialRequest::SelectIgnore(index));
            Ok(())
        })?,
    )?;
    g.set(
        "GetSelectedMute",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(model.social.selected_mute))
        })?,
    )?;
    g.set(
        "SetSelectedMute",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: SetSelectedMute(index)")?;
            let mut model = crate::script::model_mut_of(lua);
            let index = clamp_index(index, model.social.mutes.len());
            model.social.selected_mute = index;
            model.social_requests.push(SocialRequest::SelectMute(index));
            Ok(())
        })?,
    )?;

    // AddIgnore(name) (`0x6b5f70`): someone listed prints `ERR_IGNORE_ALREADY_S` locally, anyone
    // else is sent. AddMute (`0x6b62c0`) is its voice twin.
    g.set(
        "AddIgnore",
        lua.create_function(|lua, name: Value| {
            add_listed(
                lua,
                name,
                |s| &s.ignores,
                "ERR_IGNORE_ALREADY_S",
                SocialRequest::AddIgnore,
            )
        })?,
    )?;
    g.set(
        "AddMute",
        lua.create_function(|lua, name: Value| {
            add_listed(
                lua,
                name,
                |s| &s.mutes,
                "ERR_VOICE_IGNORE_ALREADY_S",
                SocialRequest::AddMute,
            )
        })?,
    )?;

    // AddOrDelIgnore(name) (`0x6b5de0`): removes someone listed, else sends the add.
    // AddOrDelMute (`0x6b6130`) is its voice twin.
    g.set(
        "AddOrDelIgnore",
        lua.create_function(|lua, name: Value| {
            toggle_listed(
                lua,
                name,
                |s| &s.ignores,
                SocialRequest::DelIgnore,
                SocialRequest::AddIgnore,
            )
        })?,
    )?;
    g.set(
        "AddOrDelMute",
        lua.create_function(|lua, name: Value| {
            toggle_listed(
                lua,
                name,
                |s| &s.mutes,
                SocialRequest::DelMute,
                SocialRequest::AddMute,
            )
        })?,
    )?;

    // DelIgnore(name) (`0x6b7040`): someone unlisted prints `ERR_IGNORE_NOT_FOUND`. DelMute
    // (`0x6b7120`) is its voice twin.
    g.set(
        "DelIgnore",
        lua.create_function(|lua, name: Value| {
            del_listed(
                lua,
                name,
                |s| &s.ignores,
                "ERR_IGNORE_NOT_FOUND",
                SocialRequest::DelIgnore,
            )
        })?,
    )?;
    g.set(
        "DelMute",
        lua.create_function(|lua, name: Value| {
            del_listed(
                lua,
                name,
                |s| &s.mutes,
                "ERR_VOICE_IGNORE_NOT_FOUND",
                SocialRequest::DelMute,
            )
        })?,
    )?;

    // ── /who ──
    // GetNumWhoResults() (`0x6b33d0`) → the kept rows, then the server's full match count.
    g.set(
        "GetNumWhoResults",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok((
                model.social.who.len() as i64,
                i64::from(model.social.who_total),
            ))
        })?,
    )?;

    g.set(
        "GetWhoInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetWhoInfo(index)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let row = usize::try_from(i64::from(index) - 1)
                .ok()
                .and_then(|i| model.social.who.get(i));
            // Past the list, seven values with a level of 0 (`0x6b4c40`).
            let Some(row) = row else {
                let mut values = vec![Value::Nil; 7];
                values[2] = Value::Integer(0);
                return Ok(MultiValue::from_vec(values));
            };
            let text = |s: &str| lua.create_string(s).map(Value::String);
            Ok(MultiValue::from_vec(vec![
                text(&row.name)?,
                text(&row.guild)?,
                Value::Integer(i64::from(row.level)),
                text(&row.race)?,
                text(&row.class)?,
                text(&row.zone)?,
                match &row.class_file {
                    Some(file) => text(file)?,
                    None => Value::Nil,
                },
            ]))
        })?,
    )?;

    g.set(
        "SendWho",
        lua.create_function(|lua, filter: Value| {
            // `lua_isstring` or the usage error (`0x6b6b5a`).
            let filter = string_arg(lua, filter, "Usage: SendWho(\"filter\")")?;
            let mut model = crate::script::model_mut_of(lua);
            model.social_requests.push(SocialRequest::Who(filter));
            Ok(())
        })?,
    )?;

    // SortWho(sortType) (`0x6b4eb0`): promotes the key in the seven-slot chain, reversing it if
    // it is already in front, sorts the list, and fires `WHO_LIST_UPDATE` synchronously
    // (`0x6b5017`), so the list redraws before the click's sound. The app gets the same click for
    // its own chain. Returns nothing; an argument neither string nor number raises (`0x6b4eba`).
    g.set(
        "SortWho",
        lua.create_function(|lua, sort_type: Value| {
            let sort_type = string_arg(lua, sort_type, "Usage: SortWho(\"type\")")?;
            {
                let mut model = crate::script::model_mut_of(lua);
                let model: &mut crate::script::Model = &mut model;
                model.social.who_sort.promote(&sort_type);
                model.social.who_sort.sort(&mut model.social.who);
                model
                    .social_requests
                    .push(SocialRequest::SortWho(sort_type));
            }
            super::tick::fire_event_into(lua, "WHO_LIST_UPDATE", Vec::new());
            Ok(())
        })?,
    )?;

    // SetWhoToUI(flag) (`0x6b3430`): the Who frame passes 1 on show and 0 on hide; the flag is the
    // bool-or-default read with default 0.
    g.set(
        "SetWhoToUI",
        lua.create_function(|lua, flag: Value| {
            let on = bool_or_default(Some(&flag), false);
            let mut model = crate::script::model_mut_of(lua);
            model.social_requests.push(SocialRequest::SetWhoToUi(on));
            Ok(())
        })?,
    )?;

    Ok(())
}

/// `AddIgnore`/`AddMute`: a listed name prints `already` with the listed spelling, else the add
/// queues; a non-string sends nothing.
fn add_listed(
    lua: &Lua,
    name: Value,
    list: fn(&SocialState) -> &Vec<String>,
    already: &'static str,
    add: fn(String) -> SocialRequest,
) -> mlua::Result<()> {
    let Some(name) = optional_string(lua, &name) else {
        return Ok(());
    };
    let mut model = crate::script::model_mut_of(lua);
    match row_named(list(&model.social), &name) {
        Some(row) => {
            let listed = list(&model.social)[row as usize - 1].clone();
            model.ui_errors_s.push((already, listed));
        }
        None => model.social_requests.push(add(name)),
    }
    Ok(())
}

/// `AddOrDelIgnore`/`AddOrDelMute`: the remove for a listed name, else the add.
fn toggle_listed(
    lua: &Lua,
    name: Value,
    list: fn(&SocialState) -> &Vec<String>,
    del: fn(u32) -> SocialRequest,
    add: fn(String) -> SocialRequest,
) -> mlua::Result<()> {
    let Some(name) = optional_string(lua, &name) else {
        return Ok(());
    };
    let mut model = crate::script::model_mut_of(lua);
    let request = match row_named(list(&model.social), &name) {
        Some(row) => del(row),
        None => add(name),
    };
    model.social_requests.push(request);
    Ok(())
}

/// `DelIgnore`/`DelMute`: the remove for a listed name, else `missing` prints.
fn del_listed(
    lua: &Lua,
    name: Value,
    list: fn(&SocialState) -> &Vec<String>,
    missing: &'static str,
    del: fn(u32) -> SocialRequest,
) -> mlua::Result<()> {
    let Some(name) = optional_string(lua, &name) else {
        return Ok(());
    };
    let mut model = crate::script::model_mut_of(lua);
    match row_named(list(&model.social), &name) {
        Some(row) => model.social_requests.push(del(row)),
        None => model.ui_errors.push(missing),
    }
    Ok(())
}

/// A selection index in `1..=len`, else 0, nothing selected.
fn clamp_index(index: i32, len: usize) -> u32 {
    if index >= 1 && index as usize <= len {
        index as u32
    } else {
        0
    }
}

/// A string copy into a buffer of `max + 1`: at most `max` bytes, cut on a character boundary.
fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::{FriendInfo, SocialRequest, SocialState, WhoInfo};
    use crate::script::UiScript;

    fn who(name: &str, level: u32, zone: &str) -> WhoInfo {
        WhoInfo {
            name: name.to_string(),
            level,
            race: "Human".to_string(),
            class: "Warrior".to_string(),
            zone: zone.to_string(),
            class_file: Some("WARRIOR".to_string()),
            ..Default::default()
        }
    }

    /// Two `/who` hits and a `WHO_LIST_UPDATE` handler that records the order it sees.
    fn seated() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"
            fired = 0
            order = ""
            local f = CreateFrame("Frame", "WhoWatcher")
            f:RegisterEvent("WHO_LIST_UPDATE")
            f:SetScript("OnEvent", function()
                fired = fired + 1
                order = ""
                for i = 1, GetNumWhoResults() do
                    order = order .. GetWhoInfo(i) .. ","
                end
            end)
            "#,
        )
        .unwrap();
        s.set_social(SocialState {
            who: vec![
                who("Galas", 60, "Elwynn Forest"),
                who("Erdrin", 12, "Elwynn Forest"),
            ],
            who_total: 2,
            ..Default::default()
        });
        s
    }

    fn friend(name: &str, level: u32, connected: bool) -> FriendInfo {
        FriendInfo {
            name: Some(name.to_string()),
            level,
            class: Some("Priest".to_string()),
            area: Some("Dalaran".to_string()),
            connected,
            ..Default::default()
        }
    }

    /// Two friends, one ignore and one mute, with `UNKNOWN` resolvable as the shipped string.
    fn contacts() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.run(r#"UNKNOWN = "Unknown""#).unwrap();
        s.set_social(SocialState {
            friends: vec![
                FriendInfo {
                    status: "<AFK>".into(),
                    note: Some("healer".into()),
                    raf: true,
                    ..friend("Galas", 80, true)
                },
                FriendInfo {
                    class: None,
                    area: None,
                    ..friend("Erdrin", 0, false)
                },
            ],
            ignores: vec!["Spammer".into()],
            mutes: vec!["Loud".into()],
            ..Default::default()
        });
        s
    }

    /// Eight values (`0x6b4130`); class and zone with no row read `UNKNOWN`, an empty note nil.
    #[test]
    fn get_friend_info_answers_eight_values_by_index_or_name() {
        let s = contacts();
        assert_eq!(s.arity("GetFriendInfo(1)").unwrap(), 8);
        assert_eq!(
            s.eval::<(String, i64, String, String, i64, String, String, i64)>(
                "return GetFriendInfo(1)"
            )
            .unwrap(),
            (
                "Galas".into(),
                80,
                "Priest".into(),
                "Dalaran".into(),
                1,
                "<AFK>".into(),
                "healer".into(),
                1
            )
        );
        assert!(s
            .eval::<bool>(
                "local n, l, c, a, on, st, note, raf = GetFriendInfo('erdrin') \
                 return n == 'Erdrin' and l == 0 and c == 'Unknown' and a == 'Unknown' \
                 and on == nil and st == '' and note == nil and raf == nil"
            )
            .unwrap());
        assert_eq!(
            s.eval::<String>("return (GetFriendInfo('2'))").unwrap(),
            "Erdrin",
            "a numeric string is an index"
        );
        for miss in ["0", "3", "-1", "'Nobody'"] {
            assert!(
                s.eval::<bool>(&format!(
                    "local n, l, c, a, on, st, note, raf = GetFriendInfo({miss}) \
                     return select('#', GetFriendInfo({miss})) == 8 and n == nil and l == 0 \
                     and c == 'Unknown' and a == 'Unknown' and on == nil and st == '' \
                     and note == nil and raf == nil"
                ))
                .unwrap(),
                "{miss}"
            );
        }
        let err = s.run("GetFriendInfo()").unwrap_err().to_string();
        assert!(err.contains("Usage: GetFriendInfo(index or name)"), "{err}");
    }

    #[test]
    fn get_num_friends_counts_the_online_ones_too() {
        let s = contacts();
        assert_eq!(
            s.eval::<(i64, i64)>("return GetNumFriends()").unwrap(),
            (2, 1)
        );
    }

    /// `RemoveFriend` (`0x6b7bd0`): a row or a listed name queues the row; an unlisted name prints
    /// `ERR_FRIEND_NOT_FOUND`; a row past the list does nothing.
    #[test]
    fn remove_friend_takes_a_row_or_a_name() {
        let mut s = contacts();
        s.run("RemoveFriend(2) RemoveFriend('GALAS') RemoveFriend(3) RemoveFriend('Nobody')")
            .unwrap();
        assert_eq!(
            s.take_social_requests(),
            vec![
                SocialRequest::RemoveFriend(2),
                SocialRequest::RemoveFriend(1)
            ]
        );
        assert_eq!(s.take_ui_errors(), vec!["ERR_FRIEND_NOT_FOUND"]);
        let err = s.run("RemoveFriend()").unwrap_err().to_string();
        assert!(
            err.contains(r#"Usage: RemoveFriend(["name"] or [index])"#),
            "{err}"
        );
    }

    /// `AddFriend` sends even a blank name with its note; `AddOrRemoveFriend` removes a listed one.
    #[test]
    fn add_friend_carries_the_note_and_add_or_remove_toggles() {
        let mut s = contacts();
        s.run("AddFriend('Bruk', 'tank') AddFriend('') AddFriend(nil) AddOrRemoveFriend('galas') AddOrRemoveFriend('Bruk', 'dps')")
            .unwrap();
        assert_eq!(
            s.take_social_requests(),
            vec![
                SocialRequest::AddFriend {
                    name: "Bruk".into(),
                    note: "tank".into()
                },
                SocialRequest::AddFriend {
                    name: String::new(),
                    note: String::new()
                },
                SocialRequest::RemoveFriend(1),
                SocialRequest::AddFriend {
                    name: "Bruk".into(),
                    note: "dps".into()
                },
            ]
        );
    }

    /// The note write (`0x6b5540`) stores the note and fires `FRIENDLIST_UPDATE` before it returns.
    #[test]
    fn set_friend_notes_stores_announces_and_queues() {
        let mut s = contacts();
        s.run(
            r#"
            seen = nil
            local f = CreateFrame("Frame")
            f:RegisterEvent("FRIENDLIST_UPDATE")
            f:SetScript("OnEvent", function() seen = select(7, GetFriendInfo(2)) end)
            SetFriendNotes(2, "met in Dalaran")
            "#,
        )
        .unwrap();
        assert_eq!(s.eval::<String>("return seen").unwrap(), "met in Dalaran");
        assert_eq!(
            s.take_social_requests(),
            vec![SocialRequest::SetFriendNotes {
                row: 2,
                note: "met in Dalaran".into()
            }]
        );
        // An empty note reads nil; an unlisted name prints the not-found line.
        s.run("SetFriendNotes('Galas', '') SetFriendNotes('Nobody', 'x')")
            .unwrap();
        assert!(s
            .eval::<bool>("return select(7, GetFriendInfo(1)) == nil")
            .unwrap());
        assert_eq!(s.take_ui_errors(), vec!["ERR_FRIEND_NOT_FOUND"]);
        let err = s.run("SetFriendNotes(1)").unwrap_err().to_string();
        assert!(err.contains("Usage: SetFriendNotes"), "{err}");
    }

    /// `GetIgnoreName` answers `UNKNOWN` past the list (`0x6b4620`); `AddIgnore` of someone listed
    /// prints `ERR_IGNORE_ALREADY_S` (`0x6b5f70`); `DelIgnore` of someone unlisted prints
    /// `ERR_IGNORE_NOT_FOUND`; the mute verbs mirror them.
    #[test]
    fn the_ignore_and_mute_verbs_check_the_list_first() {
        let mut s = contacts();
        assert_eq!(
            s.eval::<(String, String)>("return GetIgnoreName(1), GetIgnoreName(2)")
                .unwrap(),
            ("Spammer".into(), "Unknown".into())
        );
        assert_eq!(s.eval::<String>("return GetMuteName(1)").unwrap(), "Loud");
        assert_eq!(
            s.eval::<(i64, i64)>("return GetNumIgnores(), GetNumMutes()")
                .unwrap(),
            (1, 1)
        );

        s.run(
            "AddIgnore('spammer') AddIgnore('Bruk') DelIgnore('Bruk') DelIgnore('Spammer') \
             AddOrDelIgnore('SPAMMER') AddOrDelIgnore('Bruk') AddMute('Loud') DelMute('Bruk') \
             AddOrDelMute('loud')",
        )
        .unwrap();
        assert_eq!(
            s.take_social_requests(),
            vec![
                SocialRequest::AddIgnore("Bruk".into()),
                SocialRequest::DelIgnore(1),
                SocialRequest::DelIgnore(1),
                SocialRequest::AddIgnore("Bruk".into()),
                SocialRequest::DelMute(1),
            ]
        );
        assert_eq!(
            s.take_ui_errors(),
            vec!["ERR_IGNORE_NOT_FOUND", "ERR_VOICE_IGNORE_NOT_FOUND"]
        );
        assert_eq!(
            s.take_ui_errors_s(),
            vec![
                ("ERR_IGNORE_ALREADY_S", "Spammer".to_string()),
                ("ERR_VOICE_IGNORE_ALREADY_S", "Loud".to_string())
            ]
        );
        let err = s.run("GetIgnoreName()").unwrap_err().to_string();
        assert!(err.contains("Usage: GetIgnoreName(index)"), "{err}");
    }

    /// The `Is*` trio reads a name or a unit token and always answers one value.
    #[test]
    fn is_ignored_and_is_muted_read_a_name_or_a_token() {
        let mut s = contacts();
        s.set_unit(
            "party1",
            Some(crate::script::UnitState {
                exists: true,
                name: Some("Loud".into()),
                ..Default::default()
            }),
        );
        let ask =
            |s: &UiScript, call: &str| s.eval::<Option<i64>>(&format!("return {call}")).unwrap();
        assert_eq!(ask(&s, "IsIgnored('spammer')"), Some(1));
        assert_eq!(ask(&s, "IsIgnored('party1')"), None);
        assert_eq!(ask(&s, "IsMuted('party1')"), Some(1));
        assert_eq!(ask(&s, "IsIgnoredOrMuted('party1')"), Some(1));
        assert_eq!(ask(&s, "IsIgnoredOrMuted('Spammer')"), Some(1));
        assert_eq!(ask(&s, "IsIgnoredOrMuted('Galas')"), None);
        assert_eq!(s.arity("IsIgnored()").unwrap(), 1);
    }

    #[test]
    fn the_selection_setters_raise_without_a_number_and_clamp() {
        let mut s = contacts();
        s.run("SetSelectedFriend(2) SetSelectedIgnore(5) SetSelectedMute('1')")
            .unwrap();
        assert_eq!(
            s.eval::<(i64, i64, i64)>(
                "return GetSelectedFriend(), GetSelectedIgnore(), GetSelectedMute()"
            )
            .unwrap(),
            (2, 0, 1)
        );
        assert_eq!(
            s.take_social_requests(),
            vec![
                SocialRequest::SelectFriend(2),
                SocialRequest::SelectIgnore(0),
                SocialRequest::SelectMute(1)
            ]
        );
        for verb in ["SetSelectedFriend", "SetSelectedIgnore", "SetSelectedMute"] {
            let err = s.run(&format!("{verb}()")).unwrap_err().to_string();
            assert!(err.contains(&format!("Usage: {verb}(index)")), "{err}");
        }
    }

    /// `GetMuteStatus` (`0x4fcdf0`): 1 for an ignored player named directly or through a unit
    /// token, nil for anyone else, and no value at all without a string.
    #[test]
    fn get_mute_status_answers_the_ignore_list_by_name_or_token() {
        let mut s = UiScript::new().unwrap();
        s.set_social(SocialState {
            ignores: vec!["Galas".to_string()],
            ..Default::default()
        });
        s.set_unit(
            "party1",
            Some(crate::script::UnitState {
                exists: true,
                name: Some("Galas".into()),
                ..Default::default()
            }),
        );
        let muted = |s: &mut UiScript, arg: &str| {
            s.eval::<Option<i64>>(&format!("return GetMuteStatus({arg}, 'raid')"))
                .unwrap()
        };
        assert_eq!(muted(&mut s, "'galas'"), Some(1), "a name, any case");
        assert_eq!(muted(&mut s, "'party1'"), Some(1), "a token's name");
        assert_eq!(muted(&mut s, "'Erdrin'"), None);
        assert_eq!(
            s.eval::<i64>("return select('#', GetMuteStatus())")
                .unwrap(),
            0
        );
        // `UnitIsTalking` (`0x7df0b0`): nobody speaks with no voice channel joined.
        assert_eq!(
            s.eval::<(i64, i64)>(
                "return select('#', UnitIsTalking('Galas')), select('#', UnitIsTalking())"
            )
            .unwrap(),
            (1, 0)
        );
        assert!(s
            .eval::<bool>("return UnitIsTalking('party1') == nil")
            .unwrap());
    }

    /// A queued event would leave `order` one click stale.
    #[test]
    fn sort_who_sorts_in_place_and_fires_the_event_synchronously() {
        let mut s = seated();
        assert_eq!(s.eval::<i64>("return fired").unwrap(), 0);

        s.run(r#"SortWho("name")"#).unwrap();
        assert_eq!(s.eval::<i64>("return fired").unwrap(), 1);
        assert_eq!(
            s.eval::<String>("return order").unwrap(),
            "Erdrin,Galas,",
            "the handler must already see the sorted list"
        );
        assert_eq!(
            s.take_social_requests(),
            vec![SocialRequest::SortWho("name".into())],
            "and the app hears the same click, so its copy of the chain follows"
        );
        // Returns nothing (`0x6b5020`, `xor eax,eax`).
        assert_eq!(s.arity(r#"SortWho("name")"#).unwrap(), 0);
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }

    #[test]
    fn a_repeated_header_click_reverses_and_the_direction_is_remembered() {
        let s = seated();
        s.run(r#"SortWho("name")"#).unwrap();
        assert_eq!(s.eval::<String>("return order").unwrap(), "Erdrin,Galas,");

        s.run(r#"SortWho("name")"#).unwrap();
        assert_eq!(
            s.eval::<String>("return order").unwrap(),
            "Galas,Erdrin,",
            "the second click on the same key reverses it"
        );

        // Level ascending puts Erdrin (12) first; name is still descending behind it.
        s.run(r#"SortWho("level")"#).unwrap();
        assert_eq!(s.eval::<String>("return order").unwrap(), "Erdrin,Galas,");

        // Back to name, promoted from slot 1: it keeps its descending direction.
        s.run(r#"SortWho("name")"#).unwrap();
        assert_eq!(
            s.eval::<String>("return order").unwrap(),
            "Galas,Erdrin,",
            "a key promoted from behind keeps the direction it was left in"
        );
        assert_eq!(
            s.eval::<i64>("return fired").unwrap(),
            4,
            "one fire per click"
        );
    }

    /// A number is stringified and falls to the name key; anything else raises with the usage
    /// string (`0xa31114`), which 12340 spells right.
    #[test]
    fn sort_who_takes_the_reference_argument_abi() {
        let mut s = seated();
        s.run("SortWho(5)").unwrap();
        assert_eq!(
            s.eval::<String>("return order").unwrap(),
            "Erdrin,Galas,",
            "an unrecognised key is the name key"
        );
        let _ = s.take_social_requests();

        let err = s.run("SortWho({})").unwrap_err().to_string();
        assert!(err.contains(r#"Usage: SortWho("type")"#), "{err}");
        assert_eq!(
            s.eval::<i64>("return fired").unwrap(),
            1,
            "a raised call sorts nothing and fires nothing"
        );
    }

    /// Seven values a row, the class file name last; past the list, nil but a level of 0.
    #[test]
    fn get_who_info_answers_seven_values() {
        let s = seated();
        assert_eq!(s.arity("GetWhoInfo(1)").unwrap(), 7);
        assert_eq!(
            s.eval::<(String, String, i64, String, String, String, String)>("return GetWhoInfo(1)")
                .unwrap(),
            (
                "Galas".into(),
                String::new(),
                60,
                "Human".into(),
                "Warrior".into(),
                "Elwynn Forest".into(),
                "WARRIOR".into()
            )
        );
        for index in ["0", "3", "-1"] {
            assert!(
                s.eval::<bool>(&format!(
                    "local n, g, l, r, c, z, f = GetWhoInfo({index}) \
                     return select('#', GetWhoInfo({index})) == 7 and n == nil and g == nil \
                     and l == 0 and r == nil and c == nil and z == nil and f == nil"
                ))
                .unwrap(),
                "{index}"
            );
        }
        assert_eq!(
            s.eval::<String>("return (GetWhoInfo('2.5'))").unwrap(),
            "Erdrin",
            "a numeric string truncates"
        );
        let err = s.run("GetWhoInfo({})").unwrap_err().to_string();
        assert!(err.contains("Usage: GetWhoInfo(index)"), "{err}");
    }

    #[test]
    fn send_who_takes_a_string_or_number_and_raises_on_anything_else() {
        let mut s = UiScript::new().unwrap();
        s.run("SendWho('n-Galas') SendWho(80)").unwrap();
        assert_eq!(
            s.take_social_requests(),
            vec![
                SocialRequest::Who("n-Galas".into()),
                SocialRequest::Who("80".into())
            ]
        );
        let err = s.run("SendWho()").unwrap_err().to_string();
        assert!(err.contains(r#"Usage: SendWho("filter")"#), "{err}");
        assert!(s.take_social_requests().is_empty());
    }

    #[test]
    fn set_who_to_ui_reads_its_flag_as_get_bool_or_default() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "SetWhoToUI(1) SetWhoToUI(0) SetWhoToUI(0.5) SetWhoToUI('0') SetWhoToUI(true) \
             SetWhoToUI()",
        )
        .unwrap();
        assert_eq!(
            s.take_social_requests(),
            [true, false, false, false, true, false]
                .map(SocialRequest::SetWhoToUi)
                .to_vec()
        );
    }
}
