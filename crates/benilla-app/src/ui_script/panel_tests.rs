use benilla_ui::script::{
    ExtractedQuad, GossipMenu, GossipOptionView, GossipQuestRow, MerchantItem, MerchantState,
    QuadContent, SoundRequest, UiScript,
};

/// Each test loads stock `UIParent.xml` first: its `ShowUIPanel`, `HideUIPanel` and
/// `UIPanelWindows` are the panel manager every window here seats through. A missing template only
/// warns under this loader, so a list without `UIPanelTemplates.xml` loses scroll bars silently.
use super::test_ui::stock_ui;

/// The rect of the frame sized `w` by `h`, from the `QuadContent::Frame` quad every frame emits.
fn frame_rect(quads: &[ExtractedQuad], w: f32, h: f32) -> benilla_ui::layout::Rect {
    quads
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Frame => q
                .rect
                .filter(|r| (r.width() - w).abs() < 0.5 && (r.height() - h).abs() < 0.5),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no bare-frame quad sized {w}x{h}"))
}

/// Stock `GossipFrame.xml` with a two-option menu: `GOSSIP_SHOW` seats it at the left slot, a row
/// click queues its select, and the close button vacates the slot.
#[test]
fn shipped_gossip_frame_drives_end_to_end() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));

    s.resolve();
    let vendor_icon = |quads: &[ExtractedQuad]| {
        quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("VendorGossipIcon"))
        })
    };
    assert!(!vendor_icon(&s.extract()), "gossip window starts hidden");
    assert!(
        s.eval::<bool>("return GetUIPanel('left') == nil").unwrap(),
        "left slot empty before any panel opens"
    );

    s.set_gossip(Some(GossipMenu {
        greeting: "Greetings, traveler. How may I help you?".into(),
        quests: Vec::new(),
        options: vec![
            GossipOptionView {
                label: "Let me browse your goods.".into(),
                icon_type: "vendor".into(),
                ..Default::default()
            },
            GossipOptionView {
                label: "I wish to sign the petition.".into(),
                icon_type: "gossip".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    }));
    s.fire_event("GOSSIP_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(s.eval::<bool>("return GossipFrame:IsVisible()").unwrap());
    assert_eq!(
        s.eval::<String>("return GossipGreetingText:GetText()")
            .unwrap(),
        "Greetings, traveler. How may I help you?"
    );
    // Every row is enabled: `GossipFrameOptionsUpdate` greys none (`GossipFrame.lua`).
    let states: (bool, bool, bool, bool) = s
        .eval(
            "return GossipTitleButton1:IsVisible(), GossipTitleButton1:IsEnabled() ~= 0,\n\
                        GossipTitleButton2:IsEnabled() ~= 0, GossipTitleButton3:IsVisible()",
        )
        .unwrap();
    assert_eq!(
        states,
        (true, true, true, false),
        "the reference greys nothing; extras hidden"
    );

    s.resolve();
    let quads = s.extract();

    // The left slot is TOPLEFT 0,-104 (`UIParent.lua`), so the top edge is 768 - 104.
    let win = frame_rect(&quads, 384.0, 512.0);
    assert_eq!(
        (win.left, win.top),
        (0.0, 664.0),
        "gossip window landed at the left slot (TOPLEFT UIParent, 0, -104)"
    );

    // The parchment is four corner quadrants, 256 and 128 wide, 256 tall (`GossipFrame.xml`).
    let quad_rect = |needle: &str, w: f32, h: f32| {
        let q = quads
            .iter()
            .find(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                        if p.contains(needle))
            })
            .unwrap_or_else(|| panic!("no texture quad for {needle}"));
        let r = q.rect.unwrap_or_else(|| panic!("no rect for {needle}"));
        assert!(
            (r.width() - w).abs() < 0.5 && (r.height() - h).abs() < 0.5,
            "{needle} is {w}×{h}, got {}×{}",
            r.width(),
            r.height()
        );
        r
    };
    let tl = quad_rect("UI-QuestGreeting-TopLeft", 256.0, 256.0);
    assert_eq!(
        (tl.left, tl.top),
        (win.left, win.top),
        "TopLeft quadrant pinned to the window's TOPLEFT"
    );
    let tr = quad_rect("UI-QuestGreeting-TopRight", 128.0, 256.0);
    assert_eq!(
        (tr.right, tr.top),
        (win.right, win.top),
        "TopRight quadrant pinned to the window's TOPRIGHT"
    );
    let bl = quad_rect("UI-QuestGreeting-BotLeft", 256.0, 256.0);
    assert_eq!(
        (bl.left, bl.bottom),
        (win.left, win.bottom),
        "BotLeft quadrant pinned to the window's BOTTOMLEFT"
    );
    let br = quad_rect("UI-QuestGreeting-BotRight", 128.0, 256.0);
    assert_eq!(
        (br.right, br.bottom),
        (win.right, win.bottom),
        "BotRight quadrant pinned to the window's BOTTOMRIGHT"
    );

    let icon_rect = quads
        .iter()
        .find(|q| {
            // Case-folded: the 1.12 client answers the type in lowercase, and `GossipFrame.lua`
            // builds the path from it verbatim.
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.to_ascii_lowercase().contains("vendorgossipicon"))
        })
        .and_then(|q| q.rect)
        .expect("vendor option icon visible after GOSSIP_SHOW");
    assert!(
        quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Text { text: Some(t), .. }
                    if t == "Let me browse your goods.")
        }),
        "option label shows"
    );

    // The icon's centre lies inside row 1's button.
    let (cx, cy) = (
        (icon_rect.left + icon_rect.right) * 0.5,
        (icon_rect.bottom + icon_rect.top) * 0.5,
    );
    s.mouse_button(cx, cy, "LeftButton", true);
    s.mouse_button(cx, cy, "LeftButton", false);
    assert_eq!(s.take_gossip_selects(), vec![(1, None)]);
    assert!(!s.take_gossip_close());

    // The close button hides the window; its OnHide calls `CloseGossip()` (`GossipFrame.xml`).
    s.run("GossipFrameCloseButton:Click()").unwrap();
    assert!(s.take_gossip_close());
    assert!(!s.eval::<bool>("return GossipFrame:IsVisible()").unwrap());
    assert!(
        s.eval::<bool>("return GetUIPanel('left') == nil").unwrap(),
        "HideUIPanel vacated the left slot"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Quest rows fill the shared pool first, available before active, then the options
/// (`GossipFrame.lua`), all on one static anchor chain.
#[test]
fn shipped_gossip_frame_renders_quest_rows_above_options() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);

    s.set_gossip(Some(GossipMenu {
        greeting: "A word, traveler.".into(),
        quests: vec![
            GossipQuestRow {
                title: "Report to Goldshire".into(),
                level: 5,
                active: true,
                ..Default::default()
            },
            GossipQuestRow {
                title: "A Threat Within".into(),
                level: 7,
                active: false,
                ..Default::default()
            },
        ],
        options: vec![GossipOptionView {
            label: "Let me browse your goods.".into(),
            icon_type: "vendor".into(),
            ..Default::default()
        }],
        ..Default::default()
    }));
    s.fire_event("GOSSIP_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    let (r1_text, r1_vis, r2_vis, r3_text, r3_vis, r4_vis, r5_text, r5_vis, r6_vis): (
        String,
        bool,
        bool,
        String,
        bool,
        bool,
        String,
        bool,
        bool,
    ) = s
        .eval(
            "return GossipTitleButton1:GetText(), GossipTitleButton1:IsVisible(),\n\
                        GossipTitleButton2:IsVisible(),\n\
                        GossipTitleButton3:GetText(), GossipTitleButton3:IsVisible(),\n\
                        GossipTitleButton4:IsVisible(),\n\
                        GossipTitleButton5:GetText(), GossipTitleButton5:IsVisible(),\n\
                        GossipTitleButton6:IsVisible()",
        )
        .unwrap();
    // Available leads active (`GossipFrame.lua`), though the packet listed the active quest
    // first. Each quest group ends by hiding the next button and skipping it (`:80-84`,
    // `:104-108`): 1 available, 2 spacer, 3 active, 4 spacer, 5 option, the rest hidden.
    assert_eq!(
        (r1_text.as_str(), r1_vis),
        ("|cff000000A Threat Within|r", true)
    );
    assert!(!r2_vis, "the available group's trailing spacer");
    assert_eq!(
        (r3_text.as_str(), r3_vis),
        ("|cff000000Report to Goldshire|r", true)
    );
    assert!(!r4_vis, "the active group's trailing spacer");
    assert_eq!(
        (r5_text.as_str(), r5_vis),
        ("Let me browse your goods.", true),
        "the option row sits below both quest groups"
    );
    assert!(!r6_vis, "nothing past the option row");

    // Each group's icon (`GossipFrame.lua`), read from the painted quads.
    s.resolve();
    let quads = s.extract();
    let has_icon = |needle: &str| {
        quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle))
        })
    };
    assert!(
        has_icon("AvailableQuestIcon"),
        "row 1 (available) icon renders"
    );
    assert!(
        has_icon("IncompleteQuestIcon"),
        "row 2 (active, not complete) icon renders"
    );

    // The option sits below the last quest row; rects are y-up, so below is a smaller top.
    let label_top = |needle: &str| {
        quads
            .iter()
            .find_map(|q| match &q.content {
                QuadContent::Text { text: Some(t), .. } if t == needle => q.rect,
                _ => None,
            })
            .unwrap_or_else(|| panic!("no text quad for {needle:?}"))
            .top
    };
    let row2_top = label_top("|cff000000Report to Goldshire|r");
    let row3_top = label_top("Let me browse your goods.");
    assert!(
        row3_top < row2_top,
        "row 3 (option) sits below row 2 (last quest row): row2 top {row2_top}, row3 top {row3_top}"
    );

    // Button 3, the active quest, calls `SelectGossipActiveQuest(1)`, its place in the active list
    // (`GossipFrame.lua`); the binding maps it to menu position 1, the packet's order.
    s.run("GossipTitleButton3:Click()").unwrap();
    assert_eq!(s.take_gossip_quest_selects(), vec![1]);
    assert!(
        s.take_gossip_selects().is_empty(),
        "a quest-row click never queues SelectGossipOption"
    );
}

/// `GossipResize` sizes a row to its label's height + 2 (`GossipFrame.lua`) and each row
/// hangs 3 px below the previous one's BOTTOMLEFT (`GossipFrame.xml`), so wrapped labels stack
/// without overlapping. The checks pin that rule, not a glyph metric.
#[test]
fn shipped_gossip_rows_grow_to_their_wrapped_labels() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));

    // Three options that wrap at the label's 275 px (`GossipFrame.xml`).
    let long = |t: &str| GossipOptionView {
        label: t.into(),
        icon_type: "gossip".into(),
        ..Default::default()
    };
    s.set_gossip(Some(GossipMenu {
        greeting: "Make your choice!".into(),
        quests: Vec::new(),
        options: vec![
            long(
                "I slay the man on the spot as my liege would expect me to, as he has broken the \
                  law of the land and it is my sworn duty to enforce it.",
            ),
            long(
                "I turn over the man to my liege for punishment, as the man has stolen, and I am \
                  not the arbiter of his fate.",
            ),
            long(
                "I allow the man to take enough corn to feed his family for a couple of days, \
                  encouraging him to leave the land.",
            ),
        ],
        ..Default::default()
    }));
    s.fire_event("GOSSIP_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // The host's batch answers: 6 px a character, wrapped at the request's width, 14 px a line.
    let answer_measures = |s: &mut UiScript| {
        let answers: Vec<(u32, f32, f32, u64)> = s
            .fontstrings_needing_measure()
            .into_iter()
            .map(|r| {
                let ink = r.text.chars().count() as f32 * 6.0;
                match r.wrap_width {
                    Some(w) => {
                        let lines = (ink / w).ceil().max(1.0);
                        (r.id, ink.min(w), lines * 14.0, r.key)
                    }
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
            "return GossipTitleButton{i}:GetTop(), GossipTitleButton{i}:GetBottom(), \
             GossipTitleButton{i}:GetTextHeight()"
        ))
        .unwrap()
    };
    let (t1, b1, h1) = row(1);
    let (t2, b2, h2) = row(2);
    let (t3, b3, h3) = row(3);
    for (i, h) in [(1, h1), (2, h2), (3, h3)] {
        assert!(h >= 28.0, "row {i}'s label wraps: measured height {h}");
    }
    for (i, (top, bottom, h)) in [(1, (t1, b1, h1)), (2, (t2, b2, h2)), (3, (t3, b3, h3))] {
        assert!(
            (top - bottom - (h + 2.0)).abs() < 0.5,
            "row {i} height is its wrapped label + 2: got {}, label {h}",
            top - bottom
        );
    }
    // 3 px apart; rects are y-up.
    assert!(
        (b1 - t2 - 3.0).abs() < 0.5,
        "row 2 starts 3 px under row 1: row1 bottom {b1}, row2 top {t2}"
    );
    assert!(
        (b2 - t3 - 3.0).abs() < 0.5,
        "row 3 starts 3 px under row 2: row2 bottom {b2}, row3 top {t3}"
    );
}

/// The window's OnShow and OnHide play the kits (`GossipFrame.xml`).
#[test]
fn gossip_show_hide_plays_open_and_close_kits() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);

    assert!(
        s.take_sounds().is_empty(),
        "no sound at load (never transitions)"
    );

    s.set_gossip(Some(GossipMenu {
        greeting: "Well met.".into(),
        quests: Vec::new(),
        options: Vec::new(),
        ..Default::default()
    }));
    s.fire_event("GOSSIP_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igQuestListOpen".into())],
        "opening the gossip window plays igQuestListOpen"
    );

    s.fire_event("GOSSIP_CLOSED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igQuestListClose".into())],
        "closing the gossip window plays igQuestListClose"
    );
}

#[test]
fn shipped_panel_slot_replaces_gossip_with_merchant() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);

    s.set_gossip(Some(GossipMenu {
        greeting: "Well met.".into(),
        quests: Vec::new(),
        // Two options: a lone non-gossip option is selected without opening the window
        // (`GossipFrame_OnEvent`).
        options: vec![
            GossipOptionView {
                label: "Let me browse your goods.".into(),
                icon_type: "vendor".into(),
                ..Default::default()
            },
            GossipOptionView {
                label: "Tell me about this place.".into(),
                icon_type: "gossip".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    }));
    s.fire_event("GOSSIP_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return GossipFrame:IsVisible()").unwrap(),
        "gossip opened onto the (empty) left slot"
    );
    s.resolve();
    let win = frame_rect(&s.extract(), 384.0, 512.0);
    assert_eq!((win.left, win.top), (0.0, 664.0));

    // The vendor replaces gossip through the panel manager alone; no GOSSIP_CLOSED fires.
    s.set_merchant(Some(MerchantState {
        items: vec![MerchantItem {
            name: Some("Refreshing Spring Water".into()),
            texture: Some("Interface\\Icons\\INV_Drink_18".into()),
            price: 25,
            quantity: 1,
            num_available: -1,
            item_id: 159,
            stats: None,
            link: None,
            max_stack: Some(1),
            extended_cost: None,
        }],
        ..Default::default()
    }));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // Both rows are pushable 0, so the newcomer replaces gossip (`UIParent.lua`).
    assert!(
        !s.eval::<bool>("return GossipFrame:IsVisible()").unwrap(),
        "opening merchant replaced gossip at the left slot"
    );
    assert!(s.eval::<bool>("return MerchantFrame:IsVisible()").unwrap());
    s.resolve();
    let win = frame_rect(&s.extract(), 384.0, 512.0);
    assert_eq!(
        (win.left, win.top),
        (0.0, 664.0),
        "merchant took the left slot gossip vacated"
    );
    assert!(s
        .eval::<bool>("return GetUIPanel('left'):GetName() == \"MerchantFrame\"")
        .unwrap());

    s.fire_event("MERCHANT_CLOSED", vec![]);
    assert!(
        s.eval::<bool>("return GetUIPanel('left') == nil").unwrap(),
        "the slot is empty once the replacing window also closes"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A displaced NPC window's OnHide calls its `CloseX()`, whose intent ends that session; a session
/// left open keeps its window from reopening.
#[test]
fn displacing_an_npc_window_ends_the_displaced_session() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);

    // The vendor holds the left slot; drain what opening it queued.
    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_SHOW", vec![]);
    let _ = s.take_merchant_close();
    let _ = s.take_gossip_close();

    // Gossip replaces the vendor; its OnHide calls `CloseMerchant()` (`MerchantFrame.lua`).
    s.set_gossip(Some(GossipMenu {
        greeting: "Well met.".into(),
        quests: Vec::new(),
        options: Vec::new(),
        ..Default::default()
    }));
    s.fire_event("GOSSIP_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.take_merchant_close(),
        "gossip displacing the vendor ends the vendor session (OnHide → CloseMerchant)"
    );

    // And back: gossip's OnHide calls `CloseGossip()` (`GossipFrame.xml`).
    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.take_gossip_close(),
        "the vendor displacing gossip ends the gossip session (OnHide → CloseGossip)"
    );
}

/// A left occupant with a higher `pushable` slides to center when a pushable-0 window arrives
/// (`FramePositionDelegate:ShowUIPanel`, "Highest priority goes to center"); stand-ins with
/// LootFrame's row (left, 7) and the stock merchant.
#[test]
fn shipped_panel_slot_pushable_promotes_to_center() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    let at = |s: &mut UiScript, f: &str| -> (f32, f32) {
        s.resolve();
        s.eval::<(f32, f32)>(&format!("return {f}:GetLeft(), {f}:GetTop()"))
            .unwrap()
    };

    s.run(
        r#"
            UIPanelWindows["ProbeLoot"] = { area = "left", pushable = 7 }
            local loot = CreateFrame("Frame", "ProbeLoot", UIParent)
            loot:Hide()
            loot:SetWidth(50); loot:SetHeight(50)
            ShowUIPanel(loot)
        "#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return GetUIPanel('left'):GetName() == 'ProbeLoot'")
            .unwrap(),
        "loot took the empty left slot"
    );
    assert_eq!(at(&mut s, "ProbeLoot"), (0.0, 664.0));

    s.set_merchant(Some(MerchantState::default()));
    s.fire_event("MERCHANT_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return GetUIPanel('center'):GetName() == 'ProbeLoot'")
            .unwrap(),
        "loot was pushed to center, not replaced"
    );
    assert!(
        s.eval::<bool>("return GetUIPanel('left'):GetName() == 'MerchantFrame'")
            .unwrap(),
        "merchant took the left slot loot vacated"
    );
    assert_eq!(
        at(&mut s, "ProbeLoot"),
        (384.0, 664.0),
        "loot moved to the center slot (UIParent TOPLEFT 384, -104)"
    );
    assert_eq!(at(&mut s, "MerchantFrame"), (0.0, 664.0));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A banker's menu is one non-gossip option, so `GossipFrame_OnEvent` selects it without opening
/// the window; the bank then opens on `BANKFRAME_OPENED` straight into the left slot.
#[test]
fn a_bankers_lone_option_is_selected_without_a_gossip_window() {
    use benilla_ui::script::BankState;
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);

    s.set_gossip(Some(GossipMenu {
        greeting: "Welcome to the bank of Ironforge!".into(),
        quests: Vec::new(),
        options: vec![GossipOptionView {
            label: "I would like to check my deposit box.".into(),
            icon_type: "banker".into(),
            ..Default::default()
        }],
        ..Default::default()
    }));
    s.fire_event("GOSSIP_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        !s.eval::<bool>("return GossipFrame:IsShown()").unwrap(),
        "no gossip window for a lone non-gossip option"
    );
    assert_eq!(
        s.take_gossip_selects()
            .iter()
            .map(|(i, _)| *i)
            .collect::<Vec<_>>(),
        vec![1],
        "the option was selected"
    );

    s.set_money(0);
    s.set_bank(Some(BankState::default()));
    s.set_gossip(None);
    s.fire_event("GOSSIP_CLOSED", vec![]);
    s.fire_event("BANKFRAME_OPENED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s.eval::<bool>("return BankFrame:IsVisible()").unwrap());
    assert!(
        s.eval::<bool>("return GetUIPanel('left'):GetName() == 'BankFrame'")
            .unwrap(),
        "the bank seats at the left slot"
    );
}

/// The rows sit in `GossipGreetingScrollFrame` (`GossipFrame.xml`): a menu taller than the
/// frame gets a scroll range and a bar, and every row clips to the frame.
#[test]
fn an_overflowing_gossip_menu_scrolls_instead_of_spilling() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));

    // Eight options of about four lines each, far past the 334 px frame (`GossipFrame.xml`).
    let long = |n: usize| GossipOptionView {
        label: format!(
            "Option {n}: I slay the man on the spot as my liege would expect me to, as he has \
             broken the law of the land and it is my sworn duty to enforce it, whatever the cost."
        ),
        icon_type: "gossip".into(),
        ..Default::default()
    };
    s.set_gossip(Some(GossipMenu {
        greeting: "Make your choice!".into(),
        quests: Vec::new(),
        options: (1..=8).map(long).collect(),
        ..Default::default()
    }));
    s.fire_event("GOSSIP_SHOW", vec![]);

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
    // A few frames for the measures to land, the rows to fit and the bar to re-range.
    for _ in 0..3 {
        answer_measures(&mut s);
        s.resolve();
        s.tick(0.016);
    }
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // The scroll child keeps its declared 300x334 (`GossipFrame.xml`), so only the range is
    // checked: the rows' extents make it.
    let (child_h, range): (f32, f32) = s
        .eval(
            "return GossipGreetingScrollChildFrame:GetHeight(), \
             GossipGreetingScrollFrame:GetVerticalScrollRange()",
        )
        .unwrap();
    assert!(
        range > 0.0,
        "the overflowing menu has somewhere to scroll: child {child_h}, range {range}"
    );
    assert!(
        s.eval::<bool>("return GossipGreetingScrollFrameScrollBar:IsVisible()")
            .unwrap(),
        "the scrollbar shows once the menu overflows"
    );

    // A quad carries a clip rect rather than a shrunk rect: each row's clip is the frame's rect.
    let (frame_top, frame_bottom): (f32, f32) = s
        .eval("return GossipGreetingScrollFrame:GetTop(), GossipGreetingScrollFrame:GetBottom()")
        .unwrap();
    let painted_rows =
        |s: &mut UiScript| -> Vec<(benilla_ui::layout::Rect, benilla_ui::layout::Rect)> {
            s.extract()
                .into_iter()
                .filter_map(|q| match &q.content {
                    QuadContent::Text { text: Some(t), .. } if t.starts_with("Option ") => {
                        Some((q.rect?, q.clip?))
                    }
                    _ => None,
                })
                .collect()
        };
    let rows = painted_rows(&mut s);
    assert_eq!(rows.len(), 8, "all eight option labels extract");
    for (rect, clip) in &rows {
        assert!(
            (clip.top - frame_top).abs() < 0.5 && (clip.bottom - frame_bottom).abs() < 0.5,
            "row clipped to the scroll frame [{frame_bottom}, {frame_top}], got {clip:?}"
        );
        let _ = rect;
    }
    assert!(
        rows.iter().any(|(rect, _)| rect.top < frame_bottom),
        "the menu overflows: some row is entirely below the frame"
    );

    // Our kit's `BenillaScroll_Step` pans 100 px; rects are y-up, so row 1's top grows by 100.
    let first_top =
        |s: &mut UiScript| -> f32 { s.eval::<f32>("return GossipTitleButton1:GetTop()").unwrap() };
    let before = first_top(&mut s);
    s.run("BenillaScroll_Step(GossipGreetingScrollFrame, 100)")
        .unwrap();
    s.resolve();
    let after = first_top(&mut s);
    assert!(
        (after - before - 100.0).abs() < 0.5,
        "scrolling 100px lifts the content 100px: {before} → {after}"
    );
    assert_eq!(
        s.eval::<f32>("return GossipGreetingScrollFrameScrollBar:GetValue()")
            .unwrap(),
        100.0,
        "the bar seats at the scroll offset"
    );
    for (_, clip) in painted_rows(&mut s) {
        assert!(
            (clip.top - frame_top).abs() < 0.5 && (clip.bottom - frame_bottom).abs() < 0.5,
            "a scrolled row still clips to the frame, got {clip:?}"
        );
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// ── The panel manager as an addon reaches it ──

/// An addon makes its frame a panel with a `UIPanelWindows` row, and it seats where a client
/// window does.
#[test]
fn an_addons_own_frame_registered_in_uipanelwindows_takes_the_left_slot() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);

    s.run(
        r#"AddonPanel = CreateFrame("Frame", "AddonPanel", UIParent)
           AddonPanel:SetWidth(384) AddonPanel:SetHeight(512)
           AddonPanel:Hide()
           UIPanelWindows["AddonPanel"] = { area = "left", pushable = 1 }
           ShowUIPanel(AddonPanel)"#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return AddonPanel:IsVisible()").unwrap(),
        "ShowUIPanel showed the addon's frame"
    );
    s.resolve();
    let win = frame_rect(&s.extract(), 384.0, 512.0);
    assert_eq!(
        (win.left, win.top),
        (0.0, 664.0),
        "the addon's panel is at the left slot, where a client panel would be"
    );

    // A pushable-0 window arriving moves the addon's pushable-1 frame to center
    // (`UIParent.lua`); only two pushable-0 rows replace each other (`:729-732`).
    s.set_gossip(Some(GossipMenu {
        greeting: "Well met.".into(),
        quests: Vec::new(),
        options: Vec::new(),
        ..Default::default()
    }));
    s.fire_event("GOSSIP_SHOW", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s.eval::<bool>("return GossipFrame:IsVisible()").unwrap());
    assert!(
        s.eval::<bool>("return AddonPanel:IsVisible()").unwrap(),
        "the addon's pushable=1 panel was PUSHED, not closed"
    );
    s.resolve();
    assert!(
        s.extract().iter().any(|q| q
            .rect
            .is_some_and(|r| (r.left - 384.0).abs() < 0.5 && (r.top - 664.0).abs() < 0.5)),
        "and it is at the center slot (UIParent TOPLEFT +384, -104)"
    );

    s.run("HideUIPanel(AddonPanel)").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// ── The registry, pinned to the reference's rows ──

/// The registry rows as `UIParent.lua` writes them. An unregistered frame opens with a bare
/// `Show` and takes no slot, so a missing row lets the next panel seat over it.
#[test]
fn the_panel_registry_rows_match_the_reference_bytes() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = stock_ui(1024.0, 768.0);
    for probe in [
        "UIPanelWindows['ItemTextFrame'].area == 'left'",
        "UIPanelWindows['ItemTextFrame'].pushable == 0",
        "UIPanelWindows['ItemTextFrame'].whileDead == nil",
        "UIPanelWindows['CharacterFrame'].pushable == 3",
        "UIPanelWindows['CharacterFrame'].whileDead == 1",
        "UIPanelWindows['SpellBookFrame'].whileDead == 1",
        // 3.3.5a's quest log is a doublewide panel.
        "UIPanelWindows['QuestLogFrame'].area == 'doublewide'",
        "UIPanelWindows['QuestLogFrame'].whileDead == 1",
        "#UIChildWindows == 8",
        "UIChildWindows[1] == 'OpenMailFrame'",
        "UIChildWindows[2] == 'GuildControlPopupFrame'",
        "UIChildWindows[3] == 'GuildMemberDetailFrame'",
        "UIChildWindows[4] == 'TokenFramePopup'",
        "UIChildWindows[5] == 'GuildInfoFrame'",
        "UIChildWindows[6] == 'PVPTeamDetails'",
        "UIChildWindows[7] == 'GuildBankPopupFrame'",
        "UIChildWindows[8] == 'GearManagerDialog'",
    ] {
        assert!(
            s.eval::<bool>(&format!("return {probe}")).unwrap(),
            "registry drifted from the bytes: {probe}"
        );
    }
}

/// While dead, `ShowUIPanel` opens only a `whileDead` row (`UIParent.lua`).
#[test]
fn a_dead_player_opens_only_whiledead_windows() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = stock_ui(1024.0, 768.0);
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            max_health: 100,
            health: 0,
            dead: true,
            ..Default::default()
        }),
    );

    // Bare stand-ins suffice: the guard runs before any seat is chosen.
    s.run(
        r#"local g = CreateFrame("Frame", "GossipFrame") g:SetWidth(50); g:SetHeight(50) g:Hide()
           local q = CreateFrame("Frame", "QuestLogFrame") q:SetWidth(50); q:SetHeight(50) q:Hide()
           ShowUIPanel(GossipFrame)"#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        !s.eval::<bool>("return GossipFrame:IsShown()").unwrap(),
        "a row without whileDead is refused while dead"
    );
    assert!(
        s.eval::<bool>("return GetUIPanel('left') == nil").unwrap(),
        "the refused window took no slot"
    );
    // `NotWhileDeadError` (error 0x7e) queues its key for the app to show.
    assert_eq!(
        s.take_ui_errors(),
        vec!["ERR_PLAYER_DEAD"],
        "the dead refusal queued its error key"
    );

    s.run("ShowUIPanel(QuestLogFrame)").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return QuestLogFrame:IsShown()").unwrap(),
        "a whileDead = 1 row opens exactly as alive"
    );
    assert!(
        s.eval::<bool>("return GetUIPanel('doublewide'):GetName() == 'QuestLogFrame'")
            .unwrap(),
        "and it seats normally, in the doublewide slot"
    );
    assert!(
        s.take_ui_errors().is_empty(),
        "an admitted open raises no error"
    );
}

/// `FramePositionDelegate:SetUIPanel` runs `CloseChildWindows` whenever it seats a frame, in any
/// slot, so the open letter goes whether the newcomer lands at center or pushes another there.
/// Stand-ins carry the merchant (left, 0), trade (left, 1) and loot (left, 7) rows.
#[test]
fn a_seated_panel_puts_the_child_windows_away() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = stock_ui(1024.0, 768.0);
    s.run(
        r#"for name, push in pairs({ ProbeMerchant = 0, ProbeTrade = 1, ProbeLoot = 7 }) do
               UIPanelWindows[name] = { area = "left", pushable = push }
               local f = CreateFrame("Frame", name, UIParent)
               f:SetWidth(50); f:SetHeight(50); f:Hide()
           end
           OpenMailFrame:Show()
           ShowUIPanel(ProbeMerchant)
           OpenMailFrame:Show()
           ShowUIPanel(ProbeTrade)"#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return GetUIPanel('center'):GetName() == 'ProbeTrade'")
            .unwrap(),
        "the pushable=1 newcomer arrived at the center seat"
    );
    assert!(
        !s.eval::<bool>("return OpenMailFrame:IsShown()").unwrap(),
        "the arriving center frame hid the open letter"
    );

    // Loot (7) holds left; a pushable-0 window slides it to center and still closes the letter.
    s.run(
        r#"HideUIPanel(ProbeTrade) HideUIPanel(ProbeMerchant)
           ShowUIPanel(ProbeLoot)
           OpenMailFrame:Show()
           ShowUIPanel(ProbeMerchant)"#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return GetUIPanel('center'):GetName() == 'ProbeLoot'")
            .unwrap(),
        "loot slid to center, merchant took left"
    );
    assert!(
        !s.eval::<bool>("return OpenMailFrame:IsShown()").unwrap(),
        "seating the merchant at left closed the letter too"
    );
}
