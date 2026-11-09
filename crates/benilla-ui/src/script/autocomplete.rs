//! 3.3.5a's `GetAutoCompleteResults(text, include, exclude, numReturns [, cursorPosition
//! [, allowFullMatch]])` (`0x57b3a0`), the name lookup the stock edit box splits a whisper target
//! with (`ChatEdit_ExtractTellTarget`, `ChatFrame.lua:4125`), and its registrar sibling
//! `GetAutoCompletePresenceID`, which the whisper header asks of every target (`BNet.lua:38`).
//!
//! The first four arguments are checked (a string, then three numbers) and a miss raises the usage;
//! a `cursorPosition` that is no number or lies past the text is the text's end. The client keeps
//! one name list (`0x57b250`), each name with its source flags, as `AutoComplete.lua`'s
//! `AUTOCOMPLETE_FLAG_*` name them (in group 0x1, in guild 0x2, friend 0x4, interacted with 0x10,
//! online 0x20) and when it was last used. `autoCompleteResortNamesOnRecency` sorts it by that
//! time, latest first, ties and otherwise by name (`0x57b130`); `autoCompleteUseContext` gates on
//! the flags and `autoCompleteWhenEditingFromCenter` picks the match (`0x57aca0`). A whisper in or
//! out adds its name as interacted with and online and stamps the time (`0x57b7c0`).
//!
//! Deviation: the group, guild and friend names are read from the API at each call, not kept in a
//! list; no Battle.net name (0x8) exists without a session.

use mlua::{Lua, Value};

const AUTOCOMPLETE: &str = r##"
local matches, interacted = ...
local band, strlower = bit.band, string.lower
local FLAG_IN_GROUP, FLAG_IN_GUILD, FLAG_FRIEND, FLAG_INTERACTED, FLAG_ONLINE = 0x1, 0x2, 0x4, 0x10, 0x20

local function cvar_on(name)
    return tonumber(GetCVar(name) or "1") ~= 0
end

local function candidates()
    local byname, out = {}, {}
    local function add(name, flags, last)
        if name and name ~= "" then
            local key = strlower(name)
            local node = byname[key]
            if not node then
                node = { name = name, key = key, flags = 0, last = 0 }
                byname[key] = node
                out[#out + 1] = node
            end
            node.flags = bit.bor(node.flags, flags)
            if last and last > node.last then
                node.last = last
            end
        end
    end
    local raid = GetNumRaidMembers()
    if raid > 0 then
        for i = 1, raid do
            local name, _, _, _, _, _, _, online = GetRaidRosterInfo(i)
            add(name, FLAG_IN_GROUP + (online and FLAG_ONLINE or 0))
        end
    else
        for i = 1, GetNumPartyMembers() do
            add(UnitName("party" .. i), FLAG_IN_GROUP + FLAG_ONLINE)
        end
    end
    for i = 1, GetNumGuildMembers(true) do
        local name, _, _, _, _, _, _, _, online = GetGuildRosterInfo(i)
        add(name, FLAG_IN_GUILD + (online and FLAG_ONLINE or 0))
    end
    for i = 1, (GetNumFriends()) do
        local name, _, _, _, connected = GetFriendInfo(i)
        add(name, FLAG_FRIEND + (connected and FLAG_ONLINE or 0))
    end
    for _, contact in ipairs(interacted()) do
        add(contact[1], FLAG_INTERACTED + FLAG_ONLINE, contact[2])
    end
    return out
end

function GetAutoCompleteResults(text, include, exclude, numReturns, cursorPosition, allowFullMatch)
    if type(text) ~= "string" and type(text) ~= "number"
        or type(include) ~= "number" or type(exclude) ~= "number"
        or type(numReturns) ~= "number" then
        error("Usage: GetAutoCompleteResults(text, includeBitField, excludeBitField, numReturns[, cursorPosition[, allowFullMatch]])", 2)
    end
    text = tostring(text)
    local recency = cvar_on("autoCompleteResortNamesOnRecency")
    local context = cvar_on("autoCompleteUseContext")
    local center = cvar_on("autoCompleteWhenEditingFromCenter")
    local me = UnitName("player")
    me = me and strlower(me)
    local list = candidates()
    table.sort(list, function(a, b)
        if recency and a.last ~= b.last then
            return a.last > b.last
        end
        return a.key < b.key
    end)
    local results = {}
    for _, c in ipairs(list) do
        local passes = not context or (band(c.flags, include) ~= 0 and band(c.flags, exclude) == 0)
        if passes and c.key ~= me and matches(c.name, text, cursorPosition, center, allowFullMatch) then
            results[#results + 1] = c.name
            if #results == numReturns then
                break
            end
        end
    end
    return unpack(results)
end

-- GetAutoCompletePresenceID(name) (0x57b500): a Battle.net friend's id; with no session, none.
function GetAutoCompletePresenceID(name)
    if type(name) ~= "string" and type(name) ~= "number" then
        error('Usage: GetAutoCompletePresenceID("name")', 2)
    end
end
"##;

fn fold(s: &str) -> Vec<char> {
    s.chars().flat_map(char::to_lowercase).collect()
}

/// The name test (`0x57aca0`), in characters, without regard to case. `cursor` is the caret, the
/// text's end for a value that is no position in it.
fn name_matches(name: &str, text: &str, cursor: Option<f64>, center: bool, full: bool) -> bool {
    let (n, t) = (fold(name), fold(text));
    if !full && n == t {
        return false;
    }
    if !center {
        return n.starts_with(&t);
    }
    let cursor = cursor
        .map(|c| c as i64)
        .and_then(|c| usize::try_from(c).ok())
        .filter(|&c| c <= t.len())
        .unwrap_or(t.len());
    if !n.starts_with(&t[..cursor]) || n.len() < t.len() {
        return false;
    }
    if t.len() == cursor {
        return true;
    }
    // The tail must follow somewhere past the prefix, as typing into a name's middle leaves it.
    let tail = &t[cursor..];
    (cursor..cursor + (n.len() - tail.len() - cursor)).any(|p| n[p..].starts_with(tail))
}

impl super::UiScript {
    /// A whisper in or out names `name` (`0x57b7c0`): interacted with, online, used now.
    pub fn note_autocomplete_contact(&mut self, name: &str) {
        let now = self.now();
        self.model_mut()
            .autocomplete_contacts
            .insert(name.to_lowercase(), (name.to_string(), now));
    }
}

/// Install the verb on a target whose API has it.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let matches = lua.create_function(
        |lua, (name, text, cursor, center, full): (String, String, Value, bool, Value)| {
            let cursor = match cursor {
                Value::Integer(_) | Value::Number(_) => lua.coerce_number(cursor)?,
                _ => None,
            };
            Ok(name_matches(
                &name,
                &text,
                cursor,
                center,
                !matches!(full, Value::Nil | Value::Boolean(false)),
            ))
        },
    )?;
    let interacted = lua.create_function(|lua, ()| {
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model app_data");
        let out = lua.create_table()?;
        for (i, (name, last)) in model.autocomplete_contacts.values().enumerate() {
            let row = lua.create_table()?;
            row.raw_set(1, name.as_str())?;
            row.raw_set(2, *last)?;
            out.raw_set(i + 1, row)?;
        }
        Ok(out)
    })?;
    lua.load(AUTOCOMPLETE)
        .set_name("=autocomplete")
        .call::<()>((matches, interacted))
}

#[cfg(test)]
mod tests {

    use super::name_matches;
    use crate::script::UiScript;

    /// The list orders by last use, the latest first, and offers a whispered name under the
    /// interacted-with flag alone.
    #[test]
    fn a_whispered_name_is_offered_latest_first() {
        let mut s = UiScript::new().unwrap();
        let ask = "return GetAutoCompleteResults('', 0x10, 0, 5, nil, true)";
        s.note_autocomplete_contact("Bob");
        assert_eq!(s.eval::<String>(ask).unwrap(), "Bob");
        s.tick(5.0);
        s.note_autocomplete_contact("Arthas");
        assert_eq!(
            s.eval::<(String, String)>(ask).unwrap(),
            ("Arthas".into(), "Bob".into())
        );
        s.tick(5.0);
        s.note_autocomplete_contact("Bob");
        assert_eq!(
            s.eval::<(String, String)>(ask).unwrap(),
            ("Bob".into(), "Arthas".into())
        );
        assert_eq!(
            s.eval::<i64>("return select('#', GetAutoCompleteResults('', 0x2, 0, 5, nil, true))")
                .unwrap(),
            0,
            "not in the guild source"
        );
    }

    #[test]
    fn the_name_test_is_the_reference_s() {
        assert!(name_matches("Bobby", "bo", None, true, false));
        assert!(
            !name_matches("Bob", "BOB", None, true, false),
            "a full match needs the flag"
        );
        assert!(name_matches("Bob", "BOB", None, true, true));
        // Typing into the middle: the prefix before the caret and the tail somewhere after it.
        assert!(name_matches(
            "Bartholomewz",
            "Bamew",
            Some(2.0),
            true,
            false
        ));
        assert!(!name_matches(
            "Bartholomew",
            "Bamew",
            Some(2.0),
            false,
            false
        ));
        assert!(!name_matches(
            "Bartholomew",
            "Bazzz",
            Some(2.0),
            true,
            false
        ));
        assert!(name_matches("Éowyn", "éo", None, true, false));
    }

    /// A stranger is in no list, so a whisper's "name message" target finds nothing and the edit
    /// box falls back to the first word.
    #[test]
    fn a_name_in_no_list_finds_nothing() {
        let ui = UiScript::new().unwrap();
        assert_eq!(
            ui.eval::<i64>(
                "return select('#', GetAutoCompleteResults('Bob hi', 0xffffffff, 0, 1, nil, true))"
            )
            .unwrap(),
            0
        );
        assert!(
            ui.run("GetAutoCompleteResults('Bob')").is_err(),
            "the usage"
        );
    }
}
