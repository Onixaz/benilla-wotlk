//! The retrieving tooltip: a template in flight shows `RETRIEVING_ITEM_INFO` in red and shown
//! (`0x623760`), and the record's arrival repaints the tooltip with no new `Set*` call
//! (`0x626650`).

use super::{axe, lines_of, script};
use crate::script::*;

const RED: [f32; 4] = [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0];

/// The fixture with `RETRIEVING_ITEM_INFO` and a shown tooltip `TT` owned by a button.
fn retrieving_script() -> UiScript {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        RETRIEVING_ITEM_INFO = "Retrieving item information"
        local a = CreateFrame("Button", "Owner"); a:SetPoint("CENTER", 0, 0)
        a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "TT"):SetOwner(a, "ANCHOR_RIGHT")
        "#,
    )
    .unwrap();
    s
}

fn shown(s: &mut UiScript) -> bool {
    s.eval::<bool>("return TT:IsShown() == 1").unwrap()
}

/// A miss shows exactly the red retrieving line, and the plate shows.
#[test]
fn a_miss_shows_the_retrieving_line() {
    let mut s = retrieving_script();
    s.run("TT:SetHyperlink('item:19019:0:0:0')").unwrap();
    assert_eq!(
        lines_of(&mut s),
        vec![("Retrieving item information".to_string(), RED)]
    );
    assert!(shown(&mut s));
}

/// The template's arrival repaints both link shapes, bare and full, with no second call.
#[test]
fn the_arrival_repaints_a_hyperlink() {
    for link in [
        "item:19019:0:0:0",
        "|cffff8000|Hitem:19019:0:0:0|h[Ravager]|h|r",
    ] {
        let mut s = retrieving_script();
        s.run(&format!("TT:SetHyperlink({link:?})")).unwrap();
        assert_eq!(lines_of(&mut s).len(), 1, "{link}: the retrieving line");
        s.set_item_template(19019, axe());
        let lines = lines_of(&mut s);
        assert_eq!(
            lines[0].0, "Ravager",
            "{link}: repainted without a new call"
        );
        assert!(lines.len() > 1, "{link}: the full tooltip");
        assert!(shown(&mut s));
    }
}

/// A quest reward has no `UpdateTooltip` loop: only the arrival can repaint it.
#[test]
fn the_arrival_repaints_a_quest_item() {
    let mut s = retrieving_script();
    s.set_quest(Some(QuestState {
        rewards: vec![QuestItemView {
            item_id: 19019,
            count: 1,
            ..Default::default()
        }],
        ..Default::default()
    }));
    s.run("TT:SetQuestItem('reward', 1)").unwrap();
    assert_eq!(
        lines_of(&mut s),
        vec![("Retrieving item information".to_string(), RED)]
    );
    s.set_item_template(19019, axe());
    assert_eq!(lines_of(&mut s)[0].0, "Ravager");
}

/// A tooltip set to another item in between is not repainted with the old one.
#[test]
fn a_tooltip_set_to_another_item_is_not_repainted_with_the_old() {
    let mut s = retrieving_script();
    let mut ring = axe();
    ring.name = "Band".into();
    s.set_item_template(2000, ring);
    s.run("TT:SetHyperlink('item:19019:0:0:0')").unwrap();
    s.run("TT:SetHyperlink('item:2000:0:0:0')").unwrap();
    s.set_item_template(19019, axe());
    assert_eq!(lines_of(&mut s)[0].0, "Band");

    // Nor is a tooltip given text, or hidden, since.
    s.run("TT:SetHyperlink('item:3000:0:0:0') TT:SetText('Hello')")
        .unwrap();
    s.set_item_template(3000, axe());
    assert_eq!(lines_of(&mut s)[0].0, "Hello");
    s.run("TT:SetHyperlink('item:4000:0:0:0') TT:Hide()")
        .unwrap();
    s.set_item_template(4000, axe());
    assert!(!shown(&mut s), "a hidden tooltip stays hidden");
}

/// A secondary record the render read, an item set, repaints when it lands.
#[test]
fn an_item_set_landing_repaints() {
    let mut s = retrieving_script();
    let mut vest = axe();
    vest.item_set = 161;
    s.set_item_template(6303, vest);
    s.run("TT:SetHyperlink('item:6303:0:0:0')").unwrap();
    let before = lines_of(&mut s).len();
    s.set_item_set(
        161,
        ItemSetView {
            name: "Defias Leather".into(),
            members: vec![(6303, Some("Defias Mark".into()))],
            ..Default::default()
        },
    );
    let after = lines_of(&mut s);
    assert!(after.len() > before, "{before} lines, then {after:?}");
    assert!(after.iter().any(|(t, _)| t.contains("Defias Leather")));
}
