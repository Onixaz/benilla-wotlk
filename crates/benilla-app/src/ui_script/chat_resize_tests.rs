//! The chat window's move, resize and lock, driven from the mouse through the stock frames: each
//! window's one `$parentResizeButton`, shown while it is unlocked, and its tab's drag.

use benilla_ui::script::UiScript;

fn chat_ui() -> UiScript {
    super::test_ui::chat_ui()
}

fn centre(s: &mut UiScript, frame: &str) -> (f32, f32) {
    s.resolve();
    let (x, y): (f64, f64) = s
        .eval(&format!("return {frame}:GetCenter()"))
        .unwrap_or_else(|e| panic!("{frame}:GetCenter(): {e}"));
    (x as f32, y as f32)
}

fn width(s: &mut UiScript) -> f32 {
    s.resolve();
    s.eval::<f64>("return ChatFrame1:GetWidth()").unwrap() as f32
}

fn height(s: &mut UiScript) -> f32 {
    s.resolve();
    s.eval::<f64>("return ChatFrame1:GetHeight()").unwrap() as f32
}

fn left(s: &mut UiScript) -> f32 {
    s.resolve();
    s.eval::<f64>("return ChatFrame1:GetLeft()").unwrap() as f32
}

fn top(s: &mut UiScript) -> f32 {
    s.resolve();
    s.eval::<f64>("return ChatFrame1:GetTop()").unwrap() as f32
}

fn right_click(s: &mut UiScript, frame: &str) {
    let (x, y) = centre(s, frame);
    s.mouse_button(x, y, "RightButton", true);
    s.mouse_button(x, y, "RightButton", false);
    s.resolve();
}

fn left_click(s: &mut UiScript, frame: &str) {
    let (x, y) = centre(s, frame);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.resolve();
}

/// Let frames run, as the client does between input events.
fn frames(s: &mut UiScript, n: usize, dt: f32) {
    for _ in 0..n {
        s.tick(dt);
        s.resolve();
    }
}

/// The tabs fade in only under a stationary hover (`FCF_OnUpdate`), and a window moves by its tab,
/// so every drag starts here.
fn reveal(s: &mut UiScript) {
    let (cx, cy) = centre(s, "ChatFrame1");
    s.mouse_move(cx, cy);
    frames(s, 45, 0.016);
}

/// A tab drag: press, cross the 4px threshold (`OnDragStart` → `StartMoving`), move, release. The
/// release lands on the next frame, when the tab's `FCFTab_OnUpdate` sees the button up.
fn drag_tab(s: &mut UiScript, dx: f32, dy: f32) {
    reveal(s);
    let (tx, ty) = centre(s, "ChatFrame1Tab");
    s.mouse_button(tx, ty, "LeftButton", true);
    s.mouse_move(tx + 5.0, ty);
    s.mouse_move(tx + dx, ty + dy);
    s.mouse_button(tx + dx, ty + dy, "LeftButton", false);
    frames(s, 1, 0.016);
}

/// Press `frame`'s resize button: its `OnMouseDown` starts sizing from the bottom-right.
fn grab_resize(s: &mut UiScript, frame: &str) -> (f32, f32) {
    let (x, y) = centre(s, &format!("{frame}ResizeButton"));
    s.mouse_button(x, y, "LeftButton", true);
    s.resolve();
    (x, y)
}

/// The open menu's row reading the global string `text`, as its button's name.
fn menu_row(s: &UiScript, text: &str) -> Option<String> {
    s.eval(&format!(
        "for i = 1, DropDownList1.numButtons do \
             local b = _G['DropDownList1Button'..i] \
             if b:GetText() == {text} then return b:GetName() end \
         end"
    ))
    .unwrap()
}

fn resize_shown(s: &UiScript, i: u32) -> bool {
    s.eval::<bool>(&format!("return ChatFrame{i}ResizeButton:IsShown() == 1"))
        .unwrap()
}

/// The stock chat cache carries `LOCKED 1` for both dock windows, and a locked window shows no
/// resize button (`FCF_SetLocked`).
#[test]
fn both_dock_windows_ship_locked() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = chat_ui();
    for i in 1..=2 {
        assert_eq!(
            s.eval::<i64>(&format!(
                "local _,_,_,_,_,_,_,locked = GetChatWindowInfo({i}) return locked"
            ))
            .unwrap(),
            1,
            "window {i} ships locked"
        );
        assert!(
            s.eval::<bool>(&format!("return ChatFrame{i}.isLocked ~= nil"))
                .unwrap(),
            "…and the frame was seated from it"
        );
        assert!(!resize_shown(&s, i), "window {i} hides its resize button");
    }
}

/// The tab menu's lock row toggles the window, flips its label and shows or hides the resize
/// button (`FCF_ToggleLock` → `FCF_SetLocked`).
#[test]
fn the_tab_menus_lock_row_toggles_the_window_and_flips_its_label() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    reveal(&mut s);
    right_click(&mut s, "ChatFrame1Tab");
    let row = menu_row(&s, "UNLOCK_WINDOW").expect("the unlock row");
    assert_eq!(
        s.eval::<i64>(&format!("return {row}.notCheckable"))
            .unwrap(),
        1,
        "a verb, not a tick"
    );
    left_click(&mut s, &row);
    assert!(
        s.eval::<Option<i64>>("return ChatFrame1.isLocked")
            .unwrap()
            .is_none(),
        "the row unlocked the window"
    );
    assert!(
        s.eval::<Option<i64>>("local _,_,_,_,_,_,_,l = GetChatWindowInfo(1) return l")
            .unwrap()
            .is_none(),
        "…and wrote it through SetChatWindowLocked, so it can be persisted"
    );
    assert!(resize_shown(&s, 1), "the resize button is offered");

    // Time passes, or a second right-click on the tab would be its double-click.
    frames(&mut s, 30, 0.016);
    right_click(&mut s, "ChatFrame1Tab");
    let row = menu_row(&s, "LOCK_WINDOW").expect("the lock row");
    left_click(&mut s, &row);
    assert_eq!(
        s.eval::<i64>("return ChatFrame1.isLocked").unwrap(),
        1,
        "and back"
    );
    assert!(!resize_shown(&s, 1), "and the button is gone again");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Authored 430x120; `<ResizeBounds>` 296x120 to 608x400 (`FloatingChatFrame.xml`). The button
/// sizes from the bottom-right, so the top-left stays put.
#[test]
fn the_resize_button_sizes_from_the_bottom_right_and_clamps_at_the_bounds() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    assert_eq!(width(&mut s), 430.0, "the authored width");
    s.run("FCF_SetLocked(ChatFrame1, nil)").unwrap();
    let (x, y) = centre(&mut s, "ChatFrame1ResizeButton");
    assert_eq!(
        s.hit_test_name(x, y).as_deref(),
        Some("ChatFrame1ResizeButton"),
        "the button is the topmost hit"
    );

    let (left0, top0) = (left(&mut s), top(&mut s));
    grab_resize(&mut s, "ChatFrame1");
    s.mouse_move(x + 50.0, y - 30.0);
    assert_eq!(width(&mut s), 480.0, "the right edge followed the cursor");
    assert_eq!(height(&mut s), 150.0, "and the bottom edge with it");
    assert_eq!(
        (left(&mut s), top(&mut s)),
        (left0, top0),
        "the top-left stayed put"
    );

    s.mouse_move(x + 900.0, y - 900.0);
    assert_eq!((width(&mut s), height(&mut s)), (608.0, 400.0), "maxResize");
    s.mouse_move(x - 900.0, y + 900.0);
    assert_eq!((width(&mut s), height(&mut s)), (296.0, 120.0), "minResize");

    s.mouse_button(x - 900.0, y + 900.0, "LeftButton", false);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `OnMouseUp` goes to the button that took the press, not to the frame under the cursor, and its
/// `StopMovingOrSizing` ends the resize.
#[test]
fn the_release_ends_the_resize_from_anywhere_on_screen() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    s.run("FCF_SetLocked(ChatFrame1, nil)").unwrap();
    let (x, y) = grab_resize(&mut s, "ChatFrame1");
    assert_eq!(
        s.eval::<String>("return ChatFrame1ResizeButton:GetButtonState()")
            .unwrap(),
        "PUSHED"
    );

    s.mouse_move(x + 900.0, y);
    assert_eq!(width(&mut s), 608.0, "held against the maximum");
    s.mouse_button(x + 900.0, y, "LeftButton", false);
    assert_eq!(
        s.eval::<String>("return ChatFrame1ResizeButton:GetButtonState()")
            .unwrap(),
        "NORMAL",
        "the button's OnMouseUp ran"
    );

    s.mouse_move(x - 200.0, y);
    assert_eq!(
        width(&mut s),
        608.0,
        "the resize is over; walking back does not shrink the window"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A locked window refuses the tab drag (`OnDragStart` returns on `isLocked`); an unlocked one
/// follows the cursor from where the drag started, until the release.
#[test]
fn the_tab_drag_moves_an_unlocked_window_and_a_locked_one_stays_put() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    let start = left(&mut s);
    drag_tab(&mut s, 80.0, 0.0);
    assert_eq!(left(&mut s), start, "a locked window does not drag");

    s.run("FCF_SetLocked(ChatFrame1, nil)").unwrap();
    reveal(&mut s);
    let (tx, ty) = centre(&mut s, "ChatFrame1Tab");
    s.mouse_button(tx, ty, "LeftButton", true);
    s.mouse_move(tx + 5.0, ty);
    s.mouse_move(tx + 80.0, ty);
    assert_eq!(
        left(&mut s),
        start + 75.0,
        "the window followed the cursor from where the drag started"
    );
    s.mouse_button(tx + 80.0, ty, "LeftButton", false);
    frames(&mut s, 1, 0.016);
    s.mouse_move(tx + 400.0, ty);
    assert_eq!(
        left(&mut s),
        start + 75.0,
        "FCFTab_OnUpdate ended it on the release"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A window a clamp holds off its anchors moves from where it is drawn: the drag start flattens it
/// to its current rect (`FlattenFrame` `0x48a5f0`), so it does not jump back by the clamped amount.
#[test]
fn a_clamped_window_drags_from_where_it_is_drawn() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    // `SetClampRectInsets(-35, 35, 26, -50)` holds the window's left off its anchor's offset.
    let (anchored, drawn): (f64, f64) = s
        .eval("local _, _, _, x = ChatFrame1:GetPoint() return x, ChatFrame1:GetLeft()")
        .unwrap();
    assert!(
        anchored < drawn,
        "the clamp holds it: {anchored} vs {drawn}"
    );
    s.run("FCF_SetLocked(ChatFrame1, nil)").unwrap();
    drag_tab(&mut s, 80.0, 0.0);
    assert_eq!(f64::from(left(&mut s)), drawn + 75.0);
}

/// `UIParent_ManageFramePositions` skips a user-placed frame (`UIParent.lua`).
#[test]
fn a_moved_window_is_user_placed_and_the_managed_pass_skips_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    s.run("UIParent_ManageFramePositions()").unwrap();
    let seated = left(&mut s);
    s.run("FCF_SetLocked(ChatFrame1, nil)").unwrap();
    drag_tab(&mut s, 120.0, 0.0);
    let moved = left(&mut s);
    assert_eq!(
        moved,
        seated + 115.0,
        "120 less the 5px spent on the threshold"
    );
    assert!(
        s.eval::<bool>("return ChatFrame1:IsUserPlaced()").unwrap(),
        "the drag set the userPlaced bit"
    );
    s.run("UIParent_ManageFramePositions()").unwrap();
    assert_eq!(
        left(&mut s),
        moved,
        "the managed pass does not re-seat a frame the player placed"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The resize's and the drag's ends save through `FCF_SavePositionAndDimensions`, and
/// `FCF_RestorePositionAndDimensions` puts the window back from the saved values.
#[test]
fn the_geometry_round_trips_through_the_saved_position() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    s.run("FCF_SetLocked(ChatFrame1, nil)").unwrap();

    let (gx, gy) = grab_resize(&mut s, "ChatFrame1");
    s.mouse_move(gx + 70.0, gy - 40.0);
    s.mouse_button(gx + 70.0, gy - 40.0, "LeftButton", false);
    drag_tab(&mut s, 90.0, 60.0);

    let rect = |s: &mut UiScript| {
        s.resolve();
        s.eval::<(f64, f64, f64, f64)>(
            "return ChatFrame1:GetLeft(), ChatFrame1:GetBottom(), \
                    ChatFrame1:GetWidth(), ChatFrame1:GetHeight()",
        )
        .unwrap()
    };
    let want = rect(&mut s);
    assert_eq!((want.2, want.3), (500.0, 160.0), "the resize happened");
    assert!(
        s.eval::<bool>("return GetChatWindowSavedPosition(1) ~= nil")
            .unwrap(),
        "the drag's end saved a position"
    );

    s.run(
        "ChatFrame1:ClearAllPoints() ChatFrame1:SetPoint('CENTER') ChatFrame1:SetSize(300, 130) \
         FCF_RestorePositionAndDimensions(ChatFrame1)",
    )
    .unwrap();
    let got = rect(&mut s);
    for (g, w) in [got.0, got.1, got.2, got.3]
        .into_iter()
        .zip([want.0, want.1, want.2, want.3])
    {
        assert!((g - w).abs() < 1e-3, "restored {got:?}, saved {want:?}");
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `CHAT_FRAME_TEXTURES` rest at the window's alpha (`oldAlpha`), reveal at least at
/// `DEFAULT_CHATFRAME_ALPHA`, and take the window colour as one (`FCF_SetWindowColor`).
#[test]
fn the_chrome_follows_the_windows_reveal_and_tint() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    s.mouse_move(1500.0, 850.0);
    frames(&mut s, 4, 0.016);
    let rest: f64 = s.eval("return ChatFrame1.oldAlpha").unwrap();
    for piece in ["Background", "TopLeftTexture", "ButtonFrameBackground"] {
        let alpha: f64 = s
            .eval(&format!("return ChatFrame1{piece}:GetAlpha()"))
            .unwrap();
        assert!(
            (alpha - rest).abs() < 1e-6,
            "{piece} rests at the window's alpha {rest}: {alpha}"
        );
    }
    reveal(&mut s);
    let shown = rest.max(0.25);
    for piece in ["Background", "TopLeftTexture"] {
        let alpha: f64 = s
            .eval(&format!("return ChatFrame1{piece}:GetAlpha()"))
            .unwrap();
        assert!(
            (alpha - shown).abs() < 1e-6,
            "{piece} reveals at {shown}: {alpha}"
        );
    }
    s.run("FCF_SetWindowColor(ChatFrame1, 1, 0, 0)").unwrap();
    for piece in ["Background", "TopLeftTexture"] {
        let (r, g, b): (f64, f64, f64) = s
            .eval(&format!("return ChatFrame1{piece}:GetVertexColor()"))
            .unwrap();
        assert_eq!((r, g, b), (1.0, 0.0, 0.0), "{piece} takes the colour");
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `FCF_OnUpdate` keeps the chrome in while the resize button is pushed, cursor off the window or
/// not, and lets it fade after the release.
#[test]
fn a_resize_in_flight_holds_the_chrome_visible() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    s.run("FCF_SetLocked(ChatFrame1, nil)").unwrap();
    reveal(&mut s);
    grab_resize(&mut s, "ChatFrame1");
    s.mouse_move(1500.0, 820.0);
    frames(&mut s, 45, 0.016);
    let (alpha, rest): (f64, f64) = s
        .eval("return ChatFrame1Background:GetAlpha(), ChatFrame1.oldAlpha")
        .unwrap();
    let shown = rest.max(0.25);
    assert!(
        (alpha - shown).abs() < 1e-6,
        "the pushed button holds the reveal open with the cursor off the window: {alpha}"
    );
    s.mouse_button(1500.0, 820.0, "LeftButton", false);
    // `CHAT_TAB_HIDE_DELAY` of 1s, then a 2s fade (`CHAT_FRAME_FADE_OUT_TIME`).
    frames(&mut s, 40, 0.1);
    let alpha: f64 = s.eval("return ChatFrame1Background:GetAlpha()").unwrap();
    assert!(
        (alpha - rest).abs() < 1e-6,
        "and the chrome fades back once the resize ends: {alpha}"
    );
}

/// Docked windows hang from the dock's primary window, so a drag of its tab moves the dock as one.
#[test]
fn the_dock_moves_as_one() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    s.run("FCF_SelectDockFrame(ChatFrame1) FCF_SetLocked(ChatFrame1, nil)")
        .unwrap();
    drag_tab(&mut s, 60.0, 0.0);
    s.resolve();
    let (one, two): (f64, f64) = s
        .eval("return ChatFrame1:GetLeft(), ChatFrame2:GetLeft()")
        .unwrap();
    assert_eq!(one, two, "the dock moved as one");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
