# Deviations

What 12340 does that benilla leaves out on purpose, and the gaps still open next to them. Every
other deviation is written where it lives, in the code beside the reference fact (a CVar's
`Deviates` row, a comment).

The first two sections cover the 610 incoming world opcodes 12340 registers a handler for
(`reference/3.3.5a-opcodes.tsv`): each one benilla reads to no effect or does not decode, and
why. Every other opcode has a decoder that does what 12340 does. The last section covers the
engine verbs the stock interface calls that benilla leaves out.

`crates/benilla-protocol/tests/deviations.rs` holds this page to the code: an opcode with no
parse arm must be listed under "Not decoded", and a listed one must still have none. Give an
opcode a decoder and its row goes; drop one and it needs a row.

## Read and dropped

These have a decoder that reads the whole 12340 body and does nothing more. 12340 does nothing
more either, except the rows marked **console**: 12340 prints those to its developer console, and
benilla has no console. That is the deviation.

| Opcode | Name | 12340 | Why benilla drops it |
|---|---|---|---|
| `0x01b` | `SMSG_FORCEACTIONSHOW` | reads two `u32` (`0x6e2f1c`) | no effect in 12340 |
| `0x01d` | `SMSG_PETGODMODE` | **console**: "Pet Godmode enabled/disabled" (`0x6e2f00`) | no console |
| `0x023` | `SMSG_GODMODE` | **console**: "Godmode enabled/disabled" (`0x6e2ed0`) | no console |
| `0x049` | `SMSG_SERVERTIME` | **console**: server time and last tick (`0x7e2a50`) | no console |
| `0x0d9` | `MSG_MOVE_TOGGLE_COLLISION_CHEAT` | the movement relay (`0x741b60`); no movement state changes | no effect in 12340 |
| `0x157` | `SMSG_BINDZONEREPLY` | reads two `u32` (`0x6e308c`) | no effect in 12340 |
| `0x164` | `SMSG_LOOT_ITEM_NOTIFY` | reads guid, slot, item and name (`0x6ced80`) | no effect in 12340 |
| `0x22f` | `CMSG_GM_REQUEST_PLAYER_INFO` | reads nothing (`0x52664a`) | no effect in 12340 |
| `0x230` | `SMSG_GM_PLAYER_INFO` | reads nothing (`0x52664a`) | no effect in 12340 |
| `0x27c` | `SMSG_DAMAGE_CALC_LOG` | **console**: a "DMG:" damage breakdown (`0x6d3f10`) | no console |
| `0x3a9` | `SMSG_IGNORE_REQUIREMENTS_CHEAT` | reads nothing (`0x6e4186`) | no effect in 12340 |
| `0x406` | `SMSG_IGNORE_DIMINISHING_RETURNS_CHEAT` | reads nothing (`0x6e4186`) | no effect in 12340 |
| `0x4d8` | `SMSG_FORCE_ANIM` | reads a guid and a string (`0x714b20`) | no effect in 12340 |
| `0x4ed` | `SMSG_TOGGLE_XP_GAIN` | **console**: "XP Gain Enabled/Disabled" (`0x6e40d6`) | no console |
| `0x4fc` | `SMSG_DEBUG_SERVER_GEO` | reads nothing (`0x8c8de0`) | no effect in 12340 |

## Not decoded

### Left out by decision

| Opcode | Name | Why |
|---|---|---|
| `0x2e6` | `SMSG_WARDEN_DATA` | Warden is not built: a server that requires it is refused at the handshake |
| `0x39e` | `SMSG_VOICE_SESSION_ROSTER_UPDATE` | no voice chat |
| `0x39f` | `SMSG_VOICE_SESSION_LEAVE` | no voice chat |
| `0x3a2` | `SMSG_VOICE_SET_TALKER_MUTED` | no voice chat |
| `0x3b1` | `SMSG_VOICE_PARENTAL_CONTROLS` | no voice chat |
| `0x3da` | `SMSG_AVAILABLE_VOICE_CHANNEL` | no voice chat |
| `0x3e0` | `SMSG_COMSAT_RECONNECT_TRY` | no voice chat |
| `0x3e1` | `SMSG_COMSAT_DISCONNECT` | no voice chat |
| `0x3e2` | `SMSG_COMSAT_CONNECT_FAIL` | no voice chat |
| `0x3e3` | `SMSG_VOICE_CHAT_STATUS` | no voice chat |
| `0x3fc` | `SMSG_VOICESESSION_FULL` | no voice chat |
| `0x2f6` | `SMSG_MINIGAME_SETUP` | minigames not planned (#806) |
| `0x2f7` | `SMSG_MINIGAME_STATE` | minigames not planned (#806) |
| `0x3b6` | `SMSG_COMMENTATOR_STATE_CHANGED` | arena spectating not planned (#795) |
| `0x3b8` | `SMSG_COMMENTATOR_MAP_INFO` | arena spectating not planned (#795) |
| `0x3bb` | `SMSG_COMMENTATOR_PLAYER_INFO` | arena spectating not planned (#795) |
| `0x51c` | `SMSG_COMMENTATOR_SKIRMISH_QUEUE_RESULT1` | arena spectating not planned (#795) |
| `0x51d` | `SMSG_COMMENTATOR_SKIRMISH_QUEUE_RESULT2` | arena spectating not planned (#795) |
| `0x44a` | `SMSG_NOTIFY_DANCE` | dance studio not planned (#802) |
| `0x44c` | `SMSG_PLAY_DANCE` | dance studio not planned (#801) |
| `0x44f` | `SMSG_STOP_DANCE` | dance studio not planned (#800) |

### Not built yet

These are gaps, not decisions. Each has an open issue.

| Opcode | Name | Issue |
|---|---|---|
| `0x360` | `SMSG_UPDATE_LFG_LIST` | #420 |
| `0x3ae` | `MSG_RAID_READY_CHECK_CONFIRM` | #361 |
| `0x3c6` | `MSG_RAID_READY_CHECK_FINISHED` | #361 |
| `0x3c8` | `SMSG_COMPLAIN_RESULT` | #496 |
| `0x3c9` | `SMSG_FEATURE_SYSTEM_STATUS` | #496 |
| `0x3f0` | `SMSG_USERLIST_ADD` | #597 |
| `0x3f1` | `SMSG_USERLIST_REMOVE` | #597 |
| `0x3f2` | `SMSG_USERLIST_UPDATE` | #597 |
| `0x47c` | `SMSG_SET_PHASE_SHIFT` | #456 |
| `0x514` | `SMSG_SEND_ALL_COMBAT_LOG` | #442 |

## Engine verbs left out

Verbs the stock interface calls that benilla does not build, because no 3.3.5a server can serve
what they ask for. The open gaps, the verbs still to build, are tracked in #668 and listed in the
stock-UI test's known-gap list (`crates/benilla-app/src/ui_script/reference_ui.rs`).

### Battle.net (Real ID)

12340 talks to Battle.net over its own login, apart from the realm. A 3.3.5a server has no
Battle.net service, so 12340 itself answers every one of these as if disconnected. benilla builds
the ones the stock interface reads unguarded with those answers: `BNConnected`,
`BNFeaturesEnabled` and `BNFeaturesEnabledAndConnected` are false, and the friend, invite and
block counts are 0 (`crates/benilla-ui/src/script/battle_net.rs`). Each verb below acts on a
Real ID friend, invite, conversation or block row, and with every count at 0 no such row exists.

`BNAcceptFriendInvite`, `BNCreateConversation`, `BNDeclineFriendInvite`, `BNGetBlockedInfo`,
`BNGetBlockedToonInfo`, `BNGetConversationInfo`, `BNGetConversationMemberInfo`,
`BNGetCustomMessageTable`, `BNGetFOFInfo`, `BNGetFriendInfo`, `BNGetFriendInfoByID`,
`BNGetFriendInviteInfo`, `BNGetFriendToonInfo`, `BNGetInfo`, `BNGetNumConversationMembers`,
`BNGetNumFOF`, `BNGetNumFriendToons`, `BNGetSelectedBlock`, `BNGetSelectedFriend`,
`BNGetSelectedToonBlock`, `BNGetToonInfo`, `BNInviteToConversation`, `BNIsFriend`, `BNIsSelf`,
`BNIsToonBlocked`, `BNLeaveConversation`, `BNListConversation`, `BNRemoveFriend`,
`BNReportPlayer`, `BNRequestFOFInfo`, `BNSendConversationMessage`, `BNSendFriendInvite`,
`BNSendFriendInviteByID`, `BNSendWhisper`, `BNSetAFK`, `BNSetBlocked`, `BNSetCustomMessage`,
`BNSetDND`, `BNSetFriendNote`, `BNSetSelectedBlock`, `BNSetSelectedFriend`,
`BNSetSelectedToonBlock`, `BNSetToonBlocked`.

### Knowledge base

The help frame's knowledge base is downloaded over HTTP from a Blizzard web service, not asked of
the realm: the setup request builds a `getKBSetup.xml` address for the locale (`0x550e10`). That
service is gone and no 3.3.5a server stands in for it.

`KBArticle_BeginLoading`, `KBArticle_GetData`, `KBQuery_BeginLoading`,
`KBQuery_GetArticleHeaderCount`, `KBQuery_GetTotalArticleCount`, `KBSetup_BeginLoading`,
`KBSetup_GetArticleHeaderCount`, `KBSetup_GetCategoryCount`, `KBSetup_GetCategoryData`,
`KBSetup_GetSubCategoryCount`, `KBSetup_GetSubCategoryData`, `KBSetup_GetTotalArticleCount`,
`KBSetup_IsLoaded`, `KBSystem_GetServerNotice`.

### Voice chat

benilla has no voice chat (see the voice opcodes above): `ChannelSilenceVoice`,
`ChannelUnSilenceVoice`, `SetActiveVoiceChannelBySessionID`.
