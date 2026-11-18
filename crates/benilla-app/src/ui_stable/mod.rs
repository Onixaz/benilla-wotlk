//! The stable window's app side: the list handler's globals (`0x5a11a0`), the view the verbs read and send through, and the window's events; a row's family comes from its creature record, whose answer fires `PET_STABLE_UPDATE` (`0x5a0e80`).

use benilla_protocol::messages::StabledPet;
use bevy::prelude::*;

use benilla_ui::script::{
    StableCallback, StableFamily, StableIntent, StableList, StableRow, UiScript,
};

use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, SelfPlayer};
use crate::ui_pet_stats::PetFamilyTables;
use crate::ui_script::live::UiEvents;
use crate::ui_script::UiDrain;
use crate::ui_session::{npc_out_of_range, NpcSession};
use crate::unit_auras::UnitAuras;

mod net;

/// `SPELL_AURA_OPEN_STABLE`: while one is up, a stable verb goes out with no master held.
pub(crate) const SPELL_AURA_OPEN_STABLE: u32 = 292;

pub(crate) struct UiStablePlugin;

impl Plugin for UiStablePlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<StableOpen>().add_systems(
            Update,
            (
                // The range guard, after the drains and before the world update.
                close_out_of_range
                    .after(UiDrain)
                    .before(benilla_world::schedule::WorldStage::Input),
                // After the VM ticks: the selection is written by a click in `UiPress` and by `PetStable_Update`'s re-pick (`PetStable.lua:48-63`) inside the handlers' events, so an earlier read shows the previous pet for a frame.
                feed_stable_booth
                    .in_set(UiDrain)
                    // Gated: the feed starts with a name lookup over every UI frame.
                    .run_if(
                        |open: Res<StableOpen>, booth: Res<crate::portrait::StableBooth>| {
                            // One more run after the close, which empties the booth.
                            open.npc().is_some()
                                || booth.unit.is_some()
                                || booth.display_id.is_some()
                        },
                    ),
            ),
        );
    }
}

/// The stable globals: the master `[0xc0f6c0]`, cleared inside `ClosePetStables`, the bought slots and the list's rows as the wire gave them, which outlive a close until the next world entry (`0x5a0ec0`).
#[derive(Resource, Default)]
pub(crate) struct StableOpen {
    master: u64,
    /// Slots bought (0..=4), not slots occupied.
    pub(crate) num_stable_slots: u8,
    /// The wire rows in wire order.
    pub(crate) pets: Vec<StabledPet>,
    /// The record asks a stable verb's miss queued, answered in order by the creature handler.
    asks: Vec<(u32, StableCallback)>,
}

impl StableOpen {
    /// The list handler's writes (`0x5a11a0`).
    pub(crate) fn open(&mut self, npc: u64, num_stable_slots: u8, pets: Vec<StabledPet>) {
        self.master = npc;
        self.num_stable_slots = num_stable_slots;
        self.pets = pets;
    }

    /// The close (`0x5a0f10`, the verb's `0x5a1996`): the master cleared; whether one was held.
    pub(crate) fn close(&mut self) -> bool {
        std::mem::take(&mut self.master) != 0
    }

    /// The interaction close first (`0x5a0f2c`, `0x5a1174`), whose stable arm fires `PET_STABLE_CLOSED` (`0x5a0f4c`), then the caller's: two with one held, else `forced` alone; a held master is always the interact target, as every target change closes the stable first (`0x518d67`, `0x51fd1c`).
    pub(crate) fn close_through_target(&mut self, forced: bool) -> usize {
        if self.close() {
            2
        } else {
            usize::from(forced)
        }
    }

    /// The world entry's reset (`0x5a0ec0`).
    pub(crate) fn clear(&mut self) {
        self.master = 0;
        self.num_stable_slots = 0;
        self.pets.clear();
    }

    /// The current pet's record: each current row is copied into it, so the last one stands
    /// (`0x5a12c6`).
    pub(crate) fn current(&self) -> Option<&StabledPet> {
        self.pets.iter().rfind(|p| p.is_current())
    }

    /// The stabled rows, in wire order: Lua indices `1..`.
    pub(crate) fn stabled(&self) -> impl Iterator<Item = &StabledPet> {
        self.pets.iter().filter(|p| !p.is_current())
    }

    /// The globals as the verbs read them.
    fn list(&self) -> StableList {
        let row = |p: &StabledPet| StableRow {
            pet_number: p.pet_number,
            entry: p.creature_entry,
            level: p.level,
            name: p.name.clone(),
        };
        StableList {
            master: self.master,
            num_slots: u32::from(self.num_stable_slots),
            current: self.current().map(row).unwrap_or_default(),
            stabled: self.stabled().map(row).collect(),
        }
    }

    /// The asks waiting on `entry`'s record, taken in the order they were made.
    fn take_asks(&mut self, entry: u32) -> Vec<StableCallback> {
        let mut taken = Vec::new();
        self.asks.retain(|&(e, cb)| {
            let hit = e == entry;
            if hit {
                taken.push(cb);
            }
            !hit
        });
        taken
    }

    /// Every ask dropped, as the cache's pending clear drops their callbacks (`0x66a010`).
    fn drop_asks(&mut self) {
        self.asks.clear();
    }
}

/// The portrait and the interaction close find the stable by its master.
impl NpcSession for StableOpen {
    fn npc(&self) -> Option<u64> {
        Some(self.master).filter(|&m| m != 0)
    }

    fn close(&mut self) {
        StableOpen::close(self);
    }
}

/// The close's event (`0x5a0f4c`), fired in turn once the closing system's commands apply.
pub(crate) fn fire_closed(world: &mut World) {
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.fire("PET_STABLE_CLOSED", vec![]);
    }
    crate::ui_script::live::fire_handler_events(world);
}

/// The interaction range guard's close (`0x513065`): a master out of the leash or gone closes the stable, its interact target already cleared, so `PET_STABLE_CLOSED` fires once.
fn close_out_of_range(
    mut open: ResMut<StableOpen>,
    index: Res<crate::net::GuidIndex>,
    self_q: Query<&Transform, With<SelfPlayer>>,
    transforms: Query<&Transform>,
    mut queue: Commands,
) {
    let Some(npc) = open.npc() else { return };
    if npc_out_of_range(npc, &index, &self_q, &transforms) == Some(true) && open.close() {
        debug!("ui_stable: stable master {npc:#x} out of range/gone — closed");
        queue.queue(fire_closed);
    }
}

/// The forced close an open-stable aura's removal runs (`0x71ea25`): `PET_STABLE_CLOSED` fired held or not, twice with a master held.
pub(crate) fn close_forced(world: &mut World) {
    close_and_fire(world, true);
}

/// The world leave's stable part (`0x528d5a`, `0x5a1160`): a held master closes, firing `PET_STABLE_CLOSED` twice.
pub(crate) fn leave_world(world: &mut World) {
    close_and_fire(world, false);
}

/// [`StableOpen::close_through_target`] with its events queued in turn.
fn close_and_fire(world: &mut World, forced: bool) {
    let fired = world
        .get_resource_mut::<StableOpen>()
        .map_or(usize::from(forced), |mut open| {
            open.close_through_target(forced)
        });
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        for _ in 0..fired {
            ui.fire("PET_STABLE_CLOSED", vec![]);
        }
    }
}

/// The UI world entry's stable reset (`0x528244`): the globals and the selection zeroed.
pub(crate) fn enter_world(world: &mut World) {
    if let Some(mut open) = world.get_resource_mut::<StableOpen>() {
        open.clear();
    }
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.write(UiScript::reset_stable_selection);
    }
}

/// What the stable verbs read and send through when called, part of the VM's live view.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct StableView<'w, 's> {
    /// Written inside `ClosePetStables` and by a miss's ask, through the lent view.
    open: Option<ResMut<'w, StableOpen>>,
    names: Option<Res<'w, NameCache>>,
    commands: Option<Res<'w, NetCommands>>,
    families: Option<Res<'w, PetFamilyTables>>,
    talents: Option<Res<'w, crate::ui_talent::Talents>>,
    prices: Option<Res<'w, StableSlotPrices>>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    self_auras: Query<'w, 's, &'static UnitAuras, With<SelfPlayer>>,
}

impl StableView<'_, '_> {
    /// The stable globals, read by a system between its calls into Lua.
    pub(crate) fn open(&self) -> Option<&StableOpen> {
        self.open.as_deref()
    }

    pub(crate) fn list(&self) -> StableList {
        self.open.as_ref().map(|o| o.list()).unwrap_or_default()
    }

    /// Creature `entry`'s family parts; a miss asks with our guid as the requester and queues `callback` (`0x5a13c1`).
    pub(crate) fn family(
        &mut self,
        entry: u32,
        callback: Option<StableCallback>,
        me: u64,
    ) -> Option<StableFamily> {
        let names = self.names.as_deref()?;
        let Some(record) = names.creature_record(entry) else {
            // With no callback the cache asks nothing (`0x67b6d0`).
            let callback = callback?;
            if let Some(commands) = self.commands.as_deref() {
                let _ = names.resolve_creature(entry, me, commands);
            }
            if let Some(open) = self.open.as_deref_mut() {
                open.asks.push((entry, callback));
            }
            return None;
        };
        let tables = self.families.as_deref()?;
        let row = tables.families.get(record.pet_family)?;
        Some(StableFamily {
            icon: tables
                .families
                .icon(record.pet_family)
                .unwrap_or_default()
                .to_string(),
            name: row.name.clone(),
            // The first `TalentTab.dbc` row admitting the family's talent type, in file order.
            talent_tree: self.talents.as_deref().and_then(|t| {
                Some(
                    t.catalog
                        .tabs_for_pet(row.pet_talent_type)
                        .first()?
                        .name
                        .clone(),
                )
            }),
            diet: tables
                .foods
                .for_mask(row.pet_food_mask)
                .into_iter()
                .map(str::to_string)
                .collect(),
        })
    }

    /// Creature `entry`'s record is cached.
    pub(crate) fn has_record(&self, entry: u32) -> bool {
        self.names
            .as_deref()
            .is_some_and(|n| n.creature_record(entry).is_some())
    }

    /// `StableSlotPrices.dbc` row `slot`'s price.
    pub(crate) fn slot_price(&self, slot: u32) -> Option<u32> {
        let purchased = u8::try_from(slot.checked_sub(1)?).ok()?;
        self.prices.as_deref()?.0.next_slot_price(purchased)
    }

    /// Send `verb` to the held master, guid 0 with none, as the verbs put it (`0x5a19c0`).
    pub(crate) fn send(&self, verb: StableIntent) {
        let Some(commands) = self.commands.as_deref() else {
            return;
        };
        let npc = self.list().master;
        debug!("ui_stable: {verb:?} (stable master {npc:#x})");
        let command = match verb {
            StableIntent::Stable => ClientCommand::StablePet { npc },
            StableIntent::Unstable(pet_number) => ClientCommand::UnstablePet { npc, pet_number },
            StableIntent::Swap(pet_number) => ClientCommand::StableSwapPet { npc, pet_number },
            StableIntent::BuySlot => ClientCommand::BuyStableSlot { npc },
        };
        let _ = commands.0.send(command);
    }

    /// Our auras with an open-stable effect, `cancelable` keeping only those (`0x726e8c`).
    fn open_stable_auras(&self, cancelable: bool) -> Vec<u32> {
        let (Ok(auras), Some(spells)) = (self.self_auras.single(), self.spells.as_deref()) else {
            return Vec::new();
        };
        auras
            .iter()
            .filter(|a| {
                !cancelable
                    || a.flags & benilla_protocol::messages::AURA_UPDATE_FLAG_CANCELABLE != 0
            })
            .filter(|a| {
                spells
                    .catalog
                    .get(a.spell_id)
                    .is_some_and(|rec| rec.effect_apply_aura.contains(&SPELL_AURA_OPEN_STABLE))
            })
            .map(|a| a.spell_id)
            .collect()
    }

    /// An open-stable aura is on us, `StablePet`'s second gate (`0x5a19c0`).
    pub(crate) fn open_stable_aura(&self) -> bool {
        !self.open_stable_auras(false).is_empty()
    }

    /// `ClosePetStables`' writes (`0x5a1971`, `0x5a1996`): our open-stable auras cancelled, then the master cleared; whether one was held.
    pub(crate) fn close(&mut self) -> bool {
        if let Some(commands) = self.commands.as_deref() {
            for spell_id in self.open_stable_auras(true) {
                let _ = commands.0.send(ClientCommand::CancelAura { spell_id });
            }
        }
        self.open.as_deref_mut().is_some_and(StableOpen::close)
    }
}

/// Point the model pane at the pet `GetSelectedStablePet()` names, as the reference's
/// `SetPetStablePaperdoll` does: the summoned pet shows its live body (`[unit+0xb30]`), and
/// anything else, or a failed live resolve, shows the row's creature display, whose late cache
/// answer re-points the pane.
fn feed_stable_booth(
    (script, live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    pet: crate::ui_pet::PetUnit,
    names: Res<NameCache>,
    mut booth: ResMut<crate::portrait::StableBooth>,
) {
    let Some(mut script) = script else {
        return;
    };
    // Every frame, so a stale yaw cannot snap the model when a pet is picked.
    booth.yaw = script.model_pane_facing("PetStableModel");

    // `GetSelectedStablePet()`: 0 the current pet, `1..=n` a stabled pet, -1 nothing, including a
    // pet the latest list no longer holds.
    let slot = script.stable_selection();

    let view = live.view();
    let live_pet = view.pet_bar.get().map_or(0, |bar| bar.spells.pet_guid);
    let Some(open) = view.stable.open() else {
        return;
    };
    let (unit, display_id) = stable_subject(
        slot,
        (live_pet != 0).then(|| pet.entity(live_pet)).flatten(),
        open,
        |entry| names.creature_record(entry).map(|r| r.display_id),
    );
    booth.unit = unit;
    booth.display_id = display_id;
}

/// The selection's `(live unit, display id)`. The live arm reads the pet guid and no row; the
/// display is resolved too, as the reference falls through to it when the live resolve fails.
/// Display 0 reads as no answer.
fn stable_subject(
    slot: i32,
    live_pet: Option<Entity>,
    open: &StableOpen,
    display_of: impl Fn(u32) -> Option<u32>,
) -> (Option<Entity>, Option<u32>) {
    let unit = if slot == 0 { live_pet } else { None };
    let row = match slot {
        0 => open.current(),
        k => usize::try_from(k - 1)
            .ok()
            .and_then(|k| open.stabled().nth(k)),
    };
    let display = row
        .and_then(|row| display_of(row.creature_entry))
        .filter(|&d| d != 0);
    (unit, display)
}

/// `StableSlotPrices.dbc`, the purchase row's prices; absent, the row quotes 0.
#[derive(Resource)]
pub(crate) struct StableSlotPrices(pub(crate) benilla_formats::StableSlotPrices);

#[cfg(test)]
mod tests {
    use super::*;

    /// A wire row: `current` clears flags bit `0x2`, as for the current pet.
    fn row(n: u32, entry: u32, current: bool) -> StabledPet {
        StabledPet {
            pet_number: n + 100,
            creature_entry: entry,
            level: 30,
            name: format!("pet{n}"),
            flags: if current { 1 } else { 3 },
        }
    }

    fn open_with(pets: Vec<StabledPet>) -> StableOpen {
        let mut open = StableOpen::default();
        open.open(0x42, 4, pets);
        open
    }

    /// One display per entry; entry `9` is the template that ships none.
    fn display_of(entry: u32) -> Option<u32> {
        match entry {
            1 => Some(4449),
            2 => Some(822),
            9 => Some(0),
            _ => None,
        }
    }

    #[test]
    fn the_selection_forks_between_a_live_body_and_a_bare_display() {
        // The stabled row first on the wire: indices follow the flags, not the position.
        let pets = open_with(vec![row(1, 2, false), row(0, 1, true)]);
        let live = Entity::from_raw_u32(7).expect("a test entity id");

        // Nothing selected: neither source, so the pane empties.
        assert_eq!(
            stable_subject(-1, Some(live), &pets, display_of),
            (None, None)
        );

        // Slot 0 with the pet out: the body, and the display beside it so a dismiss keeps the pane.
        assert_eq!(
            stable_subject(0, Some(live), &pets, display_of),
            (Some(live), Some(4449))
        );

        // Slot 0 with the pet dismissed: the display alone.
        assert_eq!(
            stable_subject(0, None, &pets, display_of),
            (None, Some(4449))
        );

        // A stabled slot never takes the live body: the live arm is gated on the selection.
        assert_eq!(
            stable_subject(1, Some(live), &pets, display_of),
            (None, Some(822))
        );

        // An index past the stabled pets: nothing.
        assert_eq!(
            stable_subject(2, Some(live), &pets, display_of),
            (None, None)
        );
    }

    #[test]
    fn an_unanswered_or_display_less_template_empties_the_pane() {
        assert_eq!(
            stable_subject(1, None, &open_with(vec![row(1, 404, false)]), display_of),
            (None, None),
            "the query has not landed yet"
        );
        assert_eq!(
            stable_subject(1, None, &open_with(vec![row(1, 9, false)]), display_of),
            (None, None),
            "the template ships no display"
        );
    }

    /// The close clears the master and keeps the list; the world entry zeroes both.
    #[test]
    fn the_close_keeps_the_list_and_the_world_entry_zeroes_it() {
        let mut open = open_with(vec![row(1, 2, false), row(0, 1, true)]);
        assert!(open.close());
        assert!(!open.close(), "a second close finds no master");
        let list = open.list();
        assert_eq!((list.master, list.stabled.len()), (0, 1));
        assert_eq!(list.current.pet_number, 100);
        open.clear();
        assert_eq!(open.list(), StableList::default());
    }

    /// The asks on one entry come back in the order they were made, the others stay.
    #[test]
    fn the_asks_answer_by_entry_in_order() {
        let mut open = StableOpen::default();
        open.asks.extend([
            (5, StableCallback::Update),
            (6, StableCallback::Update),
            (5, StableCallback::Paperdoll),
        ]);
        assert_eq!(
            open.take_asks(5),
            vec![StableCallback::Update, StableCallback::Paperdoll]
        );
        assert_eq!(open.take_asks(5), vec![]);
        assert_eq!(open.take_asks(6), vec![StableCallback::Update]);
    }
}
