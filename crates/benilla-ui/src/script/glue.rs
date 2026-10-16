//! The glue screens' FrameScript state, which [`UiScript::new_glue`] builds: what the glue
//! manager gives Lua where the in-game UI's init gives it the world's surface.
//!
//! The glue manager's `Resume` (`0x4da5f0`) flushes the Lua state (`FrameScript_Flush`
//! `0x81ac70`), registers the system, frame-script, glue, character select, character create,
//! realm list, sound, account message and video options tables, and hands the FrameScript event
//! registry the 41-event table below in place of the in-game one. The frame types are a
//! process-wide registry instead ([`super::object::registered_frame_kind`]).
//!
//! [`UiScript::new_glue`]: super::UiScript::new_glue

use mlua::Lua;

/// Which of the reference's two FrameScript states a VM is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VmKind {
    /// `UI_Init`'s: the in-game surface over FrameXML.
    InGame,
    /// The glue builder's: the base set, the three shared registrars and the glue verbs, over
    /// GlueXML.
    Glue,
}

/// The glue event table by id (`0xb6aff8`, written by `0x4dbc70`), `reference/3.3.5a-glue-events.tsv`.
/// While the glue state runs these are the only events: `RegisterEvent` matches a name against
/// them case-insensitively and drops one they lack without an error.
pub const GLUE_EVENTS: [&str; 41] = [
    "SET_GLUE_SCREEN",
    "START_GLUE_MUSIC",
    "DISCONNECTED_FROM_SERVER",
    "OPEN_STATUS_DIALOG",
    "UPDATE_STATUS_DIALOG",
    "CLOSE_STATUS_DIALOG",
    "ADDON_LIST_UPDATE",
    "CHARACTER_LIST_UPDATE",
    "UPDATE_SELECTED_CHARACTER",
    "OPEN_REALM_LIST",
    "GET_PREFERRED_REALM_INFO",
    "UPDATE_SELECTED_RACE",
    "SELECT_LAST_CHARACTER",
    "SELECT_FIRST_CHARACTER",
    "GLUE_SCREENSHOT_SUCCEEDED",
    "GLUE_SCREENSHOT_FAILED",
    "PATCH_UPDATE_PROGRESS",
    "PATCH_DOWNLOADED",
    "SUGGEST_REALM",
    "SUGGEST_REALM_WRONG_PVP",
    "SUGGEST_REALM_WRONG_CATEGORY",
    "SHOW_SERVER_ALERT",
    "FRAMES_LOADED",
    "FORCE_RENAME_CHARACTER",
    "FORCE_DECLINE_CHARACTER",
    "SHOW_SURVEY_NOTIFICATION",
    "PLAYER_ENTER_PIN",
    "CLIENT_ACCOUNT_MISMATCH",
    "PLAYER_ENTER_MATRIX",
    "SCANDLL_ERROR",
    "SCANDLL_DOWNLOADING",
    "SCANDLL_FINISHED",
    "SERVER_SPLIT_NOTICE",
    "TIMER_ALERT",
    "ACCOUNT_MESSAGES_AVAILABLE",
    "ACCOUNT_MESSAGES_HEADERS_LOADED",
    "ACCOUNT_MESSAGES_BODY_LOADED",
    "CLIENT_TRIAL",
    "PLAYER_ENTER_TOKEN",
    "GAME_ACCOUNTS_UPDATED",
    "CLIENT_CONVERTED",
];

impl VmKind {
    /// The event table's own spelling of `name` for `RegisterEvent`/`UnregisterEvent` (`0x49efe0`,
    /// `0x49f060`), or `None` for a name the table lacks, which registers nothing.
    ///
    /// The in-game VM answers every name as given: benilla's in-game events register by name and
    /// carry no table, so a name off the reference's table registers and never fires, and only the
    /// exact spelling matches.
    pub(crate) fn event_key(self, name: &str) -> Option<&str> {
        match self {
            VmKind::InGame => Some(name),
            VmKind::Glue => GLUE_EVENTS
                .iter()
                .copied()
                .find(|e| e.eq_ignore_ascii_case(name)),
        }
    }

    /// Whether the host may fire `event` into this VM. The reference fires by id, so the glue state can only ever see one of its 41; a host's in-game event sent
    /// to the glue VM reaches no frame, `RegisterAllEvents` ones included.
    pub(crate) fn fires(self, event: &str) -> bool {
        match self {
            VmKind::InGame => true,
            VmKind::Glue => GLUE_EVENTS.contains(&event),
        }
    }
}

/// The glue core's screen verbs (registrar `0x46ddf0`, table `0x8373b8`): the screen root's size
/// and the cursor in it. The glue and in-game states share the root, `0x41ad10`'s, 768 units tall
/// and `768·a` wide for the aspect `a` (the glue builder sets it at `0x46a828`, from `0x46ab40`:
/// `gxResolution`'s width over height with `widescreen` on, its default, else 4:3). The glue has
/// no `UIParent` and so no UI scale: `GetScreenWidth` (`0x46dd30`) and `GetScreenHeight`
/// (`0x46dd80`) are the in-game pair (`0x48b480`, `0x48b4d0`) without its `0x494590` division,
/// and `GetCursorPosition` (`0x46dad0`) is byte for byte the in-game `0x48b820`.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    g.set(
        "GetScreenWidth",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            Ok(model.screen.width())
        })?,
    )?;
    g.set(
        "GetScreenHeight",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            Ok(model.screen.height())
        })?,
    )?;
    g.set(
        "GetCursorPosition",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model");
            Ok(model.cursor_pos)
        })?,
    )?;
    super::glue_core::install(lua)?;
    super::glue_system::install(lua)?;
    super::addon::install_glue(lua)?;
    super::cvars::install_cvar_verbs(lua)?;
    super::glue_realm::install(lua)?;
    super::glue_select::install(lua)?;
    super::glue_create::install(lua)
}
