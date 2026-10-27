//! The stock pet frame, `PetFrame.xml`, over `"pet"` snapshots built from synthetic descriptors
//! and the feed's events.
//! `UNIT_PET` names the owner (`arg1 == "player"`, `0x4bc84f`) and every other `UNIT_*` names the
//! pet: a frame that mixes the two repaints off the player's health.

use benilla_ui::script::{
    AuraState, QuadContent, ScriptValue, SelectionRequest, UiScript, UnitState,
};

/// The pet frame's load chain, with a player seated: `PetFrame` is a child of `PlayerFrame`
/// (`PetFrame.xml:4`), and `UnitFrame_Update` hides a frame whose unit does not exist.
fn load_pet_frame() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    super::test_ui::load_stock_ui(&mut s);
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Tri".into()),
            health: 100,
            max_health: 100,
            level: 60,
            power_type: 0,
            power: 100,
            max_power: 100,
            faction_group: Some("Alliance".into()),
            ..UnitState::default()
        }),
    );
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);
    s
}

/// The production 3.3.5a load with a player seated, the environment lock held for the test;
/// errors from the load's unrelated gaps are dropped.
fn load_stock_pet_frame() -> (std::sync::MutexGuard<'static, ()>, UiScript) {
    let lock = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _failures) =
        super::layer_tests::production_load_with("pet-frame", false, "", |_| {});
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            has_object: true,
            name: Some("Tri".into()),
            health: 100,
            max_health: 100,
            level: 60,
            power_type: 0,
            power: 100,
            max_power: 100,
            faction_group: Some("Alliance".into()),
            ..UnitState::default()
        }),
    );
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);
    s.resolve();
    let _ = s.take_errors();
    (lock, s)
}

/// Errors from the pet frame's and the pet bar's own code.
fn pet_errors(s: &mut UiScript) -> Vec<String> {
    s.take_errors()
        .into_iter()
        .filter(|e| e.contains("PetFrame") || e.contains("PetActionBar"))
        .collect()
}

/// A pet snapshot off a descriptor through the real [`crate::ui_unit::snapshot`], as the `"pet"`
/// feed builds it, so whatever that builder leaves unset (the connection flag included) reaches the
/// stock frame here as it does in game; `max_power == 0` is a powerless pet, which swaps the art.
fn pet(name: &str, health: u32, power: u32, max_power: u32, power_type: u8) -> UnitState {
    // `UNIT_FIELD_` HEALTH, POWER1, MAXHEALTH, MAXPOWER1, LEVEL, BYTES_0 (the power type is its
    // byte 3), the power pair at its type's slot.
    const HEALTH: u16 = 22;
    const POWER1: u16 = 23;
    const MAXHEALTH: u16 = 28;
    const MAXPOWER1: u16 = 29;
    const LEVEL: u16 = 34;
    const BYTES_0: u16 = 36;
    let slot = u16::from(power_type);
    let store = crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
        (HEALTH, health),
        (MAXHEALTH, 100),
        (LEVEL, 60),
        (BYTES_0, u32::from(power_type) << 24),
        (POWER1 + slot, power),
        (MAXPOWER1 + slot, max_power),
    ]));
    crate::ui_unit::snapshot(&store, 0, Some(name.into()), 0, None, Default::default())
}

/// Every texture path drawn this frame, with its vertex tint.
fn drawn(s: &mut UiScript) -> Vec<(String, Option<[f32; 4]>)> {
    s.resolve();
    s.extract()
        .into_iter()
        .filter_map(|q| match q.content {
            QuadContent::Texture {
                path: Some(p),
                color,
                ..
            } => Some((p, color)),
            _ => None,
        })
        .collect()
}

/// Exact, not `contains`: `UI-SmallTargetingFrame` prefixes the `-NoMana` plate's path.
fn draws(s: &mut UiScript, path: &str) -> bool {
    drawn(s).iter().any(|(p, _)| p == path)
}

/// One helpful aura, which the pet frame's debuff row never shows.
fn pet_buff(spell_id: u32, name: &str, count: u8) -> AuraState {
    AuraState {
        spell_id,
        name: Some(name.into()),
        icon: Some(format!("Interface\\Icons\\Spell_{spell_id}")),
        count,
        debuff_type: None,
        // No duration, so no cooldown sweep.
        duration: 0.0,
        expiration_time: 0.0,
        helpful: true,
        cancelable: false,
        until_cancelled: false,
        channeled: false,
        ..Default::default()
    }
}

/// `UNIT_PET` is the one event for both the summon and the dismiss, and a resummon shows the
/// frame again (#340).
#[test]
fn the_pet_frame_appears_on_a_summon_and_leaves_on_a_dismiss() {
    benilla_formats::wow_data_or_skip!();
    let (_lock, mut s) = load_stock_pet_frame();
    assert!(
        !s.eval::<bool>("return PetFrame:IsVisible()").unwrap(),
        "no pet, no frame"
    );

    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 0)));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);

    let ok: bool = s
        .eval(
            r#"
            local hb, mb = PetFrameHealthBar, PetFrameManaBar
            local _, hmax = hb:GetMinMaxValues()
            local _, mmax = mb:GetMinMaxValues()
            return PetFrame:IsVisible()
               and PetName:GetText() == "Grimjaw"
               and hb:GetValue() == 72 and hmax == 100
               and mb:GetValue() == 45 and mmax == 80 and mb:IsVisible()
            "#,
        )
        .unwrap();
    assert!(ok, "the summoned pet's name, health and power all draw");

    // The dismiss, as `feed_pet_unit` sends it: the token clears, then `UNIT_PET`.
    s.set_unit("pet", None);
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert!(!s.eval::<bool>("return PetFrame:IsVisible()").unwrap());

    // The resummon.
    s.set_unit("pet", Some(pet("Grimjaw", 100, 80, 80, 0)));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert!(s.eval::<bool>("return PetFrame:IsVisible()").unwrap());
    let errors = pet_errors(&mut s);
    assert!(errors.is_empty(), "pet frame errors: {errors:#?}");
}

#[test]
fn the_frame_answers_only_the_events_that_name_its_own_unit() {
    benilla_formats::wow_data_or_skip!();
    let (_lock, mut s) = load_stock_pet_frame();
    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 0)));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert!(s.eval::<bool>("return PetFrame:IsVisible()").unwrap());

    // At `predictedHealth`'s registered "1" the bar reads `UnitHealth` on its OnUpdate
    // (UnitFrame.lua:211-212), with no event at all.
    s.set_unit("pet", Some(pet("Grimjaw", 40, 45, 80, 0)));
    s.tick(0.1);
    assert_eq!(
        s.eval::<f64>("return PetFrameHealthBar:GetValue()")
            .unwrap(),
        40.0,
        "the predicted bar follows the pet's health each frame"
    );

    // Off, `VARIABLES_LOADED` puts the bar back on `UNIT_HEALTH` (UnitFrame.lua:234-240), which
    // only an event naming the pet moves.
    s.run("SetCVar('predictedHealth', '0')").unwrap();
    s.fire_event("VARIABLES_LOADED", vec![]);
    s.set_unit("pet", Some(pet("Grimjaw", 5, 45, 80, 0)));
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("player".into())]);
    s.tick(0.1);
    assert_eq!(
        s.eval::<f64>("return PetFrameHealthBar:GetValue()")
            .unwrap(),
        40.0,
        "a UNIT_HEALTH for \"player\" is not the pet's event"
    );
    s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("pet".into())]);
    assert_eq!(
        s.eval::<f64>("return PetFrameHealthBar:GetValue()")
            .unwrap(),
        5.0
    );

    s.set_unit("pet", None);
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("pet".into())]);
    assert!(
        s.eval::<bool>("return PetFrame:IsVisible()").unwrap(),
        "UNIT_PET names the OWNER — a \"pet\" arg1 is somebody else's event"
    );
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert!(!s.eval::<bool>("return PetFrame:IsVisible()").unwrap());
}

/// A powerless pet wears the `-NoMana` plate (`PetFrame.lua:49-54`), recut only on `UNIT_PET` or
/// OnShow (`PetFrame.lua:67-78`, `PetFrame.xml:279-283`). Its bar hides through
/// `TextStatusBar_UpdateTextString`, which hides a bar whose max is 0 (`TextStatusBar.lua:76-82`),
/// so only a value change brings it back.
#[test]
fn a_powerless_pet_wears_the_no_mana_plate() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();

    const PLAIN: &str = "Interface\\TargetingFrame\\UI-SmallTargetingFrame";
    const NO_MANA: &str = "Interface\\TargetingFrame\\UI-SmallTargetingFrame-NoMana";

    // Focus (power type 2), a hunter's pet: the plain plate and the focus colour.
    s.set_unit("pet", Some(pet("Boar", 100, 60, 100, 2)));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    let (visible, r, g, b) = s
        .eval::<(bool, f64, f64, f64)>(
            "local r, g, b = PetFrameManaBar:GetStatusBarColor() \
             return PetFrameManaBar:IsVisible(), r, g, b",
        )
        .unwrap();
    assert!(visible);
    assert_eq!((r, g, b), (1.0, 0.5, 0.25), "FOCUS, not the mana default");
    assert!(draws(&mut s, PLAIN), "the plate with a mana rail");
    assert!(!draws(&mut s, NO_MANA));

    // A skeleton, with no power: `UNIT_PET` recuts the plate.
    s.set_unit("pet", Some(pet("Skeleton", 100, 0, 0, 0)));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert!(
        !s.eval::<bool>("return PetFrameManaBar:IsVisible()")
            .unwrap(),
        "no power bar on the plate that has no rail for it"
    );
    assert!(draws(&mut s, NO_MANA), "…and the plate swaps with it");
    assert!(!draws(&mut s, PLAIN));
}

/// `PET_ATTACK_START`/`PET_ATTACK_STOP` show and hide the attack overlay and repaint nothing else
/// (`PetFrame.lua:86-90`).
#[test]
fn the_attack_overlay_follows_its_own_two_events() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();
    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 0)));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert!(!s
        .eval::<bool>("return PetAttackModeTexture:IsVisible()")
        .unwrap());

    s.fire_event("PET_ATTACK_START", vec![]);
    assert!(s
        .eval::<bool>("return PetAttackModeTexture:IsVisible()")
        .unwrap());

    // `PetFrame_OnUpdate` ramps the alpha down from 1 on the opening leg; its constants are 0-255,
    // divided into the API's 0-1 (`PetFrame.lua:98-119`).
    s.tick(0.1);
    s.tick(0.1);
    let tint = drawn(&mut s)
        .into_iter()
        .find(|(p, _)| p.contains("UI-Player-AttackStatus"))
        .and_then(|(_, c)| c)
        .expect("the attack overlay draws while shown");
    assert!(
        (0.0..=1.0).contains(&tint[3]),
        "the pulse stays inside the alpha range (got {})",
        tint[3]
    );
    assert!(tint[3] < 1.0, "…and it has actually ramped off full");

    s.fire_event("PET_ATTACK_STOP", vec![]);
    assert!(!s
        .eval::<bool>("return PetAttackModeTexture:IsVisible()")
        .unwrap());
}

/// The row shows the pet's debuffs: `RefreshDebuffs(self, self.unit)` reads `UnitDebuff`
/// (`PetFrame.lua:58`, `UIParent.lua:3293-3346`). Its `PartyDebuffFrameTemplate` has an icon and a
/// border and no count.
#[test]
fn the_debuff_row_fills_from_the_pets_own_auras() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();
    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 0)));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);

    // `"pet"` resolves to the pet's guid, whose list the row reads.
    const PET: u64 = 0xF140_0000_0000_0077;
    s.set_unit_guids(&benilla_ui::script::UnitGuids {
        player: 1,
        pet: PET,
        ..Default::default()
    });
    s.set_unit_auras(
        PET,
        Some(vec![
            AuraState {
                helpful: false,
                ..pet_buff(1000, "Rend", 1)
            },
            AuraState {
                helpful: false,
                ..pet_buff(1001, "Sunder", 3)
            },
            // A buff never reaches the row.
            pet_buff(1002, "Blessing", 1),
        ]),
    );
    s.fire_event("UNIT_AURA", vec![ScriptValue::Str("pet".into())]);

    let ok: bool = s
        .eval(
            r#"
            return PetFrameDebuff1:IsVisible()
               and PetFrameDebuff2:IsVisible()
               and not PetFrameDebuff3:IsVisible()
               and not PetFrameDebuff4:IsVisible()
               -- The reference has no count region on this template at all — not a hidden one.
               and getglobal("PetFrameDebuff1Count") == nil
               and getglobal("PetFrameDebuff2Count") == nil
            "#,
        )
        .unwrap();
    assert!(ok, "two auras draw, the other two rows stay down");
    // The icons: `UnitDebuff`'s third return.
    assert!(draws(&mut s, "Interface\\Icons\\Spell_1000"));
    assert!(draws(&mut s, "Interface\\Icons\\Spell_1001"));
    assert!(!draws(&mut s, "Interface\\Icons\\Spell_1002"));

    s.set_unit_auras(PET, Some(vec![]));
    s.fire_event("UNIT_AURA", vec![ScriptValue::Str("pet".into())]);
    assert!(!s
        .eval::<bool>("return PetFrameDebuff1:IsVisible()")
        .unwrap());
}

/// A plain left-click targets the pet: `PetFrame` is a secure unit button, `*type1` target
/// (`PetFrame.lua:29`, `SecureTemplates.lua:555-560`).
#[test]
fn left_clicking_the_pet_frame_targets_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();
    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 0)));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);

    s.run("PetFrame:Click(\"LeftButton\")").unwrap();
    assert_eq!(
        s.take_selection_requests(),
        vec![SelectionRequest::Unit("pet".into())]
    );

    // The right button opens `PetFrameDropDown` (`PetFrame.lua:26-29`), never a target.
    s.run("PetFrame:Click(\"RightButton\")").unwrap();
    assert!(s.take_selection_requests().is_empty());
}

/// The secure `target` action tries a spell target, then an item drop, then a target
/// (`SecureTemplates.lua:402-415`). Every global it calls must exist, so food dropped on the pet
/// queues a drop instead of raising.
#[test]
fn every_leg_of_the_pet_frame_click_reaches_a_live_binding() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();
    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 0)));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);

    for global in [
        "SpellIsTargeting",
        "CursorHasItem",
        "DropItemOnUnit",
        "SpellTargetUnit",
    ] {
        assert!(
            s.eval::<bool>(&format!("return _G[\"{global}\"] ~= nil"))
                .unwrap_or(false),
            "{global} is not registered — the pet frame's click calls it"
        );
    }

    // The middle leg: food picked up from the backpack, then a click on the pet.
    s.set_container(
        0,
        Some(benilla_ui::script::ContainerState {
            bag_family: 0,
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::from([(
                1,
                benilla_ui::script::ContainerSlot {
                    item_id: 2287, // Haunch of Meat, pet food
                    count: 1,
                    ..Default::default()
                },
            )]),
        }),
    );
    s.fire_event("BAG_UPDATE", vec![ScriptValue::Int(0)]);
    s.run("PickupContainerItem(0, 1)").unwrap();
    assert!(
        s.eval::<bool>("return CursorHasItem()").unwrap(),
        "the food is on the cursor"
    );

    s.run("PetFrame:Click(\"LeftButton\")")
        .expect("the cursor-holds-an-item leg must not error");
    assert_eq!(
        s.take_drop_item_on_unit(),
        vec!["pet".to_string()],
        "a held item + a pet click queues the drop"
    );
    // The drop leg alone: the reference's if/elseif is exclusive.
    assert!(s.take_selection_requests().is_empty());
}

fn stats(hunter: bool, happiness: Option<u32>, damage: f32) -> benilla_ui::script::PetStats {
    benilla_ui::script::PetStats {
        hunter_pet: hunter,
        happiness,
        damage_percentage: damage,
        training_points: (170, 130),
        experience: (4200, 8000),
        // Family and diet stay default: the unit frame draws neither.
        ..benilla_ui::script::PetStats::default()
    }
}

/// The tooltip is `PET_HAPPINESS<n>` and `PET_DAMAGE_PERCENTAGE` over `GetPetHappiness`' second
/// return (`PetFrame_SetHappiness`, `PetFrame.lua:144-161`), so it names the bucket that painted.
#[test]
fn the_happiness_icon_shows_per_bucket_and_hides_for_a_non_hunter_pet() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();
    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 2)));

    for (bucket, tip, damage) in [
        (3u32, "Happy", 125.0f32),
        (2, "Content", 100.0),
        (1, "Unhappy", 75.0),
    ] {
        s.set_pet_stats(true, stats(true, Some(bucket), damage));
        s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
        assert!(
            s.eval::<bool>("return PetFrameHappiness:IsVisible()")
                .unwrap(),
            "bucket {bucket} must show the icon"
        );
        assert_eq!(
            s.eval::<String>("return PetFrameHappiness.tooltip")
                .unwrap(),
            tip,
            "bucket {bucket} took the wrong texcoord branch"
        );
        assert_eq!(
            s.eval::<String>("return PetFrameHappiness.tooltipDamage")
                .unwrap(),
            format!("Causes {}% of normal damage", damage as i32),
            "bucket {bucket} damage line"
        );
    }

    // A warlock's imp: `HasPetUI`'s second return is nil, so the icon hides (`PetFrame.lua:147`).
    s.set_pet_stats(true, stats(false, Some(3), 125.0));
    s.fire_event("UNIT_HAPPINESS", vec![]);
    assert!(!s
        .eval::<bool>("return PetFrameHappiness:IsVisible()")
        .unwrap());
}

/// The frame hides on `not happiness`, and 0 is truthy in Lua: `GetPetHappiness` (`0x5d3b00`)
/// answers a bucket as a number, never a nil.
#[test]
fn happiness_bucket_zero_keeps_the_icon_showing() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();
    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 2)));

    s.set_pet_stats(true, stats(true, Some(0), 100.0));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert!(
        s.eval::<bool>("return PetFrameHappiness:IsVisible()")
            .unwrap(),
        "bucket 0 is a number, not the hide case"
    );

    // A nil does hide it.
    s.set_pet_stats(true, stats(true, None, 100.0));
    s.fire_event("UNIT_HAPPINESS", vec![]);
    assert!(!s
        .eval::<bool>("return PetFrameHappiness:IsVisible()")
        .unwrap());
}

/// `UNIT_HAPPINESS` repaints the icon alone (`PetFrame.lua:91-92`).
#[test]
fn unit_happiness_repaints_only_the_icon() {
    benilla_formats::wow_data_or_skip!();
    let mut s = load_pet_frame();
    s.set_unit("pet", Some(pet("Grimjaw", 72, 45, 80, 2)));
    s.set_pet_stats(true, stats(true, Some(1), 75.0));
    s.fire_event("UNIT_PET", vec![ScriptValue::Str("player".into())]);
    assert_eq!(
        s.eval::<String>("return PetFrameHappiness.tooltip")
            .unwrap(),
        "Unhappy"
    );

    s.set_pet_stats(true, stats(true, Some(3), 125.0));
    s.fire_event("UNIT_HAPPINESS", vec![]);
    assert_eq!(
        s.eval::<String>("return PetFrameHappiness.tooltip")
            .unwrap(),
        "Happy",
        "UNIT_HAPPINESS must re-cut the icon on its own"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The art draws over the bar fills, which only frame level can hold. The art frame is anonymous,
/// nested two deep (`PetFrame.xml:63-65`) where the bars are direct children (`PetFrame.xml:136`,
/// `:158`), so it is reached as `PetFrameTexture:GetParent()`.
#[test]
fn the_pet_art_paints_over_the_bars() {
    benilla_formats::wow_data_or_skip!();
    let s = load_pet_frame();
    let level: (i64, i64, i64) = s
        .eval(
            "return PetFrameTexture:GetParent():GetFrameLevel(), \
                    PetFrameHealthBar:GetFrameLevel(), \
                    PetFrameManaBar:GetFrameLevel()",
        )
        .unwrap();
    let (art, health, mana) = level;
    assert_eq!(health, art - 1, "the health bar sits under the art");
    assert_eq!(mana, art - 1, "and so does the mana bar");
}

/// The stock pet bar's `OnLoad` needs `IsPetAttackAction` (`PetActionBarFrame.lua:130`); with it
/// the bar registers its events, shows on the bar's `PET_BAR_UPDATE` and slides out when the pet
/// goes (#323).
#[test]
fn the_stock_pet_bar_shows_with_the_pet_and_slides_out_after_it() {
    use benilla_ui::script::PetActionView;
    benilla_formats::wow_data_or_skip!();
    let (_lock, mut s) = load_stock_pet_frame();
    assert!(!s
        .eval::<bool>("return PetActionBarFrame:IsShown()")
        .unwrap());

    let attack = PetActionView {
        name: Some("PET_ACTION_ATTACK".into()),
        texture: Some("PET_ATTACK_TEXTURE".into()),
        is_token: true,
        packed: 0x0700_0002,
        slot_usable: true,
        ..Default::default()
    };
    let firebolt = PetActionView {
        name: Some("Firebolt".into()),
        texture: Some(r"Interface\Icons\Spell_Fire_FireBolt".into()),
        packed: 0x0100_0C26,
        slot_usable: false,
        ..Default::default()
    };
    let mut slots = vec![PetActionView::default(); 10];
    slots[0] = attack;
    slots[3] = firebolt;
    s.set_unit("pet", Some(pet("Jubgup", 100, 80, 80, 0)));
    s.set_pet_actions(true, true, true, slots);
    s.run(
        "ProbeDesat = {} local orig = SetDesaturation          SetDesaturation = function(t, d) ProbeDesat[t:GetName()] = d return orig(t, d) end",
    )
    .unwrap();
    s.fire_event("PET_BAR_UPDATE", vec![]);
    s.run(
        r#"
        assert(PetActionBarFrame:IsShown(), "the bar shows with the pet")
        assert(PetActionButton1:IsShown() and PetActionButton4:IsShown())
        assert(PetActionButton1Icon:GetTexture() ~= nil)
        "#,
    )
    .unwrap();
    // A slot the pet cannot use now desaturates its icon (`PetActionBarFrame.lua:153`).
    assert!(s
        .eval::<bool>(
            "return ProbeDesat.PetActionButton4Icon == 1 and ProbeDesat.PetActionButton1Icon == nil"
        )
        .unwrap());

    // The teardown: no bar, no pet; the bar slides out over its slide time.
    s.set_unit("pet", None);
    s.set_pet_actions(false, false, false, Vec::new());
    s.fire_event("PET_BAR_UPDATE", vec![]);
    for _ in 0..10 {
        s.tick(0.1);
    }
    assert!(!s
        .eval::<bool>("return PetActionBarFrame:IsShown()")
        .unwrap());
    let errors = pet_errors(&mut s);
    assert!(errors.is_empty(), "pet bar errors: {errors:#?}");
}
