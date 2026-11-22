//! Press-again-to-cancel. The reference cancels in its dispatchers, above the cast attempt, and the
//! two toggles differ: the click path's active-spell test (`0x80da80`, here [`is_active_spell`])
//! runs in `UseAction`, the `CastSpell` dispatcher and item use, the form-match one in `CastSpell`
//! only. Both cancel through the aura cancel (`0x802f80`), the buff right-click's
//! `CMSG_CANCEL_AURA`; its own guard (`AttributesEx & 0x2000` set, `& 0x4` clear) never fires for
//! a shipped toggle and is not built.

use benilla_formats::{
    CompanionKind, ShapeshiftForm, SpellCatalog, SpellDisplay, SummonPropertiesCatalog,
    SPELL_AURA_MOUNTED,
};

use crate::unit_auras::UnitAuras;

/// `SPELL_EFFECT_SUMMON` (`0x802d20`).
const SPELL_EFFECT_SUMMON: u32 = 0x1c;

/// The active-action predicate: a nonzero raw `ActiveIconID` and the spell's own aura live with the cancelable bit, so the press cancels instead of casting. A unit with no aura table holds none.
pub(crate) fn active_action_toggle(
    spell_id: u32,
    d: &SpellDisplay,
    auras: Option<&UnitAuras>,
) -> bool {
    d.active_icon_id != 0
        && auras.is_some_and(|t| {
            t.iter()
                .any(|a| a.spell_id == spell_id && a.is_cancelable())
        })
}

/// The click-spell active test (`0x802cb0`). `own_vehicle` is the entry of the vehicle the unit is seated in (`[unit+0xf60]` -> `0x747ff0`) when the active player created it.
pub(crate) fn is_active_spell(
    spell_id: u32,
    d: &SpellDisplay,
    auras: Option<&UnitAuras>,
    catalog: &SpellCatalog,
    summons: &SummonPropertiesCatalog,
    critter: Option<u64>,
    own_vehicle: Option<u32>,
) -> bool {
    let kind = d.companion_kind(summons);
    if d.active_icon_id == 0 && kind.is_none() {
        return false;
    }
    // A summon of our own vehicle's creature, ahead of the effect's slot-5 arm (`0x802d20`-`0x802d4e`, active at `0x802dc3`).
    if let Some(entry) = own_vehicle {
        let summons_it = (0..3).any(|i| {
            d.effects[i] == SPELL_EFFECT_SUMMON
                && u32::try_from(d.effect_misc_value[i]).ok() == Some(entry)
        });
        if summons_it {
            return true;
        }
    }
    if kind == Some(CompanionKind::Critter) {
        return critter.map(crate::ui_companion::critter_entry) == Some(d.companion_creature());
    }
    let Some(auras) = auras else {
        return false;
    };
    if auras
        .iter()
        .any(|a| a.spell_id == spell_id && a.is_cancelable())
    {
        return true;
    }
    d.effect_apply_aura[0] == SPELL_AURA_MOUNTED
        && auras.iter().any(|a| {
            catalog.get(a.spell_id).is_some_and(|m| {
                m.effect_apply_aura[0] == SPELL_AURA_MOUNTED
                    && m.effect_misc_value[0] == d.effect_misc_value[0]
            })
        })
}

/// The form-match recast: `None` casts (not the active form), `Some(true)` cancels the form,
/// `Some(false)` is a silent no-op when `SpellShapeshiftForm.flags1 & 0x2` is set (twin of the
/// stance bar's).
pub(crate) fn form_recast_disposition(
    d: &SpellDisplay,
    form_byte: u8,
    form_row: Option<&ShapeshiftForm>,
) -> Option<bool> {
    if form_byte == 0 || d.shapeshift_form != Some(u32::from(form_byte)) {
        return None;
    }
    Some(form_row.is_some_and(|r| r.cancelable()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::AURA_UPDATE_FLAG_CANCELABLE;

    #[test]
    fn active_action_toggle_needs_the_icon_and_the_live_cancelable_aura() {
        let ghost_wolf = SpellDisplay {
            active_icon_id: 1,
            ..Default::default()
        };
        let plain = SpellDisplay::default();
        let shifted = UnitAuras::of(1, &[(0, 2645, AURA_UPDATE_FLAG_CANCELABLE | 1)]);
        assert!(active_action_toggle(2645, &ghost_wolf, Some(&shifted)));
        assert!(!active_action_toggle(2645, &plain, Some(&shifted)));
        let uncancelable = UnitAuras::of(1, &[(0, 2645, 1)]);
        assert!(!active_action_toggle(
            2645,
            &ghost_wolf,
            Some(&uncancelable)
        ));
        assert!(!active_action_toggle(768, &ghost_wolf, Some(&shifted)));
        assert!(!active_action_toggle(2645, &ghost_wolf, None));
    }

    #[test]
    fn form_recast_forks_on_the_active_form_and_the_cancelable_guard() {
        let ghost_wolf = SpellDisplay {
            shapeshift_form: Some(16),
            ..Default::default()
        };
        let wolf_row = ShapeshiftForm {
            flags: 0x40,
            ..Default::default()
        };
        let stance_row = ShapeshiftForm {
            flags: 0x7,
            ..Default::default()
        };
        assert_eq!(
            form_recast_disposition(&ghost_wolf, 16, Some(&wolf_row)),
            Some(true)
        );
        assert_eq!(
            form_recast_disposition(&ghost_wolf, 16, Some(&stance_row)),
            Some(false),
            "the non-cancelable guard is a silent no-op, never a cast"
        );
        assert_eq!(
            form_recast_disposition(&ghost_wolf, 0, Some(&wolf_row)),
            None
        );
        assert_eq!(
            form_recast_disposition(&ghost_wolf, 1, Some(&wolf_row)),
            None
        );
        let plain = SpellDisplay::default();
        assert_eq!(form_recast_disposition(&plain, 16, Some(&wolf_row)), None);
    }

    fn mount(creature: i32) -> SpellDisplay {
        SpellDisplay {
            effect_apply_aura: [SPELL_AURA_MOUNTED, 0, 0],
            effect_misc_value: [creature, 0, 0],
            ..Default::default()
        }
    }

    /// A flying mount has no `ActiveIconID` and its aura need not carry the cancelable bit:
    /// `WillSummonCritter` still opens the walk, and any `MOUNTED` aura of that creature is active.
    #[test]
    fn a_mount_with_no_active_icon_is_still_an_active_spell() {
        let gryphon = mount(27655);
        let catalog = SpellCatalog::from_displays([(32242, mount(27655))].into());
        let none = SummonPropertiesCatalog::default();
        let riding = UnitAuras::of(1, &[(0, 32242, 1)]);
        let active = |auras| is_active_spell(32242, &gryphon, auras, &catalog, &none, None, None);
        assert!(active(Some(&riding)));
        assert!(!active(None));
        assert!(
            !active_action_toggle(32242, &gryphon, Some(&riding)),
            "the icon-swap predicate does not cover a mount"
        );
    }

    /// A mount ridden through another spell of the same creature counts as active.
    #[test]
    fn a_mount_ridden_through_another_spell_is_active() {
        let book = mount(800);
        let catalog = SpellCatalog::from_displays([(20, mount(800)), (22, mount(800))].into());
        let none = SummonPropertiesCatalog::default();
        let riding = UnitAuras::of(1, &[(0, 22, 0)]);
        assert!(is_active_spell(
            20,
            &book,
            Some(&riding),
            &catalog,
            &none,
            None,
            None
        ));
    }

    /// A slot-5 summon is active while its own creature is the critter out (`0x802dcc`).
    #[test]
    fn a_critter_spell_is_active_while_its_creature_is_out() {
        let critter = SpellDisplay {
            effects: [28, 0, 0],
            effect_misc_value: [7555, 0, 0],
            effect_misc_value_b: [41, 0, 0],
            ..Default::default()
        };
        let catalog = SpellCatalog::from_displays([(10673, critter)].into());
        let critter = catalog.get(10673).unwrap();
        let slots = SummonPropertiesCatalog::from_slots([(41, 5)]);
        let guid = |entry: u64| 0xF130_0000_0000_0000 | (entry << 24) | 7;
        let active = |out| is_active_spell(10673, critter, None, &catalog, &slots, out, None);
        assert!(active(Some(guid(7555))));
        assert!(!active(Some(guid(7556))), "another critter is out");
        assert!(!active(None), "nothing is out");
    }

    /// A summon of the vehicle we sit in and created is active (`0x802d29`-`0x802d4e`), so a second press cancels; any other vehicle is not.
    #[test]
    fn a_summon_of_our_own_vehicle_is_active() {
        const SUIT: u32 = 54903;
        const CREATURE: i32 = 29645;
        let suit = || SpellDisplay {
            active_icon_id: 122,
            effects: [SPELL_EFFECT_SUMMON, 0, 0],
            effect_misc_value: [CREATURE, 0, 0],
            ..Default::default()
        };
        let catalog = SpellCatalog::from_displays([(SUIT, suit())].into());
        let suit = suit();
        let none = SummonPropertiesCatalog::default();
        let active = |vehicle| is_active_spell(SUIT, &suit, None, &catalog, &none, None, vehicle);
        assert!(
            active(Some(CREATURE as u32)),
            "our own vehicle of that creature"
        );
        assert!(!active(Some(29646)), "another creature's vehicle");
        assert!(!active(None), "no vehicle, or one someone else created");
    }
}
