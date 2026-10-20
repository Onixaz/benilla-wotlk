//! Measurement (#347): where the first `ToggleAchievementFrame()` spends its time.

use std::sync::Arc;
use std::time::Instant;

use benilla_ui::script::{AchievementBook, AchievementDb, AchievementUpdate, CriteriaWire, WowDate};

#[test]
#[ignore = "measurement"]
fn achievement_first_open_cost() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let data = benilla_formats::wow_data().expect("install");
    let mut chain = benilla_formats::open_chain(&data).unwrap();
    let catalog = benilla_formats::load_achievement_catalog(&mut chain).unwrap();
    // A seasoned character: every third real achievement earned, a criterion per achievement.
    let earned: Vec<(u32, u32)> = catalog
        .achievements
        .iter()
        .filter(|a| a.faction == -1 && a.category > 0)
        .step_by(3)
        .map(|a| (a.id, WowDate::from_date(0, 2, 9).0))
        .collect();
    let criteria: Vec<CriteriaWire> = catalog
        .criteria
        .iter()
        .step_by(2)
        .map(|c| CriteriaWire {
            id: c.id,
            counter: 1,
            date: WowDate::from_date(0, 1, 9).0,
            ..Default::default()
        })
        .collect();
    eprintln!(
        "rows {} criteria {} earned {} criteria held {}",
        catalog.achievements.len(),
        catalog.criteria.len(),
        earned.len(),
        criteria.len()
    );
    let db: Arc<AchievementDb> = Arc::new(crate::ui_achievement::db_of(catalog));
    let (mut s, _f) = super::layer_tests::production_load_with("achcost", false, "", |_| {});
    s.set_achievement_db(db);
    s.set_achievement_book(AchievementBook::default());
    s.apply_achievement(AchievementUpdate::List {
        achievements: earned,
        criteria,
        now: 0,
    });
    s.resolve();
    let _ = s.extract();
    let _ = s.take_errors();

    let t0 = Instant::now();
    let now = s
        .lua()
        .create_function(move |_, ()| Ok(t0.elapsed().as_secs_f64() * 1000.0))
        .unwrap();
    s.lua().globals().set("__now", now).unwrap();
    // Every engine verb with "Achievement", "Category" or "Criteria" in its name, timed and counted.
    s.run(
        r#"PROF = {}
        local wrap = {}
        for k, v in pairs(_G) do
            if type(v) == "function" and (k:find("Achievement") or k:find("Category") or k:find("Criteria") or k:find("Statistic"))
               and not k:find("^AchievementFrame") and not k:find("^Achievement[A-Z][a-z]+_") then
                wrap[k] = v
            end
        end
        for k, f in pairs(wrap) do
            _G[k] = function(...)
                local t = __now()
                local r = { f(...) }
                local p = PROF[k] or { n = 0, ms = 0 }
                p.n = p.n + 1 p.ms = p.ms + (__now() - t)
                PROF[k] = p
                return unpack(r)
            end
        end"#,
    )
    .unwrap();

    let lap = |label: &str, at: &mut Instant| {
        eprintln!("{label}: {:.2} ms", at.elapsed().as_secs_f64() * 1000.0);
        *at = Instant::now();
    };
    let mut at = Instant::now();
    s.run("LoadAddOn('Blizzard_AchievementUI')").unwrap();
    lap("LoadAddOn", &mut at);
    s.run("ToggleAchievementFrame()").unwrap();
    lap("first toggle", &mut at);
    s.resolve();
    lap("resolve", &mut at);
    let _ = s.extract();
    lap("extract", &mut at);
    s.tick(0.016);
    lap("tick", &mut at);
    let report: String = s
        .eval(
            r#"local t = {}
            for k, p in pairs(PROF) do t[#t + 1] = { k, p.n, p.ms } end
            table.sort(t, function(a, b) return a[3] > b[3] end)
            local out = {}
            for i = 1, math.min(15, #t) do out[#out + 1] = string.format("%-40s %6d calls %8.2f ms", t[i][1], t[i][2], t[i][3]) end
            return table.concat(out, "\n")"#,
        )
        .unwrap();
    eprintln!("{report}");
    s.run("PROF = {} ToggleAchievementFrame()").unwrap();
    lap("close", &mut at);
    s.run("ToggleAchievementFrame()").unwrap();
    lap("second toggle", &mut at);
    eprintln!("errors: {:?}", s.take_errors().len());
}
