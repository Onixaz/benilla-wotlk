//! The client state the verbs read at call time. 12340's verbs read the live object when called
//! (`UnitOnTaxi` `0x60f350` reads the flags word at `0x60f396`); here the app lends a [`LiveState`]
//! over its own state for the length of every call into Lua ([`UiScript::with_live`]), and a verb
//! reads it through [`read_live`]. The loan is the calling thread's: the VM runs on one thread, and a test
//! thread's loan never reaches another's.

use super::UiScript;
#[cfg(test)]
use std::cell::Cell;

/// The VM's own strings by key, which a read building display text formats through.
pub type GlobalStrings<'a> = &'a dyn Fn(&str) -> Option<String>;

/// What the verbs read live, implemented by the app over its state; the `&mut self` methods, reached through [`write_live`], write what a verb changes in its call, as `ClosePetStables` clears the master inside its call.
pub trait LiveState {
    /// A held unit's flags word (`UNIT_FIELD_FLAGS`); `None` for a guid that is no held unit.
    fn unit_flags(&self, guid: u64) -> Option<u32>;

    /// A held unit's health (`UNIT_FIELD_HEALTH`); `None` for a guid that is no held unit.
    fn unit_health(&self, guid: u64) -> Option<u32>;

    /// A held unit's `UNIT_DYNAMIC_FLAGS`, whose `0x20` the death verbs test (`0x60f504`); `None`
    /// for a guid that is no held unit.
    fn unit_dynamic_flags(&self, _guid: u64) -> Option<u32> {
        None
    }

    /// A held player's `PLAYER_FLAGS`, whose ghost bit `0x10` `UnitIsGhost` tests after the type
    /// mask's player bit (`0x60f5f8`-`0x60f612`); `None` for a guid that is no held player.
    fn player_flags(&self, _guid: u64) -> Option<u32> {
        None
    }

    /// The active player's guid (`0x4d3790`), `None` before our create names one.
    fn active_player(&self) -> Option<u64>;

    /// The UI's control flag (`0xbcfb8c`), which the control update for our own unit writes
    /// (`0x520fe0`) and a world entry sets (`0x513880`).
    fn has_control(&self) -> bool;

    /// Whether our view looks through a far-sight subject: the bit set as `CMSG_FAR_SIGHT` 1 goes
    /// out (`0x6e29b5`) and cleared ahead of the 0 (`0x6e2a65`), which `HasFullControl` refuses on.
    fn far_sight(&self) -> bool;

    /// Milliseconds left on the release timer (`0xbd0848`), `None` while its no-timer flag
    /// (`0xbd084c`) is set; a UI load zeroes both (`0x52a9f6`).
    fn release_time_left_ms(&self) -> Option<u32> {
        Some(0)
    }

    /// Milliseconds until the corpse is reclaimable (`0xbd0850`), 0 when unset or passed.
    fn corpse_recovery_delay_ms(&self) -> u32 {
        0
    }

    /// The resurrect offer's sickness byte (`0xc9eac0`).
    fn resurrect_sickness(&self) -> bool {
        false
    }

    /// The resurrect offer's timer byte (`0xc9eac4`) outside an arena (`0x515a00`).
    fn resurrect_has_timer(&self) -> bool {
        false
    }

    /// The spirit healer asked about is held and within its combat reach plus 4 yd of us
    /// (`0x6d1fc0`).
    fn spirit_healer_in_range(&self) -> bool {
        false
    }

    /// The minutes of sickness a spirit-healer res would apply, `None` for none.
    fn res_sickness_minutes(&self) -> Option<u32> {
        None
    }

    /// `ResurrectGetOfferer` (`0x515950`): the pending offer's caster in the player name cache;
    /// `None` with no offer or before the cache answers.
    fn resurrect_offerer(&self) -> Option<String> {
        None
    }

    /// `HasSoulstone`'s label: the self-res spell's name, or a carried self-res item's.
    fn self_res_label(&self) -> Option<String> {
        None
    }

    /// Our mover falling and not rooted (`0x612430`).
    fn falling(&self) -> bool {
        false
    }

    /// Our `PLAYER_FLAGS` out-of-bounds bit (`0x6123c0`).
    fn out_of_bounds(&self) -> bool {
        false
    }

    /// A live aura effect of ours prevents resurrection (`0x802270`).
    fn cannot_be_resurrected(&self) -> bool {
        false
    }

    /// The summoner's name from the name cache, `""` while unknown (`0x516510`).
    fn summon_summoner(&self) -> String {
        String::new()
    }

    /// The summoner's `AreaTable.dbc` row name, `""` with none (`0x516580`).
    fn summon_area(&self) -> String {
        String::new()
    }

    /// Milliseconds left on the summon offer (`0xbd0864`), 0 when unset or passed.
    fn summon_time_left_ms(&self) -> u32 {
        0
    }

    /// `UnitAura`'s walk over `guid`'s auras at the call (`0x6147c0`): a held unit's listed auras
    /// in display order behind its unit gate, else its roster record's block; `pick` sees each in
    /// order, the first it takes is built with its times on `ui_now`'s clock, and `raid` decides
    /// each one's second-list membership. `Some(None)` for no aura taken.
    fn find_unit_aura(
        &self,
        _guid: u64,
        _ui_now: f64,
        _raid: bool,
        _pick: &mut dyn FnMut(&super::AuraFacts<'_>) -> bool,
    ) -> Option<Option<super::AuraState>> {
        None
    }

    /// `guid`'s raw aura slots at the call, unfiltered, times on `ui_now`'s clock; `Some(None)` for
    /// a unit neither held nor on the roster.
    fn unit_aura_slots(&self, _guid: u64, _ui_now: f64) -> Option<Option<super::UnitAuraSlots>> {
        None
    }

    /// The minimap's tracking spell (`0xbeba68`), a session global the aura handler writes.
    fn tracking_spell(&self) -> Option<u32> {
        None
    }

    /// Write the tracking spell inside the call, as `SetTracking()` zeroes it (`0x57f4c2`); false
    /// where no view is lent.
    fn set_tracking_spell(&mut self, _spell_id: u32) -> bool {
        false
    }

    /// An item spell's range check from us toward `target` at the call (`0x809610`): `Some(None)` with no answer, `None` where no view lends the check.
    fn item_spell_in_range(&self, _spell_id: u32, _target: u64) -> Option<Option<bool>> {
        None
    }

    /// Queue `event` with the frame's unique signals (`0x615020`); false where no queue is lent.
    fn unique_signal(&mut self, _event: &'static str) -> bool {
        false
    }

    /// Open the chat log (`chat`) or the combat log inside `LoggingChat`/`LoggingCombat`'s call when
    /// its flag goes on and it is not open yet (`0x4fbcb1`): whether it is open after, `None` with
    /// no host.
    fn open_log(&mut self, _chat: bool) -> Option<bool> {
        None
    }

    /// Add `text` to the chat log on the catalog row's `chat_type` inside the call (`0x509dd0`): the
    /// line recorded and its event's name and arguments handed back to fire; `None` with no chat log.
    fn add_chat_line(
        &mut self,
        _chat_type: u8,
        _text: &str,
    ) -> Option<(&'static str, Vec<super::ScriptValue>)> {
        None
    }

    /// The channel records at the call (`0xbcf090` count, `0xbcf094` array), holes kept.
    fn channel_slots(&self) -> Vec<Option<super::ChannelSlotInfo>> {
        Vec::new()
    }

    /// A display list or user-list notice writes the channel record's flags byte (`0x503a69`).
    fn set_channel_flags(&mut self, _slot: usize, _flags: u8) {}

    /// The channel pane's state, the game's: a reload keeps it, only the session init clears it
    /// (`0x50f198`-`0x50f202`).
    fn channel_pane(&mut self) -> Option<&mut super::ChannelPane> {
        None
    }

    /// `ChatChannels.dbc` composed for the zone id at the call (`0xbd080c`).
    fn zone_channel_catalog(&self) -> Vec<super::ZoneChannelRow> {
        Vec::new()
    }

    /// Set a channel id's bit in the `ZONECHANNELS` mask (`0xbcf010`).
    fn add_zone_channel_bit(&mut self, _id: u32) {}

    /// The world-state readout's admitted row count at the call (`0x548720`), `None` without one.
    fn world_state_ui_count(&self) -> Option<usize> {
        None
    }

    /// The `index`th admitted readout row resolved at the call (`0x548d40`); `Some(None)` past the end, `None` without a readout.
    fn world_state_ui_info(&self, _index: usize) -> Option<Option<super::WorldStateUiView>> {
        None
    }

    /// One world state's value at the call, a miss reading 0 (`0x549440`'s table); `None` without the table.
    fn world_state(&self, _key: u32) -> Option<i32> {
        None
    }

    /// A quest tooltip's build from the caches at the call (`0x622960`), its title, objectives paragraph and no-progress leaderboard formatted through `get`, each miss asked; `None` where no view lends them.
    fn quest_tooltip(
        &mut self,
        _quest_id: u32,
        _get: &dyn Fn(&str) -> Option<String>,
    ) -> Option<super::QuestTooltipRead> {
        None
    }

    /// A logged quest's POI lines as the slot, the bags and the caches read now (`0x5e58c0`, `0x5e20a0`, `0x5e1ff0`), formatted through `get`; `Some(None)` for a quest not logged or not cached, `None` where no view lends them.
    fn quest_poi_lines(
        &self,
        _quest_id: u32,
        _get: &dyn Fn(&str) -> Option<String>,
    ) -> Option<Option<super::QuestPoiLines>> {
        None
    }

    /// A logged quest's special item as our bags hold it now (`0x5e0da0`): the template's starting item, else under flag `0x20000` a drop item, the first carried copy that can be used; `None` for none.
    fn quest_special_item(&self, _quest_id: u32) -> Option<super::QuestSpecialItemView> {
        None
    }

    /// A logged quest's answers at the call, computed from its slot and the caches (`0x5e0000`, `0x5e0070`, `0x5e0d20`, `0x5e0ea0`, `0x5e1460`), lines formatted through `get`, its detail left out; `Some(None)` for a quest not cached, `None` where no view lends them.
    fn quest_log_row(
        &mut self,
        _quest_id: u32,
        _slot: u8,
        _get: &dyn Fn(&str) -> Option<String>,
    ) -> Option<Option<super::QuestLogEntryView>> {
        None
    }

    /// A logged quest's detail pane at the call; `Some(None)` for a quest not cached, `None` where no view lends it.
    fn quest_log_detail(
        &mut self,
        _quest_id: u32,
        _slot: u8,
    ) -> Option<Option<super::QuestLogDetail>> {
        None
    }

    /// The daily-quest fields holding a quest at the call (`0x58db30`), `None` where no view lends them.
    fn daily_quests_completed(&self) -> Option<u32> {
        None
    }

    /// `GetQuestTimers`' expiry of a run-out timer inside the call (`0x5e634a`): marks the quest expired and asks every questgiver's status again. `Some(Some(title))` for a new mark whose template is cached, `Some(None)` for a quest already marked or one whose line waits on its template's answer (`0x5e635c`); `None` where no view lends it.
    fn expire_quest(&mut self, _quest_id: u32) -> Option<Option<String>> {
        None
    }

    /// The server's unix seconds now, fractional: the local clock offset by the last time response (`0x5dff40`), which the timer verbs subtract (`0x5e4320`); `None` before one.
    fn server_unix_time(&self) -> Option<f64> {
        None
    }

    /// The daily quest reset in server unix seconds as the last time response placed it (`0x5dff40`), which `GetQuestResetTime` counts down to (`0x5e6de0`); `None` before one.
    fn quest_reset_unix(&self) -> Option<f64> {
        None
    }

    /// The rebuild's tail sends in its order: one `CMSG_QUEST_POI_QUERY` naming `pois` when any (`0x5e6cc9`, `0x5e6cd1`), then `CMSG_QUERY_TIME` when `query_time` (`0x5e6d79`).
    fn send_quest_log_tail(&mut self, _pois: &[u32], _query_time: bool) {}

    /// `GetQuestResetTime`'s first step (`0x5e6dee`-`0x5e6e00`): with the time response's re-ask due, the log's rebuild runs, its tail asking the time again (`0x5e6d79`); its rows to write, `None` when not due, the rebuild skipped or no view lends it.
    fn rebuild_quest_log_if_reask_due(&mut self) -> Option<super::QuestLogPlan> {
        None
    }

    /// A questgiver panel row's item from the item cache at the call (`0x58d190`); `Some(None)` while it is not cached, `None` where no view lends the cache.
    fn quest_item_record(&self, _item_id: u32) -> Option<Option<super::QuestItemRecord>> {
        None
    }

    /// A questgiver button's call (`0x58cd20`, `0x58ce00`, `0x58d350`, `0x58cf40`, `0x58cfa0`): its send and the window's close inside the call; `None` where no view lends the giver.
    fn quest_action(&mut self, _action: super::QuestAction) -> Option<super::QuestActOutcome> {
        None
    }

    /// `SelectAvailableQuest` (`0x58cc20`) or `SelectActiveQuest` (`0x58ccb0`) inside the call; `None` where no view lends the giver.
    fn quest_select(&mut self, _select: super::QuestSelect) -> Option<super::QuestActOutcome> {
        None
    }

    /// The `"questnpc"` unit's guid (`0x6081b0`): the open quest window's inform guid or source; `None` with the window closed or no view lending the giver.
    fn quest_npc(&self) -> Option<u64> {
        None
    }

    /// The interact target (`0xbd07a8`), the `"npc"` unit's guid; `None` with none held.
    fn interact_target(&self) -> Option<u64> {
        None
    }

    /// The carried-count check (`0x754dd0`) on `count` more of `item_id`, its record cached: why it refuses, `None` when it passes, the record is not cached or no view lends the bags.
    fn carry_refusal(&self, _item_id: u32, _count: u32) -> Option<super::CarryRefusal> {
        None
    }

    /// How many of `item_id` our bags hold for a quest at the call (`0x58cbed`-`0x58cbfb`), `None` where no view lends them.
    fn carried_quest_item(&self, _item_id: u32) -> Option<u32> {
        None
    }

    /// Queue slot `index` (`0xbea4b8`) at the call, `None` off the two.
    fn battlefield_slot(&self, index: usize) -> Option<super::BattlefieldQueueSlot> {
        (index < 2).then(Default::default)
    }

    /// The active instance's globals at the call (`0xacd16c`, `0xbea570`).
    fn battlefield_instance(&self) -> super::BattlefieldInstance {
        super::BattlefieldInstance::default()
    }

    /// The world-PvP queue (`0xbea4a8`) at the call.
    fn world_pvp_queue(&self) -> super::WorldPvpQueueView {
        super::WorldPvpQueueView::default()
    }

    /// The current area's flags carry `0x8000000` (`0x54c810`).
    fn can_hearth_and_resurrect(&self) -> bool {
        false
    }

    /// The first of `AcceptBattlefieldPort`'s refusals we now meet (`0x54da10`).
    fn battlefield_port_refusal(&self) -> Option<&'static str> {
        None
    }

    /// `BattlemasterList.dbc`'s row `id` resolved for the player at the call.
    fn battlemaster_row(&self, _id: u32) -> Option<super::BattlemasterRow> {
        None
    }

    /// The battleground rows the type list sorts (`0x54a650`).
    fn battleground_types(&self) -> Vec<super::BattlegroundType> {
        Vec::new()
    }

    /// A scoreboard row's name-cache record when called (`0x54be90`), `None` while uncached.
    fn score_name(&self, _guid: u64) -> Option<super::ScoreName> {
        None
    }

    /// The scoreboard has pending names (`0xbea590`): the rebuild and sort wait (`0x54aa39`, `0x54cee3`).
    fn battlefield_score_names_pending(&self) -> bool {
        false
    }

    /// The leading arena slots holding an opponent or a pet (`0x549630`), 0 without slots.
    fn arena_opponent_count(&self) -> usize {
        0
    }

    /// Our honor block when called, `None` before its private fields stream.
    fn honor(&self) -> Option<super::HonorState> {
        None
    }

    /// The PvP toggle timer (`0x6de7a0`) when called, `None` without the active player.
    fn pvp_toggle_timer(&self) -> Option<i32> {
        None
    }

    /// Our arena-team slot's seven field words when called, zeros without them.
    fn arena_team_info(&self, _slot: usize) -> [u32; 7] {
        [0; 7]
    }

    /// Our arena-team slot's last `SMSG_ARENA_TEAM_STATS` when called.
    fn arena_team_stats(&self, _slot: usize) -> [u32; 6] {
        [0; 6]
    }

    /// Our arena-team slot's last roster team type when called (`0x5a3ea4`), 0 before a roster.
    fn arena_team_roster_type(&self, _slot: usize) -> u32 {
        0
    }

    /// Our arena-team slot's roster length when called.
    fn arena_team_member_count(&self, _slot: usize) -> usize {
        0
    }

    /// Our arena-team slot's roster member `index` in wire order when called.
    fn arena_team_member(
        &self,
        _slot: usize,
        _index: usize,
    ) -> Option<super::ArenaTeamMemberRef<'_>> {
        None
    }

    /// The team cache's record for `team_id` when called, asked on a miss.
    fn arena_team_record(&self, _team_id: u32) -> Option<super::ArenaTeamRecordView> {
        None
    }

    /// The positions list's entries outside our party and raid (`0x54a040`).
    fn battlefield_position_count(&self) -> usize {
        0
    }

    /// The `index`th entry outside our party and raid (`0x54c2e0`), its name asked on a miss.
    fn battlefield_position(&self, _index: usize) -> Option<super::BattlefieldPositionLive> {
        None
    }

    /// The flag carriers held, 2 when the second is (`0x54a0e0`).
    fn battlefield_flag_count(&self) -> usize {
        0
    }

    /// The `index`th flag carrier's sources and token (`0x54dcc0`).
    fn battlefield_flag(&self, _index: usize) -> Option<super::BattlefieldFlagLive> {
        None
    }

    /// The active map's `MinimapIconScale`, 1 without a row (`0x54c740`).
    fn battlefield_icon_scale(&self) -> f32 {
        1.0
    }

    /// `RequestBattlefieldPositions` (`0x54cf60`): cleared without an active slot, else sent at most every 5000 ms.
    fn request_battlefield_positions(&self) {}

    /// `NotifyInspect`'s `CMSG_INSPECT` and target (`0x5228c3`, `0x52291a`); a moved target resets the inspected talent tabs to its class (`0x5e76f2`); whether it moved (`0x5e76c1`).
    fn notify_inspect(&mut self, _guid: u64) -> bool {
        true
    }

    /// `ClearInspectPlayer`'s target clear (`0x5e78ae`); a held one resets the inspected talent tabs (`0x5e78d5`).
    fn clear_inspect_target(&mut self) {}

    /// Companion list `ty`'s spells in order (`0xbe8df4`), as the known-spell adds file them.
    fn companion_spells(&self, _ty: super::CompanionType) -> Vec<u32> {
        Vec::new()
    }

    /// `GetCompanionInfo`'s companion at `index` of list `ty`, read when called (`0x53e2c0`).
    fn companion(&self, _ty: super::CompanionType, _index: usize) -> Option<super::CompanionView> {
        None
    }

    /// The known spells of skill line 778 `SummonRandomCritter` draws from (`0x6e7b00`).
    fn companion_random_pool(&self) -> Vec<u32> {
        Vec::new()
    }

    /// The pet stat verbs' block for the pet list's head (`0x5d3960`), `HasPetUI`'s first return beside it.
    fn pet_stats(&self) -> (bool, super::PetStats) {
        (false, super::PetStats::default())
    }

    /// `(PetCanBeAbandoned, PetCanBeRenamed, PetCanBeDismissed)` for the pet list's head (`0x5d3780`, `0x5d38c0`, `0x5d3820`).
    fn pet_menu(&self) -> (bool, bool, bool) {
        (false, false, true)
    }

    /// A held unit's `CreatureFamily.dbc` name off its cached creature record (`0x611820`).
    fn creature_family(&self, _guid: u64) -> Option<String> {
        None
    }

    /// The possess spell `[0xc234e0]`'s record when called, `None` while none is held.
    fn possess(&self) -> Option<super::PossessView> {
        None
    }

    /// The stable globals the list handler writes (`0x5a11a0`).
    fn stable(&self) -> super::StableList {
        super::StableList::default()
    }

    /// Creature `entry`'s family parts; a miss with a `callback` asks the record and queues the callback's event for its answer (`0x5a13c1`), with none asks nothing (`0x67b6d0`).
    fn stable_family(
        &mut self,
        _entry: u32,
        _callback: Option<super::StableCallback>,
    ) -> Option<super::StableFamily> {
        None
    }

    /// The pet held as a hunter's unit with its creature record, the paperdoll's live arm (`0x5a15df`-`0x5a160a`).
    fn stable_live_pet(&self) -> bool {
        false
    }

    /// `StableSlotPrices.dbc` row `slot`'s price (`0x5a14d0`).
    fn stable_slot_price(&self, _slot: u32) -> Option<u32> {
        None
    }

    /// Send `verb` to the held stable master, or to guid 0 with none.
    fn stable_send(&self, _verb: super::StableIntent) {}

    /// The stable close (`0x5a1971`-`0x5a19aa`): our open-stable auras cancelled and the master cleared; whether one was held, so the caller fires `PET_STABLE_CLOSED`.
    fn close_stables(&mut self) -> bool {
        false
    }

    /// The pet list holds a pet, the stable drop's fork (`0x5a1ca0`).
    fn pet_out(&self) -> bool {
        false
    }

    /// Our charm or summon field is set, `UnstablePet`'s refusal (`0x5a1ac0`).
    fn charm_or_summon(&self) -> bool {
        false
    }

    /// A held unit's pet, `UNIT_FIELD_CHARM`, else `UNIT_FIELD_SUMMON`, as a token's `pet` hop
    /// reads it (`0x60ab9a`); `None` for a guid that is no held unit or has no pet.
    fn unit_pet(&self, _guid: u64) -> Option<u64> {
        None
    }

    /// The guid a token's base names at the call (`0x60abf0`), `Some(0)` for nobody; `None` where no view lends the resolver's state. `focus`, `npc` and `questnpc` are read apart.
    fn unit_base(&self, _base: super::UnitBase) -> Option<u64> {
        None
    }

    /// The guid `base` then `hops` name (`0x60abf0`, hops `0x60aaa0`) in one read, `focus` the VM's; `Some(None)` sends the token to the name lookup; `None` where no view lends the resolver's state.
    fn unit_token(
        &self,
        _base: super::UnitBase,
        _hops: super::Hops<'_>,
        _focus: u64,
    ) -> Option<Option<u64>> {
        None
    }

    /// The hops from `guid` (`0x60aaa0`) in one read, `Some(None)` where they fail; `None` where no view lends the resolver's state.
    fn unit_hops(&self, _guid: u64, _hops: super::Hops<'_>) -> Option<Option<u64>> {
        None
    }

    /// Every base's guid and each numbered base's rows at the call, in one read; `None` where no view lends them.
    fn unit_tables(&self) -> Option<super::UnitGuids> {
        None
    }

    /// An open-stable aura is on us (`0x5a19c0`).
    fn open_stable_aura(&self) -> bool {
        false
    }

    /// The pet bar's bar-wide state at the call, `None` with no pet info to read.
    fn pet_bar(&self) -> Option<super::PetBarLive> {
        None
    }

    /// Pet bar slot `index` (0-based) resolved at the call, its cooldown on the `GetTime` clock that `ui_now` seconds names at `anchor`; `None` past the ten words or with no bar.
    fn pet_action(
        &self,
        _index: usize,
        _anchor: std::time::Instant,
        _ui_now: f64,
    ) -> Option<super::PetActionView> {
        None
    }

    /// The drag's write into the pet's words and its send, `(0-based slot, word)` pairs in one packet (`0x5d5c3e`, `0x5d5cda`); whether it landed.
    fn pet_place_action(&mut self, _entries: &[(u32, u32)]) -> bool {
        false
    }

    /// `PetStopAttack`'s clear (`0x5d4650`): with the attacking global up, send the stop, lower it.
    fn pet_stop_attack(&mut self) {}

    /// A pet menu verb inside its call (`0x5d4900`, `0x5d46f0`, `0x5d5670`): `None` leaves it to the queue.
    fn pet_menu_verb(&mut self, _verb: super::PetMenuVerb<'_>) -> Option<super::PetMenuOutcome> {
        None
    }

    /// A press on 1-based `slot`, or the order word `order`, that writes only the bar (a reaction or a command other than Attack), latched inside the call (`0x5d4210`): `None` leaves it to the script call, else the latch, whose send waits for its signal (`0x5d4556`).
    fn pet_press(&mut self, _slot: u32, _order: Option<u32>) -> Option<super::PetLatch> {
        None
    }

    /// The latched press's `CMSG_PET_ACTION`, under the pet held at the send (`0x5d4585`).
    fn pet_press_send(&mut self, _word: u32, _target: u64) {}

    /// The bar toggle on 0-based slot `index` (`0x5d5d20`): past the usability gate (`0x5d5d3c`) a slot with bit 31 flips (`0x5d5d88`) and is sent (`0x5d5deb`); whether the gate passed.
    fn pet_toggle_autocast(&mut self, _index: u32) -> bool {
        false
    }

    /// The book's set (`0x5d5e10`), `None` flipping: whether it flipped and sent (`0x5d5fdc`).
    fn pet_set_spell_autocast(&mut self, _spell_id: u32, _state: Option<bool>) -> bool {
        false
    }

    /// The pet's spellbook at the call (`[0xbe7d98]`), cooldowns on the `GetTime` clock that `ui_now` seconds names at `anchor`; `None` with no pet book to read.
    fn pet_book(&self, _anchor: std::time::Instant, _ui_now: f64) -> Option<super::PetBookState> {
        None
    }

    /// The player's book as the last book update built it (`0x53ca70`); `None` with none lent.
    fn player_book(&self) -> Option<std::sync::Arc<super::PlayerBook>> {
        None
    }

    /// Whether the player knows `spell_id`, the known-spell bits (`0xbe6d88`, `0x7260e0`).
    fn spell_known(&self, _spell_id: u32) -> bool {
        false
    }

    /// Whether the player knows `spell_id` or a higher rank of it (`0x7260e0`, `0x726160`).
    fn spell_known_or_higher(&self, spell_id: u32) -> bool {
        self.spell_known(spell_id)
    }

    /// Whether the pet knows `spell_id` or a higher rank of it, the talent tooltip's pet side
    /// (`0x5c60c0`): our player and the pet list's head held (`0x5c60f4`, `0x5c6120`), then the
    /// pet's list (`0x7260e0` into `0x53b900`) or its family's rank chain (`0x726160`).
    fn pet_spell_known_or_higher(&self, _spell_id: u32) -> bool {
        false
    }

    /// A player book spell's texture at the call (`0x53dc20`): the weapon faces for the melee
    /// auto-attack and a ranged shot (`0x5a84d0`, `0x5a85e0`), else the record's icon.
    fn book_spell_texture(&self, spell_id: u32) -> Option<String> {
        super::spellbook::live_book()
            .book
            .named()
            .find(|s| s.spell_id == spell_id)?
            .texture
            .clone()
    }

    /// A player book spell's selected test at the call (`0x53d350`): the open trade-skill line's
    /// spell, or a shapeshift into the player's current form.
    fn spell_selected(&self, _spell_id: u32) -> bool {
        false
    }

    /// The bar's usable update at the call (`0x5aa470`), with whether equipment sets can be used (`0x5ad6b0`): whether a slot's bytes moved, `None` with no bar lent.
    fn bar_usable_update(&mut self, _sets_usable: bool) -> Option<bool> {
        None
    }

    /// `IsUsableAction`'s pair for the 1-based `action` off the bar's usable cache (`0x5a7e60`);
    /// `None` without a view.
    fn action_usable(&self, _action: u32) -> Option<(bool, bool)> {
        None
    }

    /// `IsCurrentAction` for the 0-based bar `slot` at the call (`0x5aa240`); `None` without a view.
    fn action_current(&self, _slot: u32) -> Option<bool> {
        None
    }

    /// `IsAutoRepeatAction` for the 0-based bar `slot` at the call (`0x5a9470`).
    fn action_auto_repeat(&self, _slot: u32) -> Option<bool> {
        None
    }

    /// `IsActionInRange` for the 0-based bar `slot` at `target`, 0 for the locked target
    /// (`0x5a94c0`): `Some(None)` where the check does not apply.
    fn action_in_range(&self, _slot: u32, _target: u64) -> Option<Option<bool>> {
        None
    }

    /// `ActionHasRange` for the 0-based bar `slot` (`0x5a95e0`).
    fn action_has_range(&self, _slot: u32) -> Option<bool> {
        None
    }

    /// `IsAttackAction` for the 0-based bar `slot` (`0x5a96d0`).
    fn action_attack(&self, _slot: u32) -> Option<bool> {
        None
    }

    /// `IsEquippedAction` for the 0-based bar `slot` (`0x5a88b0`).
    fn action_equipped(&self, _slot: u32) -> Option<bool> {
        None
    }

    /// `GetActionCooldown` for the 0-based bar `slot` (`0x5a8e40`): `(start_ms, duration_ms,
    /// enabled)` on the `GetTime` clock that `ui_now` seconds names at `anchor`, `(0, 0, false)`
    /// for a slot naming nothing; `None` without a view.
    fn action_cooldown(
        &self,
        _slot: u32,
        _anchor: std::time::Instant,
        _ui_now: f64,
    ) -> Option<(i64, u32, bool)> {
        None
    }

    /// The 0-based bar `slot`'s word (`[0xc1e358 + slot * 4]`), 0 for an empty slot; `None`
    /// without a view.
    fn action_word(&self, _slot: u32) -> Option<u32> {
        None
    }

    /// `GetActionTexture` for a spell, item or pet-action slot (`0x5a97f0`), resolved at the call;
    /// `Some(None)` for no icon.
    fn action_texture(&self, _slot: u32) -> Option<Option<super::ActionIcon>> {
        None
    }

    /// `GetActionCount` (`0x5a7d10`): an item's count (`0x5a8a60`), a spell's reagent uses
    /// (`0x801770`).
    fn action_count(&self, _slot: u32) -> Option<u32> {
        None
    }

    /// `IsConsumableAction` (`0x5a9360`).
    fn action_consumable(&self, _slot: u32) -> Option<bool> {
        None
    }

    /// `IsStackableAction` (`0x5a8470`).
    fn action_stackable(&self, _slot: u32) -> Option<bool> {
        None
    }

    /// A pet-action slot's spell (`0x5a80e0`): `Some(None)` for a mode, an order or an empty word.
    fn action_pet_spell(&self, _slot: u32) -> Option<Option<u32>> {
        None
    }

    /// The temporary bar's owners (`[0xc1e5a0]`); `None` without a view.
    fn temp_action_bar(&self) -> Option<u32> {
        None
    }

    /// The bonus offset (`[0xc1e59c]`); `None` without a view.
    fn bonus_bar_offset(&self) -> Option<u8> {
        None
    }

    /// Whether the book holds Cold Weather Flying (`0x7261b0`); `None` without a view.
    fn knows_cold_weather_flying(&self) -> Option<bool> {
        None
    }

    /// The slot write (`0x5aae80`, `0x5aaa90` for 0): the word stored, the slot's usable bytes
    /// recomputed and the slot sent (`0x5aa390`); `Some(false)` while the bar is locked
    /// (`0x5aaec1`, `0x5aaac2`), `None` without a view. The caller signals the slot.
    fn set_action_word(&mut self, _slot: u32, _word: u32) -> Option<bool> {
        None
    }

    /// A macro verb's binding change (`0x565c40` -> `0x5650c0`, `0x566d50`), stored for the bar; every binding returned by index. `None` without a view.
    fn macros_changed(
        &mut self,
        _macros: &super::MacroState,
        _change: super::MacroChange,
    ) -> Option<std::collections::HashMap<u32, super::MacroBinding>> {
        None
    }

    /// `IsUsableItem`'s pair for `item_id` (`0x51c450`); `None` without a view.
    fn item_usable(&self, _item_id: u32) -> Option<(bool, bool)> {
        None
    }

    /// The stance bar as `GetShapeshiftFormInfo` reads it at the call (`0x53de90`): the cached list
    /// in bar order, each entry's active test and icon read now, castable off its cache; `None`
    /// without a view. The cooldown is read apart, as the spell's.
    fn shapeshift_forms(&self) -> Option<Vec<super::ShapeshiftFormView>> {
        None
    }

    /// The player's shapeshift form byte now, 0 unshifted; `None` without a view.
    fn shapeshift_form_byte(&self) -> Option<u8> {
        None
    }

    /// The spell tooltip builder's view of `spell_id` at the call, against the player or with
    /// `alt_caster` its pet, its keyed cells filled through `get`; `None` without a row.
    fn spell_tooltip(
        &self,
        _spell_id: u32,
        _alt_caster: bool,
        _get: &dyn Fn(&str) -> Option<String>,
    ) -> Option<super::SpellTooltipView> {
        None
    }

    /// `f` over the spell source that reads the casters and the player's cast state at the call;
    /// a view that has none leaves `f` uncalled, and the verb asks the source installed at load.
    fn with_spell_source(&self, _f: &mut dyn FnMut(&dyn super::SpellInfoSource)) {}

    /// The player's cooldown history for `spell_id` at the call, as `GetCompanionCooldown` asks it (`0x53e54c`): `(start_ms, duration_ms, enabled)` on the `GetTime` clock that `ui_now` seconds names at `anchor`; `None` with nothing running.
    fn spell_cooldown(
        &self,
        _spell_id: u32,
        _anchor: std::time::Instant,
        _ui_now: f64,
    ) -> Option<(i64, u32, bool)> {
        None
    }

    /// The targeting cursor at the call: up, wanting an item, and its arm count.
    fn spell_targeting(&self) -> super::SpellTargetingLive {
        super::SpellTargetingLive::default()
    }

    /// `SpellCanTargetUnit`'s unit leg toward the held unit `guid` at the call (`0x804190`): the standing word binds it with no range refusal; false while not targeting.
    fn spell_can_target_unit(&self, _guid: u64) -> bool {
        false
    }

    /// A unit's cast, or with `channel` its channel, as the unit holds it at the call (`+0xa6c`-`+0xa7c`, `+0xa80`-`+0xa88`), with the not-interruptible test's answer (`0x71ab20`); `None` with none.
    fn unit_cast(&self, _guid: u64, _channel: bool) -> Option<super::UnitCast> {
        None
    }

    /// `SpellStopCasting`'s test at the call: the auto-repeating spell is set, else a cast is in flight (`0x7fe130`, `0x805f30`); never a channel.
    fn has_stoppable_cast(&self) -> bool {
        false
    }

    /// The totem table's live slot `slot` (0-based) at the call (`0xbd0b00`), `None` for an empty one.
    fn totem(&self, _slot: usize) -> Option<super::TotemView> {
        None
    }

    /// An empty totem slot's flag: the player held and a carried item covering the slot's `TotemCategory` row, the bag search run at the call (`0x51d42f`, `0x51d457`-`0x51d496`).
    fn totem_tool(&self, _slot: usize) -> bool {
        false
    }

    /// Destroy the live slot `slot` inside the call: `CMSG_TOTEM_DESTROYED`, then the slot cleared (`0x5197d0`); false for an empty slot, which sends nothing. The caller fires `PLAYER_TOTEM_UPDATE`.
    fn destroy_totem(&mut self, _slot: usize) -> bool {
        false
    }

    /// The totem bar's element list `element` (0 fire, 1 earth, 2 water, 3 air) at the call (`0xbe8ea0`).
    fn multicast_list(&self, _element: usize) -> Vec<u32> {
        Vec::new()
    }

    /// `spell`'s two `TotemCategory` rows off its record (`0x5a7bc0`), `None` for no record.
    fn spell_totem_categories(&self, _spell: u32) -> Option<[u32; 2]> {
        None
    }

    /// The `TotemCategory.dbc` row's name, `ERR_MULTI_CAST_ACTION_TOTEM_S`'s argument (`0x5ab24b`).
    fn totem_category_name(&self, _row: u32) -> Option<String> {
        None
    }

    /// The rune table (`0xc24304`-`0xc24388`) and the active player's `PLAYER_RUNE_REGEN` rates, read at the call; `None` without the active player object (`0x61306f`).
    fn runes(&self) -> Option<(super::RuneState, super::RuneRegen)> {
        None
    }

    /// The open bank's banker (`0xbeb9a0`), `None` with no bank open.
    fn banker(&self) -> Option<u64> {
        None
    }

    /// The active player's purchased bank bag slots, `PLAYER_BYTES_2` byte 2 (`0x57badb`); `None` with no active player (`0x57bae1`).
    fn bank_bag_slots(&self) -> Option<u8> {
        None
    }

    /// `BankBagSlotPrices.dbc` row `slot`'s cost, 0 with no row (`0x57ba30`).
    fn bank_slot_cost(&self, _slot: u32) -> u32 {
        0
    }

    /// `PurchaseSlot`'s gates and send (`0x57bbc0`): the refusal line's key, `None` once sent or gated silently.
    fn purchase_bank_slot(&self) -> Option<&'static str> {
        None
    }

    /// The interaction close's bank leg when the interact target is the banker (`0x518d50` -> `0x512fa7`): the target zeroed, whether it was.
    fn close_bank_interaction(&mut self) -> bool {
        false
    }

    /// The interact target's clear on `guid` inside the call (`0x518d50` with 0): the target zeroed when it held `guid`, whether it did; the caller runs its window's leg.
    fn clear_interact_target(&mut self, _guid: u64) -> bool {
        false
    }

    /// The banker cleared, the banker setter's tail with no banker (`0x57b8f9`).
    fn clear_banker(&mut self) {}

    /// The client's loaded item tables, `None` before their catalogs load.
    fn item_tables(&self) -> Option<&super::ItemTables> {
        None
    }

    /// The active player's level, class, race, honor rank, medals and class flags; `None` with no active player.
    fn player_fields(&self) -> Option<super::PlayerFields> {
        None
    }

    /// The active player's level (`UNIT_FIELD_LEVEL`); `None` with no active player.
    fn player_level(&self) -> Option<u32> {
        self.player_fields().map(|r| r.level)
    }

    /// The active player's value plus permanent bonus in `skill` (`0x51a250`); `Some(None)` for a skill not held, `None` with no active player.
    fn player_skill(&self, _skill: u32) -> Option<Option<u32>> {
        None
    }

    /// The proficiency mask for item class `class`; `Some(None)` for a class never sent, `None` with no active player.
    fn player_proficiency(&self, _class: u32) -> Option<Option<u32>> {
        None
    }

    /// The active player's reputation rank with `faction`; `None` with no active player.
    fn player_rep_rank(&self, _faction: u32) -> Option<u8> {
        None
    }

    /// Whether the spell book holds a dual-wield spell; `None` with no active player.
    fn player_can_dual_wield(&self) -> Option<bool> {
        None
    }

    /// `CurrencyTypes.dbc` and `CurrencyCategory.dbc`, `None` before they load.
    fn currency_catalog(&self) -> Option<&super::CurrencyCatalogView> {
        None
    }

    /// The active player's currencies at the call: the known bits, the token slots' counts, the two point balances and each known row's cached item name and icon; `None` with no active player.
    fn currency_holdings(&self) -> Option<super::CurrencyHoldings> {
        None
    }

    /// Ask for item `id`'s record when the cache holds none, as a cache read with a landing callback does (`0x5b01ef`).
    fn ask_item_record(&self, _id: u32) {}

    /// Leave `wait` on item `id`'s record, at the tail of its callbacks across every subsystem (`0x67cab1`); whether a view took it.
    fn wait_item_record(&mut self, _id: u32, _wait: super::ItemRecordWait) -> bool {
        false
    }

    /// Item `id`'s record read from the item cache at the call (`0x67ca30`), its catalog names resolved now; `tooltip`, the VM's strings, adds the lines only the tooltip prints, the trigger spells' `$`-text and the reputation requirement. `Some(None)` for a record not cached, `None` where no view lends the cache.
    fn item_record(
        &self,
        _id: u32,
        _tooltip: Option<GlobalStrings<'_>>,
    ) -> Option<Option<super::ItemTemplateView>> {
        None
    }

    /// The bind point's area name at the call, through the catalog the `$z` token reads; `None` where no view lends it.
    fn bind_location(&self) -> Option<String> {
        None
    }

    /// Container `bag` of the active player at the call (`0x5d7380`): `Some(None)` for an empty bag slot or an id past the eleven bags, `None` with no active player. Cooldowns on the `GetTime` clock that `ui_now` seconds names at `anchor`.
    fn container(
        &self,
        _bag: i64,
        _anchor: std::time::Instant,
        _ui_now: f64,
    ) -> Option<Option<super::ContainerState>> {
        None
    }

    /// One slot of container `bag` at the call, 1-based, without building the rest of the bag; `Some(None)` for an empty or missing slot, `None` with no active player.
    fn container_slot(
        &self,
        bag: i64,
        slot: u32,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> Option<Option<super::ContainerSlot>> {
        self.container(bag, anchor, ui_now)
            .map(|c| c.and_then(|mut c| c.slots.remove(&slot)))
    }

    /// Container `bag` as a Lua bag index names it (`0x5d7380` -> `0x5d6f20`): bags 1..11 through the watched-bag array, the bank's only with a banker set; else as [`Self::container`].
    fn indexed_container(
        &self,
        bag: i64,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> Option<Option<super::ContainerState>> {
        self.container(bag, anchor, ui_now)
    }

    /// One slot of container `bag` as [`Self::indexed_container`] names the bag.
    fn indexed_container_slot(
        &self,
        bag: i64,
        slot: u32,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> Option<Option<super::ContainerSlot>> {
        self.indexed_container(bag, anchor, ui_now)
            .map(|c| c.and_then(|mut c| c.slots.remove(&slot)))
    }

    /// Whether the keyring holds a key (`0x51ae80`); `None` with no active player.
    fn has_key(&self) -> Option<bool> {
        None
    }

    /// The played clock at the call (`0x6cf440`): the last answer's total plus the seconds since;
    /// `None` where no view lends it.
    fn played_now(&self) -> Option<i64> {
        None
    }

    /// `CMSG_PLAYED_TIME` asking for a shown answer, sent in the call (`0x5158c1`).
    fn send_played_time(&mut self) {}

    /// The refund record on held item `item` (`item+0x3d4`, filled by `0x6d1650`); `Some(None)` for none.
    fn refund_record(&self, _item: u64) -> Option<Option<super::RefundView>> {
        None
    }

    /// A bound item's trade-window stamp read off its fields at the call (`0x708b40`); `Some(None)` when it has none.
    fn bound_trade_stamp(&self, _item: u64) -> Option<Option<u32>> {
        None
    }

    /// The item swap's gate over its two ends, each `(bag, 1-based slot)` (`0x6df890`); `None` with no active player held.
    fn swap_gate(&self, _src: (i64, u32), _dst: (i64, u32)) -> Option<super::SwapGate> {
        None
    }

    /// `AutoEquipCursorItem`'s gate over the held item at `(bag, 1-based slot)` and whether it is ammunition (`0x6dfd45`-`0x6dfe51`); `None` with no active player held.
    fn auto_equip_gate(&self, _bag: i64, _slot: u32) -> Option<(super::SwapGate, bool)> {
        None
    }

    /// `f` over the client's faction list (`0xc23370`-`0xc2347c`) and the faction table and reputation store it reads, at the call; `false` where no view lends them.
    fn reputation(
        &mut self,
        _f: &mut dyn FnMut(&mut super::FactionList, &mut dyn super::FactionTable),
    ) -> bool {
        false
    }

    /// The achievement tables the game start indexes (`0x5b6df0`), `None` before they load.
    fn achievement_db(&self) -> Option<std::sync::Arc<super::AchievementDb>> {
        None
    }

    /// The client's achievement lists at the call (`0xacfd98`-`0xacfde0`, `[0xc1f454]`, `[0xc1f458]`).
    fn achievement_book(&self) -> Option<&super::AchievementBook> {
        None
    }

    /// The achievement lists, for a binding that writes them in its call (the tracked list, the compared guid).
    fn achievement_book_mut(&mut self) -> Option<&mut super::AchievementBook> {
        None
    }

    /// A held player's PvP faction index (`0x6d6e90`), `None` for a guid that is no held player
    /// (the lookup with the player type mask, `0x5b3d0a`, `0x5b3bf0`).
    fn player_pvp_faction(&self, _guid: u64) -> Option<i32> {
        None
    }

    /// A player's name-cache record read with no callback, which asks nothing (`0x5b58b0`'s owner name).
    fn cached_player_name(&self, _guid: u64) -> Option<String> {
        None
    }

    /// A player's name-cache record read with a callback: the name, or `None` with the query sent (`0x627220`).
    fn player_name_or_ask(&mut self, _guid: u64) -> Option<String> {
        None
    }

    /// `CMSG_QUERY_INSPECT_ACHIEVEMENTS` for `guid`, sent in the call (`0x5b1ab0`).
    fn query_inspect_achievements(&mut self, _guid: u64) {}

    /// `CharTitles.dbc`'s row count (`[0xad337c]`), 0 before the table loads.
    fn char_title_count(&self) -> usize {
        0
    }

    /// The `CharTitles.dbc` row whose mask id is `mask`, the first in file order (`0x719900`).
    fn char_title(&self, _mask: i64) -> Option<super::TitleRow> {
        None
    }

    /// The active player's title fields at the call, `None` with no active player held (`0x51b440`).
    fn player_titles(&self) -> Option<super::PlayerTitles> {
        None
    }

    /// `CMSG_SET_TITLE` with `mask`, sent in the call (`0x522efb`).
    fn send_set_title(&mut self, _mask: i32) {}

    /// The active player's sheet figures at the call, its fields, worn weapons and skill pairs, read where 12340's stat verbs read them (`0x610300`, `0x60ddb0`); `None` with no active player held.
    fn player_combat_stats(&self) -> Option<super::UnitCombatStats> {
        None
    }

    /// The active player's doll id `slot` (0 the ammo, 1..=23 the worn slots and equipped bags) at the call, its slot field and item object as the slot reader resolves them (`0x5e7c00`); `Some(None)` for an empty slot, `None` with no active player held.
    fn inventory_slot(&self, _slot: usize) -> Option<Option<super::InvSlotView>> {
        None
    }

    /// The active player's bank bag slot `i` (0..7, live ids 68..=74) at the call (`0x5e7c00`); `Some(None)` for an empty slot, `None` with no active player held.
    fn bank_bag_slot(&self, _i: usize) -> Option<Option<super::InvSlotView>> {
        None
    }

    /// Both weapons' temporary enchantment at the call, main hand then off hand (`0x5eaca0`); `None` with no active player held.
    fn weapon_enchants(&self) -> Option<[Option<super::WeaponEnchant>; 2]> {
        None
    }

    /// The active player's skill slot for line `skill_id` at the call, as `GetSkillLineInfo` reads it (`0x5cdfe7`), slot -1 for a line the player lacks (`0x6dc222`); `None` with no active player held.
    fn skill_slot(&self, _skill_id: u32) -> Option<super::SkillLive> {
        None
    }

    /// A line's `(rank, max)` as the trade-link builder reads the active player at the call (`0x5db120`-`0x5db134`); `None` with no active player held (`0x5db08f`).
    fn skill_link_ranks(&self, _skill_id: u32) -> Option<(u32, u32)> {
        None
    }

    /// The client's skill list at the call (`0xc22ae8`-`0xc22b18`), which a UI reload keeps; `None` where no view lends it.
    fn skill_list(&self) -> Option<&super::SkillList> {
        None
    }

    /// The skill list, for a binding that writes it in its call (the fold, the selection, the skill-ups).
    fn skill_list_mut(&mut self) -> Option<&mut super::SkillList> {
        None
    }

    /// The active player's `PLAYER_CHARACTER_POINTS2` at the call (`0x5cd970`), `None` with no active player held.
    fn skill_points(&self) -> Option<u32> {
        None
    }

    /// `CMSG_UNLEARN_SKILL` for `skill_id`, sent in the call (`0x5ce368`).
    fn send_unlearn_skill(&mut self, _skill_id: u32) {}

    /// A skill buy, sent in the call (`0x5ce686`, `0x5ce849`).
    fn send_skill_request(&mut self, _request: super::SkillRequest) {}

    /// The client's talent state at the call; `None` where no view lends it.
    fn talent_info(&self) -> Option<&super::TalentInfo> {
        None
    }

    /// The client's talent state at the call, for a verb that writes it; `None` where no view lends it.
    fn talent_info_mut(&mut self) -> Option<&mut super::TalentInfo> {
        None
    }

    /// The unit a talent verb acts for (`0x5c81df`): the pet's first guid held as a unit, else the active player held; `None` without it.
    fn talent_unit(&self, _pet: bool) -> Option<u64> {
        None
    }

    /// The pet's family category through its creature record (`0x5c5a28`), `None` without the pet held or the family's row.
    fn pet_family_category(&self) -> Option<i32> {
        None
    }

    /// `CMSG_LEARN_TALENT`, or with the pet's guid `CMSG_PET_LEARN_TALENT`, sent in the call (`0x5c8288`).
    fn send_learn_talent(&mut self, _pet: Option<u64>, _talent: u32, _rank: u32) {}

    /// `CMSG_LEARN_PREVIEW_TALENTS`, or with the pet's guid its pet form, sent in the call (`0x5c6a10`).
    fn send_learn_preview_talents(&mut self, _pet: Option<u64>, _talents: Vec<(u32, u32)>) {}

    /// The active player's `PLAYER_CHARACTER_POINTS1/2` at the call; `None` with no active player held.
    fn character_points(&self) -> Option<(u32, u32)> {
        None
    }

    /// The `GlyphProperties.dbc` row `id` with its spell's name, read at the call; `None` without the row.
    fn glyph(&self, _id: u32) -> Option<super::GlyphView> {
        None
    }

    /// The `GlyphSlot.dbc` row `id`, read at the call; `None` without the row.
    fn glyph_slot(&self, _id: u32) -> Option<super::GlyphSlotView> {
        None
    }

    /// The active player's glyph fields at the call (`+0x1230`, `+0x1248`, `+0x1260`); `None` with no active player held.
    fn glyph_sockets(&self) -> Option<super::GlyphSockets> {
        None
    }

    /// The targeting cast's glyph effects at the call (`0x7fd640` and its `Spell.dbc` row); `None` without a targeting cast or its row.
    fn glyph_targeting(&self) -> Option<super::GlyphTargeting> {
        None
    }

    /// `CMSG_REMOVE_GLYPH` with the 0-based `socket`, sent in the call (`0x5b76ea`).
    fn send_remove_glyph(&mut self, _socket: u32) {}

    /// The trade's network cells at the call: the partner's offered coins (`0xca0b78`) and each window's enchant spell (`0xca0ff0`); `None` where no view lends them.
    fn trade_cells(&self) -> Option<super::TradeCells> {
        None
    }

    /// The group loot roll `roll_id` on the client's list (`0x6fb150`), its record read with no callback and its time left off the clock at the call (`0x5889d0`, `0x588cc0`); `Some(None)` for no such roll, `None` where no view lends it.
    fn loot_roll(&self, _roll_id: i64) -> Option<Option<super::LootRollEntry>> {
        None
    }

    /// The open loot window resolved at the call: its coin slot and its 18 rows, each row's record read with no callback (`0x588310`, `0x5884a0`); `Some(None)` with no loot open, `None` where no view lends it.
    fn loot_window(&self) -> Option<Option<super::LootState>> {
        None
    }

    /// The looted object (`[0xbfa8d8]`), 0 with no loot open; `None` where no view lends it.
    fn loot_source(&self) -> Option<u64> {
        None
    }

    /// Whether the last window's loot type (`[0xbfa68c]`) is fishing; `None` where no view lends it.
    fn loot_fishing(&self) -> Option<bool> {
        None
    }

    /// The master-loot candidate at 0-based `index` (`0x6fa770`) named off the name cache, a miss asking with the list's callback (`0x58899d` -> `0x588150`); `Some(None)` for nobody, `None` where no view lends it.
    fn master_loot_candidate(&mut self, _index: u32) -> Option<Option<String>> {
        None
    }

    /// The inbox's shown rows and the server's whole count (`[0xbeb1d8]`, `[0xbeb1dc]`); `None` where no view lends the mail.
    fn mail_inbox_counts(&self) -> Option<(usize, u32)> {
        None
    }

    /// Inbox row `index` (0-based) resolved at the call, `text` the global strings and `military` the 24-hour clock: each record `read` names leaves its callback on a miss (`0x56d4b0`, `0x56d460`); `Some(None)` past the rows, `None` where no view lends the mail.
    fn mail_row(
        &mut self,
        _index: usize,
        _read: super::MailRead,
        _text: &dyn Fn(&str) -> Option<String>,
        _military: bool,
    ) -> Option<Option<super::MailInboxRow>> {
        None
    }

    /// The mark-as-read of inbox row `index` (0-based) inside the call (`0x56fe30`): the packet sent and the row turned read, then the signals it raises in order, none for no unread row; `None` where no view lends the mail.
    fn mail_mark_read(&mut self, _index: usize) -> Option<Vec<&'static str>> {
        None
    }

    /// A return or delete in flight on inbox row `index` (0-based) (`+0x2940`); `None` where no view lends the mail.
    fn mail_mark_delete_pending(&mut self, _index: usize) -> Option<()> {
        None
    }

    /// `HasNewMail()`'s test (`0x56f350`); `None` where no view lends the mail.
    fn mail_has_new(&self) -> Option<bool> {
        None
    }

    /// The delivered senders' names (`0x56f3a0`), a miss leaving the sender's callback (`0x56f3e0`); `None` where no view lends the mail.
    fn mail_latest_senders(
        &mut self,
        _text: &dyn Fn(&str) -> Option<String>,
    ) -> Option<Vec<Option<String>>> {
        None
    }

    /// The usable stationery list, rebuilt first (`0x56fc80`) on `rebuild` with no stationery record in flight; `None` where no view lends the mail.
    fn mail_stationeries(&mut self, _rebuild: bool) -> Option<Vec<super::StationeryView>> {
        None
    }

    /// `Package.dbc`'s rows; `None` where no view lends the mail.
    fn mail_packages(&self) -> Option<Vec<super::PackageView>> {
        None
    }

    /// Send `packet` to the open mailbox inside the call, as the verb's own send does; a take
    /// with `auto_loot` keeps looting its mail (`0x5703c5`). `None` where no view lends the mail.
    fn mail_send(&mut self, _packet: super::MailPacket) -> Option<()> {
        None
    }

    /// The complaint about the sender of the mail `message_id` inside the call (`0x56faf0`): with
    /// the complaint system on, a repeat is told back, else the sender joins the complained list
    /// and the report goes out; `None` where no view lends the mail.
    fn mail_complain(&mut self, _message_id: u32) -> Option<super::MailComplaint> {
        None
    }

    /// `CloseMail`'s state legs inside the call (`0x571a40`): with a mailbox open, the interact
    /// target cleared on it (`0x571a5e`), then the mailbox and the auto-loot zeroed (`0x571a6a`).
    /// `Some(held)`, `held` when the mailbox held the target, whose interaction close ran the mail
    /// close too (`0x51302d`); `Some(None)` with no mailbox; `None` where no view lends the mail.
    fn mail_close(&mut self) -> Option<Option<bool>> {
        None
    }

    /// The close's tail (`0x571a99`, `0x5717a7`): the query an arrival deferred while the mailbox
    /// was open, sent once.
    fn mail_close_query(&mut self) {}

    /// The auctioneer (`[0xc0f3e8]`), `Some(None)` with none; `None` where no view lends the
    /// auction house.
    fn auction_auctioneer(&self) -> Option<Option<u64>> {
        None
    }

    /// One list's rows and its total (`0x59c1a0`); `None` where no view lends the auction house.
    fn auction_counts(&self, _list: usize) -> Option<(usize, u32)> {
        None
    }

    /// Row `index` (0-based) of `list` read at the call; `info`, `GetAuctionItemInfo`'s read, leaves
    /// the list's callback on each name in flight once (`0x59d8dd`, `0x59d946`). `Some(None)` past the
    /// rows, `None` where no view lends the auction house.
    fn auction_row(
        &mut self,
        _list: usize,
        _index: usize,
        _info: bool,
    ) -> Option<Option<super::AuctionItemRow>> {
        None
    }

    /// The 0-based row of `auction_id` on `list` (`0x59b3a0`); `None` where no view lends the
    /// auction house.
    fn auction_find(&self, _list: usize, _auction_id: u32) -> Option<Option<usize>> {
        None
    }

    /// The house's `AuctionHouse.dbc` deposit rate (`0x59bc90`), `Some(None)` with no row; `None`
    /// where no view lends the auction house.
    fn auction_deposit_rate(&self) -> Option<Option<u32>> {
        None
    }

    /// The query gate of `list` (`0x59a2f0`) and the get-all one (`0x59c7ce`); `None` where no view
    /// lends the auction house.
    fn auction_can_send(&self, _list: usize) -> Option<(bool, bool)> {
        None
    }

    /// `list`'s sort criteria, front first; `None` where no view lends the auction house.
    fn auction_criteria(&self, _list: usize) -> Option<Vec<(u8, bool)>> {
        None
    }

    /// Replace `list`'s sort criteria; `None` where no view lends the auction house.
    fn set_auction_criteria(&mut self, _list: usize, _criteria: Vec<(u8, bool)>) -> Option<()> {
        None
    }

    /// Sort `list` in place by its criteria (`0x59b980`, `0x59b9e0`, `0x59cab0`), the verb raising
    /// its update; `None` where no view lends the auction house.
    fn auction_sort(&mut self, _list: usize) -> Option<()> {
        None
    }

    /// Send `packet` inside the verb's call, behind the gate its send reads; `None` where no view
    /// lends the auction house.
    fn auction_send(&mut self, _packet: super::AuctionPacket) -> Option<()> {
        None
    }

    /// `StartAuction`'s multi-sell state (`0x59fc92`-`0x59fcf3`), pending with more than one stack;
    /// `None` where no view lends the auction house.
    fn auction_multisell_start(&mut self, _sell: super::AuctionMultiSell) -> Option<()> {
        None
    }

    /// The next-item sell (`0x59ba80`); `None` where no view lends the auction house.
    fn auction_sell_next(&mut self) -> Option<super::AuctionSellStep> {
        None
    }

    /// Whether a multi-sell is pending (`[0xc0f434]`), with `end` ending it (`0x59ab26`); `None`
    /// where no view lends the auction house.
    fn auction_pending_sell(&mut self, _end: bool) -> Option<bool> {
        None
    }

    /// The close's interact-target clear on the auctioneer (`0x59f189`): whether the auctioneer held
    /// the target, whose interaction close runs the house's close again (`0x51304a`); `None` where no
    /// view lends the auction house.
    fn auction_clear_target(&mut self) -> Option<bool> {
        None
    }

    /// The close's state (`0x59f198`-`0x59f22b`); `None` where no view lends the auction house.
    fn auction_end_session(&mut self) -> Option<()> {
        None
    }

    /// The auction expiry (`0x59f2f0`) of `auction_id`: the events it raises, in order; `None`
    /// where no view lends the auction house.
    fn auction_expired(&mut self, _auction_id: u32) -> Option<Vec<&'static str>> {
        None
    }

    /// What the trade offer's gates read off held item `item` (`0x587aff`-`0x587bcf`, `0x5877b0`, `0x5d896d`); `None` with no item store held.
    fn trade_item_facts(&self, _item: u64) -> Option<super::TradeItemFacts> {
        None
    }

    /// The tooltip's ask of a refundable purchase's record when it holds none and none is pending (`0x62d57d`-`0x62d598`).
    fn ask_refund_record(&mut self, _item: u64) {}

    /// `CheckBinderDist` (`0x6d2290`): the latched innkeeper held as a unit within its combat reach plus 4 yd of us.
    fn binder_in_reach(&self) -> bool {
        false
    }

    /// `ConfirmBinder`'s talk with no guid (`0x6d2120`): the latched innkeeper in reach, `CMSG_BINDER_ACTIVATE` goes out (`0x6d2257`).
    fn confirm_binder(&self) {}

    /// `CheckTalentMasterDist` (`0x6d2070`): the latched trainer held as a unit within its combat reach plus 4 yd of us.
    fn talent_master_in_reach(&self) -> bool {
        false
    }

    /// `ConfirmTalentWipe`'s talk with no guid (`0x6db1c0`): the latched trainer in reach, the wipe goes out when our money covers the latched cost, else the refusal line's key (`0x6db2ae`).
    fn confirm_talent_wipe(&self) -> Option<&'static str> {
        None
    }

    /// `GetInstanceBootTimeRemaining` (`0x5162e0`): whole seconds to the boot deadline, 0 with none or past.
    fn instance_boot_secs(&self) -> u32 {
        0
    }

    /// `GetAreaSpiritHealerTime` (`0x516b90`): whole seconds to the held healer's wave, 0 with none or past.
    fn area_spirit_healer_secs(&self) -> u32 {
        0
    }

    /// `AcceptAreaSpiritHeal`'s healer poll at the call (`0x524b60` -> `0x524010`) up to its drop's cancel: whether a wave is cancelled, its event owed while the deadline still reads (`0x523eea` before `0x523efa`); `None` where no view lends it.
    fn area_spirit_poll_drop(&mut self) -> Option<bool> {
        None
    }

    /// The rest of the poll after that event, then `CMSG_AREA_SPIRIT_HEALER_QUEUE` naming the healer it left held (`0x524b60`).
    fn accept_area_spirit_heal(&mut self, _cancelled: bool) {}

    /// Whether our body is held, without which the 2584 aura cancel neither fires nor sends (`0x51f72d`, `0x51f75c`).
    fn spirit_cancel_has_body(&self) -> bool {
        false
    }

    /// `CancelAreaSpiritHeal`'s `CMSG_CANCEL_AURA` for 2584, after its event (`0x522fa0` -> `0x51f710`).
    fn cancel_area_spirit_heal(&self) {}

    /// Our player object held, the lookup the designer's verbs make first (`0x6de1d9`, `0x5997fd`).
    fn active_player_held(&self) -> bool {
        false
    }

    /// Our guild's record in the guild cache, asking nothing (`0x6de20c`): its five tabard fields, `-1`s when undesigned; `None` for no guild or no record.
    fn tabard_guild_record(&self) -> Option<[i32; 5]> {
        None
    }

    /// The tabard save latch (`0xc9eacc`).
    fn tabard_save_pending(&self) -> bool {
        false
    }

    /// The tabard save (`0x6db320`): the first failing check's message key, else `MSG_SAVE_GUILD_EMBLEM` sent and the latch set; `None` with no player object.
    fn save_tabard(&mut self, _design: [i32; 5]) -> Option<Result<(), &'static str>> {
        None
    }

    /// The designer's close (`0x598ec0`): the vendor cleared, and how many `CLOSE_TABARD_FRAME`s fire, 0 with none held.
    fn close_tabard(&mut self) -> u32 {
        0
    }

    /// The seed's and a cycle's colours onto our body (`0x5993b0`).
    fn apply_guild_color(&mut self, _design: [i32; 5]) {}

    /// A `DressUpModel` pane's own model edited inside its verb's call: undress (`0x597ba0`), re-dress from its unit (`0x5977e0`) or a try-on (`0x597fc0`), whose record miss asks inside the call and leaves the pane's callback (`0x5980fa`).
    fn dress_up(&mut self, _pane: crate::widget::FrameHandle, _intent: super::DressUpIntent) {}

    /// The gossip globals at the call (`0xc00ef0`, `0xc00bf0`, `0xbfc968`, `0xc00be8`); `None` with no view.
    fn gossip(&self) -> Option<super::GossipRead<'_>> {
        None
    }

    /// `CMSG_GOSSIP_SELECT_OPTION` for option `slot` (0-based) to the held NPC, then the menu emptied (`0x58b040`, `0x58b045`).
    fn select_gossip_option(&mut self, _slot: usize, _code: Option<String>) {}

    /// The questgiver send for quest row `row` (0-based, whole menu) to the held NPC (`0x58b070`, `0x58b130`).
    fn select_gossip_quest(&mut self, _row: usize) {}

    /// `CloseGossip`'s zero set (`0x58a550`): how many `GOSSIP_CLOSED`s it fires, 0 with no NPC held.
    fn close_gossip(&mut self) -> u32 {
        0
    }

    /// The item-text reader at the call (`0xbfc948`, `0xbfc940`, `0xbfaa00`); `None` with no view.
    fn item_text(&self) -> Option<super::ItemTextRead> {
        None
    }

    /// `ItemTextGetCreator` (`0x58a480`): the read item's creator name, a miss asked with the reader's callback left on it.
    fn item_text_creator(&mut self) -> Option<String> {
        None
    }

    /// A page step and its redraw (`0x58a110`, `0x58a150` -> `0x589e90`): whether the redraw fired `ITEM_TEXT_READY`.
    fn item_text_turn(&mut self, _next: bool) -> bool {
        false
    }

    /// `CloseItemText` (`0x58a400`): how many `ITEM_TEXT_CLOSED`s it fires, 0 with no reader.
    fn close_item_text(&mut self) -> u32 {
        0
    }

    /// The taxi map the last setup listed (`0x5932e0`), `Some(None)` with none; `None` where no view lends it.
    fn taxi_map(&self) -> Option<Option<&super::TaxiUiState>> {
        None
    }

    /// `TaxiNodeCost`'s fare at the call (`0x591350`) for a 0-based listed slot; `None` where no view lends it.
    fn taxi_node_cost(&self, _slot: u32) -> Option<i64> {
        None
    }

    /// A taxi verb's act inside its call; `None` where no view lends the map.
    fn taxi_act(&mut self, _act: super::TaxiAct) -> Option<super::TaxiOutcome> {
        None
    }

    /// The charter window and the registrar's row at the call (`0xc22b20`.., `0xc0e558`); `None` where no view lends them.
    fn petition_view(&self) -> Option<super::PetitionState> {
        None
    }

    /// Whether a petition vendor is set (`0xc0f7c8`); `None` where no view lends it.
    fn petition_vendor_open(&self) -> Option<bool> {
        None
    }

    /// The vendor's row count (`0xc0f7d0`); `None` where no view lends it.
    fn petition_vendor_count(&self) -> Option<usize> {
        None
    }

    /// Vendor row `index` (0-based) at the call, a miss asked with the vendor's record callback (`0x5a1f30`); `Some(None)` off the rows, `None` where no view lends it.
    fn petition_item(&mut self, _index: u32) -> Option<Option<super::PetitionVendorItem>> {
        None
    }

    /// A charter verb's act inside its call; `None` where no view lends the charter windows.
    fn petition_act(&mut self, _act: super::PetitionRequest) -> Option<super::PetitionOutcome> {
        None
    }

    /// The trainer window the list handler built (`0x596450`), `Some(None)` with none; `None` where no view lends it.
    fn trainer(&self) -> Option<Option<&super::TrainerWindow>> {
        None
    }

    /// The trainer window, written by a verb's mask commit or selection inside its call.
    fn trainer_mut(&mut self) -> Option<Option<&mut super::TrainerWindow>> {
        None
    }

    /// Service `position` (in list order)'s gates and product link at the call (`0x595470`, `0x5955e0`, `0x596290`); `Some(None)` past the list, `None` where no view lends it.
    fn trainer_service_live(&self, _position: usize) -> Option<Option<super::TrainerServiceLive>> {
        None
    }

    /// Service `position`'s description parsed at the call (`0x5959d0`), a recipe's product asked with the trainer's record callback; `None` where no view lends it.
    fn trainer_description(
        &mut self,
        _position: usize,
        _global: GlobalStrings<'_>,
    ) -> Option<Option<String>> {
        None
    }

    /// `CMSG_TRAINER_BUY_SPELL` to the open trainer inside the call (`0x594da0`); false where no view lends the wire.
    fn trainer_buy(&mut self, _spell_id: u32) -> bool {
        false
    }

    /// Whether a trainer is open, which the setter's zero leg tests (`0x5940fa`); `None` where no view lends the trainer.
    fn trainer_open(&self) -> Option<bool> {
        None
    }

    /// The setter's interact target clear (`0x594113`): true when the trainer held it, whose interaction close runs the zero leg again inside it (`0x512f80`).
    fn trainer_release_target(&mut self) -> bool {
        false
    }

    /// The setter's zero (`0x59411b`): the trainer and its records.
    fn trainer_zero(&mut self) {}

    /// The client's tradeskill window at the call (`0xc235a0`-`0xc23604`); `None` where no view lends it.
    fn trade_skill(&self) -> Option<&super::TradeSkillList> {
        None
    }

    /// The tradeskill window, written by a verb inside its call; `None` where no view lends it.
    fn trade_skill_mut(&mut self) -> Option<&mut super::TradeSkillList> {
        None
    }

    /// The open line's `SkillLine.dbc` names and the player's ranks of it at the call (`0x5db810`).
    fn trade_skill_line(&self) -> super::TradeSkillLineView {
        super::TradeSkillLineView::default()
    }

    /// Recipe `spell_id`'s spell record read at the call (`0x5db2a0`, `0x5db550`, `0x5db6e0`); `None` without a record.
    fn trade_skill_recipe(&self, _spell_id: u32) -> Option<super::TradeSkillRecipeView> {
        None
    }

    /// Reagent `item` of recipe `spell_id` at the call (`0x5de400`): its record asked with the list's item callback (`0x5de50d` -> `0x5de100`), and the bags' count of it (`0x754d00`).
    fn trade_skill_reagent(&mut self, _spell_id: u32, _item: u32) -> super::TradeSkillReagentView {
        super::TradeSkillReagentView::default()
    }

    /// Recipe `spell_id`'s requirements at the call (`0x5de620`): `(name, has)` for the spell focus and the required area, both met, then the tools asked with the list's item callback and the tool categories, each tested against the bags.
    fn trade_skill_tools(&mut self, _spell_id: u32) -> Vec<(String, bool)> {
        Vec::new()
    }

    /// Recipe `spell_id`'s description parsed at the call (`0x5dbe10`), `None` for none.
    fn trade_skill_description(
        &self,
        _spell_id: u32,
        _global: GlobalStrings<'_>,
    ) -> Option<String> {
        None
    }

    /// Recipe `spell_id`'s cooldown at the call (`0x5da9a0`), as [`Self::spell_cooldown`] reads it.
    fn trade_skill_cooldown(
        &self,
        spell_id: u32,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> Option<(i64, u32, bool)> {
        self.spell_cooldown(spell_id, anchor, ui_now)
    }

    /// The open line's `|Htrade:` link built at the call (`0x5dc0b0` -> `0x5db060`), `None` when it cannot link.
    fn trade_skill_list_link(&self) -> Option<String> {
        None
    }

    /// The text filter (`0x5dc110`) on recipe `spell_id` at the call: the product's required level within `min`..`max` (0 open), then `name`, folded, found in the product's, the recipe's, its reagents', tools' and focus' texts.
    fn trade_skill_filter_spell(
        &self,
        _spell_id: u32,
        _min: i32,
        _max: i32,
        _name: Option<&str>,
        _global: GlobalStrings<'_>,
    ) -> bool {
        true
    }

    /// Our coinage at the call (`PLAYER_FIELD_COINAGE`), 0 with no player object; `None` where no view lends it.
    fn player_money(&self) -> Option<u64> {
        None
    }

    /// Whether a vendor is set (`0xbfa3e8`); `None` where no view lends the merchant.
    fn merchant_open(&self) -> Option<bool> {
        None
    }

    /// The count of `list` (`0xbfa3f0`, `0xbfa3f4`); `None` where no view lends the merchant.
    fn merchant_len(&self, _list: super::MerchantList) -> Option<usize> {
        None
    }

    /// Row `index` (0-based) of `list` resolved at the call, `ask` leaving the merchant's record callback on a vendor row's miss (`0x584680`); `None` where no view lends the merchant.
    fn merchant_row(
        &mut self,
        _list: super::MerchantList,
        _index: usize,
        _ask: bool,
    ) -> Option<Option<super::MerchantItem>> {
        None
    }

    /// Whether the vendor's unit is held with the repair bit (`0x5845b0`); `None` where no view lends the merchant.
    fn can_merchant_repair(&self) -> Option<bool> {
        None
    }

    /// `GetRepairAllCost`'s sweep (`0x585990`): `Some(None)` with no repairer or no player; `None` where no view lends it.
    fn repair_all_cost(&self) -> Option<Option<(u32, bool)>> {
        None
    }

    /// An item's repair cost after the vendor's discount (`0x584b20`); `None` where no view lends it.
    fn repair_cost(&self, _at: super::RepairAt) -> Option<u32> {
        None
    }

    /// A merchant verb's act inside its call; `None` where no view lends the merchant.
    fn merchant_act(&mut self, _act: super::MerchantAct) -> Option<super::MerchantOutcome> {
        None
    }

    /// The barber shop at the call: the open flag (`[0xbd19b8]`), our look, level and coinage, and our body's dials; `None` with no player object.
    fn barber(&self) -> Option<super::BarberRead<'_>> {
        None
    }

    /// Our body's barber dials, written by a step or a reset inside its call (`0x52fc80`, `0x52f7d0`).
    fn set_barber_dials(&mut self, _dials: [u8; 5]) {}

    /// The skin style pointer (`[0xbd19b4]`), written by a skin step inside its call (`0x52fd94`).
    fn set_barber_skin_style(&mut self, _data: u8) {}

    /// `CMSG_ALTER_APPEARANCE` with the four style ids, sent inside the apply's call (`0x52eda0`).
    fn send_alter_appearance(&mut self, _styles: [u32; 4]) {}

    /// The shop's close (`0x52e9f0`): true when an open shop closed, for the caller to fire `BARBER_SHOP_CLOSE`.
    fn close_barber_shop(&mut self) -> bool {
        false
    }

    /// Our race row's hair token (`0x52e6d0`); `None` with no player object or row.
    fn hair_customization(&self) -> Option<&str> {
        None
    }

    /// Our race row's facial-hair token for our sex (`0x52e730`); `None` with no player object or row.
    fn facial_hair_customization(&self) -> Option<&str> {
        None
    }

    /// An item set's tooltip block built at the call from its `ItemSet.dbc` row, each member's name read from the name cache and a miss asked (`0x62ba3e`), the bonuses' `$`-text with `global` the VM's strings; `None` for no row.
    fn item_set(
        &self,
        _set_id: u32,
        _global: &dyn Fn(&str) -> Option<String>,
    ) -> Option<super::ItemSetView> {
        None
    }

    /// Whether a guild banker is open (`0xc1dc10`).
    fn guild_bank_open(&self) -> bool {
        false
    }

    /// The current guild bank tab, 0-based (`0xc1dc0d`).
    fn guild_bank_current_tab(&self) -> u8 {
        0
    }

    /// `SetCurrentGuildBankTab`'s write (`0x5a43c0`).
    fn set_guild_bank_current_tab(&mut self, _tab: u8) {}

    /// The bought guild bank tabs (`0xc1dc0c`).
    fn guild_bank_num_tabs(&self) -> u8 {
        0
    }

    /// Tab `tab`'s name, icon and text (`0xc11458`); `None` past the six.
    fn guild_bank_tab(&self, _tab: u8) -> Option<super::GuildBankTabView> {
        None
    }

    /// Slot `(tab, slot)`'s item id and count; `None` for an empty or unlisted slot (`0x5a4c10`).
    fn guild_bank_item(&self, _tab: u8, _slot: u8) -> Option<(u32, u32)> {
        None
    }

    /// Slot `(tab, slot)` resolved at the call: icon, link, name, enchants and stack size.
    fn guild_bank_slot(&self, _tab: u8, _slot: u8) -> Option<super::GuildBankSlotView> {
        None
    }

    /// Slot `(tab, slot)`'s icon alone, the read `GetGuildBankItemInfo` makes per slot (`0x5a6000`).
    fn guild_bank_texture(&self, tab: u8, slot: u8) -> Option<String> {
        self.guild_bank_slot(tab, slot).and_then(|s| s.texture)
    }

    /// The item lock (`item+0x394` bit 0) of `guid`; `None` with no item store lent.
    fn item_locked(&self, _guid: u64) -> Option<bool> {
        None
    }

    /// Lock (`0x706fe0`) or unlock (`0x707020`) the item `guid`: whether its bit moved; `None` with no item store lent.
    fn set_item_lock(&mut self, _guid: u64, _locked: bool) -> Option<bool> {
        None
    }

    /// Whether slot `(tab, slot)` holds `item_id` locked (`0x5a4c40`).
    fn guild_bank_locked(&self, _tab: u8, _slot: u8, _item_id: u32) -> bool {
        false
    }

    /// Lock or unlock slot `(tab, slot)` while it holds `item_id` (`0x5a4c90`, `0x5a4ce0`).
    fn set_guild_bank_lock(&mut self, _tab: u8, _slot: u8, _item_id: u32, _locked: bool) {}

    /// The last list's item withdrawals left (`0xc1dc30`).
    fn guild_bank_withdrawals(&self) -> i32 {
        0
    }

    /// The guild bank's money (`0xc1dc20`).
    fn guild_bank_money(&self) -> u64 {
        0
    }

    /// Today's money withdrawal left (`0xc1dc34`), -1 unlimited.
    fn guild_bank_withdraw_money(&self) -> i32 {
        0
    }

    /// The rows of log `tab`, 6 the money log.
    fn guild_bank_log_len(&self, _tab: u8) -> usize {
        0
    }

    /// Row `index` of log `tab` resolved at the call.
    fn guild_bank_log(&self, _tab: u8, _index: u8) -> Option<super::GuildBankLogView> {
        None
    }

    /// The banker setter's clear with no banker (`0x5a4270`): both bankers zeroed.
    fn clear_guild_banker(&mut self) {}

    /// `QueryGuildBankText`'s gate and send (`0x5a5940`): `true` when the text answers at once.
    fn guild_bank_query_text(&mut self, _tab: u8) -> bool {
        true
    }
}

impl UiScript {
    /// Run `f` with `live` lent to every verb it reaches; the loan ends before this returns, and a
    /// nested call lends its own and restores the outer one.
    pub fn with_live<V: LiveState + 'static, R>(
        &mut self,
        live: &mut V,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let lua = self.lua().clone();
        super::world_loan::lend_live(&lua, live, || f(self))
    }

    /// Whether a view is lent right now.
    pub fn has_live(&self) -> bool {
        super::world_loan::lent()
    }
}

/// The UI's control flag through the lent view; with none lent, as a VM outside the app has, the
/// value a game start sets (`0x52b8a0`).
pub(crate) fn has_control() -> bool {
    read_live(|l| l.has_control()).unwrap_or(true)
}

/// Queue `event` with the frame's unique signals (`0x615020`), which fire once each after the frame's packets (`0x614760`); with no queue lent, the model's deferred events take it.
pub(crate) fn unique_signal(model: &mut super::model::Model, event: &'static str) {
    if write_live(|l| l.unique_signal(event)) != Some(true) {
        model.defer_event(event, Vec::new());
    }
}

/// Our coinage at the call through the lent view; where none lends it, a test's pushed purse.
pub(crate) fn money(model: &super::Model) -> u64 {
    read_live(|l| l.player_money())
        .flatten()
        .unwrap_or(model.money)
}

/// `f` over the view lent right now, `None` with none lent.
pub fn read_live<R>(f: impl FnOnce(&dyn LiveState) -> R) -> Option<R> {
    super::world_loan::live_ref(f)
}

/// `f` over the view lent right now, mutably, for a verb that writes the state it reads; `None`
/// with none lent.
pub fn write_live<R>(f: impl FnOnce(&mut dyn LiveState) -> R) -> Option<R> {
    super::world_loan::live(f)
}

/// Run `f` with `view` lent as the live view, as a call into Lua lends it, for the VM's own tests.
#[cfg(test)]
pub(crate) fn lending<V: LiveState + 'static, R>(view: &mut V, f: impl FnOnce() -> R) -> R {
    super::world_loan::lend_live(&mlua::Lua::new(), view, f)
}

/// `f` over the [`TestView`] lent right now, between calls into Lua, for the VM's own tests.
#[cfg(test)]
pub(crate) fn test_view<R>(f: impl FnOnce(&mut TestView) -> R) -> R {
    super::world_loan::with_lent::<TestView, R>(f).expect("a test view is lent")
}

/// A view answering `slots` as our arena-team slots, for the VM's own tests.
#[cfg(test)]
pub(crate) fn arena_teams(slots: [super::ArenaTeamSlotView; 3]) -> TestView {
    TestView {
        arena_teams: slots,
        ..Default::default()
    }
}

/// A view over fixed state, for the VM's own tests.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestView {
    /// The active player's guid.
    pub player: Option<u64>,
    /// The carried-count check's answer, and the `(item_id, count)` it was asked, in order.
    pub carry: Option<super::CarryRefusal>,
    pub carry_asks: std::cell::RefCell<Vec<(u32, u32)>>,
    /// Each held unit's `(flags, health)`.
    pub units: std::collections::HashMap<u64, (u32, u32)>,
    /// The control flag; the default view has lost control.
    pub control: bool,
    /// Whether the view looks through a far-sight subject.
    pub far_sight: bool,
    /// Each held unit's `UNIT_DYNAMIC_FLAGS`, 0 when absent.
    pub dynamic_flags: std::collections::HashMap<u64, u32>,
    /// Each held player's `PLAYER_FLAGS`; a held unit absent here is no player.
    pub player_flags: std::collections::HashMap<u64, u32>,
    /// Each logged quest's special item.
    pub special_items: std::collections::HashMap<u32, super::QuestSpecialItemView>,
    /// The server's unix seconds now.
    pub server_unix: Option<f64>,
    /// The daily quest reset in server unix seconds.
    pub quest_reset: Option<f64>,
    /// Our honor block.
    pub honor: Option<super::HonorState>,
    /// The PvP toggle timer, `None` without the active player.
    pub pvp_timer: Option<i32>,
    /// Our arena-team slots.
    pub arena_teams: [super::ArenaTeamSlotView; 3],
    /// The inspect target the inspect verbs set.
    pub inspect_target: Cell<u64>,
    /// Each logged quest's title by id, and the quests `expire_quest` marked, in order.
    pub quest_titles: std::collections::HashMap<u32, String>,
    pub expired: Vec<u32>,
    /// Each logged quest's POI lines.
    pub poi_lines: std::collections::HashMap<u32, super::QuestPoiLines>,
    /// The questgiver calls made, in order, and the outcome each answers.
    pub quest_calls: Vec<Result<super::QuestAction, super::QuestSelect>>,
    pub quest_outcome: super::QuestActOutcome,
    /// The open banker and whether the interact target is it.
    pub banker: Option<u64>,
    pub bank_interacting: bool,
    /// The channel records and the zone's channel catalog.
    pub channel_slots: Vec<Option<super::ChannelSlotInfo>>,
    /// The channel pane's state.
    pub channel_pane: super::ChannelPane,
    pub zone_channels: Vec<super::ZoneChannelRow>,
    /// The `ZONECHANNELS` bits set, in order.
    pub zone_bits: Vec<u32>,
    /// The purchased bank bag slots, `None` without the active player, and the price rows.
    pub bank_slots: Option<u8>,
    pub bank_costs: std::collections::HashMap<u32, u32>,
    /// The item tables.
    pub item_tables: Option<super::ItemTables>,
    /// The currency tables, our currencies, and the item records asked, in order.
    pub currency_catalog: Option<super::CurrencyCatalogView>,
    pub currency_holdings: Option<super::CurrencyHoldings>,
    pub item_asks: std::cell::RefCell<Vec<u32>>,
    /// The companion lists and pool.
    pub companions: super::CompanionState,
    /// The pet stat block and `HasPetUI`'s first return.
    pub pet_stats: (bool, super::PetStats),
    /// The pet menu's three predicates; `None` answers the no-pet `(nil, nil, 1)`.
    pub pet_menu: Option<(bool, bool, bool)>,
    /// The held possess spell.
    pub possess: Option<super::PossessView>,
    /// The stable globals.
    pub stable: std::cell::RefCell<super::StableList>,
    /// The creature records the stable verbs find, by entry.
    pub stable_families: std::cell::RefCell<std::collections::HashMap<u32, super::StableFamily>>,
    /// `StableSlotPrices.dbc`, by row.
    pub stable_prices: std::cell::RefCell<std::collections::HashMap<u32, u32>>,
    /// The record asks a stable miss made, in order.
    pub stable_asks: std::cell::RefCell<Vec<(u32, super::StableCallback)>>,
    /// The stable verbs sent, in order.
    pub stable_sent: std::cell::RefCell<Vec<super::StableIntent>>,
    /// The pet list holds a pet.
    pub pet_out: bool,
    /// Our charm or summon is set.
    pub charm_or_summon: bool,
    /// Each held unit's pet, for a token's `pet` hop.
    pub unit_pets: std::collections::HashMap<u64, u64>,
    /// An open-stable aura is on us.
    pub open_stable_aura: bool,
    /// The player's cooldown history by spell, as `(start_ms, duration_ms, enabled)`.
    pub cooldowns: std::collections::HashMap<u32, (i64, u32, bool)>,
    /// The pet is out as a hunter's unit with its creature record.
    pub stable_live_pet: bool,
    /// The pet bar's bar-wide state.
    pub pet_bar: Option<super::PetBarLive>,
    /// The pet bar's slots, which the drag writes.
    pub pet_slots: std::cell::RefCell<Vec<super::PetActionView>>,
    /// `PetStopAttack` calls.
    pub pet_stops: Cell<u32>,
    /// The 0-based slots the bar toggle reached past its gate, in order.
    pub pet_toggles: std::cell::RefCell<Vec<u32>>,
    /// The units' held casts and channels, by `(guid, channel)`.
    pub unit_casts: std::collections::HashMap<(u64, bool), super::UnitCast>,
    /// `SpellStopCasting`'s test.
    pub stoppable_cast: bool,
    /// The totem table's slots, which the destroy clears, and each empty slot's tool flag.
    pub totems: std::cell::RefCell<[Option<super::TotemView>; super::MAX_TOTEM_SLOTS]>,
    pub totem_tools: [bool; super::MAX_TOTEM_SLOTS],
    /// The 0-based slots the destroy sent, in order.
    pub totem_destroys: std::cell::RefCell<Vec<usize>>,
    /// The totem bar's element lists, the spells' `TotemCategory` pairs and the rows' names.
    pub multicast_lists: [Vec<u32>; 4],
    pub totem_categories: std::collections::HashMap<u32, [u32; 2]>,
    pub totem_category_names: std::collections::HashMap<u32, String>,
    /// The rune table and the player's rates; `None` without the player object.
    pub runes: Option<(super::RuneState, super::RuneRegen)>,
    /// The guild bank, its locks as `(tab, slot, item)`, the current tab and the two moneys.
    pub guild_bank: super::GuildBankView,
    pub guild_bank_locks: Vec<(u8, u8, u32)>,
    pub guild_bank_tab: u8,
    pub guild_bank_money: u64,
    pub guild_bank_withdraw_money: i32,
    /// The tabs whose text is stale, and the text asks sent, in order.
    pub guild_bank_text_stale: Vec<u8>,
    pub guild_bank_text_sent: Vec<u8>,
    /// Our guild's cached tabard record and the save latch.
    pub tabard_record: Option<[i32; 5]>,
    pub tabard_pending: bool,
    /// The item cache's records, and each read made, `(id, tooltip)`, in order.
    pub item_records: std::cell::RefCell<std::collections::HashMap<u32, super::ItemTemplateView>>,
    pub item_record_reads: std::cell::RefCell<Vec<(u32, bool)>>,
    /// The bind point's area name.
    pub bind_location: std::cell::RefCell<Option<String>>,
    /// The gossip globals, the selects and the closes a close fires.
    pub gossip: Option<TestGossip>,
    /// The item-text reader.
    pub item_text: Option<TestItemText>,
    /// The barber shop, the alter-appearance sends and our race's two tokens.
    pub barber: Option<TestBarber>,
    pub barber_sent: Vec<[u32; 4]>,
    pub hair_tokens: Option<(String, String)>,
    /// The dressing-room edits the verbs made, in call order.
    pub dressup: Vec<(crate::widget::FrameHandle, super::DressUpIntent)>,
    /// The charter, taxi and trainer acts the verbs made inside their calls, which this view
    /// records and leaves to the test's seeded windows.
    pub petition_acts: Vec<super::PetitionRequest>,
    pub taxi_acts: Vec<super::TaxiAct>,
    pub trainer_bought: Vec<u32>,
    /// Whether a trainer is open, which `CloseTrainer`'s zero leg tests; `None` answers nothing.
    pub trainer_is_open: Option<bool>,
    /// The trainer zero legs run.
    pub trainer_zeroes: u32,
    /// The chat lines the display added inside its calls, with their wire chat type.
    pub chat_lines: Vec<(u8, String)>,
    /// The logs the logging verbs opened, `(chat, combat)`, and whether an open fails.
    pub logs_opened: (bool, bool),
    /// The played clock the refund and trade verbs read, and the `CMSG_PLAYED_TIME`s sent.
    pub played: Option<i64>,
    pub played_asks: u32,
    pub log_open_fails: bool,
    /// The client's faction list and the faction table it reads.
    pub faction_list: super::FactionList,
    pub factions: super::StaticFactions,
    /// The achievement tables and lists, the cached player names, the name queries and the
    /// comparison queries sent, in order.
    pub achievement_db: Option<std::sync::Arc<super::AchievementDb>>,
    pub achievements: super::AchievementBook,
    pub player_names: std::collections::HashMap<u64, String>,
    pub name_asks: Vec<u64>,
    pub inspect_achievement_queries: Vec<u64>,
    /// Each held player's PvP faction index; a guid absent here is no held player.
    pub player_factions: std::collections::HashMap<u64, i32>,
    /// `CharTitles.dbc`'s rows, our title fields, and the `CMSG_SET_TITLE` masks sent, in order.
    pub char_titles: Vec<super::TitleRow>,
    pub player_titles: Option<super::PlayerTitles>,
    pub title_sends: Vec<i32>,
    /// Our sheet figures, our skill slots by line, and the skill sends made, in order.
    pub combat_stats: Option<super::UnitCombatStats>,
    pub skills: Option<std::collections::HashMap<u32, super::SkillLive>>,
    /// What a line missing from `skills` reads: slot -1 (`0x6dc222`).
    pub skill_slot_minus_one: super::SkillLive,
    pub unlearn_sends: Vec<u32>,
    pub skill_sends: Vec<super::SkillRequest>,
    /// The talent state, the talent unit per side `(player, pet)`, the pet's family category, our character points, and the learn sends in order.
    pub talent_info: Option<super::TalentInfo>,
    pub talent_units: (Option<u64>, Option<u64>),
    pub pet_category: Option<i32>,
    /// The pet's known spells, its higher ranks included.
    pub pet_known: Vec<u32>,
    pub character_points: Option<(u32, u32)>,
    pub talent_learns: Vec<(Option<u64>, u32, u32)>,
    pub talent_preview_learns: Vec<super::PreviewLearn>,
    /// The glyph tables, our glyph fields (`None` without the active player), the targeting cast's glyphs, and the `CMSG_REMOVE_GLYPH` sockets sent, in order.
    pub glyph_catalog: super::GlyphCatalogView,
    pub glyph_sockets: Option<super::GlyphSockets>,
    pub glyph_targeting: Option<super::GlyphTargeting>,
    pub glyph_removes: Vec<u32>,
}

/// The gossip globals a test view lends.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestGossip {
    pub greeting: String,
    pub options: Vec<super::GossipOptionView>,
    pub quests: Vec<super::GossipQuestRow>,
    pub pending: bool,
    pub force_gossip: bool,
    pub money: u64,
    pub selects: Vec<(usize, Option<String>)>,
    pub quest_selects: Vec<usize>,
    /// What the first close returns; later closes return 0.
    pub closes: u32,
}

/// The item-text reader a test view lends.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestItemText {
    pub read: super::ItemTextRead,
    pub creator: Option<String>,
    /// What the first close returns; later closes return 0.
    pub closes: u32,
    /// Whether a turn's redraw fires, and the turns made, `true` for next.
    pub turn_ready: bool,
    pub turns: Vec<bool>,
}

/// The barber shop a test view lends.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestBarber {
    pub open: bool,
    pub class: u8,
    pub base_cost: f32,
    pub look: [u8; 5],
    pub dials: [u8; 5],
    pub skin_style: u8,
    pub styles: Vec<super::BarberStyle>,
    pub grid: Option<super::GlueCustomizeGrid>,
    pub money: u32,
}

#[cfg(test)]
impl TestView {
    /// The stable verbs sent since the last take.
    pub(crate) fn take_sent(&self) -> Vec<super::StableIntent> {
        std::mem::take(&mut *self.stable_sent.borrow_mut())
    }
}

#[cfg(test)]
impl LiveState for TestView {
    fn channel_slots(&self) -> Vec<Option<super::ChannelSlotInfo>> {
        self.channel_slots.clone()
    }

    fn set_channel_flags(&mut self, slot: usize, flags: u8) {
        if let Some(Some(s)) = self.channel_slots.get_mut(slot) {
            s.flags = flags;
        }
    }

    fn channel_pane(&mut self) -> Option<&mut super::ChannelPane> {
        Some(&mut self.channel_pane)
    }

    fn zone_channel_catalog(&self) -> Vec<super::ZoneChannelRow> {
        self.zone_channels.clone()
    }

    fn add_zone_channel_bit(&mut self, id: u32) {
        self.zone_bits.push(id);
    }

    fn reputation(
        &mut self,
        f: &mut dyn FnMut(&mut super::FactionList, &mut dyn super::FactionTable),
    ) -> bool {
        f(&mut self.faction_list, &mut self.factions);
        true
    }

    fn carry_refusal(&self, item_id: u32, count: u32) -> Option<super::CarryRefusal> {
        self.carry_asks.borrow_mut().push((item_id, count));
        self.carry.clone()
    }

    fn achievement_db(&self) -> Option<std::sync::Arc<super::AchievementDb>> {
        self.achievement_db.clone()
    }

    fn achievement_book(&self) -> Option<&super::AchievementBook> {
        Some(&self.achievements)
    }

    fn achievement_book_mut(&mut self) -> Option<&mut super::AchievementBook> {
        Some(&mut self.achievements)
    }

    fn player_pvp_faction(&self, guid: u64) -> Option<i32> {
        self.player_factions.get(&guid).copied()
    }

    fn cached_player_name(&self, guid: u64) -> Option<String> {
        self.player_names.get(&guid).cloned()
    }

    fn player_name_or_ask(&mut self, guid: u64) -> Option<String> {
        let name = self.player_names.get(&guid).cloned();
        if name.is_none() {
            self.name_asks.push(guid);
        }
        name
    }

    fn query_inspect_achievements(&mut self, guid: u64) {
        self.inspect_achievement_queries.push(guid);
    }

    fn char_title_count(&self) -> usize {
        self.char_titles.len()
    }

    fn char_title(&self, mask: i64) -> Option<super::TitleRow> {
        super::title_row(&self.char_titles, mask).cloned()
    }

    fn player_titles(&self) -> Option<super::PlayerTitles> {
        self.player_titles.clone()
    }

    fn send_set_title(&mut self, mask: i32) {
        self.title_sends.push(mask);
    }

    fn player_combat_stats(&self) -> Option<super::UnitCombatStats> {
        self.combat_stats.clone()
    }

    fn skill_slot(&self, skill_id: u32) -> Option<super::SkillLive> {
        let skills = self.skills.as_ref()?;
        Some(
            skills
                .get(&skill_id)
                .copied()
                .unwrap_or(self.skill_slot_minus_one),
        )
    }

    fn send_unlearn_skill(&mut self, skill_id: u32) {
        self.unlearn_sends.push(skill_id);
    }

    fn send_skill_request(&mut self, request: super::SkillRequest) {
        self.skill_sends.push(request);
    }

    fn talent_info(&self) -> Option<&super::TalentInfo> {
        self.talent_info.as_ref()
    }

    fn talent_info_mut(&mut self) -> Option<&mut super::TalentInfo> {
        self.talent_info.as_mut()
    }

    fn talent_unit(&self, pet: bool) -> Option<u64> {
        if pet {
            self.talent_units.1
        } else {
            self.talent_units.0
        }
    }

    fn pet_family_category(&self) -> Option<i32> {
        self.pet_category
    }

    fn pet_spell_known_or_higher(&self, spell_id: u32) -> bool {
        self.talent_units.0.is_some()
            && self.talent_units.1.is_some()
            && self.pet_known.contains(&spell_id)
    }

    fn send_learn_talent(&mut self, pet: Option<u64>, talent: u32, rank: u32) {
        self.talent_learns.push((pet, talent, rank));
    }

    fn send_learn_preview_talents(&mut self, pet: Option<u64>, talents: Vec<(u32, u32)>) {
        self.talent_preview_learns.push((pet, talents));
    }

    fn character_points(&self) -> Option<(u32, u32)> {
        self.character_points
    }

    fn glyph(&self, id: u32) -> Option<super::GlyphView> {
        self.glyph_catalog.glyph(id).cloned()
    }

    fn glyph_slot(&self, id: u32) -> Option<super::GlyphSlotView> {
        self.glyph_catalog.slot(id).copied()
    }

    fn glyph_sockets(&self) -> Option<super::GlyphSockets> {
        self.glyph_sockets
    }

    fn glyph_targeting(&self) -> Option<super::GlyphTargeting> {
        self.glyph_targeting
    }

    fn send_remove_glyph(&mut self, socket: u32) {
        self.glyph_removes.push(socket);
    }

    fn unit_cast(&self, guid: u64, channel: bool) -> Option<super::UnitCast> {
        self.unit_casts.get(&(guid, channel)).cloned()
    }

    fn has_stoppable_cast(&self) -> bool {
        self.stoppable_cast
    }

    fn spell_targeting(&self) -> super::SpellTargetingLive {
        TEST_TARGETING.with(|t| t.borrow().0)
    }

    fn spell_can_target_unit(&self, guid: u64) -> bool {
        TEST_TARGETING.with(|t| {
            let t = t.borrow();
            t.0.active && t.1.contains(&guid)
        })
    }

    fn multicast_list(&self, element: usize) -> Vec<u32> {
        self.multicast_lists
            .get(element)
            .cloned()
            .unwrap_or_default()
    }

    fn spell_totem_categories(&self, spell: u32) -> Option<[u32; 2]> {
        self.totem_categories.get(&spell).copied()
    }

    fn totem_category_name(&self, row: u32) -> Option<String> {
        self.totem_category_names.get(&row).cloned()
    }

    fn totem(&self, slot: usize) -> Option<super::TotemView> {
        self.totems.borrow().get(slot).cloned().flatten()
    }

    fn totem_tool(&self, slot: usize) -> bool {
        self.totem_tools.get(slot).copied().unwrap_or(false)
    }

    fn destroy_totem(&mut self, slot: usize) -> bool {
        let live = self
            .totems
            .borrow_mut()
            .get_mut(slot)
            .and_then(Option::take)
            .is_some();
        if live {
            self.totem_destroys.borrow_mut().push(slot);
        }
        live
    }

    fn runes(&self) -> Option<(super::RuneState, super::RuneRegen)> {
        self.runes.clone()
    }

    fn item_tables(&self) -> Option<&super::ItemTables> {
        self.item_tables.as_ref()
    }

    fn dress_up(&mut self, pane: crate::widget::FrameHandle, intent: super::DressUpIntent) {
        self.dressup.push((pane, intent));
    }

    fn petition_act(&mut self, act: super::PetitionRequest) -> Option<super::PetitionOutcome> {
        // `HasFilledPetition`'s walk is a read; the test's vendor answers it.
        if act != super::PetitionRequest::HasFilled {
            self.petition_acts.push(act);
        }
        None
    }

    fn taxi_act(&mut self, act: super::TaxiAct) -> Option<super::TaxiOutcome> {
        self.taxi_acts.push(act);
        None
    }

    fn played_now(&self) -> Option<i64> {
        self.played
    }

    fn send_played_time(&mut self) {
        self.played_asks += 1;
    }

    fn open_log(&mut self, chat: bool) -> Option<bool> {
        if self.log_open_fails {
            return Some(false);
        }
        if chat {
            self.logs_opened.0 = true;
        } else {
            self.logs_opened.1 = true;
        }
        Some(true)
    }

    /// A system line fires `CHAT_MSG_SYSTEM`, a skill line `CHAT_MSG_SKILL`, with the text alone.
    fn add_chat_line(
        &mut self,
        chat_type: u8,
        text: &str,
    ) -> Option<(&'static str, Vec<super::ScriptValue>)> {
        self.chat_lines.push((chat_type, text.to_string()));
        let event = if chat_type == 26 {
            "CHAT_MSG_SKILL"
        } else {
            "CHAT_MSG_SYSTEM"
        };
        Some((event, vec![super::ScriptValue::Str(text.to_string())]))
    }

    fn trainer_buy(&mut self, spell_id: u32) -> bool {
        self.trainer_bought.push(spell_id);
        true
    }

    fn trainer_open(&self) -> Option<bool> {
        self.trainer_is_open
    }

    fn trainer_zero(&mut self) {
        self.trainer_zeroes += 1;
        self.trainer_is_open = self.trainer_is_open.map(|_| false);
    }

    fn guild_bank_open(&self) -> bool {
        self.guild_bank.open
    }

    fn guild_bank_current_tab(&self) -> u8 {
        self.guild_bank_tab
    }

    fn set_guild_bank_current_tab(&mut self, tab: u8) {
        self.guild_bank_tab = tab;
    }

    fn guild_bank_num_tabs(&self) -> u8 {
        self.guild_bank.num_tabs
    }

    fn guild_bank_tab(&self, tab: u8) -> Option<super::GuildBankTabView> {
        self.guild_bank.tabs.get(usize::from(tab)).cloned()
    }

    fn guild_bank_item(&self, tab: u8, slot: u8) -> Option<(u32, u32)> {
        self.guild_bank
            .slot(usize::from(tab), usize::from(slot))
            .map(|s| (s.item_id, s.count))
    }

    fn guild_bank_slot(&self, tab: u8, slot: u8) -> Option<super::GuildBankSlotView> {
        self.guild_bank
            .slot(usize::from(tab), usize::from(slot))
            .cloned()
    }

    fn guild_bank_locked(&self, tab: u8, slot: u8, item_id: u32) -> bool {
        self.guild_bank_locks.contains(&(tab, slot, item_id))
    }

    fn set_guild_bank_lock(&mut self, tab: u8, slot: u8, item_id: u32, locked: bool) {
        if self.guild_bank_item(tab, slot).map(|(id, _)| id) != Some(item_id) {
            return;
        }
        self.guild_bank_locks.retain(|&l| l != (tab, slot, item_id));
        if locked {
            self.guild_bank_locks.push((tab, slot, item_id));
        }
    }

    fn guild_bank_withdrawals(&self) -> i32 {
        self.guild_bank.withdrawals_remaining
    }

    fn guild_bank_money(&self) -> u64 {
        self.guild_bank_money
    }

    fn guild_bank_withdraw_money(&self) -> i32 {
        self.guild_bank_withdraw_money
    }

    fn guild_bank_log_len(&self, tab: u8) -> usize {
        self.guild_bank
            .logs
            .get(usize::from(tab))
            .map_or(0, Vec::len)
    }

    fn guild_bank_log(&self, tab: u8, index: u8) -> Option<super::GuildBankLogView> {
        self.guild_bank
            .logs
            .get(usize::from(tab))?
            .get(usize::from(index))
            .cloned()
    }

    fn clear_guild_banker(&mut self) {
        self.guild_bank.open = false;
    }

    fn guild_bank_query_text(&mut self, tab: u8) -> bool {
        if !self.guild_bank.open || !self.guild_bank_text_stale.contains(&tab) {
            return true;
        }
        self.guild_bank_text_stale.retain(|&t| t != tab);
        self.guild_bank_text_sent.push(tab);
        false
    }

    fn banker(&self) -> Option<u64> {
        self.banker
    }

    fn close_bank_interaction(&mut self) -> bool {
        std::mem::take(&mut self.bank_interacting) && self.banker.is_some()
    }

    fn clear_banker(&mut self) {
        self.banker = None;
    }

    fn bank_bag_slots(&self) -> Option<u8> {
        self.bank_slots
    }

    fn bank_slot_cost(&self, slot: u32) -> u32 {
        self.bank_costs.get(&slot).copied().unwrap_or(0)
    }

    fn currency_catalog(&self) -> Option<&super::CurrencyCatalogView> {
        self.currency_catalog.as_ref()
    }

    fn currency_holdings(&self) -> Option<super::CurrencyHoldings> {
        self.currency_holdings.clone()
    }

    fn ask_item_record(&self, id: u32) {
        let cached = self
            .currency_holdings
            .as_ref()
            .and_then(|h| h.items.get(&id))
            .is_some_and(|(name, _)| name.is_some())
            || self.item_records.borrow().contains_key(&id);
        if !cached {
            self.item_asks.borrow_mut().push(id);
        }
    }

    fn item_record(
        &self,
        id: u32,
        tooltip: Option<GlobalStrings<'_>>,
    ) -> Option<Option<super::ItemTemplateView>> {
        self.item_record_reads
            .borrow_mut()
            .push((id, tooltip.is_some()));
        Some(self.item_records.borrow().get(&id).cloned())
    }

    fn bind_location(&self) -> Option<String> {
        self.bind_location.borrow().clone()
    }

    fn unit_flags(&self, guid: u64) -> Option<u32> {
        self.units.get(&guid).map(|u| u.0)
    }

    fn unit_health(&self, guid: u64) -> Option<u32> {
        self.units.get(&guid).map(|u| u.1)
    }

    fn unit_dynamic_flags(&self, guid: u64) -> Option<u32> {
        self.units
            .contains_key(&guid)
            .then(|| self.dynamic_flags.get(&guid).copied().unwrap_or(0))
    }

    fn player_flags(&self, guid: u64) -> Option<u32> {
        self.units
            .contains_key(&guid)
            .then(|| self.player_flags.get(&guid).copied())
            .flatten()
    }

    fn active_player(&self) -> Option<u64> {
        self.player
    }

    fn spirit_cancel_has_body(&self) -> bool {
        self.player.is_some()
    }

    fn active_player_held(&self) -> bool {
        self.player.is_some()
    }

    fn tabard_guild_record(&self) -> Option<[i32; 5]> {
        self.tabard_record
    }

    fn gossip(&self) -> Option<super::GossipRead<'_>> {
        let g = self.gossip.as_ref()?;
        Some(super::GossipRead {
            greeting: &g.greeting,
            options: &g.options,
            quests: &g.quests,
            pending: g.pending,
            force_gossip: g.force_gossip,
            money: g.money,
        })
    }

    fn select_gossip_option(&mut self, slot: usize, code: Option<String>) {
        if let Some(g) = self.gossip.as_mut() {
            g.selects.push((slot, code));
            g.options.clear();
            g.quests.clear();
        }
    }

    fn select_gossip_quest(&mut self, row: usize) {
        if let Some(g) = self.gossip.as_mut() {
            g.quest_selects.push(row);
        }
    }

    fn close_gossip(&mut self) -> u32 {
        self.gossip
            .as_mut()
            .map_or(0, |g| std::mem::take(&mut g.closes))
    }

    fn item_text(&self) -> Option<super::ItemTextRead> {
        self.item_text.as_ref().map(|t| t.read.clone())
    }

    fn item_text_creator(&mut self) -> Option<String> {
        self.item_text.as_ref()?.creator.clone()
    }

    fn item_text_turn(&mut self, next: bool) -> bool {
        self.item_text.as_mut().is_some_and(|t| {
            t.turns.push(next);
            t.turn_ready
        })
    }

    fn close_item_text(&mut self) -> u32 {
        self.item_text
            .as_mut()
            .map_or(0, |t| std::mem::take(&mut t.closes))
    }

    fn barber(&self) -> Option<super::BarberRead<'_>> {
        let b = self.barber.as_ref()?;
        Some(super::BarberRead {
            open: b.open,
            class: b.class,
            base_cost: b.base_cost,
            look: b.look,
            dials: b.dials,
            skin_style: b.skin_style,
            styles: &b.styles,
            grid: b.grid.as_ref(),
            money: b.money,
        })
    }

    fn set_barber_dials(&mut self, dials: [u8; 5]) {
        if let Some(b) = self.barber.as_mut() {
            b.dials = dials;
        }
    }

    fn set_barber_skin_style(&mut self, data: u8) {
        if let Some(b) = self.barber.as_mut() {
            b.skin_style = data;
        }
    }

    fn send_alter_appearance(&mut self, styles: [u32; 4]) {
        self.barber_sent.push(styles);
    }

    fn close_barber_shop(&mut self) -> bool {
        self.barber
            .as_mut()
            .is_some_and(|b| std::mem::take(&mut b.open))
    }

    fn hair_customization(&self) -> Option<&str> {
        self.hair_tokens.as_ref().map(|t| t.0.as_str())
    }

    fn facial_hair_customization(&self) -> Option<&str> {
        self.hair_tokens.as_ref().map(|t| t.1.as_str())
    }

    fn tabard_save_pending(&self) -> bool {
        self.tabard_pending
    }

    fn has_control(&self) -> bool {
        self.control
    }

    fn far_sight(&self) -> bool {
        self.far_sight
    }

    fn quest_special_item(&self, quest_id: u32) -> Option<super::QuestSpecialItemView> {
        self.special_items.get(&quest_id).cloned()
    }

    fn server_unix_time(&self) -> Option<f64> {
        self.server_unix
    }

    fn quest_poi_lines(
        &self,
        quest_id: u32,
        _get: &dyn Fn(&str) -> Option<String>,
    ) -> Option<Option<super::QuestPoiLines>> {
        Some(self.poi_lines.get(&quest_id).cloned())
    }

    fn expire_quest(&mut self, quest_id: u32) -> Option<Option<String>> {
        if self.expired.contains(&quest_id) {
            return Some(None);
        }
        self.expired.push(quest_id);
        Some(self.quest_titles.get(&quest_id).cloned())
    }

    fn quest_reset_unix(&self) -> Option<f64> {
        self.quest_reset
    }

    fn quest_action(&mut self, action: super::QuestAction) -> Option<super::QuestActOutcome> {
        self.quest_calls.push(Ok(action));
        Some(self.quest_outcome)
    }

    fn quest_select(&mut self, select: super::QuestSelect) -> Option<super::QuestActOutcome> {
        self.quest_calls.push(Err(select));
        Some(self.quest_outcome)
    }

    fn honor(&self) -> Option<super::HonorState> {
        self.honor
    }

    fn pvp_toggle_timer(&self) -> Option<i32> {
        self.pvp_timer
    }

    fn arena_team_info(&self, slot: usize) -> [u32; 7] {
        self.arena_teams[slot].info
    }

    fn arena_team_stats(&self, slot: usize) -> [u32; 6] {
        self.arena_teams[slot].stats
    }

    fn arena_team_roster_type(&self, slot: usize) -> u32 {
        self.arena_teams[slot].roster_type
    }

    fn arena_team_member_count(&self, slot: usize) -> usize {
        self.arena_teams[slot].members.len()
    }

    fn arena_team_member(
        &self,
        slot: usize,
        index: usize,
    ) -> Option<super::ArenaTeamMemberRef<'_>> {
        self.arena_teams[slot]
            .members
            .get(index)
            .map(|m| m.borrowed())
    }

    fn arena_team_record(&self, team_id: u32) -> Option<super::ArenaTeamRecordView> {
        self.arena_teams
            .iter()
            .find(|s| s.info[0] == team_id)
            .and_then(|s| s.record.clone())
    }

    fn notify_inspect(&mut self, guid: u64) -> bool {
        self.inspect_target.replace(guid) != guid
    }

    fn clear_inspect_target(&mut self) {
        self.inspect_target.set(0);
    }

    fn companion_spells(&self, ty: super::CompanionType) -> Vec<u32> {
        self.companions
            .list(ty)
            .iter()
            .map(|c| c.spell_id)
            .collect()
    }

    fn companion(&self, ty: super::CompanionType, index: usize) -> Option<super::CompanionView> {
        self.companions.list(ty).get(index).cloned()
    }

    fn companion_random_pool(&self) -> Vec<u32> {
        self.companions.random_pool.clone()
    }

    fn pet_stats(&self) -> (bool, super::PetStats) {
        self.pet_stats.clone()
    }

    fn pet_menu(&self) -> (bool, bool, bool) {
        self.pet_menu.unwrap_or((false, false, true))
    }

    fn creature_family(&self, guid: u64) -> Option<String> {
        (self.player != Some(guid) && self.units.contains_key(&guid))
            .then(|| self.pet_stats.1.family.clone())
            .flatten()
    }

    fn possess(&self) -> Option<super::PossessView> {
        self.possess.clone()
    }

    fn stable(&self) -> super::StableList {
        self.stable.borrow().clone()
    }

    fn stable_family(
        &mut self,
        entry: u32,
        callback: Option<super::StableCallback>,
    ) -> Option<super::StableFamily> {
        let found = self.stable_families.borrow().get(&entry).cloned();
        if let (None, Some(callback)) = (&found, callback) {
            self.stable_asks.borrow_mut().push((entry, callback));
        }
        found
    }

    fn stable_live_pet(&self) -> bool {
        self.stable_live_pet
    }

    fn stable_slot_price(&self, slot: u32) -> Option<u32> {
        self.stable_prices.borrow().get(&slot).copied()
    }

    fn stable_send(&self, verb: super::StableIntent) {
        self.stable_sent.borrow_mut().push(verb);
    }

    fn close_stables(&mut self) -> bool {
        std::mem::take(&mut self.stable.borrow_mut().master) != 0
    }

    fn pet_out(&self) -> bool {
        self.pet_out
    }

    fn charm_or_summon(&self) -> bool {
        self.charm_or_summon
    }

    fn unit_pet(&self, guid: u64) -> Option<u64> {
        self.unit_pets.get(&guid).copied()
    }

    fn open_stable_aura(&self) -> bool {
        self.open_stable_aura
    }

    fn spell_cooldown(
        &self,
        spell_id: u32,
        _anchor: std::time::Instant,
        _ui_now: f64,
    ) -> Option<(i64, u32, bool)> {
        self.cooldowns
            .get(&spell_id)
            .copied()
            .or_else(|| super::spellbook::stand_in_slot(spell_id).and_then(|s| s.cooldown))
    }

    fn spell_selected(&self, spell_id: u32) -> bool {
        super::spellbook::stand_in_slot(spell_id).is_some_and(|s| s.current)
    }

    fn spell_known(&self, spell_id: u32) -> bool {
        TEST_KNOWN.with(|k| k.borrow().0.contains(&spell_id))
    }

    fn spell_known_or_higher(&self, spell_id: u32) -> bool {
        TEST_KNOWN.with(|k| {
            let (known, next) = &*k.borrow();
            let mut cur = spell_id;
            for _ in 0..32 {
                if known.contains(&cur) {
                    return true;
                }
                let Some(&n) = next.get(&cur) else {
                    return false;
                };
                cur = n;
            }
            false
        })
    }

    fn pet_bar(&self) -> Option<super::PetBarLive> {
        self.pet_bar.clone()
    }

    fn pet_action(
        &self,
        index: usize,
        _anchor: std::time::Instant,
        _ui_now: f64,
    ) -> Option<super::PetActionView> {
        self.pet_slots.borrow().get(index).cloned()
    }

    fn pet_place_action(&mut self, entries: &[(u32, u32)]) -> bool {
        TEST_PET_WRITES.with(|w| w.borrow_mut().push(entries.to_vec()));
        let mut slots = self.pet_slots.borrow_mut();
        for &(i, word) in entries {
            if let Some(s) = slots.get_mut(i as usize) {
                s.packed = word;
            }
        }
        true
    }

    fn pet_stop_attack(&mut self) {
        self.pet_stops.set(self.pet_stops.get() + 1);
    }

    fn pet_toggle_autocast(&mut self, index: u32) -> bool {
        if !self.pet_bar.as_ref().is_some_and(|b| b.actions_usable) {
            return false;
        }
        self.pet_toggles.borrow_mut().push(index);
        if let Some(s) = self.pet_slots.borrow_mut().get_mut(index as usize) {
            if s.autocast_allowed {
                s.autocast_enabled = !s.autocast_enabled;
            }
        }
        true
    }

    fn pet_set_spell_autocast(&mut self, spell_id: u32, state: Option<bool>) -> bool {
        TEST_PET_BOOK.with(|b| {
            let mut book = b.borrow_mut();
            let Some(slot) = book
                .as_mut()
                .and_then(|book| book.slots.iter_mut().rev().find(|s| s.spell_id == spell_id))
            else {
                return false;
            };
            let Some((true, on)) = slot.autocast else {
                return false;
            };
            let on = state.unwrap_or(!on);
            slot.autocast = Some((true, on));
            TEST_SPELL_AUTOCASTS.with(|a| a.borrow_mut().push((spell_id, on)));
            true
        })
    }

    fn pet_book(&self, _: std::time::Instant, _: f64) -> Option<super::PetBookState> {
        TEST_PET_BOOK.with(|b| b.borrow().clone())
    }
}

/// The drag's writes through a test view since the last take, one packet each.
#[cfg(test)]
pub(crate) fn take_test_pet_writes() -> Vec<Vec<(u32, u32)>> {
    TEST_PET_WRITES.with(|w| std::mem::take(&mut *w.borrow_mut()))
}

/// The `(spell, state)` writes the pet book's set made through a test view since the last take.
#[cfg(test)]
pub(crate) fn take_test_spell_autocasts() -> Vec<(u32, bool)> {
    TEST_SPELL_AUTOCASTS.with(|a| std::mem::take(&mut *a.borrow_mut()))
}

#[cfg(test)]
thread_local! {
    /// The pet book every test view answers, set by `UiScript::set_pet_book`.
    static TEST_PET_BOOK: std::cell::RefCell<Option<super::PetBookState>> =
        const { std::cell::RefCell::new(None) };
    /// The drag's writes through a test view, in order.
    static TEST_PET_WRITES: std::cell::RefCell<Vec<Vec<(u32, u32)>>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// The pet book's set writes through a test view, in order.
    static TEST_SPELL_AUTOCASTS: std::cell::RefCell<Vec<(u32, bool)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
thread_local! {
    /// The targeting cursor and the guids `SpellCanTargetUnit` passes, as every test view on this thread answers them.
    static TEST_TARGETING: std::cell::RefCell<(super::SpellTargetingLive, std::collections::HashSet<u64>)> =
        std::cell::RefCell::default();
    /// The known spells with the rank table, as every test view on this thread answers them.
    static TEST_KNOWN: std::cell::RefCell<(std::collections::HashSet<u32>, std::collections::HashMap<u32, u32>)> =
        std::cell::RefCell::default();
}

/// Move the targeting cursor on a bare VM as every test view on this thread answers it.
#[cfg(test)]
pub(crate) trait TestTargeting {
    /// The cursor up or down; going up counts an arm, as the cast's cursor arm does (`0x80d7f7`).
    fn set_spell_targeting(&mut self, active: bool);
    /// The word's item half (`& 0x4010`).
    fn set_item_pick_armed(&mut self, armed: bool);
    /// The guids `SpellCanTargetUnit` passes while the cursor is up.
    fn set_spell_targetable_units(&mut self, guids: impl IntoIterator<Item = u64>);
}

/// Answer `known` as the player's known spells on this thread, as [`set_test_book`] does.
#[cfg(test)]
pub(crate) fn set_test_known(known: impl IntoIterator<Item = u32>) {
    TEST_KNOWN.with(|k| k.borrow_mut().0 = known.into_iter().collect());
}

/// Answer `next` as the rank table the known test walks on this thread.
#[cfg(test)]
pub(crate) fn set_test_next_ranks(next: std::collections::HashMap<u32, u32>) {
    TEST_KNOWN.with(|k| k.borrow_mut().1 = next);
}

#[cfg(test)]
impl TestTargeting for UiScript {
    fn set_spell_targeting(&mut self, active: bool) {
        TEST_TARGETING.with(|t| {
            let t = &mut t.borrow_mut().0;
            if active && !t.active {
                t.arms = t.arms.wrapping_add(1);
            }
            t.active = active;
            if !active {
                t.wants_item = false;
            }
        });
    }

    fn set_item_pick_armed(&mut self, armed: bool) {
        TEST_TARGETING.with(|t| t.borrow_mut().0.wants_item = armed);
    }

    fn set_spell_targetable_units(&mut self, guids: impl IntoIterator<Item = u64>) {
        TEST_TARGETING.with(|t| t.borrow_mut().1 = guids.into_iter().collect());
    }
}

/// A test's pet book: the book before it comes back when it drops.
#[cfg(test)]
#[must_use = "the pet book answers only while the guard lives"]
pub(crate) struct TestPetBook {
    previous: Option<super::PetBookState>,
}

#[cfg(test)]
impl Drop for TestPetBook {
    fn drop(&mut self) {
        TEST_PET_BOOK.with(|b| *b.borrow_mut() = self.previous.take());
    }
}

/// Answer `state` as the pet book of every test view on this thread while the guard lives.
#[cfg(test)]
pub(crate) fn set_test_pet_book(state: super::PetBookState) -> TestPetBook {
    let previous = TEST_PET_BOOK.with(|b| b.borrow_mut().replace(state));
    TestPetBook { previous }
}

/// A view with a pet bar of `slots`, for the VM's own tests: `has_bar`, `actions_usable` and `pickup_allowed` as given.
#[cfg(test)]
pub(crate) fn lend_pets(
    has_bar: bool,
    actions_usable: bool,
    pickup_allowed: bool,
    slots: Vec<super::PetActionView>,
) -> TestView {
    TestView {
        pet_bar: Some(super::PetBarLive {
            has_bar,
            actions_usable,
            pickup_allowed,
            attacking: slots.iter().any(|s| s.attack_active),
            time_remaining_ms: None,
        }),
        pet_slots: std::cell::RefCell::new(slots),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A view whose one held unit carries `flags`.
    struct One {
        guid: u64,
        flags: u32,
    }

    impl LiveState for One {
        fn unit_flags(&self, guid: u64) -> Option<u32> {
            (guid == self.guid).then_some(self.flags)
        }

        fn unit_health(&self, guid: u64) -> Option<u32> {
            (guid == self.guid).then_some(1)
        }

        fn active_player(&self) -> Option<u64> {
            Some(self.guid)
        }

        fn has_control(&self) -> bool {
            true
        }

        fn far_sight(&self) -> bool {
            false
        }
    }

    #[test]
    fn a_view_is_lent_for_the_call_only_and_a_nested_loan_restores_the_outer() {
        let mut s = UiScript::new().unwrap();
        assert!(!s.has_live());
        let mut outer = One { guid: 1, flags: 7 };
        let mut inner = One { guid: 1, flags: 9 };
        s.with_live(&mut outer, |s| {
            assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(7)));
            s.with_live(&mut inner, |_| {
                assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(9)));
            });
            assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(7)));
        });
        assert!(!s.has_live());
        assert_eq!(read_live(|l| l.unit_flags(1)), None);
    }

    /// A Lua function reaches the view lent around the call that runs it, and nothing outside one.
    #[test]
    fn a_lua_function_reads_the_view_lent_around_its_call() {
        let mut s = UiScript::new().unwrap();
        let flags = s
            .lua()
            .create_function(|_, guid: u64| Ok(read_live(|l| l.unit_flags(guid)).flatten()))
            .unwrap();
        s.lua().globals().set("Flags", flags).unwrap();
        let ask = |s: &UiScript| s.eval::<Option<u32>>("return Flags(2)").unwrap();
        assert_eq!(ask(&s), None);
        let mut riding = One {
            guid: 2,
            flags: 1 << 20,
        };
        assert_eq!(s.with_live(&mut riding, |s| ask(s)), Some(1 << 20));
        assert_eq!(ask(&s), None);
    }

    /// `UnitOnTaxi` tests bit 20 of the lent view's flags word for whichever unit the token names
    /// (`0x60f396`); nil for a unit not held, and with no view lent.
    #[test]
    fn unit_on_taxi_reads_the_lent_view_for_any_unit() {
        let mut s = UiScript::new().unwrap();
        s.set_unit_guids(&crate::script::UnitGuids {
            player: 1,
            target: 2,
            held: [(1, 0), (2, 0)].into(),
            ..Default::default()
        });
        let ask = |s: &UiScript| {
            s.eval::<String>(
                r#"return tostring(UnitOnTaxi("player")) .. tostring(UnitOnTaxi("target"))"#,
            )
            .unwrap()
        };
        assert_eq!(ask(&s), "nilnil");
        let mut riding = One {
            guid: 2,
            flags: 1 << 20,
        };
        assert_eq!(s.with_live(&mut riding, |s| ask(s)), "nil1");
        let mut grounded = One {
            guid: 1,
            flags: !(1 << 20),
        };
        assert_eq!(s.with_live(&mut grounded, |s| ask(s)), "nilnil");
    }

    /// `HasFullControl` needs the active player held, not looking through far sight, with health
    /// above zero and the control flag set (`0x611603`-`0x611637`); with no view lent it answers
    /// the boot flag.
    #[test]
    fn has_full_control_reads_the_flag_and_the_players_health() {
        let mut s = UiScript::new().unwrap();
        let ask = |s: &mut UiScript, view: &mut TestView| {
            s.with_live(view, |s| {
                s.eval::<String>("return tostring(HasFullControl())")
            })
            .unwrap()
        };
        assert_eq!(s.eval::<i64>("return HasFullControl()").unwrap(), 1);
        let alive = |control| TestView {
            player: Some(1),
            units: [(1, (0, 100))].into(),
            control,
            ..Default::default()
        };
        assert_eq!(ask(&mut s, &mut alive(true)), "1");
        assert_eq!(ask(&mut s, &mut alive(false)), "nil");
        let mut far = TestView {
            far_sight: true,
            ..alive(true)
        };
        assert_eq!(ask(&mut s, &mut far), "nil");
        let mut dead = TestView {
            player: Some(1),
            units: [(1, (0, 0))].into(),
            control: true,
            ..Default::default()
        };
        assert_eq!(ask(&mut s, &mut dead), "nil");
        let mut nobody = TestView {
            control: true,
            ..Default::default()
        };
        assert_eq!(ask(&mut s, &mut nobody), "nil");
        let mut unheld = TestView {
            player: Some(1),
            control: true,
            ..Default::default()
        };
        assert_eq!(ask(&mut s, &mut unheld), "nil");
    }

    /// A loan is its thread's: another thread sees none.
    #[test]
    fn a_loan_is_the_lending_threads_alone() {
        let mut s = UiScript::new().unwrap();
        s.with_live(&mut One { guid: 1, flags: 5 }, |_| {
            let elsewhere = std::thread::spawn(|| read_live(|l| l.unit_flags(1)))
                .join()
                .unwrap();
            assert_eq!(elsewhere, None);
            assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(5)));
        });
    }

    #[test]
    fn an_unwinding_call_ends_the_loan() {
        let mut s = UiScript::new().unwrap();
        let mut view = One { guid: 1, flags: 7 };
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            s.with_live(&mut view, |_| panic!("a call that unwinds"));
        }));
        assert!(unwound.is_err());
        assert!(!s.has_live());
    }
}
