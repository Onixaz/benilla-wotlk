//! Quest messages: [`giver`] for accepting and turning in at an NPC (386-402), [`log`] for the
//! quest log (92/93, 403-410), and `share` for the party quest share (411-413, 630).

mod giver;
mod log;
mod share;

pub use giver::{
    dialog_status, questgiver_accept_quest, questgiver_choose_reward, questgiver_complete_quest,
    questgiver_hello, questgiver_query_quest, questgiver_request_reward, questgiver_status_query,
    QuestComplete, QuestDetails, QuestExtraRewards, QuestGiverList, QuestListEntry,
    QuestOfferReward, QuestReputationRewards, QuestRequestItems, QuestRequiredItem,
    QuestRewardItem, QUEST_EMOTE_COUNT, QUEST_REPUTATIONS_COUNT,
};
pub use log::{
    quest_flags, quest_query, questlog_remove_quest, questlog_swap_quest, QuestObjective,
    QuestTemplate, QUEST_OBJECTIVES_COUNT, QUEST_REQUIRED_ITEMS_COUNT, QUEST_REWARDS_COUNT,
    QUEST_REWARD_CHOICES_COUNT,
};
pub use share::{
    push_quest_to_party, quest_confirm_accept, quest_push_result, QuestConfirmAccept,
    QuestPushResult, QuestShareMsg,
};

pub(super) use giver::{
    read_questgiver_offer_reward, read_questgiver_quest_complete, read_questgiver_quest_details,
    read_questgiver_quest_failed, read_questgiver_quest_invalid, read_questgiver_quest_list,
    read_questgiver_request_items, read_questgiver_status, read_questgiver_status_multiple,
};
pub(super) use log::{
    read_quest_query_response, read_quest_update_add_item, read_quest_update_add_kill,
    read_quest_update_complete, read_quest_update_failed, read_quest_update_failedtimer,
};
pub(super) use share::{read_quest_confirm_accept, read_quest_push_result};
