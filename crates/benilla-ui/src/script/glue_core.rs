//! The glue core's login-screen verbs: the login, the status dialog's button, the saved account,
//! the four user agreements, the build, the locale, the server, the screen name, the glue music and
//! the quit. Each reads a slot the host seeds or queues a [`GlueRequest`] the host's glue manager
//! drains; the screen state machine they drive (`CGlueMgr::Update`) is the host's.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{flag, string_arg};

/// One of the five user agreements the login screen walks before it shows the login boxes
/// (`AccountLogin_ShowUserAgreements`), each a CVar the glue manager reads once at its init
/// (`CGlueMgr::Initialize`): `readTOS`, `readEULA`, `readTerminationWithoutNotice`, `readScanning`,
/// `readContest`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlueNotice {
    Tos,
    Eula,
    TerminationWithoutNotice,
    Scanning,
    Contest,
}

impl GlueNotice {
    /// All five, in the glue manager's seeding order.
    pub const ALL: [GlueNotice; 5] = [
        GlueNotice::Tos,
        GlueNotice::Eula,
        GlueNotice::TerminationWithoutNotice,
        GlueNotice::Scanning,
        GlueNotice::Contest,
    ];

    fn index(self) -> usize {
        self as usize
    }
}

/// One agreement's two flags, as the glue manager seeds them from its CVar's integer value (for
/// `readTOS`): accepted is the value being 1, the notice (the agreement changed since it was read)
/// is the value being negative.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoticeState {
    pub accepted: bool,
    pub show_notice: bool,
}

impl NoticeState {
    /// The flags for a CVar value, read as `atoi` reads it: anything unparseable is 0.
    pub fn from_cvar(value: &str) -> Self {
        let n = leading_int(value);
        NoticeState {
            accepted: n == 1,
            show_notice: n < 0,
        }
    }
}

/// `atoi`: an optional sign and the leading digits, 0 when there are none.
pub(super) fn leading_int(s: &str) -> i64 {
    let s = s.trim_start();
    let (sign, digits) = match s.as_bytes().first() {
        Some(b'-') => (-1, &s[1..]),
        Some(b'+') => (1, &s[1..]),
        _ => (1, s),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    sign * digits[..end].parse::<i64>().unwrap_or(0)
}

/// `DefaultServerLogin`'s two strings. Never logged: the `Debug` form names the account only.
pub struct GlueCredentials {
    pub account: String,
    pub password: String,
}

impl std::fmt::Debug for GlueCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlueCredentials")
            .field("account", &self.account)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// A glue verb's effect on the host, drained by its glue manager.
#[derive(Debug)]
pub enum GlueRequest {
    /// `DefaultServerLogin(account, password)` (`0x4dc260`), handed to `CGlueMgr::Login`, whose
    /// guards and dialogs are the host's.
    Login(GlueCredentials),
    /// `StatusDialogClick()` (`0x4dc500`): the status dialog's button, per screen state.
    StatusDialogClick,
    /// `SetSavedAccountName(name)` (`0x4dbec0`): the `accountName` CVar's new value.
    SetSavedAccountName(String),
    /// `AcceptTOS` and its three siblings: the agreement's CVar set to `"1"`.
    Accept(GlueNotice),
    /// `PlayGlueMusic(file)` (`0x4dc070`).
    PlayGlueMusic(String),
    /// `StopGlueMusic()` (`0x4dc1a0`), a one-second fade.
    StopGlueMusic,
    /// `DisconnectFromServer()` (`0x4dda10`): the world connection dropped if up, then the logon
    /// cancelled.
    DisconnectFromServer,
    /// `QuitGame()` (`0x4dc050`).
    QuitGame,
    /// `RequestRealmList([dialog])`: screen state 4, the status dialog when `dialog`, and a realm
    /// list query (`0x5ab840`).
    RequestRealmList { dialog: bool },
    /// `CancelRealmListQuery()`: in screen state 4 only, the query completes cancelled and the
    /// state goes to 0; nothing is sent.
    CancelRealmListQuery,
    /// `RealmListDialogCancelled()` on the login screen (`CLoginMgr::Cancel`): the realmd link
    /// closes, with no dialog and no screen change.
    CancelLogon,
    /// `ChangeRealm` reaching `ConnectToRealm` with this realm, `realmName` already set to it.
    ChangeRealm(String),
    /// `GetCharacterListUpdate()` (`0x4e4530`): the roster is already cleared in the VM; the host
    /// takes screen state 3, the `CHAR_LIST_RETRIEVING` dialog and the `CMSG_CHAR_ENUM`.
    GetCharacterList,
    /// `LaunchURL(url)` (`0x4dc1c0`) for a URL that passed `launch_url_allowed`: the player's
    /// browser opens it.
    LaunchUrl(String),
    /// `PlayGlueAmbience(name [, fade])` (`0x4dc0c0`): the named ambience, faded in over `fade`
    /// seconds, or the sound's own when none.
    PlayGlueAmbience { name: String, fade: Option<f32> },
    /// `StopGlueAmbience()` (`0x4dc130`): a one-second fade out.
    StopGlueAmbience,
    /// `StopAllSFX(fade)` (`0x4dd380`).
    StopAllSfx(f32),
    /// `PlayCreditsMusic(kit)` (`0x4dc150`).
    PlayCreditsMusic(String),
    /// `EnterWorld()` (`0x4dc510` → `CGlueMgr::EnterWorld` `0x4d9bd0`), whose guards are the
    /// host's.
    EnterWorld,
    /// `CancelLogin()` (`0x4dce60`).
    CancelLogin,
    /// `Screenshot()` (`0x4dc520`), answered with `GLUE_SCREENSHOT_SUCCEEDED` or `_FAILED`.
    Screenshot,
    /// `ReadyForAccountDataTimes()` (`0x4de390`): a bare `CMSG_READY_FOR_ACCOUNT_DATA_TIMES`.
    ReadyForAccountDataTimes,
    /// `CMSG_REALM_SPLIT` with this state: `0xFFFFFFFF` asks (`RequestRealmSplitInfo`,
    /// `0x4de2e0`), 0 to 2 answers (`SetRealmSplitState`, `0x4de250`).
    RealmSplit(u32),
    /// `CreateCharacter(name)` past the client's name check (`0x4e0380`): the host's
    /// `CGlueMgr::CreateCharacter` (`0x4d8c60`), or a paid change of an existing character.
    CreateCharacter(super::glue_create::GlueCharCreate),
    /// `DeleteCharacter(index)` (`0x4e33a0`) for a row on the roster: the host's
    /// `CGlueMgr::DeleteCharacter` with its guid.
    DeleteCharacter(u64),
    /// `SaveAddOns()` (`0x4dcac0`): the enable bits changed since the last load or save, for the
    /// host to write to each character's enable file.
    SaveAddOns(Vec<super::glue_addons::GlueAddonChange>),
}

/// The hosts `LaunchURL` opens, the 10-entry table: a leading `*` stands for one label, so
/// `*.blizzard.com` takes `forums.blizzard.com` and not `a.b.blizzard.com`.
const LAUNCH_HOSTS: [&str; 10] = [
    "*.blizzard.com",
    "*.blizzcon.com",
    "www.battle.net",
    "*.worldofwarcraft.com",
    "www.battlenet.co.kr",
    "*.worldofwarcraft.co.kr",
    "*.blizzard-europe.com",
    "*.wow-europe.com",
    "*.wowchina.com",
    "*.wowtaiwan.com.tw",
];

/// The gate in front of the browser: the reference tests takes the URL. The URL begins `http://`
/// (compared case-folded), every byte after it is a letter, a digit, `.`, `-` or `/` (so no query,
/// port, escape or `_`), and its host, the text up to the first `/`, is one of [`LAUNCH_HOSTS`],
/// compared case-folded. The reference refuses a URL of 0x400 bytes or more. The reference works on
/// a C string, so the verb cuts its argument at a NUL before asking; a URL with a NUL in it is
/// refused here.
///
/// Deviation: a host of 0x80 bytes or more is refused. The reference copies the host into a
/// 0x80-byte buffer and writes its terminator one byte past the end, and the cut host could end in
/// a whitelisted suffix the real one does not; this takes the safe side.
pub fn launch_url_allowed(url: &str) -> bool {
    if url.len() >= 0x400 || url.contains('\0') {
        return false;
    }
    let url = url.as_bytes();
    const SCHEME: &[u8] = b"http://";
    let Some(rest) = url
        .get(..SCHEME.len())
        .filter(|head| head.eq_ignore_ascii_case(SCHEME))
        .map(|_| &url[SCHEME.len()..])
    else {
        return false;
    };
    if !rest
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'/'))
    {
        return false;
    }
    let host = rest.split(|&b| b == b'/').next().unwrap_or_default();
    if host.len() >= 0x80 {
        return false;
    }
    LAUNCH_HOSTS
        .iter()
        .any(|entry| match entry.strip_prefix('*') {
            // `SStrChr(host, '.')`, then the rest against the entry past its `*`.
            Some(suffix) => host
                .iter()
                .position(|&b| b == b'.')
                .is_some_and(|dot| host[dot..].eq_ignore_ascii_case(suffix.as_bytes())),
            None => host.eq_ignore_ascii_case(entry.as_bytes()),
        })
}

/// The realm `GetServerName` (`0x4dd900`) answers: the `realmName` CVar's text, and the
/// selected realm's PvP and RP flags off its `Cfg_Configs` row, nil without a selected realm.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlueServer {
    pub name: String,
    pub pvp: bool,
    pub rp: bool,
}

/// The glue core's slots and queue in the VM's model.
#[derive(Default)]
pub(crate) struct GlueCore {
    requests: Vec<GlueRequest>,
    /// The `accountName` CVar, which `GetSavedAccountName` (`0x4dd5d0`) reads.
    saved_account: String,
    notices: [NoticeState; 5],
    /// ClientServices' world-connection flag (`[svc+0x1b00]`), which `IsConnectedToServer`
    /// (`0x4dda50`) reads.
    connected: bool,
    server: GlueServer,
    /// The current glue screen's name, which `SetCurrentScreen` (`0x4dc000`) copies into a
    /// 0x40-byte buffer.
    pub(super) screen: String,
    /// The realm list the realm verbs read ([`super::glue_realm`]).
    pub(super) realms: super::glue_realm::GlueRealmList,
    /// The character roster and the select verbs' cells ([`super::glue_select`]).
    pub(super) roster: super::glue_select::GlueRoster,
    /// The system verbs' slots ([`super::glue_system`]).
    pub(super) system: super::glue_system::GlueSystem,
    /// The character create verbs' slots ([`super::glue_create`]).
    pub(super) create: super::glue_create::GlueCreate,
    /// The AddOns screen's per-character enable state.
    pub(super) addons: super::glue_addons::GlueAddonEnable,
}

/// `SetCurrentScreen`'s buffer, 0x40 bytes with its terminator (`SStrCopy`).
const SCREEN_NAME_MAX: usize = 0x3f;

impl super::UiScript {
    /// Drain the glue verbs' requests since the last call.
    pub fn take_glue_requests(&mut self) -> Vec<GlueRequest> {
        std::mem::take(&mut self.model_mut().glue.requests)
    }

    /// Seed the saved account name `GetSavedAccountName` answers.
    pub fn set_glue_saved_account(&mut self, name: &str) {
        self.model_mut().glue.saved_account = name.to_string();
    }

    /// Seed one agreement's flags.
    pub fn set_glue_notice(&mut self, notice: GlueNotice, state: NoticeState) {
        self.model_mut().glue.notices[notice.index()] = state;
    }

    /// One agreement's flags as the VM holds them.
    pub fn glue_notice(&self, notice: GlueNotice) -> NoticeState {
        self.model_ref().glue.notices[notice.index()]
    }

    /// Seed whether the world connection is up.
    pub fn set_glue_connected(&mut self, connected: bool) {
        self.model_mut().glue.connected = connected;
    }

    /// Seed the realm `GetServerName` answers.
    pub fn set_glue_server(&mut self, server: GlueServer) {
        self.model_mut().glue.server = server;
    }

    /// The current glue screen's name, as `SetCurrentScreen` last set it.
    pub fn glue_screen(&self) -> String {
        self.model_ref().glue.screen.clone()
    }

    /// A string global's text, `GetGlobalString`'s answer (with no gender and no ordinal takes):
    /// the global's string (a number stringified), else the empty string.
    pub fn global_text(&self, key: &str) -> String {
        global_text(&self.lua, key)
    }
}

pub(super) fn queue(lua: &Lua, request: GlueRequest) {
    crate::script::model_mut_of(lua).glue.requests.push(request);
}

/// The text of a string global, as [`super::UiScript::global_text`] reads it.
pub(super) fn global_text_of(lua: &Lua, key: &str) -> String {
    global_text(lua, key)
}

fn global_text(lua: &Lua, key: &str) -> String {
    match lua.globals().get::<Value>(key) {
        Ok(v @ (Value::String(_) | Value::Integer(_) | Value::Number(_))) => lua
            .coerce_string(v)
            .ok()
            .flatten()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // Both arguments `is-number-or-string`, else the usage error; then
    // `CGlueMgr::Login(account, password)`.
    g.set(
        "DefaultServerLogin",
        lua.create_function(|lua, (account, password): (Value, Value)| {
            const USAGE: &str = "Usage: DefaultServerLogin(\"accountName\", \"password\")";
            let ok =
                |v: &Value| matches!(v, Value::String(_) | Value::Integer(_) | Value::Number(_));
            if !ok(&account) || !ok(&password) {
                return Err(mlua::Error::RuntimeError(USAGE.into()));
            }
            let account = string_arg(lua, account, USAGE)?;
            let password = string_arg(lua, password, USAGE)?;
            // `CGlueMgr::LoginServerLogin` does nothing until the Scan.dll check has opened its
            // gate.
            let scan = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data")
                .glue
                .system
                .scan;
            if scan != super::glue_system::ScanDll::Ok {
                return Ok(());
            }
            queue(
                lua,
                GlueRequest::Login(GlueCredentials { account, password }),
            );
            Ok(())
        })?,
    )?;

    g.set(
        "StatusDialogClick",
        lua.create_function(|lua, _: MultiValue| {
            queue(lua, GlueRequest::StatusDialogClick);
            Ok(())
        })?,
    )?;

    // The `accountName` CVar's string (`[cvar+0x20]`).
    g.set(
        "GetSavedAccountName",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            lua.create_string(&model.glue.saved_account)
        })?,
    )?;
    // A string argument or `Usage: SetSavedAccountName("accountName")`, then `CVar::Set`, so the
    // next `GetSavedAccountName` reads it back at once.
    g.set(
        "SetSavedAccountName",
        lua.create_function(|lua, name: Value| {
            let name = string_arg(lua, name, "Usage: SetSavedAccountName(\"accountName\")")?;
            crate::script::model_mut_of(lua).glue.saved_account = name.clone();
            queue(lua, GlueRequest::SetSavedAccountName(name));
            Ok(())
        })?,
    )?;

    // Each agreement: `Show*Notice` and `*Accepted` push 1 or nil off the flag pair the glue
    // manager seeded; `Accept*` sets accepted, clears the notice and sets the CVar to "1".
    for (notice, show, accepted, accept) in [
        (GlueNotice::Tos, "ShowTOSNotice", "TOSAccepted", "AcceptTOS"),
        (
            GlueNotice::Eula,
            "ShowEULANotice",
            "EULAAccepted",
            "AcceptEULA",
        ),
        (
            GlueNotice::TerminationWithoutNotice,
            "ShowTerminationWithoutNoticeNotice",
            "TerminationWithoutNoticeAccepted",
            "AcceptTerminationWithoutNotice",
        ),
        (
            GlueNotice::Scanning,
            "ShowScanningNotice",
            "ScanningAccepted",
            "AcceptScanning",
        ),
        (
            GlueNotice::Contest,
            "ShowContestNotice",
            "ContestAccepted",
            "AcceptContest",
        ),
    ] {
        let i = notice.index();
        g.set(
            show,
            lua.create_function(move |lua, _: MultiValue| {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                Ok(flag(model.glue.notices[i].show_notice))
            })?,
        )?;
        g.set(
            accepted,
            lua.create_function(move |lua, _: MultiValue| {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                Ok(flag(model.glue.notices[i].accepted))
            })?,
        )?;
        g.set(
            accept,
            lua.create_function(move |lua, _: MultiValue| {
                crate::script::model_mut_of(lua).glue.notices[i] = NoticeState {
                    accepted: true,
                    show_notice: false,
                };
                queue(lua, GlueRequest::Accept(notice));
                Ok(())
            })?,
        )?;
    }

    // Five strings (`0x4dbe60`): the `VERSION` and `RELEASE_BUILD` globals' text, then the
    // version, the build and the date as the binary spells them. The in-game binding
    // (`0x50f890`) answers the last three and the interface version.
    g.set(
        "GetBuildInfo",
        lua.create_function(|lua, _: MultiValue| {
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(global_text(lua, "VERSION"))?),
                Value::String(lua.create_string(global_text(lua, "RELEASE_BUILD"))?),
                Value::String(lua.create_string(super::client::VERSION)?),
                Value::String(lua.create_string(super::client::BUILD)?),
                Value::String(lua.create_string(super::client::BUILD_DATE)?),
            ]))
        })?,
    )?;

    // The locale name table at the locale index, the same answer as the in-game binding: benilla is
    // enUS-only.
    g.set(
        "GetLocale",
        lua.create_function(|lua, _: MultiValue| lua.create_string("enUS"))?,
    )?;

    // A string argument or `Usage: SetCurrentScreen("screen")`, copied into the 0x40-byte screen
    // buffer.
    g.set(
        "SetCurrentScreen",
        lua.create_function(|lua, name: Value| {
            let mut name = string_arg(lua, name, "Usage: SetCurrentScreen(\"screen\")")?;
            if name.len() > SCREEN_NAME_MAX {
                let mut end = SCREEN_NAME_MAX;
                while !name.is_char_boundary(end) {
                    end -= 1;
                }
                name.truncate(end);
            }
            crate::script::model_mut_of(lua).glue.screen = name;
            Ok(())
        })?,
    )?;

    // Three values (`0x4dd900`): the `realmName` CVar's string, always a string, then PvP and RP
    // as 1 or nil.
    g.set(
        "GetServerName",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let server = &model.glue.server;
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&server.name)?),
                flag(server.pvp),
                flag(server.rp),
            ]))
        })?,
    )?;

    g.set(
        "IsConnectedToServer",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(model.glue.connected))
        })?,
    )?;
    g.set(
        "DisconnectFromServer",
        lua.create_function(|lua, _: MultiValue| {
            queue(lua, GlueRequest::DisconnectFromServer);
            Ok(())
        })?,
    )?;
    g.set(
        "QuitGame",
        lua.create_function(|lua, _: MultiValue| {
            queue(lua, GlueRequest::QuitGame);
            Ok(())
        })?,
    )?;

    // A string argument or `Usage: PlayGlueMusic("filename")`.
    g.set(
        "PlayGlueMusic",
        lua.create_function(|lua, file: Value| {
            let file = string_arg(lua, file, "Usage: PlayGlueMusic(\"filename\")")?;
            queue(lua, GlueRequest::PlayGlueMusic(file));
            Ok(())
        })?,
    )?;
    // A string argument or `Usage: LaunchURL("URL")`, then, which opens the browser for a URL
    // passes and does nothing for any other. The in-game state has no such verb.
    g.set(
        "LaunchURL",
        lua.create_function(|lua, url: Value| {
            let url = string_arg(lua, url, "Usage: LaunchURL(\"URL\")")?;
            // The C string ends at a NUL, for the gate and for the shell alike.
            let url = url.split('\0').next().unwrap_or_default();
            if launch_url_allowed(url) {
                queue(lua, GlueRequest::LaunchUrl(url.to_string()));
            }
            Ok(())
        })?,
    )?;
    g.set(
        "StopGlueMusic",
        lua.create_function(|lua, _: MultiValue| {
            queue(lua, GlueRequest::StopGlueMusic);
            Ok(())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    #[test]
    fn a_notice_cvar_reads_as_atoi() {
        let s = |v| NoticeState::from_cvar(v);
        assert_eq!(s("0"), NoticeState::default());
        assert!(s("1").accepted && !s("1").show_notice);
        assert!(s("-1").show_notice && !s("-1").accepted);
        assert!(!s("2").accepted && !s("2").show_notice);
        assert!(s("1x").accepted, "atoi stops at the first non-digit");
        assert_eq!(s("x"), NoticeState::default());
    }

    #[test]
    fn default_server_login_queues_the_strings_or_raises_its_usage() {
        let mut s = UiScript::new_glue().unwrap();
        // The login waits for the Scan.dll check's gate, which a failed load and the stock
        // `ScanDLLContinueAnyway` open.
        s.run("ScanDLLStart('v', 'd')").unwrap();
        s.run_glue_scan_tick();
        s.run("ScanDLLContinueAnyway()").unwrap();
        s.run("DefaultServerLogin('probe', 1234)").unwrap();
        let reqs = s.take_glue_requests();
        match reqs.as_slice() {
            [GlueRequest::Login(c)] => {
                assert_eq!((c.account.as_str(), c.password.as_str()), ("probe", "1234"));
                assert!(
                    !format!("{c:?}").contains("1234"),
                    "the password is never printed"
                );
            }
            other => panic!("{other:?}"),
        }
        let err = s
            .run("DefaultServerLogin('probe')")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Usage: DefaultServerLogin(\"accountName\", \"password\")"),
            "{err}"
        );
        assert!(s.take_glue_requests().is_empty());
    }

    #[test]
    fn the_agreements_answer_their_seeded_flags_and_accept_sets_them() {
        let mut s = UiScript::new_glue().unwrap();
        s.set_glue_notice(
            GlueNotice::Eula,
            NoticeState {
                accepted: false,
                show_notice: true,
            },
        );
        s.set_glue_notice(GlueNotice::Tos, NoticeState::from_cvar("1"));
        let read = |s: &UiScript| {
            s.eval::<(Option<i64>, Option<i64>, Option<i64>, Option<i64>)>(
                "return EULAAccepted(), ShowEULANotice(), TOSAccepted(), ScanningAccepted()",
            )
            .unwrap()
        };
        assert_eq!(read(&s), (None, Some(1), Some(1), None));
        s.run("AcceptEULA()").unwrap();
        assert_eq!(read(&s), (Some(1), None, Some(1), None));
        assert!(matches!(
            s.take_glue_requests().as_slice(),
            [GlueRequest::Accept(GlueNotice::Eula)]
        ));
    }

    #[test]
    fn the_saved_account_reads_back_what_was_set() {
        let mut s = UiScript::new_glue().unwrap();
        s.set_glue_saved_account("PROBE");
        assert_eq!(
            s.eval::<String>("return GetSavedAccountName()").unwrap(),
            "PROBE"
        );
        s.run("SetSavedAccountName('')").unwrap();
        assert_eq!(
            s.eval::<String>("return GetSavedAccountName()").unwrap(),
            ""
        );
        assert!(matches!(
            s.take_glue_requests().as_slice(),
            [GlueRequest::SetSavedAccountName(n)] if n.is_empty()
        ));
        let err = s.run("SetSavedAccountName()").unwrap_err().to_string();
        assert!(
            err.contains("Usage: SetSavedAccountName(\"accountName\")"),
            "{err}"
        );
    }

    /// The scheme, the character set past it and the host table.
    #[test]
    fn launch_url_opens_only_the_whitelisted_hosts() {
        let ok = [
            "http://www.worldofwarcraft.com/",
            "HTTP://WWW.WorldOfWarcraft.com/index.html",
            "http://forums.blizzard.com",
            "http://us.blizzard.com/en-us/alert",
            "http://www.battle.net/x",
            "http://www.wow-europe.com/en/info/theburningcrusade.html",
            "http://status.wow-europe.com/en/alert",
        ];
        for url in ok {
            assert!(launch_url_allowed(url), "{url}");
        }
        let refused = [
            "https://www.blizzard.com/",
            "ftp://www.blizzard.com/",
            "www.blizzard.com",
            "http://blizzard.com/",
            "http://a.b.blizzard.com/",
            "http://example.com/",
            "http://example.com/www.blizzard.com",
            "http://www.blizzard.com.evil.net/",
            "http://battle.net/",
            "http://www.blizzard.com/?q=1",
            "http://www.blizzard.com:80/",
            "http://www.blizzard.com/a_b",
            "http://www.blizzard.com/a b",
            "http://www.blizzard.com\n/",
            "",
        ];
        for url in refused {
            assert!(!launch_url_allowed(url), "{url:?}");
        }
        // The gate takes a C string: a NUL inside one is refused outright.
        assert!(!launch_url_allowed("http://www.blizzard.com/\0?q=1"));
        // 0x400 bytes or more is refused; one byte under is not.
        let path = |n: usize| format!("http://www.blizzard.com/{}", "a".repeat(n - 24));
        assert!(launch_url_allowed(&path(0x3ff)));
        assert!(!launch_url_allowed(&path(0x400)));
        // A host of 0x80 bytes or more is refused rather than cut to a whitelisted tail.
        let host = |n: usize| format!("http://{}.blizzard.com/", "a".repeat(n - 13));
        assert!(launch_url_allowed(&host(0x7f)));
        assert!(!launch_url_allowed(&host(0x80)));
        let cut = format!("http://{}.blizzard.com.evil.net/", "a".repeat(0x80 - 13));
        assert!(!launch_url_allowed(&cut));
    }

    #[test]
    fn launch_url_queues_a_passing_url_and_raises_its_usage() {
        let mut s = UiScript::new_glue().unwrap();
        s.run("LaunchURL('http://www.worldofwarcraft.com/')")
            .unwrap();
        s.run("LaunchURL('http://example.com/')").unwrap();
        // The text before a NUL is the argument, for the gate and for the opener.
        s.run("LaunchURL('http://www.blizzard.com/a\\0?q=1')")
            .unwrap();
        s.run("LaunchURL('http://example.com/\\0http://www.blizzard.com/')")
            .unwrap();
        let queued = s.take_glue_requests();
        assert!(
            matches!(
                queued.as_slice(),
                [GlueRequest::LaunchUrl(a), GlueRequest::LaunchUrl(b)]
                    if a == "http://www.worldofwarcraft.com/" && b == "http://www.blizzard.com/a"
            ),
            "{queued:?}"
        );
        let err = s.run("LaunchURL()").unwrap_err().to_string();
        assert!(err.contains("Usage: LaunchURL(\"URL\")"), "{err}");
        assert!(s.take_glue_requests().is_empty());
    }

    #[test]
    fn the_glue_build_info_is_five_strings() {
        let s = UiScript::new_glue().unwrap();
        s.run("VERSION = 'Version' RELEASE_BUILD = 'Release'")
            .unwrap();
        assert_eq!(
            s.eval::<Vec<String>>("return { GetBuildInfo() }").unwrap(),
            ["Version", "Release", "3.3.5", "12340", "Jun 24 2010"]
        );
    }

    #[test]
    fn the_server_name_is_always_a_string_with_two_flags() {
        let mut s = UiScript::new_glue().unwrap();
        assert_eq!(
            s.eval::<(String, Option<i64>, Option<i64>)>("return GetServerName()")
                .unwrap(),
            (String::new(), None, None)
        );
        s.set_glue_server(GlueServer {
            name: "Nostalrius".into(),
            pvp: true,
            rp: false,
        });
        assert_eq!(
            s.eval::<(String, Option<i64>, Option<i64>)>("return GetServerName()")
                .unwrap(),
            ("Nostalrius".to_string(), Some(1), None)
        );
    }

    #[test]
    fn set_current_screen_keeps_the_name_in_its_buffer() {
        let s = UiScript::new_glue().unwrap();
        s.run("SetCurrentScreen('login')").unwrap();
        assert_eq!(s.glue_screen(), "login");
        s.run(&format!("SetCurrentScreen('{}')", "x".repeat(100)))
            .unwrap();
        assert_eq!(s.glue_screen().len(), 0x3f);
        assert!(s.run("SetCurrentScreen()").is_err());
    }
}
