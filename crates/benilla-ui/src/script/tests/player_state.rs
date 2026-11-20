//! The player-state, input and group verbs no stock file calls (#399): the movement and place
//! predicates, the sided modifiers, the mouse button verbs, the real group's leader, the unit
//! flags for sanctuary and feign death, the power regen and the resurrect offerer.

use crate::script::{
    LiveState, OptionState, PartyMemberInfo, PartyState, UiScript, UnitCombatStats, UnitState,
};

fn flag(s: &UiScript, chunk: &str) -> bool {
    s.eval::<Option<i64>>(&format!("return {chunk}"))
        .unwrap()
        .is_some()
}

/// Each predicate reads the state its macro condition reads; `IsOutdoors` is `IsIndoors`'
/// opposite, and `IsFlyableArea` in Northrend needs the training as `flyable` does.
#[test]
fn the_player_state_predicates_read_the_condition_state() {
    let mut s = UiScript::new().unwrap();
    for name in [
        "IsSwimming",
        "IsFlying",
        "IsStealthed",
        "IsIndoors",
        "IsFlyableArea",
    ] {
        assert!(!flag(&s, &format!("{name}()")), "{name} off");
    }
    assert!(flag(&s, "IsOutdoors()"));
    s.set_option_state(OptionState {
        swimming: true,
        flying: true,
        stealthed: true,
        indoors: true,
        flyable: true,
        ..Default::default()
    });
    for name in [
        "IsSwimming",
        "IsFlying",
        "IsStealthed",
        "IsIndoors",
        "IsFlyableArea",
    ] {
        assert!(flag(&s, &format!("{name}()")), "{name} on");
    }
    assert!(!flag(&s, "IsOutdoors()"));
    s.set_option_state(OptionState {
        flyable: true,
        northrend: true,
        ..Default::default()
    });
    assert!(!flag(&s, "IsFlyableArea()"));
    s.set_knows_cold_weather_flying(true);
    assert!(flag(&s, "IsFlyableArea()"));
}

/// Seated in a vehicle, `IsFlying` answers for the vehicle; the `flying` macro condition still
/// reads the player's own word.
#[test]
fn is_flying_follows_the_seated_vehicle_and_the_condition_does_not() {
    let mut s = UiScript::new().unwrap();
    let condition = |s: &UiScript| {
        s.eval::<Option<String>>("return (SecureCmdOptionParse('[flying] yes; no'))")
            .unwrap()
    };
    s.set_option_state(OptionState {
        flying: false,
        seated_vehicle_flying: Some(true),
        ..Default::default()
    });
    assert!(flag(&s, "IsFlying()"));
    assert_eq!(condition(&s).as_deref(), Some("no"));
    s.set_option_state(OptionState {
        flying: true,
        seated_vehicle_flying: Some(false),
        ..Default::default()
    });
    assert!(!flag(&s, "IsFlying()"));
    assert_eq!(condition(&s).as_deref(), Some("yes"));
}

/// Each side is its own bit, and the side-blind verbs read either.
#[test]
fn each_modifier_side_reads_its_own_key() {
    let mut s = UiScript::new().unwrap();
    let sided = [
        "IsLeftShiftKeyDown",
        "IsRightShiftKeyDown",
        "IsLeftControlKeyDown",
        "IsRightControlKeyDown",
        "IsLeftAltKeyDown",
        "IsRightAltKeyDown",
    ];
    for (i, name) in sided.iter().enumerate() {
        s.set_modifier_keys(1 << i);
        for (j, other) in sided.iter().enumerate() {
            assert_eq!(
                flag(&s, &format!("{other}()")),
                i == j,
                "{name} held, {other}"
            );
        }
        let blind = ["IsShiftKeyDown", "IsControlKeyDown", "IsAltKeyDown"][i / 2];
        assert!(flag(&s, &format!("{blind}()")), "{name} held, {blind}");
    }
    // With no side known, a held modifier is its left key.
    s.set_modifiers(false, true, false);
    assert!(flag(&s, "IsLeftControlKeyDown()"));
    assert!(!flag(&s, "IsRightControlKeyDown()"));
    assert!(flag(&s, "IsControlKeyDown()"));
}

#[test]
fn mouse_button_names_follow_the_reference_index() {
    let s = UiScript::new().unwrap();
    let name = |arg: &str| {
        s.eval::<Option<String>>(&format!("return GetMouseButtonName({arg})"))
            .unwrap()
    };
    let some = |n: &str| Some(n.to_string());
    assert_eq!(name("1"), some("LeftButton"));
    assert_eq!(name("2"), some("RightButton"));
    assert_eq!(name("3"), some("MiddleButton"));
    assert_eq!(name("4.9"), some("Button4"));
    assert_eq!(name("31"), some("Button31"));
    assert_eq!(name("0"), some("UNKNOWN"));
    assert_eq!(name("32"), some("UNKNOWN"));
    assert_eq!(name("'Button7'"), some("Button7"));
    assert_eq!(name("nil"), None);
    assert_eq!(name("{}"), None);
    assert_eq!(
        s.eval::<Option<String>>("return GetMouseButtonClicked()")
            .unwrap(),
        None
    );
}

/// The real group's leader, the one outside a battleground, against the player's own guid.
#[test]
fn the_real_leader_verbs_compare_the_real_groups_leader_with_the_player() {
    let mut s = UiScript::new().unwrap();
    let both = |s: &UiScript| {
        (
            flag(s, "IsRealPartyLeader()"),
            flag(s, "IsRealRaidLeader()"),
        )
    };
    assert_eq!(both(&s), (false, false));
    let mut party = PartyState {
        own_guid: 7,
        real_leader_guid: 7,
        members: vec![PartyMemberInfo {
            name: "Ally".into(),
            guid: 8,
            online: true,
        }],
        ..Default::default()
    };
    s.set_party(party.clone());
    assert_eq!(both(&s), (true, true));
    party.real_leader_guid = 8;
    s.set_party(party);
    assert_eq!(both(&s), (false, false));
}

#[test]
fn sanctuary_reads_the_held_units_bit_and_never_raises() {
    let mut s = UiScript::new().unwrap();
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            has_object: true,
            pvp_sanctuary: true,
            ..Default::default()
        }),
    );
    assert!(flag(&s, "UnitIsPVPSanctuary('target')"));
    assert!(!flag(&s, "UnitIsPVPSanctuary('focus')"));
    assert!(!flag(&s, "UnitIsPVPSanctuary()"));
}

/// Feign death shows only on the player's own group: a feigning stranger reads nil.
#[test]
fn feign_death_reads_the_dead_bit_on_the_players_group_alone() {
    let mut s = UiScript::new().unwrap();
    s.set_party(PartyState {
        own_guid: 1,
        members: vec![PartyMemberInfo {
            name: "Hunter".into(),
            guid: 2,
            online: true,
        }],
        ..Default::default()
    });
    let feigning = |guid: u64, owner: u64| UnitState {
        exists: true,
        has_object: true,
        guid,
        owner,
        dynamic_flags: 0x20,
        ..Default::default()
    };
    s.set_unit("party1", Some(feigning(2, 0)));
    s.set_unit("partypet1", Some(feigning(3, 2)));
    s.set_unit("target", Some(feigning(9, 0)));
    assert!(flag(&s, "UnitIsFeignDeath('party1')"));
    assert!(flag(&s, "UnitIsFeignDeath('partypet1')"));
    assert!(!flag(&s, "UnitIsFeignDeath('target')"));
    s.set_unit(
        "party1",
        Some(UnitState {
            dynamic_flags: 0,
            ..feigning(2, 0)
        }),
    );
    assert!(!flag(&s, "UnitIsFeignDeath('party1')"));
    assert!(s
        .run("UnitIsFeignDeath()")
        .unwrap_err()
        .to_string()
        .contains(r#"Usage: UnitIsFeignDeath("unit")"#));
}

#[test]
fn power_regen_is_the_own_power_types_regen_plus_a_thousandth() {
    let mut s = UiScript::new().unwrap();
    assert_eq!(
        s.eval::<(f64, f64)>("return GetPowerRegen()").unwrap(),
        (0.0, 0.0)
    );
    s.set_player_combat_stats(Some(UnitCombatStats {
        power_regen: [10.0, 2.5],
        ..Default::default()
    }));
    let (out, casting) = s.eval::<(f64, f64)>("return GetPowerRegen()").unwrap();
    assert!((out - 10.001).abs() < 1e-6 && (casting - 2.501).abs() < 1e-6);
}

/// A view holding a resurrect offer from `offerer`.
struct Offer(Option<String>);

impl LiveState for Offer {
    fn unit_flags(&self, _: u64) -> Option<u32> {
        None
    }
    fn unit_health(&self, _: u64) -> Option<u32> {
        None
    }
    fn active_player(&self) -> Option<u64> {
        None
    }
    fn has_control(&self) -> bool {
        true
    }
    fn far_sight(&self) -> bool {
        false
    }
    fn resurrect_offerer(&self) -> Option<String> {
        self.0.clone()
    }
}

#[test]
fn the_resurrect_offerer_is_the_cached_casters_name_or_nil() {
    let mut s = UiScript::new().unwrap();
    let ask = |s: &mut UiScript| {
        s.eval::<Option<String>>("return ResurrectGetOfferer()")
            .unwrap()
    };
    assert_eq!(ask(&mut s), None);
    assert_eq!(
        s.with_live(&mut Offer(Some("Healer".into())), ask),
        Some("Healer".into())
    );
    assert_eq!(s.with_live(&mut Offer(None), ask), None);
}
