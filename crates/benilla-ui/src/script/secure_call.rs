//! 3.3.5a's `securecall(f, ...)`, the base-library call `UIDropDownMenu.lua` makes every menu
//! through (`UIDropDownMenu.lua:64`), and `issecure()`, which `UIDropDownMenu_CreateInfo` asks.
//!
//! `0x816f30`: a string first argument is replaced by the global of that name; the call then runs
//! under `lua_pcall` with the error handler (`geterrorhandler()`'s reference) as its message
//! handler. On success it returns the function's results; on an error the handler has seen the
//! message and `securecall` returns nothing. The taint save and restore around the call
//! (`[0xd4139c]`) has nothing to act on: benilla runs every chunk alike.

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
"##;

/// Install `securecall` on a target whose base library has it.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // `issecure()` (`0x816da0`): 1 while the taint global `[0xd4139c]` is clear, else nil.
    // benilla taints nothing, so it is always clear, as for the stock FrameXML in the reference;
    // an addon's code, which the reference runs tainted, reads 1 here too.
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
