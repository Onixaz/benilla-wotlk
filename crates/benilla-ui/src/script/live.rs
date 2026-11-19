//! The client state the verbs read at call time. 12340's verbs read the live object when called
//! (`UnitOnTaxi` `0x60f350` reads the flags word at `0x60f396`); here the app lends a [`LiveState`]
//! over its own state for the length of every call into Lua, either around one call
//! ([`UiScript::with_live`]) or for as long as a guard lives ([`LiveLoan`]), and a verb reads it
//! through [`read_live`]. The loan is the calling thread's: the VM runs on one thread, and a test
//! thread's loan never reaches another's.

use std::cell::Cell;
use std::ptr::NonNull;

use super::UiScript;

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

    /// An item spell's range check from us toward `target` at the call (`0x809610`): `Some(None)` with no answer, `None` where no view lends the check.
    fn item_spell_in_range(&self, _spell_id: u32, _target: u64) -> Option<Option<bool>> {
        None
    }

    /// Queue `event` with the frame's unique signals (`0x615020`); false where no queue is lent.
    fn unique_signal(&mut self, _event: &'static str) -> bool {
        false
    }

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

    /// `NotifyInspect`'s `CMSG_INSPECT` and target (`0x5228c3`, `0x52291a`); whether the target moved (`0x5e76c1`).
    fn notify_inspect(&self, _guid: u64) -> bool {
        true
    }

    /// `ClearInspectPlayer`'s target clear (`0x5e78ae`).
    fn clear_inspect_target(&self) {}

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

    /// A press on 1-based `slot`, or the order word `order`, that writes only the bar (a reaction or a command other than Attack) and sends, inside the call (`0x5d4210`): `None` leaves it to the script call, else whether it signalled the bar (`0x5d4530`).
    fn pet_press(&mut self, _slot: u32, _order: Option<u32>) -> Option<bool> {
        None
    }

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

    /// The player's cooldown history for `spell_id` at the call, as `GetCompanionCooldown` asks it (`0x53e54c`): `(start_ms, duration_ms, enabled)` on the `GetTime` clock that `ui_now` seconds names at `anchor`; `None` with nothing running.
    fn spell_cooldown(
        &self,
        _spell_id: u32,
        _anchor: std::time::Instant,
        _ui_now: f64,
    ) -> Option<(i64, u32, bool)> {
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

    /// The refund record on held item `item` (`item+0x3d4`, filled by `0x6d1650`); `Some(None)` for none.
    fn refund_record(&self, _item: u64) -> Option<Option<super::RefundView>> {
        None
    }

    /// A bound item's trade-window stamp read off its fields at the call (`0x708b40`); `Some(None)` when it has none.
    fn bound_trade_stamp(&self, _item: u64) -> Option<Option<u32>> {
        None
    }

    /// The tooltip's ask of a refundable purchase's record when it holds none and none is pending (`0x62d57d`-`0x62d598`).
    fn ask_refund_record(&mut self, _item: u64) {}

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

/// A lent view's address, its lifetime erased.
type Lent = Option<NonNull<dyn LiveState>>;

thread_local! {
    /// The view lent on this thread: set only by [`UiScript::with_live`] and [`LiveLoan`], each of which puts the previous one back before its borrow ends.
    static SLOT: Cell<Lent> = const { Cell::new(None) };
    /// The borrows of what is lent, live now, by [`Hold`] kind: a shared read and a write never overlap, nor two verb writes, so none aliases another.
    static HELD: Cell<[u32; 3]> = const { Cell::new([0; 3]) };
}

/// What a borrow of the lent state does.
#[derive(Clone, Copy)]
enum Hold {
    /// A read: a verb's, the host's view, or the host's read of a lendable resource.
    Shared = 0,
    /// The host's write to a lendable resource its view reads.
    Owner = 1,
    /// A verb's write to the view.
    Verb = 2,
}

/// One borrow of the lent state, counted in [`HELD`] until it drops.
struct Held(Hold);

impl Held {
    /// Count a borrow; one that would alias a live one panics.
    fn new(hold: Hold) -> Self {
        HELD.with(|h| {
            let mut held = h.get();
            let [shared, owner, verb] = held;
            let clear = match hold {
                Hold::Shared => owner == 0 && verb == 0,
                Hold::Owner => shared == 0 && verb == 0,
                Hold::Verb => shared == 0 && owner == 0 && verb == 0,
            };
            assert!(
                clear,
                "the lent state is borrowed as {} beside {held:?} (shared, owner, verb) borrows",
                ["shared", "its owner's write", "a verb's write"][hold as usize],
            );
            held[hold as usize] += 1;
            h.set(held);
        });
        Self(hold)
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        HELD.with(|h| {
            let mut held = h.get();
            held[self.0 as usize] -= 1;
            h.set(held);
        });
    }
}

/// A host's write to state its lent view reads, held while the write's borrow lives: no verb reaches the view meanwhile, and no shared read of it may be held.
pub struct OwnerWrite {
    _held: Held,
}

/// Start an owner's write; it ends when the guard drops.
pub fn owner_write() -> OwnerWrite {
    OwnerWrite {
        _held: Held::new(Hold::Owner),
    }
}

/// A host's shared read of state its lent view reads, held while the read's borrow lives: no owner writes it meanwhile.
pub struct SharedRead {
    _held: Held,
}

/// Start a host's shared read; it ends when the guard drops.
pub fn shared_read() -> SharedRead {
    SharedRead {
        _held: Held::new(Hold::Shared),
    }
}

/// The lent view, read by its host between calls into Lua; a verb that writes the view panics while it lives.
pub struct ViewRef<'a, V> {
    view: &'a V,
    _held: Held,
}

impl<V> std::ops::Deref for ViewRef<'_, V> {
    type Target = V;

    fn deref(&self) -> &V {
        self.view
    }
}

/// Lend `live` on this thread, returning what it replaces.
///
/// # Safety
/// `live` is borrowed mutably until the caller puts the returned value back with [`restore`], and
/// nothing else reaches it in between.
unsafe fn lend(live: NonNull<dyn LiveState + '_>) -> Lent {
    // SAFETY: only the lifetime is erased; the caller restores the slot before the borrow ends,
    // and `read_live` and `write_live` hand the view out only for the length of one closure.
    let ptr: NonNull<dyn LiveState + 'static> = unsafe { std::mem::transmute(live) };
    SLOT.with(|s| s.replace(Some(ptr)))
}

fn restore(previous: Lent) {
    SLOT.with(|s| s.set(previous));
}

/// Puts the slot back as it was, on return and on unwind.
struct Restore(Lent);

impl Drop for Restore {
    fn drop(&mut self) {
        restore(self.0);
    }
}

/// A view lent to every verb on this thread until the loan drops, which puts the previous one back: the guard a host keeps for the length of a stretch of calls into Lua.
pub struct LiveLoan<V: LiveState> {
    /// The boxed view, owned through this pointer, so the lent address holds while the loan moves.
    view: NonNull<V>,
    previous: Lent,
}

impl<V: LiveState> LiveLoan<V> {
    /// Lend `view` until the loan drops.
    ///
    /// # Safety
    /// The loan is dropped, never leaked, on the thread that made it, and loans on one thread
    /// drop in the reverse order of their making.
    pub unsafe fn new(view: V) -> Self {
        let view = NonNull::from(Box::leak(Box::new(view)));
        // SAFETY: `Drop` restores the slot before the box, and with it the view, is freed.
        let previous = unsafe { lend(view) };
        Self { view, previous }
    }

    /// The lent view, read between calls into Lua; a verb's write to it panics while this lives.
    pub fn view(&self) -> ViewRef<'_, V> {
        let held = Held::new(Hold::Shared);
        // SAFETY: the box lives until the drop; a verb's mutable borrow of it panics while `held` counts this one, and a verb's shared one ends with its closure.
        ViewRef {
            view: unsafe { self.view.as_ref() },
            _held: held,
        }
    }

    /// `f` over the lent view; a verb writing through the loan from inside `f` panics.
    pub fn with_view<R>(&self, f: impl FnOnce(&V) -> R) -> R {
        f(&self.view())
    }

    /// `f` over the lent view, to change it between calls into Lua; a verb reaching the loan from inside `f` panics.
    pub fn with_view_mut<R>(&mut self, f: impl FnOnce(&mut V) -> R) -> R {
        let _held = Held::new(Hold::Verb);
        // SAFETY: the box lives until the drop, and `_held` keeps every other borrow of the view
        // out while the reference, which cannot escape `f`, is out.
        f(unsafe { self.view.as_mut() })
    }
}

impl<V: LiveState> Drop for LiveLoan<V> {
    fn drop(&mut self) {
        restore(self.previous);
        // SAFETY: made by `Box::leak` in `new` and no longer lent.
        drop(unsafe { Box::from_raw(self.view.as_ptr()) });
    }
}

impl UiScript {
    /// Run `f` with `live` lent to every verb it reaches; the loan ends before this returns, and a nested call lends its own and restores the outer one.
    pub fn with_live<R>(&mut self, live: &mut dyn LiveState, f: impl FnOnce(&mut Self) -> R) -> R {
        // SAFETY: `Restore` puts the slot back before `live`'s borrow ends, on unwind too.
        let _restore = Restore(unsafe { lend(NonNull::from(live)) });
        f(self)
    }

    /// Whether a view is lent right now.
    pub fn has_live(&self) -> bool {
        SLOT.with(|s| s.get().is_some())
    }
}

/// The UI's control flag through the lent view; with none lent, as a VM outside the app has, the
/// value a game start sets (`0x52b8a0`).
pub(crate) fn has_control() -> bool {
    read_live(|l| l.has_control()).unwrap_or(true)
}

/// Queue `event` with the frame's unique signals (`0x615020`), which fire once each after the frame's packets (`0x614760`); with no queue lent, the model's pending events take it.
pub(crate) fn unique_signal(model: &mut super::model::Model, event: &'static str) {
    if write_live(|l| l.unique_signal(event)) != Some(true) {
        model.pending_events.push((event.to_string(), Vec::new()));
    }
}

/// `f` over the view lent right now, `None` with none lent.
pub fn read_live<R>(f: impl FnOnce(&dyn LiveState) -> R) -> Option<R> {
    let ptr = SLOT.with(Cell::get)?;
    let _held = Held::new(Hold::Shared);
    // SAFETY: a pointer in the slot is a mutable borrow held by an enclosing `with_live` or `LiveLoan`, which outlives this call; `_held` panics beside an owner's write, the view never calls back into Lua, and the reference cannot escape `f`.
    let live = unsafe { ptr.as_ref() };
    Some(f(live))
}

/// `f` over the view lent right now, mutably, for a verb that writes the state it reads; `None` with none lent.
pub fn write_live<R>(f: impl FnOnce(&mut dyn LiveState) -> R) -> Option<R> {
    let mut ptr = SLOT.with(Cell::get)?;
    let _held = Held::new(Hold::Verb);
    // SAFETY: as `read_live`; `_held` panics beside any of the host's own borrows of the view, so nothing else reaches it for the length of `f`.
    let live = unsafe { ptr.as_mut() };
    Some(f(live))
}

/// Lend `view` as the live view until the loan drops, for the VM's own tests.
#[cfg(test)]
pub(crate) fn lend_test(view: TestView) -> LiveLoan<TestView> {
    // SAFETY: a test drops its loans on its own thread, the latest first.
    unsafe { LiveLoan::new(view) }
}

/// Lend `slots` as our arena-team slots, for the VM's own tests.
#[cfg(test)]
pub(crate) fn lend_arena_teams(slots: [super::ArenaTeamSlotView; 3]) -> LiveLoan<TestView> {
    lend_test(TestView {
        arena_teams: slots,
        ..Default::default()
    })
}

/// A view over fixed state, for the VM's own tests.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestView {
    /// The active player's guid.
    pub player: Option<u64>,
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
    /// The guild bank, its locks as `(tab, slot, item)`, the current tab and the two moneys.
    pub guild_bank: super::GuildBankView,
    pub guild_bank_locks: Vec<(u8, u8, u32)>,
    pub guild_bank_tab: u8,
    pub guild_bank_money: u64,
    pub guild_bank_withdraw_money: i32,
    /// The tabs whose text is stale, and the text asks sent, in order.
    pub guild_bank_text_stale: Vec<u8>,
    pub guild_bank_text_sent: Vec<u8>,
    /// The item cache's records, and each read made, `(id, tooltip)`, in order.
    pub item_records: std::cell::RefCell<std::collections::HashMap<u32, super::ItemTemplateView>>,
    pub item_record_reads: std::cell::RefCell<Vec<(u32, bool)>>,
    /// The bind point's area name.
    pub bind_location: std::cell::RefCell<Option<String>>,
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
    fn item_tables(&self) -> Option<&super::ItemTables> {
        self.item_tables.as_ref()
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

    fn notify_inspect(&self, guid: u64) -> bool {
        self.inspect_target.replace(guid) != guid
    }

    fn clear_inspect_target(&self) {
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
        self.cooldowns.get(&spell_id).copied()
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

/// A test's pet book: the book before it comes back, and the view it lent with none up drops, when it drops.
#[cfg(test)]
#[must_use = "the pet book answers only while the guard lives"]
pub(crate) struct TestPetBook {
    previous: Option<super::PetBookState>,
    loan: Option<LiveLoan<TestView>>,
}

#[cfg(test)]
impl Drop for TestPetBook {
    fn drop(&mut self) {
        TEST_PET_BOOK.with(|b| *b.borrow_mut() = self.previous.take());
        drop(self.loan.take());
    }
}

/// Answer `state` as the pet book of every test view on this thread while the guard lives, lending one if none is up yet.
#[cfg(test)]
pub(crate) fn set_test_pet_book(state: super::PetBookState) -> TestPetBook {
    let previous = TEST_PET_BOOK.with(|b| b.borrow_mut().replace(state));
    // SAFETY: the guard drops on this thread, in scope order with the loans made after it.
    let loan = read_live(|_| ())
        .is_none()
        .then(|| unsafe { LiveLoan::new(TestView::default()) });
    TestPetBook { previous, loan }
}

/// Lend a pet bar with `slots`, for the VM's own tests: `has_bar`, `actions_usable` and `pickup_allowed` as given.
#[cfg(test)]
pub(crate) fn lend_pets(
    has_bar: bool,
    actions_usable: bool,
    pickup_allowed: bool,
    slots: Vec<super::PetActionView>,
) -> LiveLoan<TestView> {
    lend_test(TestView {
        pet_bar: Some(super::PetBarLive {
            has_bar,
            actions_usable,
            pickup_allowed,
            attacking: slots.iter().any(|s| s.attack_active),
            time_remaining_ms: None,
        }),
        pet_slots: std::cell::RefCell::new(slots),
        ..Default::default()
    })
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

    /// A loan lends until it drops, a call's loan inside it restores it, and dropping it restores
    /// what it replaced.
    #[test]
    fn a_loan_lends_until_it_drops() {
        let mut s = UiScript::new().unwrap();
        let mut outer = One { guid: 1, flags: 7 };
        // SAFETY: dropped below, on this thread, after the inner one.
        let loan = unsafe { LiveLoan::new(One { guid: 1, flags: 5 }) };
        assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(5)));
        s.with_live(&mut outer, |_| {
            assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(7)));
            // SAFETY: dropped at the end of this closure, before `loan`.
            let inner = unsafe { LiveLoan::new(One { guid: 1, flags: 9 }) };
            assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(9)));
            drop(inner);
            assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(7)));
        });
        assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(5)));
        assert_eq!(loan.view().flags, 5);
        drop(loan);
        assert!(!s.has_live());
    }

    /// A loan is its thread's: another thread sees none.
    #[test]
    fn a_loan_is_the_lending_threads_alone() {
        // SAFETY: dropped at the end of this test, on this thread.
        let _loan = unsafe { LiveLoan::new(One { guid: 1, flags: 5 }) };
        let elsewhere = std::thread::spawn(|| read_live(|l| l.unit_flags(1)))
            .join()
            .unwrap();
        assert_eq!(elsewhere, None);
        assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(5)));
    }

    /// A reference out of a loan shuts the verbs out of it, writes while the host reads and everything while it writes, until the reference is back.
    #[test]
    fn a_reference_out_of_a_loan_shuts_the_verbs_out() {
        let s = UiScript::new().unwrap();
        let lua = s.lua();
        let write = lua
            .create_function(|_, ()| Ok(write_live(|_| ()).is_some()))
            .unwrap();
        let read = lua
            .create_function(|_, ()| Ok(read_live(|l| l.unit_flags(1)).flatten()))
            .unwrap();
        lua.globals().set("Write", write).unwrap();
        lua.globals().set("Read", read).unwrap();
        let reached = |s: &UiScript, chunk: &str| {
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                s.eval::<mlua::Value>(chunk)
                    .map(|v| !matches!(v, mlua::Value::Nil))
            }));
            matches!(run, Ok(Ok(true)))
        };
        // SAFETY: dropped at the end of this test, on this thread.
        let mut loan = unsafe { LiveLoan::new(One { guid: 1, flags: 5 }) };
        loan.with_view(|v| {
            assert_eq!(v.flags, 5);
            assert!(!reached(&s, "return Write() or nil"));
            assert!(reached(&s, "return Read()"));
            assert_eq!(v.flags, 5);
        });
        loan.with_view_mut(|v| {
            v.flags = 6;
            assert!(!reached(&s, "return Write() or nil"));
            assert!(!reached(&s, "return Read()"));
            v.flags = 7;
        });
        assert!(reached(&s, "return Write() or nil"));
        assert_eq!(s.eval::<Option<u32>>("return Read()").unwrap(), Some(7));
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
