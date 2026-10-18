//! The unit right-click popups, reached by right-clicking a target frame, a chat name or the pet
//! frame through the real hit paths. All drive `UnitPopup.lua`; a chat name goes through
//! `SetItemRef`'s player branch to the FRIEND dropdown in `FriendsFrame.xml`.

use benilla_ui::script::{FollowRequest, PartyRequest, UiScript, UnitState};

use super::test_ui::load_ui as load_xml;

/// The files the popups need, in the production order, ending with `FriendsFrame.xml`, home of
/// `FriendsFrame_ShowDropdown` and `FriendsDropDown`.
fn load_popup_frames(s: &UiScript) {
    for file in [
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\Fonts.xml",
        "Interface\\FrameXML\\BasicControls.xml", // `TEXT`, read at file scope below
        "Interface\\FrameXML\\LocaleProperties.lua",
        r"Interface\FrameXML\UIParent.xml",
        // `SmallMoneyFrame_OnLoad`, which the chain's StaticPopup money rows call at load.
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIMenu.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        // `StaticPopupDialogs` and `PanelTemplates_*`, which FriendsFrame.xml reaches at load.
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\CharacterFrameTemplates.xml",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\BuffFrame.xml",
        "Interface\\FrameXML\\CombatFeedback.xml",
        "Interface\\FrameXML\\UnitPopup.xml",
        "Interface\\FrameXML\\UnitFrame.xml",
        "Interface\\FrameXML\\PlayerFrame.xml",
        "Interface\\FrameXML\\PartyFrame.xml",
        "Interface\\FrameXML\\TargetFrame.xml",
        "Interface\\FrameXML\\PetFrame.xml",
        "Interface\\FrameXML\\ChatFrame.xml",
        // Declares `ChatFrameEditBox`, which the rename dialog's OnHide refocuses.
        "Interface\\FrameXML\\FloatingChatFrame.xml",
        "Interface\\FrameXML\\FriendsFrame.xml",
        "Interface\\FrameXML\\ItemRef.xml",
        "ScrollTemplates.xml",
    ] {
        load_xml(s, file);
    }
}

/// The FRIEND and PLAYER menus' labels and the target-frame newbie tip, as the stock
/// `GlobalStrings.lua` has them.
fn bake_strings(s: &UiScript) {
    s.run(
        r#"
        WHISPER = "Whisper"
        PARTY_INVITE = "Invite"
        TRADE = "Trade"
        DUEL = "Duel"
        -- The inspect row's label: the value at `Interface\FrameXML\GlobalStrings.lua:2327` off
        -- the 1.12.1 patch chain, which is what the app itself runs at boot
        -- (`load_global_strings`); this stub only stands in for it here.
        INSPECT = "Inspect"
        -- The follow row's label, likewise the real
        -- `GlobalStrings.lua:1981` value off the 1.12.1 patch chain.
        FOLLOW = "Follow"
        CANCEL = "Cancel"
        RAID_TARGET_ICON = "Raid Target Icon"
        -- The newbie tooltip the stock unit frame raises on a HOVER, which every test in this file
        -- takes on its way to a right-click. `UnitFrame_OnEnter` (ref `UnitFrame.lua:58-65`) runs
        -- the detailed-tip branch whenever `SHOW_NEWBIE_TIPS == "1"` — 1.12's own default
        -- (`UIOptionsFrame.lua:100`, which the app loads off the patch chain; this harness loads
        -- no options file, so `SHOW_NEWBIE_TIPS` is nil and the branch is not reached
        -- here). For a player-controlled target that is not us that branch calls
        -- `GameTooltip_AddNewbieTip(PLAYER_OPTIONS_LABEL, 1, 1, 1, NEWBIE_TOOLTIP_PLAYEROPTIONS)`.
        -- Both are nil in a bare harness, and `GameTooltip:SetText(nil)` raises. Verbatim from the
        -- real `Interface\FrameXML\GlobalStrings.lua` off the 1.12.1 chain (l.3081 and l.2755).
        -- The stock unit frame reaches this arm, so both strings must exist.
        PLAYER_OPTIONS_LABEL = "Player Options"
        NEWBIE_TOOLTIP_PLAYEROPTIONS = "Right-click to bring up special commands for interacting with another player. You can inspect their equipment, issue a party invite, initiate a trade, or challenge a player to a duel. A group leader can promote or remove that player from the group."
    "#,
    )
    .unwrap();
}

/// `/invite`, `/uninvite`, `/promote` and a chat name's Invite row reach the 3.3.5a verbs (#190).
#[test]
fn chat_name_menu_and_the_party_slash_commands_queue_the_3_3_5a_verbs() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, failures) =
        super::layer_tests::production_load_with("partyverbs", false, "", |_| {});
    let ours: Vec<_> = failures
        .iter()
        .filter(|f| f.contains("ChatFrame.lua") || f.contains("FriendsDropDown"))
        .collect();
    assert!(ours.is_empty(), "load failures: {ours:#?}");
    s.resolve();
    s.take_errors();
    s.take_party_requests();

    for line in ["/invite Bob", "/uninvite Bob", "/promote Bob"] {
        s.run(&format!(
            "local eb = ChatFrame1EditBox \
             ChatEdit_ActivateChat(eb) eb:SetText({line:?}) ChatEdit_SendText(eb, 1)"
        ))
        .unwrap();
        assert_eq!(
            s.eval::<String>("return ChatFrame1EditBox:GetText()")
                .unwrap(),
            "",
            "{line} clears the box"
        );
    }
    let errors: Vec<_> = s.take_errors();
    assert!(errors.is_empty(), "slash errors: {errors:#?}");
    assert_eq!(
        s.take_party_requests(),
        vec![
            PartyRequest::InviteUnit {
                name: "Bob".into(),
                flags: 0
            },
            PartyRequest::UninviteUnit {
                unit: "Bob".into(),
                reason: String::new()
            },
            PartyRequest::PromoteToLeader("Bob".into()),
        ]
    );

    // Right-click the name link (`SetItemRef`), then the menu's Invite row through the hit path.
    s.run(r#"SetItemRef("player:Bob", "|Hplayer:Bob|h[Bob]|h", "RightButton")"#)
        .unwrap();
    s.resolve();
    let row: i64 = s
        .eval(
            "for i = 1, DropDownList1.numButtons do \
               if _G['DropDownList1Button' .. i]:GetText() == PARTY_INVITE then return i end \
             end return 0",
        )
        .unwrap();
    assert!(row > 0, "the FRIEND menu has an Invite row");
    let (ix, iy) = s
        .eval::<(f64, f64)>(&format!("return DropDownList1Button{row}:GetCenter()"))
        .unwrap();
    s.mouse_move(ix as f32, iy as f32);
    s.mouse_button(ix as f32, iy as f32, "LeftButton", true);
    s.mouse_button(ix as f32, iy as f32, "LeftButton", false);
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::InviteUnit {
            name: "Bob".into(),
            flags: 0
        }],
        "Invite on a chat name queues InviteUnit(name)"
    );

    let errors: Vec<_> = s.take_errors();
    assert!(errors.is_empty(), "script errors: {errors:#?}");
}

/// Solo, right-clicking a friendly player target opens the PLAYER menu; Invite invites the unit.
#[test]
#[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
fn solo_target_right_click_invites_a_player() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    bake_strings(&s);
    load_popup_frames(&s);

    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Me".into()),
            is_player: true,
            player_controlled: true,
            ..UnitState::default()
        }),
    );
    // A same-faction friendly PLAYER target (is_player + reaction 5 → UnitCanCooperate == 1).
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Ally".into()),
            health: 40,
            max_health: 40,
            is_player: true,
            player_controlled: true,
            reaction: 5,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s.resolve();
    assert!(s.errors().is_empty(), "load errors: {:?}", s.errors());

    let (cx, cy) = s
        .eval::<(f64, f64)>("return TargetFrame:GetCenter()")
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    s.resolve();
    assert!(
        s.eval::<bool>("return DropDownList1:IsVisible()").unwrap(),
        "a friendly player target opens the PLAYER menu solo"
    );
    // Title, Whisper, Inspect, Invite, Trade, Follow, Duel, Cancel: the PLAYER menu in
    // `UnitPopup.lua:72`'s order, the raid-target row hidden without a party.
    assert_eq!(
        s.eval::<i64>("return DropDownList1.numButtons").unwrap(),
        8,
        "title + Whisper + Inspect + Invite + Trade + Follow + Duel + Cancel"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button3:GetText()")
            .unwrap(),
        "Inspect"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button4:GetText()")
            .unwrap(),
        "Invite"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button5:GetText()")
            .unwrap(),
        "Trade"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button6:GetText()")
            .unwrap(),
        "Follow"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button7:GetText()")
            .unwrap(),
        "Duel"
    );

    // Click Invite → UnitPopup_OnClick → InviteUnit(fullname).
    let (ix, iy) = s
        .eval::<(f64, f64)>("return DropDownList1Button4:GetCenter()")
        .unwrap();
    s.mouse_button(ix as f32, iy as f32, "LeftButton", true);
    s.mouse_button(ix as f32, iy as f32, "LeftButton", false);
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::InviteUnit {
            name: "Ally".into(),
            flags: 0
        }],
        "Invite on a player target invites the target's name"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Clicking the Trade row calls `InitiateTrade("target")` (`UnitPopup.lua:544`).
#[test]
#[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
fn solo_target_trade_click_queues_an_initiate() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    bake_strings(&s);
    load_popup_frames(&s);

    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Me".into()),
            is_player: true,
            player_controlled: true,
            ..UnitState::default()
        }),
    );
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Ally".into()),
            health: 40,
            max_health: 40,
            is_player: true,
            player_controlled: true,
            reaction: 5,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s.resolve();
    assert!(s.errors().is_empty(), "load errors: {:?}", s.errors());

    let (cx, cy) = s
        .eval::<(f64, f64)>("return TargetFrame:GetCenter()")
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    s.resolve();
    assert_eq!(
        s.eval::<String>("return DropDownList1Button5:GetText()")
            .unwrap(),
        "Trade",
        "Trade is the fifth row (title + Whisper + Inspect + Invite + Trade) — Inspect sits \
         ahead of it"
    );

    let (tx, ty) = s
        .eval::<(f64, f64)>("return DropDownList1Button5:GetCenter()")
        .unwrap();
    s.mouse_button(tx as f32, ty as f32, "LeftButton", true);
    s.mouse_button(tx as f32, ty as f32, "LeftButton", false);
    assert_eq!(
        s.take_trade_initiates(),
        vec!["target".to_string()],
        "clicking Trade queues an initiate against the target token"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Clicking Follow calls `FollowByName("Ally", 1)` (`UnitPopup.lua:623`): the `1` asks for an
/// exact match, so the menu never prefix-matches onto a bystander as `/follow rag` may.
#[test]
#[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
fn solo_target_follow_click_queues_an_exact_by_name_follow() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    bake_strings(&s);
    load_popup_frames(&s);

    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Me".into()),
            is_player: true,
            player_controlled: true,
            ..UnitState::default()
        }),
    );
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Ally".into()),
            health: 40,
            max_health: 40,
            is_player: true,
            player_controlled: true,
            reaction: 5,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s.resolve();
    assert!(s.errors().is_empty(), "load errors: {:?}", s.errors());

    let (cx, cy) = s
        .eval::<(f64, f64)>("return TargetFrame:GetCenter()")
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    s.resolve();
    assert_eq!(
        s.eval::<String>("return DropDownList1Button6:GetText()")
            .unwrap(),
        "Follow",
        "Follow is the sixth row (title + Whisper + Inspect + Invite + Trade + Follow)"
    );

    let (fx, fy) = s
        .eval::<(f64, f64)>("return DropDownList1Button6:GetCenter()")
        .unwrap();
    s.mouse_button(fx as f32, fy as f32, "LeftButton", true);
    s.mouse_button(fx as f32, fy as f32, "LeftButton", false);
    assert_eq!(
        s.take_follow_requests(),
        vec![FollowRequest::ByName {
            name: "Ally".into(),
            exact: true,
        }],
        "the popup follows by NAME and exactly — not by unit token, and not prefix-matched"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Clicking Inspect calls `InspectUnit("target")` (`UnitPopup.lua:552`), stubbed here;
/// `inspect_tests.rs` covers the window.
#[test]
#[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
fn solo_target_inspect_click_reaches_inspect_unit() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    bake_strings(&s);
    load_popup_frames(&s);
    s.run("BENILLA_TEST_INSPECTED = nil; function InspectUnit(unit) BENILLA_TEST_INSPECTED = unit end")
        .unwrap();

    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Me".into()),
            is_player: true,
            player_controlled: true,
            ..UnitState::default()
        }),
    );
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Ally".into()),
            health: 40,
            max_health: 40,
            is_player: true,
            player_controlled: true,
            reaction: 5,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s.resolve();

    let (cx, cy) = s
        .eval::<(f64, f64)>("return TargetFrame:GetCenter()")
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    s.resolve();
    assert_eq!(
        s.eval::<String>("return DropDownList1Button3:GetText()")
            .unwrap(),
        "Inspect",
        "Inspect is the third row (title + Whisper + Inspect)"
    );

    let (ix, iy) = s
        .eval::<(f64, f64)>("return DropDownList1Button3:GetCenter()")
        .unwrap();
    s.mouse_button(ix as f32, iy as f32, "LeftButton", true);
    s.mouse_button(ix as f32, iy as f32, "LeftButton", false);
    assert_eq!(
        s.eval::<String>("return BENILLA_TEST_INSPECTED").unwrap(),
        "target",
        "clicking Inspect calls InspectUnit against the target token"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// ── The PET menu ─────────────────────────────────────────────────────────────────────

/// The pet menu's files, with `StaticPopup.xml` for the rename and abandon dialogs.
fn load_pet_menu_frames(s: &UiScript) {
    for file in [
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\Fonts.xml",
        "Interface\\FrameXML\\BasicControls.xml", // `TEXT`, read at file scope below
        "Interface\\FrameXML\\LocaleProperties.lua",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIMenu.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BuffFrame.xml",
        "Interface\\FrameXML\\CombatFeedback.xml",
        "Interface\\FrameXML\\UnitPopup.xml",
        "Interface\\FrameXML\\UnitFrame.xml",
        "Interface\\FrameXML\\PlayerFrame.xml",
        "Interface\\FrameXML\\PartyFrame.xml",
        "Interface\\FrameXML\\TargetFrame.xml",
        "Interface\\FrameXML\\PetFrame.xml",
        "Interface\\FrameXML\\ChatFrame.xml",
        // Declares `ChatFrameEditBox`, which the rename dialog's OnHide refocuses.
        "Interface\\FrameXML\\FloatingChatFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
    ] {
        load_xml(s, file);
    }
}

/// The PET rows' labels and both dialogs' text as the stock strings have them
/// (`GlobalStrings.lua:3`, `:3028-3052`), and a `ToggleCharacter` that records its panel.
fn bake_pet_strings(s: &UiScript) {
    s.run(
        r#"
        PET_PAPERDOLL = "Pet Details"
        PET_RENAME = "Rename"
        PET_ABANDON = "Abandon"
        PET_DISMISS = "Dismiss"
        CANCEL = "Cancel"
        OKAY = "Okay"
        ACCEPT = "Accept"
        YES = "Yes"
        NO = "No"
        ABANDON_PET = "Are you sure you want to permanently abandon your pet?"
        PET_RENAME_LABEL = "Enter desired name of pet:"
        PET_RENAME_CONFIRMATION = "Name your pet '%s'?"
        -- ToggleCharacter lives in CharacterFrame.xml, which the paperdoll row needs and this
        -- isolation prefix does not load. Recorded rather than stubbed away, so the row's click
        -- is still proven to reach the right panel name.
        function ToggleCharacter(tab) BENILLA_TEST_TOGGLED = tab end
    "#,
    )
    .unwrap();
}

/// A pet that exists, so `UnitExists("pet")` passes the dropdown's own gate.
fn a_pet(s: &mut UiScript, name: &str) {
    // A player first: `PetFrame` is a child of `PlayerFrame` (`PetFrame.xml:4`), and a unit
    // frame whose unit does not exist is hidden, taking the pet frame with it.
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Me".into()),
            health: 100,
            max_health: 100,
            level: 60,
            is_player: true,
            player_controlled: true,
            ..UnitState::default()
        }),
    );
    s.fire_event(
        "UNIT_HEALTH",
        vec![benilla_ui::script::ScriptValue::Str("player".into())],
    );
    s.set_unit(
        "pet",
        Some(UnitState {
            exists: true,
            name: Some(name.into()),
            health: 100,
            max_health: 100,
            level: 20,
            ..UnitState::default()
        }),
    );
    s.fire_event(
        "UNIT_PET",
        vec![benilla_ui::script::ScriptValue::Str("player".into())],
    );
}

/// Right-clicks the centre of the pet frame's hit rect, the portrait half (`PetFrame.xml:15-17`).
/// The frame's own centre lies on `PetFrameHealthBar` (`PetFrame.xml:125-135`), mouse-enabled by
/// its `TextStatusBar` OnEnter and forwarding no `OnMouseUp`, so a click there opens nothing, in
/// the reference too.
fn right_click_the_pet_frame(s: &mut UiScript) {
    s.resolve();
    let (cx, cy) = s
        .eval::<(f64, f64)>(
            "local il, ir, it, ib = PetFrame:GetHitRectInsets() \
             return (PetFrame:GetLeft() + il + PetFrame:GetRight() - ir) / 2, \
                    (PetFrame:GetBottom() + ib + PetFrame:GetTop() - it) / 2",
        )
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    s.resolve();
}

/// A hunter's pet shows Abandon and hides Dismiss, a warlock's demon the reverse:
/// `PetCanBeAbandoned` forks the menu (`UnitPopup.lua:402-417`).
#[test]
#[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
fn the_pet_menu_forks_between_abandon_and_dismiss() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    bake_pet_strings(&s);
    load_pet_menu_frames(&s);
    a_pet(&mut s, "Bruce");

    // A freshly tamed hunter pet: both bits set.
    s.set_pet_menu(true, true, true);
    right_click_the_pet_frame(&mut s);
    assert!(
        s.eval::<bool>("return DropDownList1:IsVisible()").unwrap(),
        "right-clicking the pet frame opens the PET menu"
    );
    assert_eq!(
        s.eval::<i64>("return DropDownList1.numButtons").unwrap(),
        5,
        "title + Pet Details + Rename + Abandon + Cancel — no Dismiss"
    );
    for (row, text) in [
        (2, "Pet Details"),
        (3, "Rename"),
        (4, "Abandon"),
        (5, "Cancel"),
    ] {
        assert_eq!(
            s.eval::<String>(&format!("return DropDownList1Button{row}:GetText()"))
                .unwrap(),
            text
        );
    }

    // The same pet after one rename: the server clears only the rename bit, so only that row goes.
    s.run("CloseDropDownMenus()").unwrap();
    s.set_pet_menu(true, false, true);
    right_click_the_pet_frame(&mut s);
    assert_eq!(
        s.eval::<i64>("return DropDownList1.numButtons").unwrap(),
        4,
        "title + Pet Details + Abandon + Cancel"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button3:GetText()")
            .unwrap(),
        "Abandon",
        "Rename is gone and Abandon has moved up into its row"
    );

    // A warlock's demon: neither bit. The menu flips whole.
    s.run("CloseDropDownMenus()").unwrap();
    s.set_pet_menu(false, false, true);
    right_click_the_pet_frame(&mut s);
    assert_eq!(
        s.eval::<i64>("return DropDownList1.numButtons").unwrap(),
        3,
        "title + Dismiss + Cancel"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button2:GetText()")
            .unwrap(),
        "Dismiss"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Dismiss sends at once; Abandon waits behind the `ABANDON_PET` confirm (`UnitPopup.lua:590-593`).
#[test]
#[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
fn dismiss_sends_immediately_and_abandon_waits_for_the_confirm() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    bake_pet_strings(&s);
    load_pet_menu_frames(&s);
    a_pet(&mut s, "Snuffles");

    // A demon: Dismiss is row 2, and clicking it queues the verb with no dialog in between.
    s.set_pet_menu(false, false, true);
    right_click_the_pet_frame(&mut s);
    click_row(&mut s, 2);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "Dismiss asks nothing"
    );
    assert_eq!(s.take_pet_gives_up(), (0, 1), "one dismiss, no abandon");

    // A hunter pet: Abandon is row 4, and clicking it only opens the confirm.
    s.set_pet_menu(true, true, true);
    right_click_the_pet_frame(&mut s);
    click_row(&mut s, 4);
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "Abandon opens the ABANDON_PET confirm"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Are you sure you want to permanently abandon your pet?"
    );
    assert_eq!(
        s.take_pet_gives_up(),
        (0, 0),
        "and sends NOTHING until it is accepted"
    );

    click_frame(&mut s, "StaticPopup1Button2");
    assert_eq!(s.take_pet_gives_up(), (0, 0));
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());

    right_click_the_pet_frame(&mut s);
    click_row(&mut s, 4);
    click_frame(&mut s, "StaticPopup1Button1");
    assert_eq!(s.take_pet_gives_up(), (1, 0), "one abandon");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The rename chains two dialogs, `RENAME_PET` then `PETRENAMECONFIRM` (`StaticPopup.lua:1069`,
/// `:365`), and only the confirm sends. The confirm opens while the name dialog is still up, so
/// it lands in the second popup instance.
#[test]
#[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
fn renaming_a_pet_reads_the_name_back_before_sending_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    bake_pet_strings(&s);
    load_pet_menu_frames(&s);
    a_pet(&mut s, "Bruce");
    s.set_pet_menu(true, true, true);

    right_click_the_pet_frame(&mut s);
    click_row(&mut s, 3);
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Enter desired name of pet:"
    );
    assert!(
        s.eval::<bool>("return StaticPopup1EditBox:IsVisible()")
            .unwrap(),
        "the name dialog carries an edit box"
    );

    s.run(r#"StaticPopup1EditBox:SetText("Rexxar")"#).unwrap();
    click_frame(&mut s, "StaticPopup1Button1");
    assert_eq!(
        s.take_pet_renames(),
        Vec::<String>::new(),
        "accepting the NAME dialog sends nothing — it only asks again"
    );
    assert!(
        s.eval::<bool>("return StaticPopup2:IsVisible()").unwrap(),
        "the confirm opens in the second instance, while the first is still up"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup2Text:GetText()")
            .unwrap(),
        "Name your pet 'Rexxar'?",
        "and it reads the typed name back"
    );

    click_frame(&mut s, "StaticPopup2Button1");
    assert_eq!(
        s.take_pet_renames(),
        vec!["Rexxar".to_string()],
        "only the CONFIRM sends, and it sends the name from the box"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The Pet Details row calls `ToggleCharacter("PetPaperDollFrame")` (`UnitPopup.lua:594-595`).
#[test]
#[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
fn the_pet_details_row_opens_the_pet_paper_doll() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    bake_pet_strings(&s);
    load_pet_menu_frames(&s);
    a_pet(&mut s, "Bruce");
    s.set_pet_menu(true, true, true);

    right_click_the_pet_frame(&mut s);
    click_row(&mut s, 2);
    assert_eq!(
        s.eval::<String>("return BENILLA_TEST_TOGGLED").unwrap(),
        "PetPaperDollFrame"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Click an open dropdown row through the real hit path.
fn click_row(s: &mut UiScript, row: u32) {
    click_frame(s, &format!("DropDownList1Button{row}"));
}

/// Click any named frame through the real hit path.
fn click_frame(s: &mut UiScript, name: &str) {
    let (x, y) = s
        .eval::<(f64, f64)>(&format!("return {name}:GetCenter()"))
        .unwrap();
    s.mouse_button(x as f32, y as f32, "LeftButton", true);
    s.mouse_button(x as f32, y as f32, "LeftButton", false);
    s.resolve();
}

/// The stock 3.3.5a files a target-frame right-click runs through, in `FrameXML.toc` order:
/// `SecureTemplates.xml` for the target frame's `SecureUnitButtonTemplate` click,
/// `PlayerFrame.xml` for the `PlayerFrame` `TargetFrame_UpdateAuras` reads, and `PVPFrame.xml`
/// for the `PVPTeamDetails` `UnitPopup_OnUpdate` reads. The load's own errors are returned.
fn load_target_menu_335(s: &mut UiScript) -> Vec<String> {
    let mut errors = Vec::new();
    s.register_cvars(crate::cvars::registered_pairs());
    for entry in super::test_ui::production_order(&[&[
        r"Interface\FrameXML\GlobalStrings.lua",
        r"Interface\FrameXML\Fonts.xml",
        r"Interface\FrameXML\BasicControls.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\GameTooltip.xml",
        r"Interface\FrameXML\UIDropDownMenu.xml",
        r"Interface\FrameXML\SecureTemplates.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        r"Interface\FrameXML\TextStatusBar.lua",
        r"Interface\FrameXML\TextStatusBar.xml",
        r"Interface\FrameXML\CombatFeedback.xml",
        r"Interface\FrameXML\CastingBarFrame.xml",
        r"Interface\FrameXML\UnitPopup.xml",
        r"Interface\FrameXML\UnitFrame.xml",
        r"Interface\FrameXML\PlayerFrame.xml",
        r"Interface\FrameXML\TargetFrame.xml",
        r"Interface\FrameXML\PVPFrame.xml",
    ]]) {
        let path = entry.replace('\\', "/");
        let bytes = super::test_ui::read(&path).unwrap_or_else(|| panic!("{entry}: not found"));
        if path.to_ascii_lowercase().ends_with(".lua") {
            if let Err(e) = s.run_chunk_named(&bytes, &format!("@{entry}")) {
                errors.push(e.to_string());
            }
            continue;
        }
        let doc = benilla_ui::framexml::parse(&benilla_ui::source::decode(&bytes))
            .unwrap_or_else(|e| panic!("{entry}: {e}"));
        let provider = |req: &str| super::test_ui::read(req);
        errors.extend(benilla_ui::loader::load_in(s, &doc, &path, &provider).errors);
    }
    errors.extend(s.take_errors());
    errors
}

/// A right-click on the 3.3.5a target frame over a friendly player opens the PLAYER menu. The
/// click runs `SecureUnitButton_OnClick`, whose unit lookup asks `UnitHasVehicleUI`
/// (`SecureTemplates.lua:193`), and the menu's rows ask `IsReferAFriendLinked`
/// (`UnitPopup.lua:788`); then the popup's per-frame update runs over the open menu.
#[test]
fn a_3_3_5a_target_frame_right_click_opens_the_player_menu() {
    benilla_formats::wow_data_or_skip!();
    const TARGET: u64 = 2;
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_target_menu_335(&mut s);

    let held_player = |name: &str, guid: u64, reaction: u8| UnitState {
        exists: true,
        has_object: true,
        name: Some(name.into()),
        health: 40,
        max_health: 40,
        level: 5,
        is_player: true,
        player_controlled: true,
        is_connected: true,
        reaction,
        guid,
        ..UnitState::default()
    };
    s.set_unit("player", Some(held_player("Me", 1, 0)));
    s.set_unit("target", Some(held_player("Ally", TARGET, 5)));
    s.set_unit_guids(&benilla_ui::script::UnitGuids {
        player: 1,
        target: TARGET,
        ..Default::default()
    });
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s.resolve();
    assert!(
        s.eval::<bool>("return TargetFrame:IsShown()").unwrap(),
        "fixture: the target frame shows on a target"
    );
    let target_errors = s.take_errors();
    assert!(
        target_errors.is_empty(),
        "target errors: {target_errors:#?}"
    );

    let (cx, cy) = s
        .eval::<(f64, f64)>("return TargetFrame:GetCenter()")
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    s.resolve();
    let click_errors = s.take_errors();
    assert!(click_errors.is_empty(), "click errors: {click_errors:#?}");
    assert!(
        s.eval::<bool>("return DropDownList1:IsShown()").unwrap(),
        "a friendly player target opens a menu"
    );
    assert_eq!(
        s.eval::<String>("return TargetFrameDropDown.which")
            .unwrap(),
        "PLAYER"
    );
    // The PLAYER rows (`UnitPopup.lua:141`) the predicates leave shown for an unlinked, friendly,
    // ungrouped player: the Refer-a-Friend pair hides (`UnitPopup.lua:787-794`), the rest stay.
    let rows: Vec<String> = s
        .eval(
            "local t = {} for i = 2, DropDownList1.numButtons do \
             t[#t + 1] = _G['DropDownList1Button' .. i].value end return t",
        )
        .unwrap();
    assert_eq!(
        rows,
        [
            "SET_FOCUS",
            "WHISPER",
            "INSPECT",
            "INVITE",
            "ACHIEVEMENTS",
            "TRADE",
            "FOLLOW",
            "DUEL",
            "RAID_TARGET_ICON",
            "CANCEL"
        ]
    );

    s.run("UnitPopup_OnUpdate(0.1)").unwrap();
    let update_errors = s.take_errors();
    assert!(
        update_errors.is_empty(),
        "update errors: {update_errors:#?}"
    );
}

/// The 3.3.5a chain with us and a friendly, held player targeted, the target frame shown.
fn targeting_a_player_335() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_target_menu_335(&mut s);
    let held_player = |name: &str, guid: u64, reaction: u8| UnitState {
        exists: true,
        has_object: true,
        name: Some(name.into()),
        health: 40,
        max_health: 40,
        level: 5,
        is_player: true,
        player_controlled: true,
        is_connected: true,
        reaction,
        guid,
        ..UnitState::default()
    };
    s.set_unit("player", Some(held_player("Me", 1, 0)));
    s.set_unit("target", Some(held_player("Ally", 2, 5)));
    s.set_unit_guids(&benilla_ui::script::UnitGuids {
        player: 1,
        target: 2,
        ..Default::default()
    });
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s.resolve();
    s.take_errors();
    s
}

/// Right-click `frame` through the hit path, then click the open menu's row whose value is `value`.
fn menu_row(s: &mut UiScript, frame: &str, value: &str) {
    let (cx, cy) = s
        .eval::<(f64, f64)>(&format!("return {frame}:GetCenter()"))
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    s.resolve();
    let row: i64 = s
        .eval(&format!(
            "for i = 1, DropDownList1.numButtons do \
               if _G['DropDownList1Button' .. i].value == {value:?} then return i end \
             end return 0"
        ))
        .unwrap();
    assert!(row > 0, "{frame}'s menu has a {value} row");
    click_row(s, row as u32);
}

/// The player frame's SELF menu opens: its dungeon and raid difficulty rows ask
/// `GetDungeonDifficulty` and `GetRaidDifficulty` (`UnitPopup.lua:309`, `:331`).
#[test]
fn a_3_3_5a_player_frame_right_click_opens_the_self_menu() {
    benilla_formats::wow_data_or_skip!();
    let mut s = targeting_a_player_335();
    let (cx, cy) = s
        .eval::<(f64, f64)>("return PlayerFrame:GetCenter()")
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    s.resolve();
    let errors = s.take_errors();
    assert!(errors.is_empty(), "click errors: {errors:#?}");
    assert!(s.eval::<bool>("return DropDownList1:IsShown()").unwrap());
    assert_eq!(
        s.eval::<String>("return PlayerFrameDropDown.which")
            .unwrap(),
        "SELF"
    );
    s.run("UnitPopup_OnUpdate(0.1)").unwrap();
    let errors = s.take_errors();
    assert!(errors.is_empty(), "update errors: {errors:#?}");
}

/// The target menu's Set Focus row focuses the target: the stock focus frame shows with its name,
/// and `ClearFocus()` hides it again (#271).
#[test]
fn the_target_menus_set_focus_row_shows_the_focus_frame() {
    benilla_formats::wow_data_or_skip!();
    let mut s = targeting_a_player_335();
    assert!(!s.eval::<bool>("return FocusFrame:IsShown()").unwrap());
    menu_row(&mut s, "TargetFrame", "SET_FOCUS");
    let errors = s.take_errors();
    assert!(errors.is_empty(), "set focus errors: {errors:#?}");
    assert_eq!(s.focus_guid(), 2);
    assert!(s.eval::<bool>("return FocusFrame:IsShown()").unwrap());
    assert_eq!(
        s.eval::<String>("return FocusFrameTextureFrameName:GetText()")
            .unwrap(),
        "Ally"
    );
    s.run("ClearFocus()").unwrap();
    s.resolve();
    assert!(!s.eval::<bool>("return FocusFrame:IsShown()").unwrap());
    let errors = s.take_errors();
    assert!(errors.is_empty(), "clear focus errors: {errors:#?}");
}

/// The target menu's Duel and Trade rows reach their verbs: `StartDuel("target", 1)` resolves the
/// target's guid (#341), `InitiateTrade("target")` queues the token.
#[test]
fn the_target_menus_duel_and_trade_rows_queue_their_verbs() {
    benilla_formats::wow_data_or_skip!();
    let mut s = targeting_a_player_335();
    s.take_duel_requests();
    menu_row(&mut s, "TargetFrame", "DUEL");
    assert_eq!(
        s.take_duel_requests(),
        vec![benilla_ui::script::DuelRequest::ChallengeGuid(2)]
    );
    menu_row(&mut s, "TargetFrame", "TRADE");
    assert_eq!(s.take_trade_initiates(), vec!["target".to_string()]);
    let errors = s.take_errors();
    assert!(errors.is_empty(), "row errors: {errors:#?}");
}
