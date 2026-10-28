//! The stock questgiver window (`QuestFrame.xml`, its panels drawn through `QuestInfo.lua`), driven
//! engine-only through the quest events on the whole 3.3.5a interface.

use benilla_ui::script::{
    GossipQuestRow, QuestItemView, QuestPanel, QuestState, ScriptValue, SoundRequest, UiScript,
};

use super::test_ui::stock_ui;

/// The questgiver, `QuestFrame_SetPortrait`'s `"questnpc"` unit (`QuestFrame.lua:63`).
fn seat_giver(s: &mut UiScript, name: &str) {
    s.set_unit(
        "questnpc",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some(name.into()),
            ..Default::default()
        }),
    );
}

/// A greeting listing `active` quests in progress.
fn greeting(text: &str, active: &[&str]) -> QuestState {
    QuestState {
        panel: QuestPanel::Greeting,
        greeting: text.into(),
        greeting_quests: active
            .iter()
            .enumerate()
            .map(|(i, t)| GossipQuestRow {
                quest_id: 100 + i as u32,
                title: (*t).into(),
                level: 5,
                active: true,
                ..Default::default()
            })
            .collect(),
        ..QuestState::default()
    }
}

/// A plain, usable reward row.
fn item(name: &str) -> QuestItemView {
    QuestItemView {
        name: Some(name.into()),
        count: 1,
        quality: 1,
        usable: true,
        ..Default::default()
    }
}

/// Stock `QuestFrame_OnShow` and `QuestFrame_OnHide` play `igQuestListOpen` and `igQuestListClose`
/// (`QuestFrame.lua:294`, `:312`).
#[test]
fn questgiver_show_hide_plays_open_and_close_kits() {
    benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.take_sounds();

    s.set_quest(Some(QuestState {
        panel: QuestPanel::Detail,
        title: "A Threat Within".into(),
        body: "Speak with the captain.".into(),
        objectives: "Report to the captain.".into(),
        ..QuestState::default()
    }));
    s.fire_event("QUEST_DETAIL", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return QuestFrame:IsVisible()").unwrap(),
        "the questgiver window opened onto the left slot"
    );
    assert!(
        s.take_sounds()
            .contains(&SoundRequest::KitName("igQuestListOpen".into())),
        "opening the questgiver window plays igQuestListOpen"
    );

    s.fire_event("QUEST_FINISHED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        !s.eval::<bool>("return QuestFrame:IsVisible()").unwrap(),
        "QUEST_FINISHED hid the questgiver window"
    );
    assert!(
        s.take_sounds()
            .contains(&SoundRequest::KitName("igQuestListClose".into())),
        "closing the questgiver window plays igQuestListClose"
    );
}

/// Each stock `QuestFrame*Panel_OnShow` hides its three siblings; only the window's own show and
/// hide play a sound.
#[test]
fn panel_events_show_exactly_one_child_panel_and_hide_the_others() {
    benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);

    let panels = [
        "QuestFrameGreetingPanel",
        "QuestFrameDetailPanel",
        "QuestFrameProgressPanel",
        "QuestFrameRewardPanel",
    ];
    let is_shown =
        |s: &UiScript, name: &str| s.eval::<bool>(&format!("return {name}:IsShown()")).unwrap();

    s.set_quest(Some(greeting(
        "What can I do for you?",
        &["Report to Goldshire"],
    )));
    seat_giver(&mut s, "Deputy Willem");
    s.fire_event("QUEST_GREETING", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    for name in panels {
        assert_eq!(
            is_shown(&s, name),
            name == "QuestFrameGreetingPanel",
            "only the greeting panel is shown after QUEST_GREETING"
        );
    }
    assert_eq!(
        s.eval::<String>("return QuestFrameNpcNameText:GetText()")
            .unwrap(),
        "Deputy Willem"
    );
    // Drain the first show's open sound; the switch below must play none.
    s.take_sounds();

    s.set_quest(Some(QuestState {
        panel: QuestPanel::Detail,
        title: "A Threat Within".into(),
        ..QuestState::default()
    }));
    s.fire_event("QUEST_DETAIL", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    for name in panels {
        assert_eq!(
            is_shown(&s, name),
            name == "QuestFrameDetailPanel",
            "only the detail panel is shown after QUEST_DETAIL"
        );
    }
    assert!(
        !s.take_sounds()
            .iter()
            .any(|r| matches!(r, SoundRequest::KitName(k) if k.starts_with("igQuestList"))),
        "a panel switch with the window already open plays no window kit"
    );
}

/// `QuestInfo_ShowRewards` fills one pool of 10: choices, the spell row, then fixed rewards, so two
/// choices and one reward are `QuestInfoItem1`, `2` and `3` (`QuestInfo.lua:258-480`).
#[test]
fn detail_panel_reward_grid_follows_the_refs_two_per_row_layout() {
    benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);

    s.set_quest(Some(QuestState {
        panel: QuestPanel::Detail,
        title: "A Threat Within".into(),
        body: "Kill the kobolds infesting the mine.".into(),
        objectives: "Slay 10 Kobold Vermin.".into(),
        choices: vec![item("Worn Sword"), item("Worn Mace")],
        rewards: vec![item("Militia Hammer")],
        reward_money: 150,
        ..QuestState::default()
    }));
    seat_giver(&mut s, "Marshal McBride");
    s.fire_event("QUEST_DETAIL", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    let shown = |s: &UiScript, i: u32| {
        s.eval::<bool>(&format!("return QuestInfoItem{i}:IsShown()"))
            .unwrap()
    };
    let name = |s: &UiScript, i: u32| {
        s.eval::<String>(&format!("return QuestInfoItem{i}Name:GetText()"))
            .unwrap()
    };
    assert!(shown(&s, 1) && shown(&s, 2) && shown(&s, 3) && !shown(&s, 4));
    assert_eq!(name(&s, 1), "Worn Sword");
    assert_eq!(name(&s, 3), "Militia Hammer");
    // The detail template does not choose: REWARD_CHOICES (`QuestInfo.lua:379-383`).
    assert_eq!(
        s.eval::<String>("return QuestInfoItemChooseText:GetText()")
            .unwrap(),
        "You will be able to choose one of these rewards:"
    );
    assert_eq!(
        s.eval::<String>("return QuestInfoItemReceiveText:GetText()")
            .unwrap(),
        "You will also receive:",
        "choices present -> REWARD_ITEMS (`QuestInfo.lua:330`)"
    );
    // The second choice sits beside the first, +1 (`QuestInfo.lua:356`).
    assert_eq!(
        s.eval::<String>(
            "local p, rel, rp, x, y = QuestInfoItem2:GetPoint() \
             return p .. '|' .. rel:GetName() .. '|' .. rp .. '|' .. x .. '|' .. y"
        )
        .unwrap(),
        "TOPLEFT|QuestInfoItem1|TOPRIGHT|1|0"
    );
    assert_eq!(
        s.eval::<String>("return QuestInfoRewardsHeader:GetText()")
            .unwrap(),
        "Rewards"
    );

    // A detail row has no select arm: `chooseItems` is nil (`QuestInfo.xml:45-47`).
    s.run("QuestInfoItem1:Click()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<i64>("return QuestInfoFrame.itemChoice").unwrap(),
        0,
        "a detail-panel row never selects a reward"
    );
    assert!(!s
        .eval::<bool>("return QuestInfoItemHighlight:IsShown()")
        .unwrap());
}

/// A reward row click selects its choice (`QuestInfo.lua:26-32`), and Complete sends it
/// (`QuestFrame.lua:91-104`).
#[test]
fn reward_panel_choice_click_selects_and_completes_with_zero_based_index() {
    benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);

    s.set_quest(Some(QuestState {
        panel: QuestPanel::Reward,
        title: "A Threat Within".into(),
        body: "Well done.".into(),
        choices: vec![item("Worn Sword"), item("Worn Mace")],
        ..QuestState::default()
    }));
    seat_giver(&mut s, "Marshal McBride");
    s.fire_event("QUEST_COMPLETE", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(
        !s.eval::<bool>("return QuestInfoItemHighlight:IsShown()")
            .unwrap(),
        "no row picked yet"
    );
    // No choice picked: `QuestChooseRewardError()` instead.
    s.run("QuestFrameCompleteQuestButton:Click()").unwrap();
    assert!(
        s.take_quest_actions().is_empty(),
        "must pick a choice first"
    );

    s.run("QuestInfoItem2:Click()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return QuestInfoItemHighlight:IsShown()")
            .unwrap(),
        "picking a choice row shows the highlight"
    );
    assert_eq!(
        s.eval::<i64>("return QuestInfoFrame.itemChoice").unwrap(),
        2
    );

    s.run("QuestFrameCompleteQuestButton:Click()").unwrap();
    assert_eq!(
        s.take_quest_actions(),
        vec![benilla_ui::script::QuestAction::Reward(1)],
        "row 2 (1-based) -> wire index 1 (0-based)"
    );
}

/// The greeting's Goodbye only hides the window, with no `DeclineQuest()` (`QuestFrame.xml:386-388`).
#[test]
fn greeting_goodbye_button_closes_the_window() {
    benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);

    s.set_quest(Some(greeting("Hello", &[])));
    seat_giver(&mut s, "Deputy Willem");
    s.fire_event("QUEST_GREETING", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s.eval::<bool>("return QuestFrame:IsVisible()").unwrap());

    s.run("QuestFrameGreetingGoodbyeButton:Click()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        !s.eval::<bool>("return QuestFrame:IsVisible()").unwrap(),
        "Goodbye closes the questgiver window"
    );
    assert_eq!(
        s.take_quest_actions(),
        vec![benilla_ui::script::QuestAction::Close],
        "only `QuestFrame_OnHide`'s `CloseQuest()`, never a decline"
    );
}

/// Accept and Decline keep real on-window rects through the instant-text arm's one frame of
/// disabled Accept; the test plants `QUEST_FADING_DISABLE = "1"`, the default being `"0"`.
#[test]
fn detail_panel_action_buttons_resolve_to_real_onscreen_rects() {
    benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.eval::<()>(r#"QUEST_FADING_DISABLE = "1""#).unwrap();
    s.set_quest(Some(QuestState {
        panel: QuestPanel::Detail,
        title: "A Threat Within".into(),
        body: "Kill kobolds.".into(),
        ..QuestState::default()
    }));
    seat_giver(&mut s, "Deputy Willem");
    s.fire_event("QUEST_DETAIL", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return QuestFrameAcceptButton:IsShown()")
        .unwrap());
    assert!(s
        .eval::<bool>("return QuestFrameDeclineButton:IsShown()")
        .unwrap());

    s.resolve();
    let window = s
        .eval::<(f32, f32, f32, f32)>(
            "return QuestFrame:GetLeft(), QuestFrame:GetRight(), QuestFrame:GetBottom(), \
             QuestFrame:GetTop()",
        )
        .unwrap();
    let button_art = |s: &mut UiScript| {
        s.resolve();
        let mut v = Vec::new();
        for q in s.extract() {
            if let benilla_ui::script::QuadContent::Texture { path: Some(p), .. } = &q.content {
                if p.starts_with("Interface\\Buttons\\UI-Panel-Button-") {
                    let r = q.rect.expect("button quad carries a resolved rect");
                    // Accept and Decline: the two faces inside the questgiver window.
                    if r.left >= window.0 && r.right <= window.1 && r.bottom >= window.2 {
                        v.push((p.clone(), r));
                    }
                }
            }
        }
        v
    };

    // `QuestInfo_ShowFadingFrame` still disables Accept; the reveal just starts at 1024
    // (`QuestInfo.lua:482-492`).
    let writing = button_art(&mut s);
    assert_eq!(
        writing.len(),
        2,
        "Accept + Decline both extract a real rect inside the window: {writing:?}"
    );
    assert!(
        writing
            .iter()
            .any(|(p, _)| p.ends_with("UI-Panel-Button-Disabled")),
        "Accept draws the Disabled art inside the one-frame arm window"
    );

    // The first `OnUpdate` scratches once, snaps the block opaque and enables Accept
    // (`QuestInfo.lua:3-17`).
    let scratches = |sounds: Vec<SoundRequest>| {
        sounds
            .iter()
            .filter(|r| **r == SoundRequest::KitName("WriteQuest".into()))
            .count()
    };
    s.take_sounds();
    s.tick(0.05);
    assert!(s.errors().is_empty(), "tick errors: {:?}", s.errors());
    assert_eq!(
        scratches(s.take_sounds()),
        1,
        "instant text keeps the writing sound: one scratch on the wake tick"
    );
    assert!(s
        .eval::<bool>("return QuestFrameAcceptButton:IsEnabled() == 1")
        .unwrap());
    assert_eq!(
        s.eval::<f32>("return QuestInfoFadingFrame:GetAlpha()")
            .unwrap(),
        1.0,
        "the block snaps straight to opaque, with no QUESTINFO_FADE_IN ramp in instant mode"
    );
    s.tick(0.5);
    assert_eq!(
        scratches(s.take_sounds()),
        0,
        "the quill stops after the single instant-mode scratch"
    );
    let written = button_art(&mut s);
    assert!(
        written
            .iter()
            .all(|(p, _)| p.ends_with("UI-Panel-Button-Up")),
        "both buttons draw live art once the text has written on: {written:?}"
    );
}

/// With the default `QUEST_FADING_DISABLE = "0"` the text writes on at 70 chars/s, scratching each
/// tick with Accept disabled, then the block fades in (`QuestFrame.lua:4-6`, `QuestInfo.lua:3-17`).
#[test]
fn write_on_still_fades_when_instant_text_is_off() {
    benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    assert_eq!(
        s.eval::<String>("return QUEST_FADING_DISABLE").unwrap(),
        "0"
    );
    s.set_quest(Some(QuestState {
        panel: QuestPanel::Detail,
        title: "A Threat Within".into(),
        body: "Kill kobolds.".into(),
        ..QuestState::default()
    }));
    seat_giver(&mut s, "Deputy Willem");
    s.fire_event("QUEST_DETAIL", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.take_sounds();

    // Mid-write: 0.1 s reveals 7 of the 13 characters.
    s.tick(0.1);
    assert!(
        s.take_sounds()
            .iter()
            .any(|r| *r == SoundRequest::KitName("WriteQuest".into())),
        "the write-on scratches the WriteQuest quill each tick"
    );
    assert!(!s
        .eval::<bool>("return QuestFrameAcceptButton:IsEnabled() == 1")
        .unwrap());

    // Half a second more ends the gradient: Accept wakes and the block is mid-fade.
    s.tick(0.5);
    assert!(s.errors().is_empty(), "tick errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return QuestFrameAcceptButton:IsEnabled() == 1")
        .unwrap());
    let alpha = s
        .eval::<f32>("return QuestInfoFadingFrame:GetAlpha()")
        .unwrap();
    assert!(
        alpha < 1.0,
        "fading mode ramps the block in (got alpha {alpha})"
    );
    // Frame-sized steps: stock `UIFrameFade_OnUpdate` skips the entry after each one it removes, so
    // a coarse tick that ends the load's shine fades together can skip ours (`UIParent.lua:2456`).
    for _ in 0..40 {
        s.tick(1.0 / 30.0);
    }
    assert_eq!(
        s.eval::<f32>("return QuestInfoFadingFrame:GetAlpha()")
            .unwrap(),
        1.0,
        "the fade completes at opaque"
    );
}

/// Every quest event but `QUEST_FINISHED`, `QUEST_ITEM_UPDATE` over a shown window included,
/// re-reads `UnitName("questnpc")` into the title bar (`QuestFrame.lua:26`, `:62-63`).
#[test]
fn npc_name_reaches_the_title_bar_on_open_and_on_refresh() {
    benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);

    s.set_quest(Some(QuestState {
        panel: QuestPanel::Detail,
        title: "A Threat Within".into(),
        body: "Kill things.".into(),
        ..QuestState::default()
    }));
    seat_giver(&mut s, "Marshal McBride");
    s.fire_event("QUEST_DETAIL", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<String>("return QuestFrameNpcNameText:GetText() or ''")
            .unwrap(),
        "Marshal McBride",
        "the questnpc's name lands in the title bar"
    );
    s.resolve();
    let name_quad = s
        .extract()
        .into_iter()
        .find(|q| {
            matches!(&q.content,
                benilla_ui::script::QuadContent::Text { text: Some(t), .. } if t == "Marshal McBride")
        })
        .expect("the NPC-name FontString extracts a quad");
    assert!(
        name_quad.rect.is_some(),
        "the NPC-name quad carries a resolved rect"
    );

    // A cleared name comes back on the refresh.
    s.run("QuestFrameNpcNameText:SetText('')").unwrap();
    s.fire_event(
        "QUEST_ITEM_UPDATE",
        vec![ScriptValue::Str("Marshal McBride".into())],
    );
    assert_eq!(
        s.eval::<String>("return QuestFrameNpcNameText:GetText() or ''")
            .unwrap(),
        "Marshal McBride",
        "the QUEST_ITEM_UPDATE refresh also carries the name"
    );
}

/// A title that wraps at the row label's 275px (`QuestFrameTemplates.xml:145-147`) grows its row,
/// or the rows below overlap it (`QuestFrame.lua:239`).
#[test]
fn greeting_panel_title_rows_grow_to_their_wrapped_titles() {
    benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    // Stock sizes each row right after `SetText`, `GetTextHeight() + 2`, so the measure must be
    // synchronous, as the app's is.
    s.set_text_measurer(Box::new(super::FixedWidthFont(7.0)));

    s.set_quest(Some(greeting(
        "What can I do for you?",
        &[
            "Deliver Thomas' Report to Marshal Dughan in Goldshire before the Defias move again",
            "Investigate the Echo Ridge Mine and report back to Marshal McBride at once",
        ],
    )));
    seat_giver(&mut s, "Deputy Willem");
    s.fire_event("QUEST_GREETING", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    let answer_measures = |s: &mut UiScript| {
        let answers: Vec<(u32, f32, f32, u64)> = s
            .fontstrings_needing_measure()
            .into_iter()
            .map(|r| {
                let ink = r.text.chars().count() as f32 * 6.0;
                match r.wrap_width {
                    Some(w) => (r.id, ink.min(w), (ink / w).ceil().max(1.0) * 14.0, r.key),
                    None => (r.id, ink, 14.0, r.key),
                }
            })
            .collect();
        s.set_measured_text_unwrapped(&answers);
    };
    answer_measures(&mut s);
    s.resolve();
    s.tick(0.016);
    answer_measures(&mut s);
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    let row = |i: u32| -> (f32, f32, f32) {
        s.eval::<(f32, f32, f32)>(&format!(
            "local b = QuestTitleButton{i}\n\
             return b:GetTop(), b:GetBottom(), b:GetFontString():GetHeight()"
        ))
        .unwrap()
    };
    let (t1, b1, h1) = row(1);
    let (t2, b2, h2) = row(2);
    assert!(h1 >= 24.0 && h2 >= 24.0, "both titles wrap: {h1}, {h2}");
    assert!(
        (t1 - b1 - (h1 + 2.0)).abs() < 0.5,
        "row 1 is its wrapped title + 2: got {}, title {h1}",
        t1 - b1
    );
    assert!(
        (t2 - b2 - (h2 + 2.0)).abs() < 0.5,
        "row 2 is its wrapped title + 2: got {}, title {h2}",
        t2 - b2
    );
    assert!(
        t2 <= b1 + 0.5,
        "row 2 starts at/below row 1's bottom: row1 bottom {b1}, row2 top {t2}"
    );
}

/// The reward row's `OnClick` (`QuestInfo.xml:27-49`): a modified click goes to
/// `HandleModifiedItemClick`, ctrl previewing and shift posting the link, and only a plain click on
/// the reward template selects.
#[test]
fn reward_rows_preview_and_post_without_selecting_the_choice() {
    benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);

    const SWORD: &str = "|cffffffff|Hitem:2299:0:0:0|h[Worn Sword]|h|r";
    const MACE: &str = "|cffffffff|Hitem:2300:0:0:0|h[Worn Mace]|h|r";
    let linked = |id: u32, name: &str, link: &str| QuestItemView {
        item_id: id,
        link: Some(link.into()),
        ..item(name)
    };
    s.set_quest(Some(QuestState {
        panel: QuestPanel::Reward,
        title: "A Threat Within".into(),
        body: "Well done.".into(),
        choices: vec![
            linked(2299, "Worn Sword", SWORD),
            linked(2300, "Worn Mace", MACE),
        ],
        rewards: vec![linked(
            2504,
            "Worn Shortbow",
            "|cffffffff|Hitem:2504:0:0:0|h[Worn Shortbow]|h|r",
        )],
        ..QuestState::default()
    }));
    seat_giver(&mut s, "Marshal McBride");
    // A worn-slot template makes the sword dressable (`IsDressableItem` `0x517530`).
    s.set_item_template(
        2299,
        benilla_ui::script::ItemTemplateView {
            name: "Worn Sword".into(),
            quality: 1,
            inventory_type: 21,
            ..Default::default()
        },
    );
    s.fire_event("QUEST_COMPLETE", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    let choice = |s: &UiScript| s.eval::<i64>("return QuestInfoFrame.itemChoice").unwrap();

    // A plain click selects row 2; the modified clicks below aim at row 1.
    s.run("QuestInfoItem2:Click()").unwrap();
    assert_eq!(choice(&s), 2, "a plain click selects the reward choice");
    assert!(s
        .eval::<bool>("return QuestInfoItemHighlight:IsShown()")
        .unwrap());
    assert!(
        s.take_dressup_intents().is_empty(),
        "a plain click never opens the dressing room"
    );

    s.run("ChatEdit_ActivateChat(ChatFrame1EditBox)").unwrap();
    s.set_modifiers(true, false, false);
    s.run("QuestInfoItem1:Click()").unwrap();
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.eval::<String>("return ChatFrame1EditBox:GetText()")
            .unwrap(),
        format!(" {SWORD}"),
        "shift-click posted choice 1's link after `ChatEdit_InsertLink`'s space"
    );
    assert_eq!(choice(&s), 2, "the modified arm never selects");

    // A fixed reward row posts too; only a `"choice"` row selects (`QuestInfo.lua:27`).
    s.run("ChatFrame1EditBox:SetText(\"\")").unwrap();
    s.set_modifiers(true, false, false);
    s.run("QuestInfoItem3:Click()").unwrap();
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.eval::<String>("return ChatFrame1EditBox:GetText()")
            .unwrap(),
        " |cffffffff|Hitem:2504:0:0:0|h[Worn Shortbow]|h|r"
    );

    s.set_modifiers(false, true, false);
    s.run("QuestInfoItem1:Click()").unwrap();
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.take_dressup_intents(),
        vec![
            benilla_ui::script::DressUpIntent::Dress,
            benilla_ui::script::DressUpIntent::TryOn(2299)
        ],
        "ctrl-click opened the room wearing choice 1"
    );
    assert_eq!(choice(&s), 2, "the ctrl arm never selects");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
