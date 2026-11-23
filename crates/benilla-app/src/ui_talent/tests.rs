use super::*;
use benilla_protocol::field::{FIELD_PLAYER_CHARACTER_POINTS1, FIELD_PLAYER_CHARACTER_POINTS2};
use benilla_ui::script::{LiveState, TalentSide};

/// A view lending a talent state alone, for the stock window's tests: the talent unit per side
/// `(player, pet)`, the pet's family category, and the learns sent in the call.
#[derive(Default)]
pub(crate) struct TalentTestView {
    pub(crate) info: TalentInfo,
    pub(crate) units: (Option<u64>, Option<u64>),
    pub(crate) pet_category: Option<i32>,
    /// Our `PLAYER_CHARACTER_POINTS1/2`.
    pub(crate) points: Option<(u32, u32)>,
    pub(crate) learns: Vec<(Option<u64>, u32, u32)>,
    pub(crate) preview_learns: Vec<benilla_ui::script::PreviewLearn>,
    /// The glyph tables, our glyph fields, the targeting cast's glyphs and the removes sent.
    pub(crate) glyph_catalog: benilla_ui::script::GlyphCatalogView,
    pub(crate) glyph_sockets: Option<benilla_ui::script::GlyphSockets>,
    pub(crate) glyph_targeting: Option<benilla_ui::script::GlyphTargeting>,
    pub(crate) glyph_removes: Vec<u32>,
}

impl LiveState for TalentTestView {
    fn unit_flags(&self, _guid: u64) -> Option<u32> {
        None
    }

    fn unit_health(&self, _guid: u64) -> Option<u32> {
        None
    }

    fn active_player(&self) -> Option<u64> {
        self.units.0
    }

    fn has_control(&self) -> bool {
        true
    }

    fn far_sight(&self) -> bool {
        false
    }

    fn talent_info(&self) -> Option<&TalentInfo> {
        Some(&self.info)
    }

    fn talent_info_mut(&mut self) -> Option<&mut TalentInfo> {
        Some(&mut self.info)
    }

    fn talent_unit(&self, pet: bool) -> Option<u64> {
        if pet {
            self.units.1
        } else {
            self.units.0
        }
    }

    fn pet_family_category(&self) -> Option<i32> {
        self.pet_category
    }

    fn character_points(&self) -> Option<(u32, u32)> {
        self.points
    }

    fn send_learn_talent(&mut self, pet: Option<u64>, talent: u32, rank: u32) {
        self.learns.push((pet, talent, rank));
    }

    fn send_learn_preview_talents(&mut self, pet: Option<u64>, talents: Vec<(u32, u32)>) {
        self.preview_learns.push((pet, talents));
    }

    fn glyph(&self, id: u32) -> Option<benilla_ui::script::GlyphView> {
        self.glyph_catalog.glyph(id).cloned()
    }

    fn glyph_slot(&self, id: u32) -> Option<benilla_ui::script::GlyphSlotView> {
        self.glyph_catalog.slot(id).copied()
    }

    fn glyph_sockets(&self) -> Option<benilla_ui::script::GlyphSockets> {
        self.glyph_sockets
    }

    fn glyph_targeting(&self) -> Option<benilla_ui::script::GlyphTargeting> {
        self.glyph_targeting
    }

    fn send_remove_glyph(&mut self, socket: u32) {
        self.glyph_removes.push(socket);
    }

    /// The spell targeting the test sets on this thread, as the fixed view answers it.
    fn spell_targeting(&self) -> benilla_ui::script::SpellTargetingLive {
        crate::ui_script::test_live::View.spell_targeting()
    }
}

/// One body group: its `(talentId, rank)`s and its glyphs.
pub(crate) type BodyGroup = (Vec<(u32, u8)>, Vec<u16>);

/// A player body of `groups`, each a list of `(talentId, rank)` with its glyphs.
pub(crate) fn body(unspent: u32, active: u8, groups: Vec<BodyGroup>) -> TalentPacket {
    TalentPacket {
        unspent,
        active,
        groups: groups
            .into_iter()
            .map(|(talents, glyphs)| TalentPacketGroup { talents, glyphs })
            .collect(),
    }
}

/// The install's catalogs.
pub(crate) fn catalogs() -> (TalentCatalog, benilla_formats::SpellCatalog) {
    let data = benilla_formats::wow_data().expect("install");
    let mut chain = benilla_formats::open_chain(&data).unwrap();
    (
        benilla_formats::load_talent_catalog(&mut chain).unwrap(),
        benilla_formats::load_spell_catalog(&mut chain).unwrap(),
    )
}

/// The packet readers' rank check over the install's catalogs (`0x5c9364`).
pub(crate) fn valid_over<'a>(
    catalog: &'a TalentCatalog,
    spells: &'a benilla_formats::SpellCatalog,
) -> impl Fn(u32, i32) -> bool + 'a {
    move |id, rank| {
        catalog
            .talent_by_id(id)
            .and_then(|t| t.ranks.get(usize::try_from(rank).ok()?).copied())
            .is_some_and(|s| s != 0 && spells.get(s).is_some())
    }
}

#[test]
fn an_inactive_groups_unspent_is_the_total_less_its_spent() {
    benilla_formats::wow_data_or_skip!();
    let (catalog, spells) = catalogs();
    const HUMAN: u8 = 1;
    const MAGE: u8 = 8;
    let pages = player_pages(&catalog, &spells, HUMAN, MAGE);
    let first = pages.talents[0][0].id;
    let mut info = TalentInfo::default();
    // Active group 1 holds 2 points in the first Arcane talent, 2 unspent: 4 in all. Group 2
    // holds 1 point, so it has 3 to spend.
    info.update_talent_data(
        &body(
            2,
            0,
            vec![(vec![(first, 1)], vec![]), (vec![(first, 0)], vec![])],
        ),
        &valid_over(&catalog, &spells),
    );
    info.enter_world(|| Some(pages));
    assert_eq!(info.pages(TalentSide::Player).tabs.len(), 3);
    let g = |i| info.group(TalentSide::Player, i).unwrap();
    assert_eq!(g(0).tab(0).map(|t| t.spent), Some(2));
    assert_eq!((g(0).unspent, g(1).unspent), (2, 3));
    assert!(info
        .pages(TalentSide::Player)
        .tabs
        .iter()
        .all(|t| t.icon.is_some()));
}

/// A rank naming no rank spell is dropped by the reader (`0x5c9364`): a talent id no row has, or
/// a rank past the talent's ranks.
#[test]
fn a_rank_with_no_spell_is_dropped_by_the_reader() {
    benilla_formats::wow_data_or_skip!();
    let (catalog, spells) = catalogs();
    let pages = player_pages(&catalog, &spells, 1, 8);
    let first = &pages.talents[0][0];
    let past = first.ranks.len() as u8;
    let mut info = TalentInfo::default();
    info.update_talent_data(
        &body(0, 0, vec![(vec![(first.id, past), (999_999, 0)], vec![])]),
        &valid_over(&catalog, &spells),
    );
    let g = info.group(TalentSide::Player, 0).unwrap();
    assert!(g.item(first.id).is_none() && g.item(999_999).is_none());
}

/// A wolf's tree: its family's talent type picks the tabs (`0x5c64d0`), and a family with no row
/// builds none, leaving the tabs as they were (`0x5c6526`).
#[test]
fn a_wolf_gets_its_familys_tree() {
    benilla_formats::wow_data_or_skip!();
    let (catalog, spells) = catalogs();
    let data = benilla_formats::wow_data().unwrap();
    let mut chain = benilla_formats::open_chain(&data).unwrap();
    let families = benilla_formats::load_creature_families(&mut chain).unwrap();
    let wolf = pet_pages(&catalog, &spells, &families, 1).expect("the wolf has a tree");
    assert_eq!(wolf.tabs.len(), 1);
    assert!(wolf.talents[0].iter().any(|t| t.category_mask != [0, 0]));
    assert!(pet_pages(&catalog, &spells, &families, 9999).is_none());
    let mut info = TalentInfo::default();
    info.update_pet_tabs(1, || Some(wolf.clone()));
    info.update_pet_tabs(9999, || None);
    assert_eq!(
        info.pages(TalentSide::Pet).tabs.len(),
        1,
        "a family with no tree keeps the tabs"
    );
    info.update_pet_tabs(0, || unreachable!("no family builds nothing"));
    assert!(info.pages(TalentSide::Pet).tabs.is_empty());
}

/// The points watch's deltas: the value now less the old one, signed; a point the update left
/// reads no move (`0x5e8330`).
#[test]
fn the_character_points_deltas_are_now_less_old() {
    assert!(is_character_points_watch(FIELD_PLAYER_CHARACTER_POINTS1));
    assert!(is_character_points_watch(FIELD_PLAYER_CHARACTER_POINTS2));
    assert!(!is_character_points_watch(
        FIELD_PLAYER_CHARACTER_POINTS1 - 1
    ));
    let edges = [(FIELD_PLAYER_CHARACTER_POINTS1, 3, 4)];
    assert_eq!(character_points_deltas(&edges, (4, 2)), (1, 0));
    let edges = [
        (FIELD_PLAYER_CHARACTER_POINTS1, 4, 0),
        (FIELD_PLAYER_CHARACTER_POINTS2, 0, 2),
    ];
    assert_eq!(character_points_deltas(&edges, (0, 2)), (-4, 2));
}

/// A logout then a login rebuilds the player's tabs: the UI shutdown's world leave unbuilds them
/// (`0x52901a` -> `0x528d4b`) ahead of the game shutdown (`0x5291d6`), so the entry builds again
/// (`0x528235` -> `0x5c9d50`).
#[test]
fn a_login_after_a_logout_rebuilds_the_tabs() {
    use bevy::ecs::system::RunSystemOnce;
    let one_tab = || TalentPages {
        tabs: vec![TalentTabView {
            id: 41,
            ..Default::default()
        }],
        talents: vec![vec![]],
    };
    let mut world = World::new();
    let mut info = TalentInfo::default();
    info.enter_world(|| Some(one_tab()));
    info.set_inspect_target(7, Some(one_tab()));
    world.insert_resource(TalentState(info));
    leave_world(&mut world);
    world.run_system_once(shutdown).unwrap();
    let state = &mut world.resource_mut::<TalentState>().0;
    assert_eq!(*state, TalentInfo::default(), "nothing outlives the logout");
    state.enter_world(|| Some(one_tab()));
    assert_eq!(state.pages(TalentSide::Player).tabs.len(), 1);
}

/// A prerequisite slot names the spell at the needed rank (`0x622585`), and that spell's name is
/// the tooltip's requirement line.
#[test]
fn a_prerequisite_names_the_spell_at_the_needed_rank() {
    benilla_formats::wow_data_or_skip!();
    let (catalog, spells) = catalogs();
    let pages = player_pages(&catalog, &spells, 1, 8);
    let prereqs: Vec<_> = pages
        .talents
        .iter()
        .flatten()
        .flat_map(|t| &t.prereqs)
        .collect();
    assert!(!prereqs.is_empty());
    for p in prereqs {
        let rec = catalog.talent_by_id(p.talent).unwrap();
        assert_eq!(p.spell, rec.ranks[p.rank as usize], "talent {}", p.talent);
        assert_eq!(
            p.name.as_deref(),
            spells.get(p.spell).map(|d| d.name.as_str())
        );
        assert!(p.name.is_some());
    }
}
