//! The reputation pane's client side: the faction list (`benilla_ui::script::FactionList`) the
//! bindings read and write at the call, the faction table they read it against, and 3.3.5a's four
//! reputation handlers (`0x5d2e30`, `0x5d20a0`, `0x5d2050`, `0x5d0850`), which write the store and
//! the list and raise `UPDATE_FACTION` and their chat lines at their steps. The forced reactions'
//! handler is the bridge's.
//!
//! Every faction with a reputation slot is a record, met or not: the list names its headers from
//! the table (`0x5d01f0`), and its world-entry build keeps the factions whose race/class slot fits
//! the player (`0x5d2bc0`). Flag `0x08` is `HEADER`, not "force invisible".
//!
//! The wire standing excludes the race/class base, so the pane's total is the base the world
//! entry stores in the slot plus the wire (`0x5d05b0`). The rank is [`benilla_formats::reputation_rank`], shared with the unit-reaction
//! decode; the pane shows it uncapped.

use bevy::prelude::*;

use benilla_formats::{FactionCatalog, FactionInfo};
use benilla_protocol::{ObjectFields, SessionEvent, SessionEventKind};
use benilla_ui::script::{FactionEntry, FactionFlag, FactionList, FactionTable, ReputationSend};

use crate::char_select::ClientState;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp, ObjectStore, Reputations};
use crate::target::Factions;

/// Rank `r` spans `RANK_BOUNDS[r]..RANK_BOUNDS[r + 1]`, the client's table at `0xa2d2fc`, the edges
/// [`benilla_formats::reputation_rank`] ranks by. The Lua `standingID` is the rank plus one.
const RANK_BOUNDS: [i32; 9] = [-42000, -6000, -3000, 0, 3000, 9000, 21000, 42000, 43000];

/// The stored flag `0x40` whose rows always print their change (`0x5d211d`).
const FACTION_FLAG_ALWAYS_LOGGED: u8 = 0x40;
/// The stored flag `0x80` that lets a header print its rank change (`0x5d2236`).
const FACTION_FLAG_RANK_SHOWN: u8 = 0x80;

/// The client's faction list (`0xc23370`-`0xc2347c`), which the world entry builds, the handlers
/// grow and sort, and the bindings fold, select and sort.
#[derive(Resource, Default)]
pub(crate) struct ReputationList(pub(crate) FactionList);

/// The reputation store's two other columns per slot (`0xc22b70`, 16 bytes a slot): the faction
/// id (`+0x0`) and the race/class base (`+0x8`). Only the list's world entry writes them
/// (`0x5d2d1f`, `0x5d2d6e`) and the game shutdown zeroes them (`0x5d15ba`); every reader takes
/// them from here, so before the first world entry a slot names faction 0 with base 0.
#[derive(Resource)]
pub(crate) struct FactionSlots(
    pub(crate) [(u32, i32); benilla_protocol::messages::FACTION_LIST_LEN],
);

impl Default for FactionSlots {
    fn default() -> Self {
        Self([(0, 0); benilla_protocol::messages::FACTION_LIST_LEN])
    }
}

impl FactionSlots {
    /// The faction a slot names, 0 past the store (`0x5d2155`).
    fn faction(&self, slot: usize) -> u32 {
        self.0.get(slot).map_or(0, |s| s.0)
    }

    /// The base a slot carries, 0 past the store.
    fn base(&self, slot: usize) -> i32 {
        self.0.get(slot).map_or(0, |s| s.1)
    }
}

/// The reputation handlers and the list's game-shutdown clear.
pub(crate) struct UiReputationPlugin;

impl Plugin for UiReputationPlugin {
    fn build(&self, app: &mut App) {
        use SessionEventKind as K;
        app.init_resource::<ReputationList>()
            .init_resource::<FactionSlots>()
            // After the UI shutdown's `PLAYER_LEAVING_WORLD` and `PLAYER_LOGOUT`, which still read
            // the list (`0x529160`: `0x528f00`, then `0x5d14f0` at `0x5291b3`).
            .add_systems(
                OnExit(ClientState::InWorld),
                shutdown.after(crate::ui_script::end_ui_session),
            )
            .net_handler(K::Reputations, on_initialize_factions)
            .net_handler(K::ReputationDelta, on_standing)
            .net_handler(K::ReputationVisible, on_visible)
            .net_handler(K::ReputationAtWar, on_at_war);
    }
}

/// The game shutdown's clear (`0x5d14f0`): no rows, no headers, no selection (`0x5d14f9`), then
/// every slot's faction, flags, base and standing zeroed (`0x5d15ba`).
fn shutdown(
    mut list: ResMut<ReputationList>,
    mut slots: ResMut<FactionSlots>,
    store: Option<ResMut<Reputations>>,
) {
    *list = ReputationList::default();
    *slots = FactionSlots::default();
    if let Some(mut store) = store {
        store.0.fill((0, 0));
    }
}

/// One faction's record with its slot as it stands, or `None` without a reputation slot or a
/// name; an unmet faction is still a record, with `visible: false`. `base` is the slot's stored
/// base (`0x5d05b0`); `player` the active player's race and class, `None` without one, where the
/// has-reputation test answers `false` (`0x5d0d05`).
pub(crate) fn reputation_row(
    faction_id: u32,
    info: &FactionInfo,
    catalog: &FactionCatalog,
    flags: u8,
    wire_standing: i32,
    base: i32,
    player: Option<(u8, u8)>,
) -> Option<FactionEntry> {
    use benilla_formats::faction_flags as flag;
    let standing = base + wire_standing;
    let rank = benilla_formats::reputation_rank(standing);
    Some(FactionEntry {
        faction_id,
        rep_list_id: u32::try_from(info.rep_index).ok()?,
        parent_id: info.team,
        name: catalog.faction_name(faction_id)?.to_string(),
        description: catalog
            .faction_description(faction_id)
            .unwrap_or_default()
            .to_string(),
        standing,
        standing_id: rank + 1,
        bar_min: RANK_BOUNDS[rank as usize],
        bar_max: RANK_BOUNDS[rank as usize + 1],
        fits_player: player.is_some_and(|(r, c)| info.slot_for(r, c).is_some()),
        visible: flags & flag::VISIBLE != 0,
        is_header: flags & flag::HEADER != 0,
        at_war: flags & flag::AT_WAR != 0,
        peace_forced: flags & flag::PEACE_FORCED != 0,
        inactive: flags & flag::INACTIVE != 0,
        has_rep: player.is_some_and(|(r, c)| info.header_has_rep_for(r, c)),
    })
}

/// A wire `repListId` as a store index, or `None` (logged) past the `FACTION_LIST_LEN` array
/// (128 in 12340); unchecked, `0xFFFF_FFFF` would resize the store to 34 GB.
fn reputation_slot(list_id: u32, opcode: &str) -> Option<usize> {
    let i = usize::try_from(list_id).ok()?;
    if i >= benilla_protocol::messages::FACTION_LIST_LEN {
        warn!(
            "net: {opcode} names reputation slot {list_id}, past the {}-entry faction list, dropped",
            benilla_protocol::messages::FACTION_LIST_LEN
        );
        return None;
    }
    Some(i)
}

/// The faction table at the call: `Faction.dbc`, the player's store (`0xc22b70`), the active
/// player's descriptor and the wire.
pub(crate) struct AppFactions<'a> {
    pub(crate) store: &'a mut Reputations,
    /// The store's faction and base columns.
    pub(crate) slots: &'a FactionSlots,
    pub(crate) catalog: Option<&'a FactionCatalog>,
    /// The active player's descriptor, `None` before our create.
    pub(crate) player: Option<&'a ObjectFields>,
    pub(crate) net: Option<&'a NetCommands>,
    /// A flag was written; the store reads as changed, as the at-war write re-reads reactions (`0x5d0b68`).
    pub(crate) wrote: bool,
    /// The at-war set sent its packet, after which it refreshes the player's reaction (`0x5d0b68`).
    pub(crate) reacted: bool,
}

impl AppFactions<'_> {
    fn race_class(&self) -> Option<(u8, u8)> {
        self.player
            .map(|p| (p.unit_race().unwrap_or(0), p.unit_class().unwrap_or(0)))
    }

    /// A record read against its slot's stored flags, standing and base (`0x5d05b0`: `0x5d05ea`).
    fn read_record(&self, faction_id: u32, info: &FactionInfo) -> Option<FactionEntry> {
        let slot = usize::try_from(info.rep_index).ok();
        let (flags, wire) = slot
            .and_then(|i| self.store.0.get(i))
            .copied()
            .unwrap_or((0, 0));
        let base = slot.map_or(0, |i| self.slots.base(i));
        reputation_row(
            faction_id,
            info,
            self.catalog?,
            flags,
            wire,
            base,
            self.race_class(),
        )
    }

    /// A record without a reputation slot as the faction-info helper reads it (`0x5d0da0`): its
    /// name and description, Neutral at 0 (`0x5d062d`, `0x5d05dd`), never at war or peace-forced
    /// (`0x5d04dd`, `0x5d052d`), and slot `0xFFFFFFFF` for the watched test (`0x5d0c9d`).
    fn slotless(&self, faction_id: u32, info: &FactionInfo) -> Option<FactionEntry> {
        let catalog = self.catalog?;
        Some(FactionEntry {
            faction_id,
            rep_list_id: u32::MAX,
            parent_id: info.team,
            name: catalog.faction_name_or_empty(faction_id)?.to_string(),
            description: catalog
                .faction_description(faction_id)
                .unwrap_or_default()
                .to_string(),
            standing: 0,
            standing_id: 4,
            bar_min: RANK_BOUNDS[3],
            bar_max: RANK_BOUNDS[4],
            fits_player: false,
            visible: false,
            is_header: false,
            at_war: false,
            peace_forced: false,
            inactive: false,
            has_rep: false,
        })
    }

    /// The faction the store names at a slot (`0x5d2155`), with its name and the stored base its
    /// rank reads (`0x5d0600`: `0x5d063a`); `None` for faction 0 or one without a record.
    fn slot_base(&self, list_id: u32) -> Option<(String, i32)> {
        let catalog = self.catalog?;
        let id = self.slots.faction(usize::try_from(list_id).ok()?);
        let f = catalog.reputation_faction(id)?;
        Some((
            catalog.faction_name(id).unwrap_or_default().to_string(),
            self.slots.base(usize::try_from(f.rep_index).ok()?),
        ))
    }
}

impl FactionTable for AppFactions<'_> {
    fn faction(&self, faction_id: i64) -> Option<FactionEntry> {
        let id = u32::try_from(faction_id).ok()?;
        self.read_record(id, self.catalog?.reputation_faction(id)?)
    }

    fn record(&self, faction_id: i64) -> Option<FactionEntry> {
        let id = u32::try_from(faction_id).ok()?;
        let info = self.catalog?.faction_row(id)?;
        if info.rep_index >= 0 {
            self.read_record(id, info)
        } else {
            self.slotless(id, info)
        }
    }

    /// The store's faction id at the slot (`0x5d0913`, `0x5d1285`), read as [`Self::faction`].
    fn slot_faction(&self, rep_list_id: u32) -> Option<FactionEntry> {
        let id = self.slots.faction(usize::try_from(rep_list_id).ok()?);
        self.read_record(id, self.catalog?.reputation_faction(id)?)
    }

    fn met_slots(&self) -> Vec<u32> {
        let Some(catalog) = self.catalog else {
            return Vec::new();
        };
        let Some((race, class)) = self.race_class() else {
            return Vec::new();
        };
        let mut met: Vec<(u32, u32)> = catalog
            .reputation_factions()
            .filter(|(_, f)| f.slot_for(race, class).is_some())
            .filter_map(|(id, f)| {
                let slot = usize::try_from(f.rep_index).ok()?;
                let (flags, _) = self.store.0.get(slot).copied()?;
                (flags & benilla_formats::faction_flags::VISIBLE != 0)
                    .then_some((id, u32::try_from(slot).ok()?))
            })
            .collect();
        met.sort_by_key(|&(id, _)| std::cmp::Reverse(id));
        met.into_iter().map(|(_, slot)| slot).collect()
    }

    fn watched(&self) -> Option<i32> {
        Some(self.player?.player_watched_faction().unwrap_or(0))
    }

    fn in_combat(&self) -> bool {
        self.player
            .is_some_and(|p| p.unit_flags() & crate::player::UNIT_FLAG_IN_COMBAT != 0)
    }

    fn write_flag(&mut self, rep_list_id: u32, flag: FactionFlag, set: bool) {
        use benilla_formats::faction_flags as f;
        let Ok(i) = usize::try_from(rep_list_id) else {
            return;
        };
        if self.store.0.len() <= i {
            self.store.0.resize(i + 1, (0, 0));
        }
        let bit = match flag {
            FactionFlag::AtWar => f::AT_WAR,
            FactionFlag::Inactive => f::INACTIVE,
        };
        self.wrote = true;
        let flags = &mut self.store.0[i].0;
        if set {
            *flags |= bit;
        } else {
            *flags &= !bit;
        }
    }

    fn send(&mut self, send: ReputationSend) {
        let cmd = match send {
            ReputationSend::AtWar {
                rep_list_id,
                at_war,
            } => {
                self.reacted = true;
                ClientCommand::SetFactionAtWar {
                    rep_list_id,
                    at_war,
                }
            }
            ReputationSend::Inactive {
                rep_list_id,
                inactive,
            } => ClientCommand::SetFactionInactive {
                rep_list_id,
                inactive,
            },
            ReputationSend::Watch(rep_list_id) => ClientCommand::SetWatchedFaction { rep_list_id },
        };
        if let Some(net) = self.net {
            let _ = net.0.send(cmd);
        }
    }
}

/// The active player's descriptor in `world`.
fn active_player(world: &World) -> Option<&ObjectFields> {
    let guid = world.get_resource::<crate::net::SelfGuid>()?.0?;
    let entity = *world
        .get_resource::<crate::net::GuidIndex>()?
        .0
        .get(&guid)?;
    world.get::<ObjectStore>(entity).map(|s| &s.0)
}

/// `f` over the world's list and table, `None` without the store.
fn with_table<R>(
    world: &mut World,
    f: impl FnOnce(&mut FactionList, &mut AppFactions) -> R,
) -> Option<R> {
    let (out, reacted) = with_parts(world, |list, store, slots, world| {
        // The read verbs write nothing (`0x5cff20`, `0x5d0da0`, `0x5d1240`): only a written flag
        // marks the store changed for the reaction readers.
        let mut table = AppFactions {
            store: store.bypass_change_detection(),
            slots,
            catalog: world.get_resource::<Factions>().map(|f| f.catalog()),
            player: active_player(world),
            net: world.get_resource::<NetCommands>(),
            wrote: false,
            reacted: false,
        };
        let out = f(list, &mut table);
        let reacted = table.reacted && table.player.is_some();
        if table.wrote {
            store.set_changed();
        }
        (out, reacted)
    })?;
    // The at-war set's reaction refresh with the player held (`0x5d0b60` -> `0x5d0b68`).
    if reacted {
        reask_quest_status(world);
    }
    Some(out)
}

/// `f` over the list, the store, the slot columns and the rest of the `World`, `None` without the
/// store.
fn with_parts<R>(
    world: &mut World,
    f: impl FnOnce(&mut FactionList, &mut Mut<Reputations>, &mut FactionSlots, &World) -> R,
) -> Option<R> {
    if !world.contains_resource::<Reputations>() {
        return None;
    }
    world.init_resource::<ReputationList>();
    world.init_resource::<FactionSlots>();
    Some(
        world.resource_scope(|world, mut list: Mut<ReputationList>| {
            world.resource_scope(|world, mut store: Mut<Reputations>| {
                world.resource_scope(|world, mut slots: Mut<FactionSlots>| {
                    f(&mut list.0, &mut store, &mut slots, world)
                })
            })
        }),
    )
}

/// The player's reaction refresh re-asking every questgiver's status (`0x71f94b`).
fn reask_quest_status(world: &mut World) {
    if let Some(mut quest) = world.get_resource_mut::<crate::ui_quest::QuestStatuses>() {
        quest.bump_reask();
    }
}

/// The bindings' access over the lent `World`: the list and the store straight off the `World`,
/// apart from the live view, whose other readers hold the store.
pub(crate) fn live_access(f: &mut dyn FnMut(&mut FactionList, &mut dyn FactionTable)) -> bool {
    crate::ui_script::safe_world::with_world(|w| with_table(w, |list, t| f(list, t)))
        .flatten()
        .is_some()
}

/// `UPDATE_FACTION` raised at its step, with the `World` lent.
fn update_faction(world: &mut World) {
    crate::ui_script::live::with_live_world(world, |s| s.fire_event("UPDATE_FACTION", vec![]));
}

/// The faction sort (`0x5d1880`) and the `UPDATE_FACTION` it ends with (`0x5d1bfa`).
fn sort(world: &mut World) {
    if with_table(world, |list, t| list.sort(t)).is_some() {
        update_faction(world);
    }
}

/// The list's world entry (`0x5d2bc0`), which the UI's (`0x528208`) and the initialize handler
/// (`0x5d2e84`) run: nothing without the active player (`0x5d2be6`); else the walk, the "Inactive"
/// header's fold (`0x5d2e0f`) and the sort (`0x5d2e17`), each sort raising `UPDATE_FACTION`
/// (`0x5d1bfa`). The walk reaches no Lua: its adds pass no announce (`0x5d2d86`).
pub(crate) fn enter_world(world: &mut World) {
    let Some(row) = with_parts(world, walk).flatten() else {
        return;
    };
    if with_table(world, |list, t| list.collapse(t, row, true)) == Some(true) {
        update_faction(world);
    }
    sort(world);
}

/// The world entry's walk (`0x5d2c48`-`0x5d2da2`), `None` without the active player: the list
/// cleared, then from the table's end every record with a slot below 128 writes its id to the slot
/// (`0x5d2d1f`) and, when one of its four race/class slots fits the player, that slot's base
/// (`0x5d2d6e`, the last fitting one standing) and the add of a met slot (`0x5d2d89`), which reads
/// the id just written; then the "Inactive" header row, whose index is returned.
fn walk(
    list: &mut FactionList,
    store: &mut Mut<Reputations>,
    slots: &mut FactionSlots,
    world: &World,
) -> Option<usize> {
    use benilla_formats::faction_flags as flag;
    let player = active_player(world)?;
    let (race, class) = (
        player.unit_race().unwrap_or(0),
        player.unit_class().unwrap_or(0),
    );
    let catalog = world.get_resource::<Factions>().map(|f| f.catalog());
    list.begin_world_entry();
    let mut records: Vec<(u32, &FactionInfo)> =
        catalog.map_or_else(Vec::new, |c| c.reputation_factions().collect());
    records.sort_by_key(|&(id, _)| std::cmp::Reverse(id));
    for (id, info) in records {
        let Some(slot) = usize::try_from(info.rep_index)
            .ok()
            .filter(|&i| i < slots.0.len())
        else {
            continue;
        };
        slots.0[slot].0 = id;
        if info.slot_for(race, class).is_none() {
            continue;
        }
        slots.0[slot].1 = info.base_for(race, class);
        let met = store
            .0
            .get(slot)
            .is_some_and(|&(flags, _)| flags & flag::VISIBLE != 0);
        if met {
            let table = AppFactions {
                store: store.bypass_change_detection(),
                slots,
                catalog,
                player: Some(player),
                net: None,
                wrote: false,
                reacted: false,
            };
            list.add_visible(&table, slot as u32);
        }
    }
    Some(list.end_world_entry())
}

/// `SMSG_INITIALIZE_FACTIONS` (`0x5d2e30`): each slot's flags and standing, the list's world entry
/// (`0x5d2e84`), then `UPDATE_FACTION` (`0x5d2e90`).
fn on_initialize_factions(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::Reputations { standings } = ev else {
        return;
    };
    info!("net: reputation store ({} slots)", standings.len());
    if let Some(mut store) = world.get_resource_mut::<Reputations>() {
        store.0 = standings;
    }
    enter_world(world);
    update_faction(world);
}

/// `SMSG_SET_FACTION_VISIBLE` (`0x5d2050`): an unmet slot joins the list (`0x5d2077`), announcing
/// its rank (`0x5d09f2`), its visible bit is stored, then the sort (`0x5d208e`).
fn on_visible(In(ev): In<SessionEvent>, world: &mut World) {
    use benilla_formats::faction_flags as flag;
    let SessionEvent::ReputationVisible { list_id } = ev else {
        return;
    };
    let Some(i) = reputation_slot(list_id, "SMSG_SET_FACTION_VISIBLE") else {
        return;
    };
    let Some(mut store) = world.get_resource_mut::<Reputations>() else {
        return;
    };
    if store.0.len() <= i {
        store.0.resize(i + 1, (0, 0));
    }
    if store.0[i].0 & flag::VISIBLE != 0 {
        return;
    }
    add_visible(world, list_id);
    world.resource_mut::<Reputations>().0[i].0 |= flag::VISIBLE;
    sort(world);
}

/// `SMSG_SET_FACTION_ATWAR` (`0x5d0850`): the slot's at-war bit from the packet's `0x02`,
/// `UPDATE_FACTION` (`0x5d0898`), then the player's reaction refresh (`0x5d08c0`).
fn on_at_war(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::ReputationAtWar { list_id, at_war } = ev else {
        return;
    };
    let Some(i) = reputation_slot(list_id, "SMSG_SET_FACTION_ATWAR") else {
        return;
    };
    let Some(mut store) = world.get_resource_mut::<Reputations>() else {
        return;
    };
    if store.0.len() <= i {
        store.0.resize(i + 1, (0, 0));
    }
    let flags = &mut store.0[i].0;
    if at_war {
        *flags |= benilla_formats::faction_flags::AT_WAR;
    } else {
        *flags &= !benilla_formats::faction_flags::AT_WAR;
    }
    update_faction(world);
    if active_player(world).is_some() {
        reask_quest_status(world);
    }
}

/// The visible-faction add with its announce (`0x5d08d0`): a new row prints the faction's rank
/// (`0x5d09eb` -> `0x5d09f2`) when the table has it and the active player is held (`0x50ba10`).
/// `true` when the row is new.
fn add_visible(world: &mut World, list_id: u32) -> bool {
    let added = with_table(world, |list, t| {
        list.add_visible(t, list_id).then(|| {
            let female = t.player.and_then(|p| p.unit_gender()) == Some(1);
            let line = t
                .slot_faction(list_id)
                .filter(|_| t.player.is_some())
                .map(|e| (e.name, e.standing_id - 1));
            (line, female)
        })
    })
    .flatten();
    let Some((line, female)) = added else {
        return false;
    };
    if let Some((name, rank)) = line {
        let line = crate::combat_log::chat::faction_rank_line(&name, rank, female);
        crate::ui_chat::add_combat_line_now(world, line);
    }
    true
}

/// The change line's `FACTION` combat text, raised right after the line (`0x50b9f4` ->
/// `0x74d480`), before the handler's rank line, sort and `UPDATE_FACTION`.
fn faction_combat_text(world: &mut World, text: crate::ui_unit::CombatTextEvent) {
    let mut args = Vec::with_capacity(text.args.len() + 1);
    args.push(benilla_ui::script::ScriptValue::Str(
        text.message_type.into(),
    ));
    args.extend(text.args);
    crate::ui_script::live::with_live_world(world, |s| s.fire_event("COMBAT_TEXT_UPDATE", args));
}

/// `SMSG_SET_FACTION_STANDING` (`0x5d20a0`), row by row with each line raised at its step, then
/// the sort, the quest log's update, the reaction refresh, tutorial 15 and `UPDATE_FACTION`.
fn on_standing(In(ev): In<SessionEvent>, world: &mut World) {
    use benilla_formats::faction_flags as flag;
    let SessionEvent::ReputationDelta {
        bonus,
        // The rank-up effect it gates (`0x5d22bc`-`0x5d2319`) is not played: #1442.
        show_effect: _,
        standings,
    } = ev
    else {
        return;
    };
    if !world.contains_resource::<Reputations>() {
        return;
    }
    let held = active_player(world).is_some();
    let (female, level) = active_player(world).map_or((false, None), |p| {
        (p.unit_gender() == Some(1), p.unit_level())
    });
    let (mut sort_owed, mut quests_owed, mut reaction_owed, mut revealed) =
        (false, false, false, false);
    for (row, (list_id, standing)) in standings.into_iter().enumerate() {
        let Some(i) = reputation_slot(list_id, "SMSG_SET_FACTION_STANDING") else {
            continue;
        };
        let record = with_table(world, |_, t| t.slot_base(list_id)).flatten();
        let base = record.as_ref().map_or(0, |(_, base)| *base);
        let (mut flags, old) = {
            let mut store = world.resource_mut::<Reputations>();
            if store.0.len() <= i {
                store.0.resize(i + 1, (0, 0));
            }
            store.0[i]
        };
        // The change line's form (`0x5d211d`-`0x5d2142`): flag `0x40` always, generic when hidden.
        let log_mode = if flags & FACTION_FLAG_ALWAYS_LOGGED != 0 {
            Some(flags & flag::HIDDEN != 0)
        } else if row == 0 && flags & flag::HIDDEN == 0 {
            Some(false)
        } else {
            None
        };
        // The store's faction 0, or one without a record, ranks Neutral at both reads (`0x5d0625`).
        let rank = |standing: i32| {
            record
                .as_ref()
                .map_or(3, |_| benilla_formats::reputation_rank(base + standing))
        };
        let old_rank = rank(old);
        if standing != old {
            world.resource_mut::<Reputations>().0[i].1 = standing;
            let change = match (log_mode, record.as_ref()) {
                (Some(generic), Some((name, _))) => crate::combat_log::chat::faction_change_line(
                    name,
                    standing - old,
                    generic,
                    bonus,
                ),
                _ => None,
            };
            if let Some((line, text)) = change {
                crate::ui_chat::add_combat_line_now(world, line);
                if let Some(text) = text {
                    faction_combat_text(world, text);
                }
            }
        }
        let new_rank = rank(standing);
        // An unmet, unhidden faction is revealed; a new row marks the packet (`0x5d21d9`, `0x5d21e8`).
        if flags & (flag::VISIBLE | flag::HIDDEN) == 0 {
            flags |= flag::VISIBLE;
            revealed |= add_visible(world, list_id);
        }
        // Below Unfriendly sets war, a rise clears it (`0x5d21f7`, `0x5d2200`).
        if new_rank < 2 {
            flags |= flag::AT_WAR;
        } else if new_rank > old_rank {
            flags &= !flag::AT_WAR;
        }
        {
            let mut store = world.resource_mut::<Reputations>();
            if store.0[i].0 != flags {
                // A war bit's change owes the reaction refresh (`0x5d221c`).
                reaction_owed |= (store.0[i].0 ^ flags) & flag::AT_WAR != 0;
                store.0[i].0 = flags;
                sort_owed = true;
            }
        }
        // A rank move prints unless hidden, a header without `0x80` or revealing (`0x5d222a`-`0x5d2243`).
        if new_rank != old_rank {
            let shown = flags & flag::HIDDEN == 0
                && (flags & flag::HEADER == 0 || flags & FACTION_FLAG_RANK_SHOWN != 0);
            // The line itself needs the active player (`0x50ba10`).
            if shown && !revealed && held {
                if let Some((name, _)) = record.as_ref() {
                    let line = crate::combat_log::chat::faction_rank_line(name, new_rank, female);
                    crate::ui_chat::add_combat_line_now(world, line);
                }
            }
            quests_owed = true;
            reaction_owed = true;
        }
    }
    if sort_owed {
        sort(world);
    }
    // The quest log's update with argument 0 queues its signal alone (`0x5d22a1`, `0x5e6952`).
    if quests_owed {
        if let Some(mut signals) =
            world.get_non_send_resource_mut::<crate::ui_script::live::UniqueSignals>()
        {
            signals.unique_signal("QUEST_LOG_UPDATE");
        }
    }
    // The player's reaction refresh after a war bit's change or a rank move (`0x5d2256` ->
    // `0x5d22b7`), re-asking the questgivers' status (`0x71f94b`).
    if reaction_owed && held {
        reask_quest_status(world);
    }
    // Tutorial 15 for a player of level 10 or more (`0x5d2328`, `0x5d2333`).
    if level.is_some_and(|l| l >= 10) {
        crate::tutorial::trigger_at_step(world, 15);
        crate::ui_script::live::fire_handler_events(world);
    }
    // `UPDATE_FACTION` closes the handler (`0x5d2342`).
    update_faction(world);
}

#[cfg(test)]
mod tests;

/// A view lending only a faction list over fixed rows, for the stock-UI tests whose other state the
/// VM seeds itself.
#[cfg(test)]
pub(crate) mod rep_view {
    use benilla_ui::script::{
        FactionEntry, FactionList, FactionTable, LiveState, StaticFactions, UiScript,
    };

    pub(crate) struct RepView {
        pub(crate) list: FactionList,
        pub(crate) table: StaticFactions,
    }

    impl RepView {
        /// `entries` as the table, a player watching nothing.
        pub(crate) fn new(entries: Vec<FactionEntry>) -> Self {
            Self {
                list: FactionList::default(),
                table: StaticFactions {
                    entries,
                    watched: Some(-1),
                    ..Default::default()
                },
            }
        }

        /// `f` on `s` with this view lent.
        pub(crate) fn lend<R>(
            &mut self,
            s: &mut UiScript,
            f: impl FnOnce(&mut UiScript) -> R,
        ) -> R {
            s.with_live(self, f)
        }

        /// The list's world entry as [`super::enter_world`] runs it: the build, the fold and the
        /// sort, each sort's `UPDATE_FACTION` raised with this view lent.
        pub(crate) fn enter_world(&mut self, s: &mut UiScript) {
            let row = self.list.enter_world(&self.table);
            self.list.collapse(&self.table, row, true);
            self.lend(s, |s| s.fire_event("UPDATE_FACTION", vec![]));
            self.list.sort(&self.table);
            self.lend(s, |s| s.fire_event("UPDATE_FACTION", vec![]));
        }
    }

    impl LiveState for RepView {
        fn unit_flags(&self, _guid: u64) -> Option<u32> {
            None
        }

        fn unit_health(&self, _guid: u64) -> Option<u32> {
            None
        }

        fn active_player(&self) -> Option<u64> {
            None
        }

        fn has_control(&self) -> bool {
            true
        }

        fn far_sight(&self) -> bool {
            false
        }

        fn reputation(
            &mut self,
            f: &mut dyn FnMut(&mut FactionList, &mut dyn FactionTable),
        ) -> bool {
            f(&mut self.list, &mut self.table);
            true
        }
    }
}
