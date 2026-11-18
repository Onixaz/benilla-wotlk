//! The player's four totem slots (12340 `0xbd0b00`): `SMSG_TOTEM_CREATED` stores a slot, the
//! totem unit's create and its record answers name it and fire `PLAYER_TOTEM_UPDATE`, and its
//! disable destroys the slot, each in its handler; the totem verbs read and write the table through
//! the live view when called.

use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_formats::TotemCategoryCatalog;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{ScriptValue, TotemView, MAX_TOTEM_SLOTS};

use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, GuidIndex, NetCommands, NetHandlerApp, ObjectStore, SelfPlayer};
use crate::ui_action::Spells;
use crate::ui_items::{tool_categories, InventoryScope};
use crate::ui_script::live::UiEvents;

/// Each slot's tool row, as `GetTotemInfo` asks for it (`0x51d3c9`): fire, earth, water, air.
const SLOT_TOOLS: [u32; MAX_TOTEM_SLOTS] = [4, 2, 5, 3];

/// One live slot.
#[derive(Clone, Debug, PartialEq)]
struct Totem {
    guid: u64,
    duration_ms: u32,
    /// When the packet was handled, on the `GetTime` clock in milliseconds (`+0x18`).
    start_ms: u32,
    icon: Option<String>,
    /// The unit's name; a re-summon keeps the old one until the new unit streams (`+0x10`).
    name: Option<String>,
}

/// The four slots, written by the handlers and the destroy alone.
#[derive(Resource, Default)]
pub(crate) struct Totems {
    slots: [Option<Totem>; MAX_TOTEM_SLOTS],
}

impl Totems {
    /// The slot holding `guid` (`0x5139b0`); a zero guid holds none.
    fn slot_of(&self, guid: u64) -> Option<usize> {
        (guid != 0)
            .then(|| {
                self.slots
                    .iter()
                    .position(|s| s.as_ref().is_some_and(|t| t.guid == guid))
            })
            .flatten()
    }

    /// The destroy (`0x5197d0`): `CMSG_TOTEM_DESTROYED`, then the slot cleared; false for an empty
    /// slot, which sends nothing. The caller fires `PLAYER_TOTEM_UPDATE` (`0x51986a`).
    fn destroy(&mut self, slot: usize, commands: Option<&NetCommands>) -> bool {
        let Some(cell) = self.slots.get_mut(slot).filter(|c| c.is_some()) else {
            return false;
        };
        if let Some(commands) = commands {
            let _ = commands
                .0
                .send(ClientCommand::TotemDestroyed { slot: slot as u8 });
        }
        *cell = None;
        true
    }
}

/// `TotemCategory.dbc`; empty without client data.
#[derive(Resource, Default)]
pub(crate) struct TotemCategories(pub(crate) TotemCategoryCatalog);

pub(crate) struct UiTotemPlugin;

impl Plugin for UiTotemPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Totems>()
            .init_resource::<TotemCategories>()
            .net_handler(SessionEventKind::TotemCreated, on_totem_created)
            .net_handler(SessionEventKind::ObjectCreate, on_unit_create)
            .net_handler(SessionEventKind::CreatureName, on_record_answer)
            .net_handler(SessionEventKind::PlayerName, on_record_answer)
            .add_systems(Startup, load_totem_categories.after(AssetSet::Open));
    }
}

fn load_totem_categories(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_totem_categories(&mut chain)
    };
    match loaded {
        Ok(catalog) => commands.insert_resource(TotemCategories(catalog)),
        Err(e) => warn!("ui_totem: TotemCategory.dbc failed — no slot reports its tool: {e:#}"),
    }
}

/// `SMSG_TOTEM_CREATED` (`0x526530` arm `0x413`): stores the slot's guid, duration, start and
/// icon, keeping its name, and fires nothing; a zero guid is ignored.
fn on_totem_created(
    In(ev): In<SessionEvent>,
    mut totems: ResMut<Totems>,
    spells: Option<Res<Spells>>,
    clock: Res<crate::ui_script::UiClock>,
) {
    let SessionEvent::TotemCreated(t) = ev else {
        return;
    };
    debug!(
        "ui_totem: slot {} <- guid {:#x} spell {} for {} ms",
        t.slot, t.guid, t.spell_id, t.duration_ms
    );
    let Some(cell) = totems.slots.get_mut(usize::from(t.slot)) else {
        return;
    };
    if t.guid == 0 {
        return;
    }
    let icon = spells
        .as_deref()
        .and_then(|s| s.catalog.get(t.spell_id))
        .and_then(|d| d.icon.clone());
    let name = cell.as_ref().and_then(|old| old.name.clone());
    *cell = Some(Totem {
        guid: t.guid,
        duration_ms: t.duration_ms,
        start_ms: (clock.ui_now * 1000.0) as u32,
        icon,
        name,
    });
}

/// Name the slot `guid` holds from its unit and fire `PLAYER_TOTEM_UPDATE`; nothing for a guid no
/// slot holds.
fn name_slot(
    guid: u64,
    totems: &mut Totems,
    names: &NameCache,
    store: Option<&ObjectStore>,
    commands: &NetCommands,
    ui: &mut UiEvents,
) {
    let Some(slot) = totems.slot_of(guid) else {
        return;
    };
    let name = names.resolve_unit(guid, store, commands).map(str::to_owned);
    if let Some(t) = totems.slots[slot].as_mut() {
        t.name = name;
    }
    ui.fire(
        "PLAYER_TOTEM_UPDATE",
        vec![ScriptValue::Int(slot as i64 + 1)],
    );
}

/// A unit's create: its post-init names the slot holding it and fires `PLAYER_TOTEM_UPDATE`
/// (`0x740140`), once the create has spawned it.
fn on_unit_create(
    In(ev): In<SessionEvent>,
    mut totems: ResMut<Totems>,
    index: Res<GuidIndex>,
    units: Query<&ObjectStore>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut ui: ResMut<UiEvents>,
) {
    let SessionEvent::ObjectCreate { guid, .. } = ev else {
        return;
    };
    let store = index.0.get(&guid).and_then(|&e| units.get(e).ok());
    name_slot(guid, &mut totems, &names, store, &commands, &mut ui);
}

/// A name or creature record answered: each held unit's callback re-names the slot holding it
/// and fires `PLAYER_TOTEM_UPDATE` (`0x728ce3`, `0x72ce73`).
fn on_record_answer(
    In(ev): In<SessionEvent>,
    mut totems: ResMut<Totems>,
    index: Res<GuidIndex>,
    units: Query<&ObjectStore>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut ui: ResMut<UiEvents>,
) {
    if !matches!(
        ev,
        SessionEvent::CreatureName { .. } | SessionEvent::PlayerName { .. }
    ) {
        return;
    }
    for slot in 0..MAX_TOTEM_SLOTS {
        let Some(t) = totems.slots[slot].as_ref() else {
            continue;
        };
        let guid = t.guid;
        let Some(store) = index.0.get(&guid).and_then(|&e| units.get(e).ok()) else {
            continue;
        };
        // A callback fires only for the record its unit waited on, which this answer settled.
        let name = names.resolve_unit(guid, Some(store), &commands);
        if name.is_none() || name == totems.slots[slot].as_ref().and_then(|t| t.name.as_deref()) {
            continue;
        }
        name_slot(guid, &mut totems, &names, Some(store), &commands, &mut ui);
    }
}

/// A unit's entry watch (`0x72ceb0`): with its new creature record cached, the slot holding it is
/// renamed and fires `PLAYER_TOTEM_UPDATE` (`0x72cf2b`); a miss asks, and the answer renames it.
pub(crate) fn entry_changed(world: &mut World, guid: u64) {
    if let Err(e) = world.run_system_cached_with(entry_rename, guid) {
        warn!("ui_totem: the entry watch did not run: {e}");
    }
}

fn entry_rename(
    In(guid): In<u64>,
    mut totems: ResMut<Totems>,
    index: Res<GuidIndex>,
    units: Query<&ObjectStore>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut ui: ResMut<UiEvents>,
) {
    if totems.slot_of(guid).is_none() {
        return;
    }
    let store = index.0.get(&guid).and_then(|&e| units.get(e).ok());
    if names.resolve_unit(guid, store, &commands).is_some() {
        name_slot(guid, &mut totems, &names, store, &commands, &mut ui);
    }
}

/// A unit's disable destroys the slot holding it (`0x73574a` -> `0x5197d0`): sent, cleared and
/// announced in the turn of the handler that removed it, on DESTROY, OUT_OF_RANGE and the world
/// leave alike.
pub(crate) fn unit_disabled(world: &mut World, guid: u64) {
    if !world.contains_resource::<Totems>() {
        return;
    }
    let slot = world.resource_scope(|world, mut totems: Mut<Totems>| {
        let slot = totems.slot_of(guid)?;
        totems.destroy(slot, world.get_resource::<NetCommands>());
        Some(slot)
    });
    let Some(slot) = slot else {
        return;
    };
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        ui.fire(
            "PLAYER_TOTEM_UPDATE",
            vec![ScriptValue::Int(slot as i64 + 1)],
        );
    }
}

/// What the totem verbs read and write when called, part of the VM's live view.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct TotemLive<'w, 's> {
    /// Cleared inside `DestroyTotem` through the lent view.
    totems: Option<ResMut<'w, Totems>>,
    categories: Option<Res<'w, TotemCategories>>,
    me: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    objects: Option<crate::net::Objects<'w, 's>>,
    items: Option<Res<'w, Items>>,
    commands: Option<Res<'w, NetCommands>>,
    /// The totem bar's element lists, which the learn and unlearn handlers file.
    lists: Option<Res<'w, crate::spell::multicast::MultiCastLists>>,
    spells: Option<Res<'w, Spells>>,
}

impl TotemLive<'_, '_> {
    pub(crate) fn slot(&self, slot: usize) -> Option<TotemView> {
        let t = self.totems.as_ref()?.slots.get(slot)?.as_ref()?;
        Some(TotemView {
            name: t.name.clone(),
            start: f64::from(t.start_ms) / 1000.0,
            duration_ms: t.duration_ms,
            icon: t.icon.clone(),
        })
    }

    /// The bag search for the slot's tool row with flag 8, the player held (`0x51d42f`, `0x51d457`-`0x51d496`).
    pub(crate) fn tool(&self, slot: usize) -> bool {
        let (Some(me), Some(objects), Some(categories), Some(items), Some(commands)) = (
            self.me.iter().next(),
            self.objects.as_ref(),
            self.categories.as_deref(),
            self.items.as_deref(),
            self.commands.as_deref(),
        ) else {
            return false;
        };
        let Some(&row) = SLOT_TOOLS.get(slot) else {
            return false;
        };
        if categories.0.is_empty() {
            return false;
        }
        let carried = tool_categories(&me.0, objects, items, commands, InventoryScope::TOOLS);
        categories.0.covered_by(row, carried.iter().copied())
    }

    /// The element list `element` (`0xbe8ea0`).
    pub(crate) fn list(&self, element: usize) -> Vec<u32> {
        self.lists
            .as_ref()
            .and_then(|l| l.lists.get(element).cloned())
            .unwrap_or_default()
    }

    /// The spell record's `TotemCategory` pair (`0x5a7bc0`).
    pub(crate) fn spell_categories(&self, spell: u32) -> Option<[u32; 2]> {
        Some(self.spells.as_ref()?.catalog.get(spell)?.totem_categories)
    }

    /// The `TotemCategory.dbc` row's name.
    pub(crate) fn category_name(&self, row: u32) -> Option<String> {
        Some(self.categories.as_ref()?.0.get(row)?.name.clone())
    }

    pub(crate) fn destroy(&mut self, slot: usize) -> bool {
        let commands = self.commands.as_deref();
        self.totems
            .as_mut()
            .is_some_and(|t| t.destroy(slot, commands))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOTEM: u64 = 0xF130_0000_0000_0050;

    /// A world with slot 1 holding `TOTEM`, its unit held with `entry`, and `entry`'s record cached
    /// as `name` unless `None`.
    fn with_slot(entry: u32, name: Option<&str>) -> World {
        let mut world = World::new();
        let unit = world
            .spawn(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (benilla_protocol::field::FIELD_OBJECT_ENTRY, entry),
            ])))
            .id();
        let mut index = GuidIndex::default();
        index.0.insert(TOTEM, unit);
        world.insert_resource(index);
        let mut names = NameCache::default();
        if let Some(name) = name {
            names.insert_creature(
                entry,
                Some(crate::names::CreatureRecord {
                    name: name.into(),
                    ..Default::default()
                }),
            );
        }
        world.insert_resource(names);
        world.insert_resource(NetCommands(crossbeam_channel::unbounded().0));
        world.init_resource::<UiEvents>();
        let mut totems = Totems::default();
        totems.slots[1] = Some(Totem {
            guid: TOTEM,
            duration_ms: 60_000,
            start_ms: 0,
            icon: None,
            name: None,
        });
        world.insert_resource(totems);
        world
    }

    /// The unit's entry watch renames the slot it holds and fires `PLAYER_TOTEM_UPDATE` when the new
    /// record is cached (`0x72cf2b`); a miss asks and fires nothing, the answer renaming it later.
    #[test]
    fn an_entry_change_renames_the_slot_when_the_record_is_cached() {
        let mut world = with_slot(5925, Some("Earthbind Totem"));
        entry_changed(&mut world, TOTEM);
        assert_eq!(
            world.resource::<UiEvents>().names(),
            ["PLAYER_TOTEM_UPDATE"]
        );
        let name = world.resource::<Totems>().slots[1]
            .as_ref()
            .and_then(|t| t.name.clone());
        assert_eq!(name.as_deref(), Some("Earthbind Totem"));

        let mut world = with_slot(5925, None);
        entry_changed(&mut world, TOTEM);
        assert!(world.resource::<UiEvents>().is_empty());
    }
}
