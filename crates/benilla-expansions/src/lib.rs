//! The client generations benilla targets and the structural facts of each reference client. One
//! [`Expansion`] is selected per process ([`Expansion::active`]); the crates that own a behaviour
//! read the facts they need from its [`ExpansionProfile`] and implement the behaviour themselves.

use std::fmt;
use std::sync::OnceLock;

pub mod chat;
pub mod dbc;
pub mod ui;
pub mod update_fields;

pub use chat::ChatTypes;
pub use dbc::DbcLayouts;
pub use ui::ScriptArgs;
pub use update_fields::UpdateFieldLayout;

/// A supported reference client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Expansion {
    /// World of Warcraft 1.12.1 (5875).
    Classic,
    /// World of Warcraft 3.3.5a (12340).
    Wotlk,
}

/// The variable that selects the target; unset is [`Expansion::Classic`].
pub const EXPANSION_ENV: &str = "WOW_EXPANSION";

/// A [`EXPANSION_ENV`] value that names no supported target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownExpansion(pub String);

impl fmt::Display for UnknownExpansion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "${EXPANSION_ENV}={:?} names no supported client; expected \"classic\" or \"wotlk\"",
            self.0
        )
    }
}

impl std::error::Error for UnknownExpansion {}

impl Expansion {
    pub const ALL: [Expansion; 2] = [Expansion::Classic, Expansion::Wotlk];

    /// The target an [`EXPANSION_ENV`] value names, any case; unset or empty is Classic.
    pub fn from_env_value(value: Option<&str>) -> Result<Self, UnknownExpansion> {
        match value.map(str::trim) {
            None | Some("") => Ok(Expansion::Classic),
            Some(v) if v.eq_ignore_ascii_case("classic") => Ok(Expansion::Classic),
            Some(v) if v.eq_ignore_ascii_case("wotlk") => Ok(Expansion::Wotlk),
            Some(v) => Err(UnknownExpansion(v.to_string())),
        }
    }

    /// The process's target, read from [`EXPANSION_ENV`] on first use and fixed from then on, so
    /// every subsystem sees the same one. Panics on a value naming no supported client.
    pub fn active() -> Expansion {
        static ACTIVE: OnceLock<Expansion> = OnceLock::new();
        *ACTIVE.get_or_init(|| {
            let value = std::env::var(EXPANSION_ENV).ok();
            Expansion::from_env_value(value.as_deref()).unwrap_or_else(|e| panic!("{e}"))
        })
    }

    pub fn profile(self) -> &'static ExpansionProfile {
        match self {
            Expansion::Classic => &CLASSIC,
            Expansion::Wotlk => &WOTLK,
        }
    }
}

impl fmt::Display for Expansion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let p = self.profile();
        write!(f, "{} ({})", p.version, p.build)
    }
}

/// The facts of one reference client.
#[derive(Debug)]
pub struct ExpansionProfile {
    pub expansion: Expansion,
    pub version: &'static str,
    pub build: u16,
    pub logon: LogonFacts,
    pub archives: ArchiveLayout,
    pub glue: GlueScreens,
    pub dbc: DbcLayouts,
    pub update_fields: &'static UpdateFieldLayout,
    /// `CMSG_MESSAGECHAT`'s `type` per chat lane.
    pub chat_types: &'static ChatTypes,
    /// How a script handler receives its frame and arguments.
    pub script_args: ScriptArgs,
    /// Whether the UI's Lua answers as Lua 5.0: no `select`, `coroutine`, `string.match` or string
    /// metatable, an `n`-based table library and 5.0's `collectgarbage(limit)`. Otherwise it is
    /// stock Lua 5.1 plus the client's `bit` library.
    pub lua_50: bool,
    /// Whether the unit-token resolver raises `Unknown unit name: %s` for a token it does not
    /// recognise, rather than naming nobody.
    pub unknown_unit_token_raises: bool,
    /// Whether the Lua API has `GetCVarBool`, a CVar's value read as a boolean.
    pub get_cvar_bool: bool,
    /// Whether FontString and Button have `SetFormattedText(fmt, ...)`, which sets the text
    /// through the client's own formatter.
    pub set_formatted_text: bool,
    /// Whether an XML `<Scripts>` handler's `function="Name"` binds the global `Name` as the
    /// handler, in place of its body.
    pub script_function_attribute: bool,
    /// Whether XML `inherits=` (and `CreateFrame`'s template argument) is a comma-separated list
    /// of templates applied left to right, each name trimmed, rather than one verbatim name.
    pub inherits_list: bool,
    /// Whether an XML frame or layer region's `parentKey="k"` stores the new object as its
    /// parent's field `k`.
    pub parent_key: bool,
    /// Whether frames carry `SetAttribute`/`GetAttribute`, a case-insensitive name-to-value store
    /// whose change fires `OnAttributeChanged`.
    pub frame_attributes: bool,
    /// The script names `SetScript` and XML `<Scripts>` accept beyond 1.12.1's.
    pub extra_script_kinds: &'static [&'static str],
    /// `NUM_CHAT_WINDOWS`, the chat frames the stock FrameXML builds; the engine's settings array
    /// holds ten records in both clients.
    pub chat_windows: usize,
    /// Whether the Lua API has the Battle.net verbs (`BNConnected`, `BNGetNumFriends`, …).
    pub battle_net_api: bool,
    /// Whether the base library has `securecall`, a protected call through the error handler.
    pub secure_call: bool,
    /// Whether a chat window's record carries an uninteractable flag: `SetChatWindowUninteractable`
    /// stores it and `GetChatWindowInfo` answers it tenth.
    pub chat_window_uninteractable: bool,
    /// Whether the channel joins are `JoinTemporaryChannel` and `JoinPermanentChannel` (with
    /// `JoinChannelByName` the temporary one), taking `hasVoice`, and `CMSG_JOIN_CHANNEL` and
    /// `CMSG_LEAVE_CHANNEL` lead with the channel's `ChatChannels.dbc` id.
    pub permanent_channels: bool,
    /// Whether a `CHAT_MSG_*` event carries arg11 (the line id), arg12 (a player sender's guid
    /// string) and arg13 (a Battle.net presence id) after 1.12.1's ten.
    pub chat_msg_line_args: bool,
    /// Whether frames carry `IsMouseOver(top, bottom, left, right)`.
    pub is_mouse_over: bool,
    /// Whether the Lua API has `GetAutoCompleteResults`, the name lookup behind whisper targets.
    pub auto_complete: bool,
    /// The `ChrRaces` ids a player can create, ascending.
    pub playable_races: &'static [u8],
    /// The highest geoset id the character compositor selects; a character model's higher ids
    /// keep the visibility they load with.
    pub character_geosets_through: u16,
    /// Whether a WMO's liquid resolves to a `LiquidType` id rather than reading as a type nibble:
    /// MOGP `groupLiquid` is that id (0 none) under root MOHD flag `0x4`, else the nibble + 1
    /// (`0xf` none), and an MLIQ tile nibble becomes its id the same way.
    pub wmo_liquid_type_ids: bool,
}

/// What the client tells realmd about itself.
#[derive(Debug)]
pub struct LogonFacts {
    /// The logon challenge's protocol version byte.
    pub protocol_version: u8,
    /// The logon challenge's major, minor and patch version.
    pub version: [u8; 3],
    /// The client's integrity digest `H` under the mangos-family `crc_salt`, which the logon
    /// proof's `crc_hash` answers as `SHA1(A ‖ H)`: the digest the Windows client computes.
    pub integrity_windows: [u8; 20],
    /// The same for the Mac client.
    pub integrity_macos: [u8; 20],
}

/// What the stock glue screens show, for the ones benilla draws natively.
#[derive(Debug)]
pub struct GlueScreens {
    /// The login screen's background scene, `Interface\Glues\Models\UI_<name>\UI_<name>`.
    pub login_scene: &'static str,
    /// The WoW logo on the login, character select and character create screens.
    pub logo: &'static str,
}

/// How the reference client composes its patch chain from the `Data` folder. Paths are relative to
/// `Data`, `/`-separated, matched case-insensitively; `{locale}` is the client's locale code.
#[derive(Debug)]
pub struct ArchiveLayout {
    /// The base archives, lowest priority first.
    pub base: &'static [BaseArchive],
    /// Patches mounted by name above the base, lowest priority first.
    pub patches: &'static [&'static str],
    /// Patch globs (`?` is exactly one character). Every match mounts above [`Self::patches`],
    /// ascending by case-folded `\`-separated path.
    pub patch_globs: &'static [&'static str],
    /// Archives mounted above every patch, lowest priority first.
    pub tail: &'static [&'static str],
    /// Archives the reference mounts with a mechanism benilla does not implement: present, they
    /// fail the chain rather than be skipped.
    pub unsupported: &'static [&'static str],
}

/// One base archive of an [`ArchiveLayout`].
#[derive(Debug, PartialEq, Eq)]
pub struct BaseArchive {
    pub path: &'static str,
    /// The reference fails to start without it.
    pub required: bool,
}

impl ArchiveLayout {
    /// Whether any path names the locale, so the chain needs one.
    pub fn uses_locale(&self) -> bool {
        self.base
            .iter()
            .map(|b| b.path)
            .chain(self.patches.iter().copied())
            .chain(self.patch_globs.iter().copied())
            .any(|p| p.contains(LOCALE))
    }
}

/// The locale placeholder in an [`ArchiveLayout`] path.
pub const LOCALE: &str = "{locale}";

const fn optional(path: &'static str) -> BaseArchive {
    BaseArchive {
        path,
        required: false,
    }
}

const fn required(path: &'static str) -> BaseArchive {
    BaseArchive {
        path,
        required: true,
    }
}

/// 1.12.1: the mounter's table (`0x82e12c`), priority 0x36 (`dbc.MPQ`) to 0x3f (`model.MPQ`);
/// `base.MPQ` is closed before mounting (`0x5aa2d0`). Then `patch.MPQ`, every `patch-?.MPQ`
/// (glob `0x82edbc`, sorted descending with `strnicmp` and mounted walking backwards, `0x403740`),
/// then `speech2.MPQ`. A missing base archive is skipped.
static CLASSIC: ExpansionProfile = ExpansionProfile {
    expansion: Expansion::Classic,
    version: "1.12.1",
    build: 5875,
    // `H`: vmangos's `allowed_clients` row (`20221117065844_logon.sql`), identical to cmangos's
    // `RealmList.cpp`.
    logon: LogonFacts {
        protocol_version: 3,
        version: [1, 12, 1],
        integrity_windows: [
            0x95, 0xed, 0xb2, 0x7c, 0x78, 0x23, 0xb3, 0x63, 0xcb, 0xdd, 0xab, 0x56, 0xa3, 0x92,
            0xe7, 0xcb, 0x73, 0xfc, 0xca, 0x20,
        ],
        integrity_macos: [
            0x8d, 0x17, 0x3c, 0xc3, 0x81, 0x96, 0x1e, 0xeb, 0xab, 0xf3, 0x36, 0xf5, 0xe6, 0x67,
            0x5b, 0x10, 0x1b, 0xb5, 0x13, 0xe5,
        ],
    },
    archives: ArchiveLayout {
        base: &[
            optional("dbc.MPQ"),
            optional("speech.MPQ"),
            optional("fonts.MPQ"),
            optional("interface.MPQ"),
            optional("misc.MPQ"),
            optional("sound.MPQ"),
            optional("wmo.MPQ"),
            optional("terrain.MPQ"),
            optional("texture.MPQ"),
            optional("model.MPQ"),
        ],
        patches: &["patch.MPQ"],
        patch_globs: &["patch-?.MPQ"],
        tail: &["speech2.MPQ"],
        unsupported: &[],
    },
    // `AccountLogin.xml`'s ModelFFX and `AccountLoginLogo`.
    glue: GlueScreens {
        login_scene: "MainMenu",
        logo: "Interface\\Glues\\Common\\Glues-WoW-Logo",
    },
    dbc: dbc::CLASSIC,
    update_fields: &update_fields::CLASSIC,
    chat_types: &chat::CLASSIC,
    script_args: ScriptArgs::Globals,
    // `$Lua: Lua 5.0` at `0x811b30`; `InitLua 0x7039e0` opens base, string, table, math and bit.
    lua_50: true,
    // `0x515970` ends in `luaL_error` at `0x515c14`.
    unknown_unit_token_raises: true,
    // No 1.12.1 registration table names it (`reference/1.12-shapes.tsv`).
    get_cvar_bool: false,
    // No 1.12.1 registration table names it (`reference/1.12-shapes.tsv`).
    set_formatted_text: false,
    // The handler loader reads only the element's body (`0x704c70`); the schema has no attribute.
    script_function_attribute: false,
    // The inherit lookup (`0x6ee6f0`) takes the attribute whole; it has no splitter.
    inherits_list: false,
    // The 1.12 schema has no such attribute; `LoadXML` reads no field of that name.
    parent_key: false,
    // No method table binds `SetAttribute` (`reference/1.12-shapes.tsv`).
    frame_attributes: false,
    extra_script_kinds: &[],
    // `NUM_CHAT_WINDOWS = 7` (`ChatFrame.lua:5`).
    chat_windows: 7,
    // No 1.12.1 registration table names a `BN` verb (`reference/1.12-shapes.tsv`).
    battle_net_api: false,
    // Not among the 36 base-library entries (`0x811e28`).
    secure_call: false,
    // `GetChatWindowInfo` (`0x4a0ba0`) answers nine values.
    chat_window_uninteractable: false,
    // `JoinChannelByName` (`0x49ff00`) is the one join; the body is two cstrings.
    permanent_channels: false,
    // The fire format `%s%s%s%s%s%s%d%d%s%d` (`0x844608`).
    chat_msg_line_args: false,
    // No 1.12.1 method table binds it (`reference/1.12-shapes.tsv`).
    is_mouse_over: false,
    // No 1.12.1 registration table names it (`reference/1.12-shapes.tsv`).
    auto_complete: false,
    // Human through Troll; Goblin (9) and above are not creatable.
    playable_races: &[1, 2, 3, 4, 5, 6, 7, 8],
    // `0x477520` disables 0..=1700 (`0x6a4`) before enabling its picks.
    character_geosets_through: 1700,
    // `0x6b9f10` reads `groupLiquid` as the type nibble, `0xf` none; MOHD is not consulted.
    wmo_liquid_type_ids: false,
};

/// 3.3.5a, when `Data\common.MPQ` exists (`0x402b90`; without it the client falls back to the
/// 1.x names, which no 3.3.5a install ships). The mounter (`0x405dd0`) walks the table at
/// `0xab6168` from priority 63 down, so the table reads highest first; this list is it reversed,
/// the entries for that mode only. `common`, `locale-*` and `speech-*` are required ("Failed to
/// open archive"); the rest are skipped when missing. Patches start at 64 (`0x405ab0`):
/// `patch-<loc>.MPQ`, `patch.MPQ`, then both globs' matches by full path, sorted descending with
/// `strnicmp` (`0x401200`) and mounted walking backwards.
static WOTLK: ExpansionProfile = ExpansionProfile {
    expansion: Expansion::Wotlk,
    version: "3.3.5a",
    build: 12340,
    // `H`: AzerothCore's `build_info` row for 12340 (`db_auth/build_info.sql`).
    logon: LogonFacts {
        protocol_version: 8,
        version: [3, 3, 5],
        integrity_windows: [
            0xcd, 0xcb, 0xbd, 0x51, 0x88, 0x31, 0x5e, 0x6b, 0x4d, 0x19, 0x44, 0x9d, 0x49, 0x2d,
            0xbc, 0xfa, 0xf1, 0x56, 0xa3, 0x47,
        ],
        integrity_macos: [
            0xb7, 0x06, 0xd1, 0x3f, 0xf2, 0xf4, 0x01, 0x88, 0x39, 0x72, 0x94, 0x61, 0xe3, 0xf8,
            0xa0, 0xe2, 0xb5, 0xfd, 0xc0, 0x34,
        ],
    },
    archives: ArchiveLayout {
        base: &[
            optional("{locale}/lichking-speech-{locale}.MPQ"),
            optional("{locale}/expansion-speech-{locale}.MPQ"),
            optional("{locale}/lichking-locale-{locale}.MPQ"),
            optional("{locale}/expansion-locale-{locale}.MPQ"),
            required("{locale}/speech-{locale}.MPQ"),
            required("{locale}/locale-{locale}.MPQ"),
            optional("common-2.MPQ"),
            required("common.MPQ"),
            optional("lichking.MPQ"),
            optional("expansion.MPQ"),
        ],
        patches: &["{locale}/patch-{locale}.MPQ", "patch.MPQ"],
        patch_globs: &["patch-?.MPQ", "{locale}/patch-{locale}-?.MPQ"],
        tail: &[],
        // Mounted above the patches and overlaid into each of them (`0x405f30`).
        unsupported: &["alternate.MPQ"],
    },
    // `AccountLogin_OnLoad` sets the Northrend scene (`UI_MainMenu` is the streaming trial's);
    // `AccountLoginLogo`, `CharacterSelectLogo` and `CharacterCreateLogo` name the logo.
    glue: GlueScreens {
        login_scene: "MainMenu_Northrend",
        logo: "Interface\\Glues\\Common\\Glues-WoW-WotLKLogo",
    },
    dbc: dbc::WOTLK,
    update_fields: &update_fields::WOTLK,
    chat_types: &chat::WOTLK,
    script_args: ScriptArgs::Parameters,
    // `$Lua: Lua 5.1.1` at `0xa470a0`; the base and library tables bind `select`, `coroutine`,
    // `string.match`/`gmatch`/`reverse`, `table.maxn` and `math.fmod`/`modf`
    // (`reference/3.3.5a-shapes.tsv`).
    lua_50: false,
    // `Wow.exe` holds no unit-name error string at all; its resolver also takes `focus`, `boss%d`,
    // `vehicle`, `arenapet`, `questnpc` and `commentator` (the token strings at `0x62156c`).
    unknown_unit_token_raises: false,
    // Both registration tables bind it: glue `0x4ddd90`, world `0x5100c0`.
    get_cvar_bool: true,
    // FontString's table binds it at `0x48d800`, Button's at `0x9779b0`.
    set_formatted_text: true,
    // `CSimpleFrame`'s script loader reads `function` (`0x490009`) after compiling any body.
    script_function_attribute: true,
    // The shipped FrameXML names lists both ways, `"ChatFrameTemplate,FloatingBorderedFrame"`
    // and `"SecureActionButtonTemplate, ActionButtonTemplate"`, and the client builds both.
    inherits_list: true,
    // The shipped FrameXML reads such keys from `OnLoad`s: `GeneralDockManager`'s reads the
    // `overflowButton` its XML child declares (`FloatingChatFrame.lua:2048`).
    parent_key: true,
    // Frame's method table binds both (`0x49f610`, `0x49f2d0`).
    frame_attributes: true,
    // Both have parameter templates in the script compiler (`ScriptArgs::parameters`).
    extra_script_kinds: &["OnAttributeChanged", "OnInputLanguageChanged"],
    // `NUM_CHAT_WINDOWS = 10` (`ChatFrame.lua:5`).
    chat_windows: 10,
    // The world table binds them, `BNGetNumFriends` at `0x534590`.
    battle_net_api: true,
    // The FrameScript base library binds it at `0x816f30`.
    secure_call: true,
    // The setter `0x4fc6e0` stores `+0x94` of a 0x100-byte record; `0x4fbd90` pushes it tenth.
    chat_window_uninteractable: true,
    // `0x50daa0`/`0x50dac0` over `0x50d900`; the builders `0x507c58`, `0x5095ca`.
    permanent_channels: true,
    // The fire format `%s%s%s%s%s%s%d%d%s%d%d%s%u` (`0x9fbe64`, helper `0x4fdc00`).
    chat_msg_line_args: true,
    // Frame's method table binds it at `0x49e100`.
    is_mouse_over: true,
    // The world table binds it at `0x57b3a0`.
    auto_complete: true,
    // 1.12.1's eight, Blood Elf and Draenei; the shipped `CharBaseInfo` lists exactly these.
    playable_races: &[1, 2, 3, 4, 5, 6, 7, 8, 10, 11],
    // Groups 17 (eye glow) and 18 (belt) join the selection; WowUnreal's 3.3.5a builder defaults
    // both to variant 1 and picks 18 from the belt.
    character_geosets_through: 1899,
    // The group loader (`0x7d8390`) tests MOHD `0x4` and converts through `0x7d7310`.
    wmo_liquid_type_ids: true,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_or_empty_selects_classic() {
        assert_eq!(Expansion::from_env_value(None), Ok(Expansion::Classic));
        assert_eq!(Expansion::from_env_value(Some("")), Ok(Expansion::Classic));
        assert_eq!(
            Expansion::from_env_value(Some("  ")),
            Ok(Expansion::Classic)
        );
    }

    #[test]
    fn names_select_their_target_in_any_case() {
        assert_eq!(
            Expansion::from_env_value(Some("classic")),
            Ok(Expansion::Classic)
        );
        assert_eq!(
            Expansion::from_env_value(Some("WotLK")),
            Ok(Expansion::Wotlk)
        );
        assert_eq!(
            Expansion::from_env_value(Some(" wotlk ")),
            Ok(Expansion::Wotlk)
        );
    }

    #[test]
    fn an_unknown_target_is_an_error_not_classic() {
        assert_eq!(
            Expansion::from_env_value(Some("tbc")),
            Err(UnknownExpansion("tbc".into()))
        );
        assert!(Expansion::from_env_value(Some("12340")).is_err());
    }

    #[test]
    fn each_profile_names_its_own_client() {
        for e in Expansion::ALL {
            assert_eq!(e.profile().expansion, e);
        }
        assert_eq!((CLASSIC.version, CLASSIC.build), ("1.12.1", 5875));
        assert_eq!((WOTLK.version, WOTLK.build), ("3.3.5a", 12340));
        assert_eq!(Expansion::Wotlk.to_string(), "3.3.5a (12340)");
    }

    #[test]
    fn each_client_names_its_own_version_to_realmd() {
        assert_eq!(
            (CLASSIC.logon.protocol_version, CLASSIC.logon.version),
            (3, [1, 12, 1])
        );
        assert_eq!(
            (WOTLK.logon.protocol_version, WOTLK.logon.version),
            (8, [3, 3, 5])
        );
        assert_eq!(CLASSIC.logon.integrity_windows[..2], [0x95, 0xed]);
        assert_eq!(WOTLK.logon.integrity_windows[..2], [0xcd, 0xcb]);
        assert_eq!(WOTLK.logon.integrity_macos[18..], [0xc0, 0x34]);
    }

    #[test]
    fn each_client_shows_its_own_login_scene_and_logo() {
        assert_eq!(CLASSIC.glue.login_scene, "MainMenu");
        assert_eq!(
            CLASSIC.glue.logo,
            "Interface\\Glues\\Common\\Glues-WoW-Logo"
        );
        assert_eq!(WOTLK.glue.login_scene, "MainMenu_Northrend");
        assert_eq!(
            WOTLK.glue.logo,
            "Interface\\Glues\\Common\\Glues-WoW-WotLKLogo"
        );
    }

    #[test]
    fn only_wotlk_has_a_locale_folder() {
        assert!(!CLASSIC.archives.uses_locale());
        assert!(WOTLK.archives.uses_locale());
    }

    #[test]
    fn wotlk_requires_exactly_the_reference_s_required_archives() {
        let required: Vec<_> = WOTLK
            .archives
            .base
            .iter()
            .filter(|b| b.required)
            .map(|b| b.path)
            .collect();
        assert_eq!(
            required,
            [
                "{locale}/speech-{locale}.MPQ",
                "{locale}/locale-{locale}.MPQ",
                "common.MPQ"
            ]
        );
        assert!(CLASSIC.archives.base.iter().all(|b| !b.required));
    }
}
