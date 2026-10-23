//! The durability figure: stock `DurabilityFrame.lua` over the engine's `GetInventoryAlertStatus`,
//! in the whole production interface (the layer's re-seat in `FrameXMLFixes.xml` included).

use benilla_ui::script::{InvSlotView, InventorySlots, ItemTemplateView, UiScript};

fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    super::test_ui::load_stock_ui(&mut s);
    s.run(
        r#"ALERT_EVENTS = {}
           local f = CreateFrame("Frame")
           f:RegisterEvent("UPDATE_INVENTORY_ALERTS")
           f:RegisterEvent("UPDATE_INVENTORY_DURABILITY")
           f:SetScript("OnEvent", function(_, e) ALERT_EVENTS[e] = (ALERT_EVENTS[e] or 0) + 1 end)"#,
    )
    .unwrap();
    // The frame is authored shown; its `PLAYER_ENTERING_WORLD` arm runs `SetAlerts`, which hides it.
    s.run("DurabilityFrame:GetScript('OnEvent')(DurabilityFrame, 'PLAYER_ENTERING_WORLD')")
        .unwrap();
    s
}

fn slot(item_id: u32, durability: Option<(u32, u32)>) -> Option<InvSlotView> {
    Some(InvSlotView {
        item_id,
        durability,
        count: 1,
        quality: 2,
        ..Default::default()
    })
}

fn shown(s: &UiScript, frame: &str) -> bool {
    s.eval::<bool>(&format!("return {frame}:IsShown() and true or false"))
        .unwrap()
}

fn vertex_color(s: &UiScript, texture: &str) -> [f32; 4] {
    let (r, g, b, a): (f32, f32, f32, f32) = s
        .eval(&format!("return {texture}:GetVertexColor()"))
        .unwrap();
    [r, g, b, a]
}

/// Stock's tint for an alert status (`DurabilityFrame.lua:14-16`), opaque as `SetAlerts` paints it.
fn alert_color(s: &UiScript, status: u8) -> [f32; 4] {
    let (r, g, b): (f32, f32, f32) = s
        .eval(&format!(
            "local c = INVENTORY_ALERT_COLORS[{status}] return c.r, c.g, c.b"
        ))
        .unwrap();
    [r, g, b, 1.0]
}

fn near(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
}

fn alert_events(s: &UiScript) -> (i64, i64) {
    s.eval(
        "return ALERT_EVENTS.UPDATE_INVENTORY_ALERTS or 0, \
                ALERT_EVENTS.UPDATE_INVENTORY_DURABILITY or 0",
    )
    .unwrap()
}

/// `DurabilityFrame_SetAlerts`: one body alert shows every body piece, the rest faded white; the
/// statuses are 12340's 1 and 2 (`0x5e8fe0`), the indices of stock's color table.
#[test]
fn armor_guy_shows_red_broken_yellow_damaged_and_hides_clean() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    assert!(!shown(&s, "DurabilityFrame"), "clean gear → no armor guy");

    let mut inv: InventorySlots = Default::default();
    inv[16] = slot(25, Some((0, 20)));
    s.set_inventory_slots(inv);
    assert!(s.errors().is_empty(), "alert errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<i64>("return GetInventoryAlertStatus(9)").unwrap(),
        2
    );
    assert!(
        shown(&s, "DurabilityFrame") && shown(&s, "DurabilityWeapon"),
        "a broken weapon shows the frame and its glyph"
    );
    assert!(
        !shown(&s, "DurabilityHead"),
        "no body alert → the body stays hidden"
    );
    assert!(near(
        vertex_color(&s, "DurabilityWeapon"),
        alert_color(&s, 2)
    ));

    // Legs at 3 points: damaged is an absolute 1..=5 points left (`0x5e90b2`).
    let mut inv: InventorySlots = Default::default();
    inv[7] = slot(39, Some((3, 25)));
    s.set_inventory_slots(inv);
    assert_eq!(
        s.eval::<i64>("return GetInventoryAlertStatus(5)").unwrap(),
        1
    );
    assert!(
        shown(&s, "DurabilityFrame") && shown(&s, "DurabilityLegs") && shown(&s, "DurabilityHead"),
        "one body alert shows the whole body"
    );
    assert!(
        !shown(&s, "DurabilityWeapon"),
        "the repaired weapon glyph hides"
    );
    assert!(near(vertex_color(&s, "DurabilityLegs"), alert_color(&s, 1)));
    assert!(
        near(vertex_color(&s, "DurabilityHead"), [1.0, 1.0, 1.0, 0.5]),
        "an un-alerted body piece rides faded white"
    );

    // Absolute, not a ratio: 5 of 100 is damaged, 6 of 20 is not.
    let mut inv: InventorySlots = Default::default();
    inv[7] = slot(39, Some((5, 100)));
    s.set_inventory_slots(inv);
    assert!(
        shown(&s, "DurabilityFrame"),
        "5 points left is damaged at any max"
    );
    let mut inv: InventorySlots = Default::default();
    inv[7] = slot(39, Some((6, 20)));
    s.set_inventory_slots(inv);
    assert!(
        !shown(&s, "DurabilityFrame"),
        "6 points left is never damaged"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// Item flag `0x10` forces red and `0x08` (wrapped) silences the region (`0x5e9063`, `0x5e9077`);
/// region 12 is low ammo (20 or fewer carried reads 1), which stock's 11 slots never read.
#[test]
fn flag_bits_and_the_low_ammo_region() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();

    let mut inv: InventorySlots = Default::default();
    let mut v = slot(25, Some((20, 20))).unwrap();
    v.flags = 0x10;
    inv[16] = Some(v);
    s.set_inventory_slots(inv);
    assert_eq!(
        s.eval::<i64>("return GetInventoryAlertStatus(9)").unwrap(),
        2,
        "force-red bit → broken at full durability"
    );
    assert!(near(
        vertex_color(&s, "DurabilityWeapon"),
        alert_color(&s, 2)
    ));
    assert!(
        s.eval::<bool>("return GetInventoryItemBroken('player', 16) == nil")
            .unwrap(),
        "the force-red bit is the alert's alone (`0x5e9d80`)"
    );

    let mut inv: InventorySlots = Default::default();
    let mut v = slot(25, Some((0, 20))).unwrap();
    v.flags = 0x08;
    inv[16] = Some(v);
    s.set_inventory_slots(inv);
    assert!(!shown(&s, "DurabilityFrame"), "a wrapped item never alerts");

    let mut inv: InventorySlots = Default::default();
    inv[0] = slot(2512, None).map(|mut v| {
        v.count = 15;
        v
    });
    s.set_inventory_slots(inv);
    assert_eq!(
        s.eval::<i64>("return GetInventoryAlertStatus(12)").unwrap(),
        1,
        "low carried ammo reads 1 on the 12th region"
    );
    assert!(
        !shown(&s, "DurabilityFrame"),
        "the armor guy never shows for ammo"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// `UPDATE_INVENTORY_ALERTS` only on a changed status, `UPDATE_INVENTORY_DURABILITY` on every
/// recompute (`0x5e90d0`); the broken and damaged arms raise tutorials `0x24` and `0x23`
/// (`0x5e909c`, `0x5e90b7`), the force-red arm none.
#[test]
fn the_recompute_diffs_its_alert_event_and_raises_the_tutorials() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    let _ = s.take_tutorial_triggers();

    let mut inv: InventorySlots = Default::default();
    inv[16] = slot(25, Some((0, 20)));
    s.set_inventory_slots(inv.clone());
    assert_eq!(alert_events(&s), (1, 1));
    assert_eq!(s.take_tutorial_triggers(), vec![0x24]);
    s.set_inventory_slots(inv);
    assert_eq!(
        alert_events(&s),
        (1, 2),
        "same statuses → durability event only"
    );
    // Every recompute raises it again; the tutorial's own seen flag keeps it to one popup.
    assert_eq!(s.take_tutorial_triggers(), vec![0x24]);

    let mut inv: InventorySlots = Default::default();
    inv[7] = slot(39, Some((3, 25)));
    s.set_inventory_slots(inv);
    assert_eq!(alert_events(&s), (2, 3));
    assert_eq!(s.take_tutorial_triggers(), vec![0x23]);

    let mut inv: InventorySlots = Default::default();
    let mut v = slot(25, Some((20, 20))).unwrap();
    v.flags = 0x10;
    inv[16] = Some(v);
    s.set_inventory_slots(inv);
    assert!(
        s.take_tutorial_triggers().is_empty(),
        "force-red raises none"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// `SetAlerts`' Shield arm swaps to the off-weapon glyph when `OffhandHasWeapon()` (item class 2).
#[test]
fn off_hand_glyph_follows_what_the_hand_holds() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.set_item_template(
        2362,
        ItemTemplateView {
            name: "Worn Wooden Shield".into(),
            class: 4, // armor
            ..Default::default()
        },
    );
    s.set_item_template(
        2488,
        ItemTemplateView {
            name: "Worn Dagger".into(),
            class: 2, // weapon
            ..Default::default()
        },
    );

    let mut inv: InventorySlots = Default::default();
    inv[17] = slot(2362, Some((0, 20)));
    s.set_inventory_slots(inv);
    assert!(
        shown(&s, "DurabilityShield") && !shown(&s, "DurabilityOffWeapon"),
        "a shield lights the shield glyph"
    );
    assert!(near(
        vertex_color(&s, "DurabilityShield"),
        alert_color(&s, 2)
    ));

    // A changed status, so the event runs `SetAlerts`: the same status re-fires nothing.
    let mut inv: InventorySlots = Default::default();
    inv[17] = slot(2488, Some((3, 16)));
    s.set_inventory_slots(inv);
    assert!(
        shown(&s, "DurabilityOffWeapon") && !shown(&s, "DurabilityShield"),
        "an off-hand weapon swaps to the off-weapon glyph"
    );
    assert!(near(
        vertex_color(&s, "DurabilityOffWeapon"),
        alert_color(&s, 1)
    ));
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// The manage pass seats `TOPRIGHT` at the cluster's `BOTTOMRIGHT` less `CONTAINER_OFFSET_X`, and
/// 20 more while the shield, off-weapon or ranged glyph shows (`UIParent.lua:1913-1916`).
#[test]
fn manage_pass_seats_the_frame_inside_the_cluster_edge() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    let offset: f32 = s.eval("return CONTAINER_OFFSET_X").unwrap();

    // Broken ranged; `OnShow` runs the pass.
    let mut inv: InventorySlots = Default::default();
    inv[18] = slot(2504, Some((0, 20)));
    s.set_inventory_slots(inv);
    s.resolve();
    let delta: f32 = s
        .eval("return MinimapCluster:GetRight() - DurabilityFrame:GetRight()")
        .unwrap();
    assert_eq!(
        delta,
        offset + 20.0,
        "a right-side glyph seats the frame 20 further left"
    );

    s.set_inventory_slots(Default::default());
    let mut inv: InventorySlots = Default::default();
    inv[7] = slot(39, Some((3, 25)));
    s.set_inventory_slots(inv);
    s.resolve();
    let delta: f32 = s
        .eval("return MinimapCluster:GetRight() - DurabilityFrame:GetRight()")
        .unwrap();
    assert_eq!(
        delta, offset,
        "body-only alerts seat at the container offset"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// Gear streams in slot by slot at login, so a side glyph can appear while the frame is shown;
/// the re-seat deviation still pulls it 20 in so the shield's overhang clears the edge.
#[test]
fn a_late_side_glyph_refreshes_the_seat_while_the_frame_stays_shown() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.set_item_template(
        25,
        ItemTemplateView {
            name: "Worn Shortsword".into(),
            class: 2, // weapon
            ..Default::default()
        },
    );
    s.set_item_template(
        30,
        ItemTemplateView {
            name: "Large Round Shield".into(),
            class: 4, // armor (a shield, not an off-hand weapon)
            ..Default::default()
        },
    );
    let offset: f32 = s.eval("return CONTAINER_OFFSET_X").unwrap();

    let mut inv: InventorySlots = Default::default();
    inv[16] = slot(25, Some((0, 20)));
    s.set_inventory_slots(inv);
    s.resolve();
    assert!(
        shown(&s, "DurabilityFrame") && !shown(&s, "DurabilityShield"),
        "weapon-only: frame shown, no shield glyph yet"
    );
    let delta: f32 = s
        .eval("return MinimapCluster:GetRight() - DurabilityFrame:GetRight()")
        .unwrap();
    assert_eq!(
        delta, offset,
        "the left-side weapon glyph needs no extra room"
    );

    // The frame is already shown, so no show transition fires.
    let mut inv: InventorySlots = Default::default();
    inv[16] = slot(25, Some((0, 20)));
    inv[17] = slot(30, Some((3, 35)));
    s.set_inventory_slots(inv);
    s.resolve();
    assert!(
        shown(&s, "DurabilityShield"),
        "the off-hand's arrival shows the shield glyph"
    );
    let delta: f32 = s
        .eval("return MinimapCluster:GetRight() - DurabilityFrame:GetRight()")
        .unwrap();
    assert_eq!(
        delta,
        offset + 20.0,
        "a side glyph arriving while shown must still pull the frame 20 in"
    );
    let overhang: f32 = s
        .eval("return DurabilityShield:GetRight() - GetScreenWidth()")
        .unwrap();
    assert!(
        overhang <= 0.5,
        "shield glyph clips off the right edge (overhang {overhang} px)"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// `WatchFrame` follows in the right-side walk, seated 10 under the shown durability frame
/// (`UIParent.lua:1917-1918`, `:1937`).
#[test]
fn the_quest_tracker_stacks_below_the_durability_guy() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();

    let mut inv: InventorySlots = Default::default();
    inv[18] = slot(2504, Some((0, 20)));
    s.set_inventory_slots(inv);
    s.resolve();

    let dur_bottom: f32 = s.eval("return DurabilityFrame:GetBottom()").unwrap();
    let watch_top: f32 = s.eval("return WatchFrame:GetTop()").unwrap();
    assert!(
        (dur_bottom - 10.0 - watch_top).abs() <= 0.5,
        "tracker top {watch_top} must sit 10 under the durability bottom {dur_bottom}"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}
