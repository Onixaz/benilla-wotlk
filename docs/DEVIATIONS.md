# Deviations

What 12340 does that benilla leaves out on purpose. Work not done yet is tracked in issues, not
here. Every other deviation is written where it lives, in the code beside the reference fact (a
CVar's `Deviates` row, a comment).

## Opcodes

Of the 610 incoming world opcodes 12340 registers a handler for (`reference/3.3.5a-opcodes.tsv`),
the ones benilla leaves undecoded on purpose. A test (`crates/benilla-protocol/tests/deviations.rs`)
holds this table to the decoder: an opcode with no parse arm must be listed under "Not decoded" or
be a known gap with an open issue, and a listed one must still have none.

The "AC sends" column says whether AzerothCore sends the opcode at all: "no" means it names it in
its opcode table and nowhere else, so no AzerothCore server ever sends it.

### Not decoded

| Opcode | Name | Why | AC sends |
|---|---|---|---|
| `0x39e` | `SMSG_VOICE_SESSION_ROSTER_UPDATE` | no voice chat | no |
| `0x39f` | `SMSG_VOICE_SESSION_LEAVE` | no voice chat | no |
| `0x3a2` | `SMSG_VOICE_SET_TALKER_MUTED` | no voice chat | no |
| `0x3b1` | `SMSG_VOICE_PARENTAL_CONTROLS` | no voice chat | no |
| `0x3da` | `SMSG_AVAILABLE_VOICE_CHANNEL` | no voice chat | no |
| `0x3e0` | `SMSG_COMSAT_RECONNECT_TRY` | no voice chat | no |
| `0x3e1` | `SMSG_COMSAT_DISCONNECT` | no voice chat | no |
| `0x3e2` | `SMSG_COMSAT_CONNECT_FAIL` | no voice chat | no |
| `0x3e3` | `SMSG_VOICE_CHAT_STATUS` | no voice chat | no |
| `0x3fc` | `SMSG_VOICESESSION_FULL` | no voice chat | no |
| `0x2f6` | `SMSG_MINIGAME_SETUP` | minigames not planned (#806) | no |
| `0x2f7` | `SMSG_MINIGAME_STATE` | minigames not planned (#806) | no |
| `0x3b6` | `SMSG_COMMENTATOR_STATE_CHANGED` | arena spectating not planned (#795) | no |
| `0x3b8` | `SMSG_COMMENTATOR_MAP_INFO` | arena spectating not planned (#795) | no |
| `0x3bb` | `SMSG_COMMENTATOR_PLAYER_INFO` | arena spectating not planned (#795) | no |
| `0x51c` | `SMSG_COMMENTATOR_SKIRMISH_QUEUE_RESULT1` | arena spectating not planned (#795) | no |
| `0x51d` | `SMSG_COMMENTATOR_SKIRMISH_QUEUE_RESULT2` | arena spectating not planned (#795) | no |
| `0x44a` | `SMSG_NOTIFY_DANCE` | dance studio not planned (#802) | no |
| `0x44c` | `SMSG_PLAY_DANCE` | dance studio not planned (#801) | no |
| `0x44f` | `SMSG_STOP_DANCE` | dance studio not planned (#800) | no |

## Engine verbs

Engine verbs that benilla does not build, because no 3.3.5a server can serve what they ask for or
the feature behind them is not planned. Some are called by the stock interface. Others only an
addon calls (#399).

- **Battle.net (Real ID).** 12340 talks to Battle.net over its own login, apart from the realm. A
  3.3.5a server has no Battle.net service, so 12340 itself answers as disconnected. benilla builds
  the verbs the stock interface reads unguarded with those answers: `BNConnected`,
  `BNFeaturesEnabled` and `BNFeaturesEnabledAndConnected` are false, and the friend, invite and
  block counts are 0 (`crates/benilla-ui/src/script/battle_net.rs`). The rest act on a Real ID
  friend, invite, conversation or block row, and with every count at 0 no such row exists.
- **Knowledge base.** The help frame downloads its articles over HTTP from a Blizzard web
  service, not from the realm: the setup request builds a `getKBSetup.xml` address for the locale
  (`0x550e10`). That service is gone and no 3.3.5a server stands in for it.
- **Voice chat.** Not planned: benilla has no voice chat (see the voice opcodes above). This covers the
  channel and party silence verbs: they send `CMSG_CHANNEL_SILENCE_ALL`, `CMSG_CHANNEL_UNSILENCE_ALL`
  and `CMSG_PARTY_SILENCE`, which are voice moderation.
- **Arena spectating, minigames and the dance studio.** Their packets are not decoded (see the
  opcodes above), so the `Commentator*`, minigame and `PlayDance` verbs have nothing to drive.

| Verb | 12340 | Why |
|---|---|---|
| `BNAcceptFriendInvite` | `0x5355c0` | no Battle.net |
| `BNCreateConversation` | `0x535ce0` | no Battle.net |
| `BNDeclineFriendInvite` | `0x535660` | no Battle.net |
| `BNGetBlockedInfo` | `0x53a540` | no Battle.net |
| `BNGetBlockedToonInfo` | `0x536970` | no Battle.net |
| `BNGetConversationInfo` | `0x536330` | no Battle.net |
| `BNGetConversationMemberInfo` | `0x53a150` | no Battle.net |
| `BNGetCustomMessageTable` | `0x535aa0` | no Battle.net |
| `BNGetFOFInfo` | `0x537240` | no Battle.net |
| `BNGetFriendInfo` | `0x539bf0` | no Battle.net |
| `BNGetFriendInfoByID` | `0x539cc0` | no Battle.net |
| `BNGetFriendInviteInfo` | `0x535180` | no Battle.net |
| `BNGetFriendToonInfo` | `0x539d70` | no Battle.net |
| `BNGetInfo` | `0x5343f0` | no Battle.net |
| `BNGetNumConversationMembers` | `0x536220` | no Battle.net |
| `BNGetNumFOF` | `0x5370d0` | no Battle.net |
| `BNGetNumFriendToons` | `0x5349f0` | no Battle.net |
| `BNGetSelectedBlock` | `0x536790` | no Battle.net |
| `BNGetSelectedFriend` | `0x537600` | no Battle.net |
| `BNGetSelectedToonBlock` | `0x536d40` | no Battle.net |
| `BNGetToonInfo` | `0x539f90` | no Battle.net |
| `BNInviteToConversation` | `0x535eb0` | no Battle.net |
| `BNIsBlocked` | `0x5364e0` | no Battle.net |
| `BNIsFriend` | `0x537950` | no Battle.net |
| `BNIsSelf` | `0x5378a0` | no Battle.net |
| `BNIsToonBlocked` | `0x536a90` | no Battle.net |
| `BNLeaveConversation` | `0x536030` | no Battle.net |
| `BNListConversation` | `0x53a300` | no Battle.net |
| `BNRemoveFriend` | `0x534ed0` | no Battle.net |
| `BNReportFriendInvite` | `0x535700` | no Battle.net |
| `BNReportPlayer` | `0x536e40` | no Battle.net |
| `BNRequestFOFInfo` | `0x53a660` | no Battle.net |
| `BNSendConversationMessage` | `0x536110` | no Battle.net |
| `BNSendFriendInvite` | `0x535380` | no Battle.net |
| `BNSendFriendInviteByID` | `0x535490` | no Battle.net |
| `BNSendWhisper` | `0x53a030` | no Battle.net |
| `BNSetAFK` | `0x5357a0` | no Battle.net |
| `BNSetBlocked` | `0x5365b0` | no Battle.net |
| `BNSetCustomMessage` | `0x535920` | no Battle.net |
| `BNSetDND` | `0x535860` | no Battle.net |
| `BNSetFocus` | `0x535c60` | no Battle.net |
| `BNSetFriendNote` | `0x534f80` | no Battle.net |
| `BNSetSelectedBlock` | `0x5366a0` | no Battle.net |
| `BNSetSelectedFriend` | `0x537510` | no Battle.net |
| `BNSetSelectedToonBlock` | `0x536c50` | no Battle.net |
| `BNSetToonBlocked` | `0x536b60` | no Battle.net |
| `KBArticle_BeginLoading` | `0x551d60` | no knowledge-base service |
| `KBArticle_GetData` | `0x551dd0` | no knowledge-base service |
| `KBArticle_IsLoaded` | `0x551da0` | no knowledge-base service |
| `KBQuery_BeginLoading` | `0x552580` | no knowledge-base service |
| `KBQuery_GetArticleHeaderCount` | `0x551bc0` | no knowledge-base service |
| `KBQuery_GetArticleHeaderData` | `0x551c00` | no knowledge-base service |
| `KBQuery_GetTotalArticleCount` | `0x551d10` | no knowledge-base service |
| `KBQuery_IsLoaded` | `0x551b90` | no knowledge-base service |
| `KBSetup_BeginLoading` | `0x551720` | no knowledge-base service |
| `KBSetup_GetArticleHeaderCount` | `0x5519f0` | no knowledge-base service |
| `KBSetup_GetArticleHeaderData` | `0x551a30` | no knowledge-base service |
| `KBSetup_GetCategoryCount` | `0x5518c0` | no knowledge-base service |
| `KBSetup_GetCategoryData` | `0x551900` | no knowledge-base service |
| `KBSetup_GetLanguageCount` | `0x551790` | no knowledge-base service |
| `KBSetup_GetLanguageData` | `0x5517d0` | no knowledge-base service |
| `KBSetup_GetSubCategoryCount` | `0x5523a0` | no knowledge-base service |
| `KBSetup_GetSubCategoryData` | `0x552440` | no knowledge-base service |
| `KBSetup_GetTotalArticleCount` | `0x551b40` | no knowledge-base service |
| `KBSetup_IsLoaded` | `0x551760` | no knowledge-base service |
| `KBSystem_GetServerNotice` | `0x551ff0` | no knowledge-base service |
| `KBSystem_GetServerStatus` | `0x551fd0` | no knowledge-base service |
| `ChannelSilenceAll` | `0x501250` | no voice chat |
| `ChannelSilenceVoice` | `0x501140` | no voice chat |
| `ChannelUnSilenceAll` | `0x501400` | no voice chat |
| `ChannelUnSilenceVoice` | `0x5012f0` | no voice chat |
| `ChannelVoiceOff` | `0x4fea90` | no voice chat |
| `ChannelVoiceOn` | `0x4fea70` | no voice chat |
| `DisplayChannelVoiceOff` | `0x4fead0` | no voice chat |
| `DisplayChannelVoiceOn` | `0x4feab0` | no voice chat |
| `IsVoiceChatAllowed` | `0x4fcc60` | no voice chat |
| `SetActiveVoiceChannelBySessionID` | `0x7db280` | no voice chat |
| `SilenceMember` | `0x52d9c0` | no voice chat |
| `UnSilenceMember` | `0x52daf0` | no voice chat |
| `VoiceGetCurrentCaptureDevice` | `0x7dafe0` | no voice chat |
| `VoiceGetCurrentOutputDevice` | `0x7daf90` | no voice chat |
| `VoicePushToTalkStart` | `0x511d20` | no voice chat |
| `VoicePushToTalkStop` | `0x511d20` | no voice chat |
| `CommentatorAddPlayer` | `0x56a640` | arena spectating not planned (#795) |
| `CommentatorEnterInstance` | `0x5696e0` | arena spectating not planned (#795) |
| `CommentatorExitInstance` | `0x5697b0` | arena spectating not planned (#795) |
| `CommentatorFollowPlayer` | `0x569b50` | arena spectating not planned (#795) |
| `CommentatorGetCamera` | `0x56a2a0` | arena spectating not planned (#795) |
| `CommentatorGetCurrentMapID` | `0x56a380` | arena spectating not planned (#795) |
| `CommentatorGetInstanceInfo` | `0x569520` | arena spectating not planned (#795) |
| `CommentatorGetMapInfo` | `0x5693e0` | arena spectating not planned (#795) |
| `CommentatorGetMode` | `0x569230` | arena spectating not planned (#795) |
| `CommentatorGetNumMaps` | `0x568730` | arena spectating not planned (#795) |
| `CommentatorGetNumPlayers` | `0x569820` | arena spectating not planned (#795) |
| `CommentatorGetPlayerInfo` | `0x569910` | arena spectating not planned (#795) |
| `CommentatorGetSkirmishMode` | `0x56af00` | arena spectating not planned (#795) |
| `CommentatorGetSkirmishQueueCount` | `0x56af60` | arena spectating not planned (#795) |
| `CommentatorGetSkirmishQueuePlayerInfo` | `0x56afc0` | arena spectating not planned (#795) |
| `CommentatorLookatPlayer` | `0x569cd0` | arena spectating not planned (#795) |
| `CommentatorRemovePlayer` | `0x56a860` | arena spectating not planned (#795) |
| `CommentatorRequestSkirmishMode` | `0x56ae50` | arena spectating not planned (#795) |
| `CommentatorRequestSkirmishQueueData` | `0x56ada0` | arena spectating not planned (#795) |
| `CommentatorSetBattlemaster` | `0x56aa10` | arena spectating not planned (#795) |
| `CommentatorSetCamera` | `0x56a0f0` | arena spectating not planned (#795) |
| `CommentatorSetCameraCollision` | `0x56ab70` | arena spectating not planned (#795) |
| `CommentatorSetMapAndInstanceIndex` | `0x569e50` | arena spectating not planned (#795) |
| `CommentatorSetMode` | `0x5690a0` | arena spectating not planned (#795) |
| `CommentatorSetMoveSpeed` | `0x56aaf0` | arena spectating not planned (#795) |
| `CommentatorSetPlayerIndex` | `0x569fa0` | arena spectating not planned (#795) |
| `CommentatorSetSkirmishMatchmakingMode` | `0x56ac90` | arena spectating not planned (#795) |
| `CommentatorSetTargetHeightOffset` | `0x56ac10` | arena spectating not planned (#795) |
| `CommentatorStartInstance` | `0x56a410` | arena spectating not planned (#795) |
| `CommentatorStartSkirmishMatch` | `0x56b0b0` | arena spectating not planned (#795) |
| `CommentatorToggleMode` | `0x569180` | arena spectating not planned (#795) |
| `CommentatorUpdateMapInfo` | `0x569340` | arena spectating not planned (#795) |
| `CommentatorUpdatePlayerInfo` | `0x5692a0` | arena spectating not planned (#795) |
| `CommentatorZoomIn` | `0x568770` | arena spectating not planned (#795) |
| `CommentatorZoomOut` | `0x568810` | arena spectating not planned (#795) |
| `GetMinigameState` | `0x5c55f0` | minigames not planned (#806) |
| `GetMinigameType` | `0x5c5500` | minigames not planned (#806) |
| `MakeMinigameMove` | `0x5c5550` | minigames not planned (#806) |
| `PlayDance` | `0x5761f0` | dance studio not planned (#801) |

## Bugs in 12340

Where 12340's code does something other than what it is written to do, benilla does what the code
means and records the difference here.

- **World map: an instance's entrance on its continent's zone grid.** For an instance with no
  `WorldMapArea` row, the current-zone lookup (`0x547170`) carries the `Map.dbc` entrance to its
  continent and reads the `.zmp` cell under it, to open that zone. Before the read, its range test
  clamps each grid coordinate to [0, 1], stores it as a float and compares it back with the
  unrounded value (`0x547649`, `0x547661`). The client's x87 runs at double precision, so the test
  fails unless the coordinate is exactly a float. None of the 71 entrances that land on a continent
  passes, so 12340 falls through to the zone walk, finds no zone for the instance's area and opens
  the continent: Outland in Hellfire Ramparts and the Underbog (`GetCurrentMapAreaID()` 467,
  `GetCurrentMapZone()` 0). benilla runs the test in single precision, where it passes, and opens
  the zone the lookup is written to find: Hellfire Peninsula and Zangarmarsh. An instance whose
  `Map.dbc` entrance is zero (Tempest Keep) opens the cosmic map in both.
  The Mac build of 12340 opens the zone as benilla does: its copy of the lookup (`0x6286d0`, i386)
  works each coordinate out in SSE single precision (`0x628f38`–`0x628f65`) and compares the
  clamped value with it as floats (`0x629485` for x, `0x6295ff` for y), so the cell read at
  `0x62960e` is reached.

## The Mac build

On macOS benilla answers as the Mac build of 12340: `IsMacClient()` is 1, the `platform="mac"` rows
of a binding file register, and the Mac build's own 23 verbs and 12 CVars exist
(`crates/benilla-ui/src/script/mac_client.rs`, `REGISTERED_MAC` in
`crates/benilla-app/src/cvars/table.rs`). What those verbs drive is left out. The addresses in this
table are the Mac build's.

| Verbs | Mac 12340 | What benilla answers | Why |
|---|---|---|---|
| `MovieRecording_IsSupported`, `MovieRecording_IsCursorRecordingSupported` | `0x508af0`, `0x508ba0` | `false`, the Mac client's answer on a machine below the recorder's requirements (`0x909f40`) | no movie recording |
| `MovieRecording_Toggle` | `0x511840` | shows `MOVIE_RECORDING_WARNING_REQUIREMENTS`, as on such a machine | no movie recording |
| `MovieRecording_Cancel`, `MovieRecording_SearchUncompressedMovie`, `MovieRecording_QueueMovieToCompress`, `MovieRecording_DeleteMovie` | `0x5088e0`, `0x508a50`, `0x508a90`, `0x508ac0` | nothing | no movie recording |
| `MovieRecording_IsRecording`, `MovieRecording_IsCompressing`, `MovieRecording_GetProgress`, `MovieRecording_GetTime`, `MovieRecording_GetMovieFullPath`, `MovieRecording_MaxLength` | `0x508910`, `0x508940`, `0x5089a0`, `0x508c00`, `0x508bd0`, `0x508d00` | the idle recorder's values | no movie recording |
| `MovieRecording_IsCodecSupported` | `0x508b20` | `false` for every codec | no QuickTime |
| `MusicPlayer_PlayPause`, `MusicPlayer_VolumeUp`, `MusicPlayer_VolumeDown`, `MusicPlayer_BackTrack`, `MusicPlayer_NextTrack` | `0x51c100`, `0x51c150`, `0x51c1a0`, `0x51c1f0`, `0x51c240` | nothing | no music player remote |

## DBC tables

Of the 241 tables 12340 loads (`reference/3.3.5a-dbc.tsv`), the ones benilla does not read on
purpose because nothing in 12340 makes them matter. Most are loaded at startup and never read
again, so leaving them out matches 12340. Unread tables that are open work are listed with their
issues in `NOT_READ` (`crates/benilla-formats/tests/dbc_coverage.rs`), not here.

| Table | Why |
|---|---|
| `AttackAnimKits` | 12340 loads it and never reads it |
| `AttackAnimTypes` | 12340 loads it and never reads it |
| `CreatureSpellData` | 12340 loads it and never reads it |
| `gtOCTRegenMP` | 12340 loads it as game table 8 and never evaluates that slot |
| `ItemCondExtCosts` | 12340 loads it and never reads it |
| `SheatheSoundLookups` | 12340 loads it and never reads it |
| `SoundEmitters` | 12340 loads it and never reads it |
| `SoundSamplePreferences` | 12340 loads it and never reads it |
| `SoundWaterType` | 12340 loads it and never reads it |
| `UISoundLookups` | 12340 loads it and never reads it |
| `WorldSafeLocs` | read only by the admin `worldport` console command (`0x407210`), not planned (#1119); the graveyard branch of `GetMapLandmarkInfo` (`0x545a30`) never runs, since the landmark builder (`0x546310`) never sets its flag |
