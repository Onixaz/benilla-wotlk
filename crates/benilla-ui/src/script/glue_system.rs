//! The glue core's system verbs (table `0xac3e00`, registered by the glue manager's `Resume`
//! `0x4da5f0`) past the login screen's own ([`super::glue_core`]): the CVars, the client's
//! identity, the account list and authenticator flag, the Scan.dll check, the PIN, matrix and
//! token prompts, the billing, the glue sounds, the cursor, and the requests only the host can
//! serve (`EnterWorld`, `CancelLogin`, the realm split, the screenshot).

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{flag, string_arg};
use super::glue_core::{queue, GlueRequest};
use super::ScriptValue;

/// The Scan.dll check's state, `CGlueMgr::m_scandllStatus` (`[0xb6b46c]`) with the login gate
/// `m_scandllOkayToLogIn` (`[0xb6b474]`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScanDll {
    /// Not started (status 0): `IsScanDLLFinished` is nil and the login gate is shut.
    #[default]
    Idle,
    /// The scan ended without a verdict (status 2), the outcome for an install with no
    /// `Scan.dll` to load: `ScanThreadProc` (`0x4e58d0`) fails `ScanFileForHack`, and
    /// `ScanDLLFinished` (`0x4e5940`) fires `SCANDLL_ERROR`. `ScanDLLContinueAnyway` opens the
    /// gate from here.
    Failed,
    /// The gate is open: `IsScanDLLFinished` answers 1 and the login may proceed.
    Ok,
}

/// The billing group of the last `SMSG_AUTH_RESPONSE`: the connection's flags byte, its
/// remaining minutes and its rested minutes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlueBilling {
    pub flags: u8,
    pub remaining: u32,
    pub rested: u32,
}

/// The system verbs' slots in the glue model.
#[derive(Default)]
pub(crate) struct GlueSystem {
    pub(super) scan: ScanDll,
    /// `SCANDLL_ERROR` waits for the glue manager's next idle tick (`0x4dab40` at state 14 calls
    /// `ScanDLLFinished`), never inside the `ScanDLLStart` call.
    scan_error_due: bool,
    /// `CGxDevice::CursorSetVisible`'s last answer, `HideCursor`/`ShowCursor`.
    pub(super) cursor_hidden: bool,
    pub(super) billing: GlueBilling,
    /// Whether the logon was a trial account's, `Login::IsTrialAccount`; `None` with no logon.
    pub(super) trial: Option<bool>,
}

impl super::UiScript {
    /// The Scan.dll check's state.
    pub fn glue_scan(&self) -> ScanDll {
        self.model_ref().glue.system.scan
    }

    /// Seed the billing group the last `SMSG_AUTH_RESPONSE` carried.
    pub fn set_glue_billing(&mut self, billing: GlueBilling) {
        self.model_mut().glue.system.billing = billing;
    }

    /// Seed whether the logon was a trial account's (`None` with no logon).
    pub fn set_glue_trial(&mut self, trial: Option<bool>) {
        self.model_mut().glue.system.trial = trial;
    }

    /// Whether `HideCursor` hid the pointer.
    pub fn glue_cursor_hidden(&self) -> bool {
        self.model_ref().glue.system.cursor_hidden
    }

    /// The glue manager's idle arm for the Scan.dll check: once `ScanDLLStart` ran, the check
    /// ends here with `SCANDLL_ERROR` and the `SCANDLL_MESSAGE_ERROR` text (`0x4e5940`).
    pub fn run_glue_scan_tick(&mut self) {
        let due = std::mem::take(&mut self.model_mut().glue.system.scan_error_due);
        if due {
            let text = self.global_text("SCANDLL_MESSAGE_ERROR");
            self.fire_event("SCANDLL_ERROR", vec![ScriptValue::Str(text)]);
        }
    }
}

/// `CVar::LookupRegistered` failing: `Couldn't find CVar named '%s'`, which every glue CVar
/// getter but `GetCVarBool` raises.
fn no_such_cvar(name: &str) -> mlua::Error {
    mlua::Error::runtime(format!("Couldn't find CVar named '{name}'"))
}

/// A `lua_isstring` argument as text: a string, or a number in Lua's own form.
fn string_ish(lua: &Lua, v: Option<&Value>) -> Option<String> {
    match v? {
        v @ (Value::String(_) | Value::Integer(_) | Value::Number(_)) => lua
            .coerce_string(v.clone())
            .ok()
            .flatten()
            .map(|s| s.to_string_lossy()),
        _ => None,
    }
}

/// `StringToBOOL`: a CVar value's truth, as `GetCVarBool` (`0x4ddd90`) reads it.
fn cvar_truth(value: &str) -> bool {
    let v = value.trim();
    !(v.is_empty() || v == "0" || v.eq_ignore_ascii_case("false"))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `GetCVar` (`0x4ddd10`) and `GetCVarDefault` (`0x4dde10`): a string argument or the usage
    // error, then the registered CVar's value or default, or `Couldn't find CVar named`.
    for (name, default) in [("GetCVar", false), ("GetCVarDefault", true)] {
        let usage = format!("Usage: {name}(\"cvar\")");
        g.set(
            name,
            lua.create_function(move |lua, args: MultiValue| {
                let Some(cvar) = string_ish(lua, args.front()) else {
                    return Err(mlua::Error::runtime(usage.clone()));
                };
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let slot = model
                    .cvars
                    .get(&cvar.to_ascii_lowercase())
                    .ok_or_else(|| no_such_cvar(&cvar))?;
                let text = if default { &slot.default } else { &slot.value };
                Ok(Value::String(lua.create_string(text)?))
            })?,
        )?;
    }

    // `GetCVarBool` (`0x4ddd90`): 1 when the registered CVar's value reads true, else nil; an
    // unknown name is nil, unraised.
    g.set(
        "GetCVarBool",
        lua.create_function(|lua, args: MultiValue| {
            let Some(cvar) = string_ish(lua, args.front()) else {
                return Err(mlua::Error::runtime("Usage: GetCVarBool(\"cvar\")"));
            };
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(
                model
                    .cvars
                    .get(&cvar.to_ascii_lowercase())
                    .is_some_and(|s| cvar_truth(&s.value)),
            ))
        })?,
    )?;

    // `GetCVarMin`/`GetCVarMax` (`0x4ddea0`/`0x4ddf80`) and their absolute pair (`0x4de090`,
    // `0x4de130`): a range only for `farclip` (177 to 1277) and `extShadowQuality` (0 to the
    // device's highest shadow mode), nil for any other registered CVar.
    for (name, which) in [
        ("GetCVarMin", 0u8),
        ("GetCVarMax", 1),
        ("GetCVarAbsoluteMin", 2),
        ("GetCVarAbsoluteMax", 3),
    ] {
        let usage = format!("Usage: {name}(\"cvar\")");
        g.set(
            name,
            lua.create_function(move |lua, args: MultiValue| {
                let Some(cvar) = string_ish(lua, args.front()) else {
                    return Err(mlua::Error::runtime(usage.clone()));
                };
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let slot = model
                    .cvars
                    .get(&cvar.to_ascii_lowercase())
                    .ok_or_else(|| no_such_cvar(&cvar))?;
                let key = slot.name.to_ascii_lowercase();
                // `CShadowCache::GetMaxShadowMode` (`NUM_SHADOW_MODES - 1`): benilla draws no
                // shadow maps, so the device's highest mode is the lowest.
                const SHADOW_MAX: f64 = 0.0;
                let n = match (key.as_str(), which) {
                    ("extshadowquality", 0 | 2) => Some(0.0),
                    ("extshadowquality", 1 | 3) => Some(SHADOW_MAX),
                    ("farclip", 0) => Some(177.0),
                    ("farclip", 1) => Some(1277.0),
                    _ => None,
                };
                Ok(n.map_or(Value::Nil, Value::Number))
            })?,
        )?;
    }

    // `SetCVar(name, value)` (`0x4dce70`): the usage error, `Couldn't find CVar named`, the
    // read-only refusal, then the write; a value with no string form writes "0". The glue's has
    // no `CVAR_UPDATE` token.
    g.set(
        "SetCVar",
        lua.create_function(|lua, args: MultiValue| {
            let Some(name) = string_ish(lua, args.front()) else {
                return Err(mlua::Error::runtime(
                    "Usage: SetCVar(\"cvar\", value [, \"scriptCvar\")",
                ));
            };
            let value = string_ish(lua, args.get(1)).unwrap_or_else(|| "0".to_string());
            let mut model = crate::script::model_mut_of(lua);
            let key = name.to_ascii_lowercase();
            if !model.cvars.contains_key(&key) {
                return Err(no_such_cvar(&name));
            }
            if model.cvars_read_only.contains(&key) {
                return Err(mlua::Error::runtime(format!("\"{name}\" is read-only")));
            }
            super::cvars::write_cvar(&mut model, &name, value);
            Ok(())
        })?,
    )?;

    // The client's identity: `IsWindowsClient` (`0x4dce40`) 1, `IsMacClient`/`IsLinuxClient`
    // (`0x510b90`) nil, `GetClientExpansionLevel` (`0x4dcbf0`) the installed expansion plus one,
    // 3 for Wrath of the Lich King; `IsSystemSupported` (`0x4dd540`) the processor's SSE2 bit,
    // which every machine benilla runs on has.
    g.set(
        "IsWindowsClient",
        lua.create_function(|_, _: MultiValue| Ok(1.0))?,
    )?;
    for name in ["IsMacClient", "IsLinuxClient"] {
        g.set(
            name,
            lua.create_function(|_, _: MultiValue| Ok(Value::Nil))?,
        )?;
    }
    g.set(
        "GetClientExpansionLevel",
        lua.create_function(|_, _: MultiValue| Ok(3.0))?,
    )?;
    g.set(
        "IsSystemSupported",
        lua.create_function(|_, _: MultiValue| Ok(1.0))?,
    )?;
    // `IsStreamingMode`/`IsStreamingTrial` (`0x4dd430`/`0x4dd440`) return the count their
    // `SFile` test answers, 0 for a full install: no value at all.
    for name in ["IsStreamingMode", "IsStreamingTrial"] {
        g.set(
            name,
            lua.create_function(|_, _: MultiValue| Ok(MultiValue::new()))?,
        )?;
    }
    // `IsConsoleActive` (`0x4dd450`): nil, as benilla has no console window.
    g.set(
        "IsConsoleActive",
        lua.create_function(|_, _: MultiValue| Ok(Value::Nil))?,
    )?;
    // `IsTrialAccount` (`0x4dd4d0`): the logon's trial flag; with no logon, the `converted`
    // CVar's integer being 0, nil where there is no such CVar.
    g.set(
        "IsTrialAccount",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let trial = model.glue.system.trial.or_else(|| {
                model
                    .cvars
                    .get("converted")
                    .map(|s| super::glue_core::leading_int(&s.value) == 0)
            });
            Ok(flag(trial == Some(true)))
        })?,
    )?;
    // `GetAccountExpansionLevel` (`0x4ddc20`): the world connection's expansion byte from
    // `SMSG_AUTH_RESPONSE`, 0 before one.
    g.set(
        "GetAccountExpansionLevel",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(f64::from(model.account_expansion))
        })?,
    )?;
    // `IsShiftKeyDown` (`0x4dbe10`): 1 while a shift key is held, else nil.
    g.set(
        "IsShiftKeyDown",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(model.modifiers.0))
        })?,
    )?;
    // `RunScript(text)` (`0x4dd490`): a non-empty string runs as a chunk named by itself;
    // anything else does nothing.
    g.set(
        "RunScript",
        lua.create_function(|lua, args: MultiValue| {
            if let Some(text) = string_ish(lua, args.front()).filter(|t| !t.is_empty()) {
                lua.load(text.as_str()).set_name(text.as_str()).exec()?;
            }
            Ok(())
        })?,
    )?;

    // The account list (`GetSavedAccountList`/`SetSavedAccountList`, `0x4dd5f0`/`0x4dbf80`) and
    // the authenticator flag (`GetUsesToken`/`SetUsesToken`, `0x4dbf10`/`0x4dbf30`), each a CVar
    // (`accountList`, `g_accountUsesToken`). `GetUsesToken` pushes a boolean.
    g.set(
        "GetSavedAccountList",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let list = model
                .cvars
                .get("accountlist")
                .map(|s| s.value.clone())
                .unwrap_or_default();
            lua.create_string(&list)
        })?,
    )?;
    g.set(
        "SetSavedAccountList",
        lua.create_function(|lua, list: Value| {
            let list = string_arg(lua, list, "Usage: SetSavedAccountList(\"accountList\")")?;
            super::cvars::write_cvar(&mut crate::script::model_mut_of(lua), "accountList", list);
            Ok(())
        })?,
    )?;
    g.set(
        "GetUsesToken",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model
                .cvars
                .get("g_accountusestoken")
                .is_some_and(|s| super::glue_core::leading_int(&s.value) != 0))
        })?,
    )?;
    g.set(
        "SetUsesToken",
        lua.create_function(|lua, v: Value| {
            let Value::Boolean(on) = v else {
                return Err(mlua::Error::runtime("Usage: SetUsesToken( 0 | 1 )"));
            };
            super::cvars::write_cvar(
                &mut crate::script::model_mut_of(lua),
                "g_accountUsesToken",
                if on { "1" } else { "0" }.to_string(),
            );
            Ok(())
        })?,
    )?;
    // `SetClearConfigData([on])` (`0x4dd3b0`): the flag the client's exit reads to wipe its
    // configuration; benilla clears nothing on exit, so the flag has no reader.
    g.set(
        "SetClearConfigData",
        lua.create_function(|_, _: MultiValue| Ok(()))?,
    )?;

    // The changed-option warnings (`0x4de1f0`, `0x4dcf30`, `0x4dcf20`): benilla's options never
    // queue one, so there is nothing to show and nothing to accept.
    g.set(
        "GetChangedOptionWarnings",
        lua.create_function(|_, _: MultiValue| Ok(MultiValue::new()))?,
    )?;
    g.set(
        "ShowChangedOptionWarnings",
        lua.create_function(|_, _: MultiValue| Ok(Value::Nil))?,
    )?;
    g.set(
        "AcceptChangedOptionWarnings",
        lua.create_function(|_, _: MultiValue| Ok(()))?,
    )?;

    // The Scan.dll check. `ScanDLLStart(versionURL, dllURL)` (`0x4dccf0` → `0x4e5cb0`): the usage
    // error, then, once only, the scan thread; benilla loads no `Scan.dll`, so the scan ends at
    // the next idle tick as a failed load does. `ScanDLLContinueAnyway` (`0x4e5860`) opens the
    // gate after that ending; `IsScanDLLFinished` (`0x4dce00`) reads the gate.
    g.set(
        "ScanDLLStart",
        lua.create_function(|lua, args: MultiValue| {
            if string_ish(lua, args.front()).is_none() || string_ish(lua, args.get(1)).is_none() {
                return Err(mlua::Error::runtime(
                    "Usage: ScanDLLStart(\"VersionURL\", \"DLLURL\")",
                ));
            }
            let mut model = crate::script::model_mut_of(lua);
            let system = &mut model.glue.system;
            if system.scan == ScanDll::Idle {
                system.scan = ScanDll::Failed;
                system.scan_error_due = true;
            }
            Ok(())
        })?,
    )?;
    g.set(
        "ScanDLLContinueAnyway",
        lua.create_function(|lua, _: MultiValue| {
            let mut model = crate::script::model_mut_of(lua);
            if model.glue.system.scan == ScanDll::Failed {
                model.glue.system.scan = ScanDll::Ok;
            }
            Ok(())
        })?,
    )?;
    g.set(
        "IsScanDLLFinished",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(model.glue.system.scan == ScanDll::Ok))
        })?,
    )?;

    // The PIN, matrix and token prompts answer a logon that asked for one (`PLAYER_ENTER_PIN`,
    // `_MATRIX`, `_TOKEN`); the realm servers benilla meets ask for none, so each entry has no
    // logon step to feed. Their argument checks stand.
    g.set(
        "PINEntered",
        lua.create_function(|lua, args: MultiValue| {
            let mut count = 0;
            for v in args.iter().take(10) {
                if matches!(v, Value::Nil) {
                    break;
                }
                if lua.coerce_number(v.clone()).ok().flatten().is_none() {
                    return Err(mlua::Error::runtime(format!(
                        "Usage: PINEntered(4-10 key positions) - non-numeric arg #{}",
                        count + 1
                    )));
                }
                count += 1;
            }
            if !(4..=10).contains(&count) {
                return Err(mlua::Error::runtime(format!(
                    "Usage: PINEntered(4-10 key positions) - bad number of args {count}"
                )));
            }
            Ok(())
        })?,
    )?;
    g.set(
        "MatrixEntered",
        lua.create_function(|lua, v: Value| {
            let n = lua.coerce_number(v).ok().flatten();
            match n {
                None => Err(mlua::Error::runtime("Usage: MatrixEntered(number)")),
                Some(n) if n > 9.0 => {
                    Err(mlua::Error::runtime("MatrixEntered: number must be 0 to 9"))
                }
                Some(_) => Ok(()),
            }
        })?,
    )?;
    for name in ["MatrixRevert", "MatrixCommit", "TokenEntered"] {
        g.set(name, lua.create_function(|_, _: MultiValue| Ok(()))?)?;
    }
    g.set(
        "GetMatrixCoordinates",
        lua.create_function(|_, _: MultiValue| Ok((Value::Nil, Value::Nil, Value::Nil)))?,
    )?;

    // The game-account picker is a Battle.net logon's (`0x4dd130`, `0x4dd1c0`, `0x4dd2b0`); a
    // realm server's grunt logon answers `Wrong login type`, no logon `No login available`.
    for name in ["GetNumGameAccounts", "GetGameAccountInfo", "SetGameAccount"] {
        let usage = format!("Usage: {name}(index)");
        let needs_index = name != "GetNumGameAccounts";
        g.set(
            name,
            lua.create_function(move |lua, args: MultiValue| {
                let index = args.front().cloned().unwrap_or(Value::Nil);
                if needs_index && lua.coerce_number(index)?.is_none() {
                    return Err(mlua::Error::runtime(usage.clone()));
                }
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                Err::<(), _>(mlua::Error::runtime(if model.glue.system.trial.is_some() {
                    "Wrong login type"
                } else {
                    "No login available"
                }))
            })?,
        )?;
    }

    // Russian declension (`0x4dcf70`, `0x4dd020`): an enUS client has no declension sets.
    g.set(
        "GetNumDeclensionSets",
        lua.create_function(|lua, args: MultiValue| {
            if string_ish(lua, args.front()).is_none() {
                return Err(mlua::Error::runtime(
                    "Usage: GetNumDeclensionSets(\"name\", gender)",
                ));
            }
            Ok(0.0)
        })?,
    )?;
    g.set(
        "DeclineName",
        lua.create_function(|lua, args: MultiValue| {
            let set = args
                .get(2)
                .cloned()
                .and_then(|v| lua.coerce_number(v).ok().flatten());
            if string_ish(lua, args.front()).is_none() || set.is_none() {
                return Err(mlua::Error::runtime(
                    "Usage: DeclineName(\"name\", gender, declensionSet)",
                ));
            }
            Ok(MultiValue::from_vec(vec![Value::Nil; 5]))
        })?,
    )?;

    // The billing group of the last `SMSG_AUTH_RESPONSE`: `GetBillingTimeRemaining`
    // (`0x4ddbd0`, 0 while flag 0x2 is set), `GetBillingPlan` (`0x4ddc60`: the plan of flags
    // 0x16, then 0x20 and 0x8 as 0 or 1) and `GetBillingTimeRested`.
    g.set(
        "GetBillingTimeRemaining",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let b = model.glue.system.billing;
            Ok(if b.flags & 0x2 != 0 {
                0.0
            } else {
                f64::from(b.remaining)
            })
        })?,
    )?;
    g.set(
        "GetBillingPlan",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let flags = model.glue.system.billing.flags;
            let plan = match flags & 0x16 {
                0x2 => 1.0,
                0x4 => 2.0,
                0 => 3.0,
                0x10 => 4.0,
                _ => 0.0,
            };
            Ok((
                plan,
                f64::from(u8::from(flags & 0x20 != 0)),
                f64::from(u8::from(flags & 0x8 != 0)),
            ))
        })?,
    )?;
    g.set(
        "GetBillingTimeRested",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(f64::from(model.glue.system.billing.rested))
        })?,
    )?;

    // The glue sounds (`0x4dc0c0`, `0x4dc130`, `0x4dd380`, `0x4dc150`), the host's to play.
    g.set(
        "PlayGlueAmbience",
        lua.create_function(|lua, args: MultiValue| {
            let Some(name) = string_ish(lua, args.front()) else {
                return Err(mlua::Error::runtime(
                    "Usage: PlayGlueAmbience(\"sound name\", \"(optional)fade in time\")",
                ));
            };
            let fade = args
                .get(1)
                .filter(|v| matches!(v, Value::Number(_) | Value::Integer(_)))
                .and_then(|v| lua.coerce_number(v.clone()).ok().flatten())
                .map(|f| f as f32);
            queue(lua, GlueRequest::PlayGlueAmbience { name, fade });
            Ok(())
        })?,
    )?;
    g.set(
        "StopGlueAmbience",
        lua.create_function(|lua, _: MultiValue| {
            queue(lua, GlueRequest::StopGlueAmbience);
            Ok(())
        })?,
    )?;
    g.set(
        "StopAllSFX",
        lua.create_function(|lua, args: MultiValue| {
            let fade = args
                .front()
                .cloned()
                .and_then(|v| lua.coerce_number(v).ok().flatten())
                .unwrap_or(0.0) as f32;
            queue(lua, GlueRequest::StopAllSfx(fade));
            Ok(())
        })?,
    )?;
    g.set(
        "PlayCreditsMusic",
        lua.create_function(|lua, args: MultiValue| {
            let Some(kit) = string_ish(lua, args.front()) else {
                return Err(mlua::Error::runtime(
                    "Usage: PlayCreditsMusic( \"Sound kit name\" )",
                ));
            };
            queue(lua, GlueRequest::PlayCreditsMusic(kit));
            Ok(())
        })?,
    )?;

    // `ShowCursor`/`HideCursor` (`0x4ddb90`/`0x4ddbb0`).
    for (name, hidden) in [("ShowCursor", false), ("HideCursor", true)] {
        g.set(
            name,
            lua.create_function(move |lua, _: MultiValue| {
                crate::script::model_mut_of(lua).glue.system.cursor_hidden = hidden;
                Ok(())
            })?,
        )?;
    }
    // `GetMovieResolution` (`0x4dd610`): `gxResolution`'s width.
    g.set(
        "GetMovieResolution",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let width = model
                .cvars
                .get("gxresolution")
                .and_then(|s| s.value.split('x').next().map(super::glue_core::leading_int))
                .unwrap_or(0);
            Ok(width as f64)
        })?,
    )?;

    // The patch download (`0x4dda90`, `0x4dc540`, `0x4dc550`): benilla downloads no patch, so
    // there is no progress and nothing to cancel or apply.
    g.set(
        "PatchDownloadProgress",
        lua.create_function(|_, _: MultiValue| Ok(0.0))?,
    )?;
    for name in ["PatchDownloadCancel", "PatchDownloadApply"] {
        g.set(name, lua.create_function(|_, _: MultiValue| Ok(()))?)?;
    }
    // `SurveyNotificationDone([send])` (`0x4dccc0`): no survey is ever offered, so there is
    // nothing to send or end.
    g.set(
        "SurveyNotificationDone",
        lua.create_function(|_, _: MultiValue| Ok(()))?,
    )?;

    // The host's: `EnterWorld` (`0x4dc510` → `0x4d9bd0`), `CancelLogin` (`0x4dce60` →
    // `0x4d99e0`), `Screenshot` (`0x4dc520`), `QuitGameAndRunLauncher` (`0x4dc060`, which quits:
    // benilla has no launcher), `ReadyForAccountDataTimes` (`0x4de390`), and the realm split
    // query and answer (`0x4de2e0`, `0x4de250`), each a `CMSG_REALM_SPLIT`.
    g.set(
        "EnterWorld",
        lua.create_function(|lua, _: MultiValue| {
            queue(lua, GlueRequest::EnterWorld);
            Ok(())
        })?,
    )?;
    g.set(
        "CancelLogin",
        lua.create_function(|lua, _: MultiValue| {
            queue(lua, GlueRequest::CancelLogin);
            Ok(())
        })?,
    )?;
    g.set(
        "Screenshot",
        lua.create_function(|lua, _: MultiValue| {
            queue(lua, GlueRequest::Screenshot);
            Ok(())
        })?,
    )?;
    g.set(
        "QuitGameAndRunLauncher",
        lua.create_function(|lua, _: MultiValue| {
            queue(lua, GlueRequest::QuitGame);
            Ok(())
        })?,
    )?;
    g.set(
        "ReadyForAccountDataTimes",
        lua.create_function(|lua, _: MultiValue| {
            queue(lua, GlueRequest::ReadyForAccountDataTimes);
            Ok(())
        })?,
    )?;
    g.set(
        "RequestRealmSplitInfo",
        lua.create_function(|lua, _: MultiValue| {
            queue(lua, GlueRequest::RealmSplit(0xFFFF_FFFF));
            Ok(())
        })?,
    )?;
    g.set(
        "SetRealmSplitState",
        lua.create_function(|lua, v: Value| {
            let Some(state) = lua.coerce_number(v)? else {
                return Err(mlua::Error::runtime("Usage: SetRealmSplitState(1 or 2)"));
            };
            let state = super::binding_abi::ftol(state) as u32;
            if state <= 2 {
                queue(lua, GlueRequest::RealmSplit(state));
            }
            Ok(())
        })?,
    )?;
    Ok(())
}
