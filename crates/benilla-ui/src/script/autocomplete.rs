//! 3.3.5a's `GetAutoCompleteResults(text, include, exclude, numReturns [, cursorPosition
//! [, allowFullMatch]])` (`0x57b3a0`), the name lookup the stock edit box splits a whisper target
//! with (`ChatEdit_ExtractTellTarget`, `ChatFrame.lua:4125`), and its registrar sibling
//! `GetAutoCompletePresenceID`, which the whisper header asks of every target (`BNet.lua:38`).
//!
//! The first four arguments are checked (a string, then three numbers) and a miss raises the usage.
//! The prefix is `text` cut at `cursorPosition` when that is a number within it. A name answers
//! when it starts with the prefix, ignoring case, and is longer than it unless `allowFullMatch`;
//! when its source's flags meet `include` and miss `exclude`, as `AutoComplete.lua`'s
//! `AUTOCOMPLETE_FLAG_*` name them: in group 0x1, in guild 0x2, friend 0x4, online 0x20. At most
//! `numReturns` names come back, each once.
//!
//! The reference walks registries the client keeps itself. Deviation: these are the group, guild
//! and friend lists the API already answers, in that order; the recently-interacted list (0x10) is
//! not kept, and no Battle.net name (0x8) exists without a session.

use mlua::Lua;

const AUTOCOMPLETE: &str = r##"
local band, strlower, strsub, strlen = bit.band, string.lower, string.sub, string.len
local FLAG_IN_GROUP, FLAG_IN_GUILD, FLAG_FRIEND, FLAG_ONLINE = 0x1, 0x2, 0x4, 0x20

local function candidates()
    local out = {}
    local function add(name, flags)
        if name and name ~= "" then
            out[#out + 1] = { name = name, flags = flags }
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
    return out
end

function GetAutoCompleteResults(text, include, exclude, numReturns, cursorPosition, allowFullMatch)
    if type(text) ~= "string" and type(text) ~= "number"
        or type(include) ~= "number" or type(exclude) ~= "number"
        or type(numReturns) ~= "number" then
        error("Usage: GetAutoCompleteResults(text, includeBitField, excludeBitField, numReturns[, cursorPosition[, allowFullMatch]])", 2)
    end
    local prefix = tostring(text)
    if type(cursorPosition) == "number" and cursorPosition >= 0 and cursorPosition <= strlen(prefix) then
        prefix = strsub(prefix, 1, cursorPosition)
    end
    local lower, n = strlower(prefix), strlen(prefix)
    local results, seen = {}, {}
    for _, c in ipairs(candidates()) do
        if #results >= numReturns then
            break
        end
        local key = strlower(c.name)
        if band(c.flags, include) ~= 0 and band(c.flags, exclude) == 0 and not seen[key]
            and strsub(key, 1, n) == lower and (allowFullMatch or strlen(key) > n) then
            seen[key] = true
            results[#results + 1] = c.name
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

/// Install the verb on a target whose API has it.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.load(AUTOCOMPLETE).set_name("=autocomplete").exec()
}

#[cfg(test)]
mod tests {

    use crate::script::UiScript;

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
