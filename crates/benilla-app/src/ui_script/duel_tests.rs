//! The stock duel popups, driven by the four events `ui_duel`'s feed fires (`UIParent.lua:378`).

use benilla_ui::script::{DuelRequest, ScriptValue, UiScript};

/// The production interface load, the popups' own file chain in `FrameXML.toc` order, with the
/// environment lock the load needs held for the test.
fn setup() -> (std::sync::MutexGuard<'static, ()>, UiScript) {
    let lock = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, failures) =
        super::layer_tests::production_load_with("duel", false, "", |_| {});
    let ours: Vec<_> = failures
        .iter()
        .filter(|f| f.contains("StaticPopup"))
        .collect();
    assert!(ours.is_empty(), "load failures: {ours:#?}");
    s.resolve();
    s.take_errors();
    s.take_duel_requests();
    (lock, s)
}

#[test]
fn the_challenge_popup_shows_the_name_and_accept_queues_the_accept() {
    benilla_formats::wow_data_or_skip!();
    let (_lock, mut s) = setup();
    s.fire_event(
        "DUEL_REQUESTED",
        vec![ScriptValue::Str("Onerogue".to_string())],
    );
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "DUEL_REQUESTED shows the challenge popup"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Onerogue has challenged you to a duel.",
        "arg1 fills the GlobalStrings template's %s"
    );
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_duel_requests(), vec![DuelRequest::Accept]);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "accepting closes it"
    );
}

/// Escape runs a `hideOnEscape` popup's OnCancel (`StaticPopup.lua:1884`), so it declines too.
#[test]
fn decline_and_escape_both_cancel() {
    benilla_formats::wow_data_or_skip!();
    let (_lock, mut s) = setup();
    s.fire_event("DUEL_REQUESTED", vec![ScriptValue::Str("Twomage".into())]);
    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    assert_eq!(s.take_duel_requests(), vec![DuelRequest::Cancel]);

    s.fire_event("DUEL_REQUESTED", vec![ScriptValue::Str("Twomage".into())]);
    s.run("ToggleGameMenu()").unwrap();
    assert_eq!(s.take_duel_requests(), vec![DuelRequest::Cancel]);
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
}

/// `DUEL_OUTOFBOUNDS` has no buttons and a 10 s timeout; `StaticPopup_OnUpdate` renders the count.
#[test]
fn out_of_bounds_counts_down_and_inbounds_dismisses_it() {
    benilla_formats::wow_data_or_skip!();
    let (_lock, mut s) = setup();
    s.fire_event("DUEL_OUTOFBOUNDS", vec![]);
    assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    assert!(
        !s.eval::<bool>("return StaticPopup1Button1:IsShown()")
            .unwrap(),
        "a warning, not a question — no buttons"
    );
    s.tick(0.05);
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Exiting duel area, you will forfeit in 10 Seconds.",
        "the engine's countdown branch fills %d %s"
    );
    s.fire_event("DUEL_INBOUNDS", vec![]);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "coming back inside clears the warning"
    );
    assert!(
        s.take_duel_requests().is_empty(),
        "the bounds pair never sends anything"
    );
}

/// `DUEL_FINISHED` hides both popups (`UIParent.lua:390`); a hide runs no OnCancel, so no decline.
#[test]
fn finishing_sweeps_both_dialogs_silently() {
    benilla_formats::wow_data_or_skip!();
    let (_lock, mut s) = setup();
    s.fire_event("DUEL_REQUESTED", vec![ScriptValue::Str("Twomage".into())]);
    assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    s.fire_event("DUEL_FINISHED", vec![]);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "the challenge popup goes away with the duel"
    );
    assert!(
        s.take_duel_requests().is_empty(),
        "a hide is not a decline — the duel is already over"
    );

    s.fire_event("DUEL_OUTOFBOUNDS", vec![]);
    s.fire_event("DUEL_FINISHED", vec![]);
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    assert!(s.take_duel_requests().is_empty());
}

/// The three duel verbs. `StartDuel` resolves a token to a player's guid, reads no argument as the
/// target, drops a token naming no player, and passes anything else on as a name.
#[test]
fn the_duel_globals_queue_their_intents() {
    const PLAYER: u64 = 0x22;
    let mut s = UiScript::new().unwrap();
    s.set_unit_guids(&benilla_ui::script::UnitGuids {
        player: 0x10,
        target: PLAYER,
        ..Default::default()
    });
    s.run(
        "AcceptDuel() CancelDuel() StartDuel('Onerogue') StartDuel('target', 1) StartDuel()          StartDuel('party1')",
    )
    .unwrap();
    assert_eq!(
        s.take_duel_requests(),
        vec![
            DuelRequest::Accept,
            DuelRequest::Cancel,
            DuelRequest::ChallengeName {
                name: "Onerogue".into(),
                exact: false,
            },
            DuelRequest::ChallengeGuid(PLAYER),
            DuelRequest::ChallengeGuid(PLAYER),
        ]
    );
    s.set_unit_guids(&benilla_ui::script::UnitGuids {
        player: 0x10,
        target: 0xF130_0000_0000_0001,
        ..Default::default()
    });
    s.run("StartDuel('target', 1)").unwrap();
    assert!(s.take_duel_requests().is_empty(), "a creature is no duel target");
}
