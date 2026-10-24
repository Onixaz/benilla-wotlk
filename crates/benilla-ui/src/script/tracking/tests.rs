use super::*;
use crate::script::UiScript;

const HUNTER: u8 = 3;
const ROGUE: u8 = 4;
const MAGE: u8 = 8;

fn spell(spell_id: u32, name: &str, icon: &str) -> TrackingSpell {
    TrackingSpell {
        spell_id,
        name: name.into(),
        icon: Some(icon.into()),
        active_icon: None,
    }
}

/// A VM with the CVar, two town-type names and an event log.
fn script() -> UiScript {
    let s = UiScript::new().unwrap();
    s.register_cvars([("minimapTrackedInfo", "")]);
    s.run(
        r#"
        MINIMAP_TRACKING_REPAIR = "Repair"
        MINIMAP_TRACKING_MAILBOX = "Mailbox"
        MINIMAP_TRACKING_TOOLTIP_NONE = "Click to choose tracking"
        fired = 0
        local f = CreateFrame("Frame")
        f:RegisterEvent("MINIMAP_UPDATE_TRACKING")
        f:SetScript("OnEvent", function() fired = fired + 1 end)
    "#,
    )
    .unwrap();
    s
}

fn hunter() -> UiScript {
    let mut s = script();
    s.set_tracking_spells(
        vec![
            spell(1494, "Track Beasts", r"Interface\Icons\Ability_Tracking"),
            spell(
                2580,
                "Find Minerals",
                r"Interface\Icons\Spell_Nature_Earthquake",
            ),
        ],
        HUNTER,
    );
    s
}

fn fired(s: &UiScript) -> i64 {
    s.eval::<i64>("return fired").unwrap()
}

#[test]
fn the_class_mask_offers_poisons_to_rogues_and_stables_to_hunters() {
    let count = |class| {
        let mut s = script();
        s.set_tracking_spells(Vec::new(), class);
        s.eval::<i64>("return GetNumTrackingTypes()").unwrap()
    };
    assert_eq!(count(HUNTER), 14, "ammo and stables, no poisons");
    assert_eq!(count(ROGUE), 14, "poisons and ammo, no stables");
    assert_eq!(count(MAGE), 12, "the twelve classless rows");
    assert_eq!(
        hunter()
            .eval::<i64>("return GetNumTrackingTypes()")
            .unwrap(),
        16,
        "the spells come on top"
    );
}

#[test]
fn info_lists_the_spells_then_the_town_types() {
    let s = hunter();
    s.run(
        r##"
        local n, t, a, c = GetTrackingInfo(2)
        assert(n == "Find Minerals" and t == [[Interface\Icons\Spell_Nature_Earthquake]], n)
        assert(a == nil and c == "spell")
        n, t, a, c = GetTrackingInfo(3)
        assert(n == "Repair" and t == [[Interface\Minimap\Tracking\Repair]], n)
        assert(a == nil and c == "other")
        -- Rows 13 and 14 after the spells: the hunter's offered rows skip Poisons.
        n, t = GetTrackingInfo(2 + 13)
        assert(n == "Mailbox" and t == [[Interface\Minimap\Tracking\Mailbox]], tostring(n))
        assert(select("#", GetTrackingInfo(17)) == 0, "past the list")
        assert(select("#", GetTrackingInfo(0)) == 0)
    "##,
    )
    .unwrap();
}

#[test]
fn the_texture_is_the_town_type_else_the_spell_else_none() {
    let mut s = hunter();
    let tex = |s: &UiScript| s.eval::<String>("return GetTrackingTexture()").unwrap();
    assert_eq!(tex(&s), r"Interface\Minimap\Tracking\None");
    s.set_tracking_spell(2580);
    assert_eq!(tex(&s), r"Interface\Icons\Spell_Nature_Earthquake");
    s.run("SetTracking(3)").unwrap();
    assert_eq!(tex(&s), r"Interface\Minimap\Tracking\Repair");
}

#[test]
fn a_spell_row_casts_and_a_town_row_cancels_the_aura_and_saves_the_type() {
    let mut s = hunter();
    s.run("SetTracking(1)").unwrap();
    assert_eq!(
        s.take_script_calls(),
        vec![ScriptCall::CastSpell {
            spell_id: 1494,
            at: CastAt::Selection,
            via_book: false,
        }]
    );
    assert_eq!(
        fired(&s),
        0,
        "the cast's aura moves the state, not the call"
    );

    // The aura lands: the tracking spell, and the event.
    s.set_tracking_spell(1494);
    assert_eq!(fired(&s), 1);
    assert!(s
        .eval::<bool>("local _, _, a = GetTrackingInfo(1) return a == 1")
        .unwrap());

    s.run("SetTracking(3)").unwrap();
    assert_eq!(s.take_cancel_aura_requests(), vec![1494]);
    assert_eq!(
        s.cvar("minimapTrackedInfo").unwrap(),
        "MINIMAP_TRACKING_REPAIR"
    );
    assert_eq!(fired(&s), 2);
    assert_eq!(s.minimap_town_tracking().map(|t| t.file), Some("Repair"));
    assert!(s
        .eval::<bool>("local _, _, a = GetTrackingInfo(3) return a == 1")
        .unwrap());
}

#[test]
fn no_argument_cancels_the_spell_and_clears_the_type() {
    let mut s = hunter();
    s.set_tracking_spell(2580);
    s.run("SetTracking(4)").unwrap();
    assert_eq!(s.take_cancel_aura_requests(), vec![2580]);
    // The removal arrives: the spell clears, the town type stays.
    s.set_tracking_spell(0);
    assert!(s.minimap_town_tracking().is_some());
    s.run("SetTracking()").unwrap();
    assert!(s.take_cancel_aura_requests().is_empty(), "no spell left");
    assert!(s.minimap_town_tracking().is_none());
    assert_eq!(s.cvar("minimapTrackedInfo").unwrap(), "");
}

#[test]
fn a_new_tracking_aura_clears_the_town_type_and_an_unknown_one_is_ignored() {
    let mut s = hunter();
    s.run("SetTracking(3)").unwrap();
    s.set_tracking_spell(5225);
    assert!(
        s.minimap_town_tracking().is_some(),
        "not a known tracking spell"
    );
    assert_eq!(s.tracking_spell(), 0);
    s.set_tracking_spell(2580);
    assert!(s.minimap_town_tracking().is_none());
    assert_eq!(s.cvar("minimapTrackedInfo").unwrap(), "");
}

#[test]
fn the_saved_type_restores_only_for_a_class_offered_it() {
    let mut s = script();
    s.run(r#"SetCVar("minimapTrackedInfo", "MINIMAP_TRACKING_VENDOR_POISON")"#)
        .unwrap();
    s.load_tracked_state(MAGE);
    assert!(s.minimap_town_tracking().is_none());

    let mut s = script();
    s.run(r#"SetCVar("minimapTrackedInfo", "MINIMAP_TRACKING_VENDOR_POISON")"#)
        .unwrap();
    s.load_tracked_state(ROGUE);
    assert_eq!(s.minimap_town_tracking().map(|t| t.file), Some("Poisons"));
    assert_eq!(fired(&s), 1);
}

#[test]
fn the_tooltip_names_the_town_type_or_none() {
    let s = hunter();
    s.run(
        r#"
        local tt = CreateFrame("GameTooltip", "TT")
        tt:SetOwner(UIParent or CreateFrame("Frame"), "ANCHOR_NONE")
        tt:SetTracking()
        assert(tt:IsShown() and tt:NumLines() == 1)
        assert(TTTextLeft1:GetText() == "Click to choose tracking", TTTextLeft1:GetText())
        SetTracking(3)
        tt:SetTracking()
        assert(TTTextLeft1:GetText() == "Repair", TTTextLeft1:GetText())
    "#,
    )
    .unwrap();
}

#[test]
fn a_form_with_nothing_tracked_casts_its_first_allowed_tracking_spell() {
    let mut s = hunter();
    s.track_in_form(|id| id == 2580);
    assert_eq!(
        s.take_script_calls(),
        vec![ScriptCall::CastSpell {
            spell_id: 2580,
            at: CastAt::Selection,
            via_book: false,
        }]
    );
    s.track_in_form(|_| false);
    assert!(s.take_script_calls().is_empty(), "no spell the form allows");
    s.set_tracking_spell(1494);
    s.track_in_form(|_| true);
    assert!(s.take_script_calls().is_empty(), "something is tracked");
    s.set_tracking_spell(0);
    s.run("SetTracking(3)").unwrap();
    s.track_in_form(|_| true);
    assert!(s.take_script_calls().is_empty(), "a town type counts");
}
