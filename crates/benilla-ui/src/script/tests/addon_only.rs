//! The addon-only verbs still unbuilt (#399): engine globals 12340 registers that no stock
//! FrameXML or GlueXML file calls, so no play session notices one missing. The list is exact and
//! only shrinks: building one fails this gate until its name leaves the list.

use std::collections::HashSet;

use super::reference_surface::engine_globals;

/// Engine globals the 3.3.5a client has and benilla's VM does not, none called by the stock
/// interface. Byte-sorted.
const ADDON_ONLY_UNBUILT: &[&str] = &[
    "AppendToFile",
    "ApplyBarberShopStyle",
    "BNIsBlocked",
    "BNReportFriendInvite",
    "BNSetFocus",
    "CalendarContextDeselectEvent",
    "CalendarContextInviteIsPending",
    "CalendarContextInviteModeratorStatus",
    "CalendarContextInviteStatus",
    "CalendarContextInviteType",
    "CalendarEventCanModerate",
    "CalendarEventIsModerator",
    "CalendarEventSetLockoutDate",
    "CalendarEventSetLockoutTime",
    "CalendarEventSetSize",
    "CalendarGetAbsMonth",
    "CalendarGetDayEventSequenceInfo",
    "CalendarGetMaxDate",
    "CalendarGetMinHistoryDate",
    "CalendarGetMonthNames",
    "CalendarGetWeekdayNames",
    "CanMapChangeDifficulty",
    "CanSwitchVehicleSeats",
    "ChangePlayerDifficulty",
    "ChannelSilenceAll",
    "ChannelUnSilenceAll",
    "ChannelVoiceOff",
    "ChannelVoiceOn",
    "ClearChannelWatch",
    "CommentatorAddPlayer",
    "CommentatorEnterInstance",
    "CommentatorExitInstance",
    "CommentatorFollowPlayer",
    "CommentatorGetCamera",
    "CommentatorGetCurrentMapID",
    "CommentatorGetInstanceInfo",
    "CommentatorGetMapInfo",
    "CommentatorGetMode",
    "CommentatorGetNumMaps",
    "CommentatorGetNumPlayers",
    "CommentatorGetPlayerInfo",
    "CommentatorGetSkirmishMode",
    "CommentatorGetSkirmishQueueCount",
    "CommentatorGetSkirmishQueuePlayerInfo",
    "CommentatorLookatPlayer",
    "CommentatorRemovePlayer",
    "CommentatorRequestSkirmishMode",
    "CommentatorRequestSkirmishQueueData",
    "CommentatorSetBattlemaster",
    "CommentatorSetCamera",
    "CommentatorSetCameraCollision",
    "CommentatorSetMapAndInstanceIndex",
    "CommentatorSetMode",
    "CommentatorSetMoveSpeed",
    "CommentatorSetPlayerIndex",
    "CommentatorSetSkirmishMatchmakingMode",
    "CommentatorSetTargetHeightOffset",
    "CommentatorStartInstance",
    "CommentatorStartSkirmishMatch",
    "CommentatorToggleMode",
    "CommentatorUpdateMapInfo",
    "CommentatorUpdatePlayerInfo",
    "CommentatorZoomIn",
    "CommentatorZoomOut",
    "CursorHasMacro",
    "DeclineName",
    "DeleteFile",
    "DisplayChannelVoiceOff",
    "DisplayChannelVoiceOn",
    "DownloadSettings",
    "EquipmentManagerIsSlotIgnoredForSave",
    "FindSpellBookSlotByID",
    "GMRequestPlayerInfo",
    "GMSurveyNumAnswers",
    "GetActiveLevel",
    "GetAddOnCPUUsage",
    "GetAllowLowLevelRaid",
    "GetAvailableLevel",
    "GetCVarInfo",
    "GetCoinTextureString",
    "GetCurrentKeyBoardFocus",
    "GetEquipmentSetLocations",
    "GetEventCPUUsage",
    "GetExtendedItemInfo",
    "GetFacialHairCustomization",
    "GetFrameCPUUsage",
    "GetFramesRegisteredForEvent",
    "GetFunctionCPUUsage",
    "GetInstanceDifficulty",
    "GetItemCooldown",
    "GetItemCount",
    "GetItemFamily",
    "GetItemGem",
    "GetItemIcon",
    "GetItemStatDelta",
    "GetItemStats",
    "GetItemUniqueness",
    "GetMacroBody",
    "GetMacroItem",
    "GetMacroSpell",
    "GetMaxArenaCurrency",
    "GetMinigameState",
    "GetMinigameType",
    "GetMouseButtonClicked",
    "GetMouseButtonName",
    "GetMovieResolution",
    "GetNumDeclensionSets",
    "GetNumGlyphSockets",
    "GetPlayerFacing",
    "GetPowerRegen",
    "GetQuestsCompleted",
    "GetReadyCheckTimeLeft",
    "GetRuneCount",
    "GetRunningMacro",
    "GetRunningMacroButton",
    "GetScriptCPUUsage",
    "GetSpellCount",
    "GetTaxiBenchmarkMode",
    "GetTrainerServiceStepIncrease",
    "GetTrainerSkillLineFilter",
    "GetTrainerSkillLines",
    "GetUnitPitch",
    "GetUnitSpeed",
    "InteractUnit",
    "IsAttackSpell",
    "IsAutoRepeatSpell",
    "IsAvailableQuestTrivial",
    "IsBNLogin",
    "IsConsumableItem",
    "IsConsumableSpell",
    "IsCurrentItem",
    "IsDebugBuild",
    "IsDesaturateSupported",
    "IsEquippedItemType",
    "IsFlyableArea",
    "IsFlying",
    "IsHarmfulItem",
    "IsHarmfulSpell",
    "IsHelpfulItem",
    "IsHelpfulSpell",
    "IsIgnored",
    "IsIgnoredOrMuted",
    "IsIndoors",
    "IsItemInRange",
    "IsLeftAltKeyDown",
    "IsLeftControlKeyDown",
    "IsLeftShiftKeyDown",
    "IsLoggedIn",
    "IsMounted",
    "IsOutdoors",
    "IsRealPartyLeader",
    "IsRealRaidLeader",
    "IsRightAltKeyDown",
    "IsRightControlKeyDown",
    "IsRightShiftKeyDown",
    "IsSpellInRange",
    "IsStealthed",
    "IsSwimming",
    "IsTrainerServiceSkillStep",
    "IsUsableItem",
    "IsUsableSpell",
    "IsUsingVehicleControls",
    "IsVehicleAimPowerAdjustable",
    "IsVoiceChatAllowed",
    "ItemHasRange",
    "KBArticle_IsLoaded",
    "KBQuery_GetArticleHeaderData",
    "KBQuery_IsLoaded",
    "KBSetup_GetArticleHeaderData",
    "KBSetup_GetLanguageCount",
    "KBSetup_GetLanguageData",
    "KBSystem_GetServerStatus",
    "MakeMinigameMove",
    "MoveAndSteerStart",
    "MoveAndSteerStop",
    "OpenTrainer",
    "PlayDance",
    "QueryGuildEventLog",
    "QueryQuestsCompleted",
    "QuestIsDaily",
    "QuestIsWeekly",
    "QuestPOIGetQuestIDByIndex",
    "QuestPOIUpdateTexture",
    "ReadFile",
    "RegisterForSavePerCharacter",
    "RenameEquipmentSet",
    "ReportBug",
    "ReportSuggestion",
    "ResetCPUUsage",
    "ResetPerformanceValues",
    "ResurrectGetOfferer",
    "SetAllowLowLevelRaid",
    "SetBindingClick",
    "SetBindingItem",
    "SetBindingMacro",
    "SetBindingSpell",
    "SetChannelWatch",
    "SetInventoryPortraitTexture",
    "SetMacroItem",
    "SetMacroSpell",
    "SetMouselookOverrideBinding",
    "SetPOIIconOverlapDistance",
    "SetPOIIconOverlapPushDistance",
    "SetTrainerSkillLineFilter",
    "SilenceMember",
    "SpellCanTargetGlyph",
    "SpellHasRange",
    "TargetDirectionEnemy",
    "TargetDirectionFinished",
    "TargetDirectionFriend",
    "TargetNearest",
    "TargetTotem",
    "ToggleCollision",
    "ToggleCollisionDisplay",
    "TogglePerformanceDisplay",
    "TogglePerformancePause",
    "TogglePerformanceValues",
    "TogglePlayerBounds",
    "TogglePortals",
    "ToggleTris",
    "UnSilenceMember",
    "UnitInVehicleControlSeat",
    "UnitIsFeignDeath",
    "UnitIsPVPSanctuary",
    "UnitIsTrivial",
    "UnitVehicleSeatCount",
    "UpdateAddOnCPUUsage",
    "UploadSettings",
    "VehicleAimDecrement",
    "VehicleAimDownStart",
    "VehicleAimDownStop",
    "VehicleAimGetAngle",
    "VehicleAimGetNormAngle",
    "VehicleAimGetNormPower",
    "VehicleAimIncrement",
    "VehicleAimRequestAngle",
    "VehicleAimSetNormPower",
    "VehicleAimUpStart",
    "VehicleAimUpStop",
    "VehicleCameraZoomIn",
    "VehicleCameraZoomOut",
    "VehicleNextSeat",
    "VehiclePrevSeat",
    "VoiceGetCurrentCaptureDevice",
    "VoiceGetCurrentOutputDevice",
    "VoicePushToTalkStart",
    "VoicePushToTalkStop",
    "debughook",
    "difftime",
    "hooksecurefunc",
    "issecurevariable",
];

/// `reference/3.3.5a-globals.tsv`'s `engine` names.
fn reference_engine() -> HashSet<String> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/3.3.5a-globals.tsv"
    );
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!("reading {path}: {e} — regenerate with scripts/gen-reference-wotlk-globals.py")
    });
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut cols = l.split('\t');
            let name = cols.next()?;
            (cols.nth(1)? == "engine").then(|| name.to_string())
        })
        .collect()
}

/// A listed verb the VM now exposes has been built: its name leaves the list and #399.
#[test]
fn no_listed_verb_is_built() {
    let ours = engine_globals();
    let built: Vec<&str> = ADDON_ONLY_UNBUILT
        .iter()
        .copied()
        .filter(|n| ours.contains(*n))
        .collect();
    assert!(
        built.is_empty(),
        "built, but still listed as unbuilt: {}\n\n\
         Remove each from `ADDON_ONLY_UNBUILT` in this file and tick it off #399.",
        built.join(" ")
    );
}

/// Every entry is an engine global of the 3.3.5a client, listed once, in byte order.
#[test]
fn the_list_names_reference_engine_globals_in_order() {
    let engine = reference_engine();
    let stray: Vec<&str> = ADDON_ONLY_UNBUILT
        .iter()
        .copied()
        .filter(|n| !engine.contains(*n))
        .collect();
    assert!(
        stray.is_empty(),
        "not an engine global in reference/3.3.5a-globals.tsv: {stray:?}"
    );
    let unordered: Vec<[&str; 2]> = ADDON_ONLY_UNBUILT
        .windows(2)
        .filter(|w| w[0] >= w[1])
        .map(|w| [w[0], w[1]])
        .collect();
    assert!(
        unordered.is_empty(),
        "out of byte order or repeated: {unordered:?}"
    );
}
