//! The stock 3.3.5a achievement toast (`AlertFrames.xml`/`.lua`) on the production load: its
//! `animIn` and `waitAndAnimOut` animation groups fade it in, hold it and hide it.

use std::sync::Arc;

use benilla_ui::script::{AchievementBook, AchievementDb};

fn alert_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| ["Alert", "anim", "Anim"].iter().any(|k| e.contains(k)))
        .collect()
}

/// Two alerts shown 0.3 s apart each fade in over 0.2 s, hold, fade out over 1.5 s after their
/// 4.05 s wait, and hide themselves from `animOut`'s `OnFinished`, on simulated time.
#[test]
fn two_achievement_toasts_fade_in_hold_and_hide() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let data = benilla_formats::wow_data().expect("install");
    let mut chain = benilla_formats::open_chain(&data).unwrap();
    let catalog = benilla_formats::load_achievement_catalog(&mut chain).unwrap();
    let ids: Vec<u32> = catalog
        .achievements
        .iter()
        .filter(|a| a.faction == -1 && a.category > 0 && a.points > 0)
        .take(2)
        .map(|a| a.id)
        .collect();
    let db: Arc<AchievementDb> = Arc::new(crate::ui_achievement::db_of(catalog));
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("alerts", false, "", |_| {});
    s.set_achievement_db(db);
    s.set_achievement_book(AchievementBook::default());
    s.resolve();
    let _ = s.take_errors();

    let alpha = |s: &benilla_ui::script::UiScript, n: u32| {
        s.eval::<f64>(&format!("return AchievementAlertFrame{n}:GetAlpha()"))
            .unwrap()
    };
    let shown = |s: &benilla_ui::script::UiScript, n: u32| {
        s.eval::<bool>(&format!(
            "return AchievementAlertFrame{n} ~= nil and AchievementAlertFrame{n}:IsShown() == 1"
        ))
        .unwrap()
    };
    // The stock `ACHIEVEMENT_EARNED` arm loads the window addon first (`AlertFrames.lua:12-14`).
    s.run("AchievementFrame_LoadUI()").unwrap();
    s.run(&format!("AchievementAlertFrame_ShowAlert({})", ids[0]))
        .unwrap();
    let errors = alert_errors(s.take_errors());
    assert!(errors.is_empty(), "alert errors: {errors:#?}");
    assert!(shown(&s, 1));
    s.tick(0.05);
    let early = alpha(&s, 1);
    assert!(early < 0.5, "fading in from 0, got {early}");
    s.tick(0.25);
    s.run(&format!("AchievementAlertFrame_ShowAlert({})", ids[1]))
        .unwrap();
    assert!(shown(&s, 2), "the second toast takes the second frame");
    s.tick(0.5);
    assert_eq!(alpha(&s, 1), 1.0, "faded in and holding");

    // 0.8 s in: the first starts its fade at 4.05 s, gone by 5.55 s; the second 0.3 s later.
    let mut t = 0.8;
    let mut hid = [None, None];
    while t < 8.0 {
        s.tick(0.1);
        t += 0.1;
        for (i, n) in [1, 2].into_iter().enumerate() {
            if hid[i].is_none() && !shown(&s, n) {
                hid[i] = Some(t);
            }
        }
    }
    let errors = alert_errors(s.take_errors());
    assert!(errors.is_empty(), "alert errors: {errors:#?}");
    let (a, b) = (hid[0].expect("toast 1 hid"), hid[1].expect("toast 2 hid"));
    assert!((5.5..5.8).contains(&a), "toast 1 hid at {a:.2}");
    assert!((5.8..6.1).contains(&b), "toast 2 hid at {b:.2}");
    assert_eq!(
        alpha(&s, 1),
        1.0,
        "the saved alpha is back on the hidden frame"
    );
}
