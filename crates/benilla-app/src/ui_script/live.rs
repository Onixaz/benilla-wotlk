//! The VM's live view of the client and the events a packet handler fires. 12340's verbs read the
//! live object when called (`0x60f396`) and a handler fires its events once its changes apply
//! (`0x520fe0`). [`LiveView`] is the [`LiveState`] over the `World` every call into Lua is lent: a
//! system holding the VM takes [`LendLive`], which lends it for the system's run, and an exclusive
//! one lends through [`with_live_world`] or [`lend_world`]. A resource such a system writes itself
//! is lent by its own borrow ([`OwnedMut`]). [`UiEvents`] is
//! the queue a handler fires into, emptied into the VM once that handler's commands apply.

use std::marker::PhantomData;
use std::ptr::NonNull;

use benilla_ui::script::{LiveLoan, LiveState, ScriptValue, UiScript};
use bevy::ecs::change_detection::{MutUntyped, Tick};
use bevy::ecs::component::ComponentId;
use bevy::ecs::query::FilteredAccessSet;
use bevy::ecs::system::{
    ReadOnlySystemParam, SystemMeta, SystemParam, SystemParamValidationError, SystemState,
};
use bevy::ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;

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
    battlefield: crate::ui_battlefield::BattlefieldView<'w, 's>,
    score: crate::ui_battlefield_score::ScoreView<'w>,
    arena: Option<Res<'w, crate::ui_arena::ArenaOpponents>>,
    honor: crate::ui_honor::HonorView<'w>,
    arena_teams: crate::ui_arena_team::ArenaTeamView<'w>,
    positions: crate::ui_battlefield_positions::PositionsView<'w, 's>,
    inspect: crate::ui_inspect::InspectLive<'w>,
    companions: crate::ui_companion::CompanionLive<'w, 's>,
    pets: crate::ui_pet_stats::PetView<'w>,
    possess: crate::ui_pet::possess::PossessLive<'w>,
    pub(crate) stable: crate::ui_stable::StableView<'w, 's>,
    pub(crate) cooldowns: Slot<'w, crate::spell::Cooldowns>,
    pub(crate) pet_bar: Slot<'w, crate::ui_pet::PetBar>,
    pub(crate) selection: Slot<'w, crate::target::Selection>,
    pub(crate) place: Slot<'w, crate::area::PlayerPlace>,
    /// The wire, for the sends a verb makes inside its call.
    commands: Option<Res<'w, crate::net::NetCommands>>,
    /// The usability walk's other inputs, for `GetPetActionSlotUsable` (`0x5d5400`).
    walk: Option<crate::ui_pet::PetSlotWalk<'w, 's>>,
    /// The bar's pet, our seat and the pet's auras, for the bar's usability and active reads.
    pet_unit: Option<crate::ui_pet::PetUnit<'w, 's>>,
    /// The spell records, the cooldown query's category and global-cooldown legs.
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    /// `ChrClasses.dbc`, the pet book's token.
    classes: Option<Res<'w, crate::chr_classes::ChrClassTable>>,
    _mark: Option<Res<'w, LendsLive>>,
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

/// A resource the view reads that a system holding the VM may write itself. [`LendLive`] points it
/// at the resource's storage: a system that does not write it registers a read, and one that does
/// ([`Owns`]) writes it only through [`OwnedMut`], whose write no verb reaches, so a verb reads the
/// owner's latest write when called.
pub(crate) struct Slot<'w, X: Resource> {
    ptr: Option<NonNull<X>>,
    /// The lending system writes `X` itself, so a verb inside its call may write it too.
    owned: bool,
    _life: PhantomData<&'w X>,
}

impl<X: Resource> Slot<'_, X> {
    /// The resource as the verbs read it now.
    pub(crate) fn get(&self) -> Option<&X> {
        // SAFETY: the storage pointer, read under the lending system's access; an owner's write
        // panics beside the shared view or verb this read is made through, and none outlives it.
        self.ptr.map(|p| unsafe { p.as_ref() })
    }

    /// The resource for a verb's write inside the call (`write_live`), `None` unless the lending
    /// system owns it: a system that only reads it leaves the write to its owner.
    pub(crate) fn get_mut(&mut self) -> Option<&mut X> {
        // SAFETY: an owner registered the write; a verb's write excludes every other borrow of the
        // view for its length, and none outlives the call.
        self.ptr
            .filter(|_| self.owned)
            .map(|mut p| unsafe { p.as_mut() })
    }

    fn set(&mut self, ptr: Option<NonNull<X>>, owned: bool) {
        self.ptr = ptr;
        self.owned = owned;
    }
}

// SAFETY: a slot registers no access and fetches nothing; `LendLive` fills it under its own access.
unsafe impl<X: Resource> SystemParam for Slot<'_, X> {
    type State = ();
    type Item<'w, 's> = Slot<'w, X>;

    fn init_state(_: &mut World) -> Self::State {}

    fn init_access(_: &(), _: &mut SystemMeta, _: &mut FilteredAccessSet, _: &mut World) {}

    unsafe fn get_param<'w, 's>(
        _: &'s mut (),
        _: &SystemMeta,
        _: UnsafeWorldCell<'w>,
        _: Tick,
    ) -> Self::Item<'w, 's> {
        Slot {
            ptr: None,
            owned: false,
            _life: PhantomData,
        }
    }
}

// SAFETY: it accesses nothing.
unsafe impl<X: Resource> ReadOnlySystemParam for Slot<'_, X> {}

/// Which lendable resources the system taking [`LendLive`] writes itself, through [`OwnedMut`].
/// The pet's words are every loan's: any call into Lua may toggle or drag them (`0x5d5d88`).
pub(crate) trait Owns: 'static {
    const COOLDOWNS: bool = false;
    const SELECTION: bool = false;
    const PLACE: bool = false;
}

impl Owns for () {}

/// Both sets' resources.
impl<A: Owns, B: Owns> Owns for (A, B) {
    const COOLDOWNS: bool = A::COOLDOWNS || B::COOLDOWNS;
    const SELECTION: bool = A::SELECTION || B::SELECTION;
    const PLACE: bool = A::PLACE || B::PLACE;
}

/// The system writes [`crate::area::PlayerPlace`].
pub(crate) struct OwnsPlace;
impl Owns for OwnsPlace {
    const PLACE: bool = true;
}

/// The system writes [`crate::target::Selection`].
pub(crate) struct OwnsSelection;
impl Owns for OwnsSelection {
    const SELECTION: bool = true;
}

/// The system writes [`crate::spell::Cooldowns`].
pub(crate) struct OwnsCooldowns;
impl Owns for OwnsCooldowns {
    const COOLDOWNS: bool = true;
}

/// An exclusive lender holds the whole `World`, so every verb may write what a verb writes.
pub(crate) struct OwnsAll;
impl Owns for OwnsAll {
    const COOLDOWNS: bool = true;
}

/// A VM holder's write handle on a resource its lent view reads: the owner's borrow, held as the
/// storage pointer, so a write borrows it only for its own length and a verb called after it reads
/// it. While a write lives no verb reaches the view, and no shared view of it may be held.
pub(crate) struct OwnedMut<'w, X: Resource> {
    ticks: MutUntyped<'w>,
    ptr: NonNull<X>,
}

impl<X: Resource> OwnedMut<'_, X> {
    /// The resource, marked changed, for as long as the returned borrow lives.
    pub(crate) fn write(&mut self) -> OwnedWrite<'_, X> {
        self.ticks.set_changed();
        let held = benilla_ui::script::owner_write();
        // SAFETY: the system's write access is registered; the borrow is tied to `&mut self`, and
        // `held` keeps every verb and shared view of the loan off the storage while it lives.
        OwnedWrite {
            x: unsafe { self.ptr.as_mut() },
            _held: held,
        }
    }
}

impl<X: Resource> OwnedMut<'_, X> {
    /// The resource, counted as a shared read while the returned borrow lives, so a verb's write
    /// or the owner's own write panics beside it.
    pub(crate) fn read(&self) -> LentRead<'_, X> {
        let held = benilla_ui::script::shared_read();
        // SAFETY: the system's access is registered; `held` keeps every write off the storage
        // while the borrow lives.
        LentRead {
            x: unsafe { self.ptr.as_ref() },
            _held: held,
        }
    }
}

/// One write through [`OwnedMut`], live until it drops.
pub(crate) struct OwnedWrite<'a, X> {
    x: &'a mut X,
    _held: benilla_ui::script::OwnerWrite,
}

impl<X> std::ops::Deref for OwnedWrite<'_, X> {
    type Target = X;

    fn deref(&self) -> &X {
        self.x
    }
}

impl<X> std::ops::DerefMut for OwnedWrite<'_, X> {
    fn deref_mut(&mut self) -> &mut X {
        self.x
    }
}

/// A shared read of a lendable resource beside its owner's pair, as a press applier in the owner's
/// set reads it: each read is counted while it lives, so the owner's write cannot overlap it. Absent
/// resource, no read.
pub(crate) struct LentRef<'w, X: Resource> {
    ptr: Option<NonNull<X>>,
    /// Written since the system last ran, as `Res::is_changed` answers.
    changed: bool,
    _life: PhantomData<&'w X>,
}

impl<X: Resource> LentRef<'_, X> {
    /// Whether the resource was written since the system last ran.
    pub(crate) fn is_changed(&self) -> bool {
        self.changed
    }

    /// The resource, for as long as the returned borrow lives.
    pub(crate) fn get(&self) -> Option<LentRead<'_, X>> {
        let ptr = self.ptr?;
        let held = benilla_ui::script::shared_read();
        // SAFETY: the storage pointer, read under the system's access; `held` panics beside an
        // owner's write and keeps one from starting while it lives.
        Some(LentRead {
            x: unsafe { ptr.as_ref() },
            _held: held,
        })
    }
}

/// One read through [`LentRef`], live until it drops.
pub(crate) struct LentRead<'a, X> {
    x: &'a X,
    _held: benilla_ui::script::SharedRead,
}

impl<X> std::ops::Deref for LentRead<'_, X> {
    type Target = X;

    fn deref(&self) -> &X {
        self.x
    }
}

// SAFETY: a read is registered unless the system already writes it as an owned pair, whose writes
// the read's count excludes; fetched as a pointer, never a reference.
unsafe impl<X: Resource> SystemParam for LentRef<'_, X> {
    type State = SlotIds;
    type Item<'w, 's> = LentRef<'w, X>;

    fn init_state(world: &mut World) -> Self::State {
        slot_ids::<X>(world)
    }

    fn init_access(
        &(id, loan, owner): &Self::State,
        _: &mut SystemMeta,
        component_access_set: &mut FilteredAccessSet,
        _: &mut World,
    ) {
        let access = component_access_set.combined_access();
        if !access.has_resource_write(id) {
            component_access_set.add_unfiltered_resource_read(id);
            return;
        }
        assert!(
            access.has_resource_read(loan) || access.has_resource_read(owner),
            "error[B0002]: {} is read beside a plain write of it: write it through `OwnedMut`",
            std::any::type_name::<X>()
        );
    }

    unsafe fn get_param<'w, 's>(
        &mut (id, ..): &'s mut Self::State,
        system_meta: &SystemMeta,
        world: UnsafeWorldCell<'w>,
        change_tick: Tick,
    ) -> Self::Item<'w, 's> {
        // SAFETY: registered in `init_access`, or written by the system's own pair, which has
        // not run yet; the reference ends with this statement.
        let changed = unsafe { world.get_resource_ref::<X>() }.is_some_and(|mut r| {
            r.set_ticks(system_meta.get_last_run(), change_tick);
            r.is_changed()
        });
        LentRef {
            // SAFETY: registered in `init_access`, or written by the system's own pair.
            ptr: unsafe { slot_fetch(world, id) },
            changed,
            _life: PhantomData,
        }
    }
}

/// Never inserted: read by a [`LendLive`] that owns `X`, so its [`OwnedMut`] knows its partner.
struct LoanMark<X>(PhantomData<fn() -> X>);
impl<X: 'static> Resource for LoanMark<X> {}

/// Never inserted: read by an [`OwnedMut`] of `X`, so its [`LendLive`] knows its partner.
struct OwnerMark<X>(PhantomData<fn() -> X>);
impl<X: 'static> Resource for OwnerMark<X> {}

/// Register an owner's write of lendable `id`, one half of the pair a [`LendLive`] owning it and
/// its [`OwnedMut`] make: an earlier access to it must be the partner's (`partner` marked, `mine`
/// not), else it is refused (B0002) as `ResMut` refuses it.
fn owned_access(
    name: &str,
    id: ComponentId,
    mine: ComponentId,
    partner: ComponentId,
    set: &mut FilteredAccessSet,
) {
    let access = set.combined_access();
    assert!(
        !access.has_resource_read(mine),
        "error[B0002]: {name} is lent or written twice in one system"
    );
    assert!(
        !access.has_resource_read(id) || access.has_resource_read(partner),
        "error[B0002]: {name} is borrowed beside its owner's lent view: read it through the view \
         or a `LentRef` taken after the loan, and write it through `OwnedMut`"
    );
    set.add_unfiltered_resource_write(id);
    set.add_unfiltered_resource_read(mine);
}

// SAFETY: the write is registered in `init_access` and fetched as a pointer, never a reference.
unsafe impl<X: Resource> SystemParam for OwnedMut<'_, X> {
    type State = SlotIds;
    type Item<'w, 's> = OwnedMut<'w, X>;

    fn init_state(world: &mut World) -> Self::State {
        let (id, loan, owner) = slot_ids::<X>(world);
        (id, owner, loan)
    }

    fn init_access(
        &(id, mine, partner): &Self::State,
        system_meta: &mut SystemMeta,
        component_access_set: &mut FilteredAccessSet,
        _: &mut World,
    ) {
        // Beside its loan's access alone: the view reads through the same pointer, between writes.
        owned_access(
            &format!("{} in {}", std::any::type_name::<X>(), system_meta.name()),
            id,
            mine,
            partner,
            component_access_set,
        );
    }

    unsafe fn validate_param(
        &mut (id, ..): &mut Self::State,
        _: &SystemMeta,
        world: UnsafeWorldCell,
    ) -> Result<(), SystemParamValidationError> {
        // SAFETY: read-only access to the resource's metadata.
        if unsafe { world.storages() }
            .resources
            .get(id)
            .is_some_and(bevy::ecs::storage::ResourceData::is_present)
        {
            Ok(())
        } else {
            Err(SystemParamValidationError::invalid::<Self>(
                "Resource does not exist",
            ))
        }
    }

    unsafe fn get_param<'w, 's>(
        &mut (id, ..): &'s mut Self::State,
        _: &SystemMeta,
        world: UnsafeWorldCell<'w>,
        _: Tick,
    ) -> Self::Item<'w, 's> {
        // SAFETY: the write is registered; `MutUntyped` holds a pointer, not a reference.
        let mut ticks = unsafe { world.get_resource_mut_by_id(id) }.expect("validated");
        let ptr = NonNull::new(ticks.bypass_change_detection().as_ptr().cast::<X>())
            .expect("a resource's storage is never null");
        OwnedMut { ticks, ptr }
    }
}

/// Never inserted: read by every [`LiveView`], so a system's declared access shows it lends.
#[derive(Resource)]
pub(crate) struct LendsLive;

impl LiveState for LiveView<'_, '_> {
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

    fn companion_spells(&self, ty: benilla_ui::script::CompanionType) -> Vec<u32> {
        self.companions.spells(ty)
    }

    fn companion(
        &self,
        ty: benilla_ui::script::CompanionType,
        index: usize,
    ) -> Option<benilla_ui::script::CompanionView> {
        self.companions.info(ty, index)
    }

    fn companion_random_pool(&self) -> Vec<u32> {
        self.companions.random_pool()
    }

    /// The pet list's head held as a unit (`0x5d3a4d`, type mask 8).
    fn pet_stats(&self) -> (bool, benilla_ui::script::PetStats) {
        let pet = self.pets.head().and_then(|g| self.unit(g));
        self.pets.stats(pet, |g| self.unit(g))
    }

    fn pet_menu(&self) -> (bool, bool, bool) {
        let pet = self.pets.head().and_then(|g| self.unit(g));
        self.pets.menu(pet, self.active_player())
    }

    fn creature_family(&self, guid: u64) -> Option<String> {
        self.pets.creature_family(self.unit(guid)?)
    }

    fn possess(&self) -> Option<benilla_ui::script::PossessView> {
        self.possess.view()
    }

    fn stable(&self) -> benilla_ui::script::StableList {
        self.stable.list()
    }

    fn stable_family(
        &mut self,
        entry: u32,
        callback: Option<benilla_ui::script::StableCallback>,
    ) -> Option<benilla_ui::script::StableFamily> {
        let me = self.active_player().unwrap_or(0);
        self.stable.family(entry, callback, me)
    }

    /// The pet list's head held as a unit with a pet number and a hunter creator held as a player
    /// (`0x71b630`), and its creature record cached (`0x5a1602`).
    fn stable_live_pet(&self) -> bool {
        let Some(pet) = self.pets.head().and_then(|g| self.unit(g)) else {
            return false;
        };
        let hunter = pet
            .unit_created_by()
            .and_then(|g| self.player_class(g))
            .is_some_and(|(_, class)| class == 3);
        pet.unit_pet_number() != 0
            && hunter
            && pet
                .object_entry()
                .is_some_and(|entry| self.stable.has_record(entry))
    }

    fn stable_slot_price(&self, slot: u32) -> Option<u32> {
        self.stable.slot_price(slot)
    }

    fn stable_send(&self, verb: benilla_ui::script::StableIntent) {
        self.stable.send(verb);
    }

    fn close_stables(&mut self) -> bool {
        self.stable.close()
    }

    /// The pet list's head (`0x5d3a24`).
    fn pet_out(&self) -> bool {
        self.pets.head().is_some()
    }

    /// Our `UNIT_FIELD_CHARM` or `UNIT_FIELD_SUMMON` (`0x5a1ac0`).
    fn charm_or_summon(&self) -> bool {
        let Some(me) = self.active_player().and_then(|g| self.unit(g)) else {
            return true;
        };
        me.unit_charm().is_some() || me.unit_summon().is_some()
    }

    fn open_stable_aura(&self) -> bool {
        self.stable.open_stable_aura()
    }

    /// The pet setter's words and globals (`0x5d6900`) with the usability predicate (`0x5d3e80`).
    fn pet_bar(&self) -> Option<benilla_ui::script::PetBarLive> {
        let bar = self.pet_bar.get()?;
        Some(crate::ui_pet::bar_live(bar, self.pet_unit.as_ref()))
    }

    /// The slot word at the call (`0x5d4ec0`), its cooldown asked of the pet's history
    /// (`0x5d5345`) and its usability walked at the call (`0x5d5400`).
    fn pet_action(
        &self,
        index: usize,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> Option<benilla_ui::script::PetActionView> {
        let lent = crate::ui_pet::WalkLent {
            selection: self.selection.get(),
            place: self.place.get(),
        };
        crate::ui_pet::action_live(
            self.pet_bar.get()?,
            index,
            self.spells.as_deref(),
            self.pet_unit.as_ref(),
            self.walk.as_ref().map(|w| (w, lent)),
            (anchor, ui_now),
        )
    }

    /// The drag's write into the pet's words (`0x5d5c3e`), then its one packet (`0x5d5cda`).
    fn pet_place_action(&mut self, entries: &[(u32, u32)]) -> bool {
        let Some(bar) = self.pet_bar.get_mut() else {
            return false;
        };
        for &(position, packed) in entries {
            if let Some(e) = bar.spells.bar.get_mut(position as usize) {
                *e = benilla_protocol::messages::PetActionEntry::from(packed);
            }
        }
        if let Some(c) = &self.commands {
            let _ = c.0.send(crate::net::ClientCommand::PetSetAction {
                pet_guid: bar.spells.pet_guid,
                entries: entries.to_vec(),
            });
        }
        true
    }

    /// `PetStopAttack`'s clear on the lent bar (`0x5d4650`).
    fn pet_stop_attack(&mut self) {
        if let (Some(bar), Some(c)) = (self.pet_bar.get_mut(), &self.commands) {
            crate::ui_pet::stop_pet_attack(bar, c);
        }
    }

    /// The bar toggle (`0x5d5d20`) on the lent words: gate, flip, send, in 12340's order.
    fn pet_toggle_autocast(&mut self, index: u32) -> bool {
        let usable = self.pet_bar.get().is_some_and(|bar| {
            self.pet_unit
                .as_ref()
                .is_some_and(|p| p.actions_usable(bar))
        });
        let Some(bar) = self.pet_bar.get_mut().filter(|_| usable) else {
            return false;
        };
        let pet_guid = bar.spells.pet_guid;
        if let Some(flipped) = crate::ui_pet::toggle_slot_autocast(bar, index + 1) {
            if let Some(c) = &self.commands {
                let _ = c.0.send(crate::net::ClientCommand::PetSetAction {
                    pet_guid,
                    entries: vec![(index, flipped.packed)],
                });
            }
        }
        true
    }

    /// The book's set (`0x5d5e10`) on the lent words: gate, flip, send, in 12340's order.
    fn pet_set_spell_autocast(&mut self, spell_id: u32, state: Option<bool>) -> bool {
        let usable = self.pet_bar.get().is_some_and(|bar| {
            self.pet_unit
                .as_ref()
                .is_some_and(|p| p.actions_usable(bar))
        });
        let Some(bar) = self.pet_bar.get_mut().filter(|_| usable) else {
            return false;
        };
        let Some(enabled) = crate::ui_pet_book::set_autocast(&mut bar.spells, spell_id, state)
        else {
            return false;
        };
        if let Some(c) = &self.commands {
            let _ = c.0.send(crate::net::ClientCommand::PetSpellAutocast {
                pet_guid: bar.spells.pet_guid,
                spell_id,
                enabled,
            });
        }
        true
    }

    /// The pet's book at the call, the array the last `SMSG_PET_SPELLS` filled (`0x5d6cc0`).
    fn pet_book(
        &self,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> Option<benilla_ui::script::PetBookState> {
        Some(crate::ui_pet_book::book_live(
            self.pet_bar.get()?,
            self.spells.as_deref()?,
            self.classes.as_deref(),
            self.pet_unit.as_ref()?,
            (anchor, ui_now),
        ))
    }

    /// The history's answer for the spell at the call (`0x53e54c` into `0x809000`), with the
    /// category and global-cooldown legs its record names.
    fn spell_cooldown(
        &self,
        spell_id: u32,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> Option<(i64, u32, bool)> {
        let d = self.spells.as_ref().and_then(|s| s.catalog.get(spell_id));
        self.cooldowns
            .get()?
            .info(spell_id, 0, d, std::time::Instant::now())
            .ui_triple(anchor, ui_now)
    }
}

/// [`LiveView`] lent to every verb for the run of the system that takes it: the parameter each
/// system holding the VM takes beside it, so every call into Lua it makes reads the `World`. `O`
/// names the lendable resources the system writes itself, through [`OwnedMut`].
pub(crate) struct LendLive<'w, 's, O: Owns = ()> {
    loan: LiveLoan<LiveView<'w, 's>>,
    _owns: PhantomData<O>,
}

impl<'w, 's, O: Owns> LendLive<'w, 's, O> {
    /// The lent view, read by the system between its calls into Lua; a verb that writes the view,
    /// or the system's own owned write, panics while it lives.
    // Never hold it across a call into Lua: a verb that writes the view would panic at the call.
    pub(crate) fn view(&self) -> benilla_ui::script::ViewRef<'_, LiveView<'w, 's>> {
        self.loan.view()
    }
}

/// The state [`LendLive`] builds its view from.
type ViewState = <LiveView<'static, 'static> as SystemParam>::State;

/// A lendable resource's id, then the loan's and the owner's marks.
type SlotIds = (ComponentId, ComponentId, ComponentId);

/// The lendable resources' ids: the history, the pet's words, the selection and the player's place.
type SlotState = (SlotIds, SlotIds, SlotIds, SlotIds);

/// Register `X` and its pair's marks.
fn slot_ids<X: Resource>(world: &mut World) -> SlotIds {
    let mut r = world.components_registrator();
    (
        r.register_resource::<X>(),
        r.register_resource::<LoanMark<X>>(),
        r.register_resource::<OwnerMark<X>>(),
    )
}

/// Register the view's access to lendable `X`. Unowned, a read, refused beside a write (B0002);
/// owned, the write its [`OwnedMut`] pairs with, refused beside any other access.
fn slot_access<X: Resource>(
    owned: bool,
    (id, loan, owner): SlotIds,
    system: &SystemMeta,
    set: &mut FilteredAccessSet,
) {
    let name = &format!("{} in {}", std::any::type_name::<X>(), system.name());
    if owned {
        owned_access(name, id, loan, owner, set);
        return;
    }
    assert!(
        !set.combined_access().has_resource_write(id),
        "error[B0002]: {name} is written beside the lent view: write it through `OwnedMut` and \
         name it in the system's `LendLive` owns",
    );
    set.add_unfiltered_resource_read(id);
}

/// The lendable resource `id` as its storage pointer, made without a reference, so the owner's
/// writes through its own pointer leave it valid.
///
/// # Safety
/// The calling system's access to `id` is registered.
unsafe fn slot_fetch<X: Resource>(
    world: UnsafeWorldCell<'_>,
    id: ComponentId,
) -> Option<NonNull<X>> {
    // SAFETY: registered by the caller.
    unsafe { world.get_resource_by_id(id) }.and_then(|p| NonNull::new(p.as_ptr().cast::<X>()))
}

// SAFETY: every access is `LiveView`'s or a lendable resource's, registered in `init_access`; an
// owned resource is only read through its pointer between the owner's writes.
unsafe impl<O: Owns> SystemParam for LendLive<'_, '_, O> {
    type State = (ViewState, SlotState);
    type Item<'w, 's> = LendLive<'w, 's, O>;

    fn init_state(world: &mut World) -> Self::State {
        (
            <LiveView<'static, 'static> as SystemParam>::init_state(world),
            (
                slot_ids::<crate::spell::Cooldowns>(world),
                slot_ids::<crate::ui_pet::PetBar>(world),
                slot_ids::<crate::target::Selection>(world),
                slot_ids::<crate::area::PlayerPlace>(world),
            ),
        )
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
        let set = component_access_set;
        slot_access::<crate::spell::Cooldowns>(O::COOLDOWNS, state.1 .0, system_meta, set);
        slot_access::<crate::ui_pet::PetBar>(true, state.1 .1, system_meta, set);
        slot_access::<crate::target::Selection>(O::SELECTION, state.1 .2, system_meta, set);
        slot_access::<crate::area::PlayerPlace>(O::PLACE, state.1 .3, system_meta, set);
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
        // SAFETY: the caller's guarantees are `LiveView`'s.
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
        // SAFETY: the caller's guarantees are `LiveView`'s.
        let mut view = unsafe {
            <LiveView<'static, 'static> as SystemParam>::get_param(
                &mut state.0,
                system_meta,
                world,
                change_tick,
            )
        };
        // SAFETY: both registered in `init_access`.
        unsafe {
            view.cooldowns
                .set(slot_fetch(world, state.1 .0 .0), O::COOLDOWNS);
            view.pet_bar.set(slot_fetch(world, state.1 .1 .0), true);
            view.selection
                .set(slot_fetch(world, state.1 .2 .0), O::SELECTION);
            view.place.set(slot_fetch(world, state.1 .3 .0), O::PLACE);
        }
        // SAFETY: the system drops its parameters on the thread that ran it when its run ends,
        // after any loan made inside it, and never leaks one.
        LendLive {
            loan: unsafe { LiveLoan::new(view) },
            _owns: PhantomData,
        }
    }
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

/// What [`with_live_world`] lends: the view as a system lends it, owning what it may write.
type WorldView = LendLive<'static, 'static, OwnsAll>;

/// The cached parameter state [`with_live_world`] lends from.
#[derive(Resource)]
pub(super) struct LiveViewState(SystemState<WorldView>);

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
    // Lent until it drops, after `f`.
    let _loan = state.0.get_mut(held.world);
    f(script)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spell::Cooldowns;
    use bevy::ecs::system::{RunSystemOnce, System};

    /// The message of a system's initialize panic, empty when it initializes.
    fn refusal<M>(system: impl IntoSystem<(), (), M>) -> String {
        let mut world = World::new();
        let mut system = IntoSystem::into_system(system);
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            system.initialize(&mut world);
        }))
        .err()
        .and_then(|e| {
            e.downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
        })
        .unwrap_or_default()
    }

    /// A plain write of the history beside a loan that does not name it is refused either way
    /// round (B0002), and the owner's own write handle is admitted.
    #[test]
    fn only_the_owner_writes_the_lent_history_beside_its_loan() {
        let before = refusal(|_w: ResMut<Cooldowns>, _l: LendLive| {});
        assert!(
            before.contains("B0002") && before.contains("Cooldowns"),
            "{before}"
        );
        let after = refusal(|_l: LendLive, _w: ResMut<Cooldowns>| {});
        assert!(
            after.contains("B0002") && after.contains("Cooldowns"),
            "{after}"
        );
        assert_eq!(
            refusal(|_w: OwnedMut<Cooldowns>, _l: LendLive<OwnsCooldowns>| {}),
            ""
        );
        assert_eq!(
            refusal(|_l: LendLive<OwnsCooldowns>, _w: OwnedMut<Cooldowns>| {}),
            ""
        );
    }

    /// Any other borrow of an owned resource beside its pair is refused (B0002) in every order: a
    /// `Res` or `ResMut`, a second `OwnedMut`, an `OwnedMut` beside a loan that does not own it.
    #[test]
    fn an_owned_resource_admits_no_other_borrow_in_any_order() {
        type L<'w, 's> = LendLive<'w, 's, OwnsCooldowns>;
        type W<'w> = OwnedMut<'w, Cooldowns>;
        let refused = [
            refusal(|_r: Res<Cooldowns>, _l: L, _w: W| {}),
            refusal(|_l: L, _r: Res<Cooldowns>, _w: W| {}),
            refusal(|_l: L, _w: W, _r: Res<Cooldowns>| {}),
            refusal(|_r: Res<Cooldowns>, _w: W, _l: L| {}),
            refusal(|_w: W, _r: Res<Cooldowns>, _l: L| {}),
            refusal(|_w: W, _l: L, _r: Res<Cooldowns>| {}),
            refusal(|_r: ResMut<Cooldowns>, _l: L, _w: W| {}),
            refusal(|_l: L, _w: W, _r: ResMut<Cooldowns>| {}),
            refusal(|_r: ResMut<Cooldowns>, _w: W| {}),
            refusal(|_w: W, _r: ResMut<Cooldowns>| {}),
            refusal(|_l: L, _w: W, _v: W| {}),
            refusal(|_w: W, _l: L, _v: W| {}),
            refusal(|_w: W, _l: L, _m: L| {}),
            refusal(|_l: LendLive, _w: W| {}),
            refusal(|_w: W, _l: LendLive| {}),
        ];
        for (i, message) in refused.iter().enumerate() {
            assert!(message.contains("B0002"), "case {i}: {message:?}");
        }
        // Each member of a set borrows it alone, beside the loan outside the set.
        assert_eq!(refusal(|_l: L, _s: ParamSet<(W, W)>| {}), "");
    }

    /// A shared view the system holds and its owned write exclude each other, and no verb reaches
    /// the view while the write lives: each would alias the storage.
    #[test]
    fn a_held_view_and_an_owned_write_exclude_each_other_and_the_verbs() {
        let run = |f: fn(&LendLive<OwnsCooldowns>, &mut OwnedMut<Cooldowns>)| {
            let mut world = World::new();
            world.init_resource::<Cooldowns>();
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                world
                    .run_system_once(
                        move |live: LendLive<OwnsCooldowns>, mut c: OwnedMut<Cooldowns>| {
                            f(&live, &mut c);
                        },
                    )
                    .unwrap();
            }))
            .is_err()
        };
        assert!(run(|live, c| {
            let view = live.view();
            c.write().generation += 1;
            drop(view);
        }));
        assert!(run(|live, c| {
            let mut w = c.write();
            let _view = live.view();
            w.generation += 1;
        }));
        assert!(run(|_, c| {
            let _w = c.write();
            benilla_ui::script::read_live(|_| ());
        }));
        assert!(run(|live, _| {
            let _view = live.view();
            benilla_ui::script::write_live(|_| ());
        }));
        // The owner's own read is counted too: a verb's write beside it panics.
        assert!(run(|_, c| {
            let _read = c.read();
            benilla_ui::script::write_live(|_| ());
        }));
        // Apart, each goes through, and the guards are back to zero after a run that unwound.
        assert!(!run(|live, c| {
            let seen = live.view().cooldowns.get().map(|c| c.generation);
            c.write().generation += 1;
            assert_eq!(seen, Some(0));
            assert_eq!(c.read().generation, 1);
            assert_eq!(benilla_ui::script::read_live(|_| ()), Some(()));
            assert_eq!(benilla_ui::script::write_live(|_| ()), Some(()));
        }));
    }

    /// The owner's write is the next read of its view, inside the same run, and the view lent
    /// for that run is gone once it ends, on unwind too.
    #[test]
    fn the_owners_write_is_read_at_the_next_call_and_the_loan_ends_with_the_run() {
        let mut world = World::new();
        world.init_resource::<Cooldowns>();
        let seen = world
            .run_system_once(
                |live: LendLive<OwnsCooldowns>, mut c: OwnedMut<Cooldowns>| {
                    let read = |live: &LendLive<OwnsCooldowns>| {
                        live.view().cooldowns.get().map(|c| c.generation)
                    };
                    let first = read(&live);
                    c.write().generation += 7;
                    let lent = benilla_ui::script::read_live(|_| ()).is_some();
                    (first, read(&live), lent)
                },
            )
            .unwrap();
        assert_eq!(seen, (Some(0), Some(7), true));
        assert!(benilla_ui::script::read_live(|_| ()).is_none());
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            world
                .run_system_once(|_live: LendLive<OwnsCooldowns>| panic!("a run that unwinds"))
                .unwrap();
        }));
        assert!(unwound.is_err());
        assert!(benilla_ui::script::read_live(|_| ()).is_none());
        assert!(world.resource_ref::<Cooldowns>().is_changed());
    }
}
