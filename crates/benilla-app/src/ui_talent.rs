//! The client's talent state ([`TalentInfo`]) as 12340 keeps it, which the talent verbs read and
//! write at the call through the live view: the player's tabs, built from `Talent.dbc` and
//! `TalentTab.dbc` at the world entry (`0x5c9d50`), and the groups `SMSG_TALENTS_INFO` replaces
//! whole (`0x5c9e50`), then raises `PLAYER_TALENT_UPDATE` and, when the active group moved,
//! `ACTIVE_TALENT_GROUP_CHANGED(new, old)` (`0x5c9ecf`).
//!
//! The pet's tabs follow the family the pet setter names (`0x5d6aa0` into `0x5c9df0`), and its
//! group the pet body, which raises `PET_TALENT_UPDATE` (`0x5c9f10`). The inspected player's tabs
//! follow the inspect target (`0x5c9f70`), and its groups `SMSG_INSPECT_TALENT` for that target,
//! whose handler raises `INSPECT_TALENT_READY` (`0x6ce0c0`). `CHARACTER_POINTS_CHANGED` is our
//! character points' field watch (`0x5e8330`).

use bevy::prelude::*;

use benilla_formats::TalentCatalog;
use benilla_protocol::messages::TalentsInfo;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{
    ScriptValue, TalentInfo, TalentPacket, TalentPacketGroup, TalentPages, TalentPrereqView,
    TalentRankView, TalentTabView, TalentView,
};

use crate::char_select::ClientState;
use crate::net::NetHandlerApp;
use crate::ui_action::Spells;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};

/// The talent catalog (`Talent.dbc`, `TalentTab.dbc`), absent when the client data failed to load.
#[derive(Resource)]
pub(crate) struct Talents {
    pub(crate) catalog: std::sync::Arc<TalentCatalog>,
}

/// The client's talent state (`0xc20ff0`-`0xc21138`).
#[derive(Resource, Default)]
pub(crate) struct TalentState(pub(crate) TalentInfo);

pub(crate) struct UiTalentPlugin;

impl Plugin for UiTalentPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TalentState>()
            .net_handler(SessionEventKind::TalentsInfo, on_talents_info)
            .add_systems(Startup, load_talents.after(AssetSet::Open))
            // The game shutdown's talent part (`0x5291d6`, `0x5291db`), after the UI's.
            .add_systems(
                OnExit(ClientState::InWorld),
                shutdown.after(crate::ui_script::end_ui_session),
            );
    }
}

fn load_talents(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_talent_catalog(&mut chain)
    };
    match loaded {
        Ok(catalog) => {
            info!("ui_talent: {} talent(s) in the catalog", catalog.len());
            commands.insert_resource(Talents {
                catalog: std::sync::Arc::new(catalog),
            });
        }
        Err(e) => warn!("ui_talent: Talent.dbc failed to load — no talent window: {e:#}"),
    }
}

/// The game shutdown (`0x5c9dc0`, `0x5c9f60`): every side's tabs and groups emptied, after the UI
/// shutdown's world leave unbuilt them ([`leave_world`]).
fn shutdown(mut state: ResMut<TalentState>) {
    state.0.shutdown();
}

/// Whether a packet's rank names a rank spell with a spell record: the talent row (`0x5c934e`),
/// a rank under 9 naming a spell (`0x5c935f`, `0x5c9364`), its spell record (`0x5c93a1`).
fn rank_is_valid(catalog: &TalentCatalog, spells: &Spells, id: u32, rank: i32) -> bool {
    catalog
        .talent_by_id(id)
        .and_then(|t| t.ranks.get(usize::try_from(rank).ok()?).copied())
        .is_some_and(|spell| spell != 0 && spells.catalog.get(spell).is_some())
}

/// `f` over the talent state with the rank check its packet readers make; `None` without the
/// catalogs, where no rank is valid.
fn with_state<R>(
    world: &mut World,
    f: impl FnOnce(&mut TalentInfo, &dyn Fn(u32, i32) -> bool) -> R,
) -> Option<R> {
    let catalog = world.get_resource::<Talents>().map(|t| t.catalog.clone());
    world.resource_scope(|world, mut state: Mut<TalentState>| {
        let spells = world.get_resource::<Spells>();
        let valid = |id: u32, rank: i32| match (catalog.as_deref(), spells) {
            (Some(c), Some(s)) => rank_is_valid(c, s, id, rank),
            _ => false,
        };
        Some(f(&mut state.0, &valid))
    })
}

/// A player body as the state reads it.
fn packet_of(
    unspent: u32,
    active: u8,
    groups: Vec<benilla_protocol::messages::TalentGroupInfo>,
) -> TalentPacket {
    TalentPacket {
        unspent,
        active,
        groups: groups
            .into_iter()
            .map(|g| TalentPacketGroup {
                talents: g.talents.iter().map(|t| (t.talent_id, t.rank)).collect(),
                glyphs: g.glyphs,
            })
            .collect(),
    }
}

/// `SMSG_TALENTS_INFO` (`0x6cd770`): the pet body (`0x6cd78e`) or the player's (`0x6cd7a0`), each
/// raising its events after its writes.
fn on_talents_info(In(ev): In<SessionEvent>, world: &mut World) {
    let SessionEvent::TalentsInfo(info) = ev else {
        return;
    };
    if !world.contains_resource::<TalentState>() {
        return;
    }
    match info {
        TalentsInfo::Pet { unspent, talents } => {
            debug!(
                "ui_talent: pet SMSG_TALENTS_INFO {} talent(s), {unspent} unspent",
                talents.len()
            );
            let talents: Vec<(u32, u8)> = talents.iter().map(|t| (t.talent_id, t.rank)).collect();
            with_state(world, |state, valid| {
                state.update_pet_talent_data(unspent, &talents, valid)
            });
            // `PET_TALENT_UPDATE` after the group is read (`0x5c9f50`).
            crate::ui_script::safe_world::raise(world, "PET_TALENT_UPDATE", Vec::new());
        }
        TalentsInfo::Player {
            unspent,
            active_group,
            groups,
        } => {
            debug!(
                "ui_talent: SMSG_TALENTS_INFO {} group(s), active {active_group}, {unspent} unspent",
                groups.len()
            );
            let packet = packet_of(unspent, active_group, groups);
            let moved = with_state(world, |state, valid| {
                state.update_talent_data(&packet, valid)
            })
            .flatten();
            // `PLAYER_TALENT_UPDATE` (`0x5c9eca`), then the active group's move (`0x5c9efb`).
            crate::ui_script::safe_world::raise(world, "PLAYER_TALENT_UPDATE", Vec::new());
            if let Some((new, old)) = moved {
                crate::ui_script::safe_world::raise(
                    world,
                    "ACTIVE_TALENT_GROUP_CHANGED",
                    vec![ScriptValue::Int(new), ScriptValue::Int(old)],
                );
            }
        }
    }
}

/// `SMSG_INSPECT_TALENT`'s talents for the inspect target (`0x6ce0da`): the inspected groups read
/// (`0x6ce0ee`), then `INSPECT_TALENT_READY` (`0x6ce0fa`), ahead of the worn items it carries.
pub(crate) fn inspect_talents(world: &mut World, info: TalentsInfo) {
    let TalentsInfo::Player {
        unspent,
        active_group,
        groups,
    } = info
    else {
        return;
    };
    if !world.contains_resource::<TalentState>() {
        return;
    }
    let packet = packet_of(unspent, active_group, groups);
    with_state(world, |state, valid| {
        state.update_inspect_talent_data(&packet, valid)
    });
    crate::ui_script::safe_world::raise(world, "INSPECT_TALENT_READY", Vec::new());
}

/// The player's tabs for `(race, class)` (`0x5c64d0` with a player).
pub(crate) fn player_pages(
    catalog: &TalentCatalog,
    spells: &benilla_formats::SpellCatalog,
    race: u8,
    class: u8,
) -> TalentPages {
    pages_of(catalog, &catalog.tabs_for_class(race, class), spells)
}

/// The pet's tabs for creature family `family` (`0x5c64d0` with none): `None` for a family with no
/// row or no talent type, where the build leaves the tabs as they were (`0x5c6526`).
pub(crate) fn pet_pages(
    catalog: &TalentCatalog,
    spells: &benilla_formats::SpellCatalog,
    families: &benilla_formats::CreatureFamilies,
    family: u32,
) -> Option<TalentPages> {
    let tree = families.get(family)?.pet_talent_type;
    (tree >= 0).then(|| pages_of(catalog, &catalog.tabs_for_pet(tree), spells))
}

/// The tabs' views and each tab's talents in row order.
fn pages_of(
    catalog: &TalentCatalog,
    tab_rows: &[&benilla_formats::TalentTabInfo],
    spells: &benilla_formats::SpellCatalog,
) -> TalentPages {
    let mut pages = TalentPages::default();
    for tab in tab_rows {
        let views = catalog
            .talents_in_tab(tab.id)
            .iter()
            .map(|t| TalentView {
                id: t.id,
                tier: t.row + 1,
                column: t.col + 1,
                ranks: t.ranks[..t.max_rank() as usize]
                    .iter()
                    .map(|&spell| {
                        let d = spells.get(spell);
                        TalentRankView {
                            spell,
                            name: d.map(|d| d.name.clone()).unwrap_or_default(),
                            icon: d.and_then(|d| d.icon.clone()),
                        }
                    })
                    .collect(),
                exceptional: t.exceptional,
                prereqs: t
                    .prereqs
                    .iter()
                    .map(|&(id, rank)| {
                        let rec = catalog.talent_by_id(id);
                        // The spell at the needed rank or the first later one (`0x622585`).
                        let spell = rec
                            .and_then(|r| r.ranks.iter().skip(rank as usize).find(|&&s| s != 0))
                            .copied()
                            .unwrap_or(0);
                        TalentPrereqView {
                            talent: id,
                            rank,
                            seat: rec.map(|r| (r.row + 1, r.col + 1)),
                            spell,
                            name: spells.get(spell).map(|d| d.name.clone()),
                        }
                    })
                    .collect(),
                required_spell: t.required_spell,
                required_spell_name: (t.required_spell != 0)
                    .then(|| spells.get(t.required_spell).map(|d| d.name.clone()))
                    .flatten(),
                category_mask: t.category_mask,
            })
            .collect();
        pages.tabs.push(TalentTabView {
            id: tab.id,
            name: tab.name.clone(),
            icon: tab.icon.clone(),
            background: tab.background.clone(),
        });
        pages.talents.push(views);
    }
    pages
}

/// The UI world entry's talent part (`0x528235`, `0x52823a`): the player's tabs built once from
/// our race and class, the groups laid over them, then the inspect target forgotten.
pub(crate) fn enter_world(world: &mut World) {
    if !world.contains_resource::<TalentState>() {
        return;
    }
    let race_class = world
        .query_filtered::<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>()
        .iter(world)
        .next()
        .and_then(|s| Some((s.0.unit_race()?, s.0.unit_class()?)));
    let catalog = world.get_resource::<Talents>().map(|t| t.catalog.clone());
    world.resource_scope(|world, mut state: Mut<TalentState>| {
        let spells = world.get_resource::<Spells>();
        state.0.enter_world(|| {
            let (race, class) = race_class?;
            Some(player_pages(
                catalog.as_deref()?,
                &spells?.catalog,
                race,
                class,
            ))
        });
        state.0.inspect_enter_world();
    });
}

/// The UI world leave's talent part (`0x528d4b`, `0x528d50`).
pub(crate) fn leave_world(world: &mut World) {
    if let Some(mut state) = world.get_resource_mut::<TalentState>() {
        state.0.leave_world();
        state.0.inspect_enter_world();
    }
}

/// The pet setter's tab update (`0x5c9df0`) for `family`, 0 when the pet list emptied.
pub(crate) fn update_pet_tabs(world: &mut World, family: u32) {
    if !world.contains_resource::<TalentState>() {
        return;
    }
    let catalog = world.get_resource::<Talents>().map(|t| t.catalog.clone());
    world.resource_scope(|world, mut state: Mut<TalentState>| {
        let spells = world.get_resource::<Spells>();
        let families = world.get_resource::<crate::ui_pet_stats::PetFamilyTables>();
        state.0.update_pet_tabs(family as i32, || {
            pet_pages(
                catalog.as_deref()?,
                &spells?.catalog,
                &families?.families,
                family,
            )
        });
    });
}

/// The inspect target set (`0x5c9f70`): the tabs of `guid`'s class when it is a held player
/// (`0x5c9f8f`), else none.
pub(crate) fn set_inspect_target(
    state: &mut TalentInfo,
    guid: u64,
    class: Option<(u8, u8)>,
    catalog: Option<&TalentCatalog>,
    spells: Option<&Spells>,
) {
    let pages =
        class.and_then(|(race, class)| Some(player_pages(catalog?, &spells?.catalog, race, class)));
    state.set_inspect_target(guid, pages);
}

/// Our character points' watch over `PLAYER_CHARACTER_POINTS1/2`, one 8-byte handler (`0x5e8470`).
pub(crate) fn is_character_points_watch(index: u16) -> bool {
    use benilla_protocol::field::{FIELD_PLAYER_CHARACTER_POINTS1, FIELD_PLAYER_CHARACTER_POINTS2};
    index == FIELD_PLAYER_CHARACTER_POINTS1 || index == FIELD_PLAYER_CHARACTER_POINTS2
}

/// `CHARACTER_POINTS_CHANGED`'s deltas (`0x5e8330`): each point's value now less its old one,
/// signed, a field the update left standing reading its own value as old.
pub(crate) fn character_points_deltas(edges: &[(u16, u32, u32)], now: (u32, u32)) -> (i64, i64) {
    use benilla_protocol::field::{FIELD_PLAYER_CHARACTER_POINTS1, FIELD_PLAYER_CHARACTER_POINTS2};
    let old = |index: u16, now: u32| {
        edges
            .iter()
            .find(|&&(i, ..)| i == index)
            .map_or(now, |&(_, old, _)| old)
    };
    let delta = |index, now: u32| i64::from(now as i32) - i64::from(old(index, now) as i32);
    (
        delta(FIELD_PLAYER_CHARACTER_POINTS1, now.0),
        delta(FIELD_PLAYER_CHARACTER_POINTS2, now.1),
    )
}

/// What the talent verbs reach at the call: the state, the pet list and the pet's family tables.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct TalentLive<'w> {
    pub(crate) state: Option<ResMut<'w, TalentState>>,
    pub(crate) pets: Option<Res<'w, crate::ui_pet::PetList>>,
    pub(crate) names: Option<Res<'w, crate::names::NameCache>>,
    pub(crate) families: Option<Res<'w, crate::ui_pet_stats::PetFamilyTables>>,
}

#[cfg(test)]
pub(crate) mod tests;
