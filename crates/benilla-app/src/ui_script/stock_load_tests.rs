//! The whole stock interface loads off the player's chain with no raise: every `OnLoad` and
//! load-time handler of `FrameXML.toc` runs to its end, so no frame is left half set up.

use crate::local_state::test_env::ENV_LOCK;

/// The raises whose frame waits on a port of its own, each by the frame that raises and its issue.
const UNPORTED: [(&str, &str); 3] = [
    ("TutorialFrame: OnLoad", "#290, animation groups"),
    ("MiniMapTrackingDropDown: OnLoad", "#424, minimap tracking"),
    ("ChannelFrame: OnLoad", "#470, the channel pane"),
];

/// The core alone, a level 60 human warrior seated as the in-game load always has one.
#[test]
fn the_stock_interface_loads_without_a_raise() {
    benilla_formats::wow_data_or_skip!();
    let _l = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_s, failures) = super::layer_tests::production_load_seeded(
        "stock-census",
        true,
        "",
        |_| {},
        |s| {
            // The character-select record `UnitRace("player")` reads, set before the UI loads.
            s.set_player_record(benilla_ui::script::PlayerRecord {
                name: "Probesix".into(),
                race: Some(("Human".into(), "Human".into())),
                class: Some(("Warrior".into(), "WARRIOR".into())),
                sex: 2,
            });
            // The display's mode list, which the app pushes before the load; `UpdateMenuBarTop`
            // divides the current one's sides.
            let mode = benilla_ui::script::ScreenResolution {
                width: 1024,
                height: 768,
            };
            s.set_screen_resolutions(vec![mode], Some(mode));
            s.set_unit(
                "player",
                Some(benilla_ui::script::UnitState {
                    exists: true,
                    name: Some("Probesix".into()),
                    level: 60,
                    race: Some("Human".into()),
                    race_file: Some("Human".into()),
                    class: Some("Warrior".into()),
                    class_file: Some("WARRIOR".into()),
                    ..Default::default()
                }),
            );
        },
    );
    let unported: Vec<_> = failures
        .iter()
        .filter(|f| !UNPORTED.iter().any(|(at, _)| f.contains(at)))
        .collect();
    assert!(unported.is_empty(), "stock load raised: {unported:#?}");
    for (at, issue) in UNPORTED {
        assert!(
            failures.iter().any(|f| f.contains(at)),
            "{at} loads now: drop its row ({issue})"
        );
    }
}
