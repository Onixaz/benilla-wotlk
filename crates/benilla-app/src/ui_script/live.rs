//! The VM's live view of the client and the events a packet handler fires. 12340's verbs read the live object when called (`0x60f396`) and a handler fires its events once its changes apply (`0x520fe0`). [`LiveView`] is the [`LiveState`] over the `World` every call into Lua is lent: a system holding the VM takes [`LendLive`], which lends it for the system's run, and an exclusive one lends through [`with_live_world`] or [`lend_world`]. A resource such a system writes itself is lent by its own borrow ([`OwnedMut`]). [`UiEvents`] is the queue a handler fires into, emptied into the VM once that handler's commands apply.

use std::marker::PhantomData;
use std::ptr::NonNull;

use benilla_ui::script::{LiveLoan, LiveState, ScriptValue, UiScript};
use bevy::ecs::change_detection::Tick;
use bevy::ecs::component::ComponentId;
use bevy::ecs::query::FilteredAccessSet;
use bevy::ecs::system::{
    ReadOnlySystemParam, SystemMeta, SystemParam, SystemParamValidationError, SystemState,
};
use bevy::ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;

use crate::ui_bank::BankOpen;
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
    battlefield: crate::ui_battlefield::BattlefieldView<'w, 's>,
    score: crate::ui_battlefield_score::ScoreView<'w>,
    arena: Option<Res<'w, crate::ui_arena::ArenaOpponents>>,
    honor: crate::ui_honor::HonorView<'w>,
    arena_teams: crate::ui_arena_team::ArenaTeamView<'w>,
    positions: crate::ui_battlefield_positions::PositionsView<'w, 's>,
    inspect: crate::ui_inspect::InspectLive<'w>,
    quest: crate::ui_quest_watch::QuestView<'w, 's>,
    clock: Option<Res<'w, crate::net::ServerWallClock>>,
    world_state_ui: crate::world_state_ui::WorldStateUiLive<'w>,
    item_range: crate::ui_quest_watch::ItemRangeLive<'w, 's>,
    quest_tooltip: crate::ui_quest_log::tooltip::QuestTooltipLive<'w, 's>,
    quest_rows: crate::ui_quest_log::rows::QuestRowLive<'w, 's>,
    net: Option<Res<'w, crate::net::NetCommands>>,
    bank_prices: Option<Res<'w, crate::ui_bank::BankPrices>>,
    item_tables: Option<Res<'w, crate::ui_items::feed::ItemTablesLive>>,
    /// The enchant catalog a bound item's trade stamp tests for a binding enchant (`0x708b40`).
    enchants: Option<Res<'w, crate::items::Enchants>>,
    /// The item cache, a refund ask's template test (`0x62d580`).
    items: Option<Res<'w, crate::items::Items>>,
    /// The main-thread state a verb writes inside its call, filled by [`LendLive`].
    pub(crate) lent: Lent<'w>,
    companions: crate::ui_companion::CompanionLive<'w, 's>,
    pets: crate::ui_pet_stats::PetView<'w>,
    possess: crate::ui_pet::possess::PossessLive<'w>,
    pub(crate) stable: crate::ui_stable::StableView<'w, 's>,
    pub(crate) cooldowns: Slot<'w, crate::spell::Cooldowns>,
    pub(crate) pet_bar: Slot<'w, crate::ui_pet::PetBar>,
    pub(crate) selection: Slot<'w, crate::target::Selection>,
    pub(crate) place: Slot<'w, crate::area::PlayerPlace>,
    /// The action bar's store: the spell book, the bar's slots and the equipment sets.
    pub(crate) actions: Slot<'w, crate::ui_action::PlayerActions>,
    /// The items a send locked (`item+0x314` bit 0), read into each slot's `locked`.
    pub(crate) pending: Slot<'w, crate::pending_item_ops::PendingItemOps>,
    /// The charter records a bag slot's charter reads.
    pub(crate) petitions: Slot<'w, crate::ui_petition::PetitionState>,
    /// The wire, for the sends a verb makes inside its call.
    commands: Option<Res<'w, crate::net::NetCommands>>,
    /// The usability walk's other inputs, for `GetPetActionSlotUsable` (`0x5d5400`).
    walk: Option<crate::ui_pet::PetSlotWalk<'w, 's>>,
    /// The bar's pet, our seat and the pet's auras, for the bar's usability and active reads.
    pet_unit: Option<crate::ui_pet::PetUnit<'w, 's>>,
    /// The spell records, the cooldown query's category and global-cooldown legs.
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    /// `SummonProperties.dbc`, the dismiss's no-dismiss row (`0x5d484c`).
    summons: Option<Res<'w, crate::spell::SummonSlots>>,
    /// The active mover (`[0xca1238]`), the dismiss's controlled-vehicle test (`0x5d4744`).
    mover: Query<'w, 's, &'static crate::net::Guid, With<crate::net::ActiveMover>>,
    /// `ChrClasses.dbc`, the pet book's token and the requirement state's class flags.
    classes: Option<Res<'w, crate::chr_classes::ChrClassTable>>,
    /// The requirement state's proficiency masks, standings and faction table.
    proficiencies: Option<Res<'w, crate::net::Proficiencies>>,
    reputations: Option<Res<'w, crate::net::Reputations>>,
    factions: Option<Res<'w, crate::target::Factions>>,
    /// The currency tables, the item cache and the bag icons.
    currency: crate::ui_currency::CurrencyLive<'w>,
    /// The bags' objects, item cache and catalogs, read at the call.
    containers: Option<crate::ui_items::ContainerLive<'w, 's>>,
    /// `ItemSet.dbc`, the name cache and the bonuses' token inputs, read at the call.
    item_sets: Option<crate::ui_items::ItemSetLive<'w, 's>>,
    /// The item cache and the catalogs a record's names resolve through, read at the call.
    item_records: Option<crate::ui_items::ItemRecordLive<'w, 's>>,
    /// The caches a guild bank slot and log row resolve through at the call.
    guild_bank: Option<crate::ui_guild_bank::BankCatalogs<'w>>,
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
    pub(crate) bank: LentSlot<'w, BankOpen>,
    pub(crate) refunds: LentSlot<'w, crate::ui_socket::RefundRecords>,
    pub(crate) guild_bank: LentSlot<'w, crate::ui_guild_bank::GuildBankState>,
}

/// The bits a system's [`LendLive`] names the [`Lent`] state it writes itself by.
pub(crate) mod lent {
    pub(crate) const SIGNALS: u8 = 1;
    pub(crate) const ASKS: u8 = 2;
    pub(crate) const MARKS: u8 = 4;
    pub(crate) const EXPIRIES: u8 = 8;
    pub(crate) const GIVER: u8 = 16;
    pub(crate) const BANK: u8 = 32;
    pub(crate) const REFUNDS: u8 = 64;
    pub(crate) const GUILD_BANK: u8 = 128;
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
lendable!(BankOpen, bank, lent::BANK);
lendable!(crate::ui_socket::RefundRecords, refunds, lent::REFUNDS);
lendable!(
    crate::ui_guild_bank::GuildBankState,
    guild_bank,
    lent::GUILD_BANK
);

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

// SAFETY: it registers no access and fetches nothing; `LendLive` fills it under its own access.
unsafe impl SystemParam for Lent<'_> {
    type State = ();
    type Item<'w, 's> = Lent<'w>;

    fn init_state(_: &mut World) -> Self::State {}

    fn init_access(_: &(), _: &mut SystemMeta, _: &mut FilteredAccessSet, _: &mut World) {}

    unsafe fn get_param<'w, 's>(
        _: &'s mut (),
        _: &SystemMeta,
        _: UnsafeWorldCell<'w>,
        _: Tick,
    ) -> Self::Item<'w, 's> {
        Lent::default()
    }
}

// SAFETY: it accesses nothing.
unsafe impl ReadOnlySystemParam for Lent<'_> {}

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

    /// The lent state a bag read takes.
    fn container_lent(&self) -> crate::ui_items::ContainerLent<'_> {
        crate::ui_items::ContainerLent {
            cooldowns: self.cooldowns.get(),
            pending: self.pending.get(),
            petitions: self.petitions.get(),
            banker: self.lent.bank.get().is_some_and(|b| b.banker.is_some()),
        }
    }

    /// A held player's PvP side (`0x54d074`).
    fn player_side(&self, guid: u64) -> Option<i8> {
        let entity = self.player_entity(guid)?;
        let race = self.stores.get(entity).ok()?.0.unit_race()?;
        Some(crate::ui_unit::race_pvp_team(race))
    }
}

/// A resource a VM holder may write, read by the verbs through its storage pointer at the call.
pub(crate) struct Slot<'w, X: Resource> {
    ptr: Option<NonNull<X>>,
    /// The lending system's world and `X`'s id when it writes `X` itself, so a verb's write marks it.
    owner: Option<(UnsafeWorldCell<'w>, ComponentId)>,
    _life: PhantomData<&'w X>,
}

impl<'w, X: Resource> Slot<'w, X> {
    /// The resource as the verbs read it now.
    pub(crate) fn get(&self) -> Option<&X> {
        // SAFETY: the storage pointer, read under the lending system's access; an owner's write panics beside the shared view or verb this read is made through, and none outlives it.
        self.ptr.map(|p| unsafe { p.as_ref() })
    }

    /// The resource, marked changed, for a verb's write inside the call; `None` unless the lending system owns it.
    pub(crate) fn get_mut(&mut self) -> Option<&mut X> {
        let (world, id) = self.owner?;
        let mut ptr = self.ptr?;
        // SAFETY: the owner registered the write; the ticks are borrowed only to mark it changed.
        if let Some(mut ticks) = unsafe { world.get_resource_mut_by_id(id) } {
            ticks.set_changed();
        }
        // SAFETY: a verb's write excludes every other borrow of the view for its length, and none outlives the call.
        Some(unsafe { ptr.as_mut() })
    }

    fn set(&mut self, ptr: Option<NonNull<X>>, owner: Option<(UnsafeWorldCell<'w>, ComponentId)>) {
        self.ptr = ptr;
        self.owner = owner;
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
            owner: None,
            _life: PhantomData,
        }
    }
}

// SAFETY: it accesses nothing.
unsafe impl<X: Resource> ReadOnlySystemParam for Slot<'_, X> {}

/// The lendable resources a [`LendLive`] system writes through [`OwnedMut`]; every loan owns the pet's words (`0x5d5d88`).
pub(crate) trait Owns: 'static {
    const COOLDOWNS: bool = false;
    const SELECTION: bool = false;
    const PLACE: bool = false;
    const ACTIONS: bool = false;
    const PENDING: bool = false;
    const PETITIONS: bool = false;
    /// The [`Lent`] state, by [`lent`] bits, the system lends from its own borrow ([`LendLive::lending`]).
    const LENT: u8 = 0;
}

impl Owns for () {}

/// Both sets' resources.
impl<A: Owns, B: Owns> Owns for (A, B) {
    const COOLDOWNS: bool = A::COOLDOWNS || B::COOLDOWNS;
    const SELECTION: bool = A::SELECTION || B::SELECTION;
    const PLACE: bool = A::PLACE || B::PLACE;
    const ACTIONS: bool = A::ACTIONS || B::ACTIONS;
    const PENDING: bool = A::PENDING || B::PENDING;
    const PETITIONS: bool = A::PETITIONS || B::PETITIONS;
    const LENT: u8 = A::LENT | B::LENT;
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

/// The system writes [`crate::ui_action::PlayerActions`].
pub(crate) struct OwnsActions;
impl Owns for OwnsActions {
    const ACTIONS: bool = true;
}

/// The system writes [`crate::pending_item_ops::PendingItemOps`].
pub(crate) struct OwnsPending;
impl Owns for OwnsPending {
    const PENDING: bool = true;
}

/// The system writes [`crate::ui_petition::PetitionState`].
pub(crate) struct OwnsPetitions;
impl Owns for OwnsPetitions {
    const PETITIONS: bool = true;
}

/// The system writes [`crate::spell::Cooldowns`].
pub(crate) struct OwnsCooldowns;
impl Owns for OwnsCooldowns {
    const COOLDOWNS: bool = true;
}

/// The system lends the frame's [`UniqueSignals`] from its own borrow.
pub(crate) struct OwnsSignals;
impl Owns for OwnsSignals {
    const LENT: u8 = lent::SIGNALS;
}

/// The system lends the quest timers' [`QuestExpiries`] from its own borrow.
pub(crate) struct OwnsExpiries;
impl Owns for OwnsExpiries {
    const LENT: u8 = lent::EXPIRIES;
}

/// An exclusive lender owns what the verbs write: the cooldowns (and the pet's words every loan owns).
pub(crate) struct OwnsVerbWrites;
impl Owns for OwnsVerbWrites {
    const COOLDOWNS: bool = true;
}

/// A VM holder's write handle on a resource its lent view reads, borrowed only for each write's length.
pub(crate) struct OwnedMut<'w, X: Resource> {
    world: UnsafeWorldCell<'w>,
    id: ComponentId,
    ptr: NonNull<X>,
}

impl<X: Resource> OwnedMut<'_, X> {
    /// The resource, marked changed, for as long as the returned borrow lives.
    pub(crate) fn write(&mut self) -> OwnedWrite<'_, X> {
        // SAFETY: the write is registered; the ticks are borrowed only to mark it changed.
        if let Some(mut ticks) = unsafe { self.world.get_resource_mut_by_id(self.id) } {
            ticks.set_changed();
        }
        let held = benilla_ui::script::owner_write();
        // SAFETY: the system's write access is registered; the borrow is tied to `&mut self`, and `held` keeps every verb and shared view of the loan off the storage while it lives.
        OwnedWrite {
            x: unsafe { self.ptr.as_mut() },
            _held: held,
        }
    }
}

impl<X: Resource> OwnedMut<'_, X> {
    /// The resource, counted as a shared read while the returned borrow lives.
    pub(crate) fn read(&self) -> LentRead<'_, X> {
        let held = benilla_ui::script::shared_read();
        // SAFETY: the system's access is registered; `held` keeps every write off the storage while the borrow lives.
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

/// A counted shared read of a lendable resource beside its owner's pair; absent resource, no read.
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
        // SAFETY: the storage pointer, read under the system's access; `held` panics beside an owner's write and keeps one from starting while it lives.
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

// SAFETY: a read is registered unless the system already writes it as an owned pair, whose writes the read's count excludes; fetched as a pointer, never a reference.
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
        // SAFETY: registered in `init_access`, or written by the system's own pair, which has not run yet; the reference ends with this statement.
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

/// Register an owner's write of lendable `id`, one half of the pair a [`LendLive`] owning it and its [`OwnedMut`] make: an earlier access to it must be the partner's (`partner` marked, `mine` not), else it is refused (B0002) as `ResMut` refuses it.
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
        // SAFETY: the write is registered; fetched as a pointer, the ticks borrowed only per write.
        let ptr = unsafe { slot_fetch(world, id) }.expect("validated");
        OwnedMut { world, id, ptr }
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
        let Self {
            quest_rows, lent, ..
        } = self;
        let Lent { asks, expiries, .. } = lent;
        quest_rows.row(quest_id, slot, get, expiries.get(), asks.get_mut())
    }

    fn quest_log_detail(
        &mut self,
        quest_id: u32,
        slot: u8,
    ) -> Option<Option<benilla_ui::script::QuestLogDetail>> {
        let Self {
            quest_rows, lent, ..
        } = self;
        quest_rows.detail(quest_id, slot, lent.asks.get_mut())
    }

    fn daily_quests_completed(&self) -> Option<u32> {
        self.quest_rows.daily_completed()
    }

    fn expire_quest(&mut self, quest_id: u32) -> Option<Option<String>> {
        let Self { quest, lent, .. } = self;
        quest.expire_quest(quest_id, lent.expiries.get_mut()?)
    }

    /// The last time response advanced on our monotonic clock, where the reference adds the local clock to its offset (`0x5dff40`). Deviation: a local clock step mid-session (an NTP correction) moves the reference's countdowns and not ours, as the step changes no time the server counts.
    fn server_unix_time(&self) -> Option<f64> {
        self.clock.as_ref()?.now_unix()
    }

    fn quest_reset_unix(&self) -> Option<f64> {
        self.clock.as_ref()?.quest_reset_unix()
    }

    fn send_quest_log_tail(&mut self, pois: &[u32], query_time: bool) {
        let Some(net) = self.net.as_deref() else {
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
        let Self {
            quest_rows, lent, ..
        } = self;
        quest_rows.rebuild_if_reask_due(lent.row_writes()?)
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
        let Self {
            index,
            stores,
            net,
            lent,
            ..
        } = self;
        let giver = lent.giver.get_mut()?;
        let npc_flags = giver
            .npc
            .and_then(|npc| index.as_ref()?.0.get(&npc).copied())
            .and_then(|e| stores.get(e).ok())
            .map(|s| s.0.unit_npc_flags());
        Some(crate::ui_quest::act(
            giver,
            action,
            net.as_deref()?,
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
        let Self { net, lent, .. } = self;
        Some(crate::ui_quest::select(
            lent.giver.get_mut()?,
            select,
            net.as_deref()?,
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

    /// The pet list's head held as a unit with a pet number and a hunter creator held as a player (`0x71b630`), and its creature record cached (`0x5a1602`).
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

    /// Our `UNIT_FIELD_CHARM` or `UNIT_FIELD_SUMMON` (`0x5a1b11`); no player object refuses too (`0x5a1b0b`).
    fn unit_pet(&self, guid: u64) -> Option<u64> {
        self.unit(guid)?.unit_pet_guid()
    }

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

    /// The slot word at the call (`0x5d4ec0`), its cooldown asked of the pet's history (`0x5d5345`) and its usability walked at the call (`0x5d5400`).
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

    /// A pet menu verb on the lent bar inside its call ([`crate::ui_pet::run_menu_verb`]).
    fn pet_menu_verb(
        &mut self,
        verb: benilla_ui::script::PetMenuVerb<'_>,
    ) -> Option<benilla_ui::script::PetMenuOutcome> {
        let stable_master = self
            .stable
            .open()
            .and_then(crate::ui_session::NpcSession::npc);
        let cx = crate::ui_pet::MenuCx {
            bar: self.pet_bar.get()?,
            pet: self.pet_unit.as_ref()?,
            spells: self.spells.as_deref(),
            summons: self.summons.as_deref(),
            commands: self.commands.as_deref()?,
            stable_master,
            mover: self.mover.iter().next().map(|g| g.0),
        };
        Some(crate::ui_pet::run_menu_verb(&cx, verb))
    }

    /// `PetStopAttack`'s clear on the lent bar (`0x5d4650`).
    fn pet_stop_attack(&mut self) {
        if let (Some(bar), Some(c)) = (self.pet_bar.get_mut(), &self.commands) {
            crate::ui_pet::stop_pet_attack(bar, c);
        }
    }

    /// A reaction or non-Attack command press on the lent words inside the call (`0x5d4210`): gate, latch and signal, then the send.
    fn pet_press(&mut self, slot: u32, order: Option<u32>) -> Option<bool> {
        let entry = crate::ui_pet::plain_press_word(self.pet_bar.get()?, slot, order)?;
        let selected = self.selection.get().and_then(|s| s.guid);
        let pet = self.pet_unit.as_ref()?;
        let bar = self.pet_bar.get_mut()?;
        Some(crate::ui_pet::press_plain(
            bar,
            pet,
            entry,
            selected,
            self.commands.as_deref(),
        ))
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

    /// The history's answer for the spell at the call (`0x53e54c` into `0x809000`), with the category and global-cooldown legs its record names.
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

    fn banker(&self) -> Option<u64> {
        self.lent.bank.get()?.banker
    }

    fn bank_bag_slots(&self) -> Option<u8> {
        Some(
            self.unit(self.active_player()?)?
                .player_bank_bag_slots_purchased()
                .unwrap_or(0),
        )
    }

    fn bank_slot_cost(&self, slot: u32) -> u32 {
        let row = u8::try_from(slot.saturating_sub(1)).unwrap_or(u8::MAX);
        self.bank_prices
            .as_ref()
            .and_then(|p| p.0.next_slot_price(row))
            .unwrap_or(0)
    }

    /// The active player, control, a banker and fewer than seven slots gate silently (`0x57bbe5`-`0x57bc10`); short of the price shows `ERR_BANKSLOT_INSUFFICIENT_FUNDS` (`0x57bc2a`); else `CMSG_BUY_BANK_SLOT` names the interact target (`0x57bc54`).
    fn purchase_bank_slot(&self) -> Option<&'static str> {
        let me = self.unit(self.active_player()?)?;
        let bank = self.lent.bank.get()?;
        bank.banker.filter(|_| self.has_control())?;
        let slots = me.player_bank_bag_slots_purchased().unwrap_or(0);
        if slots >= 7 {
            return None;
        }
        if me.player_money().unwrap_or(0) < self.bank_slot_cost(u32::from(slots) + 1) {
            return Some("ERR_BANKSLOT_INSUFFICIENT_FUNDS");
        }
        let guid = bank.interact_target().unwrap_or(0);
        debug!("bank: purchase next bag slot at {guid:#x}");
        let _ = self
            .net
            .as_deref()?
            .0
            .send(crate::net::ClientCommand::BuyBankSlot { guid });
        None
    }

    fn close_bank_interaction(&mut self) -> bool {
        self.lent
            .bank
            .get_mut()
            .is_some_and(|b| b.close_interaction())
    }

    fn item_tables(&self) -> Option<&benilla_ui::script::ItemTables> {
        self.item_tables.as_deref().map(|t| &t.tables)
    }

    fn item_set(
        &self,
        set_id: u32,
        global: &dyn Fn(&str) -> Option<String>,
    ) -> Option<benilla_ui::script::ItemSetView> {
        let me = self.self_guid.as_ref().and_then(|s| s.0).and_then(|g| {
            let &entity = self.index.as_ref()?.0.get(&g)?;
            self.stores.get(entity).ok()
        });
        let known = self.actions.get().map(|a| &a.spells);
        static NONE: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
        self.item_sets.as_ref()?.view(
            set_id,
            me,
            self.spells.as_deref(),
            known.unwrap_or(&NONE),
            global,
        )
    }

    fn item_record(
        &self,
        id: u32,
        tooltip: Option<benilla_ui::script::GlobalStrings<'_>>,
    ) -> Option<Option<benilla_ui::script::ItemTemplateView>> {
        let records = self.item_records.as_ref()?;
        let me = self.self_guid.as_ref().and_then(|s| s.0).and_then(|g| {
            let &entity = self.index.as_ref()?.0.get(&g)?;
            self.stores.get(entity).ok()
        });
        let known = self.actions.get().map(|a| &a.spells);
        static NONE: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
        Some(records.view(
            id,
            me,
            self.spells.as_deref(),
            known.unwrap_or(&NONE),
            tooltip,
        ))
    }

    fn bind_location(&self) -> Option<String> {
        self.item_records
            .as_ref()?
            .bind_location()
            .map(str::to_string)
    }

    fn guild_bank_open(&self) -> bool {
        self.lent.guild_bank.get().is_some_and(|b| b.is_open())
    }

    fn guild_bank_current_tab(&self) -> u8 {
        self.lent.guild_bank.get().map_or(0, |b| b.current_tab())
    }

    fn set_guild_bank_current_tab(&mut self, tab: u8) {
        if let Some(bank) = self.lent.guild_bank.get_mut() {
            bank.set_current_tab(tab);
        }
    }

    fn guild_bank_num_tabs(&self) -> u8 {
        self.lent.guild_bank.get().map_or(0, |b| b.num_tabs())
    }

    fn guild_bank_tab(&self, tab: u8) -> Option<benilla_ui::script::GuildBankTabView> {
        self.lent.guild_bank.get()?.tab_view(tab)
    }

    fn guild_bank_item(&self, tab: u8, slot: u8) -> Option<(u32, u32)> {
        let s = self.lent.guild_bank.get()?.slot(tab, slot)?;
        Some((s.item_id, s.count))
    }

    fn guild_bank_slot(&self, tab: u8, slot: u8) -> Option<benilla_ui::script::GuildBankSlotView> {
        let s = self.lent.guild_bank.get()?.slot(tab, slot)?;
        self.guild_bank
            .as_ref()?
            .slot_view(s, self.commands.as_deref()?)
    }

    fn guild_bank_texture(&self, tab: u8, slot: u8) -> Option<String> {
        let s = self.lent.guild_bank.get()?.slot(tab, slot)?;
        self.guild_bank.as_ref()?.texture(s.item_id)
    }

    fn guild_bank_locked(&self, tab: u8, slot: u8, item_id: u32) -> bool {
        self.lent
            .guild_bank
            .get()
            .is_some_and(|b| b.is_locked(tab, slot, item_id))
    }

    fn set_guild_bank_lock(&mut self, tab: u8, slot: u8, item_id: u32, locked: bool) {
        if let Some(bank) = self.lent.guild_bank.get_mut() {
            bank.set_lock(tab, slot, item_id, locked);
        }
    }

    fn guild_bank_withdrawals(&self) -> i32 {
        self.lent
            .guild_bank
            .get()
            .map_or(0, |b| b.withdrawals_remaining())
    }

    fn guild_bank_money(&self) -> u64 {
        self.lent.guild_bank.get().map_or(0, |b| b.money())
    }

    fn guild_bank_withdraw_money(&self) -> i32 {
        self.lent.guild_bank.get().map_or(0, |b| b.withdraw_money())
    }

    fn guild_bank_log_len(&self, tab: u8) -> usize {
        self.lent.guild_bank.get().map_or(0, |b| b.log_len(tab))
    }

    fn guild_bank_log(&self, tab: u8, index: u8) -> Option<benilla_ui::script::GuildBankLogView> {
        let row = self.lent.guild_bank.get()?.log_row(tab, index)?;
        Some(
            self.guild_bank
                .as_ref()?
                .log_view(row, self.commands.as_deref()?),
        )
    }

    fn clear_guild_banker(&mut self) {
        if let Some(bank) = self.lent.guild_bank.get_mut() {
            bank.clear_banker();
        }
    }

    fn guild_bank_query_text(&mut self, tab: u8) -> bool {
        let commands = self.commands.as_deref();
        match (self.lent.guild_bank.get_mut(), commands) {
            (Some(bank), Some(net)) => bank.query_text(tab, net),
            _ => true,
        }
    }

    fn player_fields(&self) -> Option<benilla_ui::script::PlayerFields> {
        let me = self.unit(self.active_player()?)?;
        Some(crate::ui_items::player_fields(me, self.classes.as_deref()))
    }

    fn player_skill(&self, skill: u32) -> Option<Option<u32>> {
        let me = self.unit(self.active_player()?)?;
        Some(crate::ui_items::player_skill(me, skill))
    }

    fn player_proficiency(&self, class: u32) -> Option<Option<u32>> {
        self.unit(self.active_player()?)?;
        Some(
            self.proficiencies
                .as_deref()
                .and_then(|p| p.0.get(&class).copied()),
        )
    }

    fn player_rep_rank(&self, faction: u32) -> Option<u8> {
        let me = self.unit(self.active_player()?)?;
        Some(crate::ui_items::player_rep_rank(
            me,
            self.reputations.as_deref(),
            self.factions.as_deref(),
            faction,
        ))
    }

    fn player_can_dual_wield(&self) -> Option<bool> {
        self.unit(self.active_player()?)?;
        Some(crate::ui_items::can_dual_wield(
            self.actions.get(),
            self.spells.as_deref(),
        ))
    }

    fn player_level(&self) -> Option<u32> {
        let me = self.unit(self.active_player()?)?;
        Some(me.unit_level().unwrap_or(0))
    }

    fn container(
        &self,
        bag: i64,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> Option<Option<benilla_ui::script::ContainerState>> {
        self.containers.as_ref()?.bag(
            &self.container_lent(),
            bag,
            crate::ui_items::BagRead::Held,
            (anchor, ui_now),
        )
    }

    fn container_slot(
        &self,
        bag: i64,
        slot: u32,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> Option<Option<benilla_ui::script::ContainerSlot>> {
        self.containers.as_ref()?.slot(
            &self.container_lent(),
            bag,
            slot,
            crate::ui_items::BagRead::Held,
            (anchor, ui_now),
        )
    }

    fn indexed_container(
        &self,
        bag: i64,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> Option<Option<benilla_ui::script::ContainerState>> {
        self.containers.as_ref()?.bag(
            &self.container_lent(),
            bag,
            crate::ui_items::BagRead::Index,
            (anchor, ui_now),
        )
    }

    fn indexed_container_slot(
        &self,
        bag: i64,
        slot: u32,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> Option<Option<benilla_ui::script::ContainerSlot>> {
        self.containers.as_ref()?.slot(
            &self.container_lent(),
            bag,
            slot,
            crate::ui_items::BagRead::Index,
            (anchor, ui_now),
        )
    }

    fn has_key(&self) -> Option<bool> {
        self.containers.as_ref()?.has_key()
    }

    fn currency_catalog(&self) -> Option<&benilla_ui::script::CurrencyCatalogView> {
        self.currency.catalog()
    }

    fn currency_holdings(&self) -> Option<benilla_ui::script::CurrencyHoldings> {
        let me = self.unit(self.active_player()?)?;
        let index = self.index.as_ref()?;
        Some(self.currency.holdings(me, |guid| {
            let &entity = index.0.get(&guid)?;
            Some(&self.stores.get(entity).ok()?.0)
        }))
    }

    fn ask_item_record(&self, id: u32) {
        self.currency.ask(id);
    }

    fn clear_banker(&mut self) {
        if let Some(b) = self.lent.bank.get_mut() {
            b.clear();
        }
    }

    fn refund_record(&self, item: u64) -> Option<Option<benilla_ui::script::RefundView>> {
        let records = self.lent.refunds.get()?;
        let held = self.index.as_ref()?.0.contains_key(&item);
        Some(held.then(|| records.view(item)).flatten())
    }

    fn bound_trade_stamp(&self, item: u64) -> Option<Option<u32>> {
        let index = self.index.as_ref()?;
        let fields = index
            .0
            .get(&item)
            .and_then(|&e| self.stores.get(e).ok())
            .map(|s| &s.0);
        let me = self.active_player();
        Some(fields.and_then(|f| crate::ui_socket::trade_stamp(f, me, self.enchants.as_deref())))
    }

    fn ask_refund_record(&mut self, item: u64) {
        let entry = self
            .index
            .as_ref()
            .and_then(|i| i.0.get(&item))
            .and_then(|&e| self.stores.get(e).ok())
            .and_then(|s| s.0.object_entry());
        let refundable = entry
            .zip(self.items.as_deref())
            .is_some_and(|(entry, items)| crate::ui_socket::refundable(items, entry));
        let Some(commands) = self.commands.as_deref() else {
            return;
        };
        if let (true, Some(records)) = (refundable, self.lent.refunds.get_mut()) {
            records.ask(item, commands);
        }
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

/// [`LiveView`] lent to every verb for the run of the system that takes it: the parameter each system holding the VM takes beside it, so every call into Lua it makes reads the `World`. `O` names the lendable resources the system writes itself, through [`OwnedMut`].
pub(crate) struct LendLive<'w, 's, O: Owns = ()> {
    loan: LiveLoan<LiveView<'w, 's>>,
    _owns: PhantomData<O>,
}

impl<'w, 's, O: Owns> LendLive<'w, 's, O> {
    /// The lent view, read by the system between its calls into Lua and never held across one: a verb that writes the view, or the system's own owned write, panics while it lives.
    pub(crate) fn view(&self) -> benilla_ui::script::ViewRef<'_, LiveView<'w, 's>> {
        self.loan.view()
    }

    /// Run `f`, a call into Lua, with `state`, a borrow this system owns, lent to its verbs until `f` returns or unwinds.
    pub(crate) fn lending<T: Lendable, R>(&mut self, state: &mut T, f: impl FnOnce() -> R) -> R {
        assert!(
            O::LENT & T::BIT != 0,
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

    /// `f` over the guild bank this parameter borrows, between calls into Lua; a verb reached from inside `f` panics.
    pub(crate) fn with_guild_bank<R>(
        &self,
        f: impl FnOnce(Option<&crate::ui_guild_bank::GuildBankState>) -> R,
    ) -> R {
        self.loan.with_view(|v| f(v.lent.guild_bank.get()))
    }

    /// `f` over the guild bank this parameter borrows, to write between calls into Lua; a verb reached from inside `f` panics.
    pub(crate) fn with_guild_bank_mut<R>(
        &mut self,
        f: impl FnOnce(Option<&mut crate::ui_guild_bank::GuildBankState>) -> R,
    ) -> R {
        self.loan.with_view_mut(|v| f(v.lent.guild_bank.get_mut()))
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
    &'a mut LiveLoan<LiveView<'w, 's>>,
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

/// A lendable resource's id, then the loan's and the owner's marks.
type SlotIds = (ComponentId, ComponentId, ComponentId);

/// The lendable resources' ids: the history, the pet's words, the selection, the player's place, the action store, the item locks and the charter records.
type SlotState = (
    SlotIds,
    SlotIds,
    SlotIds,
    SlotIds,
    SlotIds,
    SlotIds,
    SlotIds,
);

/// Register `X` and its pair's marks.
fn slot_ids<X: Resource>(world: &mut World) -> SlotIds {
    let mut r = world.components_registrator();
    (
        r.register_resource::<X>(),
        r.register_resource::<LoanMark<X>>(),
        r.register_resource::<OwnerMark<X>>(),
    )
}

/// Register the view's access to lendable `X`. Unowned, a read, refused beside a write (B0002); owned, the write its [`OwnedMut`] pairs with, refused beside any other access.
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

/// The lendable resource `id` as its storage pointer, made without a reference so the owner's writes through its own pointer leave it valid; sound while the calling system's access to `id` is registered.
unsafe fn slot_fetch<X: Resource>(
    world: UnsafeWorldCell<'_>,
    id: ComponentId,
) -> Option<NonNull<X>> {
    // SAFETY: registered by the caller.
    unsafe { world.get_resource_by_id(id) }.and_then(|p| NonNull::new(p.as_ptr().cast::<X>()))
}

/// The [`Lent`] state's ids, in [`lent`] bit order.
type LentIds = [ComponentId; 8];

/// Register the [`Lent`] state's ids.
fn lent_ids(world: &mut World) -> LentIds {
    let mut reg = world.components_registrator();
    [
        reg.register_non_send::<UniqueSignals>(),
        reg.register_non_send::<QuestAsks>(),
        reg.register_non_send::<RebuildMarks>(),
        reg.register_non_send::<QuestExpiries>(),
        reg.register_non_send::<QuestGiver>(),
        reg.register_non_send::<BankOpen>(),
        reg.register_non_send::<crate::ui_socket::RefundRecords>(),
        reg.register_non_send::<crate::ui_guild_bank::GuildBankState>(),
    ]
}

/// Declare `T`'s borrow unless the system owns it.
fn lent_access<T: 'static>(
    own: u8,
    bit: u8,
    id: &ComponentId,
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
    mut id: ComponentId,
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

// SAFETY: every access is `LiveView`'s or a lendable resource's, registered in `init_access`; an owned resource is only read through its pointer between the owner's writes.
unsafe impl<O: Owns> SystemParam for LendLive<'_, '_, O> {
    type State = (ViewState, SlotState, LentIds);
    type Item<'w, 's> = LendLive<'w, 's, O>;

    fn init_state(world: &mut World) -> Self::State {
        (
            <LiveView<'static, 'static> as SystemParam>::init_state(world),
            (
                slot_ids::<crate::spell::Cooldowns>(world),
                slot_ids::<crate::ui_pet::PetBar>(world),
                slot_ids::<crate::target::Selection>(world),
                slot_ids::<crate::area::PlayerPlace>(world),
                slot_ids::<crate::ui_action::PlayerActions>(world),
                slot_ids::<crate::pending_item_ops::PendingItemOps>(world),
                slot_ids::<crate::ui_petition::PetitionState>(world),
            ),
            lent_ids(world),
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
        slot_access::<crate::ui_action::PlayerActions>(O::ACTIONS, state.1 .4, system_meta, set);
        type Pending = crate::pending_item_ops::PendingItemOps;
        slot_access::<Pending>(O::PENDING, state.1 .5, system_meta, set);
        type Petitions = crate::ui_petition::PetitionState;
        slot_access::<Petitions>(O::PETITIONS, state.1 .6, system_meta, set);
        let [signals, asks, marks, expiries, giver, bank, refunds, guild_bank] = &state.2;
        let (meta, own) = (system_meta, O::LENT);
        lent_access::<UniqueSignals>(own, lent::SIGNALS, signals, meta, set, world);
        lent_access::<QuestAsks>(own, lent::ASKS, asks, meta, set, world);
        lent_access::<RebuildMarks>(own, lent::MARKS, marks, meta, set, world);
        lent_access::<QuestExpiries>(own, lent::EXPIRIES, expiries, meta, set, world);
        lent_access::<QuestGiver>(own, lent::GIVER, giver, meta, set, world);
        lent_access::<BankOpen>(own, lent::BANK, bank, meta, set, world);
        type Refunds = crate::ui_socket::RefundRecords;
        lent_access::<Refunds>(own, lent::REFUNDS, refunds, meta, set, world);
        type GuildBank = crate::ui_guild_bank::GuildBankState;
        lent_access::<GuildBank>(own, lent::GUILD_BANK, guild_bank, meta, set, world);
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
            let own = |owned: bool, id| owned.then_some((world, id));
            let (
                (cd, ..),
                (bar, ..),
                (sel, ..),
                (place, ..),
                (actions, ..),
                (pending, ..),
                (pet, ..),
            ) = state.1;
            view.cooldowns
                .set(slot_fetch(world, cd), own(O::COOLDOWNS, cd));
            view.pet_bar.set(slot_fetch(world, bar), own(true, bar));
            view.selection
                .set(slot_fetch(world, sel), own(O::SELECTION, sel));
            view.place
                .set(slot_fetch(world, place), own(O::PLACE, place));
            view.actions
                .set(slot_fetch(world, actions), own(O::ACTIONS, actions));
            view.pending
                .set(slot_fetch(world, pending), own(O::PENDING, pending));
            view.petitions
                .set(slot_fetch(world, pet), own(O::PETITIONS, pet));
        }
        let [signals, asks, marks, expiries, giver, bank, refunds, guild_bank] = state.2;
        let (meta, tick, own) = (system_meta, change_tick, O::LENT);
        // SAFETY: each access was declared in `init_access` for exactly the bits not owned.
        view.lent = unsafe {
            Lent {
                signals: lent_fetch(own, lent::SIGNALS, signals, meta, world, tick),
                asks: lent_fetch(own, lent::ASKS, asks, meta, world, tick),
                marks: lent_fetch(own, lent::MARKS, marks, meta, world, tick),
                expiries: lent_fetch(own, lent::EXPIRIES, expiries, meta, world, tick),
                giver: lent_fetch(own, lent::GIVER, giver, meta, world, tick),
                bank: lent_fetch(own, lent::BANK, bank, meta, world, tick),
                refunds: lent_fetch(own, lent::REFUNDS, refunds, meta, world, tick),
                guild_bank: lent_fetch(own, lent::GUILD_BANK, guild_bank, meta, world, tick),
            }
        };
        // SAFETY: the system drops its parameters on the thread that ran it when its run ends,
        // after any loan made inside it, and never leaks one.
        LendLive {
            loan: unsafe { LiveLoan::new(view) },
            _owns: PhantomData,
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

/// What [`with_live_world`] lends: the view as a system lends it, owning what it may write.
type WorldView = LendLive<'static, 'static, OwnsVerbWrites>;

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
    (script, mut live): (Option<NonSendMut<UiScript>>, LendLive<'_, '_, OwnsSignals>),
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

    /// A plain write of the history beside a loan that does not name it is refused either way round (B0002), and the owner's own write handle is admitted.
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

    /// Any other borrow of an owned resource beside its pair is refused (B0002) in every order: a `Res` or `ResMut`, a second `OwnedMut`, an `OwnedMut` beside a loan that does not own it.
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

    /// A shared view the system holds and its owned write exclude each other, and no verb reaches the view while the write lives: each would alias the storage.
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

    /// The action store is lent like the history: a plain write beside a loan is refused (B0002), the owner's handle is admitted, and its write is the next read of the view in the same run.
    #[test]
    fn the_action_store_is_written_by_its_owner_and_read_through_the_view() {
        use crate::ui_action::PlayerActions;
        let refused = refusal(|_w: ResMut<PlayerActions>, _l: LendLive| {});
        assert!(
            refused.contains("B0002") && refused.contains("PlayerActions"),
            "{refused}"
        );
        assert_eq!(
            refusal(|_l: LendLive<OwnsActions>, _w: OwnedMut<PlayerActions>| {}),
            ""
        );
        let mut world = World::new();
        world.init_resource::<PlayerActions>();
        let seen = world
            .run_system_once(
                |live: LendLive<OwnsActions>, mut a: OwnedMut<PlayerActions>| {
                    let read = |live: &LendLive<OwnsActions>| {
                        live.view().actions.get().map(|a| a.spells.contains(&54197))
                    };
                    let before = read(&live);
                    a.write().spells.insert(54197);
                    (before, read(&live))
                },
            )
            .unwrap();
        assert_eq!(seen, (Some(false), Some(true)));
        assert!(world.resource_ref::<PlayerActions>().is_changed());
    }

    /// The owner's write is the next read of its view, inside the same run, and the view lent for that run is gone once it ends, on unwind too.
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

    /// A verb's in-call write of the pet's words (the drag, `0x5d5c3e`) and the owner's own write each mark the bar changed for a later reader in the frame, and no write marks nothing.
    #[test]
    fn a_verbs_write_of_the_pet_bar_is_seen_changed_by_a_later_reader() {
        use crate::ui_pet::PetBar;
        #[derive(Resource, Default)]
        struct Step(u8, Option<bool>);
        let mut world = World::new();
        world.init_resource::<PetBar>();
        world.init_resource::<Step>();
        let mut schedule = Schedule::default();
        schedule.add_systems(
            (
                |_live: LendLive, step: Res<Step>| {
                    if step.0 == 1 {
                        let wrote =
                            benilla_ui::script::write_live(|l| l.pet_place_action(&[(0, 0)]));
                        assert_eq!(wrote, Some(true));
                    }
                },
                |mut bar: OwnedMut<PetBar>, step: Res<Step>| {
                    if step.0 == 2 {
                        let _ = &mut bar.write().spells;
                    }
                },
                |bar: LentRef<PetBar>, mut step: ResMut<Step>| step.1 = Some(bar.is_changed()),
            )
                .chain(),
        );
        let mut seen = Vec::new();
        for n in 0..4 {
            world.resource_mut::<Step>().0 = n;
            schedule.run(&mut world);
            seen.push(world.resource::<Step>().1);
        }
        assert_eq!(seen, [Some(true), Some(true), Some(true), Some(false)]);
    }

    /// `UnstablePet`'s gate with no player object refuses, as its lookup's miss returns (`0x5a1b0b`).
    #[test]
    fn unstable_pet_refuses_with_no_player_object() {
        let mut world = World::new();
        let barred = world
            .run_system_once(|_live: LendLive| {
                benilla_ui::script::read_live(|l| l.charm_or_summon())
            })
            .unwrap();
        assert_eq!(barred, Some(true));
    }
}
