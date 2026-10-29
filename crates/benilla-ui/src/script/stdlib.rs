//! The sandbox and the 1.12 stdlib layer on top of stock Lua 5.1.
//!
//! - [`sandbox`]: no filesystem, OS or native reach (`io`, `os`, `package`, `require`, `dofile`,
//!   `loadfile`, `load`, `module` and `debug` removed), the reference's `debugstack`, and a
//!   text-only `loadstring`.
//! - [`install`]: the 1.12 bare globals (string, table and math aliases, the `debug*` family, the
//!   error-handler pair), `time` and `date`, and a `string.format` that accepts positional `%N$`
//!   specs as the 1.12 client's does, rewriting them into a sequential format. Mixing positional
//!   and sequential specs raises, as in the reference, whose single argument cursor a positional
//!   spec desyncs.

use mlua::{Lua, Value, Variadic};

/// An mlua error's bare Lua message, as `loadstring` returns it: without the `syntax error: ` or
/// `runtime error: ` word mlua's `Display` adds and the reference never does.
pub(super) fn lua_message(e: &mlua::Error) -> String {
    match e {
        mlua::Error::SyntaxError { message, .. } => message.clone(),
        mlua::Error::RuntimeError(m) => m.clone(),
        other => other.to_string(),
    }
}

/// Sandbox the VM: no filesystem, OS or native reach, the reference's `debugstack`, and text-only
/// chunk loading.
pub(super) fn sandbox(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // Set to nil explicitly, whatever mlua's safe stdlib already leaves out. `newproxy` stays: it
    // is in 3.3.5a's `_G`, and `RestrictedFrames.lua:67` calls it at file scope.
    for name in [
        "io", "os", "package", "require", "dofile", "loadfile", "load", "module", "debug",
    ] {
        g.set(name, Value::Nil)?;
    }

    // debugstack([start [, count1 [, count2]]]): the reference's traceback, frames only, in Lua
    // 5.0's wording. There is no `stack traceback:` header and line 1 is the caller's frame:
    // `AceDB-2.0.lua:742` skips one line to read its caller's addon folder, and `AceLibrary.lua:70`
    // reads line 1 as a frame. `AceLibrary.lua:139` needs 5.0's ``in function `name'`` quoting,
    // not later Luas' `'name'`. `start` defaults to 1, the caller; `count1`/`count2` are 5.0's
    // `LEVELS1`/`LEVELS2`, which `FuBarPlugin-2.0.lua:752` relies on with `debugstack(6, 1, 0)`.
    g.set(
        "debugstack",
        lua.create_function(|lua, args: Variadic<Value>| {
            let num = |v: Option<&Value>, default: usize| match v {
                Some(Value::Integer(i)) => usize::try_from(*i).unwrap_or(0),
                Some(Value::Number(n)) if *n >= 0.0 => *n as usize,
                _ => default,
            };
            let start = num(args.first(), 1);
            let count1 = num(args.get(1), 12);
            let count2 = num(args.get(2), 10);
            Ok(traceback_frames(lua, start, count1, count2))
        })?,
    )?;

    // Text-only `loadstring`: a binary chunk is rejected. The BOM strip and `#`-line skip apply,
    // as in the reference they live inside `luaL_loadbuffer`/`luaX_setinput`.
    let loadstring = lua.create_function(
        |lua, (src, chunkname): (mlua::String, Option<mlua::String>)| {
            let raw = src.as_bytes();
            let bytes = crate::source::chunk(&raw).to_vec();
            // The chunk name is the caller's string verbatim, defaulting to the source itself
            // (`luaB_loadstring`), which `luaO_chunkid` renders as `[string "…"]`. It comes from
            // the raw bytes: `luaL_loadbuffer` strips a BOM only after `luaB_loadstring` took the
            // name.
            let name = match &chunkname {
                Some(n) => String::from_utf8_lossy(&n.as_bytes()).into_owned(),
                None => String::from_utf8_lossy(&raw).into_owned(),
            };
            let chunk = lua
                .load(bytes)
                .set_name(name)
                .set_mode(mlua::ChunkMode::Text);
            match chunk.into_function() {
                Ok(f) => Ok((Value::Function(f), Value::Nil)),
                // The failure leg returns nil and Lua's own message, unprefixed.
                Err(e) => Ok((
                    Value::Nil,
                    Value::String(lua.create_string(lua_message(&e))?),
                )),
            }
        },
    )?;
    g.set("loadstring", loadstring)?;

    // Captured before any interface file runs, so an addon's own `xpcall` never stands in. 5.1's
    // `xpcall` passes no arguments, so a call with some closes over them.
    let handler = lua.create_function(error_with_traceback)?;
    let pcall: mlua::Function = lua
        .load(
            "local xpcall, select, unpack, handler = xpcall, select, unpack, ...
             return function(f, ...)
                 local n = select('#', ...)
                 if n == 0 then return xpcall(f, handler) end
                 local args = { ... }
                 return xpcall(function() return f(unpack(args, 1, n)) end, handler)
             end",
        )
        .set_name(PCALL_CHUNK)
        .call(handler)?;
    lua.set_named_registry_value(REG_HANDLER_PCALL, pcall)?;

    Ok(())
}

/// The registry slot of [`protected_call`]'s `xpcall` wrapper.
const REG_HANDLER_PCALL: &str = "benilla.handler_pcall";

/// The wrapper's chunk name; its frames, and the `xpcall` between them, end a handler's traceback.
const PCALL_CHUNK: &str = "=handler_pcall";

/// Call a script handler protected, its error carrying a traceback as `Function::call`'s does.
/// mlua's own handler names each frame by searching `_G` for the function (5.3's
/// `pushglobalfuncname`), which stock 5.1 never does; under FrameXML's tens of thousands of
/// globals that search costs milliseconds per error.
pub(super) fn protected_call(
    lua: &Lua,
    f: &mlua::Function,
    args: mlua::MultiValue,
) -> mlua::Result<()> {
    let pcall = handler_pcall(lua)?;
    let call: mlua::MultiValue = std::iter::once(Value::Function(f.clone()))
        .chain(args)
        .collect();
    pcall_outcome(pcall.call(call)?)
}

/// [`protected_call`]'s `xpcall` wrapper, for a caller firing many handlers in one sweep.
pub(super) fn handler_pcall(lua: &Lua) -> mlua::Result<mlua::Function> {
    lua.named_registry_value(REG_HANDLER_PCALL)
}

/// The wrapper's `(ok, err)` as [`protected_call`]'s outcome.
pub(super) fn pcall_outcome((ok, err): (bool, Value)) -> mlua::Result<()> {
    if ok {
        return Ok(());
    }
    Err(match err {
        Value::Error(e) => *e,
        Value::String(s) => mlua::Error::RuntimeError(s.to_string_lossy()),
        other => mlua::Error::RuntimeError(
            other
                .to_string()
                .unwrap_or_else(|_| "<unprintable error>".into()),
        ),
    })
}

/// The `xpcall` message handler: the stack as stock 5.1's traceback walks it, from the frame that
/// raised (level 1, the handler being level 0), call-site names only, down to the handler.
fn error_with_traceback(lua: &Lua, e: Value) -> mlua::Result<Value> {
    let frames = traceback_frames(lua, 1, 12, 10);
    let plumbing = &PCALL_CHUNK[1..];
    let mut frames: Vec<&str> = frames
        .lines()
        .take_while(|l| !l.starts_with(plumbing))
        .collect();
    if frames.last().is_some_and(|l| l.ends_with("`xpcall'")) {
        frames.pop();
    }
    let traceback = format!("stack traceback:\n\t{}", frames.join("\n\t"));
    Ok(match e {
        Value::Error(cause) => {
            // mlua's own field type: `CallbackError::cause` is an `Arc<Error>`.
            #[allow(clippy::arc_with_non_send_sync)]
            let cause = std::sync::Arc::new(*cause);
            Value::Error(Box::new(mlua::Error::CallbackError { traceback, cause }))
        }
        Value::String(s) => {
            Value::String(lua.create_string(format!("{}\n{traceback}", s.to_string_lossy()))?)
        }
        other => other,
    })
}

/// Install the stdlib layer: the bare globals, `time`/`date` and the positional `format`.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    install_error_handler(lua)?;
    install_debuglocals(lua)?;
    install_time(lua)?;
    install_profile(lua)?;
    install_extras(lua)?;
    lua.load(WOW_STDLIB)
        .set_name("=[benilla wow stdlib]")
        .set_mode(mlua::ChunkMode::Text)
        .exec()?;
    Ok(())
}

/// `debugprofilestart`/`debugprofilestop` (`0x817350`/`0x817370`), the two real `debug*` verbs (on
/// the OS cycle clock): start latches, stop answers the milliseconds since. Ours read the
/// `GetTime()` clock ([`super::clock::now`]), the monotonic session seconds the tick advances, as
/// benilla has no cycle counter; read in Rust, so an addon that replaces the `GetTime` global
/// cannot bend it.
fn install_profile(lua: &Lua) -> mlua::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    let start = std::sync::Arc::new(AtomicU64::new(0f64.to_bits()));
    let g = lua.globals();
    let latch = start.clone();
    g.set(
        "debugprofilestart",
        lua.create_function(move |lua, ()| {
            latch.store(super::clock::now(lua).to_bits(), Ordering::Relaxed);
            Ok(())
        })?,
    )?;
    g.set(
        "debugprofilestop",
        lua.create_function(move |lua, ()| {
            let since = f64::from_bits(start.load(Ordering::Relaxed));
            Ok((super::clock::now(lua) - since) * 1000.0)
        })?,
    )?;
    Ok(())
}

/// The handler `seterrorhandler` holds, `None` until one is given.
pub(super) fn error_handler(lua: &Lua) -> Option<mlua::Function> {
    lua.named_registry_value::<Option<mlua::Function>>(super::REG_ERRORHANDLER)
        .ok()
        .flatten()
}

/// `seterrorhandler(f)` / `geterrorhandler()`, engine globals in 1.12 over one registry reference.
/// It starts at -1, so `geterrorhandler()` answers nil until something is set; in the running
/// client FrameXML sets `_ERRORMESSAGE` first (`BasicControls.xml:16`), so every addon finds that.
/// `seterrorhandler` takes a function alone and raises its `Usage:` on anything else, nil included,
/// so the slot can never be cleared.
fn install_error_handler(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    g.set(
        "seterrorhandler",
        lua.create_function(|lua, f: Value| match f {
            Value::Function(f) => lua.set_named_registry_value(super::REG_ERRORHANDLER, f),
            _ => Err(mlua::Error::RuntimeError(
                "Usage: seterrorhandler(errfunc)".into(),
            )),
        })?,
    )?;
    g.set(
        "geterrorhandler",
        lua.create_function(|lua, ()| Ok(error_handler(lua)))?,
    )?;
    Ok(())
}

/// The registry flag the script error dispatch (`0x819730`) holds around its call of the handler
/// (`[0xd3f79c]`), here around [`super::UiScript::dispatch_script_errors_to_handler`].
pub(super) const REG_IN_ERROR_HANDLER: &str = "benilla.in_error_handler";

/// `debuglocals([level])` (`0x819a60`): nothing outside the handler dispatch. Inside it, one string
/// of the frame `level` up's locals, then its function's upvalues, packed by
/// [`pack_variable`] into at most 4 KiB; `""` when there is no such frame. A level below 1 is 1,
/// the caller. The dispatch is deferred here, so the frames are the dispatch's own, not the
/// raising one's.
fn install_debuglocals(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "debuglocals",
        lua.create_function(|lua, level: Value| {
            if !lua
                .named_registry_value::<bool>(REG_IN_ERROR_HANDLER)
                .unwrap_or(false)
            {
                return Ok(mlua::MultiValue::new());
            }
            // `lua_tonumber` and a C `int` cast: a numeric string counts, anything else is 0.
            let level = match &level {
                Value::Integer(i) => *i as i32,
                Value::Number(n) => *n as i32,
                Value::String(s) => s
                    .to_str()
                    .ok()
                    .and_then(|s| s.trim().parse::<f64>().ok())
                    .map_or(0, |n| n as i32),
                _ => 0,
            }
            .max(1);
            let named = frame_variables(lua, level)?;
            let mut buffer = Vec::new();
            for [name, value] in named.as_chunks::<2>().0 {
                if let Value::String(name) = name {
                    pack_variable(lua, &name.as_bytes(), value, 0, &mut buffer);
                }
            }
            Ok(mlua::MultiValue::from_vec(vec![Value::String(
                lua.create_string(buffer)?,
            )]))
        })?,
    )
}

/// The locals of the frame `level` above the caller, then its function's upvalues, as
/// name/value pairs (`lua_getstack`, `lua_getlocal`, `lua_getinfo("f")`, `lua_getupvalue`).
fn frame_variables(lua: &Lua, level: i32) -> mlua::Result<Vec<Value>> {
    use mlua::ffi;
    // SAFETY: the closure only reads the stack through the debug API and leaves the pairs it
    // pushes as the call's results; `exec_raw` restores the stack around it.
    let pairs: mlua::MultiValue = unsafe {
        lua.exec_raw((), |state| {
            // Level 0 is `exec_raw`'s own C frame and 1 this function's, so the caller is 2.
            let mut ar: ffi::lua_Debug = std::mem::zeroed();
            if ffi::lua_getstack(state, level + 1, &mut ar) == 0 {
                return;
            }
            let mut n = 1;
            while ffi::lua_checkstack(state, 2) != 0 {
                let name = ffi::lua_getlocal(state, &ar, n);
                if name.is_null() {
                    break;
                }
                ffi::lua_pushstring(state, name);
                ffi::lua_insert(state, -2);
                n += 1;
            }
            if ffi::lua_checkstack(state, 3) == 0 {
                return;
            }
            ffi::lua_getinfo(state, c"f".as_ptr(), &mut ar);
            let func = ffi::lua_gettop(state);
            let mut n = 1;
            while ffi::lua_checkstack(state, 2) != 0 {
                let name = ffi::lua_getupvalue(state, func, n);
                if name.is_null() {
                    break;
                }
                ffi::lua_pushstring(state, name);
                ffi::lua_insert(state, -2);
                n += 1;
            }
            ffi::lua_remove(state, func);
        })?
    };
    Ok(pairs.into_vec())
}

/// `debuglocals`' buffer size (`0x819a60`), the terminator included.
const LOCALS_BUFFER: usize = 4096;

/// The variable pack (`0x819300`): one `name = value` line per variable, appended to
/// `buffer` up to [`LOCALS_BUFFER`]. A table lists its entries one level deep, each indented one
/// space; a frame or region table is named by its object.
fn pack_variable(lua: &Lua, name: &[u8], value: &Value, depth: usize, buffer: &mut Vec<u8>) {
    let indent: &[u8] = if depth > 0 { b" " } else { b"" };
    let mut line = indent.to_vec();
    line.extend_from_slice(name);
    line.extend_from_slice(b" = ");
    match value {
        Value::Nil => line.extend_from_slice(b"nil"),
        Value::Boolean(b) => line.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::LightUserData(_) | Value::UserData(_) => line.extend_from_slice(b"<userdata>"),
        Value::Integer(_) | Value::Number(_) => {
            if let Ok(Some(s)) = lua.coerce_string(value.clone()) {
                line.extend_from_slice(&s.as_bytes());
            }
        }
        Value::String(s) => {
            line.push(b'"');
            line.extend_from_slice(c_str(&s.as_bytes()));
            line.push(b'"');
        }
        Value::Table(t) => {
            line.extend_from_slice(object_display_name(t).as_bytes());
            line.extend_from_slice(b" {");
            push_line(buffer, line);
            if depth == 0 {
                for (key, entry) in t.clone().pairs::<Value, Value>().flatten() {
                    // `lua_isnumber` keys print as `%d`; any other `lua_tostring`, NULL as MSVC's.
                    let key_text = match &key {
                        Value::Integer(i) => i.to_string().into_bytes(),
                        Value::Number(n) => (*n as i32).to_string().into_bytes(),
                        Value::String(s) => {
                            match s.to_str().ok().and_then(|s| s.trim().parse::<f64>().ok()) {
                                Some(n) => (n as i32).to_string().into_bytes(),
                                None => c_str(&s.as_bytes()).to_vec(),
                            }
                        }
                        _ => b"(null)".to_vec(),
                    };
                    pack_variable(lua, &key_text, &entry, 1, buffer);
                }
            }
            let mut close = indent.to_vec();
            close.push(b'}');
            push_line(buffer, close);
            return;
        }
        Value::Function(f) => {
            // `lua_getinfo(">nS")` on a value: no call record, so never a name.
            let info = f.info();
            let source = info.source.as_deref().unwrap_or("?").to_string();
            let defined = info.line_defined.map_or(-1, |l| l as i64);
            line.extend_from_slice(format!("<function> defined {source}:{defined}").as_bytes());
        }
        Value::Thread(_) => line.extend_from_slice(b"<thread>"),
        other => line.extend_from_slice(format!("<{}>", other.type_name()).as_bytes()),
    }
    push_line(buffer, line);
}

/// A table's display: its object's name, `<unnamed>` without one (`0x482aa0`), or `<table>` when
/// `[0]` holds no object.
fn object_display_name(t: &mlua::Table) -> String {
    if !matches!(t.raw_get::<Value>(0), Ok(Value::LightUserData(_))) {
        return "<table>".into();
    }
    t.get::<mlua::Function>("GetName")
        .and_then(|f| f.call::<Option<String>>(t.clone()))
        .ok()
        .flatten()
        .unwrap_or_else(|| "<unnamed>".into())
}

/// `%s` stops at a NUL.
fn c_str(bytes: &[u8]) -> &[u8] {
    bytes.split(|b| *b == 0).next().unwrap_or(&[])
}

/// The line and its `\n`, formatted into 1 KiB, then appended to the buffer, each truncating short
/// of its terminator.
fn push_line(buffer: &mut Vec<u8>, mut line: Vec<u8>) {
    line.push(b'\n');
    line.truncate(1023);
    let room = (LOCALS_BUFFER - 1).saturating_sub(buffer.len());
    buffer.extend_from_slice(&line[..line.len().min(room)]);
}

/// The stdlib layer in Lua, run once at construction.
const WOW_STDLIB: &str = r#"
-- ── the bare string family ─────────────────────────────────────────────────────────────────────
-- Every name here is present in the REAL 1.12 client's global table — the in-world `_G` captured
-- from the running reference client (19,572 entries), which is the
-- authority this file answers to.
--
-- `strlenutf8` is an engine function, installed in Rust; `strmatch`, `strrev` and `gmatch` are
-- the stock `Compat.lua`'s. `strcmputf8i` is not in 12340's table and stays out: a global the
-- client lacks sends a feature-detecting addon down a path we cannot honour.
strlen  = string.len
strsub  = string.sub
strupper = string.upper
strlower = string.lower
strrep  = string.rep
strfind = string.find
strbyte = string.byte
strchar = string.char
gsub    = string.gsub
tinsert = table.insert
tremove = table.remove
-- `getn`, `sort`, `foreach` and `foreachi` are bound by `lua51`, from `table`.

-- ── the bare math aliases (the same 1.12 global family; the reference FrameXML uses them
-- unqualified — WorldMapFrame's overlay pool calls ceil/mod, CooldownFrame floor, …). `mod` is
-- fmod under a 5.0-era name (stock 5.1 dropped math.mod).
ceil  = math.ceil
floor = math.floor
abs   = math.abs
max   = math.max
min   = math.min
sqrt  = math.sqrt
mod   = math.mod        -- rebound to `math.fmod` by `lua51`, as the client's compat chunk does
random = math.random
-- `randomseed` is an ENGINE global in 1.12 exactly like `random` beside it (the captured `_G` types
-- both `function engine`), and it is the half that was missing: `IgniteStatus` calls it at file
-- scope in its OnLoad and dies on `attempt to call global 'randomseed'`. Seeding a PRNG is the one
-- thing an addon does that has no FrameXML counterpart to copy, so the bare name is all it has.
randomseed = math.randomseed
-- `PI` is an ENGINE global in 1.12, not a FrameXML one: the reference reads it bare (UIParent.lua's
-- `elapsedTime * 2 * PI * ROTATIONS_PER_SECOND`, TabardFrame.lua l.61-68) and no shipped FrameXML
-- file ever assigns it (grepped over the whole 1.12 extraction) — so the client supplies it, here,
-- beside the rest of the bare math family.
PI = math.pi
-- The 1.12 bare trig globals are DEGREE-based (the reference CombatText's fountain scroll calls
-- cos/sin with degree arguments); rad/deg ride along as the same family.
function sin(d) return math.sin(math.rad(d)) end
function cos(d) return math.cos(math.rad(d)) end
rad = math.rad
deg = math.deg
-- The rest of the bare math family, each verified present in the real 1.12 client's global
-- table (the captured `_G`).
-- `tan` and the inverses follow sin/cos into DEGREES — same family, same convention; that is
-- consistent-with the verified sin/cos finding rather than separately byte-verified, and it is
-- the convention every addon rotation helper assumes (`atan2` returning degrees is why
-- `atan2(dy, dx)` feeds a texture rotation directly).
function tan(d) return math.tan(math.rad(d)) end
function asin(x) return math.deg(math.asin(x)) end
function acos(x) return math.deg(math.acos(x)) end
function atan(x) return math.deg(math.atan(x)) end
function atan2(y, x) return math.deg(math.atan2(y, x)) end
exp = math.exp
log = math.log
log10 = math.log10
frexp = math.frexp
ldexp = math.ldexp

-- ── the debug* family: six verified STUBS and two real ones ────────────────────────────────────
--
-- All eight, read at the bytes:
-- `debuginfo`, `debugload`, `debugprint`, `debugdump`, `debugbreak` and `debugtimestamp` are
-- **byte-identical `xor eax,eax; ret` stubs** — three bytes, no `call`, no memory write. They
-- cannot print, log, write or set a global, and they return ZERO Lua values. So these are not
-- no-ops we invented under a real name (an "absent capability"); they are the
-- reference's own no-ops, transcribed.
--
-- That is what lets `BasicControls.xml` stop guarding its `debuginfo()` call and transcribe
-- `_ERRORMESSAGE` verbatim, which it now does.
--
-- `debugprofilestart`/`debugprofilestop` are the two that are REAL; they are Rust
-- (`install_profile`).
do
    local function noop() end
    debuginfo = noop
    debugload = noop
    debugprint = noop
    debugdump = noop
    debugbreak = noop
    debugtimestamp = noop
end

-- ── _G accessors (getglobal/setglobal: _G[name] get/set) ───────────────────────────────
function getglobal(name) return _G[name] end
function setglobal(name, value) _G[name] = value end

-- ── GetLocale: benilla ships/reads enUS data only (the 5875 MPQs + Spell.dbc enUS column) ──────
function GetLocale() return "enUS" end

-- ── NOT here: wipe / strsplit / strjoin / strconcat / strtrim ─────────────────────────────────
-- The client's own bindings, which `lua51.rs` installs after this layer.

-- ── Blizzard's positional string.format (%N$) ──────────────────────────────────────────────────
do
    -- `_find`/`_sub` are captured for the same reason `_format` is: this wrapper runs on every
    -- `format` call in the UI and must not follow an addon's later replacement of `string.*`.
    -- They are CALLS and not method syntax so the wrapper never resolves through a string
    -- metatable an addon may have replaced.
    local _format, _find, _sub, _len = string.format, string.find, string.sub, string.len
    local CONV = "diouxXeEfgGqscp"  -- Lua 5.1 conversion letters

    local function reformat(fmt, ...)
        if type(fmt) ~= "string" then
            return _format(fmt, unpack(arg, 1, arg.n))
        end
        -- fast path: no "%<digit>" at all ⇒ definitely no positional spec.
        if not _find(fmt, "%%%d") then
            return _format(fmt, unpack(arg, 1, arg.n))
        end

        local args = arg
        local pieces = {}      -- rebuilt (sequential) format fragments
        local order = {}       -- for each real conversion: the source arg index, or false=sequential
        local seen_pos, seen_seq = false, false
        local npieces, norder = 0, 0
        local i, len = 1, _len(fmt)

        while i <= len do
            local c = _sub(fmt, i, i)
            if c ~= "%" then
                npieces = npieces + 1; pieces[npieces] = c
                i = i + 1
            elseif _sub(fmt, i + 1, i + 1) == "%" then
                npieces = npieces + 1; pieces[npieces] = "%%"
                i = i + 2
            else
                local j = i + 1
                local ds, de = _find(fmt, "^%d+%$", j)  -- optional N$
                if ds then
                    seen_pos = true
                    norder = norder + 1; order[norder] = tonumber(_sub(fmt, ds, de - 1))
                    j = de + 1
                else
                    seen_seq = true
                    norder = norder + 1; order[norder] = false
                end
                -- copy flags/width/precision + the conversion letter
                local ce = _find(fmt, "[" .. CONV .. "]", j)
                if not ce then
                    error("invalid conversion in format string", 2)
                end
                npieces = npieces + 1; pieces[npieces] = "%" .. _sub(fmt, j, ce)
                i = ce + 1
            end
        end

        if seen_pos and seen_seq then
            error("cannot mix positional and sequential arguments in format string", 2)
        end

        local out, seq = {}, 0
        for k = 1, norder do
            local idx = order[k]
            if idx == false then
                seq = seq + 1
                out[k] = args[seq]
            else
                out[k] = args[idx]
            end
        end
        return _format(table.concat(pieces, "", 1, npieces), unpack(out, 1, norder))
    end

    string.format = reformat
    format = reformat
end
"#;

/// The rest of 12340's base-library extras table (`0xa443a8`) that FrameXML and addons call:
/// `strlenutf8` (`0x817c70`, stock `AutoComplete.lua` measures the typed text with it),
/// `strreplace` (`0x816c40`), `scrub` (`0x817bf0`), `difftime` (`0x8178a0`) and the no-op
/// `debughook`.
fn install_extras(lua: &Lua) -> mlua::Result<()> {
    use mlua::{MultiValue, Value};
    let g = lua.globals();
    // The UTF-8 characters of a string or number: every byte but a continuation byte.
    g.set(
        "strlenutf8",
        lua.create_function(|_, s: Value| {
            let bytes = match &s {
                Value::String(s) => s.as_bytes().to_vec(),
                Value::Integer(_) | Value::Number(_) => s.to_string()?.into_bytes(),
                _ => return Err(mlua::Error::runtime("Usage: strlenutf8(string)")),
            };
            Ok(bytes.iter().filter(|&&b| b & 0xc0 != 0x80).count() as i64)
        })?,
    )?;
    // strreplace(string, search, replace[, limit]): the text with up to `limit` plain matches
    // replaced (every one at 0) and the count, the result capped at the reference's 4096-byte
    // buffer. Deviation: an empty search answers the string unchanged, where the reference never
    // leaves its loop.
    g.set(
        "strreplace",
        lua.create_function(
            |lua,
             (string, search, replace, limit): (
                mlua::String,
                mlua::String,
                mlua::String,
                Option<i64>,
            )| {
                const BUFFER: usize = 4096;
                let (src, find, with) = (string.as_bytes(), search.as_bytes(), replace.as_bytes());
                let limit = limit.unwrap_or(0);
                let (mut out, mut count, mut at) = (Vec::new(), 0i64, 0usize);
                if !find.is_empty() {
                    while limit == 0 || count < limit {
                        let Some(hit) = src[at..].windows(find.len()).position(|w| w == &find[..])
                        else {
                            break;
                        };
                        out.extend_from_slice(&src[at..at + hit]);
                        out.extend_from_slice(&with[..]);
                        count += 1;
                        at += hit + find.len();
                    }
                }
                if count == 0 {
                    return Ok((string.clone(), 0));
                }
                out.extend_from_slice(&src[at..]);
                out.truncate(BUFFER);
                Ok((lua.create_string(&out)?, count))
            },
        )?,
    )?;
    // scrub(...): every argument that is not a boolean, number or string becomes nil.
    g.set(
        "scrub",
        lua.create_function(|_, args: MultiValue| {
            Ok(MultiValue::from_vec(
                args.into_iter()
                    .map(|v| match v {
                        Value::Boolean(_) | Value::Integer(_) | Value::Number(_) => v,
                        Value::String(_) => v,
                        _ => Value::Nil,
                    })
                    .collect(),
            ))
        })?,
    )?;
    // difftime(t2[, t1]): both truncated to whole seconds, as the C `time_t` casts do.
    g.set(
        "difftime",
        lua.create_function(|_, (t2, t1): (f64, Option<f64>)| {
            Ok(t2.trunc() - t1.unwrap_or(0.0).trunc())
        })?,
    )?;
    g.set("debughook", lua.create_function(|_, _: MultiValue| Ok(()))?)?;
    Ok(())
}

/// `time()` and `date([format [, when]])`: 1.12 engine globals, Lua 5.0's `os.time`/`os.date`
/// hoisted. `time` is wall-clock epoch seconds, as addons persist it across sessions.
///
/// Deviation: `date` formats UTC where the reference uses local time, because the tree has no
/// timezone database.
fn install_time(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    g.set(
        "time",
        lua.create_function(|_, ()| Ok(crate::civil::unix_seconds()))?,
    )?;
    g.set(
        "date",
        lua.create_function(|lua, (fmt, when): (Option<String>, Option<i64>)| {
            // Bare `date()` is `%c`, as in Lua.
            let fmt = fmt.unwrap_or_else(|| "%c".to_string());
            let secs = when.unwrap_or_else(crate::civil::unix_seconds);
            // A leading `!` selects UTC, which every time here already is, so it is only consumed.
            let body = fmt.strip_prefix('!').unwrap_or(&fmt);
            // `*t` answers a table, as in Lua: an addon that loops until its `wday` matches
            // (`Accountant_WeekStart`) spins forever on anything else.
            if body == "*t" {
                let c = crate::civil::from_unix(secs);
                let t = lua.create_table()?;
                t.set("year", c.year)?;
                t.set("month", c.month)?;
                t.set("day", c.day)?;
                t.set("hour", c.hour)?;
                t.set("min", c.min)?;
                t.set("sec", c.sec)?;
                // Lua counts weekdays 1..7 from Sunday; ours is the 0-based index into `DAYS`.
                t.set("wday", c.weekday + 1)?;
                // Already 1-based, as in Lua.
                t.set("yday", c.yearday)?;
                // UTC has no daylight saving.
                t.set("isdst", false)?;
                return Ok(mlua::Value::Table(t));
            }
            Ok(mlua::Value::String(
                lua.create_string(format_epoch(secs, body))?,
            ))
        })?,
    )?;
    Ok(())
}

const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// The `strftime` subset addons use; an unknown specifier is emitted verbatim.
fn format_epoch(secs: i64, fmt: &str) -> String {
    let crate::civil::Civil {
        year,
        month,
        day,
        hour,
        min,
        sec,
        weekday: wday,
        yearday: yday,
    } = crate::civil::from_unix(secs);
    let mut out = String::with_capacity(fmt.len() + 16);
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let Some(spec) = chars.next() else {
            out.push('%');
            break;
        };
        let hour12 = match hour % 12 {
            0 => 12,
            h => h,
        };
        match spec {
            'a' => out.push_str(&DAYS[wday as usize][..3]),
            'A' => out.push_str(DAYS[wday as usize]),
            'b' | 'h' => out.push_str(&MONTHS[(month - 1) as usize][..3]),
            'B' => out.push_str(MONTHS[(month - 1) as usize]),
            'c' => out.push_str(&format!(
                "{} {} {:2} {:02}:{:02}:{:02} {}",
                &DAYS[wday as usize][..3],
                &MONTHS[(month - 1) as usize][..3],
                day,
                hour,
                min,
                sec,
                year
            )),
            'd' => out.push_str(&format!("{day:02}")),
            'H' => out.push_str(&format!("{hour:02}")),
            'I' => out.push_str(&format!("{hour12:02}")),
            'j' => out.push_str(&format!("{yday:03}")),
            'm' => out.push_str(&format!("{month:02}")),
            'M' => out.push_str(&format!("{min:02}")),
            'p' => out.push_str(if hour < 12 { "AM" } else { "PM" }),
            'S' => out.push_str(&format!("{sec:02}")),
            'w' => out.push_str(&format!("{wday}")),
            'x' => out.push_str(&format!("{month:02}/{day:02}/{:02}", year.rem_euclid(100))),
            'X' => out.push_str(&format!("{hour:02}:{min:02}:{sec:02}")),
            'y' => out.push_str(&format!("{:02}", year.rem_euclid(100))),
            'Y' => out.push_str(&format!("{year}")),
            '%' => out.push('%'),
            other => {
                out.push('%');
                out.push(other);
            }
        }
    }
    out
}
/// `luaO_chunkid`, for `debugstack`'s `short_src`. An `@` name longer than [`CHUNKID_KEEP`] keeps
/// only its tail behind `"..."`, so a long addon path loses the `\AddOns\` that corpus libraries
/// search for.
fn chunk_id(source: &str) -> String {
    if let Some(rest) = source.strip_prefix('=') {
        // A `=name` chunk is taken verbatim, clipped to the buffer from the front.
        return rest.chars().take(LUA_IDSIZE - 1).collect();
    }
    if let Some(rest) = source.strip_prefix('@') {
        let n = rest.chars().count();
        if n <= CHUNKID_KEEP {
            return rest.to_string();
        }
        let tail: String = rest.chars().skip(n - CHUNKID_KEEP).collect();
        return format!("...{tail}");
    }
    // A string chunk: `[string "<first line, clipped>"]`.
    let first = source.split('\n').next().unwrap_or_default();
    let budget = LUA_IDSIZE - STRING_CHUNK_OVERHEAD;
    if first.chars().count() > budget || first.len() < source.len() {
        let clipped: String = first.chars().take(budget).collect();
        format!("[string \"{clipped}...\"]")
    } else {
        format!("[string \"{first}\"]")
    }
}

/// `LUA_IDSIZE`: the `short_src` buffer, 60 bytes with the terminator.
const LUA_IDSIZE: usize = 60;

/// Characters an `@` name keeps, tail-first, behind `"..."`.
const CHUNKID_KEEP: usize = 52;

/// What `[string "…"]` costs the budget in the third `luaO_chunkid` arm.
const STRING_CHUNK_OVERHEAD: usize = 17;

/// One `debugstack` frame, `short_src ":" [currentline ":"] DESC`, the caller pushing its `\n`.
/// DESC is `` in function `%s' `` for a named frame (``), else by `what`: `" in main
/// chunk"` (``), `" ?"` for C or a tail call (``), or `" in function <%s:%d>"` (``).
fn traceback_frame(d: &mlua::Debug) -> String {
    let src = d.source();
    let names = d.names();
    let short = src
        .source
        .as_deref()
        .map(chunk_id)
        .or_else(|| src.short_src.as_deref().map(str::to_owned))
        .unwrap_or_else(|| "?".to_string());
    let mut line = String::with_capacity(short.len() + 32);
    line.push_str(&short);
    line.push(':');
    if let Some(n) = d.current_line() {
        line.push_str(&n.to_string());
        line.push(':');
    }
    // A tail call has nothing to name it from and takes the `" ?"` arm, known by its `what` or as
    // the `(tail call)` pseudo-frame.
    let tail_call = src.what == "t" || short == "(tail call)";
    match names.name.as_deref() {
        Some(name) if !name.is_empty() && !tail_call => {
            line.push_str(" in function `");
            line.push_str(name);
            line.push('\'');
        }
        _ if src.what == "main" => line.push_str(" in main chunk"),
        _ if src.what == "C" || tail_call => line.push_str(" ?"),
        _ => {
            line.push_str(" in function <");
            line.push_str(&short);
            line.push(':');
            line.push_str(&src.line_defined.unwrap_or(0).to_string());
            line.push('>');
        }
    }
    line
}

/// `debugstack`'s walk: Lua 5.0's `db_errorfb` without its header, each frame followed by `"\n"`
/// rather than preceded by `"\n\t"`, so line 1 is a frame. Frames print while the level is
/// `<= start + count1` (`jbe`); then a failed `getstack(level + count2)` probe prints that level as
/// an ordinary frame, else `"...\n"` and the last `count2` follow. So `count1 = 1` gives two frames
/// on a two-deep stack, not one.
fn traceback_frames(lua: &Lua, start: usize, count1: usize, count2: usize) -> String {
    let mut out = String::new();
    let mut level = start;
    let mut first_part = true;
    while lua.inspect_stack(level, |_| ()).is_some() {
        level += 1;
        if level > start + count1 && first_part {
            first_part = false;
            if lua.inspect_stack(level + count2, |_| ()).is_none() {
                level -= 1; // nothing to elide: print this level after all
            } else {
                out.push_str("...\n");
                while lua.inspect_stack(level + count2, |_| ()).is_some() {
                    level += 1;
                }
            }
            continue;
        }
        if let Some(f) = lua.inspect_stack(level - 1, traceback_frame) {
            out.push_str(&f);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod date_table_tests {
    use crate::script::UiScript;

    /// `Accountant.lua:364-375` steps back a day until `date("*t", ct).wday` matches, which a
    /// string return never does.
    #[test]
    fn date_star_t_answers_a_table_that_walks_with_its_argument() {
        let s = UiScript::new().unwrap();
        // A Wednesday at 00:00 UTC: wday 4, counting from Sunday = 1.
        let t: i64 = 1_786_492_800;
        assert_eq!(
            s.eval::<(i64, i64, i64, i64, bool)>(&format!(
                "local d = date(\"*t\", {t}) return d.year, d.month, d.day, d.wday, d.isdst"
            ))
            .unwrap(),
            (2026, 8, 12, 4, false)
        );
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>(&format!(
                "local d = date(\"*t\", {t} + 3661) return d.hour, d.min, d.sec, d.yday"
            ))
            .unwrap(),
            (1, 1, 1, 224)
        );

        // Accountant's loop: one step back from Wednesday reaches Tuesday.
        assert_eq!(
            s.eval::<i64>(&format!(
                "local ct = {t} local n = 0 \
                 while date(\"*t\", ct).wday ~= 3 and n < 100 do ct = ct - 86400 n = n + 1 end \
                 return n"
            ))
            .unwrap(),
            1,
            "Wednesday(4) back to Tuesday(3) is exactly one step; a constant wday would spin"
        );

        // `!` is consumed, in the table form and the string form.
        assert_eq!(
            s.eval::<i64>(&format!("return date(\"!*t\", {t}).wday"))
                .unwrap(),
            4
        );
        assert!(
            !s.eval::<String>(&format!("return date(\"!%A\", {t})"))
                .unwrap()
                .contains('!'),
            "the UTC flag is consumed, not printed"
        );
        assert_eq!(
            s.eval::<String>(&format!("return date(\"%A\", {t})"))
                .unwrap(),
            "Wednesday"
        );
    }
}
