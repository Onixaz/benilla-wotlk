//! The registry's table: every host-backed CVar, its default, and that default's standing against
//! the reference's registered one ([`Reference`]). The runtime that holds the live values is the
//! parent module.

/// One host-backed CVar: its name, benilla's default, and where that default stands against the
/// reference's ([`Reference`]).
pub(crate) struct Registered {
    /// The registered name, in the reference's own spelling.
    pub(crate) name: &'static str,
    /// What a fresh `benilla-config` runs at, and what `GetCVar` answers until the player moves it.
    pub(crate) default: &'static str,
    /// Where `default` stands against the reference's; the archive compares against the
    /// reference's string ([`Registered::reference_default`]).
    pub(crate) reference: Reference,
    /// Registered with flag bit1 (`rec+0x1c & 0x2`, `flags` 2 or 3 at the register site): a write
    /// is staged in [`super::Row::pending`] until [`super::Cvars::commit_latched`].
    pub(crate) latched: bool,
    /// The archive the row is saved into, its register site's `flags & 0x30`.
    pub(crate) scope: Scope,
    /// Registered with `0x40`: Lua's `GetCVar`, `SetCVar` and `GetCVarInfo` treat it as unknown
    /// (`0x510083`, `0x514c5f`), and it is still archived.
    pub(crate) hidden: bool,
}

/// Which file a row is archived in (`CVar::IterateForArchive` `0x767030` walks one at a time).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    /// `Config.wtf` (`CVar::SaveAll` `0x767100`).
    Config,
    /// `0x10`: the account's `config-cache.wtf`, account-data slot 0; reset at each account login
    /// (`CVar::DeleteAccountCVars` `0x7673a0`).
    Account,
    /// `0x20`: the character's `config-cache.wtf`, slot 1; dropped at every UI shutdown
    /// (`CVar::DeleteCharacterCVars` `0x767350`).
    Character,
}

impl Registered {
    /// The string 12340 registers, which its archive compares a value against: benilla's own
    /// default where the reference has no such setting.
    pub(crate) const fn reference_default(&self) -> &'static str {
        match self.reference {
            Reference::Same(d) => d,
            Reference::Overridden { registered, .. } => registered,
            Reference::Deviates { value, .. } => value,
            Reference::Ours(_) => self.default,
        }
    }

    /// Mark the row latched.
    pub(crate) const fn latched(self) -> Self {
        Self {
            latched: true,
            ..self
        }
    }

    /// Mark the row per-character (`0x20`).
    pub(crate) const fn per_character(self) -> Self {
        Self {
            scope: Scope::Character,
            ..self
        }
    }

    /// Mark the row per-account (`0x10`).
    pub(crate) const fn per_account(self) -> Self {
        Self {
            scope: Scope::Account,
            ..self
        }
    }

    /// Mark the row hidden from Lua (`0x40`).
    pub(crate) const fn hidden(self) -> Self {
        Self {
            hidden: true,
            ..self
        }
    }
}

/// benilla's default against the reference's: the string the reference's `CVar::Register`
/// (`0x63db90`) passes for the name, or, for a setting 1.12 keeps in FrameXML, the value
/// `UIOptionsFrame.lua` boots it at. Not a `Config.wtf` line (`SaveConfig 0x63d980` writes only
/// values off their default), and not always what a fresh install runs at: `hwDetect` rewrites
/// sixteen video CVars from `VideoHardware.dbc` before the first frame ([`Reference::Overridden`]).
/// Each row's register site or FrameXML line is cited above it.
#[allow(dead_code)] // read only by the tests
pub(crate) enum Reference {
    /// The reference registers this default and benilla ships it; the test compares the two.
    Same(&'static str),
    /// The reference registers `registered`, but its own boot code overwrites it before the first
    /// frame; `default` is where that lands, and `why` is the override.
    Overridden {
        registered: &'static str,
        why: &'static str,
    },
    /// The reference ships `value` and benilla ships another; `why` is the reason.
    Deviates {
        value: &'static str,
        why: &'static str,
    },
    /// No reference setting to match: benilla's own knob, or a later-era name for something 1.12
    /// never made settable. `why` says which, and what the reference does instead.
    Ours(&'static str),
}

/// A row whose default is the reference's own registered string.
pub(super) const fn same(name: &'static str, default: &'static str) -> Registered {
    Registered {
        name,
        default,
        reference: Reference::Same(default),
        latched: false,
        scope: Scope::Config,
        hidden: false,
    }
}

/// A row whose default follows the reference's boot-time override of its registered string.
const fn overridden(
    name: &'static str,
    default: &'static str,
    registered: &'static str,
    why: &'static str,
) -> Registered {
    Registered {
        name,
        default,
        reference: Reference::Overridden { registered, why },
        latched: false,
        scope: Scope::Config,
        hidden: false,
    }
}

/// A row that ships something other than the reference's `value`, for `why`.
const fn deviates(
    name: &'static str,
    default: &'static str,
    value: &'static str,
    why: &'static str,
) -> Registered {
    Registered {
        name,
        default,
        reference: Reference::Deviates { value, why },
        latched: false,
        scope: Scope::Config,
        hidden: false,
    }
}

/// A row the reference has no counterpart for.
const fn ours(name: &'static str, default: &'static str, why: &'static str) -> Registered {
    Registered {
        name,
        default,
        reference: Reference::Ours(why),
        latched: false,
        scope: Scope::Config,
        hidden: false,
    }
}

/// The table as the script VM's registrar wants it: `(name, default)` pairs in table order.
pub(crate) fn registered_pairs() -> impl Iterator<Item = (&'static str, &'static str)> {
    registered_rows().map(|r| (r.name, r.default))
}

/// [`REGISTERED`], then [`REGISTERED_WOTLK`], then the camera's generated tuning rows.
pub(crate) fn registered_rows() -> impl Iterator<Item = &'static Registered> {
    REGISTERED
        .iter()
        .chain(REGISTERED_WOTLK)
        .chain(super::camera_tuning::ROWS.iter())
}

/// The rows 3.3.5a registers and 1.12.1 has no name for, each with a reader in the 3.3.5a stock
/// UI. Their register sites sit in one run (`0x51e2d8`-`0x51f2d2`, `CVar::Register` `0x767fc0`).
pub(crate) const REGISTERED_WOTLK: &[Registered] = &[
    // "The style of Edit Boxes for the ChatFrame" (`0x51e2d8`, "im"); `ChatFrame.lua` hides the
    // edit box between lines in "im" and keeps it in "classic".
    same("chatStyle", "im").per_account(),
    // `0x51e2fb`, "1": a click on the window focuses the edit box, in "im" style only.
    same("wholeChatWindowClickable", "1").per_account(),
    // `0x51e31b`, "popout": where a new Real ID conversation opens.
    same("conversationMode", "popout").per_account(),
    // `0x51e35e`, "1": the mouse wheel scrolls a chat window (`FloatingChatFrame.lua`).
    same("chatMouseScroll", "1").per_account(),
    // `0x51f2d2`, "": the GM a reload should reopen the GM chat window for; UIParent's
    // `VARIABLES_LOADED` arm loads `Blizzard_GMChatUI` when it is not "".
    same("lastTalkedToGM", "").per_account(),
    // `0x51f147`, "3": when `IsThreatWarningEnabled` answers 1 (0 off, 1 in dungeons, 2 in a
    // group, 3 always), which the stock unit frames' threat glow reads.
    same("threatWarning", "3").per_character(),
    // `0x51ee61` and `0x51ee84`, "" with flags 0x20: the panes the character sheet's two stat
    // dropdowns show; `PaperDollFrame.lua`'s `VARIABLES_LOADED` arm writes the class default
    // when either is "". Kept in the character's `config-cache.wtf`, account-data slot 1.
    same("playerStatLeftDropdown", "").per_character(),
    same("playerStatRightDropdown", "").per_character(),
    // `0x51ee19`, "0" with flags 0x20: the keyring button, which the stock bar's first-key
    // `BAG_UPDATE` arm turns on (`MainMenuBar.lua:243-249`).
    same("showKeyring", "0").per_character(),
    // `0x51e027`, "" with flags 0x120: the tracked achievements, which `LoadTrackedState` reads
    // and the tracking verbs write (`0x5b3840`, `0x5b1fa0`); per character like the dropdowns.
    same("trackedAchievements", "").per_character(),
    // `0x51e001`, "" with flags 0x120: the untimed quest watches, which `LoadTrackedQuests`
    // (`0x5e65b0`) reads once the first quest log is built and `SaveTrackedQuests` (`0x5e0240`)
    // writes; per character like the achievements.
    same("trackedQuests", "").per_character(),
    // `0x51f3a9`, "" with flags 0x20: the minimap's town tracking type, which `LoadTrackingInfo`
    // (`0x57e070`) writes and `LoadTrackedState` (`0x57ea60`) restores.
    same("minimapTrackedInfo", "").per_character(),
    // `0x51e0d0` "0" and `0x51e0f3` "7", flags 0x20: the objective tracker's sort and its filter
    // bits (achievements 1, completed quests 2, remote zones 4), read on `VARIABLES_LOADED`
    // (`WatchFrame.lua:279-280`).
    same("trackerSorting", "0").per_character(),
    same("trackerFilter", "7").per_character(),
    // `0x51f3ce`, "1" with flags 0x20: the quest POI switch `UpdateAllQuestPOI` (`0x5e5a50`) reads.
    same("questPOI", "1").per_character(),
    // `0x51f25c`, `0x51f282`, `0x51f2aa`, flags 0x120: the dungeon finder's roles ("0", a bit
    // mask) and its selected dungeons and collapsed headers ("", bit arrays), per character.
    same("lfgSelectedRoles", "0").per_character(),
    same("lfdCollapsedHeaders", "").per_character(),
    same("lfdSelectedDungeons", "").per_character(),
    // The login screen's third agreement (`readTerminationWithoutNotice`, "0"), and the saved
    // Battle.net account list (`accountList`, "") and authenticator flag
    // (`g_accountUsesToken`, "0") the glue's account verbs read and write.
    same("readTerminationWithoutNotice", "0"),
    same("accountList", ""),
    same("g_accountUsesToken", "0"),
    // The stock options panels' remaining rows, glue and in-game alike: "Display the launcher
    // when starting the game" (`showToolsUI`, "-1"), "prevent resizing in windowed mode"
    // (`windowResizeLock`, "0"), and the 3.3.5a sound switches the Sound panel binds, each at its
    // registered default. Their host wiring is the options work's (#150).
    same("showToolsUI", "-1"),
    // `windowResizeLock` (`0x76a630`, "0", flags 1, callback `0x769890`): live, it refuses the
    // window's border drag (`ResizeDragWindow 0x869970`). The knob is
    // [`crate::video::VideoConfig::resize_lock`].
    same("windowResizeLock", "0"),
    same("Sound_EnableAllSound", "1"),
    same("Sound_EnableSFX", "1"),
    same("Sound_EnableErrorSpeech", "1"),
    same("Sound_EnableEmoteSounds", "1"),
    same("Sound_EnablePetSounds", "1"),
    same("Sound_EnableMusic", "1"),
    same("Sound_ZoneMusicNoDelay", "0"),
    same("Sound_EnableAmbience", "1"),
    same("Sound_EnableReverb", "0"),
    same("Sound_EnableSoftwareHRTF", "0"),
    // "1" on a machine with more than one processor.
    same("Sound_EnableDSPEffects", "1"),
    // Read by `SE2::Init` only under `Sound_EnableMixMode2` (`0x87d8b4` skips
    // `setSoftwareFormat` at its "0"), so the slider never changes the mix; benilla mixes at the
    // device's rate.
    same("Sound_OutputQuality", "1"),
    // The output device, kept by name and resolved to an index at each init
    // (`crate::sound::engine`). The name default is DirectSound's primary device, which no host
    // here lists, so it resolves to the system default.
    same("Sound_OutputDriverIndex", "0"),
    same("Sound_OutputDriverName", "Primary Sound Driver"),
    same("Sound_NumChannels", "32"),
    // The Effects panel's particle density (`CWorldParam::Initialize 0x78e400`, "1.0", flags 1,
    // callback `0x78d860`: refused outside [0.1, 1.0], else the emission scalar); the first-run
    // pass `0x78dd40` writes "1.0" again on any pixel-shader target past 3. The knob is
    // `benilla_world::particles::ParticleTuning`. Then the Stereo panel's three rows.
    same("particleDensity", "1.0"),
    same("gxStereoEnabled", "0"),
    same("gxStereoConvergence", "1"),
    same("gxStereoSeparation", "25"),
    same("extShadowQuality", "0"),
    // Hardware voices need 12 or more from the driver (`SE2::Init` at `0x87d0e0`); a host without
    // DirectSound 3D hardware reports none and the reference mixes in software, as benilla always
    // does.
    same("Sound_EnableHardware", "0"),
    same("Sound_MasterVolume", "1.0"),
    same("Sound_SFXVolume", "1.0"),
    same("Sound_MusicVolume", "0.4"),
    same("Sound_AmbienceVolume", "0.6"),
    // `environmentDetail` (`0x78e400`, "1.0", callback `0x78dc60`): clamped to [0.5, 1.5], it
    // scales doodad fade levels 1-3 (`CWorldScene::ScaleFadeDist 0x78f570`). The knob is
    // `benilla_world::model_fade::FadeScale`.
    same("environmentDetail", "1.0"),
    // `groundEffectDensity` ("16", callback `0x78dab0`: refused outside [16, 256]), the ground
    // clutter's cells per chunk, and `groundEffectDist` ("70.0", callback `0x78db10`: refused
    // outside [0, 140]), its reach. The knob is `benilla_world::clutter::ClutterConfig`.
    overridden(
        "groundEffectDensity",
        "24",
        "16",
        "the first-run defaults pass (`CWorldParam::SetDefaults 0x78e1a0`) writes \
         `s_detailDoodadDensity[idx]` (`0xadbe5c`: 8, 12, 16, 24) of the matched \
         `VideoHardware.dbc` row, and the fallback row a modern GPU reaches (693 on the reference \
         install's `Logs/gx.log`) holds index 3",
    ),
    same("groundEffectDist", "70.0"),
    // `projectedTextures` ("0", callback `0x78dcf0`: bit 4 of `CWorld::enables2`), which gates
    // the M2 projected batches (`World::ProjectTextureCallback 0x77f500`).
    overridden(
        "projectedTextures",
        "1",
        "0",
        "`CWorldParam::SetDefaultProjectedTextures 0x78ddf0` writes \"1\" on any pixel-shader \
         target past 2 when `videoOptionsVersion` is below 2, which a fresh install's is",
    ),
    // `textureFilteringMode` (`InitializeEngineCallback 0x4047e0`, "1", callback `0x4023c0`:
    // refused outside [0, 5]), read once at engine start into the filter and the anisotropy
    // (`0xab6128`, `0xab6140`). The first-run pass writes "1" from the hardware row's trilinear
    // bit, which every fallback row past 168 sets. The knob is `benilla_assets::TexFilterSetting`.
    same("textureFilteringMode", "1"),
    // `componentTextureLevel` (`CCharacterComponent::Initialize 0x401ff0`, "8"), read once: the
    // composited body texture is `1 << level` px square, clamped to [7, 9]. The knob is
    // `crate::entities::skin_composite::ComponentLevel`.
    same("componentTextureLevel", "8"),
    // The sound system's (`SI2::RegisterCVars`): the chat-duck volumes and the listener seat.
    same("ChatMusicVolume", "0.3"),
    same("ChatSoundVolume", "0.4"),
    same("ChatAmbienceVolume", "0.3"),
    same("Sound_ListenerAtCharacter", "1"),
    // The Interface panels' rows (`0x51dc20`-`0x51f239`); flags 0x20 are per character.
    same("stopAutoAttackOnTargetChange", "0").per_character(),
    same("showTargetOfTarget", "0").per_account(),
    same("targetOfTargetMode", "5").per_account(),
    same("showTargetCastbar", "1").per_account(),
    same("showVKeyCastbar", "1").per_account(),
    same("rotateMinimap", "0").per_account(),
    same("scriptErrors", "0").per_account(),
    same("screenEdgeFlash", "1").per_account(),
    same("displayFreeBagSlots", "0").per_account(),
    same("displayWorldPVPObjectives", "1").per_account(),
    same("showClock", "1").per_account(),
    same("colorblindMode", "0").per_account(),
    same("questFadingDisable", "1").per_account(),
    same("autoQuestWatch", "1").per_account(),
    same("autoQuestProgress", "1").per_account(),
    same("mapQuestDifficulty", "0").per_character(),
    same("advancedWorldMap", "0").per_character(),
    same("watchFrameWidth", "0"),
    same("removeChatDelay", "0").per_account(),
    same("guildRecruitmentChannel", "1").per_account(),
    same("showTimestamps", "none").per_account(),
    same("lockActionBars", "1").per_account(),
    same("alwaysShowActionBars", "0").per_account(),
    same("secureAbilityToggle", "1").per_account(),
    same("CombatHealing", "1").per_account(),
    same("enableCombatText", "1").per_account(),
    same("combatTextFloatMode", "1").per_account(),
    same("fctCombatState", "0").per_account(),
    same("fctDodgeParryMiss", "0").per_account(),
    same("fctDamageReduction", "0").per_account(),
    same("fctRepChanges", "0").per_account(),
    same("fctReactives", "0").per_account(),
    same("fctFriendlyHealers", "0").per_account(),
    same("fctComboPoints", "0").per_account(),
    same("fctLowManaHealth", "1").per_account(),
    same("fctEnergyGains", "0").per_account(),
    same("fctPeriodicEnergyGains", "0").per_account(),
    same("fctHonorGains", "0").per_account(),
    same("fctAuras", "0").per_account(),
    same("fctSpellMechanics", "0").per_account(),
    same("fctSpellMechanicsOther", "0").per_account(),
    same("xpBarText", "0").per_account(),
    same("playerStatusText", "0").per_account(),
    same("petStatusText", "0").per_account(),
    same("partyStatusText", "0").per_account(),
    same("targetStatusText", "0").per_account(),
    same("statusTextPercentage", "0").per_account(),
    same("showPartyBackground", "0").per_account(),
    same("hidePartyInRaid", "0").per_account(),
    same("showPartyPets", "1").per_character(),
    same("showRaidRange", "0").per_character(),
    same("showArenaEnemyFrames", "1").per_character(),
    same("showArenaEnemyCastbar", "1").per_character(),
    same("showArenaEnemyPets", "1").per_character(),
    same("fullSizeFocusFrame", "0").per_character(),
    same("buffDurations", "1").per_account(),
    same("showDispelDebuffs", "0").per_character(),
    same("showCastableBuffs", "0").per_character(),
    same("consolidateBuffs", "0").per_character(),
    same("showCastableDebuffs", "0").per_character(),
    same("showToastOnline", "1").per_account(),
    same("showToastOffline", "1").per_account(),
    same("showToastBroadcast", "1").per_account(),
    same("showToastFriendRequest", "1").per_account(),
    same("showToastConversation", "1").per_account(),
    same("showToastWindow", "1").per_account(),
    same("toastDuration", "4").per_account(),
    same("showNewbieTips", "1").per_account(),
    same("showItemLevel", "0").per_account(),
    same("showTutorials", "1").per_account(),
    // The V keys' two switches, read by `vplates`.
    same("nameplateShowEnemies", "0").per_character(),
    same("nameplateShowEnemyPets", "1").per_character(),
    same("nameplateShowEnemyGuardians", "1").per_character(),
    same("nameplateShowEnemyTotems", "1").per_character(),
    same("nameplateShowFriends", "0").per_character(),
    same("nameplateShowFriendlyPets", "1").per_character(),
    same("nameplateShowFriendlyGuardians", "1").per_character(),
    same("nameplateShowFriendlyTotems", "1").per_character(),
    same("nameplateAllowOverlap", "1").per_character(),
    same("PushToTalkSound", "0").per_account(),
    same("timeMgrAlarmEnabled", "0").per_account(),
    // `0x51ef47`: "0" in enUS, koKR and esMX, "1" elsewhere; the calendar's clock text reads it.
    same("timeMgrUseMilitaryTime", "0").per_account(),
    // `0x51eaf1`-`0x51eb92`, flags 0x20: the calendar's five filters (`SetCalendarFilterCVarCallback`
    // `0x5127a0`).
    same("calendarShowWeeklyHolidays", "1").per_character(),
    same("calendarShowDarkmoon", "1").per_character(),
    same("calendarShowBattlegrounds", "0").per_character(),
    same("calendarShowLockouts", "1").per_character(),
    same("calendarShowResets", "0").per_character(),
    same("threatShowNumeric", "0").per_account(),
    same("threatPlaySounds", "1").per_account(),
    same("ShowClassColorInNameplate", "0").per_character(),
    same("previewTalents", "0").per_character(),
    // Voice chat (`0x4d10f4`-`0x4d12e7`): with no voice, these hold the panel's state only.
    same("OutboundChatVolume", "1.0"),
    same("InboundChatVolume", "1.0"),
    same("VoiceChatMode", "0"),
    same("VoiceActivationSensitivity", "0.4"),
    same("EnableMicrophone", "1"),
    same("EnableVoiceChat", "0"),
    same("PushToTalkButton", "`"),
    same("Sound_VoiceChatInputDriverIndex", "0"),
    same("Sound_VoiceChatOutputDriverIndex", "0"),
    // The Names panel's unit-name rows (`0x7e61f1`-`0x7e630f`).
    same("UnitNameEnemyPlayerName", "1").per_account(),
    same("UnitNameEnemyPetName", "1").per_account(),
    same("UnitNameEnemyGuardianName", "0").per_account(),
    same("UnitNameEnemyTotemName", "0").per_account(),
    same("UnitNameFriendlyPlayerName", "1").per_account(),
    same("UnitNameFriendlyPetName", "1").per_account(),
    same("UnitNameFriendlyGuardianName", "0").per_account(),
    same("UnitNameFriendlyTotemName", "0").per_account(),
    same("UnitNameNonCombatCreatureName", "0").per_account(),
    // The Controls and Combat panels' rows (`0x51da81`-`0x51dbd3`).
    same("autoDismountFlying", "0").per_account(),
    same("lootUnderMouse", "1").per_account(),
    same("equipmentManager", "0").per_character(),
    // The currency tab's marks and the backpack's token bar (`0x51f01a`-`0x51f0d4`).
    same("currencyTokensUnused1", "0").per_character(),
    same("currencyTokensUnused2", "0").per_character(),
    same("currencyTokensBackpack1", "0").per_character(),
    same("currencyTokensBackpack2", "0").per_character(),
    same("showTokenFrame", "0").per_character(),
    same("showTokenFrameHonor", "0").per_character(),
    same("autoRangedCombat", "1").per_account(),
    // The camera's pitch follow speed, which the Mouse panel's Save writes beside the yaw one.
    same("cameraPitchSmoothSpeed", "45").per_account(),
    // The WowMouse switch (`0x5fd355`).
    same("enableWowMouse", "0"),
    // `locale` (`0x4068c6`): the boot writes the install's locale over the registered "****";
    // benilla is enUS-only. `useEnglishAudio` (`0x40690f`, "0").
    overridden(
        "locale",
        "enUS",
        "****",
        "the boot writes the running locale over it before the first frame, and benilla \
         reads enUS alone",
    ),
    same("useEnglishAudio", "0"),
    // The account data's sync switches: `synchronizeSettings` for all of it (`0x6b991b`), then
    // one per slot pair (`0x6b9997`, registered by every `AccountDataInitialize`), all "1".
    same("synchronizeSettings", "1"),
    same("synchronizeConfig", "1"),
    same("synchronizeBindings", "1"),
    same("synchronizeMacros", "1"),
    // The camera's pose, per character (`CameraRegisterCVars` `0x5fd942`, `0x5fd99a`): written at
    // the UI shutdown by `CGCamera::SaveCameraSettings` (`0x5ff320`), read back by `0x5ff3e0`.
    same("cameraSavedDistance", "5.55").per_character(),
    same("cameraSavedPitch", "10.0").per_character(),
    // The login screen's remembered account (`0x401ba7`, flags 0x40): hidden from Lua, which
    // reaches it through `GetSavedAccountName`/`SetSavedAccountName`.
    same("accountName", "").hidden(),
    // The rest of `CGGameUI::RegisterGameCVars` (`0x51d9b0`), `0x51da34`-`0x51f41b` in site order,
    // each at its registered string; flags 0x20 are per character. The stock interface reads and
    // writes them; the engine readers of `autoStand`, `autoDismount`, `autoUnshift`,
    // `unitHighlights` and `threatWorldText` have no counterpart here.
    same("autoStand", "1").per_account(),
    same("autoDismount", "1").per_account(),
    same("autoUnshift", "1").per_account(),
    same("alwaysCompareItems", "0").per_account(),
    same("minimapPortalMax", "99").per_character(),
    same("showQuestTrackingTooltips", "1").per_character(),
    same("questLogCollapseFilter", "0").per_character(),
    same("advancedWatchFrame", "0").per_account(),
    same("watchFrameIgnoreCursor", "0").per_account(),
    same("watchFrameBaseAlpha", "0").per_account(),
    same("watchFrameState", "0").per_account(),
    same("showQuestObjectivesOnMap", "1").per_character(),
    // `0x51e04f`, "" with flags 0x110: the completed tutorials in order, which
    // `CGTutorial::SaveFlaggedTutorials` (`0x530300`) writes and `LoadFlaggedTutorials`
    // (`0x5303a0`) reads; no host reader here.
    same("flaggedTutorials", "").per_account(),
    same("worldMapOpacity", "0").per_character(),
    same("guildShowOffline", "1").per_account(),
    same("lfgAutoFill", "0").per_account(),
    same("lfgAutoJoin", "0").per_account(),
    same("friendsViewButtons", "0").per_character(),
    same("friendsSmallView", "0").per_character(),
    same("fctAllSpellMechanics", "0").per_account(),
    // `0x51e7ac`, "0.5": `PartyFrame.xml`'s `VARIABLES_LOADED` arm sets the opacity slider from it.
    same("partyBackgroundOpacity", "0.5").per_account(),
    same("unitHighlights", "1").per_account(),
    same("enablePVPNotifyAFK", "1").per_account(),
    // `0x51ed69`: "%d" of 3, every trainer service shown.
    same("serviceTypeFilter", "3").per_account(),
    same("autojoinPartyVoice", "1").per_account(),
    same("autojoinBGVoice", "0").per_account(),
    same("combatLogOn", "1").per_character(),
    same("showBattlefieldMinimap", "0").per_character(),
    same("talentFrameShown", "0").per_account(),
    same("auctionDisplayOnCharacter", "0").per_account(),
    same("addFriendInfoShown", "0").per_account(),
    same("pendingInviteInfoShown", "0").per_account(),
    // `0x51ef6c`, `0x51ef8f`, `0x51efaf`: the clock's source and its alarm, minutes past midnight
    // and the message, which `TimeManagerFrame_OnLoad` reads (`Blizzard_TimeManager.lua:33-37`).
    same("timeMgrUseLocalTime", "0").per_account(),
    same("timeMgrAlarmTime", "0").per_account(),
    same("timeMgrAlarmMessage", "").per_account(),
    same("predictedHealth", "1").per_account(),
    same("predictedPower", "1").per_account(),
    same("threatWorldText", "1").per_account(),
    same("autoCompleteResortNamesOnRecency", "1").per_account(),
    same("autoCompleteWhenEditingFromCenter", "1").per_account(),
    same("autoCompleteUseContext", "1").per_account(),
    same("colorChatNamesByClass", "0").per_account(),
    same("miniWorldMap", "0").per_character(),
    same("dontShowEquipmentSetsOnItems", "0").per_account(),
    // The rest of `CameraRegisterCVars` (`0x5fd910`), `0x5fd964`-`0x5fe2a7`, all flags 0x10 but
    // the vehicle distance's 0x20. No host reader: the follow rig keeps its own model.
    same("cameraSavedVehicleDistance", "-1.0").per_character(),
    same("mouseInvertYaw", "0").per_account(),
    same("cameraDistanceMoveSpeed", "8.33").per_account(),
    same("cameraFoVSmoothSpeed", "0.5").per_account(),
    same("cameraDistanceSmoothSpeed", "8.33").per_account(),
    same("cameraHeightSmoothSpeed", "1.2").per_account(),
    same("cameraFlyingMountHeightSmoothSpeed", "2.0").per_account(),
    same("cameraViewBlendStyle", "1").per_account(),
    same("camerasmooth", "1").per_account(),
    same("cameraSmoothPitch", "1").per_account(),
    same("cameraSmoothYaw", "1").per_account(),
    same("cameraCustomViewSmoothing", "0").per_account(),
    same("cameraHeightIgnoreStandState", "0").per_account(),
    same("cameraDive", "1").per_account(),
    same("cameraSurfacePitch", "0.0").per_account(),
    same("cameraSubmergePitch", "18.0").per_account(),
    same("cameraSurfaceFinalPitch", "5.0").per_account(),
    same("cameraSubmergeFinalPitch", "5.0").per_account(),
    same("cameraPitchSmoothMin", "0.0").per_account(),
    same("cameraPitchSmoothMax", "30.0").per_account(),
    same("cameraYawSmoothMin", "0.0").per_account(),
    same("cameraYawSmoothMax", "0.0").per_account(),
    same("cameraSmoothTimeMin", "0.1").per_account(),
    same("cameraSmoothTimeMax", "2.0").per_account(),
    // `0x7f3a10`: "%d" of the locale's ceiling, 2 on enUS; the blood spurt reads it
    // (`creature_anim::blood`).
    same("violenceLevel", "2").per_account(),
    // `SI2::RegisterSoundEngineCVars` (`0x4d11cf`, "0"): the Voice panel's self-mute and its
    // binding's toggle (`VoiceChat.lua`).
    same("VoiceChatSelfMute", "0"),
];

/// The host-backed CVars, one row per knob that has a reader: a host knob, or a Lua consumer
/// such as the stock Video Options window's verbs.
///
/// Not registered for want of a reader, though pfUI's `hdgraphic` writes them: `lodDist`
/// (`0x688524`, "100.0", read at `0x6afb1d` for the doodad LOD swap), `footstepBias` (`0x6888b4`,
/// "0.125", read at `0x68fcb6`), `mapObjLightLOD` (`0x6886ec`, "0") and `SkyCloudLOD` (`0x6d1d33`,
/// "0"). `DistCull` (`0x688570`) has no reader in the reference either. `maxLOD` is no 1.12 CVar.
pub(crate) const REGISTERED: &[Registered] = &[
    // `realmName` (`0x83f2d0`): registered `""` (`0x882748`), help "Last realm connected to"
    // (`0x85d684`); the client builds its SavedVariables path from it (`0x5ab7d0`). Written from
    // the session's realm when addons load. `Ace/AceState.lua:27` trims it at
    // PLAYER_ENTERING_WORLD, so a nil breaks every Ace addon.
    same("realmName", ""),
    // The logon server address (register site `0x5ab6a6`), a string row judged by
    // `realmlist::on_cvar`.
    deviates(
        crate::realmlist::CVAR_REALMLIST,
        crate::realmlist::DEFAULT_REALMLIST,
        "us.logon.worldofwarcraft.com:3724",
        "that host has not resolved since 2019, so shipping it makes every first launch a \
         DNS failure; benilla dials the machine it is running on",
    ),
    // `autoClearAFK` (`0x5e24d4`, "1" `0x82e748`, handle `[0xc4d68c]` set at `0x5e24ef`, read at
    // `0x5eb84b`) gates five implicit AFK clears: any chat send but type `0x14`, Jump,
    // forward/back, strafe and turn (`0x513d36`/`0x514e23`/`0x514f0b`/`0x514fca`). Off, the clear
    // does nothing at all: no echo, no mirror write, no packet.
    same("autoClearAFK", "1").per_account(),
    // `0x4d18c1`, "0": the reference mutes on losing focus. The knob is
    // `SoundConfig::background_sound`.
    same("Sound_EnableSoundWhenGameIsInBG", "0"),
    // FMOD 3's mix-ahead buffer in ms (`0x4571ca`, flags 2: read once at sound init); here it sizes
    // the render thread's ring ahead of the IO callback (`sound::output`). `0x457520` registers
    // "50" or "100" by a host probe (`0x835e10`/`0x835e0c`), which one on a current machine
    // untraced; ours is the larger, since the depth has to hide a whole stalled IO cycle.
    same("SoundBufferSize", "100").latched(),
    // benilla's own, not a 1.12 CVar. Deviation: the reference clips at full scale and has no
    // headroom mechanism (its SFX-bus duck `0x457960` is a sidechain armed only by server-pushed
    // voice lines); benilla limits because every SFX is mastered to full scale and overlapping
    // kits clip.
    ours(
        "SoundOutputLimiter",
        "1",
        "benilla's own — the reference sums at full scale and clips; every WoW SFX is mastered \
         to full scale, so overlapping kits need a limiter to keep from distorting",
    ),
    // `uiScale` ("1.0", callback `0x518b60`) and `useUiScale` ("0", callback `0x5237e0`), both
    // registered at `0x5238a0`. With the switch on, the game UI's parent takes `uiScale`, floored
    // at 0.64 and capped at `aspect × 0.75` under 4:3 (`CGGameUI::SetUIScale 0x513240`); off, it
    // takes `clamp(768 / height, 0.9, 1.0)` with the same cap (`ScaleToScreenSize 0x51fb80`). The
    // knob is [`crate::ui_script::UiScaleCvar`].
    same("uiScale", "1.0"),
    same("useUiScale", "0"),
    // `farclip` (`CWorldParam::Initialize 0x78e400`, "350", callback `0x78d780`), validated per
    // map by `World::ValidateFarClip 0x780770` ([`benilla_world::view::validate_farclip`]).
    overridden(
        "farclip",
        "777",
        "350",
        "the first-run defaults pass (`CWorldParam::SetDefaults 0x78e1a0`) writes \
         `s_farClip[2 × idx + cpu]` (`0xadbf00`) of the matched `VideoHardware.dbc` row, and the \
         fallback row a modern GPU reaches (693 on the reference install's `Logs/gx.log`, index 4) \
         on a multi-core CPU gives 777",
    ),
    // `nearclip` (`0x68867a`: name `0x84ffb0`, default `0x84fb48` "0.1", flags 1, callback
    // `0x688d90`, record `[0xc7f348]`). The camera re-reads the record every frame: `0x511bc0`
    // (sole caller `0x483094`) stamps `[cam+0x38]` from it through the handle `[0xbe1078]` that
    // `0x50b728` caches, overwriting the ctor's 1/9 (`0x3de38e39`);
    // `benilla_world::view::stamp_near_clip` is that.
    // The callback's derived global `[0xc7b480]` has no reader. pfUI's `hdgraphic` writes
    // 0.06..0.30, inside `[0.01, 0.33]`.
    same("nearclip", "0.1"),
    // `deselectOnClick` and `mouseInvertPitch` are 1.12's own (`UIOptionsFrame.lua:8,4`);
    // `autoLootDefault` is the later-era name, 1.12 having only the shift gesture.
    same("deselectOnClick", "1").per_account(),
    // `AutoInteract` (`0x603390`, record `[0xc4d9a4]`), Click-to-Move (`UIOptionsFrame.lua:7`):
    // "1" on koKR alone (`0x603368`). The knob is [`crate::player::Approach`].
    same("AutoInteract", "0").per_account(),
    // `BlockTrades` (`0x842fbc`), `UIOptionsFrame.lua:11`: the refusal leg `0x4bf7bc` fires only
    // when it is set. The knob is [`crate::ui_trade::BlockTrades`].
    same("BlockTrades", "0").per_character(),
    // `autoSelfCast` (register site `0x51dbee`, name `0xa025e4`, default `0x9e1464` "1"): a
    // friendly cast that binds nothing falls back to the caster. `TOGGLEAUTOSELFCAST` toggles it.
    same("autoSelfCast", "1").per_character(),
    // `ShowAllSpellRanks` (register site `0x51f1e6`, name `0x9ffea0`, default `0x9e1464` "1"):
    // `SpellBook_GetTabInfo` pages the full book while set, the highest-rank view while clear.
    same("ShowAllSpellRanks", "1").per_account(),
    // `autoFilledMultiCastSlots` (`0x51f38b`, default `0x9e14a0` "0"): elements the bar seeded.
    same(crate::spell::multicast::CVAR_AUTO_FILLED, "0"),
    // The five saved camera views and the live index, at the reference's names and default strings;
    // owned by [`crate::player::camera_view`]. Registered so a `SaveView` persists.
    same(
        crate::player::camera_view::CVAR_ACTIVE_VIEW,
        crate::player::camera_view::ACTIVE_VIEW_DEFAULT,
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[0][0],
        crate::player::camera_view::VIEW_DEFAULTS[0][0],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[0][1],
        crate::player::camera_view::VIEW_DEFAULTS[0][1],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[0][2],
        crate::player::camera_view::VIEW_DEFAULTS[0][2],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[1][0],
        crate::player::camera_view::VIEW_DEFAULTS[1][0],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[1][1],
        crate::player::camera_view::VIEW_DEFAULTS[1][1],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[1][2],
        crate::player::camera_view::VIEW_DEFAULTS[1][2],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[2][0],
        crate::player::camera_view::VIEW_DEFAULTS[2][0],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[2][1],
        crate::player::camera_view::VIEW_DEFAULTS[2][1],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[2][2],
        crate::player::camera_view::VIEW_DEFAULTS[2][2],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[3][0],
        crate::player::camera_view::VIEW_DEFAULTS[3][0],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[3][1],
        crate::player::camera_view::VIEW_DEFAULTS[3][1],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[3][2],
        crate::player::camera_view::VIEW_DEFAULTS[3][2],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[4][0],
        crate::player::camera_view::VIEW_DEFAULTS[4][0],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[4][1],
        crate::player::camera_view::VIEW_DEFAULTS[4][1],
    ),
    same(
        crate::player::camera_view::VIEW_CVARS[4][2],
        crate::player::camera_view::VIEW_DEFAULTS[4][2],
    ),
    same("mouseInvertPitch", "0").per_account(),
    // `autoLootDefault` (`0x51db43`, "0"): a loot opens without the auto-loot modifier's sweep.
    same("autoLootDefault", "0").per_account(),
    // The overhead-name gates, registered at `0x6c7470` into mask `0xce8720`: `UnitNamePlayer`
    // (`0x86c694`) "1" (`0x82e748`), `UnitNameNPC` (`0x86c6a4`) and `UnitNameOwn` (`0x86c6b0`) "0"
    // (`0x82e570`).
    same("UnitNamePlayer", "1"),
    same("UnitNameNPC", "0").per_account(),
    same("UnitNameOwn", "0").per_account(),
    // `UnitNamePlayerGuild` (`0x86c680`, "1", mask bit `0x10`) is not a show gate: `ShouldShowName`
    // (`0x6070a0`) reads bits `0x1/0x2/0x4`, and this gates the `"\n<%s>"` guild line at
    // `0x609085`.
    same("UnitNamePlayerGuild", "1").per_account(),
    // `UnitNamePlayerPVPTitle` (`0x86c668`, "1", mask bit `0x20`): the a4 rank prefix on the main
    // name line (`0x609370`), `PVP_RANK_%d_%d` through `0x5efe60`.
    same("UnitNamePlayerPVPTitle", "1").per_account(),
    // ── Floating combat text ─────────────────────────────────────────────────────────────────────
    //
    // `CombatDamage` (`0x51e3ee`, `[0xbd0980]`) is the master: the word and number emitters
    // (`0x71f990`, `0x722340`) return at "0", so nothing floats. The `Pet*` rows gate only the
    // owned-by-you branch.
    same("CombatDamage", "1").per_account(),
    same("PetMeleeDamage", "1").per_account(),
    same("PetSpellDamage", "1").per_account(),
    // `CombatLogPeriodicSpells` (`0x51e411`, `[0xbd0984]`): a periodic tick's floating word or
    // number.
    same("CombatLogPeriodicSpells", "1").per_account(),
    // `combatLogRetentionTime` (`0x51efed`): seconds the combat log keeps an entry (`0x750400`).
    same(benilla_ui::script::RETENTION_CVAR, "300"),
    // `assistAttack` (`0x48fc50`, record `[0xb4d8f8]`): `/assist` also starts the swing. The "3"
    // beside it is the next registration's, `minimapZoom`: stock `/assist` selects without
    // swinging.
    same("assistAttack", "0").per_character(),
    // ── Mouse-look speed, per axis ───────────────────────────────────────────────────────────────
    //
    // `cameraYawMoveSpeed` is the MOUSE_LOOK_SPEED slider (`UIOptionsFrame.lua:89`); a nil there
    // raises in `slider:SetValue` (`0x790980`) and stops `UIOptionsFrame_Load`. The stock Save
    // writes `cameraPitchMoveSpeed` as half of it (`:355-356`). The reference integrates
    // OS-accelerated pixels where we take raw device deltas, so the unit factor lives in
    // `camera::LOOK_YAW_PER_SPEED` and these defaults stay the reference's. The validator
    // `0x50c000` → `0x50b330` rejects values outside [0.1, 360] rather than clamping, and
    // `player::camera::on_cvar` does the same.
    same("cameraYawMoveSpeed", "180").per_account(),
    same("cameraPitchMoveSpeed", "90").per_account(),
    // MOUSE_SENSITIVITY (`UIOptionsFrame.lua:87`): FrameXML's spelling of the binary's `mouseSpeed`
    // (`0x402c7b`); lookups are case-insensitive (`CVar::Lookup 0x63de30`), here as there. The
    // reference's default is `SPI_GETMOUSESPEED × 0.1`, "1.0" on stock Windows, and its record
    // `[0x882704]` has no reader: the slider sets the OS pointer speed (`0x402ec0`, [0.1, 2.0]).
    // The default matches; Deviation: here the dial multiplies the camera's own rate, because
    // benilla does not change the OS pointer speed, a system-wide setting.
    same("mousespeed", "1"),
    // The zoom-out limit is `cameraDistanceMax × cameraDistanceMaxFactor`, held to [0, 50]
    // (`0x5112d6`). `cameraDistanceMax` (`0x50beb2`, default `0x84fbd0`) has no panel row; its
    // validator `0x50b310` refuses a value outside [0, 50], and `player::camera::on_cvar` does the
    // same. The factor is MAX_FOLLOW_DIST (`UIOptionsFrame.lua:90`), registered "1.0" (`0x82e92c`)
    // with no validator: the slider's 1 to 2 is the slider's, not the CVar's.
    same("cameraDistanceMax", "15.0").per_account(),
    same("cameraDistanceMaxFactor", "1.0").per_account(),
    // `cameraSmoothStyle` (`0x50ba92`, default `[0x84f4f4]` "1"), the auto-return behind the
    // character. The engine's enum is 0 Never, 1 Smart, 2 Always, as the stock dropdown writes it
    // (`UIOptionsFrame.lua:525,536,547`); 3, the Never entry's position, is the validator's upper
    // bound (`0x50b330(v, 0, 3)`). See `FollowStyle`.
    same("cameraSmoothStyle", "1").per_account(),
    // Read instead of `cameraSmoothStyle` while the state mask holds Track or Fear; no panel row.
    same("cameraSmoothTrackingStyle", "1").per_account(),
    // AUTO_FOLLOW_SPEED (`UIOptionsFrame.lua:88`), deg/s, registered "180.0" (`[0xbe1070]`): it
    // sets the transition's duration (`|dyaw| / rate * factor`), an average rate, not a slew. The
    // knob clamps it to `FOLLOW_SPEED_RANGE`. The stock Save also writes `cameraPitchSmoothSpeed`
    // at a quarter (`:353`), unregistered here: `FollowRig` has one rate.
    same("cameraYawSmoothSpeed", "180").per_account(),
    // `cameraPivot` `[0xbe10a4]` "1" (`0x50bda3`), smart pivot: gate `0x510690`, routing
    // `0x50fee0`, release `0x5107f0`; ours is `player::camera_dynamics::SmartPivot`.
    same("cameraPivot", "1").per_account(),
    // Read by the routing (`0x50fff5`/`0x510004`) in radians of camera rotation, so they carry to
    // benilla's raw-device units unchanged.
    same("cameraPivotDXMax", "0.05").per_account(),
    same("cameraPivotDYMin", "0").per_account(),
    // The pitch bias's ease-back once the pivot lets go, deg/s (`[0xbe0fc8]`; `0x512a50` divides
    // |Δ| by rate · π/180 for the duration); no panel row.
    same("cameraTargetSmoothSpeed", "90").per_account(),
    // `cameraWaterCollision` `[0xbe1088]` "1" (`0x50bd63`, `0x82e748`): one register, two consumers
    // that must ship together. `0x50e5ec` builds it; its `0xf0000` nibble joins the trace mask of
    // all three `0x50e570` queries (reaching `0x69cc13`), and `0x50e629` tests it to lift the sweep
    // origin to `surface + 2/9`. Ours: `benilla_world::collision::camera_filter` and
    // `player::camera_water`.
    same("cameraWaterCollision", "1").per_account(),
    // `cameraTerrainTilt` `[0xbe0fd4]` "0" (`0x50bcfd`), Follow Terrain: probe and staircase
    // `0x50d900`, arm `0x50dbc0`; ours is `player::camera_dynamics::TerrainTilt`.
    same("cameraTerrainTilt", "0").per_account(),
    // Rate in deg/s (`[0xbe0fc0]`), duration bounds in seconds (`[0xbe1050]`/`[0xbe1054]`). The 3 s
    // floor always binds (20° at 7.5°/s is 2.67 s), so the camera leans rather than tracks.
    same("cameraGroundSmoothSpeed", "7.5").per_account(),
    same("cameraTerrainTiltTimeMin", "3").per_account(),
    same("cameraTerrainTiltTimeMax", "10").per_account(),
    // `cameraBobbing` `[0xbe10c0]` "0" (`0x50b76d`), head bob: kernel `0x511920`, gate `0x5105e0`;
    // ours is `player::camera_dynamics::HeadBob`.
    same("cameraBobbing", "0"),
    // Amplitudes in the CVar's units, scaled by 1/36 (`[0x7ff9d0]`) to yards.
    // `cameraBobbingSmoothSpeed` is the decay rate, read only in the disarm `0x51113a`, which
    // divides the largest component by it for the ramp's duration (~0.069 s at these defaults).
    same("cameraBobbingLRAmplitude", "2"),
    same("cameraBobbingUDAmplitude", "2"),
    same("cameraBobbingFrequency", "0.8"),
    same("cameraBobbingSmoothSpeed", "0.8").per_account(),
    // `statusBarText` (`0x48fc34`, record `[0xb4d904]`, no engine reader), read by
    // `TextStatusBar.lua:47,97`: "0" shows the numbers on hover only.
    same("statusBarText", "0"),
    // Enhanced Tooltips (`UIOptionsFrame.lua:15`), registered "1" at `0x48fddd` (`0x82e748`); read
    // only by the stock interface.
    same("UberTooltips", "1").per_account(),
    // `chatBubbles` (`0x51e16a`) and `chatBubblesParty` (`0x51e192`), both "1".
    same("ChatBubbles", "1").per_account(),
    same("ChatBubblesParty", "1").per_account(),
    // Registered "1" (`0x82e748`), category 4. `profanityFilter` (`0x402e68`, name `0x82e7f4`,
    // callback `0x403570`) masks `ChatProfanity.dbc` spans inside the shared masker `0x4a1a66`,
    // covering all thirteen call sites; `spamFilter` (`0x402e8e`, name `0x82e7d4`, callback
    // `0x4035b0`) silently drops a line matching `SpamMessages.dbc`. The knob is
    // [`crate::text_filter::TextFilterSwitches`].
    same("profanityFilter", "1").per_account(),
    same("spamFilter", "1").per_account(),
    // Registered by `CGlueMgr::EnterWorld` (`0x46b633` `gameTip` "0", `0x46b658` `showGameTips`
    // "1"), category 5. `gameTip` is the cursor and holds the next tip, not the one on screen;
    // `crate::game_tip` advances it.
    same("gameTip", "0"),
    same("showGameTips", "1"),
    // `showLootSpam` (`0x48fd1c`, name `0x8430a0`, "1" `0x82e748`, record `0xb4e2bc`): off, group
    // loot-roll lines are hidden and only the winner shows. Its three readers are the roll-line
    // composers; the knob is [`crate::ui_loot::LootConfig::show_loot_spam`].
    same("showLootSpam", "1").per_account(),
    // `guildMemberNotify` (`0x5e24c7`, "0" `0x82e570`, record `0xc4d3c4`): guildmate log on/off
    // lines, read only in `SMSG_GUILD_EVENT`'s handler. The knob is
    // [`crate::ui_guild::GuildMemberNotify`].
    same("guildMemberNotify", "0").per_account(),
    // Both registered "3" (`0x48fc6c`, `0x48fc88`); the minimap's +/- buttons write them through
    // `Minimap:SetZoom`. The knob is [`crate::minimap::MinimapZoom`].
    same("minimapZoom", "3").per_character(),
    same("minimapInsideZoom", "3").per_character(),
    // Load out of date AddOns, inverted (`0x402c3b`): "1" enforces the check. Read by the load walk
    // ([`Cvars::addon_version_check`]) and live by the VM's gate.
    same("checkAddonVersion", "1"),
    // `gxApi` (`0x63a833`: name `0x842a64`, default `0x864f7c` "direct3d", flags 3, callback
    // `0x63b030`, record `[0xc4ea94]`): the reference builds D3D9 unless it reads "OpenGl"
    // (`0x63a3c4`, `0x842a5c`). Here it reports the wgpu backend (`wgpu::Backend::to_str`), pushed
    // from `RenderAdapterInfo` and owned by the session, so it is never persisted. pfUI's
    // `panel.lua:185` concatenates it.
    deviates(
        "gxApi",
        "",
        "direct3d",
        "descriptive, not a selector — benilla renders through wgpu, which has no D3D9 \
         backend and no chooser; the value is the live adapter's own and is never persisted",
    )
    .latched(),
    // `gxVSync` (`0x63a859`, "1", flags 3), `OptionsFrame.lua:9`. The knob is
    // [`crate::video::VideoConfig::vsync`], which the window's present mode follows at the
    // `RestartGx` commit. `$WOW_NOVSYNC=1` overrides it for the session.
    same("gxVSync", "1").latched(),
    // `gxWindow` (`RegisterGxCVars 0x76a630`, callback `0x769550`): "0" on enUS, "1" on zhCN, as
    // `gxMaximize`. "0" mode-sets the display to `gxResolution` at `gxRefresh`; where the platform
    // has no mode-set the window is borderless over the monitor instead ([`crate::video`]). The
    // knob is [`crate::video::VideoConfig::display`].
    same("gxWindow", "0").latched(),
    // `gxResolution` (`0x76a630`, the hardware row's format size, callback `0x76a220`): the
    // fullscreen mode and the window's size, parsed by `video::on_cvar`. "" is benilla's "not
    // picked": fullscreen keeps the desktop's own mode and a window opens at
    // [`crate::video::DEFAULT_WINDOWED`].
    deviates(
        "gxResolution",
        "",
        "1024x768",
        "a fresh reference install mode-sets the display to its hardware row's format \
         (`s_formats[resIdx]` at `0xcabe98`, index 4 on the fallback rows), which on any current \
         monitor is a blurry 4:3 mode switch; until the player picks a size, ours keeps the \
         desktop's own mode",
    )
    .latched(),
    // benilla's own: a body pane's doll renders at half the frame rate while the pane is open; the
    // reference draws it in the main pass. The knob is [`crate::portrait::PaneRate`].
    ours(
        "boothHalfRate",
        "1",
        "benilla's own — the reference draws its doll inside the main pass and has no \
         second view to rate-limit",
    ),
    // `gxMultisample` (`0x63a950`, flags 3). The reference formats its default from field 21 of the
    // `VideoHardware.dbc` row `DetectHardware` (`0x641260`) matches, which is 1, no multisampling,
    // on every fallback row a modern GPU reaches. The knob is [`benilla_world::view::MsaaSetting`],
    // read once at the camera's spawn; `$WOW_MSAA` overrides it for the session.
    same("gxMultisample", "1").latched(),
    // `GetCurrentMultisampleFormat 0x48c580` looks up all three of the Video dropdown's values by
    // name, so these must exist. They describe the swapchain's own pair and steer nothing;
    // `SetMultisampleFormat` writes them as `0x48c640` does.
    deviates(
        "gxColorBits",
        "32",
        "16",
        "these describe, they do not steer — the pair is our swapchain's own, and every \
         format `MsaaFormats` publishes carries it",
    )
    .latched(),
    deviates(
        "gxDepthBits",
        "32",
        "16",
        "as `gxColorBits` — the depth half of the same descriptive pair",
    )
    .latched(),
    // The two Effects rows behind `Get/SetTerrainMip` and the stock slider's `BaseMip`
    // (`CWorldParam::Initialize 0x78e400`), and `texLodBias` beside them.
    //
    // `shadowLevel` ("1", callback `0x78d6f0`: refused above 1, "Shadow mip level changed upon
    // restart."), behind `Get/SetTerrainMip` as `1 − level` (`0x54eb10`/`0x54eb40`): read once into
    // `CWorld::shadowMipLevel` at world init, it allocates every terrain chunk's layer textures one
    // mip down (`CMapChunk::RenderPrep 0x7d3fe0` sets flag `0x10`, which `0x7ba050` reads), where
    // otherwise only chunks past 777 yd take it. Applied live here, the stock logout prompt dropped
    // (`FrameXMLFixes.xml`); the knob is `benilla_world::terrain_stream::TerrainOptions::low_mip`.
    same("shadowLevel", "1"),
    // `texLodBias` ("0.0", callback `0x78d730`: refused outside [-1.0, 1.0]), behind
    // `Get/SetTexLodBias`; the stock UI binds no control to it.
    same("texLodBias", "0.0"),
    // `baseMip` ("0", callback `0x78df00`: refused above 1): the device's first uploaded mip, so a
    // 1 halves every texture, applied live (`TextureNotifyGxRestart` reloads them). The knob is
    // `benilla_assets::TexFilterSetting::base_mip`.
    same("baseMip", "0"),
    // `spellEffectLevel` (`0x78e400`, "9", flags 1, no callback): `SpellVisualsInitialize`
    // (`0x7fc5a0`) caches the record and each blizzard shard emitter scales its rate by
    // `level / 9 × 0.9 + 0.1` at spawn (`0x7fb5e9`). The stock UI binds no control to it.
    same("spellEffectLevel", "9"),
    // Weather Intensity (`OptionsFrame.lua:33`), registered "2" (`0x67b806`, flags 0, callback
    // `0x67b870`, name `0x8685ac`). The reader is `benilla_world::weather::WeatherState`, which
    // scales the precipitation spawn rate by `0x67b870`'s table {0.1, 0.33, 0.66, 1.0}; rendering
    // only.
    deviates(
        "weatherDensity",
        "3",
        "2",
        "every precipitation rate in `benilla-world`'s own precipitation module was \
         derived and graded against the reference install's own apitrace captures, and that \
         install runs \
         `SET weatherDensity \"3\"` (K = 1.0) — so 3 is the value a benilla-vs-reference \
         side-by-side is correct at, and the registered 2 would thin every rate to 0.66 against \
         the only client we compare with. The slider is how a player takes it back down",
    ),
    // `Gamma` ("1.0", callback `0x401a30`) and `DesktopGamma` ("0", callback `0x401a80`), both
    // registered at `0x401dad`. The ramp `pow(i/255, gamma)` is uploaded only while
    // `DesktopGamma` reads 0, and the device skips it while windowed
    // (`CGxDeviceD3d::DeviceSetGamma`); `DesktopGamma` 1 puts the desktop's own ramp back. The
    // knob is [`crate::ui_gamma::DisplayGamma`], the same curve in the composite pass. Spelled
    // "1.000000" because `SetGamma 0x54ea90` formats with `"%f"`, and Restore Defaults must
    // compare equal to the default; [`sync_cvars`] seeds it the same way.
    same("gamma", "1.000000"),
    same("DesktopGamma", "0"),
    // ── The stock Video Options window's other boxes (`OptionsFrame.lua:5-22`) ───────────────────
    //
    // `ffx` (`0x6ccfe3`: name `0x86f2ac`, record `[0xce8a04]`), `ffxGlow` (`0x6cc17d`, name
    // `0x86f200`) and `ffxDeath` (`0x6cc6d1`, name `0x86f238`): each "1", flags 1, with the echo stub
    // `0x6cde30` as callback, and each record's integer read every frame. The FFX begin/end pair
    // (`0x6cd890`/`0x6cda70`) runs the active pass only while `ffx` (`0x6cd8a6`), `pixelShaders`
    // (`0x6cd8b6`) and that pass's own enable hold: `CFFXGlow` (`0x6cc5a0`) reads `ffxGlow` at
    // `0x6cc5a8`, `CFFXDeath` (`0x6cded0`) reads `ffxDeath` at `0x6cdf10`. The stock Okay writes
    // `ffx` as Enable All Shaders (`OptionsFrame.lua:202-204`). The knob is
    // [`benilla_world::ffx_glow::FfxSwitches`].
    same("ffx", "1"),
    same("ffxGlow", "1"),
    same("ffxDeath", "1"),
    // `specular` (`CWorldParam::Initialize 0x78e400`, "0", callback `0x78de60`: "Specular
    // disabled on restart."), Terrain Highlights: `CWorld::enables` bit `0x8000000`, read into
    // `CMap::enableSpecular` at each map load (`CMap::LoadWdt` → `0x7bd8a0`), hence the stock
    // row's logout requirement. The knob is `benilla_world::terrain_stream::TerrainSpecular`.
    overridden(
        "specular",
        "1",
        "0",
        "the first-run defaults pass (`CWorldParam::SetDefaults 0x78e1a0`) writes the matched          `VideoHardware.dbc` row's specularity bit, which the fallback row a modern GPU reaches          (693 on the reference install's `Logs/gx.log`) sets",
    ),
    // `lod` (`0x68848c`: name `0x8697d0`, "1", flags 1, callback `0x688920`, record `[0xc7f314]`),
    // World LOD: bit 2 of `[0xc7b2a4]`, which gates the doodad LOD distance test at `0x6afb11`. No
    // effect here: benilla has no doodad LOD swap, as `lodDist` above has no row.
    same("lod", "1"),
    // `movieSubtitle` (`0x402c18`: name `0x82e9d8`, "0", flags 0, record `[0x882640]`), Cinematic
    // Subtitles: the pre-rendered movies' subtitles, read by the glue's `GetMovieSubtitles`
    // (`0x46dbc0`). No effect here: benilla plays no pre-rendered movie.
    same("movieSubtitle", "0"),
    // `useWeatherShaders` (`0x67b81d`: name `0x868598`, "1", flags 0, no callback, record
    // `[0xc7b588]`), Weather Shaders: read in each precipitation effect's constructor (`0x67480f`,
    // `0x6775a7`, `0x679274`), which clears the effect's shader flag at 0. No effect here: rain's
    // two legs are one draw with other scalars, but snow's fixed-function leg (`0x678960`) is a
    // CPU-built triangle per flake where the shader leg (`0x678610`) is a point sprite, a second
    // draw path `benilla_world::weather` does not have, so the shader leg always runs.
    same("useWeatherShaders", "1"),
    // The latched `gx*` rows the stock window's boxes and refresh dropdown write (flags 3, committed
    // by `RestartGx`), each callback storing one field of the device-format record `0xc4eb98`.
    //
    // `gxMaximize` (`0x63a8e0`: name `0x864edc`, "0", "1" on zhCN like `gxWindow`, callback
    // `0x63b1f0` → `+0x09` `[0xc4eba1]`): the window rebuild `0x58cf10` makes a windowed, maximized
    // window a borderless popup (style `0x90000000`) at the screen's full size (`GetSystemMetrics`
    // 0 and 1). The knob is [`crate::video::VideoConfig::maximize`].
    same("gxMaximize", "0").latched(),
    // `gxTripleBuffer` ("0", callback `0x769360`: refused above 1): a second back buffer, used
    // only with vsync (`ISetPresentParms 0x68e250`). The knob is
    // [`crate::video::VideoConfig::triple_buffer`], one more swapchain frame in flight.
    same("gxTripleBuffer", "0").latched(),
    // `gxCursor` ("1", callback `0x7695e0`): the hardware cursor, else the device draws the cursor
    // texture over each presented frame (`CGxDevice::ICursorDraw 0x687a90`). The knob is
    // [`crate::video::VideoConfig::hw_cursor`], which `crate::cursor` follows.
    same("gxCursor", "1").latched(),
    // `gxAspect` ("1", callback `0x769580`): a windowed window wider or taller than the screen
    // shrinks keeping its aspect (`AdjustWindowRectToWorkArea 0x684d70`). The knob is
    // [`crate::video::VideoConfig::aspect`].
    same("gxAspect", "1").latched(),
    // `gxFixLag` (callback `0x7696a0`), Reduce Input Lag: after each present the device waits for
    // the GPU to drain (`CGxDeviceD3d::ScenePresent 0x6a3450`). The knob is
    // [`crate::video::VideoConfig::fix_lag`], a swapchain of one frame in flight.
    deviates(
        "gxFixLag",
        "0",
        "1",
        "the reference formats its default from `VideoHardware.dbc` field 20 of the matched row, \
         1 on every fallback row, but a frame latency of one ends the CPU/GPU overlap and costs \
         frame time on every machine we measure; the box is how a player trades it for latency",
    )
    .latched(),
    // `gxRefresh` ("75", callback `0x76a580`: refused unless a monitor mode offers it), the
    // fullscreen mode's rate. The knob is [`crate::video::VideoConfig::refresh`].
    same("gxRefresh", "75").latched(),
    // benilla's own: the world renders at `window × this` while the UI stays native. The knob is
    // [`crate::world_backdrop::RenderScale`], clamped to `RENDER_SCALE_RANGE`; at "1" nothing is
    // resampled. `$WOW_RENDER_SCALE` overrides it for the session.
    ours(
        "renderScale",
        "1",
        "benilla's own — the reference has no off-screen buffer to hang a resolution dial \
         on; its nearest equivalent, `gxResolution`, drops the interface with the world",
    ),
    // benilla's own: `/console fpsJournal 1` appends a per-second row of position, frame cost and
    // per-pass GPU time to `benilla-config/Diagnostics/fps-journal.csv`. The knob is
    // [`crate::perf::FpsJournalSetting`].
    ours(
        "fpsJournal",
        "0",
        "benilla's own — 1.12 has no player-side perf log; its nearest thing is the \
         Ctrl+R framerate label, a number with no file behind it",
    ),
    // `lastCharacterIndex` (`0x402d93`, "0" `0x82e570`, category 4, handle `[0x882674]`), help
    // "Last character selected": a 0-based row (the selection cell `[0x83856c]` under `"%d"`), so
    // "0" is the first character. It mirrors [`crate::char_select::Roster::pending_index`].
    same(crate::char_select::CVAR_LAST_CHARACTER, "0"),
    // The four user agreements the login screen walks (`0x402db6`, `0x402dd9`, `0x402dfc`,
    // `0x402e1f`; "0" `0x82e570`, category 4): "Status of the TOS" (`0x82e8e4`), "... the EULA"
    // (`0x82e8c8`), "... the Scanning notice" (`0x82e89c`) and "... the Contest notice"
    // (`0x82e86c`). The glue manager reads them once at startup (`0x46a400`) as accepted (1) and
    // changed (negative), and its `Accept*` writes "1" (`0x46aef0` and its siblings).
    same("readTOS", "0"),
    same("readEULA", "0"),
    same("readScanning", "0"),
    same("readContest", "0"),
    // The screenshot writer's (`ClientRegisterConsoleCommands`, `0x401ee4` "jpeg", `0x401f0c` "3",
    // flags 1): `tga` in any case writes a Targa, anything else a JPEG, at quality 1..10
    // ([`crate::screenshot`]).
    same("screenshotFormat", "jpeg"),
    same("screenshotQuality", "3"),
];
