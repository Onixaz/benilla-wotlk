//! The client state the verbs read at call time. 12340's verbs read the live object when called
//! (`UnitOnTaxi` `0x60f350` reads the flags word at `0x60f396`); here the app lends a [`LiveState`]
//! over its own state for the length of every call into Lua, either around one call
//! ([`UiScript::with_live`]) or for as long as a guard lives ([`LiveLoan`]), and a verb reads it
//! through [`read_live`]. The loan is the calling thread's: the VM runs on one thread, and a test
//! thread's loan never reaches another's.

use std::cell::Cell;
use std::ptr::NonNull;

use super::UiScript;

/// What the verbs read live, implemented by the app over its state; the `&mut self` methods, reached through [`write_live`], write what a verb changes in its call.
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
}

/// A lent view's address, its lifetime erased, and whether its loan lets verbs write through it.
type Lent = Option<(NonNull<dyn LiveState>, bool)>;

/// No reference into the lent view is out.
const FREE: u32 = 0;
/// A [`write_live`] closure holds the view.
const WRITING: u32 = u32::MAX;

thread_local! {
    /// The view lent on this thread, set only by [`UiScript::with_live`], [`UiScript::with_live_mut`] and [`LiveLoan`], each restoring the previous one.
    static SLOT: Cell<Lent> = const { Cell::new(None) };
    /// The references into the lent view now out: [`FREE`], a count of readers, or [`WRITING`].
    static HELD: Cell<u32> = const { Cell::new(FREE) };
}

/// Lend `live` on this thread, returning what it replaces.
///
/// # Safety
/// The caller puts the returned value back with [`restore`] before `live`'s borrow ends; `live` came from a `&mut` borrow when `writable`.
unsafe fn lend(live: NonNull<dyn LiveState + '_>, writable: bool) -> Lent {
    // SAFETY: only the lifetime is erased; the caller restores the slot before the borrow ends,
    // and `read_live` and `write_live` hand the view out only for the length of one closure.
    let ptr: NonNull<dyn LiveState + 'static> = unsafe { std::mem::transmute(live) };
    SLOT.with(|s| s.replace(Some((ptr, writable))))
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

/// A view lent to every verb on this thread until the loan drops, which restores the previous one.
pub struct LiveLoan<V: LiveState> {
    /// Boxed and held raw, so the lent address holds while the loan moves and [`Self::with_view_mut`] shares its borrow.
    view: NonNull<V>,
    previous: Lent,
}

impl<V: LiveState> LiveLoan<V> {
    /// Lend `view` until the loan drops; verbs may write through it.
    ///
    /// # Safety
    /// The loan is dropped, never leaked, on the thread that made it, and loans on one thread
    /// drop in the reverse order of their making.
    pub unsafe fn new(view: V) -> Self {
        let view = NonNull::from(Box::leak(Box::new(view)));
        // SAFETY: `Drop` restores the slot before the box, and with it the view, is freed.
        let previous = unsafe { lend(view as NonNull<dyn LiveState + '_>, true) };
        Self { view, previous }
    }

    /// `f` over the lent view; a verb writing through the loan from inside `f` panics.
    pub fn with_view<R>(&self, f: impl FnOnce(&V) -> R) -> R {
        let held = HELD.with(Cell::get);
        assert_ne!(held, WRITING, "a loan's view was read inside a write");
        HELD.with(|h| h.set(held + 1));
        let _release = Release(held);
        // SAFETY: the box lives until the drop, and the hold count keeps every writer out while
        // the reference, which cannot escape `f`, is out.
        f(unsafe { self.view.as_ref() })
    }

    /// `f` over the lent view, to change it between calls into Lua; a verb reaching the loan from inside `f` panics.
    pub fn with_view_mut<R>(&mut self, f: impl FnOnce(&mut V) -> R) -> R {
        let held = HELD.with(Cell::get);
        assert_eq!(
            held, FREE,
            "a loan's view was written while a reference into it was out"
        );
        HELD.with(|h| h.set(WRITING));
        let _release = Release(FREE);
        // SAFETY: the box lives until the drop, and the hold count keeps every reader and writer
        // out while the reference, which cannot escape `f`, is out.
        f(unsafe { self.view.as_mut() })
    }
}

impl<V: LiveState> Drop for LiveLoan<V> {
    fn drop(&mut self) {
        restore(self.previous);
        // SAFETY: made by `Box::leak` in `new`, and the slot no longer names it.
        drop(unsafe { Box::from_raw(self.view.as_ptr()) });
    }
}

impl UiScript {
    /// Run `f` with `live` lent read-only to every verb it reaches, a nested call restoring the outer loan.
    pub fn with_live<R>(&mut self, live: &dyn LiveState, f: impl FnOnce(&mut Self) -> R) -> R {
        // SAFETY: `Restore` puts the slot back before `live`'s borrow ends, on unwind too; the
        // loan is read-only, so the shared borrow is never written through.
        let _restore = Restore(unsafe { lend(NonNull::from(live), false) });
        f(self)
    }

    /// [`Self::with_live`], with the verbs' writes going through `live`.
    pub fn with_live_mut<R>(
        &mut self,
        live: &mut dyn LiveState,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        // SAFETY: `Restore` puts the slot back before `live`'s borrow ends, on unwind too.
        let _restore = Restore(unsafe { lend(NonNull::from(live), true) });
        f(self)
    }

    /// Whether a view is lent right now.
    pub fn has_live(&self) -> bool {
        SLOT.with(|s| s.get().is_some())
    }
}

/// Puts the hold count back, on return and on unwind.
struct Release(u32);

impl Drop for Release {
    fn drop(&mut self) {
        HELD.with(|h| h.set(self.0));
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
///
/// # Panics
/// Inside a [`write_live`] closure: a verb's write never reaches back into Lua.
pub fn read_live<R>(f: impl FnOnce(&dyn LiveState) -> R) -> Option<R> {
    let (ptr, _) = SLOT.with(Cell::get)?;
    let held = HELD.with(Cell::get);
    assert_ne!(
        held, WRITING,
        "the live view was read inside a write through it"
    );
    HELD.with(|h| h.set(held + 1));
    let _release = Release(held);
    // SAFETY: a pointer in the slot is a live borrow held by an enclosing `with_live` or
    // `LiveLoan`, which outlives this call; no writer holds it, and the reference cannot escape `f`.
    let live = unsafe { ptr.as_ref() };
    Some(f(live))
}

/// `f` over the view lent right now, to write what a verb changes in its call; `None` with none lent or a read-only loan.
///
/// # Panics
/// Inside a [`read_live`] or [`write_live`] closure: one writer and no reader at a time.
pub fn write_live<R>(f: impl FnOnce(&mut dyn LiveState) -> R) -> Option<R> {
    let (mut ptr, writable) = SLOT.with(Cell::get)?;
    if !writable {
        return None;
    }
    let held = HELD.with(Cell::get);
    assert_eq!(
        held, FREE,
        "the live view was written while a reference into it was out"
    );
    HELD.with(|h| h.set(WRITING));
    let _release = Release(FREE);
    // SAFETY: a writable slot holds a live `&mut` borrow lent by an enclosing `with_live_mut` or
    // `LiveLoan`, which outlives this call; no other reference into it is out, and this one
    // cannot escape `f`.
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
}

#[cfg(test)]
impl LiveState for TestView {
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
        let outer = One { guid: 1, flags: 7 };
        let inner = One { guid: 1, flags: 9 };
        s.with_live(&outer, |s| {
            assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(7)));
            s.with_live(&inner, |_| {
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
        let riding = One {
            guid: 2,
            flags: 1 << 20,
        };
        assert_eq!(s.with_live(&riding, |s| ask(s)), Some(1 << 20));
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
        let riding = One {
            guid: 2,
            flags: 1 << 20,
        };
        assert_eq!(s.with_live(&riding, |s| ask(s)), "nil1");
        let grounded = One {
            guid: 1,
            flags: !(1 << 20),
        };
        assert_eq!(s.with_live(&grounded, |s| ask(s)), "nilnil");
    }

    /// `HasFullControl` needs the active player held, not looking through far sight, with health
    /// above zero and the control flag set (`0x611603`-`0x611637`); with no view lent it answers
    /// the boot flag.
    #[test]
    fn has_full_control_reads_the_flag_and_the_players_health() {
        let mut s = UiScript::new().unwrap();
        let ask = |s: &mut UiScript, view: &TestView| {
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
        assert_eq!(ask(&mut s, &alive(true)), "1");
        assert_eq!(ask(&mut s, &alive(false)), "nil");
        let far = TestView {
            far_sight: true,
            ..alive(true)
        };
        assert_eq!(ask(&mut s, &far), "nil");
        let dead = TestView {
            player: Some(1),
            units: [(1, (0, 0))].into(),
            control: true,
            ..Default::default()
        };
        assert_eq!(ask(&mut s, &dead), "nil");
        let nobody = TestView {
            control: true,
            ..Default::default()
        };
        assert_eq!(ask(&mut s, &nobody), "nil");
        let unheld = TestView {
            player: Some(1),
            control: true,
            ..Default::default()
        };
        assert_eq!(ask(&mut s, &unheld), "nil");
    }

    /// A loan lends until it drops, a call's loan inside it restores it, and dropping it restores
    /// what it replaced.
    #[test]
    fn a_loan_lends_until_it_drops() {
        let mut s = UiScript::new().unwrap();
        let outer = One { guid: 1, flags: 7 };
        // SAFETY: dropped below, on this thread, after the inner one.
        let loan = unsafe { LiveLoan::new(One { guid: 1, flags: 5 }) };
        assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(5)));
        s.with_live(&outer, |_| {
            assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(7)));
            // SAFETY: dropped at the end of this closure, before `loan`.
            let inner = unsafe { LiveLoan::new(One { guid: 1, flags: 9 }) };
            assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(9)));
            drop(inner);
            assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(7)));
        });
        assert_eq!(read_live(|l| l.unit_flags(1)), Some(Some(5)));
        assert_eq!(loan.with_view(|v| v.flags), 5);
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
        let view = One { guid: 1, flags: 7 };
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            s.with_live(&view, |_| panic!("a call that unwinds"));
        }));
        assert!(unwound.is_err());
        assert!(!s.has_live());
    }
}
