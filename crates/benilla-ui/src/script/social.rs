//! The social API: friends, ignores and `/who`. The app pushes a display-ready [`SocialState`]
//! (the reference's formatter `0x5ae160` also resolves names engine-side) and drains the queued
//! [`SocialRequest`]s. Selection is engine state in the reference (guids at FriendList `+0x648` and
//! `+0x720`, read back by `0x5ad260` and `0x5ae510`), so a set changes the snapshot at once and
//! queues the change for the app.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{bool_or_default, number_arg, string_arg};
use super::who_sort::WhoSortChain;

/// One friend row in `GetFriendInfo`'s order; offline, the level is 0 and class and area empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FriendInfo {
    /// From the name cache; empty while the name query is in flight.
    pub name: String,
    pub level: u32,
    pub class: String,
    /// The zone name, also empty when the id has no `AreaTable` row.
    pub area: String,
    pub connected: bool,
    /// The away tag the friends-list template takes: `""`, `"<AFK>"` or `"<DND>"`.
    pub status: String,
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
    /// In display order: the app sorts, as the reference's comparator `0x5ada00` does.
    pub friends: Vec<FriendInfo>,
    /// The selected friend, 1-based, 0 for none (`0x5ad260` returns the stored slot + 1).
    pub selected_friend: u32,
    pub ignores: Vec<String>,
    /// The selected ignore, on the same scale.
    pub selected_ignore: u32,
    /// The last `/who` answer's rows, at most 50.
    pub who: Vec<WhoInfo>,
    /// The total match count, `GetNumWhoResults`'s second return; it can exceed `who.len()`.
    pub who_total: u32,
    /// The `/who` sort chain, pushed so `SortWho` can re-sort before its synchronous redraw.
    pub who_sort: WhoSortChain,
}

/// Outbound social intents, drained by the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SocialRequest {
    /// `ShowFriends()`: `CMSG_FRIEND_LIST`.
    RefreshFriends,
    /// `AddFriend(name)`.
    AddFriend(String),
    /// `RemoveFriend(index)`, from the friends frame's button.
    RemoveFriendIndex(u32),
    /// `RemoveFriend(name)`, from `/removefriend`; the app resolves either shape to the guid.
    RemoveFriendName(String),
    /// `AddIgnore(name)`.
    AddIgnore(String),
    /// `DelIgnore(name)`.
    DelIgnore(String),
    /// `AddOrDelIgnore(name)`, `/ignore`'s toggle; the app decides which, as it holds the list.
    ToggleIgnore(String),
    /// A changed `SetLookingForGroup`: slots and comment for `CMSG_SET_LOOKING_FOR_GROUP`.
    SetLookingForGroup { slots: [u32; 3], comment: String },
    /// `SetSelectedFriend(index)`, mirrored so the next push agrees.
    SelectFriend(u32),
    /// `SetSelectedIgnore(index)`.
    SelectIgnore(u32),
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

    // GetFriendInfo(index) (`0x5ad060`): an index past the end answers nils, not an error;
    // FrameXML tests `if ( not name )`.
    g.set(
        "GetFriendInfo",
        lua.create_function(|lua, index: i64| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let Some(friend) = friend_at(&model.social.friends, index) else {
                return Ok((
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                ));
            };
            Ok((
                Value::String(lua.create_string(&friend.name)?),
                Value::Integer(i64::from(friend.level)),
                Value::String(lua.create_string(&friend.class)?),
                Value::String(lua.create_string(&friend.area)?),
                if friend.connected {
                    Value::Integer(1)
                } else {
                    Value::Nil
                },
                Value::String(lua.create_string(&friend.status)?),
            ))
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

    // SetSelectedFriend(index): set now, since `FriendsList_Update` reads it back in the same
    // call, and queue it for the app.
    g.set(
        "SetSelectedFriend",
        lua.create_function(|lua, index: i64| {
            let mut model = crate::script::model_mut_of(lua);
            let index = clamp_index(index, model.social.friends.len());
            model.social.selected_friend = index;
            model
                .social_requests
                .push(SocialRequest::SelectFriend(index));
            Ok(())
        })?,
    )?;

    // The LFG pair (`0x4e95d0`, `0x4e96b0`) is for addons: stock FrameXML's only calls sit inside
    // an XML comment. GetLookingForGroup() → the three slot names, each nil as the pack below
    // always stores 0, then the comment string.
    g.set(
        "GetLookingForGroup",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(mlua::MultiValue::from_vec(vec![
                Value::Nil,
                Value::Nil,
                Value::Nil,
                Value::String(lua.create_string(&model.lfg_comment)?),
            ]))
        })?,
    )?;
    // SetLookingForGroup(type1, entry1, type2, entry2, type3, entry3, comment): the loop over the
    // pairs ends at a non-number type or one >= 6. The stored word is `(type << 24) & entry`
    // (`0x4e9713`), an AND every reader decodes as an OR, so an admissible pair stores 0; the
    // per-type eligible-count check that also ends the reference's loop is not modelled. The
    // comment is argument 7, read only when argument 4 passes `lua_isstring`, and keeps 127 bytes
    // (`SStrCopy(…, 0x80)`). `CMSG_SET_LOOKING_FOR_GROUP` goes out only on a change.
    g.set(
        "SetLookingForGroup",
        lua.create_function(|lua, args: mlua::MultiValue| {
            let args: Vec<Value> = args.into_iter().collect();
            let arg = |i: usize| args.get(i - 1).cloned().unwrap_or(Value::Nil);
            let number = |v: &Value| match v {
                Value::Integer(i) => Some(*i as f64),
                Value::Number(n) => Some(*n),
                Value::String(s) => s.to_str().ok().and_then(|s| s.trim().parse::<f64>().ok()),
                _ => None,
            };
            let mut slots = [0u32; 3];
            for (slot, i) in slots.iter_mut().zip([1usize, 3, 5]) {
                let Some(ty) = number(&arg(i)) else { break };
                let ty = ty.trunc();
                if !(0.0..6.0).contains(&ty) {
                    break;
                }
                let entry = number(&arg(i + 1)).unwrap_or(0.0).trunc();
                // `&`, not `|`: the reference's own pack.
                *slot = ((ty as u32) << 24) & (entry as i64 as u32);
            }
            let comment = match arg(4) {
                Value::Integer(_) | Value::Number(_) | Value::String(_) => match arg(7) {
                    Value::String(s) => Some(s.to_string_lossy()),
                    Value::Integer(i) => Some(i.to_string()),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                },
                _ => None,
            };
            let mut model = crate::script::model_mut_of(lua);
            let mut changed = false;
            if model.lfg_slots != slots {
                model.lfg_slots = slots;
                changed = true;
            }
            if let Some(comment) = comment {
                let mut kept: String = comment
                    .chars()
                    .take_while({
                        let mut n = 0usize;
                        move |c| {
                            n += c.len_utf8();
                            n <= 127
                        }
                    })
                    .collect();
                kept.shrink_to_fit();
                if model.lfg_comment != kept {
                    model.lfg_comment = kept;
                    changed = true;
                }
            }
            if changed {
                let slots = model.lfg_slots;
                let comment = model.lfg_comment.clone();
                model
                    .social_requests
                    .push(SocialRequest::SetLookingForGroup { slots, comment });
            }
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

    // AddFriend(name): a blank name is not sent, as vmangos drops it without a reply
    // (`MiscHandler.cpp:468`).
    g.set(
        "AddFriend",
        lua.create_function(|lua, name: String| {
            if !name.trim().is_empty() {
                let mut model = crate::script::model_mut_of(lua);
                model.social_requests.push(SocialRequest::AddFriend(name));
            }
            Ok(())
        })?,
    )?;

    // RemoveFriend(indexOrName): the friends frame passes a row, `/removefriend` a name.
    g.set(
        "RemoveFriend",
        lua.create_function(|lua, who: Value| {
            let request = match who {
                Value::Integer(i) if i >= 1 => Some(SocialRequest::RemoveFriendIndex(i as u32)),
                Value::Number(n) if n >= 1.0 => Some(SocialRequest::RemoveFriendIndex(n as u32)),
                Value::String(s) => {
                    let name = s.to_str()?.to_string();
                    (!name.trim().is_empty()).then_some(SocialRequest::RemoveFriendName(name))
                }
                _ => None,
            };
            if let Some(request) = request {
                let mut model = crate::script::model_mut_of(lua);
                model.social_requests.push(request);
            }
            Ok(())
        })?,
    )?;

    // ── The ignore list ──
    // GetNumIgnores(): CountIgnores `0x5ae550`, over the 25 slots at `+0x650`.
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
        "GetIgnoreName",
        lua.create_function(|lua, index: i64| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let name = usize::try_from(index - 1)
                .ok()
                .and_then(|i| model.social.ignores.get(i));
            Ok(match name {
                Some(name) => Value::String(lua.create_string(name)?),
                None => Value::Nil,
            })
        })?,
    )?;

    // GetMuteStatus(nameOrUnit[, channel]) (`0x4fcdf0`): 1 for a player on the ignore or mute list
    // (`0x4fce5b`), the argument read as a cached name, else a unit token (`0x4fb580`); no value,
    // not an error, without a string (`0x4fce0d`). A voice-channel squelch also answers 1, and
    // benilla joins no voice channel.
    g.set(
        "GetMuteStatus",
        lua.create_function(|lua, args: MultiValue| {
            let arg = match args.into_iter().next() {
                Some(Value::String(s)) => s.to_string_lossy(),
                Some(Value::Integer(n)) => n.to_string(),
                Some(Value::Number(n)) => n.to_string(),
                _ => return Ok(MultiValue::new()),
            };
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let listed = |name: &str| {
                model
                    .social
                    .ignores
                    .iter()
                    .any(|i| i.eq_ignore_ascii_case(name))
            };
            let ignored = listed(&arg)
                || model
                    .unit(&arg)
                    .and_then(|u| u.name.as_deref())
                    .is_some_and(listed);
            Ok(MultiValue::from_vec(vec![super::binding_abi::flag(
                ignored,
            )]))
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

    // GetSelectedIgnore / SetSelectedIgnore (`0x5ae630` / `0x5ae5f0`), as the friend pair.
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
        lua.create_function(|lua, index: i64| {
            let mut model = crate::script::model_mut_of(lua);
            let index = clamp_index(index, model.social.ignores.len());
            model.social.selected_ignore = index;
            model
                .social_requests
                .push(SocialRequest::SelectIgnore(index));
            Ok(())
        })?,
    )?;

    for (global, make) in [
        ("AddIgnore", SocialRequest::AddIgnore as fn(String) -> _),
        ("DelIgnore", SocialRequest::DelIgnore as fn(String) -> _),
        (
            "AddOrDelIgnore",
            SocialRequest::ToggleIgnore as fn(String) -> _,
        ),
    ] {
        g.set(
            global,
            lua.create_function(move |lua, name: String| {
                if !name.trim().is_empty() {
                    let mut model = crate::script::model_mut_of(lua);
                    model.social_requests.push(make(name));
                }
                Ok(())
            })?,
        )?;
    }

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

    // SetWhoToUI(flag) (`0x6b3430`): the Who frame passes 1 on show and 0 on hide; the flag is
    // `GetBoolOrDefault` with default 0 (`0x815500`).
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

/// The 1-based friend lookup `GetFriendInfo` does, `None` past either end.
fn friend_at(friends: &[FriendInfo], index: i64) -> Option<&FriendInfo> {
    usize::try_from(index - 1).ok().and_then(|i| friends.get(i))
}

/// A selection index in `1..=len`, else 0, nothing selected.
fn clamp_index(index: i64, len: usize) -> u32 {
    if index >= 1 && index <= len as i64 {
        index as u32
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::{SocialRequest, SocialState, WhoInfo};
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

    #[test]
    fn the_lfg_pair_stores_the_comment_and_sends_only_on_a_change() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.arity("GetLookingForGroup()").unwrap(), 4);
        assert!(s
            .eval::<bool>(
                "local a, b, c, d = GetLookingForGroup() return a == nil and b == nil and c == nil and d == \"\""
            )
            .unwrap());
        // Three admissible pairs: every word packs to 0, so nothing changes or sends.
        s.run("SetLookingForGroup(1, 3, 3, 12, 5, 0)").unwrap();
        assert!(
            s.take_social_requests().is_empty(),
            "the AND pack stores zero"
        );
        // A comment behind the gate: argument 4 is a number, argument 7 the text.
        s.run(r#"SetLookingForGroup(1, 3, 3, 12, 5, 0, "LF2M UBRS")"#)
            .unwrap();
        assert_eq!(
            s.take_social_requests(),
            vec![SocialRequest::SetLookingForGroup {
                slots: [0; 3],
                comment: "LF2M UBRS".into()
            }]
        );
        assert_eq!(
            s.eval::<String>("local _, _, _, comment = GetLookingForGroup() return comment")
                .unwrap(),
            "LF2M UBRS"
        );
        s.run(r#"SetLookingForGroup(1, 3, 3, 12, 5, 0, "LF2M UBRS")"#)
            .unwrap();
        assert!(s.take_social_requests().is_empty());
        // Argument 4 nil: the comment at 7 is not read.
        s.run(r#"SetLookingForGroup(1, 3, nil, nil, nil, nil, "ignored")"#)
            .unwrap();
        assert!(s.take_social_requests().is_empty());
        assert_eq!(
            s.eval::<String>("local _, _, _, comment = GetLookingForGroup() return comment")
                .unwrap(),
            "LF2M UBRS"
        );
        // 127 bytes kept of a longer comment (`SStrCopy` into the 0x80 buffer).
        s.run(&format!(
            r#"SetLookingForGroup(0, 0, 0, 0, 0, 0, "{}")"#,
            "x".repeat(200)
        ))
        .unwrap();
        assert_eq!(
            s.eval::<String>("local _, _, _, comment = GetLookingForGroup() return comment")
                .unwrap()
                .len(),
            127
        );
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }
}
