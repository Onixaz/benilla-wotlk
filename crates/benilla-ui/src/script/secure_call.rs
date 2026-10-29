//! 3.3.5a's `securecall(f, ...)`, the base-library call `UIDropDownMenu.lua` makes every menu
//! through, `issecure()`, which `UIDropDownMenu_CreateInfo` asks, and the rest of the security
//! family: `hooksecurefunc`, `issecurevariable` and `forceinsecure`.
//!
//! `0x816f30`: a string first argument is replaced by the global of that name; the call then runs
//! under `lua_pcall` with the error handler (`geterrorhandler()`'s reference) as its message
//! handler. On success it returns the function's results; on an error the handler has seen the
//! message and `securecall` returns nothing. The taint save and restore around the call has nothing
//! to act on: benilla runs every chunk alike.

use mlua::Lua;

const SECURECALL: &str = r##"
local type, xpcall, geterrorhandler, select, unpack = type, xpcall, geterrorhandler, select, unpack
local function results(ok, ...)
    if ok then
        return ...
    end
end
function securecall(...)
    local n = select("#", ...)
    if n == 0 then
        return
    end
    local f = ...
    local tf = type(f)
    if tf == "string" or tf == "number" then
        f = _G[f .. ""]
    end
    local args = { select(2, ...) }
    return results(xpcall(function()
        return f(unpack(args, 1, n - 1))
    end, function(msg)
        local handler = geterrorhandler()
        if handler then
            handler(msg)
        end
        return msg
    end))
end

-- hooksecurefunc([table,] "name", hook) (`0x817170`): the named function becomes a wrapper
-- (`FrameScript_HookWrapper`) that calls the original, its errors propagating, then the hook with
-- the same arguments under the error handler, and answers the original's results.
local error, rawset, tostring = error, rawset, tostring
function hooksecurefunc(t, name, hook)
    if type(t) ~= "table" then
        t, name, hook = _G, t, name
    end
    local tn = type(name)
    if (tn ~= "string" and tn ~= "number") or type(hook) ~= "function" then
        error('Usage: hooksecurefunc([table,] "function", hookfunc)', 2)
    end
    name = tostring(name)
    local original = t[name]
    if type(original) ~= "function" then
        error("hooksecurefunc(): " .. name .. " is not a function", 2)
    end
    rawset(t, name, function(...)
        local n = select("#", ...)
        local args = { ... }
        local out = { original(...) }
        local outn = table.maxn(out)
        xpcall(function()
            hook(unpack(args, 1, n))
        end, function(msg)
            local handler = geterrorhandler()
            if handler then
                handler(msg)
            end
            return msg
        end)
        return unpack(out, 1, outn)
    end)
end

-- issecurevariable([table,] "name") (`0x816de0`): 1 and nil for an untainted variable; benilla
-- taints nothing.
function issecurevariable(t, name)
    if type(t) ~= "table" then
        t, name = _G, t
    end
    local tn = type(name)
    if tn ~= "string" and tn ~= "number" then
        error('Usage: issecurevariable([table,] "variable")', 2)
    end
    return 1, nil
end

-- forceinsecure() (`0x816f00`) taints the running code; with no taint to set, it does nothing.
function forceinsecure() end
"##;

/// Install `securecall` on a target whose base library has it.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // `issecure()` (`0x816da0`): 1 while the taint global is clear, else nil. benilla taints
    // nothing, so it is always clear, as for the stock FrameXML in the reference; an addon's code,
    // which the reference runs tainted, reads 1 here too.
    lua.globals()
        .set("issecure", lua.create_function(|_, ()| Ok(1i64))?)?;
    lua.load(SECURECALL).set_name("=securecall").exec()
}

#[cfg(test)]
mod tests {

    use crate::script::UiScript;

    #[test]
    fn a_call_returns_its_results_and_a_name_reads_the_global() {
        let ui = UiScript::new().unwrap();
        ui.run("function Pair(a, b) return a + b, a * b end")
            .unwrap();
        let (sum, product): (i64, i64) = ui.eval("return securecall(Pair, 3, 4)").unwrap();
        assert_eq!((sum, product), (7, 12));
        let (sum, _): (i64, i64) = ui.eval("return securecall('Pair', 1, 2)").unwrap();
        assert_eq!(sum, 3);
    }

    #[test]
    fn an_error_goes_to_the_handler_and_returns_nothing() {
        let ui = UiScript::new().unwrap();
        ui.run(
            "seterrorhandler(function(msg) SEEN = msg end)
             N = select('#', securecall(function() error('boom', 0) end))",
        )
        .unwrap();
        assert_eq!(ui.eval::<i64>("return N").unwrap(), 0);
        assert_eq!(ui.eval::<String>("return SEEN").unwrap(), "boom");
    }
}
