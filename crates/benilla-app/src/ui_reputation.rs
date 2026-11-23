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
//! The wire standing excludes the race/class base, so the pane's total is base plus wire
//! (`0x5d05b0`). The rank is [`benilla_formats::reputation_rank`], shared with the unit-reaction
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

/// The reputation handlers and the list's game-shutdown clear.
pub(crate) struct UiReputationPlugin;

impl Plugin for UiReputationPlugin {
    fn build(&self, app: &mut App) {
        use SessionEventKind as K;
        app.init_resource::<ReputationList>()
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
/// every slot's flags and standing zeroed (`0x5d15ba`).
fn shutdown(mut list: ResMut<ReputationList>, store: Option<ResMut<Reputations>>) {
    *list = ReputationList::default();
    if let Some(mut store) = store {
        store.0.fill((0, 0));
    }
}

/// One faction's record with its slot as it stands, or `None` without a reputation slot or a
/// name; an unmet faction is still a record, with `visible: false`.
pub(crate) fn reputation_row(
    faction_id: u32,
    info: &FactionInfo,
    catalog: &FactionCatalog,
    flags: u8,
    wire_standing: i32,
    race: u8,
    class: u8,
) -> Option<FactionEntry> {
    use benilla_formats::faction_flags as flag;
    let standing = info.base_for(race, class) + wire_standing;
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
        fits_player: info.slot_for(race, class).is_some(),
        visible: flags & flag::VISIBLE != 0,
        is_header: flags & flag::HEADER != 0,
        at_war: flags & flag::AT_WAR != 0,
        peace_forced: flags & flag::PEACE_FORCED != 0,
        inactive: flags & flag::INACTIVE != 0,
        has_rep: info.header_has_rep_for(race, class),
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
    pub(crate) catalog: Option<&'a FactionCatalog>,
    /// The active player's descriptor, `None` before our create.
    pub(crate) player: Option<&'a ObjectFields>,
    pub(crate) net: Option<&'a NetCommands>,
    /// A flag was written; the store reads as changed, as the at-war write re-reads reactions (`0x5d0b68`).
    pub(crate) wrote: bool,
}

impl AppFactions<'_> {
    fn race_class(&self) -> (u8, u8) {
        self.player.map_or((0, 0), |p| {
            (p.unit_race().unwrap_or(0), p.unit_class().unwrap_or(0))
        })
    }

    fn record(&self, faction_id: u32, info: &FactionInfo) -> Option<FactionEntry> {
        let (flags, wire) = usize::try_from(info.rep_index)
            .ok()
            .and_then(|i| self.store.0.get(i))
            .copied()
            .unwrap_or((0, 0));
        let (race, class) = self.race_class();
        reputation_row(faction_id, info, self.catalog?, flags, wire, race, class)
    }

    /// The faction a slot names, with the race/class base its rank reads (`0x5d0600`).
    fn slot_base(&self, list_id: u32) -> Option<(String, i32)> {
        let catalog = self.catalog?;
        let slot = i32::try_from(list_id).ok()?;
        let (id, f) = catalog
            .reputation_factions()
            .find(|(_, f)| f.rep_index == slot)?;
        let (race, class) = self.race_class();
        Some((
            catalog.faction_name(id).unwrap_or_default().to_string(),
            f.base_for(race, class),
        ))
    }
}

impl FactionTable for AppFactions<'_> {
    fn faction(&self, faction_id: i64) -> Option<FactionEntry> {
        let id = u32::try_from(faction_id).ok()?;
        self.record(id, self.catalog?.reputation_faction(id)?)
    }

    fn slot_faction(&self, rep_list_id: u32) -> Option<FactionEntry> {
        let slot = i32::try_from(rep_list_id).ok()?;
        let (id, info) = self
            .catalog?
            .reputation_factions()
            .find(|(_, f)| f.rep_index == slot)?;
        self.record(id, info)
    }

    fn met_slots(&self) -> Vec<u32> {
        let Some(catalog) = self.catalog else {
            return Vec::new();
        };
        let (race, class) = self.race_class();
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
            } => ClientCommand::SetFactionAtWar {
                rep_list_id,
                at_war,
            },
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
    if !world.contains_resource::<Reputations>() {
        return None;
    }
    world.init_resource::<ReputationList>();
    Some(
        world.resource_scope(|world, mut list: Mut<ReputationList>| {
            world.resource_scope(|world, mut store: Mut<Reputations>| {
                // The read verbs write nothing (`0x5cff20`, `0x5d0da0`, `0x5d1240`): only a written
                // flag marks the store changed for the reaction readers.
                let mut table = AppFactions {
                    store: store.bypass_change_detection(),
                    catalog: world.get_resource::<Factions>().map(|f| f.catalog()),
                    player: active_player(world),
                    net: world.get_resource::<NetCommands>(),
                    wrote: false,
                };
                let out = f(&mut list.0, &mut table);
                if table.wrote {
                    store.set_changed();
                }
                out
            })
        }),
    )
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
/// (`0x5d2e84`) run: nothing without the active player; else the build, the "Inactive" header's
/// fold (`0x5d2e0f`) and the sort (`0x5d2e17`), each sort raising `UPDATE_FACTION` (`0x5d1bfa`).
pub(crate) fn enter_world(world: &mut World) {
    let built = with_table(world, |list, t| {
        t.player.is_some().then(|| list.enter_world(t))
    });
    let Some(Some(row)) = built else {
        return;
    };
    if with_table(world, |list, t| list.collapse(t, row, true)) == Some(true) {
        update_faction(world);
    }
    sort(world);
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

/// `SMSG_SET_FACTION_ATWAR` (`0x5d0850`): the slot's at-war bit from the packet's `0x02`, then
/// `UPDATE_FACTION` (`0x5d0898`); the reaction readers read the store.
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

/// `SMSG_SET_FACTION_STANDING` (`0x5d20a0`), row by row, each line raised at its step:
///
/// - The line (`0x5d211d`-`0x5d2142`): a faction with flag `0x40` always, in the generic form
///   with `HIDDEN`; any other only as the packet's first row and not `HIDDEN`.
/// - A new standing is stored and its delta printed, with its `FACTION` combat text (`0x5d217b`,
///   `0x5d21b2`).
/// - A faction neither `VISIBLE` nor `HIDDEN` is revealed and added to the list (`0x5d21d9`); a
///   new row announces its rank and marks the packet as revealing (`0x5d21e8`), for every later
///   row too.
/// - A rank below Unfriendly sets `AT_WAR` (`0x5d21f7`); a rise in rank clears it (`0x5d2200`).
/// - A rank change prints `FACTION_STANDING_CHANGED` (`0x5d2243`) unless the faction is `HIDDEN`,
///   a header without `0x80`, or the packet has revealed a row.
///
/// Then a stored flag's change sorts the list (`0x5d2277`), any rank move queues the quest log's
/// update (`0x5d22a1`), a level-10 player is shown tutorial 15 (`0x5d2333`), and `UPDATE_FACTION`
/// ends it (`0x5d2342`).
fn on_standing(In(ev): In<SessionEvent>, world: &mut World) {
    use benilla_formats::faction_flags as flag;
    let SessionEvent::ReputationDelta {
        bonus,
        show_effect: _,
        standings,
    } = ev
    else {
        return;
    };
    if !world.contains_resource::<Reputations>() {
        return;
    }
    let (female, level) = active_player(world).map_or((false, None), |p| {
        (p.unit_gender() == Some(1), p.unit_level())
    });
    let (mut sort_owed, mut quests_owed, mut revealed) = (false, false, false);
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
        let log_mode = if flags & FACTION_FLAG_ALWAYS_LOGGED != 0 {
            Some(flags & flag::HIDDEN != 0)
        } else if row == 0 && flags & flag::HIDDEN == 0 {
            Some(false)
        } else {
            None
        };
        // A faction without a record ranks Neutral at both reads (`0x5d0600`: `0x5d0625`).
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
        if flags & (flag::VISIBLE | flag::HIDDEN) == 0 {
            flags |= flag::VISIBLE;
            revealed |= add_visible(world, list_id);
        }
        if new_rank < 2 {
            flags |= flag::AT_WAR;
        } else if new_rank > old_rank {
            flags &= !flag::AT_WAR;
        }
        {
            let mut store = world.resource_mut::<Reputations>();
            if store.0[i].0 != flags {
                store.0[i].0 = flags;
                sort_owed = true;
            }
        }
        if new_rank != old_rank {
            let shown = flags & flag::HIDDEN == 0
                && (flags & flag::HEADER == 0 || flags & FACTION_FLAG_RANK_SHOWN != 0);
            // The line itself needs the active player (`0x50ba10`).
            if shown && !revealed && level.is_some() {
                if let Some((name, _)) = record.as_ref() {
                    let line = crate::combat_log::chat::faction_rank_line(name, new_rank, female);
                    crate::ui_chat::add_combat_line_now(world, line);
                }
            }
            quests_owed = true;
        }
    }
    if sort_owed {
        sort(world);
    }
    // The log update with argument 0 queues the signal alone (`0x5d229f`, `0x5e6952`).
    if quests_owed {
        if let Some(mut signals) =
            world.get_non_send_resource_mut::<crate::ui_script::live::UniqueSignals>()
        {
            signals.unique_signal("QUEST_LOG_UPDATE");
        }
    }
    // Standing feeds questgiver status (`SatisfyQuestReputation`); the reference re-asks here too.
    if let Some(mut quest) = world.get_resource_mut::<crate::ui_quest::QuestStatuses>() {
        quest.bump_reask();
    }
    if level.is_some_and(|l| l >= 10) {
        crate::tutorial::trigger_at_step(world, 15);
        crate::ui_script::live::fire_handler_events(world);
    }
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
