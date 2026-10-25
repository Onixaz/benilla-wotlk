//! The CVar table: the reference's store of case-insensitively named string settings with
//! registered defaults, behind `GetCVar`/`SetCVar`/`GetCVarDefault`/`RegisterCVar`.
//!
//! The host registers the vars it backs and drains Lua's writes from a change queue; its own
//! writes do not queue. The table dies with the VM, where the reference's outlives every
//! `ReloadUI`, so the host seeds each new VM with the saved values before anything registers.
//! An unknown name warns once and is ignored (`GetCVar` answers nil), where the reference's
//! `GetCVar`, `SetCVar` and `GetCVarDefault` raise `Couldn't find CVar named '%s'` (`0x842310`).

use mlua::{Lua, Value};

use super::binding_abi::lua_number_text;
use super::{Model, ScriptValue};

/// One registered CVar.
#[derive(Clone, Debug)]
pub(crate) struct CvarSlot {
    /// The registered spelling, reported on change events and in snapshots; the table key is
    /// its lowercase form.
    pub name: String,
    pub value: String,
    pub default: String,
    /// The reference's flag bit1 (`rec+0x1c & 0x2`): a write goes to [`Self::pending`] and
    /// `GetCVar` answers the applied value until the host commits it (`CVar::Update 0x63e060`).
    /// Host-seeded only; a VM-registered row is never latched, like a console-created one.
    pub latched: bool,
    /// A latched row's staged value, reported to the host through the change queue when written.
    pub pending: Option<String>,
}

/// One row of the host's registry as the VM's mirror is seeded from it; `value` is the applied
/// value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeededCvar {
    pub name: String,
    pub value: String,
    pub default: String,
    pub latched: bool,
}

/// One multisample format the device supports, the reference's triple (`[0xb4b444]`, count
/// `[0xb4b440]`, built by `0x48c3e0`). `samples == 1` means no multisampling, as both reference
/// enumerators normalise it (D3D `0x58b8b1`, GL `0x58d5fa`) and the device paths test `> 1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MultisampleFormat {
    pub color_bits: u32,
    pub depth_bits: u32,
    pub samples: u32,
}

/// One screen resolution the Video options dropdown offers, a monitor mode's size. `"WxH"` is a
/// contract: `VideoOptionsPanels.lua:216-218` splits on the `x` and `CT_Viewport.lua:107`
/// matches `"(%d+)x(%d+)"`, both failing silently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenResolution {
    pub width: u32,
    pub height: u32,
}

impl ScreenResolution {
    /// Pixel area first, the reference's whole comparator (`0x58be40`), then width, then height.
    /// Deviation: the tie-break is ours, because area alone with the reference's adjacent-only
    /// dedupe can leave a duplicate `"WxH"` when two modes share an area (1280x960, 1600x768).
    pub(crate) fn sort_key(&self) -> (u64, u32, u32) {
        (
            u64::from(self.width) * u64::from(self.height),
            self.width,
            self.height,
        )
    }
}

impl std::fmt::Display for ScreenResolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

/// One listed size and the refresh rates the monitor offers at it, ascending and deduplicated
/// (`GetRefreshRates 0x54f690`); `[0]` where the platform has no mode-set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenMode {
    pub size: ScreenResolution,
    pub refresh_rates: Vec<u32>,
}

/// What `GetVideoCaps` answers, in `OptionsFrame.lua:59`'s order: anisotropic, pixel shaders,
/// vertex shaders, trilinear, triple buffering, max anisotropy, hardware cursor. The host pushes
/// what wgpu and this client's presentation path really offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct VideoCaps {
    pub anisotropic: bool,
    pub pixel_shaders: bool,
    pub vertex_shaders: bool,
    pub trilinear: bool,
    /// A backend constant, not a device capability: 1 under Direct3D, 0 under OpenGL
    /// (`caps+0x20`, read only by `GetVideoCaps 0x48db40`). False here, pushed as the number 0.
    pub triple_buffering: bool,
    /// The raw maximum: `OptionsFrame.lua:122-127` makes a value in `ANISOTROPIC_VALUES` the
    /// slider's ceiling as its index, and any other number the ceiling as is. Pushed as nil when 0.
    pub max_anisotropy: u32,
    pub hardware_cursor: bool,
}

impl super::UiScript {
    /// Hand registration the config file's saved values (keys lowercased), before any
    /// `register_cvars` or `RegisterCVar` runs in this VM: a name registered while present here
    /// starts at its saved value. Without it a knobless or addon-declared CVar would revert to its
    /// default in every new VM, and the saver would then drop the player's setting from the file.
    pub fn set_cvar_saved_base(&mut self, entries: impl IntoIterator<Item = (String, String)>) {
        set_field!(
            self,
            cvars_saved_base,
            entries
                .into_iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), v))
                .collect()
        );
    }

    /// Register the host-backed CVars as `(name, default)`, idempotently: a re-register refreshes
    /// the default and keeps a live value, and a new row starts at its saved value if the config
    /// file has one. `&self` so the interface loader can register from a `&UiScript`: the stock
    /// `UIOptionsFrame.xml` reads CVars in its own `OnLoad`.
    pub fn register_cvars<'a>(&self, vars: impl IntoIterator<Item = (&'a str, &'a str)>) {
        let mut model = self.model_mut();
        for (name, default) in vars {
            let key = name.to_ascii_lowercase();
            match model.cvars.get_mut(&key) {
                Some(slot) => slot.default = default.to_string(),
                None => {
                    let value = model
                        .cvars_saved_base
                        .get(&key)
                        .cloned()
                        .unwrap_or_else(|| default.to_string());
                    model.cvars.insert(
                        key,
                        CvarSlot {
                            name: name.to_string(),
                            value,
                            default: default.to_string(),
                            latched: false,
                            pending: None,
                        },
                    );
                }
            }
        }
    }

    /// Seed the mirror from the host's registry, creating or overwriting each row. The host's
    /// table outlives the VM, so a re-seed re-asserts it and drops any staged value (the host
    /// holds the pending copy). `&self` for [`Self::register_cvars`]'s reason.
    pub fn seed_cvars(&self, rows: impl IntoIterator<Item = SeededCvar>) {
        let mut model = self.model_mut();
        for row in rows {
            let key = row.name.to_ascii_lowercase();
            model.cvars.insert(
                key,
                CvarSlot {
                    name: row.name,
                    value: row.value,
                    default: row.default,
                    latched: row.latched,
                    pending: None,
                },
            );
        }
    }

    /// Host-side write (a loaded value, a committed latch): sets the value and clears any staged
    /// one without queueing a change, since an echo would re-dirty the file the host just read.
    /// An unknown name warns once.
    pub fn set_cvar_host(&mut self, name: &str, value: &str) {
        let mut model = self.model_mut();
        let key = name.to_ascii_lowercase();
        match model.cvars.get_mut(&key) {
            Some(slot) => {
                slot.value = value.to_string();
                slot.pending = None;
            }
            None => warn_unknown(&mut model, name),
        }
    }

    /// Read one CVar's live value (host side).
    pub fn cvar(&self, name: &str) -> Option<String> {
        self.model_ref()
            .cvars
            .get(&name.to_ascii_lowercase())
            .map(|s| s.value.clone())
    }

    /// Snapshot the table as `(name, value, default)`; the saver writes only rows off their
    /// default, so the config file stays a diff.
    pub fn cvars_snapshot(&self) -> Vec<(String, String, String)> {
        self.model_mut()
            .cvars
            .values()
            .map(|s| (s.name.clone(), s.value.clone(), s.default.clone()))
            .collect()
    }

    /// Drain the `(name, new_value)` changes queued since the last call, under the registered
    /// spelling: the host's cue to sync its resources and mark the config dirty.
    pub fn take_cvar_changes(&mut self) -> Vec<(String, String)> {
        drain_field!(self, cvar_changes)
    }

    /// Drain the `(name, default)` rows an addon's `RegisterCVar` created since the last call; the
    /// host's registry keeps them beside its own, as the reference's one table does.
    pub fn take_cvar_registrations(&mut self) -> Vec<(String, String)> {
        drain_field!(self, cvar_registrations)
    }

    /// Flag a CVar read-only to Lua, or clear it: `CVar::SetReadOnly` (`0x63e030`). A flagged name
    /// makes `SetCVar` raise `"%s" is read-only` (`0x488c67`-`0x488c78`); host and engine writes,
    /// and `ConsoleExec`, still land, as the reference's `CVar::Set` (`0x63df50`) never tests the
    /// flag. The name need not be registered yet.
    pub fn set_cvar_read_only(&mut self, name: &str, read_only: bool) {
        let key = name.to_ascii_lowercase();
        let mut model = self.model_mut();
        if read_only {
            model.cvars_read_only.insert(key);
        } else {
            model.cvars_read_only.remove(&key);
        }
    }

    /// A native widget's write (the glue AddOns screen's "Load out of date AddOns" checkbox): sets
    /// the value and queues the change like a Lua `SetCVar`, so the config file is dirtied.
    pub fn set_cvar_engine(&mut self, name: &str, value: &str) {
        set_from_engine(&mut self.model_mut(), name, value.to_string());
    }

    /// Publish the multisample formats this run's device accepts, in dropdown order. The
    /// reference's `0x48c3e0` sweeps its candidate table `0x85a83c` (D3D) or every pixel format
    /// (GL); here the host asks wgpu.
    pub fn set_multisample_formats(&mut self, formats: Vec<MultisampleFormat>) {
        set_field!(self, multisample_formats, formats);
    }

    /// Publish the dropdown's sizes, each with no refresh rate: [`Self::set_screen_modes`] for a
    /// caller with no monitor modes to hand.
    pub fn set_screen_resolutions(
        &mut self,
        offered: Vec<ScreenResolution>,
        current: Option<ScreenResolution>,
    ) {
        let modes = offered
            .into_iter()
            .map(|size| ScreenMode {
                size,
                refresh_rates: Vec::new(),
            })
            .collect();
        self.set_screen_modes(modes, current);
    }

    /// Publish the dropdown's modes and the client's current size, sorted by
    /// [`ScreenResolution::sort_key`] and deduplicated (a duplicate's rates merge). `current` is
    /// inserted when the list lacks it (a window sized by hand): `CT_Viewport.lua:201` reads
    /// `arg[GetCurrentResolution()]` and silently falls back to 4:3 on a miss.
    pub fn set_screen_modes(
        &mut self,
        mut offered: Vec<ScreenMode>,
        current: Option<ScreenResolution>,
    ) {
        offered.sort_by_key(|m| m.size.sort_key());
        let mut merged: Vec<ScreenMode> = Vec::with_capacity(offered.len());
        for m in offered {
            match merged.last_mut() {
                Some(last) if last.size == m.size => last.refresh_rates.extend(m.refresh_rates),
                _ => merged.push(m),
            }
        }
        for m in &mut merged {
            m.refresh_rates.sort_unstable();
            m.refresh_rates.dedup();
        }
        let current = current.map(|c| {
            merged.iter().position(|m| m.size == c).unwrap_or_else(|| {
                let at = merged.partition_point(|m| m.size.sort_key() < c.sort_key());
                merged.insert(
                    at,
                    ScreenMode {
                        size: c,
                        refresh_rates: Vec::new(),
                    },
                );
                at
            })
        });
        let mut model = self.model_mut();
        model.screen_modes = merged;
        model.current_resolution = current;
    }

    /// Publish what this run's device and presentation path offer, behind `GetVideoCaps`.
    pub fn set_video_caps(&mut self, caps: VideoCaps) {
        set_field!(self, video_caps, caps);
    }

    /// Drain the `RestartGx()` calls queued since the last call; the host answers each by
    /// re-asserting the staged video settings against the window.
    pub fn take_restart_gx_asks(&mut self) -> u32 {
        drain_field!(self, restart_gx_asks)
    }
}

/// An engine-side write, for a value the engine owns: queued like a Lua `SetCVar` so the config
/// file is dirtied, as the reference's minimap `set_zoom` (`0x6da8e0`) also `CVar::Set`s
/// `minimapZoom`/`minimapInsideZoom`. An unregistered name is a silent no-op (a bare VM).
pub(super) fn set_from_engine(model: &mut Model, name: &str, value: String) {
    store(model, &name.to_ascii_lowercase(), value);
}

/// The one store every write goes through; returns the registered spelling when the value moved
/// and the change was queued. A latched slot stages the write, as the reference's `Set 0x63df50`
/// stores `latchedValue` and skips `InternalSet`; writing back the applied value unstages it.
fn store(model: &mut Model, key: &str, value: String) -> Option<String> {
    let slot = model.cvars.get_mut(key)?;
    if slot.latched {
        let staged = (value != slot.value).then_some(value.clone());
        if slot.pending == staged && staged.is_some() {
            return None;
        }
        if staged.is_none() && slot.pending.is_none() {
            return None;
        }
        slot.pending = staged;
    } else {
        if slot.value == value {
            return None;
        }
        slot.value = value.clone();
    }
    let registered = slot.name.clone();
    model.cvar_changes.push((registered.clone(), value));
    Some(registered)
}

/// A by-name write from a binding (`SetCVar`, `ConsoleExec`, `SetGamma`, `SetWorldDetail`): the
/// change is queued only when the value moved. `false`, with a warning, for an unknown name.
pub(super) fn write_cvar(model: &mut Model, name: &str, value: String) -> bool {
    let key = name.to_ascii_lowercase();
    if !model.cvars.contains_key(&key) {
        warn_unknown(model, name);
        return false;
    }
    if store(model, &key, value).is_some() {
        super::calendar::filter_cvar_written(model, &key);
    }
    true
}

/// A read verb's lookup through a shared borrow, so a read repeated every frame leaves the model's
/// mutation count standing; an unknown name takes the mutable borrow only for its one warning.
fn read_cvar<T>(lua: &Lua, name: &str, read: impl FnOnce(&CvarSlot) -> T) -> Option<T> {
    let key = name.to_ascii_lowercase();
    let warned = {
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model app_data set");
        if let Some(slot) = model.cvars.get(&key) {
            return Some(read(slot));
        }
        model.cvars_warned.contains(&key)
    };
    if !warned {
        warn_unknown(&mut crate::script::model_mut_of(lua), name);
    }
    None
}

fn warn_unknown(model: &mut Model, name: &str) {
    let key = name.to_ascii_lowercase();
    if model.cvars_warned.insert(key) {
        model.record_warning(format!(
            "unknown CVar '{name}' (not host-registered) — ignored"
        ));
    }
}

/// An argument as the CVar bindings read it, `lua_tostring` (`0x6f3690`): a string as it is, a
/// number as Lua's `%.14g` text (`luaV_tostring` `0x6f7c80`), so the stock Sound slider's
/// single-precision 0.4 stores `"0.40000000596046"`. `None` for anything `lua_isstring`
/// (`0x6f3510`) refuses: nil, an absent argument, a boolean, a table or a function.
fn cvar_arg(v: Option<&Value>) -> Option<String> {
    match v? {
        // Lossy: the reference takes bytes, and invalid UTF-8 must not read as absent.
        Value::String(s) => Some(s.to_string_lossy()),
        Value::Integer(i) => Some(lua_number_text(*i as f64)),
        Value::Number(n) => Some(lua_number_text(*n)),
        _ => None,
    }
}

/// A value as the video setters print it, `SStrPrintf(buf, 0x10, "%f", v)` (`0x64a7f0` with the
/// format at `0x835160`): six decimals in a 16-byte buffer, so the stored string is cut at 15
/// characters.
pub(super) fn format_f(v: f64) -> String {
    let mut written = format!("{v:.6}");
    written.truncate(15);
    written
}

/// Storm's `SStrToInt` (`0x64ac60`), how a CVar record's integer (`rec+0x28`) is parsed from its
/// value and how a change callback reads it: an optional `-`, then decimal digits up to the first
/// other byte, with no whitespace skip, no `+` and no overflow check (`10·n + d` wraps); no digit
/// is 0.
pub fn sstr_to_int(s: &str) -> i32 {
    let (neg, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let n = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i32, |n, d| {
            n.wrapping_mul(10).wrapping_add(i32::from(d - b'0'))
        });
    if neg {
        n.wrapping_neg()
    } else {
        n
    }
}

/// How 3.3.5a reads a CVar value as a boolean (`0x815400`, default false): by its first byte, a
/// digit other than `0` or one of `tTyY` is true and `0` or one of `fFnN` false; any other first
/// byte, the empty string included, is false unless the whole value is `on` or `enabled`, in any
/// case (`off` and `disabled` are tested first, also false).
fn cvar_truth(value: &str) -> bool {
    match value.as_bytes().first() {
        Some(b'0' | b'f' | b'F' | b'n' | b'N') => false,
        Some(b'1'..=b'9' | b't' | b'T' | b'y' | b'Y') => true,
        _ => value.eq_ignore_ascii_case("on") || value.eq_ignore_ascii_case("enabled"),
    }
}

/// What `SetCVar` and `RegisterCVar` store for a value with no string form (`0x82e570`, loaded
/// at `0x488c98` and `0x488b6f`).
const NO_STRING_FORM: &str = "0";

/// `0x84235c`, verbatim, the unbalanced parenthesis included.
const USAGE_SET_CVAR: &str = "Usage: SetCVar(\"cvar\", value [, \"scriptCvar\")";

/// 3.3.5a `0x9f5758`, verbatim.
const USAGE_GET_CVAR_BOOL: &str = "Usage: GetCVarBool(\"cvar\")";

/// `0x8422e8`, verbatim.
const USAGE_REGISTER_CVAR: &str = "Usage: RegisterCVar(\"cvar\" [, default])";

/// The CVars `CGGameUI::InitializeGame` flags read-only for the whole world session, the table
/// at `0x83de4c` walked by `0x48f566`-`0x48f584`; `ShutdownGame` clears them again
/// (`0x491240`-`0x49125d`). The host flags them on each in-game VM before its UI loads, as
/// `InitializeGame` does before `UI_Init`.
pub const IN_WORLD_READ_ONLY_CVARS: [&str; 3] = ["realmList", "realmName", "scriptMemory"];

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // `RegisterCVar(name, value)` (`0x488b00`) declares a CVar the client does not ship, which is
    // how an addon persists a setting. Re-declaring a live name is a no-op: the reference never
    // resets a live value on re-registration (`0x488b7d`).
    lua.globals().set(
        "RegisterCVar",
        lua.create_function(|lua, args: mlua::MultiValue| {
            // `lua_isstring(1)` (`0x488b09`): a string or a number names it.
            let Some(name) = cvar_arg(args.front()) else {
                return Err(mlua::Error::runtime(USAGE_REGISTER_CVAR));
            };
            // The default only when `lua_isstring(2)` passes (`0x488b56`), else "0".
            let value = cvar_arg(args.get(1)).unwrap_or_else(|| NO_STRING_FORM.to_string());
            let mut model = crate::script::model_mut_of(lua);
            let key = name.to_ascii_lowercase();
            // A saved value outranks the declared one, which stays the default for the saver.
            let saved = model.cvars_saved_base.get(&key).cloned();
            if model.cvars.contains_key(&key) {
                return Ok(());
            }
            // The host gives the row a place in its registry, which outlives this VM.
            model.cvar_registrations.push((name.clone(), value.clone()));
            model.cvars.insert(
                key,
                CvarSlot {
                    name,
                    value: saved.unwrap_or_else(|| value.clone()),
                    default: value,
                    latched: false,
                    pending: None,
                },
            );
            Ok(())
        })?,
    )?;

    install_cvar_verbs(lua)?;
    super::glue_system::install_cvar_ranges(lua)?;
    // `CanUseEquipmentSets()` (`0x5ad880`): the number 1 while `equipmentManager` reads non-zero,
    // else 0.
    lua.globals().set(
        "CanUseEquipmentSets",
        lua.create_function(|lua, _: mlua::MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let on = model
                .cvars
                .get("equipmentmanager")
                .is_some_and(|s| sstr_to_int(&s.value) != 0);
            Ok(if on { 1.0 } else { 0.0 })
        })?,
    )?;
    // `RestoreVideoResolutionDefaults` (`0x510dc0`) and `RestoreVideoEffectsDefaults` (`0x510dd0`),
    // `ConsoleDeviceDefaults(page)` (`0x76a130`) for pages 0 and 1: page 0 rewrites the device
    // format's `gx*` rows and restarts the device at once (`CCGxRestart`), then each page runs its
    // defaults callbacks (`Client.cpp`'s `SetDefaults 0x4048a0`, `CWorldParam::SetDefaults
    // 0x78e1a0`). Each row goes back to its registered default here, which for the hardware-detected
    // rows is the matched `VideoHardware.dbc` row's value (the host table's `overridden` rows).
    lua.globals().set(
        "RestoreVideoResolutionDefaults",
        lua.create_function(|lua, _: mlua::MultiValue| {
            let mut model = crate::script::model_mut_of(lua);
            restore_defaults(&mut model, VIDEO_RESOLUTION_DEFAULT_CVARS);
            model.restart_gx_asks += 1;
            Ok(())
        })?,
    )?;
    lua.globals().set(
        "RestoreVideoEffectsDefaults",
        lua.create_function(|lua, _: mlua::MultiValue| {
            let mut model = crate::script::model_mut_of(lua);
            restore_defaults(&mut model, VIDEO_EFFECTS_DEFAULT_CVARS);
            Ok(())
        })?,
    )?;
    // `RestoreVideoStereoDefaults` (`0x4dd420`): the stereo panel's three rows.
    lua.globals().set(
        "RestoreVideoStereoDefaults",
        lua.create_function(|lua, _: mlua::MultiValue| {
            let mut model = crate::script::model_mut_of(lua);
            restore_defaults(
                &mut model,
                &[
                    "gxStereoEnabled",
                    "gxStereoConvergence",
                    "gxStereoSeparation",
                ],
            );
            Ok(())
        })?,
    )?;

    super::video_pairs::install(lua)
}

/// The CVar verbs both states register, each a binding of its own (glue table `0xac3e00`, in-game
/// `0xac80b0`) with the same behaviour: `GetCVar`, `GetCVarDefault`, `GetCVarBool` and `SetCVar`.
pub(super) fn install_cvar_verbs(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "GetCVar",
        lua.create_function(|lua, name: String| {
            match read_cvar(lua, &name, |slot| slot.value.clone()) {
                Some(value) => Ok(Value::String(lua.create_string(&value)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    lua.globals().set(
        "GetCVarDefault",
        lua.create_function(|lua, name: String| {
            match read_cvar(lua, &name, |slot| slot.default.clone()) {
                Some(default) => Ok(Value::String(lua.create_string(&default)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    // `GetCVarBool(name)` (world `0x5100c0`): 1 when the value reads as true, else
    // nil. A name `lua_isstring` refuses raises the usage; an unknown name is nil, unraised.
    lua.globals().set(
        "GetCVarBool",
        lua.create_function(|lua, args: mlua::MultiValue| {
            let Some(name) = cvar_arg(args.front()) else {
                return Err(mlua::Error::runtime(USAGE_GET_CVAR_BOOL));
            };
            let truth = read_cvar(lua, &name, |slot| cvar_truth(&slot.value)).unwrap_or(false);
            Ok(if truth {
                Value::Number(1.0)
            } else {
                Value::Nil
            })
        })?,
    )?;
    // `SetCVar(name, value [, token])` (`0x488c10`).
    lua.globals().set(
        "SetCVar",
        lua.create_function(|lua, args: mlua::MultiValue| {
            // `lua_isstring(1)` (`0x488c1a`): a string or a number names it.
            let Some(name) = cvar_arg(args.front()) else {
                return Err(mlua::Error::runtime(USAGE_SET_CVAR));
            };
            // A value with no string form stores "0" (`0x488c8d`-`0x488c98`): an unchecked
            // CheckButton's `GetChecked()` is nil, and `SoundOptionsCheckButton_OnClick` writes
            // it as is.
            let value = cvar_arg(args.get(1)).unwrap_or_else(|| NO_STRING_FORM.to_string());
            {
                let mut model = crate::script::model_mut_of(lua);
                // Flag bit2 (`0x488c67`), tested before anything is written.
                if model.cvars_read_only.contains(&name.to_ascii_lowercase()) {
                    return Err(mlua::Error::runtime(format!("\"{name}\" is read-only")));
                }
                if !write_cvar(&mut model, &name, value.clone()) {
                    return Ok(());
                }
            }
            // A third argument that passes `lua_isstring` is the `CVAR_UPDATE` token, arg1, with
            // the value as arg2 (`0x488cad`-`0x488cd5`), fired on every such call, changed or not,
            // and inside it: `SignalEvent2` (`0x703f50`) walks the listeners in place. The 1.12
            // options panels pass their CheckButtons key, such as "STATUS_BAR_TEXT"
            // (`UIOptionsFrame.lua` l.335), which is how `UIOptionsFrame_OnEvent` finds its row.
            if let Some(token) = cvar_arg(args.get(2)) {
                super::tick::fire_event_into(
                    lua,
                    "CVAR_UPDATE",
                    vec![ScriptValue::Str(token), ScriptValue::Str(value)],
                );
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

/// The video options table both states register (`CGVideoOptions::RegisterScriptFunctions`,
/// `0x54ec00`, table `0xacd320`; the glue manager's `Resume` `0x4da5f0` calls it too), with
/// `RestartGx`, which both states' own tables carry.
pub(super) fn install_video_options(lua: &Lua) -> mlua::Result<()> {
    // ── The Video options Multisampling dropdown ──────────────────────────────────────────────
    // `VideoOptionsResolutionPanelMultiSampleDropDown_OnLoad` seeds the selection from
    // `GetCurrentMultisampleFormat()`, `_Initialize` walks `GetMultisampleFormats()` three values at
    // a time, and the Okay handler calls `SetMultisampleFormat` (`VideoOptionsPanels.lua:314-366`).
    lua.globals().set(
        "GetMultisampleFormats",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            // Flat, three per entry (`0x54f820`); no formats is no values and an empty menu.
            let mut out = mlua::MultiValue::new();
            for f in &model.multisample_formats {
                out.push_back(Value::Number(f.color_bits as f64));
                out.push_back(Value::Number(f.depth_bits as f64));
                out.push_back(Value::Number(f.samples as f64));
            }
            Ok(out)
        })?,
    )?;
    lua.globals().set(
        "GetCurrentMultisampleFormat",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let cvar = |n: &str| model.cvars.get(n).map(|s| sstr_to_int(&s.value));
            let (Some(color), Some(depth), Some(samples)) = (
                cvar("gxcolorbits"),
                cvar("gxdepthbits"),
                cvar("gxmultisample"),
            ) else {
                return Ok(1.0f64);
            };
            // 1-based, and 1.0 on no match (`0x54f8b0`): `_OnLoad` hands it straight to
            // `UIDropDownMenu_SetSelectedID`, so a miss must still name a real row.
            let idx = model
                .multisample_formats
                .iter()
                .position(|f| {
                    f.color_bits as i32 == color
                        && f.depth_bits as i32 == depth
                        && f.samples as i32 == samples
                })
                .map_or(1.0, |i| (i + 1) as f64);
            Ok(idx)
        })?,
    )?;
    lua.globals().set(
        "SetMultisampleFormat",
        lua.create_function(|lua, id: Value| {
            let mut model = crate::script::model_mut_of(lua);
            // 1-based; anything `lua_isnumber` refuses selects the first entry (`0x54f980`).
            // Deviation: an id past the count is ignored, because the reference then reads one
            // entry past the last, and a clamp would apply a format the player did not pick.
            let n = lua.coerce_number(id).ok().flatten().map_or(1.0, f64::trunc);
            let Some(f) = (n >= 1.0)
                .then(|| model.multisample_formats.get(n as usize - 1).copied())
                .flatten()
            else {
                return Ok(());
            };
            // All three from the chosen entry when any differs, staged for `RestartGx`, which the
            // panel's `restart` flag asks for after Okay.
            set_from_engine(&mut model, "gxColorBits", f.color_bits.to_string());
            set_from_engine(&mut model, "gxDepthBits", f.depth_bits.to_string());
            set_from_engine(&mut model, "gxMultisample", f.samples.to_string());
            Ok(())
        })?,
    )?;
    install_video_verbs(lua)
}

/// `ConsoleDeviceDefaults(0)`'s rows: the device format `SetGxCVars` (`0x769950`) writes, then
/// `Client.cpp`'s page-0 callback (`DesktopGamma`, `Gamma`, `windowResizeLock`).
pub const VIDEO_RESOLUTION_DEFAULT_CVARS: &[&str] = &[
    "gxColorBits",
    "gxDepthBits",
    "gxWindow",
    "gxResolution",
    "gxRefresh",
    "gxTripleBuffer",
    "gxVSync",
    "gxMaximize",
    "gxCursor",
    "gxMultisample",
    "gxFixLag",
    "DesktopGamma",
    CVAR_GAMMA,
    "windowResizeLock",
];

/// `ConsoleDeviceDefaults(1)`'s rows: `CWorldParam::SetDefaults` (`0x78e1a0`) and `Client.cpp`'s
/// page-1 callback (`textureFilteringMode`), among those benilla registers.
pub const VIDEO_EFFECTS_DEFAULT_CVARS: &[&str] = &[
    "farclip",
    "shadowLevel",
    "specular",
    "particleDensity",
    "baseMip",
    "groundEffectDensity",
    "environmentDetail",
    "groundEffectDist",
    "weatherDensity",
    "extShadowQuality",
    "ffxDeath",
    "ffxGlow",
    "projectedTextures",
    "textureFilteringMode",
];

/// Put each registered name in `names` back at its default through the change queue; a name a bare
/// VM never registered is skipped.
fn restore_defaults(model: &mut Model, names: &[&str]) {
    for name in names {
        let Some(default) = model
            .cvars
            .get(&name.to_ascii_lowercase())
            .map(|slot| slot.default.clone())
        else {
            continue;
        };
        set_from_engine(model, name, default);
    }
}

/// The Video options window's engine verbs that nothing else here provides. Three run at load,
/// as `UIDropDownMenu_Initialize` calls its initializer at once (`UIDropDownMenu.lua:48-50`):
/// `GetScreenResolutions`, `GetCurrentResolution` and `GetRefreshRates`.
///
/// Ten names must stay nil so their slider rows fall back to `GetCVar`/`SetCVar`
/// (`OptionsFrame_Load:110` tries `getglobal("Get"..value.func)`): `Getuiscale`, `Getfarclip`,
/// `Getanisotropic`, `GetspellEffectLevel`, `GetweatherDensity` and their setters.
/// `GetFarclip`/`SetFarclip` (`0x488f00`/`0x488f30`, in [`super::video_pairs`]) do exist;
/// `getglobal` is case-sensitive.
fn install_video_verbs(lua: &Lua) -> mlua::Result<()> {
    // `GetTerrainMip`/`SetTerrainMip` (`0x54eb10`/`0x54eb40`): the terrain detail slider over
    // `shadowLevel`, inverted (`1 - shadowLevel`); the setter takes a number or raises its usage.
    lua.globals().set(
        "GetTerrainMip",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let level = model
                .cvars
                .get("shadowlevel")
                .map_or(0, |s| super::glue_core::leading_int(&s.value));
            Ok((1 - level) as f64)
        })?,
    )?;
    lua.globals().set(
        "SetTerrainMip",
        lua.create_function(|lua, v: Value| {
            let Some(n) = lua.coerce_number(v)? else {
                return Err(mlua::Error::runtime("Usage: SetTerrainMip(value)"));
            };
            let value = 1 - super::binding_abi::ftol(n);
            set_from_engine(
                &mut crate::script::model_mut_of(lua),
                "shadowLevel",
                value.to_string(),
            );
            Ok(())
        })?,
    )?;
    // `IsStereoVideoAvailable` (`0x54ef90`): the device's stereo capability, which benilla's
    // renderer does not offer.
    lua.globals().set(
        "IsStereoVideoAvailable",
        lua.create_function(|_, ()| Ok(Value::Nil))?,
    )?;
    // `IsPlayerResolutionAvailable` (`0x54ebc0`): 1 on a machine with more than one processor,
    // which every machine benilla runs on is.
    lua.globals().set(
        "IsPlayerResolutionAvailable",
        lua.create_function(|_, ()| Ok(1.0))?,
    )?;
    // ── GetScreenResolutions ─────────────────────────────────────────────────────────────────
    // A vararg of `"WxH"` strings (`0x54f430`), the spelling `ScreenResolution` pins.
    lua.globals().set(
        "GetScreenResolutions",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let mut out = mlua::MultiValue::new();
            for m in &model.screen_modes {
                out.push_back(Value::String(lua.create_string(m.size.to_string())?));
            }
            Ok(out)
        })?,
    )?;
    // ── GetCurrentResolution ─────────────────────────────────────────────────────────────────
    // The 1-based index of `gxResolution` in that list (`0x54f4a0`), and 1.0 on any miss, never
    // nil or 0, so a 1 does not prove a match. The host publishes the entry; for benilla's
    // unpicked "" it is the size in use (`set_screen_modes`).
    lua.globals().set(
        "GetCurrentResolution",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.current_resolution.map_or(1.0, |i| (i + 1) as f64))
        })?,
    )?;
    // ── SetScreenResolution ──────────────────────────────────────────────────────────────────
    // 1-based (`0x54f570`): anything `lua_isnumber` refuses selects the first entry, and a size
    // equal to `gxResolution`'s writes nothing. It asks for no restart: the panel's `restart` flag
    // calls `RestartGx` after Okay.
    //
    // Deviation: an index past the count clamps to the last, because the reference reads one
    // element past it there, uninitialised memory.
    lua.globals().set(
        "SetScreenResolution",
        lua.create_function(|lua, id: Value| {
            let n = lua.coerce_number(id).ok().flatten().map_or(1.0, f64::trunc);
            let mut model = crate::script::model_mut_of(lua);
            if model.screen_modes.is_empty() {
                return Ok(());
            }
            let last = model.screen_modes.len() - 1;
            let r = model.screen_modes[if n < 1.0 {
                0
            } else {
                (n as usize - 1).min(last)
            }]
            .size;
            let want = r.to_string();
            if model
                .cvars
                .get("gxresolution")
                .is_some_and(|slot| slot.value.eq_ignore_ascii_case(&want))
            {
                return Ok(());
            }
            set_from_engine(&mut model, "gxResolution", want);
            Ok(())
        })?,
    )?;
    // ── GetRefreshRates ─────────────────────────────────────────────────────────────────────
    // The rates the monitor offers at one listed size, ascending and deduplicated (`0x54f690`):
    // the size is the argument's 1-based entry, or the first without a number, which is how the
    // stock dropdown calls it (`VideoOptionsPanels.lua:274`). An index past the count answers no
    // values. A platform with no mode-set publishes the single rate 0, the stock panel's sentinel
    // for a disabled dropdown (`:280`).
    lua.globals().set(
        "GetRefreshRates",
        lua.create_function(|lua, args: mlua::MultiValue| {
            let n = match args.front() {
                Some(v) => lua.coerce_number(v.clone()).ok().flatten(),
                None => None,
            };
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let idx = n.map_or(0, |v| (v.trunc() as i64 - 1).max(0) as usize);
            let mut out = mlua::MultiValue::new();
            if let Some(m) = model.screen_modes.get(idx) {
                for rate in &m.refresh_rates {
                    out.push_back(Value::Number(f64::from(*rate)));
                }
            }
            Ok(out)
        })?,
    )?;
    // ── GetVideoCaps ─────────────────────────────────────────────────────────────────────────
    // Seven values in `OptionsFrame.lua:59`'s order, never a boolean (`0x48db40`): slots 1-4 and
    // 7 are nil or the number 1, slot 5 is always a number, slot 6 is nil iff 0. Slot 5 is 0
    // under OpenGL and 1 under Direct3D; 0 is truthy, so `OptionsFrame_Load`'s two
    // `not hasTripleBuffering` tests never fire in the reference, and a nil would revive them.
    lua.globals().set(
        "GetVideoCaps",
        lua.create_function(|lua, ()| {
            let caps = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data")
                .video_caps;
            let flag = |b: bool| if b { Value::Number(1.0) } else { Value::Nil };
            let mut out = mlua::MultiValue::new();
            out.push_back(flag(caps.anisotropic));
            out.push_back(flag(caps.pixel_shaders));
            out.push_back(flag(caps.vertex_shaders));
            out.push_back(flag(caps.trilinear));
            // Slot 5: always a number, a straight-line `fild` (`0x48dbcb`).
            out.push_back(Value::Number(if caps.triple_buffering { 1.0 } else { 0.0 }));
            // Slot 6: nil iff 0; the reference's is seeded to 1 (`caps+0xac`), so never nil there.
            out.push_back(if caps.max_anisotropy == 0 {
                Value::Nil
            } else {
                Value::Number(caps.max_anisotropy as f64)
            });
            out.push_back(flag(caps.hardware_cursor));
            Ok(out)
        })?,
    )?;
    // ── RestartGx ────────────────────────────────────────────────────────────────────────────
    // "Apply the staged video settings now": the console command `gxRestart`, which commits the
    // latched `gx*` rows (`UpdateGxCVars 0x7698b0`) and rebuilds the device from them. Here it
    // queues a counted request, as `Screenshot()` does; the host commits its gx stages
    // (`Cvars::commit_latched`) and re-asserts them against the window and the world camera.
    // The reference runs each gx change callback at `SetCVar`, where it can veto the write; the
    // host runs it at the commit.
    lua.globals().set(
        "RestartGx",
        lua.create_function(|lua, ()| {
            crate::script::model_mut_of(lua).restart_gx_asks += 1;
            Ok(())
        })?,
    )?;
    // ── GetGamma / SetGamma ──────────────────────────────────────────────────────────────────
    // `GetGamma 0x4891c0` and `SetGamma 0x4891f0` work in the slider's offset, not the CVar's
    // unit: `0x4891d0` is FSUBR over the f64 1.0 at `[0x8015b8]`, so `GetGamma()` answers
    // `1.0 - gamma` and `SetGamma(v)` writes `gamma = 1.0 - v`; the registered `"1.0"` reads as
    // 0.0, the slider's centre. The reference clamps nowhere (`SetGamma(5)` writes
    // `"-4.000000"`); the clamp is the consumer's (`ui_gamma::GAMMA_RANGE`).
    lua.globals().set(
        "GetGamma",
        // Any argument is ignored (the getter never calls `lua_gettop`); one number comes back.
        lua.create_function(|lua, _: mlua::MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let gamma = model
                .cvars
                .get(&CVAR_GAMMA.to_ascii_lowercase())
                .and_then(|slot| slot.value.parse::<f64>().ok())
                // A bare VM with no `gamma` row answers as if the default stood; the reference
                // always finds its record (`0x63de30`).
                .unwrap_or(1.0);
            Ok(1.0 - gamma)
        })?,
    )?;
    lua.globals().set(
        "SetGamma",
        lua.create_function(|lua, value: Value| {
            // `lua_isnumber` (`0x4891fe`): a number or a numeric string; anything else raises
            // (`0x6f4940`).
            let v = match &value {
                Value::Integer(i) => *i as f64,
                Value::Number(n) => *n,
                Value::String(s) => {
                    match s.to_str().ok().and_then(|s| s.trim().parse::<f64>().ok()) {
                        Some(n) => n,
                        None => return Err(mlua::Error::runtime(USAGE_SET_GAMMA)),
                    }
                }
                _ => return Err(mlua::Error::runtime(USAGE_SET_GAMMA)),
            };
            let mut model = crate::script::model_mut_of(lua);
            write_cvar(&mut model, CVAR_GAMMA, format_f(1.0 - v));
            // Zero return values, not nil (`eax = 0` at every `ret`).
            Ok(mlua::MultiValue::new())
        })?,
    )?;
    Ok(())
}

/// The reference's display-gamma CVar: registered at `0x402d70` as `"1.0"`, flags 0, name
/// string `0x82e924`.
pub const CVAR_GAMMA: &str = "gamma";

/// `0x8424cc`, verbatim.
const USAGE_SET_GAMMA: &str = "Usage: SetGamma(value)";

#[cfg(test)]
mod tests {
    use super::{MultisampleFormat, SeededCvar};
    use crate::script::UiScript;

    fn script_with_volume() -> UiScript {
        let s = UiScript::new().unwrap();
        s.register_cvars([("MusicVolume", "0.4"), ("MasterVolume", "1.0")]);
        s
    }

    #[test]
    fn the_table_round_trips_and_queues_changes_case_insensitively() {
        let mut s = script_with_volume();
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("musicvolume")"#)
                .unwrap(),
            "0.4"
        );
        s.run(r#"SetCVar("MUSICVOLUME", 0.7)"#).unwrap();
        assert_eq!(
            s.take_cvar_changes(),
            vec![("MusicVolume".to_string(), "0.7".to_string())]
        );
        assert!(s.take_cvar_changes().is_empty(), "drained");
        assert_eq!(
            s.eval::<String>(r#"return GetCVarDefault("MusicVolume")"#)
                .unwrap(),
            "0.4"
        );
        assert_eq!(s.cvar("MusicVolume").as_deref(), Some("0.7"));
        s.run(r#"SetCVar("MusicVolume", "0.7")"#).unwrap();
        assert!(s.take_cvar_changes().is_empty());
    }

    /// The reference's `Set 0x63df50` on flag bit1: `GetCVar` (`rec+0x20`) answers the old value
    /// until `CVar::Update 0x63e060` commits, which here arrives as a host write.
    #[test]
    fn a_latched_row_stages_the_write_until_the_host_commits_it() {
        let mut s = UiScript::new().unwrap();
        s.seed_cvars([
            SeededCvar {
                name: "gxVSync".into(),
                value: "1".into(),
                default: "1".into(),
                latched: true,
            },
            SeededCvar {
                name: "MusicVolume".into(),
                value: "0.4".into(),
                default: "0.4".into(),
                latched: false,
            },
        ]);
        s.run(r#"SetCVar("gxVSync", 0)"#).unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("gxVSync")"#).unwrap(),
            "1",
            "the applied value stands until the boundary"
        );
        assert_eq!(
            s.take_cvar_changes(),
            vec![("gxVSync".to_string(), "0".to_string())],
            "the host hears the staged write"
        );
        // Staging the same value again is quiet; staging the applied value back clears the stage
        // and is reported, so the host drops its pending copy too.
        s.run(r#"SetCVar("gxVSync", "0")"#).unwrap();
        assert!(s.take_cvar_changes().is_empty());
        s.run(r#"SetCVar("gxVSync", "1")"#).unwrap();
        assert_eq!(
            s.take_cvar_changes(),
            vec![("gxVSync".to_string(), "1".to_string())]
        );
        assert!(
            s.take_cvar_changes().is_empty(),
            "nothing staged, nothing to say"
        );
        // The commit is a host write: the value moves, the stage clears, no echo.
        s.run(r#"SetCVar("gxVSync", 0)"#).unwrap();
        s.take_cvar_changes();
        s.set_cvar_host("gxVSync", "0");
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("gxVSync")"#).unwrap(),
            "0"
        );
        assert!(s.take_cvar_changes().is_empty());
        s.run(r#"SetCVar("MusicVolume", 0.7)"#).unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("MusicVolume")"#)
                .unwrap(),
            "0.7"
        );
    }

    #[test]
    fn an_addon_registration_is_reported_to_the_host_once() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"RegisterCVar("myAddonKnob", "7")"#).unwrap();
        s.run(r#"RegisterCVar("myAddonKnob", "9")"#).unwrap();
        assert_eq!(
            s.take_cvar_registrations(),
            vec![("myAddonKnob".to_string(), "7".to_string())]
        );
        assert!(s.take_cvar_registrations().is_empty(), "drained");
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("myAddonKnob")"#)
                .unwrap(),
            "7"
        );
    }

    /// The reference prints `CVar "%s" is "%s"` for a bare name (`0x63dde0`); that printing is the
    /// host's, while a valued write lands here at once.
    #[test]
    fn console_exec_writes_a_valued_cvar_and_hands_a_bare_name_to_the_host() {
        let mut s = script_with_volume();
        s.run(r#"ConsoleExec("MusicVolume 0.2")"#).unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("MusicVolume")"#)
                .unwrap(),
            "0.2"
        );
        assert!(
            s.take_console_lines().is_empty(),
            "a valued write is consumed here"
        );
        s.run(r#"ConsoleExec("MusicVolume")"#).unwrap();
        assert_eq!(s.take_console_lines(), vec!["MusicVolume".to_string()]);
    }

    #[test]
    fn host_writes_do_not_echo_and_snapshots_carry_defaults() {
        let mut s = script_with_volume();
        s.set_cvar_host("MasterVolume", "0.25");
        assert!(
            s.take_cvar_changes().is_empty(),
            "a host write must not re-dirty the config it just loaded"
        );
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("MasterVolume")"#)
                .unwrap(),
            "0.25"
        );
        let snap = s.cvars_snapshot();
        let master = snap.iter().find(|(n, _, _)| n == "MasterVolume").unwrap();
        assert_eq!((master.1.as_str(), master.2.as_str()), ("0.25", "1.0"));
    }

    #[test]
    fn unknown_names_warn_once_and_no_op() {
        let mut s = script_with_volume();
        assert!(s
            .eval::<bool>(r#"return GetCVar("bogusKnob") == nil"#)
            .unwrap());
        s.run(r#"SetCVar("bogusKnob", 1)"#).unwrap();
        s.run(r#"SetCVar("bogusKnob", 2)"#).unwrap();
        assert!(s.take_cvar_changes().is_empty());
        let warns: Vec<String> = s.take_warnings();
        assert_eq!(warns.len(), 1, "warn-once: {warns:?}");
        assert!(warns[0].contains("bogusKnob"));
        // Registration is idempotent and never clobbers a live value.
        s.set_cvar_host("MusicVolume", "0.9");
        s.register_cvars([("MusicVolume", "0.4")]);
        assert_eq!(s.cvar("MusicVolume").as_deref(), Some("0.9"));
    }
    #[test]
    fn registration_starts_at_the_saved_value_not_the_default() {
        let mut s = UiScript::new().unwrap();
        s.set_cvar_saved_base([("StatusBarText".to_string(), "1".to_string())]);
        s.register_cvars([("statusBarText", "0"), ("farclip", "500")]);

        assert_eq!(
            s.eval::<String>(r#"return GetCVar("statusBarText")"#)
                .unwrap(),
            "1",
            "a saved value outranks the registered default (any key case)"
        );
        assert_eq!(
            s.eval::<String>(r#"return GetCVarDefault("statusBarText")"#)
                .unwrap(),
            "0",
            "…while the default stays the default"
        );
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("farclip")"#).unwrap(),
            "500",
            "a key the file never carried starts at its default"
        );
    }

    #[test]
    fn an_addon_registered_cvar_starts_at_its_saved_value() {
        let mut s = UiScript::new().unwrap();
        s.set_cvar_saved_base([("myaddon_scale".to_string(), "2.5".to_string())]);
        s.run(r#"RegisterCVar("MyAddon_Scale", "1.0")"#).unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("MyAddon_Scale")"#)
                .unwrap(),
            "2.5"
        );
        assert_eq!(
            s.eval::<String>(r#"return GetCVarDefault("MyAddon_Scale")"#)
                .unwrap(),
            "1.0",
            "the declared value is the DEFAULT, not the live value"
        );
    }

    #[test]
    fn the_multisample_dropdown_round_trips_through_the_three_bindings() {
        let mut s = UiScript::new().unwrap();
        s.register_cvars([
            ("gxColorBits", "32"),
            ("gxDepthBits", "32"),
            ("gxMultisample", "1"),
        ]);
        s.set_multisample_formats(vec![
            MultisampleFormat {
                color_bits: 32,
                depth_bits: 32,
                samples: 1,
            },
            MultisampleFormat {
                color_bits: 32,
                depth_bits: 32,
                samples: 2,
            },
            MultisampleFormat {
                color_bits: 32,
                depth_bits: 32,
                samples: 4,
            },
        ]);

        let flat: Vec<f64> = s.eval(r#"return { GetMultisampleFormats() }"#).unwrap();
        assert_eq!(
            flat,
            vec![32.0, 32.0, 1.0, 32.0, 32.0, 2.0, 32.0, 32.0, 4.0],
            "three fields per entry, entries in offer order"
        );

        // 1-based: the default `gxMultisample "1"` is the first row.
        assert_eq!(
            s.eval::<f64>("return GetCurrentMultisampleFormat()")
                .unwrap(),
            1.0
        );

        // Pick 4x, the third row: all three CVars move together, as `0x48c640` writes them.
        s.eval::<()>("SetMultisampleFormat(3)").unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("gxMultisample")"#)
                .unwrap(),
            "4"
        );
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("gxColorBits")"#)
                .unwrap(),
            "32"
        );
        assert_eq!(
            s.eval::<f64>("return GetCurrentMultisampleFormat()")
                .unwrap(),
            3.0,
            "the selection round-trips: what Set wrote, GetCurrent finds"
        );

        let changed: Vec<String> = s
            .take_cvar_changes()
            .into_iter()
            .map(|(n, v)| format!("{n}={v}"))
            .collect();
        assert!(
            changed.contains(&"gxMultisample=4".to_string()),
            "expected gxMultisample on the change queue, got {changed:?}"
        );

        s.eval::<()>("SetMultisampleFormat(99)").unwrap();
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("gxMultisample")"#)
                .unwrap(),
            "4",
            "an out-of-range id must leave the selection alone"
        );
    }

    /// `realmName` is a real 1.12 CVar (`0x83f2d0`) and must be a string: every Ace addon runs
    /// `ace.trim(GetCVar("realmName"))` at `PLAYER_ENTERING_WORLD` (`Ace/AceState.lua:28`). Set
    /// through `set_realm_name`, the seam `GetRealmName()` also reads.
    #[test]
    fn the_realm_name_cvar_and_get_realm_name_are_one_fact() {
        let mut s = UiScript::new().unwrap();
        s.register_cvars([("realmName", "")]);

        assert_eq!(
            s.eval::<String>(r#"return GetCVar("realmName")"#).unwrap(),
            ""
        );

        s.set_realm_name("Archimonde");
        assert_eq!(
            s.eval::<String>(r#"return GetCVar("realmName")"#).unwrap(),
            "Archimonde",
            "the realm seam must write the CVar too, or the two facts drift"
        );

        // Ace's own line, run for real.
        let trimmed: String = s
            .eval(r#"return string.gsub(GetCVar("realmName"), "^%s*", "")"#)
            .unwrap();
        assert_eq!(trimmed, "Archimonde");
    }

    /// `0x815400` by the first byte, then `on`/`enabled` whole and in any case.
    #[test]
    fn get_cvar_bool_reads_the_value_as_wotlk_does() {
        let s = UiScript::new().unwrap();
        let rows = [
            ("1", true),
            ("10", true),
            ("9", true),
            ("0", false),
            ("0.5", false),
            ("", false),
            ("-1", false),
            ("true", true),
            ("Yes", true),
            ("false", false),
            ("no", false),
            ("ON", true),
            ("Enabled", true),
            ("off", false),
            ("disabled", false),
            ("onward", false),
            ("x", false),
        ];
        let names: Vec<String> = (0..rows.len()).map(|i| format!("Bool{i}")).collect();
        s.register_cvars(names.iter().zip(rows).map(|(n, (v, _))| (n.as_str(), v)));
        for (name, (value, want)) in names.iter().zip(rows) {
            let got = s
                .eval::<Option<f64>>(&format!("return GetCVarBool({name:?})"))
                .unwrap();
            assert_eq!(got, want.then_some(1.0), "{value:?}");
        }
    }

    #[test]
    fn get_cvar_bool_answers_nil_for_an_unknown_name_and_raises_on_no_name() {
        let s = UiScript::new().unwrap();
        let out: mlua::MultiValue = s
            .lua()
            .load(r#"return GetCVarBool("noSuchCVar")"#)
            .eval()
            .unwrap();
        assert!(matches!(out.into_vec().as_slice(), [mlua::Value::Nil]));
        let err = s.run("GetCVarBool()").unwrap_err().to_string();
        assert!(err.contains(r#"Usage: GetCVarBool("cvar")"#), "{err}");
    }

    #[test]
    fn get_cvar_bool_is_a_global() {
        let s = UiScript::new().unwrap();
        assert!(s
            .eval::<bool>(r#"return type(GetCVarBool) == "function""#)
            .unwrap());
    }
}
