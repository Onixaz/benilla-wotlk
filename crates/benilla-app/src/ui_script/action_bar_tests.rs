use benilla_ui::script::{ActionSlot, QuadContent, ScriptValue, UiScript};

/// The queued `UseAction` ids, without the self-cast flag `take_action_uses` carries beside them.
fn action_ids(s: &mut UiScript) -> Vec<u32> {
    s.take_action_uses().into_iter().map(|u| u.action).collect()
}

/// The stock main bar without Bevy: a stance's bonus page, its icons, a click and a key.
#[test]
fn shipped_action_bar_drives_end_to_end() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);
    // What the three stock bar files declare (MainMenuBar.xml, ActionBarFrame.xml,
    // BonusActionBarFrame.xml), each action, bonus, shapeshift and possess button with the
    // template's `$parentCooldown` (ActionButtonTemplate.xml:63).
    let mut named: Vec<String> = [
        "MainMenuBar",
        "MainMenuExpBar",
        "MainMenuBarOverlayFrame",
        "MainMenuBarMaxLevelBar",
        "MainMenuBarArtFrame",
        "MainMenuBarVehicleLeaveButton",
        "ExhaustionTick",
        "ActionBarUpButton",
        "ActionBarDownButton",
        "BonusActionBarFrame",
        "ShapeshiftBarFrame",
        "PossessBarFrame",
    ]
    .map(String::from)
    .to_vec();
    for (button, n) in [
        ("ActionButton", 12),
        ("BonusActionButton", 12),
        ("ShapeshiftButton", 10),
        ("PossessButton", 2),
    ] {
        for i in 1..=n {
            named.push(format!("{button}{i}"));
            named.push(format!("{button}{i}Cooldown"));
        }
    }
    for name in &named {
        assert!(
            s.eval::<bool>(&format!("return {name} ~= nil")).unwrap(),
            "{name} is built"
        );
    }

    // A warrior in Battle Stance: bonus offset 1, actions 73..84.
    s.set_bonus_bar_offset(1);
    s.set_action(
        73,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Ability_SteelMelee".into()),
            kind: 0x00,
            action: 100,
            count: 0,
            consumable: false,
            stackable: false,
        }),
    );
    s.set_action(
        74,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Ability_Rogue_Ambush".into()),
            kind: 0x00,
            action: 101,
            count: 0,
            consumable: false,
            stackable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    // Fired on the offset's edge; the bonus frame's slide takes two OnUpdates: start, then land.
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    s.tick(10.0);
    s.tick(0.2);
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );

    // The stance page is painted by `BonusActionButton1/2`. The 1024-wide bar's left edge is 0;
    // the 43-high bonus frame's TOPLEFT lands at the bar's BOTTOMLEFT + (4, 43), and button 1 at
    // (5, 4), 36x36: x[9,45] y[4,40], stride 36 + 6 (BonusActionBarFrame.xml:54-105).
    s.resolve();
    let quads = s.extract();
    let icon = |path: &str| {
        quads
            .iter()
            .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p == path))
            .and_then(|q| q.rect)
    };
    let r = icon("Interface\\Icons\\Ability_SteelMelee").expect("bonus button 1 icon");
    assert_eq!((r.left, r.bottom, r.right, r.top), (9.0, 4.0, 45.0, 40.0));
    let r2 = icon("Interface\\Icons\\Ability_Rogue_Ambush").expect("bonus button 2 icon");
    assert_eq!(r2.left, 9.0 + 42.0); // bonus button 2 left = 51
                                     // The main and bonus bars' own quads: other frames of the interface draw the same files.
    let on_bars = |pred: &dyn Fn(&str) -> bool| -> Vec<String> {
        quads
            .iter()
            .filter(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if pred(p)))
            .filter_map(|q| s.quad_owner_name(q.target))
            .filter(|o| o.starts_with("ActionButton") || o.starts_with("BonusActionButton"))
            .collect()
    };
    let rings = on_bars(&|p| p.contains("UI-Quickslot2"));
    assert_eq!(
        rings.len(),
        2,
        "the two occupied bonus buttons; every empty main and bonus slot is hidden: {rings:?}"
    );
    let icons = on_bars(&|p| p.contains("Icons"));
    assert_eq!(icons.len(), 2, "empty slots draw no icon quad: {icons:?}");

    // A click at (26, 22) lands on `BonusActionButton1` and queues its paged id, 73.
    s.mouse_button(26.0, 22.0, "LeftButton", true);
    s.mouse_button(26.0, 22.0, "LeftButton", false);
    assert_eq!(action_ids(&mut s), vec![73]);

    // `ACTIONBUTTONn` runs `ActionButtonDown/Up(n)` on the key's two edges (Bindings.xml:121-127).
    s.run("ActionButtonUp(2)").unwrap();
    assert!(
        s.take_action_uses().is_empty(),
        "an Up without a Down is a no-op (the PUSHED gate)"
    );
    let depressed = |s: &UiScript| {
        s.extract()
            .iter()
            .filter(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                        if p.contains("UI-Quickslot-Depress"))
            })
            .count()
    };
    s.run("ActionButtonDown(2)").unwrap();
    assert_eq!(depressed(&s), 1, "key DOWN shows the pushed texture");
    assert_eq!(
        s.eval::<String>("return BonusActionButton2:GetButtonState()")
            .unwrap(),
        "PUSHED",
        "with the overlay up a key drives the BONUS button (ActionButton.lua:15-22)"
    );
    s.run("ActionButtonUp(2)").unwrap();
    assert_eq!(action_ids(&mut s), vec![74], "key '2' fires action 74");
    assert_eq!(depressed(&s), 0, "key UP restores the normal state");

    s.set_bonus_bar_offset(0);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    // The overlay descends and hides one OnUpdate past the slide time.
    s.tick(0.2);
    s.tick(0.01);
    s.resolve();
    let icons_after = s
        .extract()
        .iter()
        .filter(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("Icons"))
        })
        .count();
    assert_eq!(icons_after, 0, "re-page to an empty page clears the icons");
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );
}

/// The whole production interface, the stock bars among it.
fn load_action_bar(s: &mut UiScript) {
    super::test_ui::load_stock_ui(s);
}

/// The state events through the stock bar: the cooldown, the checked ring and the usable tint.
#[test]
fn state_feedback_drives_cooldown_checked_and_usable_through_the_xml() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ActionState;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
            kind: 0x00,
            action: 133,
            count: 0,
            consumable: false,
            stackable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.tick(10.0); // a nonzero GetTime epoch

    // A 10 s cooldown with 6 s left: the pane shows mid-sweep.
    s.set_action_state(
        1,
        Some(ActionState {
            usable: true,
            cooldown: Some((6_000, 10_000, true)),
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
    // `CooldownFrame_SetTimer` arms sequence 0; the next paint's `OnUpdateModel` scrubs it.
    s.tick(0.0);
    let sweep = super::test_ui::cooldown_sweep(&mut s, "ActionButton1Cooldown")
        .expect("the button's cooldown is showing and armed");
    assert_eq!(
        sweep.swipe,
        super::test_ui::swept(0.4),
        "4 s of 10 s run ⇒ the wedge sits at 40 %"
    );

    // The checked ring on the current action (`ActionButton_UpdateState`).
    s.set_action_state(
        1,
        Some(ActionState {
            usable: true,
            current: true,
            cooldown: Some((6_000, 10_000, true)),
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_STATE", vec![]);
    assert!(s.eval::<bool>("return ActionButton1:GetChecked()").unwrap());

    // Out of power (not usable, not enough mana): the blue tint.
    s.set_action_state(
        1,
        Some(ActionState {
            usable: false,
            not_enough_mana: true,
            cooldown: Some((6_000, 10_000, true)),
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_USABLE", vec![]);
    s.resolve();
    let icon_color = s.extract().into_iter().find_map(|q| match &q.content {
        QuadContent::Texture {
            path: Some(p),
            color,
            ..
        } if p.contains("Spell_Fire_FlameBolt") => Some(*color),
        _ => None,
    });
    let c = icon_color.expect("icon quad").expect("vertex color set");
    assert_eq!(
        (c[0], c[1], c[2]),
        (0.5, 0.5, 1.0),
        "the ref's out-of-power blue-grey"
    );

    // Unusable for another reason (food in combat): the `else` arm greys the icon to 0.4 and
    // leaves the ring at full strength (ActionButton.lua:279-281).
    s.set_action_state(
        1,
        Some(ActionState {
            usable: false,
            not_enough_mana: false,
            cooldown: Some((6_000, 10_000, true)),
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_USABLE", vec![]);
    s.resolve();
    let c = s
        .extract()
        .into_iter()
        .find_map(|q| match &q.content {
            QuadContent::Texture {
                path: Some(p),
                color,
                ..
            } if p.contains("Spell_Fire_FlameBolt") => Some(*color),
            _ => None,
        })
        .expect("icon quad")
        .expect("vertex color set");
    assert_eq!(
        (c[0], c[1], c[2]),
        (0.4, 0.4, 0.4),
        "the ref's unusable grey — the icon a player sees on food in combat"
    );

    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );
}

/// `CooldownFrame_SetTimer` hides the frame unless `start > 0 and duration > 0 and enable > 0`
/// (Cooldown.lua:2): a running cooldown whose start predates a restarted `GetTime` never draws.
#[test]
fn a_start_behind_the_clocks_epoch_hides_the_stock_sweep() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ActionState;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
            kind: 0x00,
            action: 133,
            count: 0,
            consumable: false,
            stackable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.tick(10.0); // GetTime is 10: a clock that restarted ten seconds ago

    // A 10-minute cooldown armed 30 s ago, converted against that restarted clock: start = −20 s.
    s.set_action_state(
        1,
        Some(ActionState {
            usable: true,
            cooldown: Some((-20_000, 600_000, true)),
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
    s.tick(0.0);
    assert_eq!(
        super::test_ui::cooldown_sweep(&mut s, "ActionButton1Cooldown"),
        None,
        "the stock `start > 0` guard hides the frame outright — 9.5 minutes still to run and the \
         button shows nothing"
    );

    // The same cooldown on a clock that never restarted: GetTime 100, armed at 70.
    s.tick(90.0);
    s.set_action_state(
        1,
        Some(ActionState {
            usable: true,
            cooldown: Some((70_000, 600_000, true)),
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
    s.tick(0.0);
    assert_eq!(
        super::test_ui::cooldown_sweep(&mut s, "ActionButton1Cooldown").map(|p| p.swipe),
        Some(super::test_ui::swept(0.05)),
        "30 s of 600 s run ⇒ the wedge sits at 5 %"
    );

    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );
}

/// The cooldown child sits at its button's level + 1 and frame level outranks draw layer, so the
/// sweep paints over the button's icon and ring.
#[test]
fn the_cooldown_sweep_paints_over_the_buttons_icon_and_ring() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ActionState;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
            kind: 0x00,
            action: 133,
            count: 0,
            consumable: false,
            stackable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.tick(10.0);
    s.set_action_state(
        1,
        Some(ActionState {
            usable: true,
            cooldown: Some((6_000, 10_000, true)),
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
    s.resolve();

    let quads = s.extract();
    let pos = |pred: &dyn Fn(&QuadContent) -> bool| quads.iter().position(|q| pred(&q.content));
    let icon = pos(&|c| {
        matches!(c, QuadContent::Texture { path: Some(p), .. } if p.contains("Spell_Fire_FlameBolt"))
    })
    .expect("the icon texture quad");
    let ring = pos(
        &|c| matches!(c, QuadContent::Texture { path: Some(p), .. } if p.contains("UI-Quickslot2")),
    )
    .expect("the NormalTexture ring quad");
    let sweep = quads
        .iter()
        .position(|q| {
            matches!(q.content, QuadContent::Cooldown(_))
                && s.quad_owner_name(q.target).as_deref() == Some("ActionButton1Cooldown")
        })
        .expect("the sweep's quad");
    assert!(
        icon < sweep,
        "the sweep (index {sweep}) must paint over the icon (index {icon})"
    );
    assert!(
        ring < sweep,
        "the sweep (index {sweep}) must paint over the button ring (index {ring})"
    );
}

/// A cooldown-count addon's wrap of `CooldownFrame_SetTimer` leaves the sweep as it was.
#[test]
fn a_cooldown_count_addons_hook_leaves_the_sweep_running() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ActionState;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);
    // Addons load after FrameXML, so the hook captures the stock global.
    s.run(COOLDOWN_COUNT_HOOK)
        .expect("the addon's hook installs");

    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
            kind: 0x00,
            action: 133,
            count: 0,
            consumable: false,
            stackable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.tick(10.0);
    s.set_action_state(
        1,
        Some(ActionState {
            usable: true,
            cooldown: Some((6_000, 10_000, true)),
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
    s.tick(0.0);

    assert!(
        s.eval::<i64>("return seen").unwrap() > 0,
        "the bar must reach the addon's replacement, not a captured original"
    );
    assert!(
        s.eval::<bool>("return ActionButton1Cooldown.textFrame ~= nil")
            .unwrap(),
        "the addon hangs its countdown off the cooldown widget — a field the widget must accept"
    );
    assert_eq!(
        super::test_ui::cooldown_sweep(&mut s, "ActionButton1Cooldown").map(|p| p.swipe),
        Some(super::test_ui::swept(0.4)),
        "the wrap calls through, so the sweep paints at 40 %"
    );
    s.set_action_state(
        1,
        Some(ActionState {
            usable: true,
            ..Default::default()
        }),
    );
    s.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
    s.tick(0.0);
    assert_eq!(
        super::test_ui::cooldown_sweep(&mut s, "ActionButton1Cooldown"),
        None,
        "an elapsed cooldown hides the frame through the wrap exactly as it does without it"
    );
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );
}

/// `!OmniCC` 6.8.30's hook in shape: it wraps `CooldownFrame_SetTimer` and hangs its countdown
/// off the button at `button + 2`, one level over the cooldown, kept in `cd.textFrame`.
const COOLDOWN_COUNT_HOOK: &str = r#"
    local original = CooldownFrame_SetTimer
    seen = 0
    CooldownFrame_SetTimer = function(cd, start, duration, enable)
        seen = seen + 1
        original(cd, start, duration, enable)
        if start > 0 and duration > 3 and enable > 0 then
            local count = cd.textFrame
            if not count then
                local icon = getglobal(cd:GetParent():GetName() .. "Icon")
                if icon then
                    count = CreateFrame("Frame", nil, cd:GetParent())
                    count:SetAllPoints(cd:GetParent())
                    count:SetFrameLevel(count:GetFrameLevel() + 1)
                    count.text = count:CreateFontString(nil, "OVERLAY")
                    count.text:SetFontObject(GameFontNormal)
                    count.text:SetPoint("CENTER", count, "CENTER", 0, 1)
                    count.icon = icon
                    count:SetScript("OnUpdate", function() end)
                    cd.textFrame = count
                end
            end
            if count then
                count.start = start
                count.duration = duration
                count:Show()
            end
        elseif cd.textFrame then
            cd.textFrame:Hide()
        end
    end
"#;

/// `BonusActionButtonTemplate`'s OnLoad raises the button by 2, then its cooldown by 2
/// (BonusActionBarFrame.xml:13-15). A script level change carries the frame's same-strata
/// children by the same step (the `SetFrameLevel` method `0x49e9d0` passes 1 to the frame's level
/// setter `0x4910a0`), so the cooldown ends three levels over its button.
#[test]
fn the_bonus_buttons_cooldown_sits_three_levels_over_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);
    let level = |s: &UiScript, frame: &str| {
        s.eval::<i64>(&format!("return {frame}:GetFrameLevel()"))
            .unwrap()
    };
    for i in 1..=12 {
        assert_eq!(
            level(&s, &format!("BonusActionButton{i}Cooldown")),
            level(&s, &format!("BonusActionButton{i}")) + 3,
            "BonusActionButton{i}: the carried +2 and the cooldown's own +2 over its +1"
        );
    }
}

/// `ActionButton_OnLoad` registers every button's release (`RegisterForClicks("AnyUp")`,
/// ActionButton.lua:90), and the secure click's `type` attribute is `"action"` whatever the
/// button (`SecureActionButton_OnClick`, SecureTemplates.lua:478-530): a right or middle click
/// uses the action too, a modifier included.
#[test]
fn every_mouse_button_uses_the_action() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);

    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\INV_Misc_Food_11".into()),
            kind: 0x80, // an ITEM action: food
            action: 4540,
            count: 5,
            consumable: false,
            stackable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();

    for button in ["RightButton", "MiddleButton"] {
        s.mouse_button(26.0, 22.0, button, true);
        s.mouse_button(26.0, 22.0, button, false);
        assert_eq!(
            action_ids(&mut s),
            vec![1],
            "{button} queues the same UseAction a left-click does"
        );
    }
    // A shift+right-click is a click too: no pickup on the click path.
    s.set_modifiers(true, false, false);
    s.mouse_button(26.0, 22.0, "RightButton", true);
    s.mouse_button(26.0, 22.0, "RightButton", false);
    s.set_modifiers(false, false, false);
    assert_eq!(action_ids(&mut s), vec![1]);
    assert!(s.cursor_payload().is_none(), "nothing picked up");
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );
}

/// The click path never picks up: a shift-click uses the action like any click, and with a payload
/// held the same `UseAction` places it (`0x5abbc0`, which reads the cursor and no modifier).
/// Picking up is the drag's (ActionBarFrame.xml:18-24).
#[test]
fn a_shift_click_uses_and_a_held_payload_places() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);

    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_A".into()),
            kind: 0x00,
            action: 111,
            count: 0,
            consumable: false,
            stackable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();

    s.set_modifiers(true, false, false); // IsShiftKeyDown() true
    s.mouse_button(26.0, 22.0, "LeftButton", true);
    s.mouse_button(26.0, 22.0, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert_eq!(action_ids(&mut s), vec![1], "shift-click uses");
    assert!(s.cursor_payload().is_none(), "and picks nothing up");

    // Picked up by the drag script (the bars ship locked, so with the PICKUPACTION modifier).
    s.set_modifiers(true, false, false);
    s.run("ActionButton1:GetScript(\"OnDragStart\")(ActionButton1)")
        .unwrap();
    s.set_modifiers(false, false, false);
    assert!(s.cursor_payload().is_some(), "action 1 is on the cursor");
    assert_eq!(
        s.take_action_sets(),
        vec![(1, 0)],
        "the clear-the-slot send"
    );

    // A plain click while holding places through `UseAction`.
    s.mouse_button(26.0, 22.0, "LeftButton", true);
    s.mouse_button(26.0, 22.0, "LeftButton", false);
    assert!(s.cursor_payload().is_none(), "empty destination clears");
    assert!(s.eval::<bool>("return HasAction(1)").unwrap());
    assert_eq!(s.take_action_sets(), vec![(1, 111)]);
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );
}

/// The bars ship locked: the `lockActionBars` CVar defaults to `"1"`, which the Action Bars panel
/// copies into `LOCK_ACTIONBAR` at world entry (InterfaceOptionsPanels.lua:1144). The lock
/// guards the drag's start alone, which the `PICKUPACTION` modifier overrides
/// (ActionBarFrame.xml:18-24); the drop takes no lock (l.25-29).
#[test]
fn the_action_bar_lock_stops_a_plain_drag_and_yields_to_the_pickup_modifier() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);

    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_A".into()),
            kind: 0x00,
            action: 111,
            count: 0,
            consumable: false,
            stackable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();
    assert_eq!(
        s.eval::<String>("return LOCK_ACTIONBAR").unwrap(),
        "1",
        "the bar ships locked, the reference's own default"
    );

    let drag_start = |s: &mut UiScript| {
        s.run("ActionButton1:GetScript(\"OnDragStart\")(ActionButton1)")
            .unwrap()
    };
    drag_start(&mut s);
    assert!(
        s.cursor_payload().is_none(),
        "a locked bar does not give the action up to a plain drag"
    );
    assert!(
        s.eval::<bool>("return HasAction(1)").unwrap(),
        "slot intact"
    );
    assert!(s.take_action_sets().is_empty(), "and nothing is sent");

    s.set_modifiers(true, false, false);
    drag_start(&mut s);
    s.set_modifiers(false, false, false);
    assert!(
        s.cursor_payload().is_some(),
        "the PICKUPACTION modifier (shift) overrides the lock"
    );

    s.run("ActionButton1:GetScript(\"OnReceiveDrag\")(ActionButton1)")
        .unwrap();
    assert!(
        s.cursor_payload().is_none(),
        "a locked slot still takes the drop"
    );
    assert_eq!(
        s.take_action_sets(),
        vec![(1, 0), (1, 111)],
        "the pickup's clear and the drop's set — nothing from the refused drag"
    );
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );
}

/// A drag onto an occupied button puts the displaced action on the cursor, in two sends; the bars
/// ship locked, so the drag starts with the `PICKUPACTION` modifier held.
#[test]
fn drag_drop_onto_another_button_hops_the_displaced_action() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);

    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_A".into()),
            kind: 0x00,
            action: 111,
            count: 0,
            consumable: false,
            stackable: false,
        }),
    );
    s.set_action(
        2,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_B".into()),
            kind: 0x00,
            action: 222,
            count: 0,
            consumable: false,
            stackable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();

    s.set_modifiers(true, false, false);
    s.mouse_button(26.0, 22.0, "LeftButton", true);
    s.mouse_move(40.0, 22.0); // past the 4px drag-start threshold
    s.set_modifiers(false, false, false);
    let consumed = s.mouse_button(68.0, 22.0, "LeftButton", false);
    assert!(consumed, "OnReceiveDrag consumed the release");

    assert!(
        !s.eval::<bool>("return HasAction(1)").unwrap(),
        "slot 1 emptied"
    );
    assert!(s.eval::<bool>("return HasAction(2)").unwrap());
    assert_eq!(
        s.eval::<String>("return GetActionTexture(2)").unwrap(),
        "Interface\\Icons\\Spell_A",
        "slot 2 now shows the placed action"
    );
    assert!(
        matches!(s.cursor_payload(), Some(benilla_ui::script::CursorPayload::Action(c)) if c.src_slot == 2),
        "the displaced action hopped on, sourced from slot 2: {:?}",
        s.cursor_payload()
    );

    // Two independent sends across the one gesture: the pickup's clear, then the place's write.
    assert_eq!(s.take_action_sets(), vec![(1, 0), (2, 111)]);
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );
}

/// The count shows only for `IsConsumableAction` (ActionButton.lua:285-292), false for a mount's
/// item: zero charges, `InventoryType` 0. The flag rides the slot, not the state map:
/// `ACTIONBAR_SLOT_CHANGED` repaints before the state feed writes.
#[test]
fn count_fontstring_follows_is_consumable_action_not_the_bag_count() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);

    // Multi-digit counts: a single digit could match a button's one-character HotKey label.
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_A".into()),
            kind: 0x00, // SPELL
            action: 111,
            count: 42, // the app never sets this for a spell; the XML must ignore it
            consumable: false,
            stackable: false,
        }),
    );
    s.set_action(
        2,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
            kind: 0x80, // ITEM: a stack of food
            action: 117,
            count: 15,
            consumable: true,
            stackable: false,
        }),
    );
    // A mount: an ITEM held eleven times that is not consumable.
    s.set_action(
        3,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Ability_Mount_Undeadhorse".into()),
            kind: 0x80,
            action: 13332,
            count: 11,
            consumable: false,
            stackable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();

    // Read the fontstrings by name: a one-character count reads like a HotKey label.
    let count_of = |s: &UiScript, n: u32| {
        s.eval::<String>(&format!("return ActionButton{n}Count:GetText() or \"\""))
            .unwrap()
    };
    assert_eq!(
        count_of(&s, 1),
        "",
        "SPELL kind never shows a count, however GetActionCount answers"
    );
    assert_eq!(
        count_of(&s, 2),
        "15",
        "a consumable ITEM shows its bag count"
    );
    assert_eq!(
        count_of(&s, 3),
        "",
        "a NON-consumable ITEM (a mount) shows no stack number, whatever the count says"
    );
    assert!(
        s.extract()
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "15")),
        "the consumable's count reaches the screen"
    );

    // A spent consumable reads "0": no count test guards the `SetText` (ActionButton.lua:288).
    s.set_action(
        2,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
            kind: 0x80,
            action: 117,
            count: 0,
            consumable: true,
            stackable: false,
        }),
    );
    s.fire_event("ACTIONBAR_SLOT_CHANGED", vec![ScriptValue::Int(2)]);
    s.resolve();
    assert_eq!(
        count_of(&s, 2),
        "0",
        "a spent consumable reads 0, it does not go blank"
    );
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );
}

/// `ActionButton_Update` writes `GetActionText` into the `$parentName` line unconditionally
/// (ActionButton.lua:236-238): a macro slot shows its name, a spell slot or an emptied one none.
#[test]
fn macro_name_line_follows_get_action_text_through_the_xml() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::{MacroState, MacroView};

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);
    s.set_macros(MacroState {
        account: vec![MacroView {
            name: "spawn".into(),
            texture: Some("Interface\\Icons\\Ability_Racial_Cannibalize".into()),
            body: ".spawn 16032".into(),
        }],
        character: Vec::new(),
    });
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Ability_Racial_Cannibalize".into()),
            kind: 0x40, // MACRO
            action: 1,  // macro index 1
            count: 0,
            consumable: false,
            stackable: false,
        }),
    );
    s.set_action(
        2,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_A".into()),
            kind: 0x00, // SPELL
            action: 111,
            count: 0,
            consumable: false,
            stackable: false,
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();

    // Read the fontstrings by name: painted text is ambiguous.
    let name_of = |s: &UiScript, n: u32| {
        s.eval::<String>(&format!("return ActionButton{n}Name:GetText() or \"\""))
            .unwrap()
    };
    assert_eq!(
        name_of(&s, 1),
        "spawn",
        "a MACRO slot wears its macro's name"
    );
    assert_eq!(name_of(&s, 2), "", "a SPELL slot has no name line");
    assert!(
        s.extract()
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "spawn")),
        "the name reaches the screen"
    );

    s.set_action(1, None);
    s.fire_event("ACTIONBAR_SLOT_CHANGED", vec![ScriptValue::Int(1)]);
    s.resolve();
    assert_eq!(name_of(&s, 1), "", "an emptied slot loses the name");
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );
}

/// The stock bag bar (`MainMenuBarBagButtons.xml`), which seats on `MainMenuBarArtFrame`: its
/// toggle opens the backpack, the stack paints in its slot, and the slot's clicks queue intents.
#[test]
fn shipped_bag_frame_drives_end_to_end() {
    let _data = benilla_formats::wow_data_or_skip!();
    use super::test_ui::{bag_open, bag_slot_button, centre_of};
    use benilla_ui::script::{ContainerSlot, ContainerState};

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);
    // The bag bar is six CheckButtons, each with its `$parentItemAnim` Model; the four bag slots
    // inherit `ItemButtonTemplate` through `BagSlotButtonTemplate`, which carries no cooldown
    // (MainMenuBarBagButtons.xml:17-20, :77-157).
    for button in [
        "MainMenuBarBackpackButton",
        "CharacterBag0Slot",
        "CharacterBag1Slot",
        "CharacterBag2Slot",
        "CharacterBag3Slot",
        "KeyRingButton",
    ] {
        assert!(
            s.eval::<bool>(&format!(
                "return {button} ~= nil and {button}ItemAnim ~= nil and {button}Cooldown == nil"
            ))
            .unwrap(),
            "{button}: built, with its item anim and no cooldown"
        );
    }

    // The app's feed: a backpack with Tough Jerky ×5 in slot 1.
    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            guid: 0,
            gems: [0; 3],
            quest_item: false,
            start_quest: 0,
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
            count: 5,
            quality: Some(1),
            item_id: 117,
            link: None,
            locked: false,
            equip_slots: Vec::new(),
            cooldown: None,
            readable: false,
            creator: None,
            flags: 0,
            enchants: Vec::new(),
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            bag_family: 0,
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }),
    );
    s.fire_event("BAG_UPDATE", vec![ScriptValue::Int(0)]);
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );

    s.resolve();
    let jerky_visible = |quads: &[benilla_ui::script::ExtractedQuad]| {
        quads.iter().any(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("INV_Misc_Food_16"))
        })
    };
    assert!(!jerky_visible(&s.extract()), "no bag window at load");

    // The toggle seats at the art frame's BOTTOMRIGHT + (-6, 2), 37x37
    // (MainMenuBarBagButtons.xml:54-64), and that corner is the bar's (1024, 0): x[981,1018]
    // y[2,39]. The click stays at literal coordinates because hitting them is under test.
    s.mouse_button(999.0, 20.0, "LeftButton", true);
    s.mouse_button(999.0, 20.0, "LeftButton", false);
    s.resolve();
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );
    assert!(bag_open(&s, 0), "the bar's toggle opened the backpack");
    let quads = s.extract();
    let icon = quads
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("INV_Misc_Food_16"))
        })
        .and_then(|q| q.rect)
        .expect("slot 1 icon visible after toggle");
    // Ask the slot's button by GetID: the buttons are numbered backwards, `…Item1` is the bag's
    // last slot (ContainerFrame.lua:425-428).
    let button = bag_slot_button(&s, 0, 1);
    let (bx, by) = centre_of(&mut s, &button);
    assert!(
        icon.left <= bx && bx <= icon.right && icon.bottom <= by && by <= icon.top,
        "the jerky icon {icon:?} is not painted on slot 1's button ({button} at {bx},{by})"
    );
    assert!(
        quads
            .iter()
            .any(|q| { matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "5") }),
        "stack count shows"
    );

    // Left-click picks the item up (`PickupContainerItem`, ContainerFrame.lua:580).
    s.mouse_button(bx, by, "LeftButton", true);
    s.mouse_button(bx, by, "LeftButton", false);
    assert!(
        s.take_container_uses().is_empty(),
        "left-click is a pickup, not a use"
    );
    assert!(
        s.cursor_item().is_some(),
        "left-click put the item on the cursor"
    );
    assert!(
        s.take_container_moves().is_empty(),
        "a pickup alone queues no move"
    );

    // Right-click while holding: the right arm has no cursor test (ContainerFrame.lua:583-599), and
    // `UseContainerItem` clears the cursor before it reads the slot.
    s.mouse_button(bx, by, "RightButton", true);
    s.mouse_button(bx, by, "RightButton", false);
    assert_eq!(
        s.take_container_uses(),
        vec![(0, 1)],
        "the right arm uses the slot even with a full cursor"
    );
    assert!(
        s.cursor_item().is_none(),
        "…after putting the held item back"
    );

    s.mouse_button(bx, by, "RightButton", true);
    s.mouse_button(bx, by, "RightButton", false);
    assert_eq!(s.take_container_uses(), vec![(0, 1)]);

    s.mouse_button(999.0, 20.0, "LeftButton", true);
    s.mouse_button(999.0, 20.0, "LeftButton", false);
    s.resolve();
    assert!(!bag_open(&s, 0), "toggle closes the window");
    assert!(!jerky_visible(&s.extract()), "…and its slots with it");
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );
}

/// `ActionButton_Update` registers the state events only while the button has an action
/// (ActionButton.lua:175-212), so an empty well's texture-less icon never takes a tint.
#[test]
fn state_events_leave_empty_wells_untinted() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ActionState;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);
    // One occupied slot; 2..12 empty.
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
            kind: 0x00,
            action: 133,
            count: 0,
            consumable: false,
            stackable: false,
        }),
    );
    s.set_action_state(
        1,
        Some(ActionState {
            usable: false,
            not_enough_mana: true,
            ..Default::default()
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.fire_event("ACTIONBAR_UPDATE_USABLE", vec![]);
    s.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
    s.fire_event("ACTIONBAR_UPDATE_STATE", vec![]);
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s.resolve();

    // No well-sized texture-less coloured quad; the occupied icon keeps its out-of-power blue.
    let mut oom_icon = None;
    for q in s.extract() {
        match &q.content {
            QuadContent::Texture {
                path: None,
                color: Some(c),
                ..
            } if q.rect.is_some_and(|r| r.right - r.left <= 40.0) => {
                // Well-sized only: the XP bar's 1024-wide black backdrop is a legitimate solid.
                panic!("an empty well gained a solid color quad: {c:?}")
            }
            QuadContent::Texture {
                path: Some(p),
                color,
                ..
            } if p.contains("Spell_Fire_FlameBolt") => oom_icon = *color,
            _ => {}
        }
    }
    assert_eq!(
        oom_icon,
        Some([0.5, 0.5, 1.0, 1.0]),
        "the occupied button still tints OOM blue"
    );
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );
}

/// A slot going empty draws no plate: the empty arm hides the icon (ActionButton.lua:168), and a
/// texture-less region never draws, whatever its tint (the texture draw gates on `+0xcc`).
#[test]
fn an_occupied_slot_going_empty_leaves_no_white_plate() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ActionState;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);
    s.set_action(
        3,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Spell_Nature_HealingTouch".into()),
            kind: 0x00,
            action: 5185,
            count: 0,
            consumable: false,
            stackable: false,
        }),
    );
    s.set_action_state(
        3,
        Some(ActionState {
            usable: true,
            ..Default::default()
        }),
    );
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    // The usable pass tints the occupied icon 1/1/1, a tint that outlives the emptying.
    s.fire_event("ACTIONBAR_UPDATE_USABLE", vec![]);
    s.resolve();
    assert!(
        s.extract().iter().any(|q| matches!(&q.content,
            QuadContent::Texture { path: Some(p), .. } if p.contains("Spell_Nature_HealingTouch"))),
        "the occupied slot draws its icon"
    );

    // The character switch: the new character's table has nothing in slot 3.
    s.set_action(3, None);
    s.set_action_state(3, None);
    s.fire_event("ACTIONBAR_SLOT_CHANGED", vec![ScriptValue::Int(3)]);
    s.resolve();
    for q in s.extract() {
        match &q.content {
            QuadContent::Texture { path: Some(p), .. }
                if p.contains("Spell_Nature_HealingTouch") =>
            {
                panic!("the emptied slot still draws the old icon")
            }
            QuadContent::Texture {
                path: None,
                color: Some(c),
                ..
            } if q.rect.is_some_and(|r| r.right - r.left <= 40.0) => {
                // Well-sized only: page-wide solids are legitimate.
                panic!("the emptied slot draws its surviving tint as a solid plate: {c:?}")
            }
            _ => {}
        }
    }
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );
}

/// `BonusActionBarFrame` is declared hidden (BonusActionBarFrame.xml:54); addons lay out its
/// buttons before it ever shows (`CT_BarModOptions.lua:154`).
#[test]
fn the_bonus_action_bar_exists_hidden_and_takes_layout_calls() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);

    assert!(
        s.eval::<bool>("return BonusActionBarFrame ~= nil").unwrap(),
        "5 corpus addons index BonusActionBarFrame by name"
    );
    assert!(
        !s.eval::<bool>("return BonusActionBarFrame:IsShown()")
            .unwrap(),
        "it must ship HIDDEN — benilla re-pages the main bar instead of showing this one"
    );

    for i in [1, 12] {
        assert!(
            s.eval::<bool>(&format!("return BonusActionButton{i} ~= nil"))
                .unwrap(),
            "BonusActionButton{i} must exist"
        );
        // CT_BarModOptions.lua:154's exact pair, on a bar that has never been shown.
        s.run(&format!("BonusActionButton{i}:ClearAllPoints()"))
            .unwrap();
        s.run(&format!(
            "BonusActionButton{i}:SetPoint(\"TOP\", \"ActionButton1\", \"BOTTOM\", 0, -4)"
        ))
        .unwrap();
    }

    // The hidden bar leaves the main bar alone. An empty main-bar button hides while
    // `showgrid == 0` (ActionButton.lua:214-215), so slot 1 is occupied first.
    s.set_action(
        1,
        Some(ActionSlot {
            texture: Some("Interface\\Icons\\Ability_SteelMelee".into()),
            kind: 0x00,
            action: 100,
            count: 0,
            consumable: false,
            stackable: false,
        }),
    );
    s.fire_event("ACTIONBAR_SLOT_CHANGED", vec![ScriptValue::Int(1)]);
    assert!(
        s.eval::<bool>("return ActionButton1:IsShown()").unwrap(),
        "the main bar is untouched"
    );
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );
}

/// ActionButton.lua:1-13's constants are globals (`zBar.lua:40` loops to `NUM_ACTIONBAR_BUTTONS`).
#[test]
fn the_reference_action_bar_constants_are_defined() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_action_bar(&mut s);

    for (name, want) in [
        ("NUM_ACTIONBAR_PAGES", 6),
        ("NUM_ACTIONBAR_BUTTONS", 12),
        ("BOTTOMLEFT_ACTIONBAR_PAGE", 6),
        ("BOTTOMRIGHT_ACTIONBAR_PAGE", 5),
        ("LEFT_ACTIONBAR_PAGE", 4),
        ("RIGHT_ACTIONBAR_PAGE", 3),
    ] {
        assert_eq!(
            s.eval::<i64>(&format!("return {name}")).unwrap(),
            want,
            "{name} must be the reference's value"
        );
    }

    // zBar's expression.
    assert_eq!(
        s.eval::<i64>("local to = nil or nil or NUM_ACTIONBAR_BUTTONS local n = 0 for i = 1, to do n = n + 1 end return n")
            .unwrap(),
        12,
        "zBar.lua:40's numeric for must have a limit"
    );

    // `CURRENT_ACTIONBAR_PAGE` is declared (ActionButton.lua:1) and never written: the page is the
    // engine's, `GetActionBarPage`, which the paging functions read and move.
    assert_eq!(s.eval::<i64>("return CURRENT_ACTIONBAR_PAGE").unwrap(), 1);
    assert_eq!(s.eval::<i64>("return GetActionBarPage()").unwrap(), 1);
    s.run("ActionBar_PageUp()").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetActionBarPage()").unwrap(),
        2,
        "the engine's page moves"
    );
    assert_eq!(
        s.eval::<i64>("return CURRENT_ACTIONBAR_PAGE").unwrap(),
        1,
        "and the stock global stays its declared 1"
    );
    s.run("ActionBar_PageDown()").unwrap();
}

/// `ActionButtonTemplate` is regions only (ActionButtonTemplate.xml:3); `ActionBarButtonTemplate`
/// adds the handlers (ActionBarFrame.xml:4), and `zBar.xml:7` inherits it.
#[test]
fn both_reference_action_button_templates_are_inheritable() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    load_action_bar(&mut s);

    // zBar's exact shape: inherit the bar template, supply your own OnLoad.
    let doc = benilla_ui::framexml::parse(
        r#"<Ui>
            <CheckButton name="ZLikeButton" inherits="ActionBarButtonTemplate" parent="UIParent" id="1">
                <Anchors><Anchor point="CENTER"/></Anchors>
            </CheckButton>
            <CheckButton name="BareLikeButton" inherits="ActionButtonTemplate" id="1">
                <Anchors><Anchor point="TOPLEFT"/></Anchors>
            </CheckButton>
        </Ui>"#,
    )
    .unwrap();
    let report = benilla_ui::loader::load(&s, &doc, &|_| None);
    assert!(
        report.errors.is_empty(),
        "loader errors: {:?}",
        report.errors
    );
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| w.contains("unknown template")),
        "both names must resolve: {:?}",
        report.warnings
    );

    // The derived name zBar reads, on a button built from each half.
    for owner in ["ZLikeButton", "BareLikeButton"] {
        assert!(
            s.eval::<bool>(&format!("return {owner}NormalTexture ~= nil"))
                .unwrap(),
            "{owner}NormalTexture — zBar.lua:88's read"
        );
        assert!(
            s.eval::<bool>(&format!(
                "return {owner}Icon ~= nil and {owner}Cooldown ~= nil"
            ))
            .unwrap(),
            "{owner} must carry the template's regions"
        );
    }

    // The base half carries no handlers: an addon inheriting it wires its own.
    assert!(
        !s.eval::<bool>("return BareLikeButton:GetScript(\"OnClick\") ~= nil")
            .unwrap(),
        "ActionButtonTemplate is regions only; handlers belong to the bar half"
    );
    assert!(
        s.eval::<bool>("return ZLikeButton:GetScript(\"OnClick\") ~= nil")
            .unwrap(),
        "ActionBarButtonTemplate carries the handler set"
    );

    // ...and the stock bar's own buttons carry both.
    assert!(
        s.eval::<bool>("return ActionButton1NormalTexture ~= nil and ActionButton1:GetScript(\"OnClick\") ~= nil")
            .unwrap(),
        "BenillaActionButtonTemplate's 48 inherits= sites must be untouched by the split"
    );
}

/// Main-bar paging (ActionButton.lua:45-78): page-up past the last viewable page wraps to 1,
/// page-down below 1 rescans for the last viewable one.
#[test]
fn the_main_bar_pages_and_a_bonus_page_still_outranks_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);

    assert_eq!(s.eval::<i64>("return GetActionBarPage()").unwrap(), 1);
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        1,
        "page 1 button 1 is action 1"
    );
    assert!(
        s.eval::<bool>(
            "for i = 1, NUM_ACTIONBAR_PAGES do \
               if not VIEWABLE_ACTION_BAR_PAGES[i] then return false end \
             end return true"
        )
        .unwrap(),
        "all six pages are viewable at rest — every extra bar ships off"
    );
    // The page numeral: `MainMenuBar` writes it at load and on `ACTIONBAR_PAGE_CHANGED`
    // (MainMenuBar.lua:185, 192).
    let page_text = |s: &UiScript| {
        s.eval::<String>("return MainMenuBarPageNumber:GetText()")
            .unwrap()
    };
    assert_eq!(page_text(&s), "1", "the load seed is the current page");

    // Raising both bottom bars takes pages 5 and 6 out of the cycle (MultiActionBars.lua:53, 61).
    s.run("SHOW_MULTI_ACTIONBAR_1 = 1 SHOW_MULTI_ACTIONBAR_2 = 1 MultiActionBar_Update()")
        .unwrap();

    // Page up walks to 2, so button 1 shows action 13.
    s.run("ActionBar_PageUp()").unwrap();
    assert_eq!(s.eval::<i64>("return GetActionBarPage()").unwrap(), 2);
    assert_eq!(page_text(&s), "2", "the numeral follows the page up");
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        13
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton12)")
            .unwrap(),
        24
    );

    // Down again; below page 1 it rescans for the last viewable page.
    s.run("ActionBar_PageDown()").unwrap();
    assert_eq!(s.eval::<i64>("return GetActionBarPage()").unwrap(), 1);
    s.run("ActionBar_PageDown()").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetActionBarPage()").unwrap(),
        4,
        "page-down off the bottom rescans for the last VIEWABLE page — 4, not 6, because the \
         two raised bottom multibars own pages 5 and 6"
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        37
    );
    assert_eq!(
        page_text(&s),
        "4",
        "the numeral follows a wrap, not just a step"
    );
    // The bottom bars' pages are unreachable from the main bar.
    assert!(s
        .eval::<bool>(
            "return VIEWABLE_ACTION_BAR_PAGES[5] == nil and VIEWABLE_ACTION_BAR_PAGES[6] == nil"
        )
        .unwrap());
    s.run("ChangeActionBarPage(4) ActionBar_PageUp()").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetActionBarPage()").unwrap(),
        1,
        "walking up from the last viewable page skips 5 and 6 and wraps to the LITERAL 1 — the \
         reference's own asymmetry with page-down, which rescans instead"
    );

    // The bonus branch needs `isBonus` and page 1 (ActionButton.lua:137-142); a main-bar button has
    // no `isBonus`, so slot 1 on page 3 is action 25 whatever the offset.
    s.run("ChangeActionBarPage(3)").unwrap();
    s.set_bonus_bar_offset(1);
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        25,
        "a main-bar button follows its page, never the bonus offset (ActionButton.lua:131-152)"
    );
    s.run("ChangeActionBarPage(1)").unwrap();
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(BonusActionButton1)")
            .unwrap(),
        73,
        "the bonus frame's own button, on page 1 with offset 1, is 72 + 1"
    );
    s.run("ChangeActionBarPage(3)").unwrap();
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(BonusActionButton1)")
            .unwrap(),
        25,
        "and off page 1 even the bonus button follows the page"
    );
    s.set_bonus_bar_offset(0);
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        25,
        "and the page comes back when the form drops"
    );
}

/// The form swap (BonusActionBarFrame.lua:1-98): the bonus frame slides up and sounds on landing,
/// stays while the form holds, and slides down silently with the old page.
#[test]
fn bonus_bar_slides_up_with_sound_and_down_without() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.tick(10.0); // a nonzero clock epoch
    let _ = s.take_sounds();
    assert!(
        !s.eval::<bool>("return BonusActionBarFrame:IsShown()")
            .unwrap(),
        "no form, no overlay"
    );

    // ── Enter a form: offset 0 to 1, fired on the app's offset edge ──
    s.set_bonus_bar_offset(1);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );
    assert!(s
        .eval::<bool>("return BonusActionBarFrame:IsShown()")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return BonusActionBarFrame.mode").unwrap(),
        "show"
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        1,
        "the main bar holds the OLD page under the rising overlay"
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(BonusActionButton1)")
            .unwrap(),
        73,
        "the overlay paints the bonus page from the first slide frame"
    );
    assert!(s.take_sounds().is_empty(), "no sound until the bar lands");

    // Keys route to the overlay at once: `ActionButtonDown/Up` fork on
    // `BonusActionBarFrame:IsShown()` (ActionButton.lua:16, 31).
    s.run("ActionButtonDown(1) ActionButtonUp(1)").unwrap();
    assert_eq!(
        action_ids(&mut s),
        vec![73],
        "a key pressed mid-slide already drives the bonus page"
    );

    // Self-cast is the SELFCAST modifier on the key's secure click, read through the button's
    // `checkselfcast` (ActionButton.lua:83, SecureTemplates.lua:207): the same on either branch.
    let uses = |s: &mut UiScript| {
        s.take_action_uses()
            .into_iter()
            .map(|u| {
                (
                    u.action,
                    matches!(&u.at, benilla_ui::script::CastAt::Unit(t) if t == "player"),
                )
            })
            .collect::<Vec<_>>()
    };
    s.set_modifiers(false, false, true); // ALT, the SELFCAST default
    s.run("ActionButtonDown(1) ActionButtonUp(1)").unwrap();
    assert_eq!(uses(&mut s), vec![(73, true)], "the bonus page, self-cast");
    s.run("BonusActionBarFrame:Hide() ActionButtonDown(1) ActionButtonUp(1)")
        .unwrap();
    assert_eq!(uses(&mut s), vec![(1, true)], "the main bar, self-cast");
    s.set_modifiers(false, false, false);
    s.run("BonusActionBarFrame:Show()").unwrap();

    // `BonusActionBar_OnUpdate` paints, then advances (BonusActionBarFrame.lua:37-52): top 0, then
    // 0.5 * 43, then the landing.
    s.tick(0.075);
    let top = s
        .eval::<f64>("return BonusActionBarFrame:GetTop()")
        .unwrap();
    assert!(top.abs() < 0.01, "first frame top = {top}, want 0");
    assert!(s.take_sounds().is_empty());
    s.tick(0.075);
    let top = s
        .eval::<f64>("return BonusActionBarFrame:GetTop()")
        .unwrap();
    assert!(
        (top - 21.5).abs() < 0.6,
        "half-slide top = {top}, want ~21.5"
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        1
    );
    assert!(s.take_sounds().is_empty());

    // Landing snaps to 43 and plays the sound (BonusActionBarFrame.lua:61-64); the main bar keeps
    // its page, having no `isBonus` (ActionButton.lua:447).
    s.tick(0.01);
    assert_eq!(
        s.take_sounds(),
        vec![benilla_ui::script::SoundRequest::KitName(
            "igBonusBarOpen".into()
        )]
    );
    let top = s
        .eval::<f64>("return BonusActionBarFrame:GetTop()")
        .unwrap();
    assert!((top - 43.0).abs() < 0.01, "landed top = {top}, want 43");
    assert_eq!(
        s.eval::<String>("return BonusActionBarFrame.mode").unwrap(),
        "none"
    );
    assert!(
        s.eval::<bool>("return BonusActionBarFrame:IsShown()")
            .unwrap(),
        "the overlay stays up while the form holds — the ref-visible state"
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        1,
        "the main bar keeps its page under the landed overlay"
    );
    s.tick(0.5);
    assert!(s.take_sounds().is_empty(), "a landed bar never re-sounds");

    // ── A form to form swap: repaint, no slide, no sound ──
    s.set_bonus_bar_offset(3);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    assert_eq!(
        s.eval::<String>("return BonusActionBarFrame.mode").unwrap(),
        "none"
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(BonusActionButton1)")
            .unwrap(),
        97
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        1,
        "the main bar stays on its page through a form swap"
    );
    s.tick(0.2);
    assert!(
        s.take_sounds().is_empty(),
        "form→form never re-slides or re-sounds"
    );

    // ── Drop the form: the overlay descends with the old page, silently ──
    s.set_bonus_bar_offset(0);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    assert_eq!(
        s.eval::<String>("return BonusActionBarFrame.mode").unwrap(),
        "hide"
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(ActionButton1)")
            .unwrap(),
        1,
        "the main bar returns to the page immediately — it is being revealed"
    );
    assert_eq!(
        s.eval::<i64>("return ActionButton_GetPagedID(BonusActionButton1)")
            .unwrap(),
        97,
        "the descending overlay carries the OLD form's page (lastBonusBar)"
    );
    // A key mid-descent drives the old page: `ActionButton_GetPagedID` falls back to
    // `lastBonusBar` (ActionButton.lua:449-451).
    s.run("ActionButtonDown(1) ActionButtonUp(1)").unwrap();
    assert_eq!(action_ids(&mut s), vec![97]);
    // The frame that finds the timer past the slide time hides it (BonusActionBarFrame.lua:53-68),
    // one OnUpdate past the time.
    s.tick(0.2);
    s.tick(0.01);
    assert!(
        !s.eval::<bool>("return BonusActionBarFrame:IsShown()")
            .unwrap(),
        "the descent ends hidden"
    );
    assert!(
        s.take_sounds().is_empty(),
        "the down-slide is silent — the ref plays only on open"
    );
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );
}

/// A form dropped mid-rise: `HideBonusActionBar` keeps the running timer
/// (BonusActionBarFrame.lua:90-92), and the hide arm paints `(1 - timer / 0.15) * 43` (l.48).
#[test]
fn bonus_bar_turnaround_continues_from_position() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);
    s.tick(10.0);
    let _ = s.take_sounds();

    // The form drops with the rise's timer at half.
    s.set_bonus_bar_offset(1);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    s.tick(0.075);
    s.set_bonus_bar_offset(0);
    s.fire_event("UPDATE_BONUS_ACTIONBAR", vec![]);
    assert_eq!(
        s.eval::<String>("return BonusActionBarFrame.mode").unwrap(),
        "hide"
    );
    // The rise's one OnUpdate left the timer at 0.075, so the hide arm paints
    // (1 - 0.075/0.15) * 43 = 21.5, then 12.9 a frame later: paint, then advance.
    s.tick(0.03);
    let top = s
        .eval::<f64>("return BonusActionBarFrame:GetTop()")
        .unwrap();
    assert!(
        (top - 21.5).abs() < 0.6,
        "turnaround descends from 21.5, top = {top}"
    );
    s.tick(0.03);
    let top = s
        .eval::<f64>("return BonusActionBarFrame:GetTop()")
        .unwrap();
    assert!(
        (top - 12.9).abs() < 0.6,
        "next frame top = {top}, want ~12.9"
    );
    s.tick(0.2);
    s.tick(0.01);
    assert!(!s
        .eval::<bool>("return BonusActionBarFrame:IsShown()")
        .unwrap());
    assert!(
        s.take_sounds().is_empty(),
        "an aborted rise never lands, so it never sounds"
    );
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );
}

/// The page arrows are 32x32 squares 20 px apart, overlapping by 12 px; their `<HitRectInsets>`
/// (6 each side, 7 top and bottom, ActionBarFrame.xml:170-172) shrink each hit rect to 20x18,
/// which separates them.
#[test]
fn the_page_arrows_do_not_steal_each_other_s_clicks() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_action_bar(&mut s);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.resolve();

    let centre = |s: &UiScript, name: &str| {
        s.eval::<(f64, f64)>(&format!(
            "return ({name}:GetLeft() + {name}:GetRight()) / 2, \
                    ({name}:GetBottom() + {name}:GetTop()) / 2"
        ))
        .unwrap()
    };
    for name in ["ActionBarUpButton", "ActionBarDownButton"] {
        let (x, y) = centre(&s, name);
        assert_eq!(
            s.hit_test_name(x as f32, y as f32).as_deref(),
            Some(name),
            "{name} must eat the click at its own centre"
        );
    }

    // 8 px above the up button's bottom edge is inside the down button's raw square but only the
    // up button's hit rect.
    let (x, up_bottom) = s
        .eval::<(f64, f64)>(
            "return (ActionBarUpButton:GetLeft() + ActionBarUpButton:GetRight()) / 2, \
                    ActionBarUpButton:GetBottom()",
        )
        .unwrap();
    assert_eq!(
        s.hit_test_name(x as f32, up_bottom as f32 + 8.0).as_deref(),
        Some("ActionBarUpButton"),
        "the lower third of the visible UP arrow must page up, not down"
    );
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "script errors: {:?}",
        super::test_ui::errors_but_unported(&s)
    );
}
