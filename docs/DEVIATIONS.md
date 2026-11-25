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

## Bugs in 12340

Where 12340's code does something other than what it is written to do, benilla does what the code
means and records the difference here.

### World map: an instance's entrance on its continent's zone grid

- **12340:** For an instance with no `WorldMapArea` row, the current-zone lookup (`0x547170`)
  carries the `Map.dbc` entrance to its continent and reads the `.zmp` cell under it, to open that
  zone. Before the read, its range test clamps each grid coordinate to [0, 1], stores it as a float
  and compares it back with the unrounded value (`0x547649`, `0x547661`). The client's x87 runs at
  double precision, so the test fails unless the coordinate is exactly a float. None of the 71
  entrances that land on a continent passes, so 12340 falls through to the zone walk, finds no zone
  for the instance's area and opens the continent: Outland in Hellfire Ramparts and the Underbog
  (`GetCurrentMapAreaID()` 467, `GetCurrentMapZone()` 0).
- **benilla:** runs the test in single precision, where it passes, and opens the zone the lookup is
  written to find: Hellfire Peninsula and Zangarmarsh. An instance whose `Map.dbc` entrance is zero
  (Tempest Keep) opens the cosmic map in both.
- **The Mac build:** opens the zone as benilla does: its copy of the lookup (`0x6286d0`, i386) works
  each coordinate out in SSE single precision (`0x628f38`–`0x628f65`) and compares the clamped value
  with it as floats (`0x629485` for x, `0x6295ff` for y), so the cell read at `0x62960e` is reached.

### Chat log: a channel notice's line

- **12340:** While `LoggingChat` is on, the chat add writes each channel notice
  (`CHAT_MSG_CHANNEL_NOTICE`, `CHAT_MSG_CHANNEL_NOTICE_USER`) to `WoWChatLog.txt` with the global
  string `CHAT_<TOKEN>_NOTICE` as the format. Earlier it prints the channel's `"%d. %s"` name into a
  stack buffer (`0x50a44e`); the log arm (`0x50aa65`–`0x50aaa9`) then passes that buffer's address,
  the sender and the target as the format's arguments. In enUS, `CHAT_YOU_JOINED_NOTICE` is `Joined
  Channel: |Hchannel:%d|h[%s]|h`, so the `%d` takes the buffer's address and the brackets the
  sender, which a notice leaves empty: 12340 logs `Joined Channel: |Hchannel:<address>|h[]|h`, with
  a stack address that changes from run to run.
- **benilla:** fills every `CHAT_<TOKEN>_NOTICE` in the log the way the chat window fills it for the
  same event (`ChatFrame.lua`: the channel number, arg8, for `%d` and the channel's display name,
  arg4, for `[%s]`, then the sender and target for a user notice), so the log reads `Joined Channel:
  |Hchannel:1|h[1. General - Stormwind City]|h`, the line the player saw.

### Honor line: a victim with no side

- **12340:** The honor-gain line (`0x50b6d0`) names a ranked victim by the global string
  `PVP_RANK_<rank>_<side>`, printed into a stack buffer at `0x50b789`–`0x50b799`. The side comes
  from the victim's faction template group: 0 for the `0x4` bit, 1 for the `0x2` bit
  (`0x50b755`–`0x50b784`). When the template has neither bit, or no record (`0x50b76e`), the code
  jumps past the print (`0x50b787`) and still looks the buffer up as a global string name
  (`0x50b7b0`), so 12340 reads whatever the stack held.
- **benilla:** names such a victim with side 0. Every faction template a server sends for a player
  carries one of the two bits.

### Chat log: a language with no record

- **12340:** When a line's language is not the active player's default, the log arm prints its
  `[Language] ` header from the `Languages.dbc` record's name (`0x50abcb`–`0x50abeb`). It reads the
  name at `0x50abd6` without testing the record, and the lookup returns null for an id outside the
  table (`0x65c2ae`), so 12340 faults writing that log line.
- **benilla:** writes an empty name. Every language a server sends in chat has its record.

### `GetTradeSkillCooldown` across a clock tick

- **12340:** The cooldown history keeps a record whose time left, read against the OS clock
  (`0x807a7f`), is not negative (`0x807ae5`); the verb then reads the clock again (`0x5daa17`) and
  pushes the end less it as an unsigned count of milliseconds (`0x5daa24`-`0x5daa36`). When the
  clock ticks past the end between the two reads, the difference is -1 and the verb answers about
  4294967 seconds.
- **benilla:** answers nil for a cooldown already past its end at the call, and 0 at its end's
  millisecond.

### `EquipItemByName` with a slot name

- **12340:** The verb (`0x51cdb0`) takes its target slot as a number or as a slot name. A number is
  made 0-based (`0x51cf23`); a name's slot id is used as it is (`0x51cfb4`), so 12340 equips the
  item one slot past the one named: `HeadSlot` to the neck, `Trinket0Slot` to the second trinket,
  and `TabardSlot` (19) fails the range test (`0x51cfbf`) with "Invalid inventory dstSlot".
- **benilla:** makes a name 0-based as it does a number, so a name equips to the slot it names.

### Item tooltip: a token's bind line

- **12340:** The tooltip builder prints a token's (item class 10) bind line only when the requester
  guid it was handed equals the loot window's source (`0x627ef7`–`0x627f17`), so the loot window
  (`0x62e638`) shows it. A tooltip built from an id or a link hands it a zero requester (a chat link
  `0x62dbbc`, an unheld token `0x630750`), which equals the loot source only while no loot window is
  open, its guid then being 0. So 12340 shows "Binds when picked up" on a linked token, and drops it
  while any loot window is open.
- **benilla:** shows it on every tooltip built with a zero requester; a held token, whose requester
  is the player (`0x630789`), still shows none.

### Inventory verbs with a slot name

- **12340:** The inventory slot reader (`0x5e7c00`) behind `GetInventoryItemLink`,
  `GetInventoryItemID`, `GetInventoryItemTexture`, `GetInventoryItemQuality`,
  `GetInventoryItemCount`, `GetInventoryItemBroken`, `SocketInventoryItem`,
  `ShowInventorySellCursor`, `SetInventoryPortraitTexture` and the other verbs that take a slot
  makes a number 0-based (`0x5e7c2c`) but keeps a slot name's id (`0x5e7c9e`), so
  `GetInventoryItemLink("player", "HeadSlot")` reads the neck. `GetInventoryItemsForSlot` does the
  same in its own read (a number less one at `0x5e95e8`, a name's id kept at `0x5e967f`), listing
  what fits the slot after the one named.
- **benilla:** makes a name 0-based in both, as a number is, so a name reads the slot it names.

### `ListChannels`: the line's length check

- **12340:** `ListChannels` (`0x50dae0`) prints each joined channel as `[%d. %s] ` and packs it
  into a 3000-byte line, adding the line as `CHAT_MSG_CHANNEL_LIST` first when the entry would not
  fit. The fit test (`0x50db51`–`0x50db63`) takes the length of the string at the line's address
  plus the entry's length, where it means the line's length plus the entry's length. While the line
  is shorter than the entry, that reads past the line's end into stack bytes nothing wrote, so
  whether 12340 adds the line before that entry (an empty one, at the first) depends on what the
  stack held; once the line is longer, the test passes later than the 3000-byte limit means.
- **benilla:** compares the line's length plus the entry's length with 3000. Ten channels never
  reach it, so benilla lists them all on one line.

### Group chat: the battleground test with no map record

- **12340:** `SendChatMessage`'s `PARTY` check asks whether the current map is a battleground or
  an arena (`0x572f50`, `InstanceType` 3 or 4). When the map id is outside `Map.dbc`'s range
  (`0x572f5d`, `0x572f65` → `0x572f88`) or its row is missing (`0x572f74`), it still reads the
  instance type through the null record (`0x572f7c`), so 12340 faults. In the world the current
  map is always a `Map.dbc` row, so the fault is not reached.
- **benilla:** treats a map with no record as neither.
