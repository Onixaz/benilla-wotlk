//! The cooldown bindings: per-action state and the `GetTime`-space triples stock `Cooldown.lua`
//! consumes, and the `Cooldown` widget it drives.

use super::common::script;
use crate::script::*;

/// The 1/nil conventions, `IsActionInRange`'s tri-state, and a cooldown that goes cold at expiry.
#[test]
fn action_state_bindings_answer_the_reference_conventions() {
    let mut s = script();

    assert!(s.eval::<bool>("return IsUsableAction(3) == nil").unwrap());
    assert!(s.eval::<bool>("return IsActionInRange(3) == nil").unwrap());
    assert!(s
        .eval::<bool>("local st, d, e = GetActionCooldown(3); return st == 0 and d == 0 and e == 1")
        .unwrap());

    s.tick(100.0); // an arbitrary clock epoch
    s.set_action_state(
        3,
        Some(ActionState {
            usable: false,
            not_enough_mana: true,
            in_range: Some(false),
            has_range: true,
            current: true,
            auto_repeat: false,
            is_attack: false,
            equipped: false,
            // 4 s remaining of a 10 s cooldown, running: started at GetTime 94.
            cooldown: Some((94_000, 10_000, true)),
        }),
    );

    assert!(s
        .eval::<bool>("local u, oom = IsUsableAction(3); return u == nil and oom == 1")
        .unwrap());
    assert!(s.eval::<bool>("return IsActionInRange(3) == 0").unwrap());
    assert!(s.eval::<bool>("return ActionHasRange(3) == 1").unwrap());
    assert!(s.eval::<bool>("return IsCurrentAction(3) == 1").unwrap());
    assert!(s
        .eval::<bool>("return IsAutoRepeatAction(3) == nil")
        .unwrap());
    // `IsConsumableAction` reads the slot, not this state: it arrives with the icon.
    assert!(s
        .eval::<bool>("return IsConsumableAction(3) == nil")
        .unwrap());
    s.set_action(
        3,
        Some(ActionSlot {
            texture: None,
            kind: 0x80,
            action: 117,
            count: 4,
            consumable: true,
            stackable: false,
            pet_spell: None,
        }),
    );
    assert!(s.eval::<bool>("return IsConsumableAction(3) == 1").unwrap());

    assert!(s
        .eval::<bool>(
            "local st, d, e = GetActionCooldown(3); \
             return math.abs(st - 94) < 0.001 and d == 10 and e == 1"
        )
        .unwrap());

    // It ends at 94 + 10 = 104: live at 103.9 and cold after, so a re-fed finished pair cannot
    // replay the sweep.
    s.tick(3.9);
    assert!(s
        .eval::<bool>("local st, d = GetActionCooldown(3); return d == 10")
        .unwrap());
    s.tick(0.2);
    assert!(s
        .eval::<bool>("local st, d, e = GetActionCooldown(3); return st == 0 and d == 0 and e == 1")
        .unwrap());

    // An on-hold cooldown (enable 0) never goes cold on its own; it waits for the event.
    s.set_action_state(
        3,
        Some(ActionState {
            cooldown: Some((100_000, 30_000, false)),
            ..Default::default()
        }),
    );
    s.tick(120.0);
    assert!(s
        .eval::<bool>("local st, d, e = GetActionCooldown(3); return d == 30 and e == 0")
        .unwrap());

    s.set_action_state(3, None);
    assert!(s.eval::<bool>("return IsCurrentAction(3) == nil").unwrap());
}

/// The absolute start anchors the sweep: re-pushing a running cooldown (a kill flipping `usable`)
/// keeps it, so the sweep does not reset, and a re-arm moves it even at the same duration.
#[test]
fn the_absolute_start_triple_holds_the_anchor_and_a_rearm_moves_it() {
    let mut s = script();
    s.tick(100.0);
    let cooling = |usable: bool| ActionState {
        usable,
        cooldown: Some((100_000, 15_000, true)),
        ..Default::default()
    };
    s.set_action_state(3, Some(cooling(false)));
    assert!(s
        .eval::<bool>("local st = GetActionCooldown(3); return math.abs(st - 100) < 1e-3")
        .unwrap());

    s.tick(8.0);
    s.set_action_state(3, Some(cooling(true)));
    assert!(s
        .eval::<bool>(
            "local st, d = GetActionCooldown(3); \
             return math.abs(st - 100) < 1e-3 and d == 15"
        )
        .unwrap());

    s.set_action_state(
        3,
        Some(ActionState {
            cooldown: Some((107_000, 15_000, true)),
            ..Default::default()
        }),
    );
    assert!(s
        .eval::<bool>("local st = GetActionCooldown(3); return math.abs(st - 107) < 1e-3")
        .unwrap());
}

#[test]
fn shapeshift_and_container_cooldowns_keep_their_anchors_too() {
    let mut s = script();
    s.tick(50.0);
    let form = || ShapeshiftFormView {
        spell_id: 2457,
        name: "Battle Stance".into(),
        cooldown: Some((50_000, 10_000, true)),
        ..Default::default()
    };
    s.set_shapeshift_forms(vec![form()]);
    let slot = || ContainerSlot {
        item_id: 118,
        count: 1,
        cooldown: Some((50_000, 30_000, true)),
        ..Default::default()
    };
    let bag = || ContainerState {
        bag_family: 0,
        name: Some("Backpack".into()),
        num_slots: 16,
        slots: std::collections::HashMap::from([(1, slot())]),
    };
    s.set_container(0, Some(bag()));
    assert!(s
        .eval::<bool>("local st = GetShapeshiftFormCooldown(1); return math.abs(st - 50) < 1e-3")
        .unwrap());
    assert!(s
        .eval::<bool>("local st = GetContainerItemCooldown(0, 1); return math.abs(st - 50) < 1e-3")
        .unwrap());

    // The same triples re-pushed 5 s later: the anchors hold at 50.
    s.tick(5.0);
    s.set_shapeshift_forms(vec![form()]);
    s.set_container(0, Some(bag()));
    assert!(s
        .eval::<bool>("local st = GetShapeshiftFormCooldown(1); return math.abs(st - 50) < 1e-3")
        .unwrap());
    assert!(s
        .eval::<bool>("local st = GetContainerItemCooldown(0, 1); return math.abs(st - 50) < 1e-3")
        .unwrap());
}

/// 3.3.5a's native `Cooldown` widget: a `Frame` subtype with five methods of its own;
/// `SetCooldown` shows it and starts the sweep, which the tick runs through a one-second flash to
/// a hide, and the extract paints from the frame's own slot.
#[test]
fn the_cooldown_widget_sweeps_flashes_and_hides() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.tick(100.0);
    s.run(
        r#"
        cd = CreateFrame("Cooldown", "SweepCD")
        cd:SetPoint("CENTER", 0, 0) cd:SetWidth(36) cd:SetHeight(36)
        cd:Hide()
        assert(cd:GetObjectType() == "Cooldown")
        assert(cd:IsObjectType("Frame") and cd:IsObjectType("Cooldown"))
        assert(cd:GetReverse() == false and cd:GetDrawEdge() == false)
        cd:SetReverse(1) assert(cd:GetReverse() == true) cd:SetReverse(nil)
        cd:SetCooldown(GetTime(), 10)
        assert(cd:IsShown(), "SetCooldown shows the frame")
    "#,
    )
    .unwrap();
    let paint = |s: &mut UiScript| {
        s.resolve();
        s.extract().into_iter().find_map(|q| match q.content {
            QuadContent::Cooldown(p) => Some(p),
            _ => None,
        })
    };
    s.tick(5.0);
    let half = paint(&mut s).expect("a running sweep paints");
    assert_eq!(half.swipe.len(), 5, "half-way: six o'clock back to twelve");
    assert!(half.flash.is_none() && half.edge.is_none());
    s.tick(5.0);
    let flash = paint(&mut s).expect("the flash paints");
    assert!(flash.swipe.is_empty() && flash.flash.is_some());
    assert!(s.eval::<bool>("return SweepCD:IsShown()").unwrap());
    s.tick(1.0);
    assert!(
        !s.eval::<bool>("return SweepCD:IsShown()").unwrap(),
        "the flash's end hides the frame"
    );

    let e = s
        .run(r#"SweepCD:SetCooldown("soon", 1)"#)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("Usage: SweepCD:SetCooldown(start, duration)"),
        "{e}"
    );
}

/// The stock template, `<Cooldown name="CooldownFrameTemplate" setAllPoints="true"
/// hidden="true" virtual="true"/>` (`Cooldown.xml`), builds through the XML loader, and the stock
/// `CooldownFrame_SetTimer` drives it.
#[test]
fn the_stock_cooldown_template_loads_and_cooldown_frame_set_timer_drives_it() {
    let s = script();
    let doc = crate::framexml::parse(
        r#"<Ui>
        <Cooldown name="CooldownFrameTemplate" setAllPoints="true" hidden="true" virtual="true"/>
        <Button name="CDButton">
            <Size><AbsDimension x="36" y="36"/></Size>
            <Frames><Cooldown name="$parentCooldown" inherits="CooldownFrameTemplate"/></Frames>
        </Button>
        </Ui>"#,
    )
    .unwrap();
    let report = crate::loader::load_in(&s, &doc, "test.xml", &|_| None);
    assert!(report.errors.is_empty(), "{:#?}", report.errors);
    assert!(report.warnings.is_empty(), "{:#?}", report.warnings);
    s.run(
        r#"
        function CooldownFrame_SetTimer(self, start, duration, enable)
            if ( start > 0 and duration > 0 and enable > 0) then
                self:SetCooldown(start, duration);
                self:Show();
            else
                self:Hide();
            end
        end
        assert(CDButtonCooldown:GetObjectType() == "Cooldown")
        assert(not CDButtonCooldown:IsShown())
        CooldownFrame_SetTimer(CDButtonCooldown, 1, 30, 1)
        assert(CDButtonCooldown:IsShown())
        CooldownFrame_SetTimer(CDButtonCooldown, 0, 0, 1)
        assert(not CDButtonCooldown:IsShown())
    "#,
    )
    .unwrap();
}
