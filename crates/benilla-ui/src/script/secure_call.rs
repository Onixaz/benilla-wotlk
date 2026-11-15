//! 3.3.5a's `securecall(f, ...)`, the base-library call `UIDropDownMenu.lua` makes every menu
//! through, and `hooksecurefunc`; the taint family they save and restore is in [`super::taint`].
//!
//! `0x816f30`: a string first argument is replaced by the global of that name; the call then runs
//! under `lua_pcall` with the error handler (`geterrorhandler()`'s reference) as its message
//! handler, and the caller's taint is put back after it. On success it returns the function's
//! results; on an error the handler has seen the message and `securecall` returns nothing.

use mlua::Lua;

const SECURECALL: &str = r##"
local get_taint, set_taint = ...
local type, xpcall, geterrorhandler, select, unpack = type, xpcall, geterrorhandler, select, unpack
local function results(taint, ok, ...)
    set_taint(taint)
    if ok then
        return ...
    end
end
function securecall(...)
    local taint = get_taint()
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
    return results(taint, xpcall(function()
        return f(unpack(args, 1, n - 1))
    end, function(msg)
        local handler = geterrorhandler()
        if handler then
            handler(msg)
        end
        return msg
    end))
end

-- hooksecurefunc([table,] "name", hook) (`0x817170`): the named function becomes a wrapper that
-- calls the original, its errors propagating, then the hook with the same arguments under the
-- error handler and the taint the original left, put back after, and answers the original's
-- results. The wrapper is made secure, as the reference's C closure carries no taint.
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
    local taint = get_taint()
    set_taint(0)
    local wrapper = function(...)
        local n = select("#", ...)
        local args = { ... }
        local out = { original(...) }
        local outn = table.maxn(out)
        local left = get_taint()
        xpcall(function()
            hook(unpack(args, 1, n))
        end, function(msg)
            local handler = geterrorhandler()
            if handler then
                handler(msg)
            end
            return msg
        end)
        set_taint(left)
        return unpack(out, 1, outn)
    end
    set_taint(taint)
    rawset(t, name, wrapper)
end
"##;

/// Registry key of the compiled script hook wrapper factory.
const REG_HOOK_WRAPPER: &str = "__benilla_hook_wrapper";

/// The script hook wrapper (`0x817050`): the original with the arguments, its errors propagating,
/// then the hook with the same arguments under a protected call whose message handler is the error
/// handler, so it runs at the error, with the taint the original left put back after it, and the
/// original's results returned.
const HOOK_WRAPPER: &str = r##"
local get_taint, set_taint = ...
local xpcall, select, unpack = xpcall, select, unpack
local function pack(...)
    return { n = select("#", ...), ... }
end
local function report(msg)
    local handler = geterrorhandler and geterrorhandler()
    if handler then
        handler(msg)
    end
    return msg
end
return function(original, hook)
    return function(...)
        local out = pack(original(...))
        local args = pack(...)
        local left = get_taint()
        xpcall(function()
            hook(unpack(args, 1, args.n))
        end, report)
        set_taint(left)
        return unpack(out, 1, out.n)
    end
end
"##;

/// `HookScript`'s wrapper over a set script and the hook, shared by every object kind; made
/// secure, as the reference's C closure carries no taint.
pub(crate) fn hook_wrapper(
    lua: &Lua,
    original: mlua::Function,
    hook: mlua::Function,
) -> mlua::Result<mlua::Function> {
    let factory = match lua.named_registry_value::<Option<mlua::Function>>(REG_HOOK_WRAPPER)? {
        Some(f) => f,
        None => {
            let (get, set) = super::taint::accessors(lua)?;
            let f: mlua::Function = super::taint::run_as(0, || {
                lua.load(HOOK_WRAPPER)
                    .set_name("=HookScript")
                    .call((get, set))
            })?;
            lua.set_named_registry_value(REG_HOOK_WRAPPER, f.clone())?;
            f
        }
    };
    let running = super::taint::running();
    super::taint::set_running(0);
    let wrapped = factory.call((original, hook));
    super::taint::set_running(running);
    wrapped
}

/// Install `securecall`, `hooksecurefunc` and the taint family on a target whose base library has
/// them.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    super::taint::install(lua)?;
    let (get, set) = super::taint::accessors(lua)?;
    lua.load(SECURECALL)
        .set_name("=securecall")
        .call::<()>((get, set))
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
