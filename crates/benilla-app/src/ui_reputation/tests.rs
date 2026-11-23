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
        let row = reputation_row(id, info, &cat, 0, 0, 1, 8)
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
            reputation_row(id, info, &cat, flags, 0, race, class)
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

/// A world with the install's faction table and a level-80 Human Warrior held, no VM: the lines
/// fall to the chat feed's queue, where they are counted.
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
