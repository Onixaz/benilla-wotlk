//! The addon-only verbs still unbuilt (#399): engine globals 12340 registers that no stock
//! FrameXML or GlueXML file calls, so no play session notices one missing. The list is exact and
//! only shrinks: building one fails this gate until its name leaves the list. The ones left out on
//! purpose are a second list, held to `docs/DEVIATIONS.md` and to staying unbuilt.

use std::collections::HashSet;

use super::reference_surface::engine_globals;

/// Engine globals the 3.3.5a client has and benilla's VM does not, none called by the stock
/// interface. Byte-sorted.
const ADDON_ONLY_UNBUILT: &[&str] = &[
    "ChangePlayerDifficulty",
    "ClearChannelWatch",
    "CursorHasMacro",
    "DeclineName",
    "DownloadSettings",
    "GMSurveyNumAnswers",
    "GetAllowLowLevelRaid",
    "GetMacroBody",
    "GetMacroItem",
    "GetMacroSpell",
    "GetMaxArenaCurrency",
    "GetNumDeclensionSets",
    "GetRunningMacro",
    "GetRunningMacroButton",
    "GetTaxiBenchmarkMode",
    "GetUnitPitch",
    "GetUnitSpeed",
    "InteractUnit",
    "MoveAndSteerStart",
    "MoveAndSteerStop",
    "OpenTrainer",
    "QuestIsDaily",
    "QuestIsWeekly",
    "QuestPOIGetQuestIDByIndex",
    "ReportBug",
    "ReportSuggestion",
    "SetAllowLowLevelRaid",
    "SetChannelWatch",
    "SetMacroItem",
    "SetMacroSpell",
    "TargetDirectionEnemy",
    "TargetDirectionFinished",
    "TargetDirectionFriend",
    "TargetNearest",
    "TargetTotem",
    "UploadSettings",
];

/// Addon-only engine globals benilla leaves out on purpose, each a row of `docs/DEVIATIONS.md`'s
/// engine-verb table: Battle.net, the knowledge base, voice chat, arena spectating, minigames and the
/// dance studio. Byte-sorted.
const ADDON_ONLY_DEVIATED: &[&str] = &[
    "BNIsBlocked",
    "BNReportFriendInvite",
    "BNSetFocus",
    "ChannelSilenceAll",
    "ChannelUnSilenceAll",
    "ChannelVoiceOff",
    "ChannelVoiceOn",
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
    "DisplayChannelVoiceOff",
    "DisplayChannelVoiceOn",
    "GetMinigameState",
    "GetMinigameType",
    "IsVoiceChatAllowed",
    "KBArticle_IsLoaded",
    "KBQuery_GetArticleHeaderData",
    "KBQuery_IsLoaded",
    "KBSetup_GetArticleHeaderData",
    "KBSetup_GetLanguageCount",
    "KBSetup_GetLanguageData",
    "KBSystem_GetServerStatus",
    "MakeMinigameMove",
    "PlayDance",
    "SilenceMember",
    "UnSilenceMember",
    "VoiceGetCurrentCaptureDevice",
    "VoiceGetCurrentOutputDevice",
    "VoicePushToTalkStart",
    "VoicePushToTalkStop",
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

/// A deviated verb stays unbuilt, is a 3.3.5a engine global, and has its row in
/// `docs/DEVIATIONS.md`; the two lists share no name.
#[test]
fn the_deviated_verbs_are_written_down_and_unbuilt() {
    let doc = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/DEVIATIONS.md"
    ))
    .expect("docs/DEVIATIONS.md");
    let rows: HashSet<&str> = doc
        .lines()
        .filter_map(|l| l.strip_prefix("| `"))
        .filter_map(|l| l.split('`').next())
        .collect();
    let engine = reference_engine();
    let ours = engine_globals();
    let mut wrong: Vec<String> = Vec::new();
    for name in ADDON_ONLY_DEVIATED {
        if !rows.contains(name) {
            wrong.push(format!("{name}: no row in docs/DEVIATIONS.md"));
        }
        if !engine.contains(*name) {
            wrong.push(format!("{name}: not an engine global of 3.3.5a"));
        }
        if ours.contains(*name) {
            wrong.push(format!("{name}: built, so not a deviation any more"));
        }
        if ADDON_ONLY_UNBUILT.contains(name) {
            wrong.push(format!("{name}: also listed as unbuilt"));
        }
    }
    wrong.extend(
        ADDON_ONLY_DEVIATED
            .windows(2)
            .filter(|w| w[0] >= w[1])
            .map(|w| format!("out of byte order or repeated: {} {}", w[0], w[1])),
    );
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
