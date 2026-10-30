//! The CVar registry, host side: the reference's engine-side table (records of name, value,
//! default, latch slot and the change callback handed over at registration). [`Cvars`] holds every
//! registered row's live value, survives a VM replacement and is what the archives are composed
//! from; the script VM keeps a mirror for Lua's synchronous `GetCVar`/`SetCVar`
//! ([`benilla_ui::script::UiScript::seed_cvars`]) whose writes queue back here.
//!
//! - [`REGISTERED`] holds only vars something reads, a host knob or a Lua consumer. A row's default
//!   is the reference's, and [`table::Registered::reference`] says where it stands against it.
//! - The change callback is a Bevy observer on [`CvarChanged`], beside the knob it writes; the
//!   registry applies nothing itself.
//! - A latched row's write is staged in [`Row::pending`] until [`Cvars::commit_latched`], the
//!   reference's commit inside `RestartGx`; the archive writes the stage, as the reference's
//!   archive walk (`0x767030`) prefers the latched value.
//! - Each row is archived in one file by its [`table::Scope`]: `WTF\Config.wtf`, or the account's
//!   or the character's `config-cache.wtf`, account-data slots 0 and 1 ([`crate::account_data`]).
//!   All three are `SET name "value"` lines ([`wtf`]), every row off its default (compared
//!   case-blind) in creation order; a `Config.wtf` name no row claims is written back as read,
//!   and an addon's `RegisterCVar` row is never archived (flag `0x80`).
//! - Boot folds `Config.wtf`, then `RunOnce*.wtf` and `realmlist.wtf`, in and fires the observers
//!   before anything after [`CvarLoad`]; each frame drains the VM's writes in and pushes the
//!   registry's out. `Config.wtf` is written when something moved, at Enter World, at the UI
//!   shutdown and at exit (`0x767100`); the scoped files at the UI shutdown.
//!
//! Env overrides (`WOW_UI_SCALE`, `WOW_FARCLIP`, …) win for the session and never touch the file
//! ([`Cvars::own_for_session`]).

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use crate::ui_script::VmMemo;
use benilla_ui::script::{SeededCvar, UiScript};

mod camera_tuning;
#[cfg(test)]
mod stock_census_tests;
mod table;
pub(crate) mod wtf;
#[cfg(test)]
use table::Reference;
pub(crate) use table::Scope;
pub(crate) use table::{registered_pairs, registered_rows, REGISTERED};

// ─── The registry ────────────────────────────────────────────────────────────────────────────

/// An accepted move of a CVar's applied value: the reference's change callback, as a Bevy event.
/// Not fired for a no-op write, a staged latched value, or a session override.
#[derive(Event, Clone, Debug, PartialEq, Eq)]
pub(crate) struct CvarChanged {
    /// The registered spelling (`MasterVolume`).
    pub(crate) name: String,
    pub(crate) old: String,
    pub(crate) new: String,
}

impl CvarChanged {
    /// Case-insensitive, like every lookup the client makes.
    pub(crate) fn is(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
    }

    /// The lowercased name, as observers' `match` arms spell it.
    pub(crate) fn key(&self) -> String {
        self.name.to_ascii_lowercase()
    }

    /// The new value as a number; `0` on a string row, whose observer reads [`Self::new`]. The
    /// registry refuses an unparseable write to a numeric row ([`Cvars::set`]).
    pub(crate) fn num(&self) -> f32 {
        self.new.trim().parse().unwrap_or(0.0)
    }

    /// The new value as the client's flag: int-parse, then `!= 0`.
    pub(crate) fn flag(&self) -> bool {
        self.num() != 0.0
    }
}

/// One row of the live registry: the parts of the reference's `CVar` record benilla keeps.
#[derive(Clone, Debug)]
pub(crate) struct Row {
    /// The registered spelling.
    pub(crate) name: String,
    pub(crate) default: String,
    /// The reference's registered string, what the archive compares against
    /// ([`table::Registered::reference_default`]), so a file means to 12340 what it means here.
    pub(crate) archive_default: String,
    /// The applied value: what `GetCVar` answers and what the file is composed from.
    pub(crate) value: String,
    /// A latched row's staged value (`rec+0x38`), applied by [`Cvars::commit_latched`].
    pub(crate) pending: Option<String>,
    /// The reference's flag bit1.
    pub(crate) latched: bool,
    /// Declared by an addon's `RegisterCVar`: never archived (`0x50fea0` registers it with
    /// `0x80`), and re-seeded into every later VM, so the addon's re-declaration is a no-op.
    pub(crate) addon: bool,
    /// The file it is archived in.
    pub(crate) scope: Scope,
    /// [`table::Registered::hidden`]: kept out of every VM's mirror.
    pub(crate) hidden: bool,
}

impl Row {
    /// A row whose default parses as a number refuses a write that does not ([`Cvars::set`]).
    fn numeric(&self) -> bool {
        self.default.trim().parse::<f32>().is_ok()
    }

    /// What the archive writes for the row, or `None` at the reference's default (`0x767030`: the
    /// latched value first, compared case-blind); an addon's row is never archived.
    fn archived(&self) -> Option<&str> {
        if self.addon {
            return None;
        }
        let value = self.pending.as_deref().unwrap_or(&self.value);
        (!value.eq_ignore_ascii_case(&self.archive_default)).then_some(value)
    }
}

/// What a write did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SetOutcome {
    /// No such row (warned).
    Unknown,
    /// A numeric row and a value that does not parse: refused, the applied value stands (warned).
    Refused,
    /// Already the applied value (or already the staged one).
    Unchanged,
    /// A latched row: staged, not applied; nothing fires until the commit.
    Staged,
    /// Applied: a [`CvarChanged`] is queued for the next flush and the config is dirty.
    Changed,
}

/// The engine-side CVar table.
#[derive(Resource)]
pub(crate) struct Cvars {
    rows: Vec<Row>,
    /// Lowercased name → row.
    index: HashMap<String, usize>,
    /// `Config.wtf`'s entries in file order and their own spelling, the merge base of every save:
    /// a name no row claims is written back from here.
    file: Vec<(String, String)>,
    /// A `config-cache.wtf`'s names no row claims, by scope: 12340 registers every one of them
    /// with its scope, so each goes back to the file it came from.
    scoped_orphans: Vec<(Scope, String, String)>,
    /// Lowercased names the session owns rather than the player: the env levers, and `gxApi`, the
    /// render backend. Never saved; the file's entry is left as found.
    session_owned: HashSet<String>,
    /// Accepted moves not yet triggered, flushed by [`sync_cvars`], the boot load, the session-edge
    /// fold, or a caller of [`Cvars::take_events`].
    events: Vec<CvarChanged>,
    /// Host-side writes the VM's mirror has not seen; cleared by a seed.
    outbox: Vec<(String, String)>,
    /// A value moved since the last `Config.wtf` save (the reference's needs-save flag,
    /// `0xca19f8`).
    dirty: bool,
}

impl Default for Cvars {
    fn default() -> Self {
        let mut cvars = Self {
            rows: Vec::with_capacity(REGISTERED.len()),
            index: HashMap::with_capacity(REGISTERED.len()),
            file: Vec::new(),
            scoped_orphans: Vec::new(),
            session_owned: HashSet::new(),
            events: Vec::new(),
            outbox: Vec::new(),
            dirty: false,
        };
        for r in registered_rows() {
            cvars.insert_row(Row {
                name: r.name.to_string(),
                default: r.default.to_string(),
                archive_default: r.reference_default().to_string(),
                value: r.default.to_string(),
                pending: None,
                latched: r.latched,
                addon: false,
                scope: r.scope,
                hidden: r.hidden,
            });
        }
        cvars
    }
}

impl Cvars {
    fn insert_row(&mut self, row: Row) {
        let key = row.name.to_ascii_lowercase();
        debug_assert!(
            !self.index.contains_key(&key),
            "{}: registered twice",
            row.name
        );
        self.index.insert(key, self.rows.len());
        self.rows.push(row);
    }

    fn slot(&self, name: &str) -> Option<usize> {
        self.index.get(&name.to_ascii_lowercase()).copied()
    }

    /// The row, matched case-insensitively.
    pub(crate) fn row(&self, name: &str) -> Option<&Row> {
        self.slot(name).map(|i| &self.rows[i])
    }

    /// Every row, in registration order.
    pub(crate) fn rows(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter()
    }

    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.row(name).map(|r| r.value.as_str())
    }

    pub(crate) fn num(&self, name: &str) -> Option<f32> {
        self.get(name).and_then(|v| v.trim().parse().ok())
    }

    /// The applied value as the client's flag (int-parse, `!= 0`).
    pub(crate) fn flag(&self, name: &str) -> Option<bool> {
        self.num(name).map(|v| v != 0.0)
    }

    pub(crate) fn default_of(&self, name: &str) -> Option<&str> {
        self.row(name).map(|r| r.default.as_str())
    }

    pub(crate) fn is_session_owned(&self, name: &str) -> bool {
        self.session_owned.contains(&name.to_ascii_lowercase())
    }

    /// The persisted `checkAddonVersion` the addon load walk gates on; registered on.
    pub(crate) fn addon_version_check(&self) -> bool {
        self.flag("checkAddonVersion").unwrap_or(true)
    }

    fn touch(&mut self) {
        self.dirty = true;
    }

    /// Remove a `Config.wtf` entry no row claims, a setting retired from the table, and return
    /// its value; the next save writes the file without it.
    pub(crate) fn retire_file_entry(&mut self, name: &str) -> Option<String> {
        if self.index.contains_key(&name.to_ascii_lowercase()) {
            return None;
        }
        let at = self
            .file
            .iter()
            .position(|(k, _)| k.eq_ignore_ascii_case(name))?;
        let (_, value) = self.file.remove(at);
        self.file.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
        self.touch();
        Some(value)
    }

    /// A write from either side of the VM boundary. A `from_vm` write is not echoed back, but a
    /// refusal is, because the mirror already stored the refused value.
    fn write(&mut self, name: &str, value: &str, from_vm: bool) -> SetOutcome {
        let Some(i) = self.slot(name) else {
            warn!("cvar {name}: not registered — write ignored");
            return SetOutcome::Unknown;
        };
        let row = &mut self.rows[i];
        if row.numeric() && value.trim().parse::<f32>().is_err() {
            warn!(
                "cvar {}: unparseable value '{value}' refused (still {:?})",
                row.name, row.value
            );
            if from_vm {
                self.outbox.push((row.name.clone(), row.value.clone()));
            }
            return SetOutcome::Refused;
        }
        if row.latched {
            // The reference's set on flag bit1 stores the latched value and skips the internal
            // write: no dirty, no callback. The stage lives here, not in the mirror, which only
            // ever learns applied values.
            let staged = (value != row.value).then(|| value.to_string());
            if row.pending == staged {
                return SetOutcome::Unchanged;
            }
            row.pending = staged;
            return if row.pending.is_some() {
                SetOutcome::Staged
            } else {
                SetOutcome::Unchanged // the stage cleared: the boundary has nothing to do
            };
        }
        if row.value == value {
            return SetOutcome::Unchanged;
        }
        let old = std::mem::replace(&mut row.value, value.to_string());
        let name = row.name.clone();
        self.events.push(CvarChanged {
            name: name.clone(),
            old,
            new: value.to_string(),
        });
        if !from_vm && !self.rows[i].hidden {
            self.outbox.push((name, value.to_string()));
        }
        self.touch();
        SetOutcome::Changed
    }

    /// A host-side write (minimap zoom, camera views, the remembered character, the tip cursor):
    /// mirrored into the VM, persisted and observed.
    pub(crate) fn set(&mut self, name: &str, value: &str) -> SetOutcome {
        self.write(name, value, false)
    }

    /// A write the VM's mirror already made, drained from its change queue.
    pub(crate) fn set_from_vm(&mut self, name: &str, value: &str) -> SetOutcome {
        self.write(name, value, true)
    }

    /// Follow a value the engine already applied, for a second spelling of one knob
    /// (`WorldDetail`/`frillDensity`): the row moves, persists and reaches the mirror, but no
    /// observer fires, since a full write would queue an event that lands a flush late and wins
    /// stale. Returns whether the row moved.
    pub(crate) fn mirror(&mut self, name: &str, value: &str) -> bool {
        let Some(i) = self.slot(name) else {
            warn!("cvar {name}: not registered — mirror ignored");
            return false;
        };
        let row = &mut self.rows[i];
        if row.value == value {
            return false;
        }
        row.value = value.to_string();
        row.pending = None;
        self.outbox.push((row.name.clone(), value.to_string()));
        self.touch();
        true
    }

    /// The latch boundary `RestartGx` crosses, where the reference commits its fourteen gx records.
    /// Each staged `gx*` value is applied, fires, persists and reaches the mirror; returns how many
    /// moved.
    ///
    /// Only `gx*` rows: `SoundBufferSize`'s register site discards its record, so nothing commits
    /// it and its stage is lost at exit, as in the reference. The reference runs a gx callback at
    /// `SetCVar` time; ours fire here, before the device rebuild reads them.
    pub(crate) fn commit_latched(&mut self) -> usize {
        let mut moved = 0;
        for row in &mut self.rows {
            if !row.name.starts_with("gx") {
                continue;
            }
            let Some(staged) = row.pending.take() else {
                continue;
            };
            if staged == row.value {
                continue;
            }
            let old = std::mem::replace(&mut row.value, staged.clone());
            self.events.push(CvarChanged {
                name: row.name.clone(),
                old,
                new: staged.clone(),
            });
            self.outbox.push((row.name.clone(), staged));
            moved += 1;
        }
        if moved > 0 {
            self.touch();
        }
        moved
    }

    /// The session owns this row: it is never saved and the file's entry is left alone. `value` is
    /// what it answers this session; `None` marks it without moving it. No observer fires: the knob
    /// already read the env.
    pub(crate) fn own_for_session(&mut self, name: &str, value: Option<&str>) {
        let key = name.to_ascii_lowercase();
        self.session_owned.insert(key.clone());
        let Some(value) = value else {
            return;
        };
        let Some(&i) = self.index.get(&key) else {
            warn!("cvar {name}: not registered — session value ignored");
            return;
        };
        let row = &mut self.rows[i];
        if row.value != value {
            row.value = value.to_string();
            row.pending = None;
            self.outbox.push((row.name.clone(), value.to_string()));
        }
    }

    /// An addon's `RegisterCVar`: a row of its own at the file's value for the name, else at the
    /// declared default. A name already registered is a no-op.
    pub(crate) fn learn_addon_row(&mut self, name: &str, default: &str) {
        if self.slot(name).is_some() {
            return;
        }
        let saved = self
            .file
            .iter()
            .rfind(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone());
        self.insert_row(Row {
            name: name.to_string(),
            default: default.to_string(),
            archive_default: default.to_string(),
            value: saved.unwrap_or_else(|| default.to_string()),
            pending: None,
            latched: false,
            addon: true,
            scope: Scope::Config,
            hidden: false,
        });
    }

    /// Fold the file in: a known, player-owned key becomes its row's applied value and fires (the
    /// reference's `Register` on a record `Config.wtf` created calls the callback); an unknown key
    /// is kept for the save, as the placeholder `SET` makes for it (`0x768307`); a session-owned
    /// one is skipped. Nothing dirties.
    fn load_file(&mut self, file: Vec<(String, String)>) {
        for (name, value) in &file {
            let key = name.to_ascii_lowercase();
            let Some(&i) = self.index.get(&key) else {
                // A retired row is dropped by `load_config`, not kept.
                if !RETIRED_ROWS.iter().any(|r| r.eq_ignore_ascii_case(name)) {
                    warn!("config: unknown cvar '{name}' — preserved, not applied");
                }
                continue;
            };
            if self.session_owned.contains(&key) {
                info!("config: {name} is owned by this session, not the file (file value kept)");
                continue;
            }
            let row = &mut self.rows[i];
            if row.scope != Scope::Config {
                warn!("config: {name} is kept in a config-cache.wtf, not read from Config.wtf");
                continue;
            }
            if row.numeric() && value.trim().parse::<f32>().is_err() {
                warn!("config: {name}: unparseable value '{value}' ignored");
                continue;
            }
            if row.value == *value {
                continue;
            }
            let old = std::mem::replace(&mut row.value, value.clone());
            self.events.push(CvarChanged {
                name: row.name.clone(),
                old,
                new: value.clone(),
            });
        }
        self.file = file;
    }

    /// The character rows' delete (`0x767350`) at the UI shutdown, and the account rows' delete
    /// (`0x7673a0`) at an account login: every row of the scope back at its default, silently, as
    /// the reference deletes the records without a callback.
    pub(crate) fn reset_scope(&mut self, scope: Scope) {
        for row in self.rows.iter_mut().filter(|r| r.scope == scope) {
            row.value = row.default.clone();
            row.pending = None;
        }
        self.scoped_orphans.retain(|(s, _, _)| *s != scope);
    }

    /// A slot's `config-cache.wtf` run in, its `SET`s in order: a row takes the value and fires;
    /// a name no row claims is kept for the same file, where 12340's registered row would save it.
    pub(crate) fn load_scope(&mut self, scope: Scope, text: &str) {
        for (name, value) in wtf::parse(text) {
            match self.slot(&name) {
                Some(i) if self.rows[i].numeric() && value.trim().parse::<f32>().is_err() => {
                    warn!("config-cache: {name}: unparseable value '{value}' ignored");
                }
                Some(i) => {
                    let row = &mut self.rows[i];
                    if row.value == value {
                        continue;
                    }
                    let old = std::mem::replace(&mut row.value, value.clone());
                    row.pending = None;
                    let name = row.name.clone();
                    self.outbox.push((name.clone(), value.clone()));
                    self.events.push(CvarChanged {
                        name,
                        old,
                        new: value,
                    });
                }
                None => {
                    let kept = self
                        .scoped_orphans
                        .iter_mut()
                        .find(|(s, k, _)| *s == scope && k.eq_ignore_ascii_case(&name));
                    match kept {
                        Some((_, _, v)) => *v = value,
                        None => self.scoped_orphans.push((scope, name, value)),
                    }
                }
            }
        }
    }

    /// A scoped file's body (`0x512890`): the scope's rows off their default, in creation order,
    /// then the names it held that no row claims.
    pub(crate) fn compose_scope(&self, scope: Scope) -> String {
        wtf::compose(
            self.rows
                .iter()
                .filter(|r| {
                    r.scope == scope && !self.session_owned.contains(&r.name.to_ascii_lowercase())
                })
                .filter_map(|r| r.archived().map(|v| (r.name.as_str(), v)))
                .chain(
                    self.scoped_orphans
                        .iter()
                        .filter(|(s, _, _)| *s == scope)
                        .map(|(_, k, v)| (k.as_str(), v.as_str())),
                ),
        )
    }

    /// The file's entries no row claims: a newer build's keys, or an addon's not yet registered.
    /// Handed to the VM as its saved base, so an addon's `RegisterCVar` starts at the saved value.
    pub(crate) fn orphans(&self) -> Vec<(String, String)> {
        self.file
            .iter()
            .filter(|(k, _)| !self.index.contains_key(&k.to_ascii_lowercase()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// The table as a VM's mirror is seeded from it, a hidden row left out.
    pub(crate) fn vm_seed(&self) -> Vec<SeededCvar> {
        self.rows
            .iter()
            .filter(|r| !r.hidden)
            .map(|r| SeededCvar {
                name: r.name.clone(),
                value: r.value.clone(),
                default: r.default.clone(),
                latched: r.latched,
            })
            .collect()
    }

    /// Read before taking, so a quiet frame never deref-muts the registry.
    pub(crate) fn has_events(&self) -> bool {
        !self.events.is_empty()
    }

    /// The accepted moves since the last flush, for the caller to trigger.
    pub(crate) fn take_events(&mut self) -> Vec<CvarChanged> {
        std::mem::take(&mut self.events)
    }

    fn has_outbox(&self) -> bool {
        !self.outbox.is_empty()
    }

    fn take_outbox(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.outbox)
    }

    /// `Config.wtf` to save, in the reference's list order: the file's names first, as its `SET`
    /// placeholders were created first, then every other row in registration order. A name no row
    /// claims goes back as read, a session-owned row as the file had it, a `Config.wtf` row when
    /// off its default; the scoped rows and an addon's are left out.
    fn compose(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for (name, value) in &self.file {
            let key = name.to_ascii_lowercase();
            if !seen.insert(key.clone()) {
                continue;
            }
            match self.index.get(&key) {
                None => out.push((name.clone(), value.clone())),
                Some(_) if self.session_owned.contains(&key) => {
                    out.push((name.clone(), value.clone()));
                }
                Some(&i) => {
                    let row = &self.rows[i];
                    if row.scope == Scope::Config {
                        if let Some(v) = row.archived() {
                            out.push((row.name.clone(), v.to_string()));
                        }
                    }
                }
            }
        }
        for row in &self.rows {
            let key = row.name.to_ascii_lowercase();
            if row.scope != Scope::Config
                || seen.contains(&key)
                || self.session_owned.contains(&key)
            {
                continue;
            }
            if let Some(v) = row.archived() {
                out.push((row.name.clone(), v.to_string()));
            }
        }
        out
    }
}

/// The startup fold of `config.toml` into the registry and the knobs ([`load_config`]). A set
/// because the world camera reads `gxMultisample` once at spawn, so `setup_player` must order after
/// it; without the constraint that order is the executor's choice.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CvarLoad;

pub(crate) struct CvarPlugin;

impl Plugin for CvarPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Cvars>()
            .add_systems(
                Startup,
                (load_config, publish_filter_policy)
                    .chain()
                    .in_set(CvarLoad),
            )
            // After the tick, so a `SetCVar` from this frame reaches the registry and its observers
            // before the frame's drains; `video::drain_restart_gx` runs in `Last`, so the commit
            // finds the stage. A texture-policy write is published the same frame.
            .add_systems(
                Update,
                (sync_cvars, follow_filter_policy)
                    .chain()
                    .after(crate::ui_script::UiInput),
            )
            // Enter World's save (`0x4d9ed8`), in `Last`, after every `Update` writer of the
            // registry.
            .add_message::<SaveConfig>()
            .add_systems(
                Last,
                save_config_on_request.before(crate::shutdown::OnAppExit),
            );
        // The exit flush runs on the exit edge: the close button's `AppExit` is written in
        // `PostUpdate`, so an `Update` save never sees it.
        crate::shutdown::on_app_exit(app, save_config_on_exit.into_configs());
    }
}

/// What the environment took for this session and its value, read off the knobs the env levers
/// seeded (`RenderScale::default()` reads `$WOW_RENDER_SCALE`, …). A lever whose resource is absent
/// still marks its row, so the file cannot apply over the env.
fn session_values(world: &World) -> Vec<(&'static str, Option<String>)> {
    let set = |k: &str| std::env::var_os(k).is_some();
    let flag = |b: bool| if b { "1" } else { "0" }.to_string();
    let mut out: Vec<(&'static str, Option<String>)> = Vec::new();
    if set("WOW_UI_SCALE") {
        let v = world.get_resource::<crate::ui_script::UiScaleSetting>();
        out.push(("uiScale", v.and_then(|s| s.env).map(|e| e.to_string())));
    }
    if set("WOW_FARCLIP") {
        let v = world.get_resource::<benilla_world::view::ViewDistance>();
        out.push(("farclip", v.map(|s| s.farclip.to_string())));
    }
    if set("WOW_CLUTTER_DENSITY") {
        let v = world.get_resource::<benilla_world::clutter::ClutterConfig>();
        out.push(("groundEffectDensity", v.map(|c| c.cells().to_string())));
    }
    if crate::video::novsync_env() {
        let v = world.get_resource::<crate::video::VideoConfig>();
        out.push(("gxVSync", v.map(|c| flag(c.vsync))));
    }
    if set("WOW_TEXTURE_FILTERING") {
        let v = world.get_resource::<benilla_assets::TexFilterSetting>();
        out.push(("textureFilteringMode", v.map(|t| t.mode.to_string())));
    }
    // `$WOW_WIN`, a capture scenario or an instrumented run owns the window geometry.
    if crate::video::windowed_env() {
        let v = world.get_resource::<crate::video::VideoConfig>();
        out.push((
            "gxWindow",
            v.map(|c| flag(c.display == crate::video::DisplayMode::Windowed)),
        ));
        out.push(("gxMaximize", v.map(|c| flag(c.maximize))));
        out.push((
            "gxResolution",
            v.map(|c| {
                c.resolution
                    .map_or_else(String::new, |r| format!("{}x{}", r.x, r.y))
            }),
        ));
    }
    // The multisampling and render-scale levers: an instrument run must not pin 4× into the file.
    if set("WOW_MSAA") {
        let v = world.get_resource::<benilla_world::view::MsaaSetting>();
        out.push(("gxMultisample", v.map(|m| m.samples.to_string())));
    }
    if set("WOW_RENDER_SCALE") {
        let v = world.get_resource::<crate::world_backdrop::RenderScale>();
        out.push(("renderScale", v.map(|r| r.0.to_string())));
    }
    // `$WOW_HOST` is the session's realmlist, which a test run must never write into the file.
    if set("WOW_HOST") {
        let v = world.get_resource::<crate::realmlist::Realmlist>();
        out.push((
            crate::realmlist::CVAR_REALMLIST,
            v.map(|r| r.address().to_string()),
        ));
    }
    out
}

/// Startup: mark what the session owns, fold `config.toml` in (no file means all defaults) and fire
/// the observers now, so everything after [`CvarLoad`] finds its knob written. The VM does not
/// exist yet; [`sync_cvars`] seeds its mirror.
fn load_config(world: &mut World) {
    let session = session_values(world);
    let stored = stored_config();
    let extra = startup_extras();
    let events = {
        let mut cvars = world.resource_mut::<Cvars>();
        for (name, value) in session {
            cvars.own_for_session(name, value.as_deref());
        }
        // `gxApi` is the render backend, a fact about the machine that `sync_cvars` pushes live, so
        // it is never persisted.
        cvars.own_for_session("gxApi", None);
        match stored {
            StoredConfig::Absent => {} // no file, hermetic capture, or no install
            StoredConfig::Bad(msg) => {
                // A malformed file is kept: nothing loads, and nothing saves over it until a
                // change.
                warn!("{msg}");
            }
            StoredConfig::Table(table) => {
                cvars.load_file(table);
                // The 1.12 rows 3.3.5a has no record of, retired from the table: their file entries
                // go at the next save rather than riding along as unknown keys.
                for name in RETIRED_ROWS {
                    if cvars.retire_file_entry(name).is_some() {
                        info!("config: {name} is not a 3.3.5a setting; dropped from the file");
                    }
                }
            }
        }
        // `RunOnce*.wtf` and the realmlist files run after `Config.wtf`, each `SET` a write that
        // reaches the file at the next save (`0x4067f0`, `0x6b0bf0`).
        for (name, value) in extra {
            if cvars.row(&name).is_some() {
                if !cvars.is_session_owned(&name) {
                    cvars.set(&name, &value);
                }
            } else if !cvars
                .file
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case(&name))
            {
                cvars.file.push((name, value));
                cvars.touch();
            }
        }
        cvars.take_events()
    };
    for event in events {
        world.trigger(event);
    }
}

/// Rows earlier builds registered from 1.12.1 that the 3.3.5a client has no record of, each now
/// served by its 3.3.5a row: the texture filter by `textureFilteringMode`, the ground clutter by
/// `groundEffectDensity`, the rest by nothing the stock UI binds.
const RETIRED_ROWS: [&str; 9] = [
    "trilinear",
    "anisotropic",
    "WorldDetail",
    "frillDensity",
    "SmallCull",
    "doodadAnim",
    "pixelShaders",
    "M2UseShaders",
    "M2UsePixelShaders",
];

/// What the one read of `Config.wtf` found.
enum StoredConfig {
    /// No file, no install, or a hermetic capture: every value is its registered default.
    Absent,
    Table(Vec<(String, String)>),
    /// Unreadable or malformed; the message is carried because this read runs before `LogPlugin`
    /// exists.
    Bad(String),
}

/// Read `Config.wtf`, for [`load_config`] and for the primary window in [`crate::run`], which
/// needs `gxWindow`/`gxResolution` before it exists. Not cached: a process-wide cache would answer
/// every test from the first one's file. Read as bytes, since a value is raw (`flaggedTutorials`
/// carries a 0x01).
fn stored_config() -> StoredConfig {
    let Some(path) = crate::local_state::config_path() else {
        return StoredConfig::Absent; // hermetic capture or no install: session-only state
    };
    match std::fs::read(&path) {
        Ok(bytes) => StoredConfig::Table(wtf::parse(&String::from_utf8_lossy(&bytes))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => StoredConfig::Absent,
        Err(e) => StoredConfig::Bad(format!("config: cannot read {}: {e}", path.display())),
    }
}

/// The `SET`s that run after `Config.wtf` at startup: each `WTF\RunOnce*.wtf`, names sorted
/// case-blind (`0x406740`), then `Data\<locale>\realmlist.wtf` and the root's `realmlist.wtf`
/// (`0x6b0bf0`).
fn startup_extras() -> Vec<(String, String)> {
    let mut files = crate::local_state::run_once_files();
    if let Some(data) = benilla_formats::wow_data() {
        if let Ok(locale) = benilla_formats::data_locale(&data) {
            files.push(data.join(locale).join("realmlist.wtf"));
        }
    }
    files.extend(crate::local_state::client_root().map(|r| r.join("realmlist.wtf")));
    files
        .iter()
        .filter_map(|f| std::fs::read(f).ok())
        .flat_map(|b| wtf::parse(&String::from_utf8_lossy(&b)))
        .collect()
}

/// One CVar as `Config.wtf` holds it, before the `App` exists: for the primary window, which is
/// built with its display mode resolved. Everything else reads [`Cvars::get`].
pub(crate) fn boot_cvar(name: &str) -> Option<String> {
    match stored_config() {
        StoredConfig::Table(t) => t
            .into_iter()
            .rfind(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v),
        StoredConfig::Absent | StoredConfig::Bad(_) => None,
    }
}

/// Per frame: seed a new VM's mirror, drain its registrations and writes into the registry, push
/// the registry's writes into the mirror, and trigger every accepted move.
pub(crate) fn sync_cvars(
    script: Option<NonSendMut<UiScript>>,
    mut cvars: ResMut<Cvars>,
    adapter: Option<Res<bevy::render::renderer::RenderAdapterInfo>>,
    msaa_formats: Option<Res<benilla_world::view::MsaaFormats>>,
    mut seeded: Local<VmMemo<bool>>,
    mut commands: Commands,
) {
    // The live render backend; absent headless, where the registered `""` stands.
    if let Some(adapter) = adapter.as_deref() {
        let backend = adapter.backend.to_str();
        if cvars.get("gxApi") != Some(backend) {
            cvars.own_for_session("gxApi", Some(backend));
        }
    }
    let Some(mut script) = script else {
        // Nothing to mirror into; a later VM is seeded from the table, which carries it all.
        if cvars.has_outbox() {
            cvars.take_outbox();
        }
        if cvars.has_events() {
            for event in cvars.take_events() {
                commands.trigger(event);
            }
        }
        return;
    };
    // The VM's writes before the seed: an addon's `SetCVar` at load is already queued, and seeding
    // first would overwrite the mirror with the older value. Registrations before writes, since an
    // addon declares a row and sets it together.
    let registrations = script.take_cvar_registrations();
    let changes = script.take_cvar_changes();
    if !registrations.is_empty() || !changes.is_empty() {
        for (name, default) in registrations {
            cvars.learn_addon_row(&name, &default);
        }
        for (name, value) in changes {
            cvars.set_from_vm(&name, &value);
        }
    }
    if seeded.claim(&script) {
        // The file's unclaimed entries first, so an addon's `RegisterCVar` starts at the saved
        // value; then the table.
        script.set_cvar_saved_base(cvars.orphans());
        script.seed_cvars(cvars.vm_seed());
        if cvars.has_outbox() {
            cvars.take_outbox(); // the seed just carried everything
        }
        // The Video dropdown's menu: what this device accepts, enumerated by
        // `view::MsaaSupportPlugin`.
        script.set_multisample_formats(
            msaa_formats
                .as_deref()
                .map(|f| {
                    f.formats
                        .iter()
                        .map(|&(color_bits, depth_bits, samples)| {
                            benilla_ui::script::MultisampleFormat {
                                color_bits,
                                depth_bits,
                                samples,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default(),
        );
    }
    if cvars.has_outbox() {
        for (name, value) in cvars.take_outbox() {
            script.set_cvar_host(&name, &value);
        }
    }
    if cvars.has_events() {
        for event in cvars.take_events() {
            commands.trigger(event);
        }
    }
}

/// Fold the dying VM's last writes into the registry. Called from
/// [`crate::ui_script::end_ui_session`] after the shutdown events (a `PLAYER_LOGOUT` handler may
/// `SetCVar`, and the reference keeps that write) and before the VM is replaced, so a final-frame
/// write is not lost to the next VM's seed. The observers fire here.
pub(crate) fn fold_dying_vm_cvars(world: &mut World) {
    // No registry: a test or stripped world without the plugin.
    if !world.contains_resource::<Cvars>() {
        return;
    }
    let (registrations, changes) = {
        let Some(mut script) = world.get_non_send_resource_mut::<UiScript>() else {
            return;
        };
        (script.take_cvar_registrations(), script.take_cvar_changes())
    };
    let events = {
        let mut cvars = world.resource_mut::<Cvars>();
        for (name, default) in registrations {
            cvars.learn_addon_row(&name, &default);
        }
        for (name, value) in changes {
            cvars.set_from_vm(&name, &value);
        }
        if cvars.has_outbox() {
            cvars.take_outbox(); // the VM this was for is going away
        }
        if cvars.has_events() {
            cvars.take_events()
        } else {
            Vec::new()
        }
    };
    for event in events {
        world.trigger(event);
    }
}

/// The UI shutdown's delete of the character rows (`0x767350`), after the fold kept the dying VM's
/// last writes and the character's file took them, so the next character starts from the defaults.
pub(crate) fn drop_character_cvars(world: &mut World) {
    if let Some(mut cvars) = world.get_resource_mut::<Cvars>() {
        cvars.reset_scope(Scope::Character);
    }
}

/// Ask for `Config.wtf`'s save this frame: Enter World's (`0x4d9ed8`).
#[derive(Message)]
pub(crate) struct SaveConfig;

fn save_config_on_request(mut asks: MessageReader<SaveConfig>, mut cvars: ResMut<Cvars>) {
    if asks.read().count() > 0 && cvars.dirty {
        write_config(&mut cvars);
    }
}

/// The exit frame (`0x7673f7`): the `RunOnce*.wtf` files go first (`0x406b70`), then whatever is
/// dirty is saved.
fn save_config_on_exit(mut cvars: ResMut<Cvars>) {
    for path in crate::local_state::run_once_files() {
        if let Err(e) = std::fs::remove_file(&path) {
            warn!("config: cannot delete {}: {e}", path.display());
        }
    }
    if cvars.dirty {
        write_config(&mut cvars);
    }
}

/// The reference's save of every row (`0x767100`), from a world handle: the UI shutdown's, after
/// the scoped files.
pub(crate) fn save_config_now(world: &mut World) {
    if let Some(mut cvars) = world.get_resource_mut::<Cvars>() {
        if cvars.dirty {
            write_config(&mut cvars);
        }
    }
}

/// Rewrite `Config.wtf` whole from the registry (`CREATE_ALWAYS`, no backup), so a session with
/// no VM saves what it changed.
fn write_config(cvars: &mut Cvars) {
    let Some(path) = crate::local_state::config_path() else {
        cvars.dirty = false; // hermetic/session-only: nothing to write, stop retrying
        return;
    };
    let file = cvars.compose();
    let body = wtf::compose(file.iter().map(|(n, v)| (n.as_str(), v.as_str())));
    match crate::local_state::write_atomic(&path, &body) {
        Ok(()) => {
            cvars.file = file;
            cvars.dirty = false;
        }
        Err(e) => {
            warn!("config: cannot write {}: {e}", path.display());
            cvars.dirty = false; // don't retry every frame into the same error
        }
    }
}

/// A `textureFilteringMode` or `baseMip` write after boot: publish it, which re-samples the world.
fn follow_filter_policy(filter: Res<benilla_assets::TexFilterSetting>) {
    if filter.is_changed() && !filter.is_added() {
        publish_filter_policy(filter);
    }
}

/// Publish the texture policy the file held and log it, so a player's log says which mode the run
/// was in. Its own system after [`load_config`], before any texture loads.
fn publish_filter_policy(filter: Res<benilla_assets::TexFilterSetting>) {
    benilla_assets::publish_tex_filter(*filter);
    info!(
        "texture filter: textureFilteringMode {} (anisotropy {}), baseMip {}",
        filter.mode,
        filter.anisotropy_clamp(),
        filter.base_mip
    );
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::chat_bubble::BubbleConfig;
    use crate::minimap::MinimapZoom;
    use crate::nameplates::NameConfig;
    use crate::player::camera::{FollowConfig, FollowStyle, LookConfig, FOLLOW_SPEED_RANGE};
    use crate::player::camera_zoom::ZoomLimit;
    use crate::portrait::PaneRate;
    use crate::sound::SoundConfig;
    use crate::target::ClickConfig;
    use crate::ui_loot::LootConfig;
    use crate::ui_script::{UiScaleCvar, UiScaleSetting};
    use crate::video::VideoConfig;
    use crate::world_backdrop::{RenderScale, RENDER_SCALE_RANGE};
    use benilla_ui::widget::MINIMAP_ZOOM_LEVELS;
    use benilla_world::clutter::ClutterConfig;
    use benilla_world::view::{MsaaSetting, ViewDistance, MSAA_RANGE};

    /// Every row's [`Reference`] column holds, both ways: a `Same` row that drifted fails, and so
    /// does a `Deviates` or `Overridden` row back in agreement. Numbers parse-compare, so "1" and
    /// "1.0" agree.
    #[test]
    fn defaults_stand_where_the_reference_column_says() {
        /// Numeric when both parse, textual otherwise.
        fn agrees(ours: &str, theirs: &str) -> bool {
            match (ours.parse::<f32>(), theirs.parse::<f32>()) {
                (Ok(a), Ok(b)) => a == b,
                _ => ours == theirs,
            }
        }
        for row in registered_rows() {
            let name = row.name;
            match &row.reference {
                Reference::Same(value) => assert!(
                    agrees(row.default, value),
                    "{name}: the row claims the reference registers {value:?} and we ship the \
                     same, but our default is {:?}. If the reference really does differ, this is \
                     a `deviates` row and owes a reason.",
                    row.default,
                ),
                Reference::Deviates { value, why } => {
                    assert!(
                        !agrees(row.default, value),
                        "{name}: a `deviates` row that no longer deviates — our {:?} IS the \
                         reference's. Demote it to `same`; a stale deviation hides that we are \
                         faithful again.",
                        row.default,
                    );
                    assert!(!why.trim().is_empty(), "{name}: a deviation owes a reason");
                }
                Reference::Overridden { registered, why } => {
                    assert!(
                        !agrees(row.default, registered),
                        "{name}: an `overridden` row whose default is just the registered string \
                         {registered:?} — that is `same`, and saying otherwise buries a real \
                         override behind a false one.",
                    );
                    assert!(
                        !why.trim().is_empty(),
                        "{name}: an override owes its mechanism"
                    );
                }
                Reference::Ours(why) => assert!(
                    !why.trim().is_empty(),
                    "{name}: a CVar the reference does not have owes the reason it exists",
                ),
            }
        }
    }

    #[test]
    fn the_options_that_leave_the_reference_are_this_list_and_no_other() {
        let mut names: Vec<&str> = REGISTERED
            .iter()
            .filter(|r| matches!(r.reference, Reference::Deviates { .. }))
            .map(|r| r.name)
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec![
                "gxApi",
                "gxColorBits",
                "gxDepthBits",
                "gxFixLag",
                "gxResolution",
                "realmList",
                "weatherDensity",
            ],
        );
    }

    /// Every numeric registered default equals the code constant it mirrors; the string rows are
    /// checked in [`the_string_valued_cvars_are_the_realm_and_the_windowed_size`].
    #[test]
    fn registered_defaults_mirror_the_code_truths() {
        let d: BTreeMap<&str, f32> = registered_rows()
            .filter_map(|r| r.default.parse::<f32>().ok().map(|f| (r.name, f)))
            .collect();
        let sound = SoundConfig::default();
        assert_eq!(d["Sound_MasterVolume"], sound.master);
        assert_eq!(d["Sound_SFXVolume"], sound.sfx);
        assert_eq!(d["Sound_MusicVolume"], sound.music);
        assert_eq!(d["Sound_AmbienceVolume"], sound.ambience);
        assert_eq!(d["Sound_EnableAllSound"] != 0.0, sound.enabled);
        assert_eq!(d["Sound_EnableSFX"] != 0.0, sound.sfx_enabled);
        assert_eq!(d["Sound_EnableMusic"] != 0.0, sound.music_enabled);
        assert_eq!(d["Sound_EnableAmbience"] != 0.0, sound.ambience_enabled);
        assert_eq!(d["Sound_EnableErrorSpeech"] != 0.0, sound.error_speech);
        assert_eq!(
            d["Sound_ListenerAtCharacter"] != 0.0,
            sound.listener_at_character
        );
        assert_eq!(d["Sound_EnableEmoteSounds"] != 0.0, sound.emote_sounds);
        assert_eq!(
            d["Sound_ZoneMusicNoDelay"] != 0.0,
            sound.zone_music_no_delay
        );
        assert_eq!(
            d["Sound_EnableSoundWhenGameIsInBG"] != 0.0,
            sound.background_sound
        );
        assert!(
            !sound.background_sound,
            "the reference goes quiet in the background and offers no way out"
        );
        assert_eq!(d["Sound_EnableReverb"] != 0.0, sound.reverb);
        assert_eq!(d["SoundOutputLimiter"] != 0.0, sound.limiter);
        assert!(sound.limiter, "the output limiter ships on");
        assert!(!sound.reverb, "zone reverb ships off");
        // `UiScaleSetting::default()` reads `$WOW_UI_SCALE`; its rows are the env-less literals.
        assert_eq!(d["uiScale"], 1.0);
        assert_eq!(d["useUiScale"], 0.0);
        // `ViewDistance::default()` reads `$WOW_FARCLIP`, so this is the env-less literal.
        assert_eq!(d["farclip"], benilla_world::view::FARCLIP_DEFAULT);
        assert_eq!(d["nearclip"], benilla_world::view::NEARCLIP_DEFAULT);
        assert_eq!(
            d["nearclip"],
            ViewDistance::default().nearclip,
            "the registered default and the resource's own must be one number"
        );
        // `MsaaSetting::default()` reads `$WOW_MSAA`, so this is the env-less literal.
        assert_eq!(d["gxMultisample"], 1.0);
        assert_eq!(
            d["deselectOnClick"] != 0.0,
            ClickConfig::default().deselect_on_click
        );
        assert_eq!(
            d["mouseInvertPitch"] != 0.0,
            LookConfig::default().invert_pitch
        );
        assert_eq!(d["mousespeed"], LookConfig::default().sensitivity);
        assert_eq!(d["cameraDistanceMax"], ZoomLimit::default().distance_max());
        assert_eq!(d["cameraDistanceMaxFactor"], ZoomLimit::default().factor());
        let follow = FollowConfig::default();
        assert_eq!(
            d["cameraSmoothStyle"],
            follow.style.cvar().parse::<f32>().unwrap()
        );
        assert_eq!(
            d["cameraSmoothTrackingStyle"],
            follow.tracking_style.cvar().parse::<f32>().unwrap()
        );
        assert_eq!(d["cameraYawSmoothSpeed"], follow.yaw_speed);
        assert_eq!(FollowStyle::default(), FollowStyle::Smart);
        assert_eq!(d["autoLootDefault"] != 0.0, LootConfig::default().auto_loot);
        assert_eq!(
            d["showLootSpam"] != 0.0,
            LootConfig::default().show_loot_spam
        );
        assert_eq!(
            d["guildMemberNotify"] != 0.0,
            crate::ui_guild::GuildMemberNotify::default().0
        );
        assert_eq!(d["guildMemberNotify"], 0.0, "the binary registers \"0\"");
        assert_eq!(
            d["blockTrades"] != 0.0,
            crate::ui_trade::BlockTrades::default().0
        );
        assert_eq!(d["blockTrades"], 0.0, "an unset blockTrades allows trades");
        let names = NameConfig::default();
        for (cvar, bit) in crate::nameplates::NAME_CVARS {
            let row = registered_rows()
                .find(|r| r.name.eq_ignore_ascii_case(cvar))
                .unwrap_or_else(|| panic!("{cvar} has a row"));
            assert_eq!(
                d[row.name] != 0.0,
                names.has(bit),
                "{cvar}'s bit boots at its row's default"
            );
        }
        assert_eq!(names.0, 0x1bc, "the name registrar's \"1\" rows");
        let camera_opts = crate::player::camera_dynamics::CameraOptions::default();
        assert_eq!(d["cameraPivot"] != 0.0, camera_opts.pivot);
        assert!(camera_opts.pivot, "the binary registers cameraPivot \"1\"");
        assert_eq!(
            d["cameraWaterCollision"] != 0.0,
            camera_opts.water_collision
        );
        assert!(
            camera_opts.pivot && camera_opts.water_collision,
            "the binary registers cameraPivot and cameraWaterCollision both \"1\""
        );
        assert_eq!(d["cameraTerrainTilt"] != 0.0, camera_opts.terrain_tilt);
        assert!(
            !camera_opts.terrain_tilt,
            "the binary registers cameraTerrainTilt \"0\""
        );
        assert_eq!(
            d["cameraGroundSmoothSpeed"],
            camera_opts.ground_smooth_speed
        );
        assert_eq!(d["cameraTerrainTiltTimeMin"], camera_opts.tilt_time_min);
        assert_eq!(d["cameraTerrainTiltTimeMax"], camera_opts.tilt_time_max);
        assert_eq!(d["cameraBobbing"] != 0.0, camera_opts.bobbing);
        assert!(
            !camera_opts.bobbing && !camera_opts.terrain_tilt,
            "the binary registers cameraBobbing and cameraTerrainTilt both \"0\""
        );
        assert_eq!(d["cameraBobbingLRAmplitude"], camera_opts.bob_lr_amplitude);
        assert_eq!(d["cameraBobbingUDAmplitude"], camera_opts.bob_ud_amplitude);
        assert_eq!(d["cameraBobbingFrequency"], camera_opts.bob_frequency);
        assert_eq!(d["cameraBobbingSmoothSpeed"], camera_opts.bob_smooth_speed);
        assert_eq!(d["cameraPivotDXMax"], camera_opts.pivot_dx_max);
        assert_eq!(d["cameraPivotDYMin"], camera_opts.pivot_dy_min);
        assert_eq!(
            d["cameraTargetSmoothSpeed"],
            camera_opts.target_smooth_speed
        );
        assert_eq!(
            d["weatherDensity"],
            f32::from(benilla_world::weather::WeatherState::default().weather_density)
        );
        // `ClutterConfig::default()` reads `$WOW_CLUTTER_DENSITY`; its env-less density is ×1.5.
        assert_eq!(
            d["groundEffectDensity"],
            1.5 * benilla_formats::FRILL_DENSITY as f32
        );
        assert_eq!(
            d["groundEffectDist"],
            benilla_world::clutter::ClutterConfig::default().fade_far
        );
        assert_eq!(
            d["spellEffectLevel"],
            crate::video::SpellEffectLevel::default().0 as f32
        );
        assert_eq!(
            d["componentTextureLevel"],
            f32::from(crate::video::ComponentTextureLevel::default().0)
        );
        assert_eq!(
            d["projectedTextures"] != 0.0,
            benilla_world::view::ProjectedTextures::default().0
        );
        let terrain = benilla_world::terrain_stream::TerrainOptions::default();
        assert_eq!(d["specular"] != 0.0, terrain.specular);
        assert_eq!(d["shadowLevel"] != 0.0, terrain.low_mip);
        // `TexFilterSetting::default()` reads `$WOW_TEXTURE_FILTERING`; the env-less mode is 1.
        assert_eq!(d["textureFilteringMode"], 1.0);
        assert_eq!(d["baseMip"], 0.0);
        let bubbles = BubbleConfig::default();
        assert_eq!(d["ChatBubbles"] != 0.0, bubbles.all);
        assert_eq!(d["ChatBubblesParty"] != 0.0, bubbles.party);
        assert!(bubbles.all && bubbles.party, "the binary's own pair");
        let zoom = MinimapZoom::default();
        assert_eq!(d["minimapZoom"], f32::from(zoom.outdoor));
        assert_eq!(d["minimapInsideZoom"], f32::from(zoom.inside));
        assert_eq!(zoom.outdoor, benilla_ui::widget::MINIMAP_DEFAULT_ZOOM);
        // `video::tests` welds `VideoConfig::vsync` to the window's boot present mode.
        assert_eq!(d["gxVSync"] != 0.0, VideoConfig::default().vsync);
        assert_eq!(d["boothHalfRate"] != 0.0, PaneRate::default().half);
        // Every visual golden assumes a 1:1 backdrop.
        assert_eq!(d["renderScale"], 1.0);
    }

    /// The FFX pass's three switches reach it one by one (`ffx`, `ffxGlow`, `ffxDeath`).
    #[test]
    fn the_ffx_switches_reach_the_pass() {
        use benilla_world::ffx_glow::FfxSwitches;
        let mut app = cvar_app();
        let on = FfxSwitches::default();
        assert_eq!(*res::<FfxSwitches>(&app), on);
        apply(&mut app, "ffxGlow", "0");
        assert_eq!(*res::<FfxSwitches>(&app), FfxSwitches { glow: false, ..on });
        apply(&mut app, "ffxglow", "1");
        apply(&mut app, "ffxDeath", "0");
        assert_eq!(
            *res::<FfxSwitches>(&app),
            FfxSwitches { death: false, ..on }
        );
        apply(&mut app, "ffxDeath", "1");
        apply(&mut app, "ffx", "0");
        assert_eq!(
            *res::<FfxSwitches>(&app),
            FfxSwitches {
                master: false,
                ..on
            }
        );
    }

    /// `gxMaximize` registers latched (flags 3): a write is staged and moves nothing until the
    /// `RestartGx` commit, where the window knob takes it.
    #[test]
    fn gx_maximize_waits_for_the_restart_commit() {
        let mut app = cvar_app();
        let outcome = app
            .world_mut()
            .resource_mut::<Cvars>()
            .set("gxMaximize", "1");
        assert_eq!(outcome, SetOutcome::Staged);
        let events = app.world_mut().resource_mut::<Cvars>().take_events();
        assert!(events.is_empty(), "a staged write fires no callback");
        assert!(!res::<VideoConfig>(&app).maximize);
        assert_eq!(app.world_mut().resource_mut::<Cvars>().commit_latched(), 1);
        let events = app.world_mut().resource_mut::<Cvars>().take_events();
        for event in events {
            app.world_mut().trigger(event);
        }
        assert!(res::<VideoConfig>(&app).maximize, "the commit applies it");
    }

    #[test]
    fn the_observers_apply_every_arm() {
        let mut app = cvar_app();
        apply(&mut app, "Sound_MusicVolume", "0.7");
        assert_eq!(res::<SoundConfig>(&app).music, 0.7);
        // A string row reaches its knob; a value that is not an address leaves the knob alone.
        apply(&mut app, "realmList", "logon.example.org:3724");
        assert_eq!(
            res::<crate::realmlist::Realmlist>(&app).address(),
            "logon.example.org:3724"
        );
        apply(
            &mut app,
            "realmlist",
            r#"SET realmlist "elsewhere.example.org""#,
        );
        assert_eq!(
            res::<crate::realmlist::Realmlist>(&app).address(),
            "elsewhere.example.org"
        );
        apply(&mut app, "realmList", "not an address");
        assert_eq!(
            res::<crate::realmlist::Realmlist>(&app).address(),
            "elsewhere.example.org",
            "a known key with a bad value is consumed, and the resource keeps its truth",
        );
        // Clamps are the knob's own: volume to [0,1], farclip to the map's validated range.
        apply(&mut app, "mastervolume", "7");
        assert_eq!(res::<SoundConfig>(&app).master, 1.0);
        apply(&mut app, "farclip", "50");
        assert_eq!(res::<ViewDistance>(&app).farclip, 550.0 / 3.0);
        apply(&mut app, "farclip", "1277");
        assert_eq!(
            res::<ViewDistance>(&app).farclip,
            2375.0 / 3.0,
            "the old continents cap at 2375/3 yd"
        );
        assert_eq!(res::<ViewDistance>(&app).setting, 1277.0);
        // `nearclip` clamps to the callback's `[0.01, 0.33]`.
        apply(&mut app, "nearclip", "0.001");
        assert_eq!(
            res::<ViewDistance>(&app).nearclip,
            0.01,
            "the callback's low bound"
        );
        apply(&mut app, "nearclip", "9");
        assert_eq!(res::<ViewDistance>(&app).nearclip, 0.33, "its high bound");
        apply(&mut app, "nearclip", "0.3");
        assert_eq!(res::<ViewDistance>(&app).nearclip, 0.3);
        // A sample count, clamped to the reference's [1, 16]; 1 is none.
        apply(&mut app, "gxMultisample", "4");
        assert_eq!(res::<MsaaSetting>(&app).samples, 4);
        // `textureFilteringMode`: 0 to 5, anything else refused and put back.
        apply(&mut app, "textureFilteringMode", "5");
        assert_eq!(res::<benilla_assets::TexFilterSetting>(&app).mode, 5);
        apply(&mut app, "textureFilteringMode", "6");
        assert_eq!(res::<benilla_assets::TexFilterSetting>(&app).mode, 5);
        assert_eq!(res::<Cvars>(&app).get("textureFilteringMode"), Some("5"));
        apply(&mut app, "baseMip", "1");
        assert_eq!(res::<benilla_assets::TexFilterSetting>(&app).base_mip, 1);
        apply(&mut app, "baseMip", "2");
        assert_eq!(res::<benilla_assets::TexFilterSetting>(&app).base_mip, 1);
        assert_eq!(res::<Cvars>(&app).get("baseMip"), Some("1"));
        // Clamped to the device's ceiling too: this GPU offers 4, and wgpu refuses a count it
        // lacks.
        apply(&mut app, "gxmultisample", "99");
        assert_eq!(res::<MsaaSetting>(&app).samples, 4);
        // The realistic route in: a config written where 8x exists, opened where it does not.
        apply(&mut app, "gxMultisample", "8");
        assert_eq!(
            res::<MsaaSetting>(&app).samples,
            4,
            "a device that stops at 4x must never be handed an 8"
        );
        apply(&mut app, "gxmultisample", "2");
        assert_eq!(res::<MsaaSetting>(&app).samples, 2);
        apply(&mut app, "gxmultisample", "0");
        assert_eq!(res::<MsaaSetting>(&app).samples, *MSAA_RANGE.start());
        apply(&mut app, "renderScale", "0.75");
        assert_eq!(res::<RenderScale>(&app).0, 0.75);
        apply(&mut app, "renderscale", "9");
        assert_eq!(res::<RenderScale>(&app).0, *RENDER_SCALE_RANGE.end());
        apply(&mut app, "renderscale", "0");
        assert_eq!(res::<RenderScale>(&app).0, *RENDER_SCALE_RANGE.start());
        assert!(!res::<crate::perf::FpsJournalSetting>(&app).0);
        apply(&mut app, "fpsJournal", "1");
        assert!(res::<crate::perf::FpsJournalSetting>(&app).0);
        apply(&mut app, "fpsjournal", "0");
        assert!(!res::<crate::perf::FpsJournalSetting>(&app).0);
        // Enable flags: any nonzero is on, zero is off (the client's int-parse + != 0).
        apply(&mut app, "Sound_EnableMusic", "0");
        assert!(!res::<SoundConfig>(&app).music_enabled);
        apply(&mut app, "Sound_EnableSFX", "0");
        assert!(!res::<SoundConfig>(&app).sfx_enabled);
        apply(&mut app, "sound_enableallsound", "1");
        assert!(res::<SoundConfig>(&app).enabled);
        apply(&mut app, "deselectonclick", "0");
        assert!(!res::<ClickConfig>(&app).deselect_on_click);
        apply(&mut app, "MouseInvertPitch", "1");
        assert!(res::<LookConfig>(&app).invert_pitch);
        apply(&mut app, "mousespeed", "1.4");
        assert_eq!(res::<LookConfig>(&app).sensitivity, 1.4);
        apply(&mut app, "mousespeed", "9");
        assert_eq!(res::<LookConfig>(&app).sensitivity, 1.5);
        // The engine's enum (0 Never, 1 Smart, 2 Always); 3, the validator's upper bound, is Never.
        apply(&mut app, "cameraSmoothStyle", "0");
        assert_eq!(res::<FollowConfig>(&app).style, FollowStyle::Never);
        apply(&mut app, "camerasmoothstyle", "2");
        assert_eq!(res::<FollowConfig>(&app).style, FollowStyle::Always);
        apply(&mut app, "cameraSmoothStyle", "3");
        assert_eq!(res::<FollowConfig>(&app).style, FollowStyle::Never);
        apply(&mut app, "cameraSmoothStyle", "1");
        assert_eq!(res::<FollowConfig>(&app).style, FollowStyle::Smart);
        apply(&mut app, "cameraSmoothTrackingStyle", "2");
        assert_eq!(
            res::<FollowConfig>(&app).tracking_style,
            FollowStyle::Always
        );
        assert_eq!(
            res::<FollowConfig>(&app).style,
            FollowStyle::Smart,
            "and only that one"
        );
        apply(&mut app, "cameraYawSmoothSpeed", "270");
        assert_eq!(res::<FollowConfig>(&app).yaw_speed, 270.0);
        apply(&mut app, "cameraYawSmoothSpeed", "9000");
        assert_eq!(
            res::<FollowConfig>(&app).yaw_speed,
            *FOLLOW_SPEED_RANGE.end()
        );
        // The max orbit lands as yards on the knob: `cameraDistanceMax` x the factor, held to
        // [0, 50]; the factor is not held to its slider's 1 to 2.
        apply(&mut app, "cameraDistanceMaxFactor", "1");
        assert_eq!(res::<ZoomLimit>(&app).max, 15.0);
        apply(&mut app, "cameradistancemaxfactor", "3");
        assert_eq!(res::<ZoomLimit>(&app).max, 45.0);
        apply(&mut app, "cameraDistanceMaxFactor", "1");
        apply(&mut app, "cameraDistanceMax", "25");
        assert_eq!(res::<ZoomLimit>(&app).max, 25.0);
        apply(&mut app, "cameraDistanceMaxFactor", "3");
        assert_eq!(res::<ZoomLimit>(&app).max, 50.0);
        apply(&mut app, "cameraDistanceMaxFactor", "-1");
        assert_eq!(res::<ZoomLimit>(&app).max, 0.0);
        apply(&mut app, "cameraDistanceMaxFactor", "nan");
        assert_eq!(res::<ZoomLimit>(&app).max, 50.0);
        // Out of `cameraDistanceMax`'s validator range: refused at the knob, 25 stands.
        apply(&mut app, "cameraDistanceMaxFactor", "1");
        apply(&mut app, "cameraDistanceMax", "60");
        assert_eq!(res::<ZoomLimit>(&app).max, 25.0);
        apply(&mut app, "autoLootDefault", "1");
        assert!(res::<LootConfig>(&app).auto_loot);
        apply(&mut app, "showLootSpam", "0");
        assert!(!res::<LootConfig>(&app).show_loot_spam);
        apply(&mut app, "guildMemberNotify", "1");
        assert!(res::<crate::ui_guild::GuildMemberNotify>(&app).0);
        apply(&mut app, "blockTrades", "1");
        assert!(res::<crate::ui_trade::BlockTrades>(&app).0);
        use crate::nameplates::name_bit;
        apply(&mut app, "UnitNameNPC", "1");
        assert!(res::<NameConfig>(&app).has(name_bit::NPC));
        apply(&mut app, "unitnameown", "1");
        assert!(res::<NameConfig>(&app).has(name_bit::OWN));
        apply(&mut app, "unitnameplayerpvptitle", "0");
        assert!(!res::<NameConfig>(&app).has(name_bit::PLAYER_PVP_TITLE));
        apply(&mut app, "UnitNameFriendlyGuardianName", "1");
        assert!(res::<NameConfig>(&app).has(name_bit::FRIENDLY_GUARDIAN));
        apply(&mut app, "ChatBubbles", "0");
        assert!(!res::<BubbleConfig>(&app).all);
        apply(&mut app, "chatbubblesparty", "0");
        assert!(!res::<BubbleConfig>(&app).party);
        // Ground clutter: cells per chunk in [16, 256], reach in [0, 140]; outside, refused.
        apply(&mut app, "groundEffectDensity", "64");
        assert_eq!(res::<ClutterConfig>(&app).cells(), 64.0);
        apply(&mut app, "groundEffectDensity", "8");
        assert_eq!(res::<ClutterConfig>(&app).cells(), 64.0);
        assert_eq!(res::<Cvars>(&app).get("groundEffectDensity"), Some("64"));
        apply(&mut app, "groundEffectDist", "140");
        assert_eq!(res::<ClutterConfig>(&app).fade_far, 140.0);
        apply(&mut app, "groundEffectDist", "200");
        assert_eq!(res::<ClutterConfig>(&app).fade_far, 140.0);
        // `particleDensity` in [0.1, 1.0]; outside, refused.
        apply(&mut app, "particleDensity", "0.4");
        assert_eq!(
            res::<benilla_world::particles::ParticleTuning>(&app).density(),
            0.4
        );
        apply(&mut app, "particleDensity", "0.05");
        assert_eq!(
            res::<benilla_world::particles::ParticleTuning>(&app).density(),
            0.4
        );
        // Terrain Texture and Terrain Highlights, live.
        apply(&mut app, "shadowLevel", "0");
        assert!(!res::<benilla_world::terrain_stream::TerrainOptions>(&app).low_mip);
        apply(&mut app, "shadowLevel", "2");
        assert!(
            !res::<benilla_world::terrain_stream::TerrainOptions>(&app).low_mip,
            "refused above 1"
        );
        apply(&mut app, "specular", "0");
        assert!(!res::<benilla_world::terrain_stream::TerrainOptions>(&app).specular);
        apply(&mut app, "projectedTextures", "0");
        assert!(!res::<benilla_world::view::ProjectedTextures>(&app).0);
        apply(&mut app, "componentTextureLevel", "9");
        assert_eq!(res::<crate::video::ComponentTextureLevel>(&app).0, 9);
        apply(&mut app, "componentTextureLevel", "4");
        assert_eq!(res::<crate::video::ComponentTextureLevel>(&app).0, 7);
        // `spellEffectLevel` has no callback: the record's integer, read by the shard emitter.
        apply(&mut app, "spellEffectLevel", "3");
        assert_eq!(res::<crate::video::SpellEffectLevel>(&app).0, 3);
        // `environmentDetail` reaches the doodad fade scale, clamped to [0.5, 1.5].
        apply(&mut app, "environmentDetail", "9");
        assert_eq!(benilla_world::model_fade::fade_scale(), 1.5);
        apply(&mut app, "environmentDetail", "1.0");
        assert_eq!(benilla_world::model_fade::fade_scale(), 1.0);
        for (wrote, want) in [
            ("0", 0u8),
            ("1", 1),
            ("2", 2),
            ("3", 3),
            ("2.9", 2),
            ("9", 3),
            ("-4", 0),
        ] {
            apply(&mut app, "weatherDensity", wrote);
            assert_eq!(
                res::<benilla_world::weather::WeatherState>(&app).weather_density,
                want,
                "weatherDensity {wrote}"
            );
        }
        // `gamma` is the ramp exponent: `SetGamma` applies `1 - v` before it arrives here. The
        // consumer clamps what the reference accepts unclamped (`SetGamma(5)` writes -4 there).
        for (wrote, want) in [("1.000000", 1.0), ("0.500000", 0.5), ("1.500000", 1.5)] {
            apply(&mut app, "gamma", wrote);
            assert_eq!(
                res::<crate::ui_gamma::DisplayGamma>(&app).0,
                want,
                "gamma {wrote}"
            );
        }
        apply(&mut app, "gamma", "-4.000000");
        assert_eq!(
            res::<crate::ui_gamma::DisplayGamma>(&app).0,
            1.0,
            "the callback reads an exponent under 0.001 as 1.0 (`0x401a30`)"
        );
        apply(&mut app, "gamma", "0.1");
        assert_eq!(
            res::<crate::ui_gamma::DisplayGamma>(&app).0,
            *crate::ui_gamma::GAMMA_RANGE.start(),
            "a tiny exponent clamps at the consumer, where it cannot blank the screen"
        );
        apply(&mut app, "gamma", "99");
        assert_eq!(
            res::<crate::ui_gamma::DisplayGamma>(&app).0,
            *crate::ui_gamma::GAMMA_RANGE.end()
        );
        // The same for `GetGamma`/`SetGamma`, which read and write `1 - gamma`.
        assert!(REGISTERED
            .iter()
            .any(|r| r.name == benilla_ui::script::CVAR_GAMMA));
        // The Restore verbs' row lists live in `benilla-ui` too; an unregistered row would be
        // skipped silently.
        for key in benilla_ui::script::VIDEO_RESOLUTION_DEFAULT_CVARS
            .iter()
            .chain(benilla_ui::script::VIDEO_EFFECTS_DEFAULT_CVARS)
        {
            assert!(
                registered_rows().any(|r| r.name.eq_ignore_ascii_case(key)),
                "{key}: a Restore verb would restore it, and nothing registers it"
            );
        }
        // The video pairs write by name; a missing row would warn and store nothing.
        for key in benilla_ui::script::VIDEO_PAIR_CVARS {
            assert!(
                registered_rows().any(|r| r.name.eq_ignore_ascii_case(key)),
                "{key}: a video verb writes it, and nothing registers it"
            );
        }
        apply(&mut app, "minimapZoom", "5");
        assert_eq!(res::<MinimapZoom>(&app).outdoor, 5);
        assert_eq!(
            res::<MinimapZoom>(&app).inside,
            3,
            "the two indices are independent"
        );
        apply(&mut app, "minimapinsidezoom", "9");
        assert_eq!(res::<MinimapZoom>(&app).inside, MINIMAP_ZOOM_LEVELS - 1);
        apply(&mut app, "minimapZoom", "-2");
        assert_eq!(res::<MinimapZoom>(&app).outdoor, 0);
        assert_eq!(apply(&mut app, "uiScale", "banana"), SetOutcome::Refused);
        assert_eq!(res::<UiScaleSetting>(&app).ui_scale, 1.0);
        apply(&mut app, "uiScale", "0.75");
        apply(&mut app, "useUiScale", "1");
        let ui = res::<UiScaleSetting>(&app);
        assert!(ui.use_ui_scale);
        assert_eq!(ui.ui_scale, 0.75);
        assert_eq!(apply(&mut app, "bogus", "1"), SetOutcome::Unknown);
    }

    /// A refused write echoes its callback's line to the console and leaves the row as it was.
    #[test]
    fn a_refused_write_echoes_the_callbacks_line() {
        let mut app = cvar_app();
        app.init_resource::<crate::console::ConsoleEcho>();
        assert_eq!(
            crate::console::execute(app.world_mut(), "particleDensity 5"),
            ["Value must be between 0.1 and 1.0."]
        );
        assert_eq!(res::<Cvars>(&app).get("particleDensity"), Some("1.0"));
        assert_eq!(
            crate::console::execute(app.world_mut(), "farclip 400"),
            Vec::<String>::new(),
            "a callback that prints nothing adds nothing"
        );
    }

    #[test]
    fn compose_writes_the_diff_and_preserves_what_it_does_not_own() {
        let mut cvars = Cvars::default();
        cvars.own_for_session("uiScale", Some("1.2")); // env-overridden this session
        cvars.load_file(
            [
                ("FutureKnob".to_string(), "3".to_string()), // a newer build's key: preserved
                ("uiScale".to_string(), "0.8".to_string()),  // the file's own, kept as found
                ("farclip".to_string(), "400".to_string()),  // will return to default
                ("Sound_MusicVolume".to_string(), "0.3".to_string()),
            ]
            .into(),
        );
        cvars.set("Sound_MusicVolume", "0.7"); // moved: written
        cvars.set("Sound_AmbienceVolume", "0.6"); // set at its default: removed
        cvars.set("farclip", "777"); // back to ours, which 12340 boots into over its "350"
        let out: BTreeMap<String, String> = cvars.compose().into_iter().collect();
        assert_eq!(
            out.get("Sound_MusicVolume").map(String::as_str),
            Some("0.7")
        );
        assert!(
            !out.contains_key("Sound_MasterVolume"),
            "at default: absent"
        );
        assert_eq!(out.get("uiScale").map(String::as_str), Some("0.8"));
        assert!(!out.contains_key("Sound_AmbienceVolume"));
        assert_eq!(
            out.get("farclip").map(String::as_str),
            Some("777"),
            "compared with 12340's registered string, as its own file keeps the boot's value"
        );
        assert_eq!(out.get("FutureKnob").map(String::as_str), Some("3"));
    }

    /// `Config.wtf` in the reference's list order: the file's names first, then the rest in
    /// registration order; a name the file holds twice is written once, a latched stage is what is
    /// archived, and the default compares case-blind.
    #[test]
    fn compose_keeps_the_files_order_and_archives_the_stage() {
        let mut cvars = Cvars::default();
        cvars.load_file(vec![
            ("Zeta".to_string(), "1".to_string()),
            ("Sound_MusicVolume".to_string(), "0.3".to_string()),
            ("zeta".to_string(), "2".to_string()),
        ]);
        cvars.set("farclip", "500");
        cvars.set("gxWindow", "0");
        cvars.set("gxWindow", "1");
        let names: Vec<String> = cvars.compose().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names[..2], ["Zeta", "Sound_MusicVolume"]);
        assert_eq!(
            names
                .iter()
                .filter(|n| n.eq_ignore_ascii_case("zeta"))
                .count(),
            1
        );
        assert!(names.contains(&"farclip".to_string()));
        let gx = cvars.compose().into_iter().find(|(n, _)| n == "gxWindow");
        assert!(gx.is_none() || gx.unwrap().1 == "1");
        let mut case = Cvars::default();
        case.set("chatStyle", "IM");
        assert!(
            case.compose_scope(Scope::Account).is_empty(),
            "IM is the default im, compared case-blind"
        );
    }

    /// The stat dropdowns are per-character (flags 0x20): never read from or written to
    /// `Config.wtf`, archived in the character's `config-cache.wtf`, and back at "" after the UI
    /// shutdown drops the scope.
    #[test]
    fn a_per_character_row_lives_in_its_scope_file_and_drops_at_shutdown() {
        let mut cvars = Cvars::default();
        assert_eq!(cvars.get("playerStatLeftDropdown"), Some(""));
        cvars.load_file([("playerStatRightDropdown".to_string(), "X".to_string())].into());
        assert_eq!(cvars.get("playerStatRightDropdown"), Some(""), "not read");
        cvars.set("playerStatLeftDropdown", "PLAYERSTAT_SPELL_COMBAT");
        let out: BTreeMap<String, String> = cvars.compose().into_iter().collect();
        assert!(
            !out.contains_key("playerStatLeftDropdown"),
            "not in Config.wtf"
        );
        assert!(
            !out.contains_key("playerStatRightDropdown"),
            "nor the stale entry"
        );
        assert_eq!(
            cvars.compose_scope(Scope::Character),
            "SET playerStatLeftDropdown \"PLAYERSTAT_SPELL_COMBAT\"\n"
        );
        cvars.reset_scope(Scope::Character);
        assert_eq!(cvars.get("playerStatLeftDropdown"), Some(""));
        cvars.load_scope(
            Scope::Character,
            "SET playerStatLeftDropdown \"PLAYERSTAT_MELEE_COMBAT\"\nSET newKnob \"4\"\n",
        );
        assert_eq!(
            cvars.get("playerStatLeftDropdown"),
            Some("PLAYERSTAT_MELEE_COMBAT"),
            "the next login reads it back"
        );
        assert!(
            cvars
                .compose_scope(Scope::Character)
                .contains("SET newKnob \"4\""),
            "a name no row claims goes back to the file it came from"
        );
        assert!(
            !cvars
                .compose()
                .iter()
                .any(|(n, v)| n == "newKnob" && v == "4"),
            "an unclaimed name in a scope file becomes a Config.wtf placeholder (0x768307)"
        );
    }

    /// No save on a timer: a change waits for a save point, here Enter World's.
    #[test]
    fn a_change_waits_for_a_save_point() {
        use crate::local_state::test_env::{EnvGuard, ENV_LOCK};
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-cvar-quiet-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _u = EnvGuard::unset("WOW_UI_SCALE");
        let _f = EnvGuard::unset("WOW_FARCLIP");
        let _d = EnvGuard::unset("WOW_CLUTTER_DENSITY");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());
        let _w = EnvGuard::set("WOW_DATA", "");

        let mut app = cvar_app();
        app.update();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(r#"SetCVar("Sound_MusicVolume", 0.75)"#)
            .unwrap();
        app.update();
        app.update();
        assert!(
            !tmp.join("WTF/Config.wtf").exists(),
            "nothing between save points"
        );
        app.world_mut().write_message(SaveConfig);
        app.update();
        let text = std::fs::read_to_string(tmp.join("WTF/Config.wtf")).unwrap();
        assert!(text.contains("SET Sound_MusicVolume \"0.75\""), "{text}");
        assert!(!app.world().resource::<Cvars>().dirty);
        assert!(app.should_exit().is_none());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_lua_setcvar_lands_in_config_wtf_end_to_end() {
        use crate::local_state::test_env::{EnvGuard, ENV_LOCK};
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-cvar-e2e-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _u = EnvGuard::unset("WOW_UI_SCALE");
        let _f = EnvGuard::unset("WOW_FARCLIP");
        let _d = EnvGuard::unset("WOW_CLUTTER_DENSITY");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());
        let _w = EnvGuard::set("WOW_DATA", "");
        crate::local_state::write_atomic(
            &tmp.join("WTF/Config.wtf"),
            "SET Sound_MusicVolume \"0.1\"\n",
        )
        .unwrap();

        let mut app = cvar_app();

        // Startup: the file's MusicVolume reaches the knob; Update: the VM table seeds from it.
        app.update();
        assert_eq!(app.world().resource::<SoundConfig>().music, 0.1);
        assert_eq!(
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .cvar("Sound_MusicVolume")
                .as_deref(),
            Some("0.1")
        );

        // The Lua write (what a settings slider will do) reaches the knob on the next frame…
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(r#"SetCVar("Sound_MusicVolume", 0.75)"#)
            .unwrap();
        app.update();
        assert_eq!(app.world().resource::<SoundConfig>().music, 0.75);

        // …and the exit flush writes the diff: the moved value, nothing at its default.
        app.world_mut().write_message(AppExit::Success);
        app.update();
        let text = std::fs::read_to_string(tmp.join("WTF/Config.wtf")).unwrap();
        assert!(text.contains("SET Sound_MusicVolume \"0.75\""), "{text}");
        assert!(!text.contains("MasterVolume"), "defaults stay out:\n{text}");
        // A diff, not a dump: past the moved value, only the rows whose default is not the string
        // 12340 registers, which its own file carries the same way.
        for (name, _) in wtf::parse(&text) {
            let row = app.world().resource::<Cvars>().row(&name).cloned().unwrap();
            assert!(
                name == "Sound_MusicVolume" || row.default != row.archive_default,
                "{name} written at the reference's default: {text}"
            );
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// [`Cvars::compose`] as a map, for the tests that ask what it holds rather than its order.
    fn composed(cvars: &Cvars) -> BTreeMap<String, String> {
        cvars.compose().into_iter().collect()
    }

    /// A client with a real CVar host: every knob resource an observer writes, every observer,
    /// [`CvarPlugin`] and a VM for the mirror.
    fn cvar_app() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins)
            .insert_resource(SoundConfig::default())
            .insert_resource(UiScaleCvar(1.0))
            .insert_resource(UiScaleSetting {
                ui_scale: 1.0,
                use_ui_scale: false,
                env: None,
            })
            .insert_resource(ViewDistance {
                farclip: benilla_world::view::FARCLIP_DEFAULT,
                setting: benilla_world::view::FARCLIP_DEFAULT,
                nearclip: benilla_world::view::NEARCLIP_DEFAULT,
            })
            .insert_resource(MsaaSetting { samples: 1 })
            // Literal: `TexFilterSetting::default()` reads `$WOW_TEXTURE_FILTERING`.
            .insert_resource(benilla_assets::TexFilterSetting {
                mode: 1,
                base_mip: 0,
            })
            .init_resource::<benilla_world::terrain_stream::TerrainOptions>()
            .init_resource::<benilla_world::view::ProjectedTextures>()
            .init_resource::<crate::video::ComponentTextureLevel>()
            .init_resource::<crate::ui_gamma::GammaRows>()
            // A real-shaped device menu: `GetCurrentMultisampleFormat` needs rows to find.
            .insert_resource(benilla_world::view::MsaaFormats {
                formats: vec![(32, 32, 1), (32, 32, 2), (32, 32, 4)],
            })
            .init_resource::<LookConfig>()
            .init_resource::<crate::player::camera_dynamics::CameraOptions>()
            .init_resource::<crate::combat_text::DamageTextGates>()
            .init_resource::<benilla_world::weather::WeatherState>()
            .init_resource::<benilla_world::particles::ParticleTuning>()
            .init_resource::<crate::video::SpellEffectLevel>()
            .init_resource::<crate::ui_gamma::DisplayGamma>()
            .init_resource::<ClickConfig>()
            .init_resource::<crate::target::AssistAttack>()
            .init_resource::<crate::target::StopAttackOnTargetChange>()
            .init_resource::<LootConfig>()
            .init_resource::<NameConfig>()
            .init_resource::<ClutterConfig>()
            .init_resource::<MinimapZoom>()
            .init_resource::<crate::minimap::MinimapRotate>()
            .init_resource::<BubbleConfig>()
            .init_resource::<ZoomLimit>()
            .init_resource::<FollowConfig>()
            .init_resource::<VideoConfig>()
            .init_resource::<benilla_world::ffx_glow::FfxSwitches>()
            // Literal, not Default: RenderScale::default() reads $WOW_RENDER_SCALE.
            .insert_resource(RenderScale(1.0))
            // Literal: `Realmlist::default()` reads `$WOW_HOST`.
            .insert_resource(crate::realmlist::Realmlist::unpinned(
                crate::realmlist::DEFAULT_REALMLIST,
            ))
            .init_resource::<PaneRate>()
            .init_resource::<crate::ui_guild::GuildMemberNotify>()
            .init_resource::<crate::ui_trade::BlockTrades>()
            .init_resource::<crate::spell::AutoSelfCast>()
            .init_resource::<crate::spell::AutoDismount>()
            .init_resource::<crate::spell::SecureAbilityToggle>()
            .init_resource::<crate::perf::FpsJournalSetting>()
            .init_resource::<crate::text_filter::TextFilterSwitches>()
            .init_resource::<crate::game_tip::GameTipSetting>()
            .add_plugins(CvarPlugin);
        for observer in ALL_OBSERVERS {
            observer(&mut app);
        }
        app.insert_non_send_resource(UiScript::new().unwrap());
        app
    }

    /// Every CVar observer in the crate; a new one is a line here and an `add_observer` in its
    /// plugin.
    const ALL_OBSERVERS: &[fn(&mut App)] = &[
        |app| {
            app.add_observer(crate::sound::on_cvar);
        },
        |app| {
            app.add_observer(crate::ui_script::on_cvar);
        },
        |app| {
            app.add_observer(crate::video::on_cvar);
        },
        |app| {
            app.add_observer(crate::player::camera::on_cvar);
        },
        |app| {
            app.add_observer(crate::player::camera_dynamics::on_cvar);
        },
        |app| {
            app.add_observer(crate::target::on_cvar);
        },
        |app| {
            app.add_observer(crate::spell::cast_target::on_cvar);
        },
        |app| {
            app.add_observer(crate::combat_text::on_cvar);
        },
        |app| {
            app.add_observer(crate::ui_loot::on_cvar);
        },
        |app| {
            app.add_observer(crate::nameplates::on_cvar);
        },
        |app| {
            app.add_observer(crate::game_tip::on_cvar);
        },
        |app| {
            app.add_observer(crate::text_filter::on_cvar);
        },
        |app| {
            app.add_observer(crate::chat_bubble::on_cvar);
        },
        |app| {
            app.add_observer(crate::ui_guild::on_cvar);
        },
        |app| {
            app.add_observer(crate::ui_trade::on_cvar);
        },
        |app| {
            app.add_observer(crate::minimap::on_cvar);
        },
        |app| {
            app.add_observer(crate::portrait::on_cvar);
        },
        |app| {
            app.add_observer(crate::perf::on_cvar);
        },
        |app| {
            app.add_observer(crate::realmlist::on_cvar);
        },
        |app| {
            app.add_observer(crate::ui_gamma::on_cvar);
        },
        |app| {
            app.add_observer(crate::world_backdrop::on_cvar);
        },
    ];

    /// A host write, its latch committed at once, and its observers run before this returns.
    fn apply(app: &mut App, name: &str, value: &str) -> SetOutcome {
        let world = app.world_mut();
        let (outcome, events) = {
            let mut cvars = world.resource_mut::<Cvars>();
            let outcome = cvars.set(name, value);
            cvars.commit_latched();
            (outcome, cvars.take_events())
        };
        for event in events {
            world.trigger(event);
        }
        outcome
    }

    fn res<T: Resource>(app: &App) -> &T {
        app.world().resource::<T>()
    }

    /// The last character entered survives a quit: two launches over one `benilla-config/` with the
    /// real [`CvarPlugin`] and [`crate::char_select`] systems, so the screen's host write must
    /// reach the file through the registry's dirty/compose path.
    #[test]
    fn entering_the_world_survives_the_quit_and_comes_back_selected() {
        use crate::char_select::{ClientState, Roster};
        use crate::local_state::test_env::{EnvGuard, ENV_LOCK};
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-lastchar-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _u = EnvGuard::unset("WOW_UI_SCALE");
        let _f = EnvGuard::unset("WOW_FARCLIP");
        let _d = EnvGuard::unset("WOW_CLUTTER_DENSITY");
        let _w = EnvGuard::unset("WOW_CHAR");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());
        let _w = EnvGuard::set("WOW_DATA", "");
        let roster = || {
            (1..=4)
                .map(|g| crate::char_select::test_character(g, &format!("Char{g}")))
                .collect::<Vec<_>>()
        };

        // ── Launch 1: the roster lands, and the player enters the world as the third row. ────
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = cvar_app();
        app.add_plugins(bevy::state::app::StatesPlugin);
        crate::char_select::add_test_systems(&mut app, tx);
        app.update(); // Startup loads the (absent) file; the first Update seeds the VM table
        app.world_mut().write_message(crate::net::CharListMessage {
            characters: roster(),
            realm: None,
            cause: benilla_protocol::RosterCause::Park,
            epoch: 0,
        });
        app.update();
        app.world_mut().resource_mut::<Roster>().pending_pick = Some(3); // guid 3 = row 2
        app.update();
        app.world_mut().write_message(AppExit::Success);
        app.update();

        let text = std::fs::read_to_string(tmp.join("WTF/Config.wtf")).unwrap();
        assert!(
            text.contains("SET lastCharacterIndex \"2\""),
            "entering the world must reach the file, 0-based like Config.wtf:\n{text}"
        );

        // ── Launch 2: a fresh client over the same folder, and the roster arrives. ───────────
        let (tx, _rx2) = crossbeam_channel::unbounded();
        let mut app = cvar_app();
        app.add_plugins(bevy::state::app::StatesPlugin);
        crate::char_select::add_test_systems(&mut app, tx);
        app.update();
        app.world_mut().write_message(crate::net::CharListMessage {
            characters: roster(),
            realm: None,
            cause: benilla_protocol::RosterCause::Park,
            epoch: 0,
        });
        app.update();

        assert_eq!(
            res::<Cvars>(&app).get(crate::char_select::CVAR_LAST_CHARACTER),
            Some("2"),
            "the second launch must read the SAME row back, which the stock screen selects",
        );
        assert_eq!(
            *app.world().resource::<State<ClientState>>().get(),
            ClientState::CharSelect,
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// `minimapZoom` is per character (`0x20`): it comes back from the character's
    /// `config-cache.wtf` and goes out in it, never in `Config.wtf`.
    #[test]
    fn a_minimap_setzoom_reaches_the_knob_and_the_characters_file() {
        let mut app = cvar_app();
        app.update();
        // The character's file says the outdoor map was zoomed right in.
        let events = {
            let mut cvars = app.world_mut().resource_mut::<Cvars>();
            cvars.load_scope(Scope::Character, "SET minimapZoom \"5\"\n");
            cvars.take_events()
        };
        for e in events {
            app.world_mut().trigger(e);
        }
        app.update();
        assert_eq!(app.world().resource::<MinimapZoom>().outdoor, 5);
        assert_eq!(app.world().resource::<MinimapZoom>().inside, 3);
        let seed = {
            let z = app.world().resource::<MinimapZoom>();
            (z.outdoor, z.inside)
        };
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            assert_eq!(script.cvar("minimapZoom").as_deref(), Some("5"));
            // The widget is born first, then the persisted level is pushed into it: seeding a
            // widget that does not exist is a no-op.
            script.run(r#"m = CreateFrame("Minimap", "Mini")"#).unwrap();
            script.set_minimap_zoom(seed.0, seed.1);
            assert_eq!(script.eval::<u8>("return m:GetZoom()").unwrap(), 5);
            script.run("m:SetZoom(m:GetZoom() - 2)").unwrap();
        }
        app.update();
        assert_eq!(app.world().resource::<MinimapZoom>().outdoor, 3);
        let cvars = app.world().resource::<Cvars>();
        assert!(
            !cvars
                .compose_scope(Scope::Character)
                .contains("minimapZoom"),
            "back at the registered default 3, so it leaves the file"
        );
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("m:SetZoom(1)")
            .unwrap();
        app.update();
        let cvars = app.world().resource::<Cvars>();
        assert!(cvars
            .compose_scope(Scope::Character)
            .contains("SET minimapZoom \"1\""));
        assert!(
            !composed(cvars).contains_key("minimapZoom"),
            "never in Config.wtf"
        );
        assert_eq!(app.world().resource::<MinimapZoom>().outdoor, 1);
    }

    /// The string-valued rows, a closed list, each default checked on its own terms: `realmName`
    /// (the reference's registered `""`) and `gxApi` are empty until the session writes them;
    /// `screenshotFormat` is `jpeg`; `gxResolution` and `realmList` pass their observers' parsers
    /// ([`crate::video::parse_resolution`], [`crate::realmlist::normalize`]).
    #[test]
    fn the_string_valued_cvars_are_the_realm_and_the_windowed_size() {
        let mut strings: Vec<&str> = REGISTERED
            .iter()
            .filter(|r| r.default.parse::<f32>().is_err())
            .map(|r| r.name)
            .collect();
        strings.sort_unstable(); // the list is the claim, not where the rows sit in the table
        assert_eq!(
            strings,
            vec![
                "gxApi",
                "gxResolution",
                "realmList",
                "realmName",
                "screenshotFormat"
            ]
        );
        let default_of = |name: &str| {
            REGISTERED
                .iter()
                .find(|r| r.name == name)
                .map(|r| r.default)
                .expect("registered")
        };
        assert_eq!(default_of("realmName"), "");
        assert_eq!(default_of("gxApi"), "");
        assert_eq!(
            default_of("screenshotFormat"),
            "jpeg",
            "anything but tga is a JPEG"
        );
        assert_eq!(
            default_of("gxResolution"),
            "",
            "unpicked: the desktop's mode fullscreen, `DEFAULT_WINDOWED` in a window"
        );
        // A default `realmlist::normalize` rejects would ship a client that cannot dial.
        assert_eq!(
            crate::realmlist::normalize(default_of(crate::realmlist::CVAR_REALMLIST)).as_deref(),
            Some(crate::realmlist::DEFAULT_REALMLIST),
        );
    }

    /// A registry loaded from `file`, with no disk.
    fn registry(file: &[(&str, &str)]) -> Cvars {
        let mut cvars = Cvars::default();
        cvars.load_file(
            file.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        );
        cvars
    }

    /// A latched row's write is staged: the applied value stands and nothing fires or dirties (the
    /// reference's set stores the latched value and skips the internal write) until the commit
    /// applies, fires, persists and mirrors it.
    #[test]
    fn a_latched_row_stages_the_write_until_the_boundary_commits_it() {
        let mut cvars = Cvars::default();
        assert!(
            cvars.row("gxVSync").unwrap().latched,
            "the reference's flags=3 row"
        );
        assert_eq!(cvars.set("gxVSync", "0"), SetOutcome::Staged);
        assert_eq!(cvars.get("gxVSync"), Some("1"), "applied value stands");
        assert_eq!(cvars.row("gxVSync").unwrap().pending.as_deref(), Some("0"));
        assert!(!cvars.has_events(), "nothing fires before the boundary");
        assert!(!cvars.dirty, "nothing to save before the boundary");
        assert_eq!(
            cvars.set("gxVSync", "0"),
            SetOutcome::Unchanged,
            "same stage"
        );
        assert_eq!(
            composed(&cvars).get("gxVSync").map(String::as_str),
            Some("0"),
            "the archive writes the stage (`0x767077`), committed or not"
        );
        assert_eq!(cvars.commit_latched(), 1);
        assert_eq!(cvars.get("gxVSync"), Some("0"));
        assert_eq!(cvars.row("gxVSync").unwrap().pending, None);
        assert_eq!(
            cvars.take_events(),
            vec![CvarChanged {
                name: "gxVSync".into(),
                old: "1".into(),
                new: "0".into()
            }]
        );
        assert!(cvars.dirty);
        assert_eq!(
            composed(&cvars).get("gxVSync").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            cvars.take_outbox(),
            vec![("gxVSync".to_string(), "0".to_string())]
        );
        cvars.set("gxVSync", "1");
        assert_eq!(cvars.set("gxVSync", "0"), SetOutcome::Unchanged);
        assert_eq!(cvars.row("gxVSync").unwrap().pending, None);
        assert_eq!(cvars.commit_latched(), 0);
        // A latched row outside the gx set is not the video restart's to commit.
        assert_eq!(cvars.set("SoundBufferSize", "200"), SetOutcome::Staged);
        assert_eq!(cvars.commit_latched(), 0);
        assert_eq!(cvars.get("SoundBufferSize"), Some("100"));
        assert_eq!(
            cvars.row("SoundBufferSize").unwrap().pending.as_deref(),
            Some("200")
        );
        assert_eq!(
            composed(&cvars).get("SoundBufferSize").map(String::as_str),
            Some("200"),
            "uncommitted, and still the value the archive writes (`0x767077`)"
        );
        assert_eq!(cvars.set("Sound_MusicVolume", "0.7"), SetOutcome::Changed);
        assert_eq!(cvars.get("Sound_MusicVolume"), Some("0.7"));
        assert_eq!(cvars.take_events().len(), 1);
    }

    #[test]
    fn a_numeric_row_refuses_what_does_not_parse_and_corrects_the_mirror() {
        let mut cvars = Cvars::default();
        assert_eq!(cvars.set_from_vm("uiScale", "banana"), SetOutcome::Refused);
        assert_eq!(cvars.get("uiScale"), Some("1.0"));
        assert!(!cvars.has_events());
        assert!(!cvars.dirty);
        assert_eq!(
            cvars.take_outbox(),
            vec![("uiScale".to_string(), "1.0".to_string())],
            "the VM stored 'banana' synchronously; the host writes the truth back"
        );
        assert_eq!(cvars.set("uiScale", "banana"), SetOutcome::Refused);
        assert!(
            cvars.take_outbox().is_empty(),
            "a host write has no mirror to correct"
        );
        assert_eq!(
            cvars.set("realmList", "not an address"),
            SetOutcome::Changed
        );
        assert_eq!(cvars.set("nosuchrow", "1"), SetOutcome::Unknown);
    }

    #[test]
    fn an_addon_row_persists_like_the_clients_own() {
        let mut cvars = registry(&[("myAddonKnob", "3")]);
        assert_eq!(
            cvars.orphans(),
            vec![("myAddonKnob".to_string(), "3".to_string())],
            "unclaimed until the addon declares it — the VM's saved base"
        );
        cvars.learn_addon_row("myAddonKnob", "1");
        assert_eq!(
            cvars.get("myAddonKnob"),
            Some("3"),
            "starts at the saved value"
        );
        assert_eq!(cvars.default_of("myAddonKnob"), Some("1"));
        assert!(cvars.orphans().is_empty());
        cvars.learn_addon_row("myAddonKnob", "9");
        assert_eq!(
            cvars.default_of("myAddonKnob"),
            Some("1"),
            "a re-declaration is a no-op"
        );
        assert!(!cvars.dirty);
        assert_eq!(cvars.set_from_vm("myAddonKnob", "5"), SetOutcome::Changed);
        assert!(cvars.dirty, "an addon-only change is a change");
        assert!(
            !composed(&cvars).contains_key("myAddonKnob"),
            "an addon's row is never archived (`RegisterCVar` registers it with 0x80)"
        );
        let cvars = registry(&[("FutureKnob", "3")]);
        assert_eq!(
            composed(&cvars).get("FutureKnob").map(String::as_str),
            Some("3")
        );
    }

    #[test]
    fn a_session_owned_row_answers_the_env_and_never_reaches_the_file() {
        let mut cvars = Cvars::default();
        cvars.own_for_session("uiScale", Some("1.2"));
        cvars.load_file(Vec::from([("uiScale".to_string(), "0.8".to_string())]));
        assert_eq!(
            cvars.get("uiScale"),
            Some("1.2"),
            "the env's, not the file's"
        );
        assert!(
            !cvars.has_events(),
            "the knob already read the env — nothing to apply"
        );
        assert_eq!(cvars.set("uiScale", "1.4"), SetOutcome::Changed);
        assert_eq!(
            composed(&cvars).get("uiScale").map(String::as_str),
            Some("0.8"),
            "the file keeps what it said"
        );
        // A lever with no resource still marks the row, so the file cannot apply over the env.
        cvars.own_for_session("farclip", None);
        cvars.load_file(Vec::from([("farclip".to_string(), "500".to_string())]));
        assert_eq!(cvars.get("farclip"), Some("777"));
        assert!(cvars.is_session_owned("FARCLIP"));
    }

    /// A retired 1.12 row's saved line leaves the file; a live row's and an unknown key's stay.
    #[test]
    fn a_retired_row_leaves_the_file_and_nothing_else_does() {
        let mut cvars = Cvars::default();
        cvars.load_file(Vec::from([
            ("worlddetail".to_string(), "2".to_string()),
            ("farclip".to_string(), "500".to_string()),
            ("FutureKnob".to_string(), "3".to_string()),
        ]));
        for name in RETIRED_ROWS {
            assert!(cvars.default_of(name).is_none(), "{name} is still a row");
        }
        assert_eq!(cvars.retire_file_entry("WorldDetail").as_deref(), Some("2"));
        assert_eq!(
            cvars.retire_file_entry("farclip"),
            None,
            "a row is never retired"
        );
        let file = composed(&cvars);
        assert!(!file.keys().any(|k| k.eq_ignore_ascii_case("WorldDetail")));
        assert_eq!(file.get("farclip").map(String::as_str), Some("500"));
        assert_eq!(file.get("FutureKnob").map(String::as_str), Some("3"));
    }

    /// The camera reads `gxMultisample` once at spawn, so the file must be applied inside
    /// [`CvarLoad`].
    #[test]
    fn the_loaded_file_reaches_a_knob_before_anything_ordered_after_cvar_load() {
        use crate::local_state::test_env::{EnvGuard, ENV_LOCK};
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-cvar-load-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _u = EnvGuard::unset("WOW_UI_SCALE");
        let _f = EnvGuard::unset("WOW_FARCLIP");
        let _d = EnvGuard::unset("WOW_CLUTTER_DENSITY");
        let _m = EnvGuard::unset("WOW_MSAA");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());
        let _w = EnvGuard::set("WOW_DATA", "");
        crate::local_state::write_atomic(
            &tmp.join("WTF/Config.wtf"),
            "SET Sound_MusicVolume \"0.1\"\nSET gxMultisample \"4\"\n",
        )
        .unwrap();
        #[derive(Resource, Default)]
        struct SeenAtStartup(Option<(f32, u32)>);
        fn after_load(
            sound: Res<SoundConfig>,
            msaa: Res<MsaaSetting>,
            mut seen: ResMut<SeenAtStartup>,
        ) {
            seen.0 = Some((sound.music, msaa.samples));
        }
        let mut app = cvar_app();
        app.init_resource::<SeenAtStartup>()
            .add_systems(Startup, after_load.after(CvarLoad));
        app.update();
        assert_eq!(
            app.world().resource::<SeenAtStartup>().0,
            Some((0.1, 4)),
            "both the plain row and the latched one are applied by the time CvarLoad is over"
        );
        assert_eq!(
            app.world().resource::<Cvars>().get("gxMultisample"),
            Some("4"),
            "a file value is applied, not staged: the reference's LoadFile runs before Register"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// Every registered row has a reader: a string literal naming it in this crate's or
    /// `benilla-ui`'s code, or a `LUA_ONLY` entry naming its stock reader. Test files do not count.
    #[test]
    fn every_registered_row_has_a_reader_in_the_source() {
        /// Rows whose only reader is the stock interface.
        const LUA_ONLY: &[(&str, &str)] = &[
            (
                "statusBarText",
                "TextStatusBar.lua reads it on each CVAR_UPDATE",
            ),
            (
                "UberTooltips",
                "GameTooltip's binding-line gate, stock and pfUI",
            ),
            ("gxApi", "pfUI's system tooltip names the backend"),
            ("lod", "OptionsFrame.lua's World LOD box"),
            (
                "movieSubtitle",
                "OptionsFrame.lua's Cinematic Subtitles box",
            ),
            (
                "useWeatherShaders",
                "OptionsFrame.lua's Weather Shaders box",
            ),
            (
                "ShowAllSpellRanks",
                "SpellBookFrame.lua's SpellBook_GetTabInfo and Show All Spell Ranks box",
            ),
        ];
        let app_src = crate::test_support::src_dir();
        let crates = app_src
            .parent()
            .and_then(std::path::Path::parent)
            .expect("crates/");
        let ui_src = crates.join("benilla-ui").join("src");
        let mut code = String::new();
        for file in crate::test_support::rust_files(&app_src)
            .into_iter()
            .chain(crate::test_support::rust_files(&ui_src))
        {
            // Relative to `crates/`, so a checkout's own directory name never matches.
            let rel = file
                .strip_prefix(crates)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            if rel.contains("benilla-app/src/cvars/") || rel.contains("tests") {
                continue;
            }
            let text = std::fs::read_to_string(&file).expect("source is readable");
            for line in text.lines() {
                if !line.trim_start().starts_with("//") {
                    code.push_str(line);
                    code.push('\n');
                }
            }
        }
        let mut orphans = Vec::new();
        for row in REGISTERED {
            if LUA_ONLY.iter().any(|(n, _)| *n == row.name) {
                continue;
            }
            let exact = format!("\"{}\"", row.name);
            let lower = format!("\"{}\"", row.name.to_ascii_lowercase());
            if !(code.contains(&exact) || code.contains(&lower)) {
                orphans.push(row.name);
            }
        }
        assert!(
            orphans.is_empty(),
            "registered rows nothing in the source reads (an observer arm, a `cvars.get`, or a \
             `LUA_ONLY` entry with its stock reader): {orphans:?}"
        );
        for (name, _) in LUA_ONLY {
            assert!(
                REGISTERED.iter().any(|r| r.name == *name),
                "{name}: named Lua-only but not registered"
            );
        }
    }

    /// The rows the reference latches (`flags` 2 or 3 at the register site): the sound-init row and
    /// the `gx*` block `RestartGx` commits. The Effects rows register with `flags = 1` and apply
    /// live.
    #[test]
    fn the_latched_rows_are_the_references_own() {
        let mut latched: Vec<&str> = REGISTERED
            .iter()
            .filter(|r| r.latched)
            .map(|r| r.name)
            .collect();
        latched.sort_unstable();
        assert_eq!(
            latched,
            vec![
                "SoundBufferSize",
                "gxApi",
                "gxAspect",
                "gxColorBits",
                "gxCursor",
                "gxDepthBits",
                "gxFixLag",
                "gxMaximize",
                "gxMultisample",
                "gxRefresh",
                "gxResolution",
                "gxTripleBuffer",
                "gxVSync",
                "gxWindow",
            ]
        );
    }
}
