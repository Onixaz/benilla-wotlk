//! The reputation pane's feed: the player's wire reputation slots resolved against `Faction.dbc`,
//! pushed as one snapshot with `UPDATE_FACTION` on a change, and the pane's three verbs sent.
//!
//! Every faction with a reputation slot is pushed, met or not: the engine's list names its
//! headers from the table (`0x5d01f0`), and its login build keeps the factions whose race/class
//! slot fits the player (`0x5d2bc0`). Flag `0x08` is `HEADER`, not "force invisible".
//!
//! The wire standing excludes the race/class base, so the pane's total is base plus wire
//! (`0x5d05b0`). The rank is [`benilla_formats::reputation_rank`], shared with the unit-reaction
//! decode; the pane shows it uncapped.

use bevy::prelude::*;

use benilla_ui::script::{FactionEntry, ReputationSend, ReputationState, UiScript};

use crate::net::{ClientCommand, NetCommands, ObjectStore, Reputations, SelfPlayer};
use crate::target::Factions;
use crate::ui_script::{UiFeed, UiInput};

/// Rank `r` spans `RANK_BOUNDS[r]..RANK_BOUNDS[r + 1]`, the client's table at `0xa2d2fc`, the edges
/// [`benilla_formats::reputation_rank`] ranks by. The Lua `standingID` is the rank plus one.
const RANK_BOUNDS: [i32; 9] = [-42000, -6000, -3000, 0, 3000, 9000, 21000, 42000, 43000];

/// The reputation pane's feed and outbound drain; the bindings are `benilla-ui`'s.
pub(crate) struct UiReputationPlugin;

impl Plugin for UiReputationPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                feed_reputation.in_set(UiFeed),
                drain_reputation_sends.after(UiInput),
            ),
        );
    }
}

/// One faction's row, or `None` without a reputation slot or a name; an unmet faction is still a
/// row, with `visible: false`.
pub(crate) fn reputation_row(
    faction_id: u32,
    info: &benilla_formats::FactionInfo,
    catalog: &benilla_formats::FactionCatalog,
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

/// The by-value inputs of the feed's gate, `(race, class, watched)`; the other two are resources.
type ReputationInputs = Option<(u8, u8, Option<u32>)>;

/// Push the snapshot and fire `UPDATE_FACTION` when it differs. The build runs only when the wire
/// slots, the catalog, race/class or the watched index move, and the equality check behind it
/// keeps a change to no faction quiet. The first push fires too: the stock watch bar initializes
/// off `UPDATE_FACTION` at login.
fn feed_reputation(
    script: Option<NonSendMut<UiScript>>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    reputations: Res<Reputations>,
    factions: Option<Res<Factions>>,
    mut last: Local<crate::ui_script::VmMemo<Option<ReputationState>>>,
    mut last_inputs: Local<crate::ui_script::VmMemo<ReputationInputs>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let last_inputs = last_inputs.get(&script);
    let (Ok(store), Some(factions_res)) = (self_store.single(), factions.as_ref()) else {
        return;
    };
    let catalog = factions_res.catalog();
    let (race, class) = (
        store.0.unit_race().unwrap_or(0),
        store.0.unit_class().unwrap_or(0),
    );
    let watched = store
        .0
        .player_watched_faction()
        .and_then(|i| u32::try_from(i).ok());

    let inputs = (race, class, watched);
    let moved = reputations.is_changed()
        || factions_res.is_changed()
        || last_inputs.as_ref() != Some(&inputs)
        || last.is_none();
    if !moved {
        return;
    }
    *last_inputs = Some(inputs);

    let mut entries = Vec::new();
    for (faction_id, info) in catalog.reputation_factions() {
        // An uncovered slot reads `(0, 0)`, unmet.
        let (flags, wire) = usize::try_from(info.rep_index)
            .ok()
            .and_then(|i| reputations.0.get(i))
            .copied()
            .unwrap_or((0, 0));
        if let Some(row) = reputation_row(faction_id, info, catalog, flags, wire, race, class) {
            entries.push(row);
        }
    }
    // Sorted before the equality check, so a map reshuffle is not a change.
    entries.sort_by_key(|e| e.faction_id);

    let fresh = ReputationState { entries, watched };
    if last.as_ref() == Some(&fresh) {
        return;
    }
    script.set_reputation(fresh.clone());
    *last = Some(fresh);
    script.fire_event("UPDATE_FACTION", vec![]);
}

/// Send the pane's queued verbs. None is acked, so the war and inactive flags are written into
/// the store the way the client writes its own (`0x5d0a10`, `0x5d1c10`): the next push keeps them.
fn drain_reputation_sends(
    script: Option<NonSendMut<UiScript>>,
    commands: Res<NetCommands>,
    mut reputations: ResMut<Reputations>,
) {
    use benilla_formats::faction_flags as flag;
    let Some(mut script) = script else {
        return;
    };
    for send in script.take_reputation_sends() {
        let cmd = match send {
            ReputationSend::AtWar {
                rep_list_id,
                at_war,
            } => {
                if let Some((flags, _)) = reputations.0.get_mut(rep_list_id as usize) {
                    if at_war && *flags & flag::PEACE_FORCED == 0 {
                        *flags |= flag::AT_WAR;
                    } else {
                        *flags &= !flag::AT_WAR;
                    }
                }
                ClientCommand::SetFactionAtWar {
                    rep_list_id,
                    at_war,
                }
            }
            ReputationSend::Inactive {
                rep_list_id,
                inactive,
            } => {
                if let Some((flags, _)) = reputations.0.get_mut(rep_list_id as usize) {
                    if inactive {
                        *flags |= flag::INACTIVE;
                    } else {
                        *flags &= !flag::INACTIVE;
                    }
                }
                ClientCommand::SetFactionInactive {
                    rep_list_id,
                    inactive,
                }
            }
            // No watch is -1 on the wire: slot 0 is a real faction.
            ReputationSend::Watch(slot) => ClientCommand::SetWatchedFaction {
                rep_list_id: slot.map_or(benilla_protocol::messages::WATCHED_FACTION_NONE, |s| {
                    s as i32
                }),
            },
        };
        let _ = commands.0.send(cmd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pane's bar reads [`RANK_BOUNDS`], the nameplate the rank function: they must agree.
    #[test]
    fn rank_bounds_agree_with_the_rank_function() {
        for rank in 0u8..=7 {
            let (floor, ceiling) = (RANK_BOUNDS[rank as usize], RANK_BOUNDS[rank as usize + 1]);
            assert_eq!(
                benilla_formats::reputation_rank(floor),
                rank,
                "rank {rank}'s floor {floor}"
            );
            assert_eq!(
                benilla_formats::reputation_rank(ceiling - 1),
                rank,
                "rank {rank}'s top {}",
                ceiling - 1
            );
            if rank > 0 {
                assert_eq!(
                    benilla_formats::reputation_rank(floor - 1),
                    rank - 1,
                    "one below rank {rank}'s floor"
                );
            }
        }
    }

    /// Every faction some faction names as its parent has a reputation slot and a name, so each
    /// header the list adds (`0x5d01f0`) reaches the engine named. Skips without client data.
    #[test]
    fn every_parent_faction_reaches_the_engine_named() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let cat = benilla_formats::load_faction_catalog(&mut chain).expect("factions");
        let parents: std::collections::BTreeSet<u32> = cat
            .reputation_factions()
            .map(|(_, f)| f.team)
            .filter(|&t| t != 0)
            .collect();
        assert!(!parents.is_empty());
        for id in parents {
            let info = cat
                .reputation_faction(id)
                .unwrap_or_else(|| panic!("parent {id} has a reputation slot"));
            let row = reputation_row(id, info, &cat, 0, 0, 1, 8)
                .unwrap_or_else(|| panic!("parent {id} resolves"));
            assert!(!row.name.is_empty(), "parent {id} is named");
        }
    }

    /// A human mage who has met every faction the table seeds for them: the engine nests the
    /// expansions' sub-headers, and every shown row reads as the stock pane needs it. Skips
    /// without client data.
    #[test]
    fn the_3_3_5a_tree_nests_and_every_row_reads_whole() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let cat = benilla_formats::load_faction_catalog(&mut chain).expect("factions");
        use benilla_formats::faction_flags as flag;
        let (race, class) = (1, 8);
        let mut entries: Vec<_> = cat
            .reputation_factions()
            .filter_map(|(id, info)| {
                let seed = u8::try_from(info.default_flags_for(race, class)).ok()?;
                let hidden = seed & (flag::HIDDEN | flag::HEADER) != 0;
                let flags = if hidden { seed } else { seed | flag::VISIBLE };
                reputation_row(id, info, &cat, flags, 0, race, class)
            })
            .collect();
        entries.sort_by_key(|e| e.faction_id);

        let mut s = UiScript::new().expect("VM");
        s.set_reputation(ReputationState {
            entries,
            watched: None,
        });
        let (names, standing_ids, headers, children) = s
            .eval::<(Vec<String>, Vec<i64>, Vec<bool>, Vec<bool>)>(
                "local n, s, h, c = {}, {}, {}, {} for i = 1, GetNumFactions() do \
                   local a,_,b,_,_,_,_,_,d,_,_,_,e = GetFactionInfo(i) \
                   n[i], s[i], h[i], c[i] = a or '', b, d ~= nil, e ~= nil end \
                 return n, s, h, c",
            )
            .unwrap();
        assert!(names.len() > 10, "{names:#?}");
        assert!(headers[0] && !children[0], "the list opens on a top header");
        for (name, standing_id) in names.iter().zip(&standing_ids) {
            assert!(!name.is_empty());
            assert!((1..=8).contains(standing_id), "{name}: {standing_id}");
        }
        assert!(
            headers.iter().zip(&children).any(|(&h, &c)| h && c),
            "some header is a sub-header: {names:#?}"
        );
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }
}
