//! The client generations benilla targets and the structural facts of each reference client. One
//! [`Expansion`] is selected per process ([`Expansion::active`]); the crates that own a behaviour
//! read the facts they need from its [`ExpansionProfile`] and implement the behaviour themselves.

use std::fmt;
use std::sync::OnceLock;

pub mod dbc;

pub use dbc::DbcLayouts;

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
    /// The `ChrRaces` ids a player can create, ascending.
    pub playable_races: &'static [u8],
    /// The highest geoset id the character compositor selects; a character model's higher ids
    /// keep the visibility they load with.
    pub character_geosets_through: u16,
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
    // Human through Troll; Goblin (9) and above are not creatable.
    playable_races: &[1, 2, 3, 4, 5, 6, 7, 8],
    // `0x477520` disables 0..=1700 (`0x6a4`) before enabling its picks.
    character_geosets_through: 1700,
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
    // 1.12.1's eight, Blood Elf and Draenei; the shipped `CharBaseInfo` lists exactly these.
    playable_races: &[1, 2, 3, 4, 5, 6, 7, 8, 10, 11],
    // Groups 17 (eye glow) and 18 (belt) join the selection; WowUnreal's 3.3.5a builder defaults
    // both to variant 1 and picks 18 from the belt.
    character_geosets_through: 1899,
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
