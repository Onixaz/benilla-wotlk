use benilla_protocol::field::{FIELD_UNIT_BYTES_0, FIELD_UNIT_LEVEL};
use benilla_protocol::messages::ObjectType;

use super::rep_view::RepView;
use super::*;
use crate::net::{GuidIndex, SelfGuid};
use crate::ui_script::live::UniqueSignals;

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
/// header the list adds (`0x5d01f0`) reaches the bindings named. Skips without client data.
#[test]
fn every_parent_faction_reaches_the_bindings_named() {
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
        let row = reputation_row(id, info, &cat, 0, 0, info.base_for(1, 8), Some((1, 8)))
            .unwrap_or_else(|| panic!("parent {id} resolves"));
        assert!(!row.name.is_empty(), "parent {id} is named");
    }
}

/// A human mage who has met every faction the table seeds for them: the list nests the
/// expansions' sub-headers, and every shown row reads as the stock pane needs it. Skips without
/// client data.
#[test]
fn the_3_3_5a_tree_nests_and_every_row_reads_whole() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let cat = benilla_formats::load_faction_catalog(&mut chain).expect("factions");
    use benilla_formats::faction_flags as flag;
    let (race, class) = (1, 8);
    let entries: Vec<_> = cat
        .reputation_factions()
        .filter_map(|(id, info)| {
            let seed = u8::try_from(info.default_flags_for(race, class)).ok()?;
            let hidden = seed & (flag::HIDDEN | flag::HEADER) != 0;
            let flags = if hidden { seed } else { seed | flag::VISIBLE };
            let base = info.base_for(race, class);
            reputation_row(id, info, &cat, flags, 0, base, Some((race, class)))
        })
        .collect();

    let mut s = benilla_ui::script::UiScript::new().expect("VM");
    let mut v = RepView::new(entries);
    v.enter_world(&mut s);
    let (names, standing_ids, headers, children) = v.lend(&mut s, |s| {
        s.eval::<(Vec<String>, Vec<i64>, Vec<bool>, Vec<bool>)>(
            "local n, s, h, c = {}, {}, {}, {} for i = 1, GetNumFactions() do \
               local a,_,b,_,_,_,_,_,d,_,_,_,e = GetFactionInfo(i) \
               n[i], s[i], h[i], c[i] = a or '', b, d ~= nil, e ~= nil end \
             return n, s, h, c",
        )
        .unwrap()
    });
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

/// A world with the install's faction table and a level-80 Human Warrior held, past the list's
/// world entry, no VM: the lines fall to the chat feed's queue, where they are counted.
fn standing_world() -> Option<(World, FactionCatalog)> {
    let data = benilla_formats::wow_data_or_skip!(None);
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let cat = benilla_formats::load_faction_catalog(&mut chain).expect("dbc");
    let mut world = World::new();
    let me = world
        .spawn(ObjectStore(
            ObjectFields::from_pairs(&[(FIELD_UNIT_BYTES_0, 1 | 1 << 8), (FIELD_UNIT_LEVEL, 80)])
                .into_created(ObjectType::Player),
        ))
        .id();
    world.insert_resource(SelfGuid(Some(1)));
    world.insert_resource(GuidIndex([(1, me)].into_iter().collect()));
    let mut store = Reputations::default();
    store.0.resize(128, (0, 0));
    world.insert_resource(store);
    world.insert_resource(Factions::from_catalog(load_again(&data)));
    world.init_resource::<crate::ui_chat::ChatLog>();
    world.init_resource::<crate::ui_quest::QuestStatuses>();
    world.insert_non_send_resource(UniqueSignals::default());
    enter_world(&mut world);
    assert!(
        world.resource::<FactionSlots>().0.iter().any(|s| s.0 != 0),
        "the walk wrote the columns"
    );
    Some((world, cat))
}

fn load_again(data: &std::path::Path) -> FactionCatalog {
    let mut chain = benilla_formats::open_chain(data).expect("open chain");
    benilla_formats::load_faction_catalog(&mut chain).expect("dbc")
}

fn slot(cat: &FactionCatalog, name: &str) -> u32 {
    cat.reputation_factions()
        .find(|(id, _)| cat.faction_name(*id) == Some(name))
        .map(|(_, f)| f.rep_index as u32)
        .unwrap_or_else(|| panic!("{name} has a reputation slot"))
}

fn standing(world: &mut World, rows: Vec<(u32, i32)>) {
    world
        .run_system_cached_with(
            on_standing,
            SessionEvent::ReputationDelta {
                bonus: 0.0,
                show_effect: false,
                standings: rows,
            },
        )
        .unwrap();
}

/// Whether the quest log's update waits in the frame's unique signals, emptying them.
fn quests_owed(world: &mut World) -> bool {
    let mut signals = world.non_send_resource_mut::<UniqueSignals>();
    let owed = signals.names().contains(&"QUEST_LOG_UPDATE");
    signals.clear();
    owed
}

fn lines(world: &World) -> usize {
    world.resource::<crate::ui_chat::ChatLog>().pending_len()
}

/// One `SMSG_SET_FACTION_STANDING` through the row rule (`0x5d20a0`), as a Human Warrior
/// (Stormwind's base 4000, Friendly).
#[test]
fn the_rows_pick_their_lines_and_the_rank_moves_the_war_flag() {
    use benilla_formats::faction_flags as flag;
    let Some((mut world, cat)) = standing_world() else {
        return;
    };
    let (stormwind, darnassus) = (slot(&cat, "Stormwind"), slot(&cat, "Darnassus"));
    {
        let mut store = world.resource_mut::<Reputations>();
        store.0[stormwind as usize].0 = flag::VISIBLE;
        store.0[darnassus as usize].0 = flag::VISIBLE | flag::AT_WAR;
    }
    let darnassus_base = cat
        .reputation_factions()
        .find(|(_, f)| f.rep_index == darnassus as i32)
        .map(|(_, f)| f.base_for(1, 1))
        .unwrap();
    // Stormwind first, up to Honored: its change line and the rank line; its `FACTION` text goes
    // to the VM, pinned in the characterization rig. Darnassus second changes silently but still
    // prints its rank and drops below Unfriendly.
    standing(
        &mut world,
        vec![(stormwind, 5100), (darnassus, -9000 - darnassus_base)],
    );
    assert_eq!(
        lines(&world),
        3,
        "change + rank for Stormwind, rank for Darnassus"
    );
    assert!(
        quests_owed(&mut world),
        "a rank move owes the quest log's update"
    );
    let store = world.resource::<Reputations>();
    assert_eq!(store.0[stormwind as usize], (flag::VISIBLE, 5100));
    assert_ne!(
        store.0[darnassus as usize].0 & flag::AT_WAR,
        0,
        "Hated sets war"
    );

    // Back up past Hostile: a rise in rank clears the war flag.
    standing(&mut world, vec![(darnassus, -darnassus_base)]);
    assert_eq!(
        world.resource::<Reputations>().0[darnassus as usize].0 & flag::AT_WAR,
        0,
        "a rise clears war"
    );

    quests_owed(&mut world);

    // A change inside one rank owes no update (`0x5d2228`).
    standing(&mut world, vec![(stormwind, 5200)]);
    assert!(!quests_owed(&mut world), "an in-rank change owes nothing");

    // A hidden faction prints nothing, first row or not, and is not revealed, but its rank move
    // still owes the update (`0x5d222a` -> `0x5d2256`).
    world.resource_mut::<Reputations>().0[stormwind as usize].0 = flag::HIDDEN;
    let before = lines(&world);
    standing(&mut world, vec![(stormwind, 40000)]);
    assert_eq!(lines(&world), before);
    assert_eq!(
        world.resource::<Reputations>().0[stormwind as usize].0 & flag::VISIBLE,
        0
    );
    assert!(
        quests_owed(&mut world),
        "a hidden rank move owes the update"
    );
}

/// A slot no `Faction.dbc` record names ranks Neutral at both reads (`0x5d0600` at `0x5d2161` and
/// `0x5d21c1`, `0x5d0625`): its standing is stored, but no drop sets war, no rise clears it and no
/// rank move owes the quest log's update.
#[test]
fn a_slot_without_a_record_never_moves_rank() {
    use benilla_formats::faction_flags as flag;
    let Some((mut world, cat)) = standing_world() else {
        return;
    };
    let named: std::collections::HashSet<i32> = cat
        .reputation_factions()
        .map(|(_, f)| f.rep_index)
        .collect();
    let bare = (0..128u32)
        .find(|i| !named.contains(&(*i as i32)))
        .expect("an unnamed slot below 128");
    world.resource_mut::<Reputations>().0[bare as usize].0 = flag::VISIBLE;
    standing(&mut world, vec![(bare, -9000)]);
    assert_eq!(
        world.resource::<Reputations>().0[bare as usize],
        (flag::VISIBLE, -9000),
        "stored, war untouched"
    );
    assert!(!quests_owed(&mut world), "no rank move");
    world.resource_mut::<Reputations>().0[bare as usize].0 = flag::VISIBLE | flag::AT_WAR;
    standing(&mut world, vec![(bare, 40000)]);
    assert_eq!(
        world.resource::<Reputations>().0[bare as usize],
        (flag::VISIBLE | flag::AT_WAR, 40000),
        "a rise keeps war"
    );
    assert!(!quests_owed(&mut world));
    assert_eq!(lines(&world), 0, "no change or rank line without a name");
}

/// The read verbs write nothing (`0x5cff20`, `0x5d0da0`, `0x5d1240`), so a read leaves the store
/// unchanged for the reaction readers; a written flag marks it (`0x5d0a10`).
#[test]
fn a_read_leaves_the_store_unchanged_and_a_flag_write_marks_it() {
    let Some((mut world, cat)) = standing_world() else {
        return;
    };
    let stormwind = slot(&cat, "Stormwind");
    world.resource_mut::<Reputations>().0[stormwind as usize].0 =
        benilla_formats::faction_flags::VISIBLE;
    enter_world(&mut world);
    world.clear_trackers();
    let changed = |world: &World| world.is_resource_changed::<Reputations>();
    let read = with_table(&mut world, |list, t| {
        (
            list.shown(),
            t.watched(),
            t.slot_faction(stormwind).is_some(),
        )
    });
    assert!(
        read.is_some_and(|(n, w, row)| n > 0 && w.is_some() && row),
        "{read:?}"
    );
    assert!(!changed(&world), "a read marks nothing");
    with_table(&mut world, |_, t| {
        t.write_flag(stormwind, FactionFlag::AtWar, true)
    });
    assert!(changed(&world), "a flag write marks the store");
}

/// A row that reveals its faction adds it to the list and announces its rank (`0x5d21d9` ->
/// `0x5d09f2`); the reveal marks the packet (`0x5d21e8`), so a later row's rank move prints no
/// line, where the rank line of a packet without a reveal does (`0x5d2238`).
#[test]
fn a_reveal_announces_its_rank_and_silences_every_later_rank_line() {
    use benilla_formats::faction_flags as flag;
    let Some((mut world, cat)) = standing_world() else {
        return;
    };
    let (stormwind, ironforge, gnomeregan) = (
        slot(&cat, "Stormwind"),
        slot(&cat, "Ironforge"),
        slot(&cat, "Gnomeregan Exiles"),
    );
    {
        let mut store = world.resource_mut::<Reputations>();
        store.0[stormwind as usize].0 = flag::VISIBLE;
        store.0[ironforge as usize].0 = flag::VISIBLE;
    }
    enter_world(&mut world);
    let listed = world.resource::<ReputationList>().0.len();
    // Gnomeregan unmet first (revealed and announced, its change line too), then Stormwind's rank
    // rise: no rank line.
    standing(&mut world, vec![(gnomeregan, 10), (stormwind, 6000)]);
    assert_eq!(
        lines(&world),
        2,
        "Gnomeregan's change line and its announce"
    );
    assert_eq!(world.resource::<ReputationList>().0.len(), listed + 1);
    assert_ne!(
        world.resource::<Reputations>().0[gnomeregan as usize].0 & flag::VISIBLE,
        0
    );
    // Without a reveal the same rise prints (a second row prints no change line).
    standing(&mut world, vec![(gnomeregan, 20), (ironforge, 20000)]);
    assert_eq!(
        lines(&world),
        2 + 2,
        "Gnomeregan's change line, Ironforge's rank"
    );
}

/// Between a pre-player `SMSG_INITIALIZE_FACTIONS` and the UI's world entry (`0x528208`), the
/// store's faction and base columns are still zero: only the list's world entry writes them
/// (`0x5d2d1f`, `0x5d2d6e`, after its player test at `0x5d2be6`). A faction read by id answers the
/// wire standing alone (`0x5d05ea`), a slot names no faction (`0x5d1285`), and a standing packet
/// ranks both reads Neutral and prints no line (`0x5d2161`, `0x5d21c1`). The world entry fills
/// them; the game shutdown zeroes them again (`0x5d15ba`).
#[test]
fn the_slot_columns_are_zero_until_the_world_entry_writes_them() {
    use benilla_formats::faction_flags as flag;
    let Some((mut world, cat)) = standing_world() else {
        return;
    };
    world.insert_resource(FactionSlots::default());
    let stormwind = slot(&cat, "Stormwind");
    world.resource_mut::<Reputations>().0[stormwind as usize] = (flag::VISIBLE, 5100);
    let read = |world: &mut World| {
        with_table(world, |_, t| {
            (
                t.faction(72).map(|e| (e.standing, e.standing_id)),
                t.slot_faction(stormwind).map(|e| e.name),
            )
        })
        .unwrap()
    };
    assert_eq!(
        read(&mut world),
        (Some((5100, 5)), None),
        "the wire alone ranks Friendly, the slot names nothing"
    );

    // 5100 -> 9500 crosses Honored on base 4000; on base 0 both read Neutral at the handler.
    standing(&mut world, vec![(stormwind, 5500)]);
    assert_eq!(lines(&world), 0, "faction 0 prints no change or rank line");
    assert!(!quests_owed(&mut world), "no rank move");
    assert_eq!(
        world.resource::<Reputations>().0[stormwind as usize].1,
        5500
    );

    enter_world(&mut world);
    assert_eq!(
        read(&mut world),
        (Some((9500, 6)), Some("Stormwind".to_string())),
        "base 4000 and the named slot after the world entry"
    );
    assert_eq!(
        world.resource::<FactionSlots>().0[stormwind as usize],
        (72, 4000)
    );

    world.init_resource::<ReputationList>();
    world.run_system_cached(shutdown).unwrap();
    assert!(
        world
            .resource::<FactionSlots>()
            .0
            .iter()
            .all(|&s| s == (0, 0)),
        "zeroed at the game shutdown"
    );
}

fn reasks(world: &World) -> u32 {
    world
        .resource::<crate::ui_quest::QuestStatuses>()
        .reask_epoch()
}

/// The questgivers' status is re-asked by the player's reaction refresh (`0x71f94b`) alone: the
/// standing handler's after a war bit's change or a rank move (`0x5d221c`, `0x5d2256` ->
/// `0x5d22b7`), the at-war handler's after its `UPDATE_FACTION` (`0x5d08c0`), and the at-war set's
/// after its send (`0x5d0b68`); an in-rank change re-asks nothing.
#[test]
fn the_reaction_refresh_reasks_the_questgivers_on_a_war_or_rank_move() {
    use benilla_formats::faction_flags as flag;
    let Some((mut world, cat)) = standing_world() else {
        return;
    };
    let stormwind = slot(&cat, "Stormwind");
    world.resource_mut::<Reputations>().0[stormwind as usize].0 = flag::VISIBLE;
    let before = reasks(&world);
    standing(&mut world, vec![(stormwind, 100)]);
    assert_eq!(reasks(&world), before, "an in-rank change");
    standing(&mut world, vec![(stormwind, 5100)]);
    assert_eq!(reasks(&world), before + 1, "a rank move");
    world
        .run_system_cached_with(
            on_at_war,
            SessionEvent::ReputationAtWar {
                list_id: stormwind,
                at_war: true,
            },
        )
        .unwrap();
    assert_eq!(reasks(&world), before + 2, "the at-war handler");
    with_table(&mut world, |_, t| {
        t.send(benilla_ui::script::ReputationSend::Inactive {
            rep_list_id: stormwind,
            inactive: true,
        })
    });
    assert_eq!(
        reasks(&world),
        before + 2,
        "the inactive set refreshes nothing"
    );
    with_table(&mut world, |_, t| {
        t.send(benilla_ui::script::ReputationSend::AtWar {
            rep_list_id: stormwind,
            at_war: false,
        })
    });
    assert_eq!(reasks(&world), before + 3, "the at-war set");
}
