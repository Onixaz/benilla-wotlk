//! The stock 3.3.5a achievement window (`Blizzard_AchievementUI`) on the production load, over a
//! book the live client builds: the install's tables and a login list with one completed
//! achievement and one criterion in progress, as `SMSG_ALL_ACHIEVEMENT_DATA` would carry them.

use std::sync::Arc;

use benilla_ui::script::{
    AchievementBook, AchievementDb, AchievementUpdate, CriteriaWire, WowDate,
};

/// Errors from the achievement window's own code; the production load carries unrelated gaps.
fn achievement_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| e.contains("Achievement"))
        .collect()
}

/// A listed achievement with points (`earned`), and one whose single visible criterion draws a
/// progress bar of more than one step (`bar`), both chosen from the install's tables.
fn pick(db: &benilla_formats::AchievementCatalog) -> (u32, u32, u32, u32) {
    let listed = |a: &&benilla_formats::Achievement| {
        a.faction == -1 && a.flags & 0x8a3 == 0 && a.category > 0 && a.category != 81
    };
    let earned = db
        .achievements
        .iter()
        .filter(listed)
        .find(|a| a.points > 0 && a.supercedes == 0)
        .expect("an achievement with points")
        .id;
    let (bar, criterion, quantity) = db
        .achievements
        .iter()
        .filter(listed)
        .filter(|a| a.id != earned && a.shares_criteria == 0)
        .find_map(|a| {
            let mut own = db
                .criteria
                .iter()
                .filter(|c| c.achievement == a.id && c.flags & 2 == 0);
            match (own.next(), own.next()) {
                (Some(c), None) if c.flags & 1 != 0 && c.quantity > 2 => {
                    Some((a.id, c.id, c.quantity))
                }
                _ => None,
            }
        })
        .expect("a one-criterion progress-bar achievement");
    (earned, bar, criterion, quantity)
}

#[test]
fn the_stock_achievement_window_draws_the_book_and_moves_a_progress_bar() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let data = benilla_formats::wow_data().expect("install");
    let mut chain = benilla_formats::open_chain(&data).unwrap();
    let catalog = benilla_formats::load_achievement_catalog(&mut chain).unwrap();
    let (earned, bar, criterion, quantity) = pick(&catalog);
    let db: Arc<AchievementDb> = Arc::new(crate::ui_achievement::db_of(catalog));

    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("achievements", false, "", |_| {});
    let _ = s.take_errors();
    s.set_achievement_db(db.clone());
    s.set_achievement_book(AchievementBook::default());
    let step = |counter: u64| CriteriaWire {
        id: criterion,
        counter,
        date: WowDate::from_date(0, 1, 9).0,
        ..Default::default()
    };
    s.apply_achievement(AchievementUpdate::List {
        achievements: vec![(earned, WowDate::from_date(0, 2, 9).0)],
        criteria: vec![step(1)],
        now: 0,
    });

    s.run("ToggleAchievementFrame()").unwrap();
    let errors = achievement_errors(s.take_errors());
    assert!(errors.is_empty(), "achievement window errors: {errors:#?}");
    s.run(
        r#"
        assert(IsAddOnLoaded("Blizzard_AchievementUI") == 1, "the addon loaded")
        assert(AchievementFrame:IsShown(), "the window is open")
        local points = AchievementFrameHeaderPoints:GetText()
        assert(tonumber(points) == GetTotalAchievementPoints() and tonumber(points) > 0,
            "the header reads the held points, got " .. tostring(points))
        local total, completed = GetNumCompletedAchievements()
        local bar = AchievementFrameSummaryCategoriesStatusBarText:GetText()
        assert(bar == completed .. "/" .. total and completed == 1, "summary bar " .. tostring(bar))
        local latest = AchievementFrameSummaryAchievement1.label:GetText()
        assert(latest == select(2, GetAchievementInfo(GetLatestCompletedAchievements())),
            "the summary lists the earned achievement, got " .. tostring(latest))
        local first = AchievementFrameCategoriesContainerButton2.label:GetText()
        assert(first == GetCategoryInfo(GetCategoryList()[1]), "first category " .. tostring(first))
        "#,
    )
    .unwrap();

    // A category's list draws its rows: the earned one's category opens with its first row.
    s.run(&format!("AchievementFrame_SelectAchievement({earned})"))
        .unwrap();
    s.run(&format!(
        r#"
        local category = GetAchievementCategory({earned})
        local row = AchievementFrameAchievementsContainerButton1.label:GetText()
        assert(row == select(2, GetAchievementInfo(category, 1)), "first row " .. tostring(row))
        "#
    ))
    .unwrap();

    // The progress bar reads the criterion, and a criteria update moves it.
    s.run(&format!("AchievementFrame_SelectAchievement({bar})"))
        .unwrap();
    let read = |s: &benilla_ui::script::UiScript| {
        s.eval::<(f64, f64, String)>(
            "local lo, hi = AchievementFrameProgressBar1:GetMinMaxValues() \
             return AchievementFrameProgressBar1:GetValue(), hi, \
             AchievementFrameProgressBar1.text:GetText()",
        )
        .unwrap()
    };
    let (value, max, text) = read(&s);
    assert_eq!((value, max), (1.0, f64::from(quantity)), "{text}");
    s.apply_achievement(AchievementUpdate::Criteria {
        criterion: step(2),
        now: 0,
    });
    s.run(&format!("AchievementFrame_SelectAchievement({bar}, true)"))
        .unwrap();
    let (value, _, moved) = read(&s);
    assert_eq!(value, 2.0, "the bar moved: {text} -> {moved}");
    assert_ne!(text, moved);
    let errors = achievement_errors(s.take_errors());
    assert!(errors.is_empty(), "achievement window errors: {errors:#?}");
}
