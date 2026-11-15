//! Taint: which code runs as the stock UI's and which as an addon's or a macro's, and the
//! protected-action gate it feeds.
//!
//! The reference's execution taint (`0xd4139c`) is the running owner of `lobject.h`'s owner tag
//! (`third_party/lua-src/BENILLA.md`): an addon's files run under its tag, a closure made under a
//! tag carries it, entering such a closure makes it the running one, and a call from the host
//! starts from the tag the host set. A tag names an addon by its place in
//! [`super::Model::addon_owners`]; `RunScript`'s macro taint and `forceinsecure`'s are two more
//! names there. What the reference also tags (every value written, so a read taints the reader)
//! is not carried: a value's taint is its object's owner.

use std::ffi::c_int;

use mlua::{Lua, MultiValue, Table, Value};

use super::ScriptValue;

unsafe extern "C" {
    fn lua_getgcowner() -> c_int;
    fn lua_getgcbaseowner() -> c_int;
    fn lua_setgcowner(owner: c_int);
    fn lua_setgcrunning(owner: c_int);
    fn lua_getobjowner(state: *mut mlua::ffi::lua_State, idx: c_int) -> c_int;
}

/// `RunScript`'s taint (`[0xac804c]`), an empty name no addon can hold.
pub(crate) const MACRO_TAINT: &str = "";
/// `forceinsecure`'s taint (`0x816f00`).
const FORCED_TAINT: &str = "*** TaintForced ***";

/// The running tag, 0 while the running code is secure.
pub(crate) fn running() -> u8 {
    // SAFETY: a plain load from the thread's owner cell.
    unsafe { lua_getgcowner() as u8 }
}

/// Make `tag` the running one, as the reference's stores to its taint global do.
pub(crate) fn set_running(tag: u8) {
    // SAFETY: a plain store to the thread's owner cell.
    unsafe { lua_setgcrunning(tag.into()) };
}

/// Run `f` as a dispatch under `tag` from the host or from a binding: running and base both, so a
/// call that enters Lua from outside starts there too, and both restored after.
pub(crate) fn run_as<R>(tag: u8, f: impl FnOnce() -> R) -> R {
    // SAFETY: plain loads and stores to the thread's owner cells.
    let (run, base) = unsafe { (lua_getgcowner(), lua_getgcbaseowner()) };
    unsafe { lua_setgcowner(tag.into()) };
    let out = f();
    unsafe {
        lua_setgcowner(base);
        lua_setgcrunning(run);
    }
    out
}

/// The tag for owner `name`, allotting one past the last on first use; 255 is shared by every
/// owner past the 254th.
pub(crate) fn tag_of(model: &mut super::Model, name: &str) -> u8 {
    let owners = &mut model.addon_owners;
    let at = owners.iter().position(|n| n.eq_ignore_ascii_case(name));
    let at = at.unwrap_or_else(|| {
        owners.push(name.to_string());
        owners.len() - 1
    });
    (at + 1).min(255) as u8
}

/// The name tag `tag` stands for, `None` for 0.
fn name_of(model: &super::Model, tag: u8) -> Option<String> {
    let at = usize::from(tag).checked_sub(1)?;
    let owners = &model.addon_owners;
    owners.get(at).or_else(|| owners.last()).cloned()
}

/// A collectable value's owner tag, 0 for none or a plain value.
fn owner_of(lua: &Lua, value: Value) -> mlua::Result<u8> {
    if !matches!(
        value,
        Value::Function(_) | Value::Table(_) | Value::UserData(_) | Value::Thread(_)
    ) {
        return Ok(0);
    }
    // SAFETY: the value is pushed by `exec_raw`; the callback reads its header and pushes one int,
    // which comes back after the value.
    let (_, tag): (Value, i64) = unsafe {
        lua.exec_raw(value, |state| {
            let tag = lua_getobjowner(state, -1);
            mlua::ffi::lua_pushinteger(state, tag as mlua::ffi::lua_Integer);
        })?
    };
    Ok(tag as u8)
}

/// `0x488540`: region `id`'s protected verbs are open when it is not protected (`0x489bb0`), the
/// running code is secure, or protected actions are allowed (`[+0x124c]`).
pub(crate) fn allowed(lua: &Lua, id: u32) -> bool {
    if running() == 0 {
        return true;
    }
    let model = lua
        .app_data_ref::<super::ModelCell>()
        .expect("model app_data");
    model.protected_actions_allowed || !model.protection_cache.borrow_mut().contains(&model, id)
}

/// A gated verb's prologue: refused, the game UI's blocked-action callback runs (`0x513640`) and the
/// verb does nothing.
pub(crate) fn gate(lua: &Lua, this: &Table) -> mlua::Result<bool> {
    if running() == 0 {
        return Ok(true);
    }
    if allowed(lua, super::object::decode_id(this)?) {
        return Ok(true);
    }
    blocked(lua, this);
    Ok(false)
}

/// [`gate`] over a method's argument list, whose first value is the receiver.
pub(crate) fn gate_args(lua: &Lua, args: &MultiValue) -> mlua::Result<bool> {
    match args.front() {
        Some(Value::Table(this)) if running() != 0 => gate(lua, this),
        _ => Ok(true),
    }
}

/// `0x513530` with a blocked frame verb: once per nesting, `MACRO_ACTION_BLOCKED` with the calling
/// function under the macro taint, else `ADDON_ACTION_BLOCKED` with the taint and the function.
pub(crate) fn blocked(lua: &Lua, this: &Table) {
    let tag = running();
    let taint = {
        let mut model = super::model_mut_of(lua);
        if model.blocked_feedback_showing {
            return;
        }
        model.blocked_feedback_showing = true;
        name_of(&model, tag).unwrap_or_default()
    };
    let function = format!("{}()", current_function(lua, this));
    if taint == MACRO_TAINT {
        super::event::fire_global(lua, "MACRO_ACTION_BLOCKED", &[ScriptValue::Str(function)]);
    } else {
        super::event::fire_global(
            lua,
            "ADDON_ACTION_BLOCKED",
            &[ScriptValue::Str(taint), ScriptValue::Str(function)],
        );
    }
    super::model_mut_of(lua).blocked_feedback_showing = false;
}

/// `0x817ee0`: the running function's name as its caller spelled it, `Object:name` for a method
/// call, `UNKNOWN` when the caller gave none.
fn current_function(lua: &Lua, this: &Table) -> String {
    let named = lua.inspect_stack(0, |d| {
        let n = d.names();
        (
            n.name.map(|s| s.into_owned()),
            n.name_what.is_some_and(|w| w == "method"),
        )
    });
    match named {
        Some((Some(name), true)) => {
            let object = this
                .get::<mlua::Function>("GetName")
                .and_then(|f| f.call::<Option<String>>(this.clone()))
                .ok()
                .flatten()
                .unwrap_or_else(|| "<unnamed>".to_string());
            format!("{object}:{name}")
        }
        Some((Some(name), false)) => name,
        _ => "UNKNOWN".to_string(),
    }
}

/// Install the taint family on the base table: `issecure`, `issecurevariable`, `forceinsecure`,
/// and the get and set `securecall`'s and the hook wrappers' Lua halves use.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    // issecure() (`0x816da0`): 1 while the running code is secure, else nil.
    g.set(
        "issecure",
        lua.create_function(|_, ()| Ok((running() == 0).then_some(1i64)))?,
    )?;
    // issecurevariable([table,] "name") (`0x816de0`): 1 for a secure value, else nil and its taint.
    g.set(
        "issecurevariable",
        lua.create_function(|lua, args: MultiValue| {
            let mut it = args.into_iter();
            let first = it.next().unwrap_or(Value::Nil);
            let (t, name) = match first {
                Value::Table(t) => (t, it.next().unwrap_or(Value::Nil)),
                other => (lua.globals(), other),
            };
            if !matches!(
                name,
                Value::String(_) | Value::Integer(_) | Value::Number(_)
            ) {
                return Err(mlua::Error::runtime(
                    "Usage: issecurevariable([table,] \"variable\")",
                ));
            }
            let key = lua.coerce_string(name)?;
            let value: Value = t.get(key)?;
            let tag = owner_of(lua, value)?;
            if tag == 0 {
                return Ok((Some(1i64), None));
            }
            let model = lua
                .app_data_ref::<super::ModelCell>()
                .expect("model app_data");
            Ok((None, name_of(&model, tag)))
        })?,
    )?;
    // forceinsecure() (`0x816f00`): secure code becomes tainted with the forced taint.
    g.set(
        "forceinsecure",
        lua.create_function(|lua, ()| {
            if running() == 0 {
                let tag = tag_of(&mut super::model_mut_of(lua), FORCED_TAINT);
                set_running(tag);
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

/// The running tag as a Lua value and its store, for the Lua halves of `securecall` and the hook
/// wrappers, which save the caller's taint around a call and put it back.
pub(super) fn accessors(lua: &Lua) -> mlua::Result<(mlua::Function, mlua::Function)> {
    let get = lua.create_function(|_, ()| Ok(i64::from(running())))?;
    let set = lua.create_function(|_, tag: i64| {
        set_running(tag.clamp(0, 255) as u8);
        Ok(())
    })?;
    Ok((get, set))
}

/// `RunScript`'s execute (`0x510b30`): `f` runs under the macro taint.
pub(crate) fn as_macro<R>(lua: &Lua, f: impl FnOnce() -> R) -> R {
    let tag = tag_of(&mut super::model_mut_of(lua), MACRO_TAINT);
    run_as(tag, f)
}

/// The taint a handler was set under, which its dispatch runs with (`0x81a2c0`).
pub(crate) fn script_taint(lua: &Lua, id: u32, kind: &str) -> u8 {
    let model = lua
        .app_data_ref::<super::ModelCell>()
        .expect("model app_data");
    if model.script_taint.is_empty() {
        return 0;
    }
    model.script_taint.get(&id).map_or(0, |kinds| {
        kinds
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(kind))
            .map_or(0, |&(_, tag)| tag)
    })
}

/// Record `tag` as the taint frame `id`'s `kind` handler was set under (`0x49ec80`).
pub(crate) fn note_script_taint(lua: &Lua, id: u32, kind: &'static str, tag: u8) {
    let mut model = super::model_mut_of(lua);
    if tag == 0 && model.script_taint.is_empty() {
        return;
    }
    let kinds = model.script_taint.entry(id).or_default();
    kinds.retain(|(k, _)| *k != kind);
    if tag != 0 {
        kinds.push((kind, tag));
    }
    if kinds.is_empty() {
        model.script_taint.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// A protected header, a plain frame and a listener logging the blocked events, in combat.
    fn script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        let doc = crate::framexml::parse(
            r#"<Ui><Frame name="ProtectedTemplate" virtual="true" protected="true"/></Ui>"#,
        )
        .unwrap();
        let report = crate::loader::load(&s, &doc, &|_| None);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        s.run(
            r#"
            Header = CreateFrame("Frame", "Header", nil, "ProtectedTemplate")
            Header:SetPoint("CENTER", 0, 0)
            Plain = CreateFrame("Frame", "Plain")
            Plain:SetPoint("CENTER", 0, 0)
            Blocked = {}
            local log = CreateFrame("Frame")
            log:RegisterEvent("ADDON_ACTION_BLOCKED")
            log:RegisterEvent("MACRO_ACTION_BLOCKED")
            log:SetScript("OnEvent", function(self, event, a, b)
                Blocked[#Blocked + 1] = event .. "|" .. tostring(a) .. "|" .. tostring(b)
            end)
            "#,
        )
        .unwrap();
        s.set_regen_enabled(false);
        s
    }

    fn blocked(s: &UiScript) -> Vec<String> {
        s.eval::<Vec<String>>("return Blocked").unwrap()
    }

    fn point_x(s: &UiScript, frame: &str) -> f64 {
        s.eval::<f64>(&format!("return select(4, {frame}:GetPoint())"))
            .unwrap()
    }

    #[test]
    fn a_tainted_move_or_show_of_a_protected_frame_in_combat_is_refused_and_reported() {
        let s = script();
        s.with_addon_owner("TaintTest", || {
            s.run(r#"Header:SetPoint("CENTER", 5, 0) Header:Hide()"#)
        })
        .unwrap();
        assert_eq!(point_x(&s, "Header"), 0.0);
        assert!(s.eval::<bool>("return Header:IsShown() == 1").unwrap());
        assert_eq!(
            blocked(&s),
            [
                "ADDON_ACTION_BLOCKED|TaintTest|Header:SetPoint()",
                "ADDON_ACTION_BLOCKED|TaintTest|Header:Hide()",
            ]
        );
        assert_eq!(s.eval::<i64>("return InCombatLockdown()").unwrap(), 1);
    }

    #[test]
    fn stock_code_and_code_out_of_combat_go_through() {
        let mut s = script();
        s.run(r#"Header:SetPoint("CENTER", 5, 0) Header:Hide() Header:Show()"#)
            .unwrap();
        assert_eq!(point_x(&s, "Header"), 5.0);
        s.set_regen_enabled(true);
        s.with_addon_owner("TaintTest", || {
            s.run(r#"Header:SetPoint("CENTER", 7, 0) Header:Hide()"#)
        })
        .unwrap();
        assert_eq!(point_x(&s, "Header"), 7.0);
        assert!(s.eval::<bool>("return Header:IsShown() == nil").unwrap());
        assert!(blocked(&s).is_empty());
    }

    #[test]
    fn an_unprotected_frame_is_never_gated() {
        let s = script();
        s.with_addon_owner("TaintTest", || {
            s.run(r#"Plain:SetPoint("CENTER", 3, 0) Plain:Hide() Plain:SetFrameStrata("HIGH")"#)
        })
        .unwrap();
        assert_eq!(point_x(&s, "Plain"), 3.0);
        assert!(blocked(&s).is_empty());
    }

    #[test]
    fn issecure_answers_from_the_running_code() {
        let s = script();
        assert_eq!(s.eval::<Option<i64>>("return issecure()").unwrap(), Some(1));
        s.with_addon_owner("TaintTest", || {
            s.run("AddonSecure = issecure() function AddonFunc() return 1 end")
        })
        .unwrap();
        assert_eq!(s.eval::<Option<i64>>("return AddonSecure").unwrap(), None);
        // A function an addon made is that addon's; a stock one is secure.
        let (secure, taint): (Option<i64>, Option<String>) =
            s.eval("return issecurevariable('AddonFunc')").unwrap();
        assert_eq!((secure, taint.as_deref()), (None, Some("TaintTest")));
        let (secure, taint): (Option<i64>, Option<String>) =
            s.eval("return issecurevariable('CreateFrame')").unwrap();
        assert_eq!((secure, taint), (Some(1), None));
        // forceinsecure taints secure code.
        let forced: (Option<i64>, Option<i64>) = s
            .eval("local a = issecure() forceinsecure() return a, issecure()")
            .unwrap();
        assert_eq!(forced, (Some(1), None));
    }

    #[test]
    fn calling_tainted_code_taints_the_caller_but_securecall_and_hooks_put_it_back() {
        let s = script();
        s.with_addon_owner("TaintTest", || {
            s.run("function AddonFunc() end function AddonHook() end")
        })
        .unwrap();
        let spread: Option<i64> = s.eval("AddonFunc() return issecure()").unwrap();
        assert_eq!(spread, None);
        let kept: Option<i64> = s
            .eval("securecall(AddonFunc) securecall('AddonFunc') return issecure()")
            .unwrap();
        assert_eq!(kept, Some(1));
        s.run("function StockFunc() end").unwrap();
        s.with_addon_owner("TaintTest", || {
            s.run("hooksecurefunc('StockFunc', AddonHook)")
        })
        .unwrap();
        let hooked: Option<i64> = s.eval("StockFunc() return issecure()").unwrap();
        assert_eq!(hooked, Some(1));
        // A stock handler an addon hooked still moves the protected frame.
        s.run(r#"Plain:SetScript("OnShow", function() Header:SetPoint("CENTER", 9, 0) end)"#)
            .unwrap();
        s.with_addon_owner("TaintTest", || {
            s.run(r#"Plain:HookScript("OnShow", AddonHook)"#)
        })
        .unwrap();
        s.run("Plain:Hide() Plain:Show()").unwrap();
        assert_eq!(point_x(&s, "Header"), 9.0);
        assert!(blocked(&s).is_empty());
    }

    #[test]
    fn a_handler_runs_under_the_taint_it_was_set_with() {
        let s = script();
        s.run(r#"function StockOnShow() Header:SetPoint("CENTER", 4, 0) end"#)
            .unwrap();
        // A stock handler an addon sets runs tainted, whoever shows the frame.
        s.with_addon_owner("TaintTest", || {
            s.run(r#"Plain:SetScript("OnShow", StockOnShow) Plain:Hide()"#)
        })
        .unwrap();
        s.run("Plain:Show()").unwrap();
        assert_eq!(point_x(&s, "Header"), 0.0);
        assert_eq!(
            blocked(&s),
            ["ADDON_ACTION_BLOCKED|TaintTest|Header:SetPoint()"]
        );
        // One stock code set runs secure, even when an addon shows the frame.
        s.run(r#"Plain:SetScript("OnShow", StockOnShow) Plain:Hide()"#)
            .unwrap();
        s.with_addon_owner("TaintTest", || s.run("Plain:Show()"))
            .unwrap();
        assert_eq!(point_x(&s, "Header"), 4.0);
    }

    /// Warden's executor runs secure, even under an addon's taint, and an error never raises.
    #[test]
    fn the_warden_executor_runs_secure_and_reports_without_raising() {
        let mut s = script();
        s.with_addon_owner("TaintTest", || {
            s.execute_secure("WardenSecure = issecure() forceinsecure() WardenForced = issecure()")
        });
        let ran: (Option<i64>, Option<i64>) = s.eval("return WardenSecure, WardenForced").unwrap();
        assert_eq!(ran, (Some(1), None));
        s.execute_secure("this is not lua");
        let errs = s.take_errors();
        assert!(
            errs.iter().any(|e| e.contains("[string \"?\"]:1:")),
            "{errs:?}"
        );
    }

    #[test]
    fn run_script_is_macro_tainted_and_a_nested_refusal_is_silent() {
        let s = script();
        s.run(r#"RunScript("Header:SetFrameLevel(9)")"#).unwrap();
        assert_eq!(
            blocked(&s),
            ["MACRO_ACTION_BLOCKED|Header:SetFrameLevel()|nil"]
        );
        assert_ne!(s.eval::<i64>("return Header:GetFrameLevel()").unwrap(), 9);
        // A listener whose own call is refused does not hear of it again.
        s.with_addon_owner("TaintTest", || {
            s.run(
                r#"
                local again = CreateFrame("Frame")
                again:RegisterEvent("ADDON_ACTION_BLOCKED")
                again:SetScript("OnEvent", function() Header:Raise() end)
                Header:SetScale(2)
                "#,
            )
        })
        .unwrap();
        assert_eq!(
            blocked(&s),
            [
                "MACRO_ACTION_BLOCKED|Header:SetFrameLevel()|nil",
                "ADDON_ACTION_BLOCKED|TaintTest|Header:SetScale()",
            ]
        );
    }

    #[test]
    fn attribute_changes_follow_the_gate_or_a_secure_allowance() {
        let s = script();
        s.with_addon_owner("TaintTest", || {
            s.run(r#"CanChange = Header:CanChangeAttribute() Header:SetAttribute("x", 1)"#)
        })
        .unwrap();
        assert_eq!(s.eval::<Option<i64>>("return CanChange").unwrap(), None);
        assert_eq!(
            s.eval::<Option<i64>>("return Header:GetAttribute('x')")
                .unwrap(),
            None
        );
        s.run("Header:AllowAttributeChanges()").unwrap();
        s.with_addon_owner("TaintTest", || s.run(r#"Header:SetAttribute("x", 2)"#))
            .unwrap();
        assert_eq!(
            s.eval::<Option<i64>>("return Header:GetAttribute('x')")
                .unwrap(),
            Some(2)
        );
    }
}
