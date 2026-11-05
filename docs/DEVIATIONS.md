# Deviations

What 12340 does that benilla leaves out on purpose. Work not done yet is tracked in issues, not
here. Every other deviation is written where it lives, in the code beside the reference fact (a
CVar's `Deviates` row, a comment).

## Opcodes

Of the 610 incoming world opcodes 12340 registers a handler for (`reference/3.3.5a-opcodes.tsv`),
the ones benilla reads to no effect or leaves undecoded on purpose. A test
(`crates/benilla-protocol/tests/deviations.rs`) holds these tables to the decoder: an opcode with
no parse arm must be listed under "Not decoded" or be a known gap with an open issue, and a listed
one must still have none.

### Read and dropped

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

### Not decoded

| Opcode | Name | Why |
|---|---|---|
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

## Engine verbs

Engine verbs the stock interface calls that benilla does not build, because no 3.3.5a server can
serve what they ask for.

- **Battle.net (Real ID).** 12340 talks to Battle.net over its own login, apart from the realm. A
  3.3.5a server has no Battle.net service, so 12340 itself answers as disconnected. benilla builds
  the verbs the stock interface reads unguarded with those answers: `BNConnected`,
  `BNFeaturesEnabled` and `BNFeaturesEnabledAndConnected` are false, and the friend, invite and
  block counts are 0 (`crates/benilla-ui/src/script/battle_net.rs`). The rest act on a Real ID
  friend, invite, conversation or block row, and with every count at 0 no such row exists.
- **Knowledge base.** The help frame downloads its articles over HTTP from a Blizzard web
  service, not from the realm: the setup request builds a `getKBSetup.xml` address for the locale
  (`0x550e10`). That service is gone and no 3.3.5a server stands in for it.
- **Voice chat.** benilla has no voice chat (see the voice opcodes above).

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
| `BNIsFriend` | `0x537950` | no Battle.net |
| `BNIsSelf` | `0x5378a0` | no Battle.net |
| `BNIsToonBlocked` | `0x536a90` | no Battle.net |
| `BNLeaveConversation` | `0x536030` | no Battle.net |
| `BNListConversation` | `0x53a300` | no Battle.net |
| `BNRemoveFriend` | `0x534ed0` | no Battle.net |
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
| `BNSetFriendNote` | `0x534f80` | no Battle.net |
| `BNSetSelectedBlock` | `0x5366a0` | no Battle.net |
| `BNSetSelectedFriend` | `0x537510` | no Battle.net |
| `BNSetSelectedToonBlock` | `0x536c50` | no Battle.net |
| `BNSetToonBlocked` | `0x536b60` | no Battle.net |
| `KBArticle_BeginLoading` | `0x551d60` | no knowledge-base service |
| `KBArticle_GetData` | `0x551dd0` | no knowledge-base service |
| `KBQuery_BeginLoading` | `0x552580` | no knowledge-base service |
| `KBQuery_GetArticleHeaderCount` | `0x551bc0` | no knowledge-base service |
| `KBQuery_GetTotalArticleCount` | `0x551d10` | no knowledge-base service |
| `KBSetup_BeginLoading` | `0x551720` | no knowledge-base service |
| `KBSetup_GetArticleHeaderCount` | `0x5519f0` | no knowledge-base service |
| `KBSetup_GetCategoryCount` | `0x5518c0` | no knowledge-base service |
| `KBSetup_GetCategoryData` | `0x551900` | no knowledge-base service |
| `KBSetup_GetSubCategoryCount` | `0x5523a0` | no knowledge-base service |
| `KBSetup_GetSubCategoryData` | `0x552440` | no knowledge-base service |
| `KBSetup_GetTotalArticleCount` | `0x551b40` | no knowledge-base service |
| `KBSetup_IsLoaded` | `0x551760` | no knowledge-base service |
| `KBSystem_GetServerNotice` | `0x551ff0` | no knowledge-base service |
| `ChannelSilenceVoice` | `0x501140` | no voice chat |
| `ChannelUnSilenceVoice` | `0x5012f0` | no voice chat |
| `SetActiveVoiceChannelBySessionID` | `0x7db280` | no voice chat |
