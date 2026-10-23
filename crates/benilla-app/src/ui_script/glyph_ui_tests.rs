//! The stock 3.3.5a glyph window (`Blizzard_GlyphUI`) on the production load, inside the stock
//! talent window, over the glyph model the live client builds: the install's glyph tables, the
//! six sockets AzerothCore opens at level 80 (`GlyphSlot.dbc` 21-26, all unlocked) and one major
//! glyph inscribed in socket 1 of the active group.

use std::collections::{BTreeSet, HashMap};

use benilla_ui::script::{
    GlyphPlacement, GlyphSockets, GlyphTargeting, ScriptCall, ScriptValue, SpellInfo,
    SpellInfoSource, SpellTooltipView, UiScript, UnitState,
};

use crate::ui_talent::{build_pages, PlayerTalentGroups};

const DWARF: u8 = 3;
const HUNTER: u8 = 3;

/// Errors from the talent and glyph windows' own code.
fn glyph_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| ["Talent", "Glyph", "glyph"].iter().any(|k| e.contains(k)))
        .collect()
}

/// `GetSpellInfo`'s name and rank off the install, which the remove confirm reads.
struct Names(benilla_formats::SpellCatalog);

impl SpellInfoSource for Names {
    fn spell_info(&self, spell_id: u32, _pet: bool) -> Option<SpellInfo> {
        let d = self.0.get(spell_id)?;
        Some(SpellInfo {
            name: d.name.clone(),
            rank: d.rank.clone().unwrap_or_default(),
            ..Default::default()
        })
    }
}

struct Model {
    /// The inscribed major glyph, another major and a minor, by `GlyphProperties.dbc` id.
    major: u32,
    spare_major: u32,
    minor: u32,
    major_spell: u32,
    major_name: String,
}

/// A level 80 hunter's trees and both groups, the glyph tables, the sockets and one glyph.
fn push_model(s: &mut UiScript) -> Model {
    let data = benilla_formats::wow_data().expect("install");
    let mut chain = benilla_formats::open_chain(&data).unwrap();
    let talents = benilla_formats::load_talent_catalog(&mut chain).unwrap();
    let spells = benilla_formats::load_spell_catalog(&mut chain).unwrap();
    let glyphs = benilla_formats::load_glyph_catalog(&mut chain).unwrap();
    let named = |kind: u32| {
        let mut ids: Vec<u32> = glyphs
            .glyphs()
            .filter(|g| g.slot_type == kind && g.icon.is_some() && spells.get(g.spell).is_some())
            .map(|g| g.id)
            .collect();
        ids.sort();
        ids
    };
    let (majors, minors) = (named(0), named(1));
    let major = majors[0];
    let major_spell = glyphs.glyph(major).unwrap().spell;
    let model = Model {
        major,
        spare_major: majors[1],
        minor: minors[0],
        major_spell,
        major_name: spells.get(major_spell).unwrap().name.clone(),
    };

    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            has_object: true,
            name: Some("Claude".into()),
            level: 80,
            health: 100,
            max_health: 100,
            ..Default::default()
        }),
    );
    let mut held = PlayerTalentGroups::fresh(0);
    held.ranks = vec![HashMap::new(), HashMap::new()];
    held.glyphs = vec![[major, 0, 0, 0, 0, 0], [0; 6]];
    s.set_talents(build_pages(
        &talents,
        &BTreeSet::new(),
        &spells,
        DWARF,
        HUNTER,
        (0, 0),
        &held,
    ));
    s.set_glyph_catalog(crate::ui_glyph::catalog_view(&glyphs, &spells));
    s.set_spell_info_source(std::rc::Rc::new(Names(spells.clone())));
    s.set_glyph_sockets(GlyphSockets {
        slots: [21, 22, 23, 24, 25, 26],
        glyphs: [major, 0, 0, 0, 0, 0],
        enabled: 0x3f,
    });
    s.set_spell_tooltip(
        major_spell,
        SpellTooltipView {
            name: model.major_name.clone(),
            description: "Your glyph does a thing.".into(),
            ..Default::default()
        },
    );
    s.fire_event("PLAYER_TALENT_UPDATE", Vec::<ScriptValue>::new());
    model
}

fn open_glyph_tab(s: &mut UiScript) {
    s.run("ToggleTalentFrame()").unwrap();
    s.run(
        r#"
        local tab = _G["PlayerTalentFrameTab" .. GLYPH_TALENT_TAB]
        assert(tab:IsShown(), "level 80 shows the Glyphs tab")
        tab:Click()
        "#,
    )
    .unwrap();
}

/// The Glyphs tab loads `Blizzard_GlyphUI` and shows the sockets as the fields and the group name
/// them: socket 1 holds its glyph's icon, the rest are open, majors and minors by `GlyphSlot.dbc`.
#[test]
fn the_glyphs_tab_shows_the_inscribed_glyph_and_the_open_sockets() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("glyphs", false, "", |_| {});
    let _ = s.take_errors();
    let m = push_model(&mut s);
    open_glyph_tab(&mut s);
    let errors = glyph_errors(s.take_errors());
    assert!(errors.is_empty(), "glyph window errors: {errors:#?}");

    let (enabled, kind, spell, icon): (Option<i64>, i64, i64, String) =
        s.eval("return GetGlyphSocketInfo(1)").unwrap();
    assert_eq!(
        (enabled, kind, spell),
        (Some(1), 1, i64::from(m.major_spell))
    );
    assert!(icon.contains("Glyph"), "a glyph icon: {icon}");
    let second: (Option<i64>, i64, Option<i64>) = s.eval("return GetGlyphSocketInfo(2)").unwrap();
    assert_eq!(second, (Some(1), 2, None), "socket 2 is an open minor");
    s.run(
        r#"
        assert(IsAddOnLoaded("Blizzard_GlyphUI"), "the glyph addon loaded")
        assert(GlyphFrame:IsShown(), "the glyph frame is up")
        assert(GlyphFrameGlyph1.glyph:IsShown(), "socket 1 draws its glyph")
        assert(not GlyphFrameGlyph2.glyph:IsShown(), "socket 2 is empty")
        assert(GlyphFrameGlyph2.glyphType == GLYPHTYPE_MINOR, "socket 2 is a minor")
        assert(GetNumGlyphSockets() == 6)
        assert(GetGlyphSocketInfo(7) == nil and GetGlyphSocketInfo(0) == nil)
        assert(GetGlyphSocketInfo(1, 2) == 1, "group 2's socket 1 is unlocked")
        assert(select(3, GetGlyphSocketInfo(1, 2)) == nil, "and empty")
        assert(GetGlyphSocketInfo(1, 3) == nil, "no group 3")
        "#,
    )
    .unwrap();
    let link: String = s.eval("return GetGlyphLink(1)").unwrap();
    assert_eq!(
        link,
        format!("|cff66bbff|Hglyph:21:{}|h[{}]|h|r", m.major, m.major_name)
    );
    assert_eq!(s.eval::<String>("return GetGlyphLink(2)").unwrap(), "");

    // The hover: name, type, description and the remove hint; an open minor's hint.
    let lines = |s: &mut UiScript, frame: &str| -> Vec<String> {
        s.run(&format!(
            "GlyphFrameGlyph_OnEnter({frame}) GameTooltip:Show()"
        ))
        .unwrap();
        s.eval::<Vec<String>>(
            "local t = {} for i = 1, GameTooltip:NumLines() do \
             t[i] = _G['GameTooltipTextLeft' .. i]:GetText() end return t",
        )
        .unwrap()
    };
    assert_eq!(
        lines(&mut s, "GlyphFrameGlyph1"),
        [
            m.major_name.as_str(),
            "Major Glyph",
            "Your glyph does a thing.",
            "<Shift Right Click to Remove>"
        ]
    );
    assert_eq!(
        lines(&mut s, "GlyphFrameGlyph2"),
        [
            "Empty",
            "Minor Glyph",
            "Use a Glyph from your inventory to inscribe your spellbook."
        ]
    );
    let errors = glyph_errors(s.take_errors());
    assert!(errors.is_empty(), "glyph window errors: {errors:#?}");
}

/// Shift-right-click on the inscribed socket asks `CONFIRM_REMOVE_GLYPH` with the glyph's name,
/// and Accept queues `CMSG_REMOVE_GLYPH` for socket 0; the field edge's `GLYPH_REMOVED` then
/// empties the socket.
#[test]
fn a_shift_right_click_removes_the_glyph_after_the_confirm() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("glyphs", false, "", |_| {});
    let _ = s.take_errors();
    let m = push_model(&mut s);
    open_glyph_tab(&mut s);
    let _ = s.take_glyph_removes();

    s.set_modifiers(true, false, false);
    s.run(r#"GlyphFrameGlyph1:Click("RightButton")"#).unwrap();
    s.set_modifiers(false, false, false);
    let text: String = s.eval("return StaticPopup1Text:GetText()").unwrap();
    assert!(
        text.contains(&m.major_name),
        "the confirm names the glyph: {text}"
    );
    assert!(s.take_glyph_removes().is_empty(), "nothing before Accept");
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert_eq!(s.take_glyph_removes(), vec![0]);

    // An unmodified right-click asks nothing.
    s.run(r#"GlyphFrameGlyph1:Click("RightButton")"#).unwrap();
    assert!(!s
        .eval::<bool>("return StaticPopup1:IsShown() == 1")
        .unwrap());

    // The server's answer: the group empties, then the field edge fires the watcher's event.
    let mut held = PlayerTalentGroups::fresh(0);
    held.ranks = vec![HashMap::new(), HashMap::new()];
    held.glyphs = vec![[0; 6], [0; 6]];
    let data = benilla_formats::wow_data().expect("install");
    let mut chain = benilla_formats::open_chain(&data).unwrap();
    let talents = benilla_formats::load_talent_catalog(&mut chain).unwrap();
    let spells = benilla_formats::load_spell_catalog(&mut chain).unwrap();
    s.set_talents(build_pages(
        &talents,
        &BTreeSet::new(),
        &spells,
        DWARF,
        HUNTER,
        (0, 0),
        &held,
    ));
    s.fire_event("GLYPH_REMOVED", vec![ScriptValue::Int(1)]);
    assert!(s
        .eval::<bool>("return not GlyphFrameGlyph1.glyph:IsShown()")
        .unwrap());
    let errors = glyph_errors(s.take_errors());
    assert!(errors.is_empty(), "glyph window errors: {errors:#?}");
}

/// With a glyph item's cast targeting, the open sockets of its type match and a click places it:
/// the matching socket queues the bind, a full or mismatched socket the reference's refusals.
#[test]
fn a_targeting_glyph_matches_its_sockets_and_a_click_places_it() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("glyphs", false, "", |_| {});
    let _ = s.take_errors();
    let m = push_model(&mut s);
    open_glyph_tab(&mut s);
    let _ = s.take_script_calls();

    let arm = |s: &mut UiScript, glyph: u32| {
        s.set_spell_targeting(true);
        s.set_glyph_targeting(Some(GlyphTargeting {
            glyphs: [Some(glyph), None, None],
            wants_socket: true,
        }));
    };
    arm(&mut s, m.spare_major);
    s.run(
        r#"
        assert(SpellCanTargetGlyph() == 1)
        assert(GlyphMatchesSocket(4) == 1, "socket 4 is a major")
        assert(GlyphMatchesSocket(2) == nil, "socket 2 is a minor")
        GlyphFrameGlyph4:Click("LeftButton")
        "#,
    )
    .unwrap();
    assert_eq!(
        s.take_script_calls(),
        vec![ScriptCall::PlaceGlyph {
            socket: 3,
            verdict: GlyphPlacement::Bind
        }]
    );
    // A minor into a major socket, the inscribed glyph again, a locked socket.
    arm(&mut s, m.minor);
    s.run("PlaceGlyphInSocket(6)").unwrap();
    arm(&mut s, m.major);
    s.run("PlaceGlyphInSocket(4)").unwrap();
    s.set_glyph_sockets(GlyphSockets {
        slots: [21, 22, 23, 24, 25, 26],
        glyphs: [m.major, 0, 0, 0, 0, 0],
        enabled: 0x07,
    });
    arm(&mut s, m.spare_major);
    s.run("PlaceGlyphInSocket(6)").unwrap();
    let verdicts: Vec<_> = s
        .take_script_calls()
        .into_iter()
        .filter_map(|c| match c {
            ScriptCall::PlaceGlyph { socket, verdict } => Some((socket, verdict)),
            _ => None,
        })
        .collect();
    assert_eq!(
        verdicts,
        [
            (
                5,
                GlyphPlacement::Fail(benilla_ui::script::GLYPH_FAILED_INVALID)
            ),
            (
                3,
                GlyphPlacement::Fail(benilla_ui::script::GLYPH_FAILED_UNIQUE)
            ),
            (
                5,
                GlyphPlacement::Fail(benilla_ui::script::GLYPH_FAILED_LOCKED)
            ),
        ]
    );
    let errors = glyph_errors(s.take_errors());
    assert!(errors.is_empty(), "glyph window errors: {errors:#?}");
}
