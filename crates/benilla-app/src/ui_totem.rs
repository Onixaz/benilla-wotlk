//! The player's four totem slots (12340 `0xbd0b00`) and their tools, fed to the totem verbs.

use std::time::Instant;

use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_formats::TotemCategoryCatalog;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{
    MultiCastView, ScriptValue, TotemSlots, TotemView, UiScript, MAX_TOTEM_SLOTS,
};

use crate::items::Items;
use crate::names::NameCache;
use crate::net::{
    ClientCommand, EnteredWorldMessage, GuidIndex, NetCommands, NetHandlerApp, ObjectStore,
    Objects, SelfPlayer,
};
use crate::ui_action::Spells;
use crate::ui_items::{tool_categories, InventoryScope};
use crate::ui_unit::UnitFeed;

/// Each slot's tool row, as `GetTotemInfo` asks for it (`0x51d3c9`): fire, earth, water, air.
const SLOT_TOOLS: [u32; MAX_TOTEM_SLOTS] = [4, 2, 5, 3];

/// One live slot.
#[derive(Clone, Debug, PartialEq)]
struct Totem {
    guid: u64,
    duration_ms: u32,
    /// When the packet arrived; the feed turns it into `GetTime` seconds once.
    arrived: Instant,
    start: Option<f64>,
    icon: Option<String>,
    /// The unit's name; a re-summon keeps the old one until the new unit streams (`+0x10`).
    name: Option<String>,
    /// The unit was seen streamed; its sighting announced the slot.
    sighted: bool,
}

/// The four slots.
#[derive(Resource, Default)]
pub(crate) struct Totems {
    slots: [Option<Totem>; MAX_TOTEM_SLOTS],
    /// Slots whose `PLAYER_TOTEM_UPDATE` waits for the feed, 0-based.
    announce: Vec<usize>,
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
            .add_systems(Startup, load_totem_categories.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    clear_at_session_start.after(benilla_world::schedule::WorldStage::Net),
                    feed_totems
                        .in_set(UnitFeed)
                        .after(clear_at_session_start)
                        .before(crate::ui_unit::EnterWorldFeed),
                    // Before `PLAYER_ENTERING_WORLD`, whose stock handler builds the bar.
                    feed_multicast
                        .in_set(UnitFeed)
                        .before(crate::ui_unit::EnterWorldFeed),
                ),
            );
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

/// `SMSG_TOTEM_CREATED` (`0x526530` arm `0x413`): stores the slot, ignoring a zero guid.
fn on_totem_created(
    In(ev): In<SessionEvent>,
    mut totems: ResMut<Totems>,
    spells: Option<Res<Spells>>,
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
        arrived: Instant::now(),
        start: None,
        icon,
        name,
        sighted: false,
    });
}

/// The session's first world entry empties the table (`0x5280dd`); a later worldport keeps it.
fn clear_at_session_start(
    mut entered: MessageReader<EnteredWorldMessage>,
    mut totems: ResMut<Totems>,
) {
    if entered.read().count() > 0 {
        *totems = Totems::default();
    }
}

/// The view, the sightings and the destroys.
#[allow(clippy::too_many_arguments)] // a Bevy system's full input set
fn feed_totems(
    script: Option<NonSendMut<UiScript>>,
    mut totems: ResMut<Totems>,
    categories: Res<TotemCategories>,
    index: Res<GuidIndex>,
    units: Query<&ObjectStore>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    objects: Objects,
    items: Res<Items>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    clock: Res<crate::ui_script::UiClock>,
    mut pushed: Local<crate::ui_script::VmMemo<Option<TotemSlots>>>,
) {
    let Some(mut script) = script else {
        return;
    };

    // `DestroyTotem` cleared and announced the slot in the VM; send it and forget it here.
    for slot in script.take_totem_destroys() {
        let _ = commands.0.send(ClientCommand::TotemDestroyed { slot });
        if let Some(cell) = totems.slots.get_mut(usize::from(slot)) {
            *cell = None;
        }
    }

    // The unit announces its slot on streaming (`0x73fcc0`) and on its name (`0x72cde0`).
    let Totems { slots, announce } = &mut *totems;
    for (i, totem) in slots.iter_mut().enumerate() {
        let Some(totem) = totem else { continue };
        if totem.start.is_none() {
            let (anchor, ui_now) = (clock.anchor, clock.ui_now);
            totem.start = Some(match totem.arrived.checked_duration_since(anchor) {
                Some(ahead) => ui_now + ahead.as_secs_f64(),
                None => ui_now - anchor.duration_since(totem.arrived).as_secs_f64(),
            });
        }
        let Some(store) = index.0.get(&totem.guid).and_then(|&e| units.get(e).ok()) else {
            continue;
        };
        let name = names
            .resolve_unit(totem.guid, Some(store), &commands)
            .map(str::to_owned);
        if !totem.sighted || (name.is_some() && name != totem.name) {
            totem.sighted = true;
            if name.is_some() {
                totem.name = name;
            }
            announce.push(i);
        }
    }

    let tools = match self_q.iter().next() {
        Some(me) if !categories.0.is_empty() => {
            let carried =
                tool_categories(&me.0, &objects, &items, &commands, InventoryScope::TOOLS);
            SLOT_TOOLS.map(|row| categories.0.covered_by(row, carried.iter().copied()))
        }
        _ => [false; MAX_TOTEM_SLOTS],
    };
    let view = TotemSlots {
        live: std::array::from_fn(|i| {
            totems.slots[i].as_ref().map(|t| TotemView {
                name: t.name.clone(),
                start: t.start.unwrap_or(clock.ui_now),
                duration_ms: t.duration_ms,
                icon: t.icon.clone(),
            })
        }),
        tools,
    };
    let last = pushed.get(&script);
    if last.as_ref() != Some(&view) {
        script.set_totems(view.clone());
        *last = Some(view);
    }
    for slot in totems.announce.drain(..) {
        script.fire_event(
            "PLAYER_TOTEM_UPDATE",
            vec![ScriptValue::Int(slot as i64 + 1)],
        );
    }
}

/// What the bar feed last pushed, and the spell table it built once per catalog.
#[derive(Default)]
struct MultiCastMemo {
    pushed: Option<MultiCastView>,
    spell_categories: Option<std::collections::HashMap<u32, [u32; 2]>>,
}

/// The element rows `SetMultiCastSpell` and `PlaceAction` test a totem slot against.
const ELEMENT_ROWS: [u32; 5] = [2, 3, 4, 5, 21];

/// The totem bar's lists and tables, and `UPDATE_MULTI_CAST_ACTIONBAR` after a refill.
fn feed_multicast(
    script: Option<NonSendMut<UiScript>>,
    mut lists: ResMut<crate::spell::multicast::MultiCastLists>,
    spells: Option<Res<Spells>>,
    categories: Res<TotemCategories>,
    mut memo: Local<crate::ui_script::VmMemo<MultiCastMemo>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let memo = memo.get(&script);
    let spell_categories = memo
        .spell_categories
        .get_or_insert_with(|| {
            spells.as_deref().map_or_else(Default::default, |s| {
                s.catalog
                    .iter()
                    .filter(|(_, d)| d.totem_categories.iter().any(|c| ELEMENT_ROWS.contains(c)))
                    .map(|(id, d)| (id, d.totem_categories))
                    .collect()
            })
        })
        .clone();
    let view = MultiCastView {
        lists: lists.lists.clone(),
        spell_categories,
        category_names: [2, 3, 4, 5]
            .into_iter()
            .filter_map(|row| Some((row, categories.0.get(row)?.name.clone())))
            .collect(),
    };
    if memo.pushed.as_ref() != Some(&view) {
        script.set_multicast(view.clone());
        memo.pushed = Some(view);
    }
    if std::mem::take(&mut lists.announce) {
        script.fire_event("UPDATE_MULTI_CAST_ACTIONBAR", vec![]);
    }
}
