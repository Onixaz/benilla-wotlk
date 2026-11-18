//! The `World` lent to every call into Lua as its [`LiveState`], as 12340's verbs read the live object at the call (`0x60f396`), and the [`UiEvents`] a packet handler fires once its changes apply (`0x520fe0`).

use benilla_ui::script::{LiveLoan, LiveState, ScriptValue, UiScript};
use bevy::ecs::change_detection::Tick;
use bevy::ecs::query::FilteredAccessSet;
use bevy::ecs::system::{SystemMeta, SystemParam, SystemParamValidationError, SystemState};
use bevy::ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use std::ptr::NonNull;

use crate::ui_quest::QuestGiver;
use crate::ui_quest_log::rows::{QuestAsks, RebuildMarks};
use crate::ui_quest_log::QuestExpiries;

/// What the verbs read live, a system parameter so a VM system lends it with its own access. The
/// index is optional, so an app without the net plugin still lends a view that holds nobody.
#[derive(SystemParam)]
pub(crate) struct LiveView<'w, 's> {
    index: Option<Res<'w, crate::net::GuidIndex>>,
    stores: Query<'w, 's, &'static crate::net::ObjectStore>,
    control: Option<Res<'w, crate::net::ControlFlag>>,
    self_guid: Option<Res<'w, crate::net::SelfGuid>>,
    far_sight_clear: Option<Res<'w, crate::player::view_subject::FarSightClear>>,
    death: crate::death::DeathView<'w, 's>,
    summon: crate::ui_summon::SummonView<'w>,
    quest: crate::ui_quest_watch::QuestView<'w, 's>,
    clock: Option<Res<'w, crate::net::ServerWallClock>>,
    battlefield: crate::ui_battlefield::BattlefieldView<'w, 's>,
    score: crate::ui_battlefield_score::ScoreView<'w>,
    arena: Option<Res<'w, crate::ui_arena::ArenaOpponents>>,
    honor: crate::ui_honor::HonorView<'w>,
    arena_teams: crate::ui_arena_team::ArenaTeamView<'w>,
    positions: crate::ui_battlefield_positions::PositionsView<'w, 's>,
    inspect: crate::ui_inspect::InspectLive<'w>,
    world_state_ui: crate::world_state_ui::WorldStateUiLive<'w>,
    item_range: crate::ui_quest_watch::ItemRangeLive<'w, 's>,
    quest_tooltip: crate::ui_quest_log::tooltip::QuestTooltipLive<'w, 's>,
    quest_rows: crate::ui_quest_log::rows::QuestRowLive<'w, 's>,
    net: Option<Res<'w, crate::net::NetCommands>>,
    _mark: Option<Res<'w, LendsLive>>,
}

/// The main-thread state a verb writes inside its call, as 12340's verbs write the game's globals: each lent mutably beside the [`LiveView`], by [`LendLive`] or by the system that owns the borrow ([`LendLive::lending`]).
#[derive(Default)]
pub(crate) struct Lent<'w> {
    pub(crate) signals: LentSlot<'w, UniqueSignals>,
    pub(crate) asks: LentSlot<'w, QuestAsks>,
    pub(crate) marks: LentSlot<'w, RebuildMarks>,
    pub(crate) expiries: LentSlot<'w, QuestExpiries>,
    pub(crate) giver: LentSlot<'w, QuestGiver>,
}

/// The bits a system's [`LendLive`] names the [`Lent`] state it writes itself by.
pub(crate) mod lent {
    pub(crate) const SIGNALS: u8 = 1;
    pub(crate) const ASKS: u8 = 2;
    pub(crate) const MARKS: u8 = 4;
    pub(crate) const EXPIRIES: u8 = 8;
    pub(crate) const GIVER: u8 = 16;
}

/// One kind of [`Lent`] state.
pub(crate) trait Lendable: Sized + 'static {
    /// Its bit in [`lent`].
    const BIT: u8;
    /// Its slot.
    fn slot<'a, 'w>(lent: &'a mut Lent<'w>) -> &'a mut LentSlot<'w, Self>;
}

macro_rules! lendable {
    ($ty:ty, $field:ident, $bit:path) => {
        impl Lendable for $ty {
            const BIT: u8 = $bit;
            fn slot<'a, 'w>(lent: &'a mut Lent<'w>) -> &'a mut LentSlot<'w, Self> {
                &mut lent.$field
            }
        }
    };
}
lendable!(UniqueSignals, signals, lent::SIGNALS);
lendable!(QuestAsks, asks, lent::ASKS);
lendable!(RebuildMarks, marks, lent::MARKS);
lendable!(QuestExpiries, expiries, lent::EXPIRIES);
lendable!(QuestGiver, giver, lent::GIVER);

/// One piece of [`Lent`] state: the borrow [`LendLive`] took, or the one its owner lends for the call in progress.
pub(crate) struct LentSlot<'w, T: 'static> {
    fetched: Option<NonSendMut<'w, T>>,
    /// Set only inside [`LendLive::lending`], from a `&mut` that outlives it.
    overlay: Option<NonNull<T>>,
}

impl<T: 'static> Default for LentSlot<'_, T> {
    fn default() -> Self {
        Self {
            fetched: None,
            overlay: None,
        }
    }
}

impl<T: 'static> LentSlot<'_, T> {
    pub(crate) fn get(&self) -> Option<&T> {
        match self.overlay {
            // SAFETY: an overlay is a live `&mut` lent for the length of the call in progress.
            Some(p) => Some(unsafe { p.as_ref() }),
            None => self.fetched.as_deref(),
        }
    }

    pub(crate) fn get_mut(&mut self) -> Option<&mut T> {
        match self.overlay {
            // SAFETY: as `get`; `&mut self` keeps this the only reference made through it.
            Some(mut p) => Some(unsafe { p.as_mut() }),
            None => self.fetched.as_deref_mut(),
        }
    }

    /// Whether the borrow was written since the system last ran; an owner's loan counts as written.
    fn changed(&self) -> bool {
        self.overlay.is_some() || self.fetched.as_ref().is_some_and(|f| f.is_changed())
    }
}

/// Every piece of [`Lent`] state, borrowed by an exclusive lender.
#[derive(SystemParam)]
pub(crate) struct LentAll<'w> {
    signals: Option<NonSendMut<'w, UniqueSignals>>,
    asks: Option<NonSendMut<'w, QuestAsks>>,
    marks: Option<NonSendMut<'w, RebuildMarks>>,
    expiries: Option<NonSendMut<'w, QuestExpiries>>,
    giver: Option<NonSendMut<'w, QuestGiver>>,
}

impl<'w> From<LentAll<'w>> for Lent<'w> {
    fn from(all: LentAll<'w>) -> Self {
        Self {
            signals: LentSlot {
                fetched: all.signals,
                overlay: None,
            },
            asks: LentSlot {
                fetched: all.asks,
                overlay: None,
            },
            marks: LentSlot {
                fetched: all.marks,
                overlay: None,
            },
            expiries: LentSlot {
                fetched: all.expiries,
                overlay: None,
            },
            giver: LentSlot {
                fetched: all.giver,
                overlay: None,
            },
        }
    }
}

/// What a verb is lent: the [`LiveView`] it reads and the [`Lent`] state it writes.
pub(crate) struct LiveNow<'w, 's> {
    view: LiveView<'w, 's>,
    lent: Lent<'w>,
}

impl<'w, 's> std::ops::Deref for LiveNow<'w, 's> {
    type Target = LiveView<'w, 's>;
    fn deref(&self) -> &Self::Target {
        &self.view
    }
}

impl LiveView<'_, '_> {
    /// A held unit's fields: the verbs' unit lookup (`0x60c1f0`) asks for type mask 8 (`0x60c21e`).
    fn unit(&self, guid: u64) -> Option<&benilla_protocol::ObjectFields> {
        let &entity = self.index.as_ref()?.0.get(&guid)?;
        let fields = &self.stores.get(entity).ok()?.0;
        fields.is_unit().then_some(fields)
    }

    /// A held player's entity, the object lookup with the player type mask (`0x54c3e7`).
    fn player_entity(&self, guid: u64) -> Option<Entity> {
        let &entity = self.index.as_ref()?.0.get(&guid)?;
        let fields = &self.stores.get(entity).ok()?.0;
        (fields.object_type() == Some(benilla_protocol::messages::ObjectType::Player))
            .then_some(entity)
    }

    /// A held player's `(race, class)`, the talent target's lookup with the player type mask (`0x5c9f8f`).
    fn player_class(&self, guid: u64) -> Option<(u8, u8)> {
        let fields = &self.stores.get(self.player_entity(guid)?).ok()?.0;
        Some((fields.unit_race()?, fields.unit_class()?))
    }

    /// A held player's PvP side (`0x54d074`).
    fn player_side(&self, guid: u64) -> Option<i8> {
        let entity = self.player_entity(guid)?;
        let race = self.stores.get(entity).ok()?.0.unit_race()?;
        Some(crate::ui_unit::race_pvp_team(race))
    }
}

/// Never inserted: read by every [`LiveView`], so a system's declared access shows it lends.
#[derive(Resource)]
pub(crate) struct LendsLive;

impl LiveState for LiveNow<'_, '_> {
    fn unit_flags(&self, guid: u64) -> Option<u32> {
        self.unit(guid).map(|f| f.unit_flags())
    }

    fn unit_health(&self, guid: u64) -> Option<u32> {
        self.unit(guid).map(|f| f.unit_health().unwrap_or(0))
    }

    fn unit_dynamic_flags(&self, guid: u64) -> Option<u32> {
        self.unit(guid).map(|f| f.unit_dynamic_flags())
    }

    /// The type mask's player bit first, as `UnitIsGhost` tests it (`0x60f5fe`).
    fn player_flags(&self, guid: u64) -> Option<u32> {
        self.unit(guid)
            .filter(|f| f.object_type() == Some(benilla_protocol::messages::ObjectType::Player))
            .map(|f| f.player_flags())
    }

    fn active_player(&self) -> Option<u64> {
        self.self_guid.as_ref()?.0
    }

    /// An app without the net plugin boots in control.
    fn has_control(&self) -> bool {
        self.control.as_ref().is_none_or(|c| c.0)
    }

    /// Our `PLAYER_FARSIGHT` naming a held object, as the bit needs (`0x6e28c6`-`0x6e28ff`), and
    /// no immediate clear ending it (`0x6e2b00`).
    fn far_sight(&self) -> bool {
        let Some(index) = self.index.as_ref() else {
            return false;
        };
        let field = self
            .active_player()
            .and_then(|me| index.0.get(&me))
            .and_then(|&e| self.stores.get(e).ok())
            .and_then(|store| store.0.player_farsight());
        field.is_some_and(|subject| {
            index.0.contains_key(&subject)
                && self
                    .far_sight_clear
                    .as_ref()
                    .is_none_or(|c| !c.cuts(subject))
        })
    }

    fn release_time_left_ms(&self) -> Option<u32> {
        self.death.release_time_left_ms()
    }

    fn corpse_recovery_delay_ms(&self) -> u32 {
        self.death.corpse_recovery_delay_ms()
    }

    fn resurrect_sickness(&self) -> bool {
        self.death.resurrect_sickness()
    }

    fn resurrect_has_timer(&self) -> bool {
        self.death.resurrect_has_timer()
    }

    fn spirit_healer_in_range(&self) -> bool {
        self.death.spirit_healer_in_range()
    }

    fn res_sickness_minutes(&self) -> Option<u32> {
        self.death.res_sickness_minutes()
    }

    fn resurrect_offerer(&self) -> Option<String> {
        self.death.resurrect_offerer()
    }

    fn self_res_label(&self) -> Option<String> {
        self.death.self_res_label()
    }

    fn falling(&self) -> bool {
        self.death.falling()
    }

    fn out_of_bounds(&self) -> bool {
        self.death.out_of_bounds()
    }

    fn cannot_be_resurrected(&self) -> bool {
        self.death.cannot_be_resurrected()
    }

    fn summon_summoner(&self) -> String {
        self.summon.summoner()
    }

    fn summon_area(&self) -> String {
        self.summon.area()
    }

    fn summon_time_left_ms(&self) -> u32 {
        self.summon.time_left_ms()
    }

    fn item_spell_in_range(&self, spell_id: u32, target: u64) -> Option<Option<bool>> {
        Some(self.item_range.in_range(spell_id, target))
    }

    fn unique_signal(&mut self, event: &'static str) -> bool {
        self.lent
            .signals
            .get_mut()
            .map(|s| s.unique_signal(event))
            .is_some()
    }

    fn world_state_ui_count(&self) -> Option<usize> {
        self.world_state_ui.count()
    }

    fn world_state_ui_info(
        &self,
        index: usize,
    ) -> Option<Option<benilla_ui::script::WorldStateUiView>> {
        self.world_state_ui.info(index)
    }

    fn world_state(&self, key: u32) -> Option<i32> {
        self.world_state_ui.value(key)
    }

    fn quest_tooltip(
        &mut self,
        quest_id: u32,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Option<benilla_ui::script::QuestTooltipRead> {
        self.quest_tooltip.read(quest_id, get)
    }

    fn quest_poi_lines(
        &self,
        quest_id: u32,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Option<Option<benilla_ui::script::QuestPoiLines>> {
        Some(self.quest.quest_poi_lines(quest_id, get))
    }

    fn quest_special_item(
        &self,
        quest_id: u32,
    ) -> Option<benilla_ui::script::QuestSpecialItemView> {
        self.quest.quest_special_item(quest_id)
    }

    fn quest_log_row(
        &mut self,
        quest_id: u32,
        slot: u8,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Option<Option<benilla_ui::script::QuestLogEntryView>> {
        let Self { view, lent } = self;
        let Lent { asks, expiries, .. } = lent;
        view.quest_rows
            .row(quest_id, slot, get, expiries.get(), asks.get_mut())
    }

    fn quest_log_detail(
        &mut self,
        quest_id: u32,
        slot: u8,
    ) -> Option<Option<benilla_ui::script::QuestLogDetail>> {
        let Self { view, lent } = self;
        view.quest_rows.detail(quest_id, slot, lent.asks.get_mut())
    }

    fn daily_quests_completed(&self) -> Option<u32> {
        self.quest_rows.daily_completed()
    }

    fn expire_quest(&mut self, quest_id: u32) -> Option<Option<String>> {
        let Self { view, lent } = self;
        view.quest.expire_quest(quest_id, lent.expiries.get_mut()?)
    }

    /// The last time response advanced on our monotonic clock, where the reference adds the local clock to its offset (`0x5dff40`). Deviation: a local clock step mid-session (an NTP correction) moves the reference's countdowns and not ours, as the step changes no time the server counts.
    fn server_unix_time(&self) -> Option<f64> {
        self.clock.as_ref()?.now_unix()
    }

    fn quest_reset_unix(&self) -> Option<f64> {
        self.clock.as_ref()?.quest_reset_unix()
    }

    fn send_quest_log_tail(&mut self, pois: &[u32], query_time: bool) {
        let Some(net) = self.view.net.as_deref() else {
            return;
        };
        if !pois.is_empty() {
            debug!("ui_quest_log: asking POI for {pois:?}");
            let _ = net.0.send(crate::net::ClientCommand::QuestPoiQuery {
                quests: pois.to_vec(),
            });
        }
        if query_time {
            let _ = net.0.send(crate::net::ClientCommand::QueryTime);
        }
    }

    fn rebuild_quest_log_if_reask_due(&mut self) -> Option<benilla_ui::script::QuestLogPlan> {
        let Self { view, lent } = self;
        view.quest_rows.rebuild_if_reask_due(lent.row_writes()?)
    }

    fn quest_item_record(
        &self,
        item_id: u32,
    ) -> Option<Option<benilla_ui::script::QuestItemRecord>> {
        self.quest.quest_item_record(item_id)
    }

    fn quest_action(
        &mut self,
        action: benilla_ui::script::QuestAction,
    ) -> Option<benilla_ui::script::QuestActOutcome> {
        let Self { view, lent } = self;
        let giver = lent.giver.get_mut()?;
        let npc_flags = giver
            .npc
            .and_then(|npc| view.index.as_ref()?.0.get(&npc).copied())
            .and_then(|e| view.stores.get(e).ok())
            .map(|s| s.0.unit_npc_flags());
        Some(crate::ui_quest::act(
            giver,
            action,
            view.net.as_deref()?,
            npc_flags,
        ))
    }

    fn quest_npc(&self) -> Option<u64> {
        self.lent.giver.get()?.quest_npc()
    }

    fn quest_select(
        &mut self,
        select: benilla_ui::script::QuestSelect,
    ) -> Option<benilla_ui::script::QuestActOutcome> {
        let Self { view, lent } = self;
        Some(crate::ui_quest::select(
            lent.giver.get_mut()?,
            select,
            view.net.as_deref()?,
        ))
    }

    fn carried_quest_item(&self, item_id: u32) -> Option<u32> {
        self.quest_rows.carried(item_id)
    }

    fn battlefield_slot(&self, index: usize) -> Option<benilla_ui::script::BattlefieldQueueSlot> {
        self.battlefield.slot(index)
    }

    fn battlefield_instance(&self) -> benilla_ui::script::BattlefieldInstance {
        self.battlefield.instance()
    }

    fn world_pvp_queue(&self) -> benilla_ui::script::WorldPvpQueueView {
        self.battlefield.world_pvp()
    }

    fn can_hearth_and_resurrect(&self) -> bool {
        self.battlefield.can_hearth_and_resurrect()
    }

    fn battlefield_port_refusal(&self) -> Option<&'static str> {
        self.battlefield.port_refusal()
    }

    fn battlemaster_row(&self, id: u32) -> Option<benilla_ui::script::BattlemasterRow> {
        self.battlefield.row(id)
    }

    fn battleground_types(&self) -> Vec<benilla_ui::script::BattlegroundType> {
        self.battlefield.types()
    }

    /// A held unit's own race and class stand in where the name answer carried none.
    fn score_name(&self, guid: u64) -> Option<benilla_ui::script::ScoreName> {
        self.score.name(guid, |g| {
            let f = self.unit(g)?;
            Some((f.unit_race()?, f.unit_class()?))
        })
    }

    fn battlefield_score_names_pending(&self) -> bool {
        self.score.names_pending()
    }

    fn arena_opponent_count(&self) -> usize {
        self.arena.as_ref().map_or(0, |a| a.count())
    }

    fn honor(&self) -> Option<benilla_ui::script::HonorState> {
        crate::ui_honor::honor_of(self.unit(self.active_player()?)?)
    }

    fn pvp_toggle_timer(&self) -> Option<i32> {
        let me = self.unit(self.active_player()?)?;
        Some(self.honor.toggle_timer(me))
    }

    fn arena_team_info(&self, slot: usize) -> [u32; 7] {
        let me = self.active_player().and_then(|g| self.unit(g));
        crate::ui_arena_team::ArenaTeamView::info(me, slot)
    }

    fn arena_team_stats(&self, slot: usize) -> [u32; 6] {
        let me = self.active_player().and_then(|g| self.unit(g));
        self.arena_teams.stats(me, slot)
    }

    fn arena_team_roster_type(&self, slot: usize) -> u32 {
        let me = self.active_player().and_then(|g| self.unit(g));
        self.arena_teams.roster_type(me, slot)
    }

    fn arena_team_member_count(&self, slot: usize) -> usize {
        let me = self.active_player().and_then(|g| self.unit(g));
        self.arena_teams.member_count(me, slot)
    }

    fn arena_team_member(
        &self,
        slot: usize,
        index: usize,
    ) -> Option<benilla_ui::script::ArenaTeamMemberRef<'_>> {
        let me = self.active_player().and_then(|g| self.unit(g));
        self.arena_teams.member(me, slot, index)
    }

    fn arena_team_record(&self, team_id: u32) -> Option<benilla_ui::script::ArenaTeamRecordView> {
        self.arena_teams.record(team_id)
    }

    fn battlefield_position_count(&self) -> usize {
        self.positions.count(self.active_player())
    }

    fn battlefield_position(
        &self,
        index: usize,
    ) -> Option<benilla_ui::script::BattlefieldPositionLive> {
        self.positions
            .position(self.active_player(), index, |g| self.player_entity(g))
    }

    fn battlefield_flag_count(&self) -> usize {
        self.positions.flag_count()
    }

    fn battlefield_flag(&self, index: usize) -> Option<benilla_ui::script::BattlefieldFlagLive> {
        self.positions.flag(
            self.active_player(),
            index,
            |g| self.player_entity(g),
            |g| self.player_side(g),
        )
    }

    fn battlefield_icon_scale(&self) -> f32 {
        self.positions.icon_scale()
    }

    fn request_battlefield_positions(&self) {
        self.positions.request();
    }

    fn notify_inspect(&self, guid: u64) -> bool {
        self.inspect.notify(guid, || self.player_class(guid))
    }

    fn clear_inspect_target(&self) {
        self.inspect.clear();
    }
}

impl Lent<'_> {
    /// The rebuild's writes, `None` while any is missing.
    fn row_writes(&mut self) -> Option<crate::ui_quest_log::rows::RowWrites<'_>> {
        let Self {
            signals,
            marks,
            expiries,
            ..
        } = self;
        Some(crate::ui_quest_log::rows::RowWrites {
            marks: marks.get_mut()?,
            expiries: expiries.get_mut()?,
            signals: signals.get_mut()?,
        })
    }
}

/// [`LiveNow`] lent to every call into Lua of the VM-holding system that takes it; `OWN` names by [`lent`] bits the state the system lends from its own borrow ([`Self::lending`]).
pub(crate) struct LendLive<'w, 's, const OWN: u8 = 0> {
    loan: LiveLoan<LiveNow<'w, 's>>,
}

impl<'w, 's, const OWN: u8> LendLive<'w, 's, OWN> {
    /// Run `f`, a call into Lua, with `state`, a borrow this system owns, lent to its verbs until `f` returns or unwinds.
    pub(crate) fn lending<T: Lendable, R>(&mut self, state: &mut T, f: impl FnOnce() -> R) -> R {
        assert!(
            OWN & T::BIT != 0,
            "a system lends only the state it leaves out of `LendLive`"
        );
        self.loan
            .with_view_mut(|v| T::slot(&mut v.lent).overlay = Some(NonNull::from(state)));
        let _clear = ClearOverlay::<'_, 'w, 's, T>(&mut self.loan, std::marker::PhantomData);
        f()
    }

    /// `f` over the quest giver this parameter borrows, between calls into Lua; a verb reached from inside `f` panics.
    pub(crate) fn with_giver<R>(&self, f: impl FnOnce(Option<&QuestGiver>) -> R) -> R {
        self.loan.with_view(|v| f(v.lent.giver.get()))
    }

    /// Whether the quest giver changed since this parameter's system last ran.
    pub(crate) fn giver_changed(&self) -> bool {
        self.loan.with_view(|v| v.lent.giver.changed())
    }

    /// `f` over the quest giver this parameter borrows, to write between calls into Lua; a verb reached from inside `f` panics.
    pub(crate) fn with_giver_mut<R>(&mut self, f: impl FnOnce(Option<&mut QuestGiver>) -> R) -> R {
        self.loan.with_view_mut(|v| f(v.lent.giver.get_mut()))
    }

    /// The overlay of `T`, set only inside [`Self::lending`].
    #[cfg(test)]
    pub(crate) fn overlay<T: Lendable>(&mut self) -> bool {
        self.loan
            .with_view_mut(|v| T::slot(&mut v.lent).overlay.is_some())
    }
}

/// Ends a [`LendLive::lending`] loan, on return and on unwind.
struct ClearOverlay<'a, 'w, 's, T: Lendable>(
    &'a mut LiveLoan<LiveNow<'w, 's>>,
    std::marker::PhantomData<T>,
);

impl<T: Lendable> Drop for ClearOverlay<'_, '_, '_, T> {
    fn drop(&mut self) {
        self.0
            .with_view_mut(|v| T::slot(&mut v.lent).overlay = None);
    }
}

/// The state [`LendLive`] builds its view from.
type ViewState = <LiveView<'static, 'static> as SystemParam>::State;

/// The [`Lent`] state's ids, in [`lent`] bit order.
type LentIds = [bevy::ecs::component::ComponentId; 5];

/// Declare `T`'s borrow unless the system owns it.
fn lent_access<T: 'static>(
    own: u8,
    bit: u8,
    id: &bevy::ecs::component::ComponentId,
    system_meta: &mut SystemMeta,
    component_access_set: &mut FilteredAccessSet,
    world: &mut World,
) {
    if own & bit == 0 {
        <Option<NonSendMut<'static, T>> as SystemParam>::init_access(
            id,
            system_meta,
            component_access_set,
            world,
        );
    }
}

/// `T`'s borrow, unless the system owns it.
///
/// # Safety
/// As [`SystemParam::get_param`], with `T`'s access declared by [`lent_access`].
unsafe fn lent_fetch<'w, T: 'static>(
    own: u8,
    bit: u8,
    mut id: bevy::ecs::component::ComponentId,
    system_meta: &SystemMeta,
    world: UnsafeWorldCell<'w>,
    change_tick: Tick,
) -> LentSlot<'w, T> {
    if own & bit != 0 {
        return LentSlot::default();
    }
    LentSlot {
        // SAFETY: the caller's guarantees; the access was declared in `lent_access`.
        fetched: unsafe {
            <Option<NonSendMut<'static, T>> as SystemParam>::get_param(
                &mut id,
                system_meta,
                world,
                change_tick,
            )
        },
        overlay: None,
    }
}

// SAFETY: every access is `LiveView`'s, registered and fetched through it, or a `Lent` borrow
// registered in `init_access` and fetched in `get_param` as `Option<NonSendMut<T>>` does.
unsafe impl<const OWN: u8> SystemParam for LendLive<'_, '_, OWN> {
    type State = (ViewState, LentIds);
    type Item<'w, 's> = LendLive<'w, 's, OWN>;

    fn init_state(world: &mut World) -> Self::State {
        let view = <LiveView<'static, 'static> as SystemParam>::init_state(world);
        let mut reg = world.components_registrator();
        let ids = [
            reg.register_non_send::<UniqueSignals>(),
            reg.register_non_send::<QuestAsks>(),
            reg.register_non_send::<RebuildMarks>(),
            reg.register_non_send::<QuestExpiries>(),
            reg.register_non_send::<QuestGiver>(),
        ];
        (view, ids)
    }

    fn init_access(
        state: &Self::State,
        system_meta: &mut SystemMeta,
        component_access_set: &mut FilteredAccessSet,
        world: &mut World,
    ) {
        <LiveView<'static, 'static> as SystemParam>::init_access(
            &state.0,
            system_meta,
            component_access_set,
            world,
        );
        let [signals, asks, marks, expiries, giver] = &state.1;
        let (meta, set) = (system_meta, component_access_set);
        lent_access::<UniqueSignals>(OWN, lent::SIGNALS, signals, meta, set, world);
        lent_access::<QuestAsks>(OWN, lent::ASKS, asks, meta, set, world);
        lent_access::<RebuildMarks>(OWN, lent::MARKS, marks, meta, set, world);
        lent_access::<QuestExpiries>(OWN, lent::EXPIRIES, expiries, meta, set, world);
        lent_access::<QuestGiver>(OWN, lent::GIVER, giver, meta, set, world);
    }

    fn apply(state: &mut Self::State, system_meta: &SystemMeta, world: &mut World) {
        <LiveView<'static, 'static> as SystemParam>::apply(&mut state.0, system_meta, world);
    }

    fn queue(state: &mut Self::State, system_meta: &SystemMeta, world: DeferredWorld) {
        <LiveView<'static, 'static> as SystemParam>::queue(&mut state.0, system_meta, world);
    }

    unsafe fn validate_param(
        state: &mut Self::State,
        system_meta: &SystemMeta,
        world: UnsafeWorldCell,
    ) -> Result<(), SystemParamValidationError> {
        // SAFETY: the caller's guarantees are `LiveView`'s; the `Lent` borrows are optional.
        unsafe {
            <LiveView<'static, 'static> as SystemParam>::validate_param(
                &mut state.0,
                system_meta,
                world,
            )
        }
    }

    unsafe fn get_param<'w, 's>(
        state: &'s mut Self::State,
        system_meta: &SystemMeta,
        world: UnsafeWorldCell<'w>,
        change_tick: Tick,
    ) -> Self::Item<'w, 's> {
        let (view_state, ids) = state;
        // SAFETY: the caller's guarantees are `LiveView`'s.
        let view = unsafe {
            <LiveView<'static, 'static> as SystemParam>::get_param(
                view_state,
                system_meta,
                world,
                change_tick,
            )
        };
        let [signals, asks, marks, expiries, giver] = *ids;
        let (meta, tick) = (system_meta, change_tick);
        // SAFETY: each access was declared in `init_access` for exactly the bits not owned.
        let lent = unsafe {
            Lent {
                signals: lent_fetch(OWN, lent::SIGNALS, signals, meta, world, tick),
                asks: lent_fetch(OWN, lent::ASKS, asks, meta, world, tick),
                marks: lent_fetch(OWN, lent::MARKS, marks, meta, world, tick),
                expiries: lent_fetch(OWN, lent::EXPIRIES, expiries, meta, world, tick),
                giver: lent_fetch(OWN, lent::GIVER, giver, meta, world, tick),
            }
        };
        // SAFETY: the system drops its parameters on the thread that ran it when its run ends,
        // after any loan made inside it, and never leaks one.
        LendLive {
            loan: unsafe { LiveLoan::new(LiveNow { view, lent }) },
        }
    }
}

/// The `"questnpc"` unit's guid through the view lent to the running system (`0x6081b0`), `None` outside a lending system.
pub(crate) fn quest_npc() -> Option<u64> {
    benilla_ui::script::read_live(|l| l.quest_npc()).flatten()
}

/// A handler's write to state the VM owns, run in turn with the events around it.
type VmWrite = Box<dyn FnOnce(&mut UiScript) + Send + Sync>;

/// A step that needs the whole `World`, run in turn with the events around it.
type WorldStep = Box<dyn FnOnce(&mut World) + Send + Sync>;

/// One queued call into the VM.
enum UiEvent {
    Fire(&'static str, Vec<ScriptValue>),
    /// The cursor clear a handler runs beside its event (`0x520fab`).
    ClearCursor,
    /// A write to what the VM owns, such as the battlefield list a list packet fills (`0x54e390`).
    Write(VmWrite),
    /// A system chat line, its template read off the VM and filled (`0x5498f6`, `0x5cfb20`).
    SystemLine(&'static str, Vec<LineArg>),
    /// A step over the `World`, such as the unit pushes an event's tokens read.
    World(WorldStep),
}

/// An argument a system line's template takes.
pub(crate) enum LineArg {
    S(String),
    D(i64),
}

/// The events this frame's handlers fired, in order; the drain empties it after each handler.
#[derive(Resource, Default)]
pub(crate) struct UiEvents(Vec<UiEvent>);

impl UiEvents {
    /// Fire `event` once the calling handler's changes have applied, before the next handler runs.
    pub(crate) fn fire(&mut self, event: &'static str, args: Vec<ScriptValue>) {
        self.0.push(UiEvent::Fire(event, args));
    }

    /// Clear the cursor in turn with the events around it.
    pub(crate) fn clear_cursor(&mut self) {
        self.0.push(UiEvent::ClearCursor);
    }

    /// Add the system chat line `key`'s template names, filled with `args` as the handler runs.
    pub(crate) fn system_line(&mut self, key: &'static str, args: Vec<LineArg>) {
        self.0.push(UiEvent::SystemLine(key, args));
    }

    /// Write state the VM owns in turn with the events around it; `write` may fire its own.
    pub(crate) fn write(&mut self, write: impl FnOnce(&mut UiScript) + Send + Sync + 'static) {
        self.0.push(UiEvent::Write(Box::new(write)));
    }

    /// Run `step` over the `World` in turn with the events around it; what it queues fires next.
    pub(crate) fn world(&mut self, step: impl FnOnce(&mut World) + Send + Sync + 'static) {
        self.0.push(UiEvent::World(Box::new(step)));
    }

    /// The fired event names, in order.
    #[cfg(test)]
    pub(crate) fn names(&self) -> Vec<&'static str> {
        self.0
            .iter()
            .map(|e| match e {
                UiEvent::Fire(name, _) => *name,
                UiEvent::ClearCursor => "<clear cursor>",
                UiEvent::Write(_) => "<write>",
                UiEvent::SystemLine(key, _) => key,
                UiEvent::World(_) => "<world>",
            })
            .collect()
    }

    /// Whether nothing waits to fire.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The cached parameter state [`with_live_world`] lends from.
#[derive(Resource)]
pub(super) struct LiveViewState(SystemState<(LiveView<'static, 'static>, LentAll<'static>)>);

/// The VM taken out of the world for a call, put back on return and on unwind.
struct HeldScript<'w> {
    world: &'w mut World,
    script: Option<UiScript>,
}

impl Drop for HeldScript<'_> {
    fn drop(&mut self) {
        if let Some(script) = self.script.take() {
            self.world.insert_non_send_resource(script);
        }
    }
}

/// The view's state taken out of the world for a call, put back on return and on unwind.
struct HeldState<'w> {
    world: &'w mut World,
    state: Option<LiveViewState>,
}

impl Drop for HeldState<'_> {
    fn drop(&mut self) {
        if let Some(state) = self.state.take() {
            self.world.insert_resource(state);
        }
    }
}

/// Run `f` on the VM with the `World` lent as its live view; `None` with no VM.
pub(crate) fn with_live_world<R>(
    world: &mut World,
    f: impl FnOnce(&mut UiScript) -> R,
) -> Option<R> {
    let script = world.remove_non_send_resource::<UiScript>()?;
    let mut held = HeldScript {
        world,
        script: Some(script),
    };
    let script = held.script.as_mut().expect("held until the drop");
    Some(lend_world(held.world, script, f))
}

/// Run `f` on `script`, a VM held outside the world, with the `World` lent as its live view.
pub(crate) fn lend_world<R>(
    world: &mut World,
    script: &mut UiScript,
    f: impl FnOnce(&mut UiScript) -> R,
) -> R {
    let state = match world.remove_resource::<LiveViewState>() {
        Some(state) => state,
        None => LiveViewState(SystemState::new(world)),
    };
    let mut held = HeldState {
        world,
        state: Some(state),
    };
    let state = held.state.as_mut().expect("held until the drop");
    let (view, lent) = state.0.get_mut(held.world);
    let mut now = LiveNow {
        view,
        lent: lent.into(),
    };
    script.with_live_mut(&mut now, f)
}

/// Fires what the handler that just ran queued, in its order, with the `World` lent; it runs after
/// each handler, not after each packet. With no VM they are dropped, as the reference has no
/// listener before its UI loads.
pub(crate) fn fire_handler_events(world: &mut World) {
    let Some(mut queued) = world.get_resource_mut::<UiEvents>() else {
        return;
    };
    if queued.0.is_empty() {
        return;
    }
    let mut events = std::collections::VecDeque::from(std::mem::take(&mut queued.0));
    let mut lines = Vec::new();
    let mut batch = Vec::new();
    while let Some(event) = events.pop_front() {
        match event {
            UiEvent::World(step) => {
                fire_batch(world, &mut batch, &mut lines);
                step(world);
                // What the step queued fires in its turn, ahead of the rest.
                if let Some(mut queued) = world.get_resource_mut::<UiEvents>() {
                    for e in std::mem::take(&mut queued.0).into_iter().rev() {
                        events.push_front(e);
                    }
                }
            }
            other => batch.push(other),
        }
    }
    fire_batch(world, &mut batch, &mut lines);
    // The chat log carries the line to the chat frame, as the chat feed's other system lines go.
    if let Some(mut chat) = world.get_resource_mut::<crate::ui_chat::ChatLog>() {
        for line in lines {
            crate::ui_chat::away::push_system(&mut chat, line);
        }
    }
}

/// Fire `batch` into the VM, in order, with the `World` lent, collecting the system lines.
fn fire_batch(world: &mut World, batch: &mut Vec<UiEvent>, lines: &mut Vec<String>) {
    if batch.is_empty() {
        return;
    }
    let events = std::mem::take(batch);
    with_live_world(world, |script| {
        for event in events {
            match event {
                UiEvent::Fire(event, args) => script.fire_event(event, args),
                UiEvent::ClearCursor => script.clear_cursor_payload(),
                UiEvent::Write(write) => write(script),
                UiEvent::SystemLine(key, args) => {
                    use benilla_ui::strings::Arg;
                    let args: Vec<Arg> = args
                        .iter()
                        .map(|a| match a {
                            LineArg::S(s) => Arg::S(s),
                            LineArg::D(d) => Arg::D(*d),
                        })
                        .collect();
                    let text = crate::ui_chat::combat::global_string(script, key)
                        .map(|t| benilla_ui::strings::fill(&t, &args));
                    lines.extend(text.filter(|t| !t.is_empty()));
                }
                // Split out by the caller.
                UiEvent::World(_) => {}
            }
        }
    });
}

/// The unique signals queued this frame: each event once, however often it is queued (`0x6143f0`), fired in queue order once per frame after the packets (`0x52b247` -> `0x614760`). Main-thread state the handlers fill and a verb's call queues into ([`Lent`]).
#[derive(Default)]
pub(crate) struct UniqueSignals(Vec<&'static str>);

impl UniqueSignals {
    /// Queue `event` for the frame's flush unless it already waits (`0x615020`).
    pub(crate) fn unique_signal(&mut self, event: &'static str) {
        if !self.0.contains(&event) {
            self.0.push(event);
        }
    }

    /// The game UI's shutdown empties the queue (`0x52902f`), so nothing waits for the next login.
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    /// The queued event names, in order.
    #[cfg(test)]
    pub(crate) fn names(&self) -> Vec<&'static str> {
        self.0.clone()
    }
}

/// The game UI's idle tail (`0x52af40`): the watch timers run (`0x52b229`), then the frame's unique signals fire in queue order without arguments (`0x52b247` -> `0x614760`), after the feeds that queue them; with no VM they are dropped.
pub(crate) fn flush_unique_signals(
    (script, mut live): (
        Option<NonSendMut<UiScript>>,
        LendLive<'_, '_, { lent::SIGNALS }>,
    ),
    mut signals: NonSendMut<UniqueSignals>,
) {
    let Some(mut script) = script else {
        signals.clear();
        return;
    };
    live.lending(&mut *signals, || script.update_quest_watch_timers());
    // The count taken at entry, its entries still queued while they fire, so a re-queue matches one in flight (`0x6143f0`); the count is zeroed after the loop, dropping what the handlers queued (`0x6147a3`).
    let events = signals.0.clone();
    live.lending(&mut *signals, || {
        for event in events {
            script.fire_event(event, Vec::new());
        }
    });
    signals.clear();
}

/// The disconnect runs the game UI's shutdown, which empties the unique signals (`0x52902f`).
pub(crate) fn clear_unique_signals(
    In(_): In<benilla_protocol::SessionEvent>,
    mut signals: NonSendMut<UniqueSignals>,
) {
    signals.clear();
}
