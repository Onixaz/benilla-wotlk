//! Press-again-to-cancel. The reference cancels in its dispatchers, above the cast attempt, and the
//! two toggles differ: the active-action one runs in both `UseAction` and the `CastSpell`
//! dispatcher, the form-match one in `CastSpell` only. `UseAction` is `ClickSpell` (`0x80da80`):
//! it uses [`is_active_spell`], not only the `ActiveIconID` predicate, so a mount with no active
//! icon still dismounts. Both cancel through the aura cancel, the buff right-click's
//! `CMSG_CANCEL_AURA`; its own guard (`AttributesEx & 0x2000` set, `& 0x4` clear) never fires for
//! a shipped toggle and is not built.

use benilla_formats::{ShapeshiftForm, SpellCatalog, SpellDisplay, SPELL_AURA_MOUNTED};

use crate::unit_auras::UnitAuras;

/// The active-action predicate: a nonzero raw `ActiveIconID` and the spell's own aura live with the
/// cancelable bit, so the press cancels instead of casting. No form guard: a warrior stance has
/// `ActiveIconID` 0. A unit with no aura table holds none.
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

/// The click-spell active test (`0x802cb0`): a nonzero `ActiveIconID` or a mount (`WillSummonCritter`
/// is aura 0 `MOUNTED`), then a cancelable aura of this spell, else any `MOUNTED` aura riding the
/// same creature. A critter's slot-5 arm is the companion pane's.
pub(crate) fn is_active_spell(
    spell_id: u32,
    d: &SpellDisplay,
    auras: Option<&UnitAuras>,
    catalog: &SpellCatalog,
) -> bool {
    if d.active_icon_id == 0 && d.effect_apply_aura[0] != SPELL_AURA_MOUNTED {
        return false;
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

/// The aura `ClickSpell` cancels (`0x802f80`): another `MOUNTED` aura of the same creature while
/// mounted (`0x8030bc`), else the clicked spell.
pub(crate) fn click_cancel_aura(
    spell_id: u32,
    d: &SpellDisplay,
    auras: Option<&UnitAuras>,
    catalog: &SpellCatalog,
    mount_display: u32,
) -> u32 {
    if mount_display > 0 && d.effect_apply_aura.contains(&SPELL_AURA_MOUNTED) {
        if let Some(other) = auras.and_then(|t| {
            t.iter().find_map(|a| {
                let m = catalog.get(a.spell_id)?;
                (m.effect_apply_aura[0] == SPELL_AURA_MOUNTED
                    && m.effect_misc_value[0] == d.effect_misc_value[0]
                    && a.spell_id != spell_id)
                    .then_some(a.spell_id)
            })
        }) {
            return other;
        }
    }
    spell_id
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
        let riding = UnitAuras::of(1, &[(0, 32242, 1)]);
        assert!(is_active_spell(32242, &gryphon, Some(&riding), &catalog));
        assert!(!is_active_spell(32242, &gryphon, None, &catalog));
        assert!(
            !active_action_toggle(32242, &gryphon, Some(&riding)),
            "the icon-swap predicate does not cover a mount"
        );
        assert_eq!(
            click_cancel_aura(32242, &gryphon, Some(&riding), &catalog, 27655),
            32242
        );
    }

    /// A mount ridden through another spell of the same creature cancels that spell (`0x8030bc`).
    #[test]
    fn a_mount_ridden_through_another_spell_cancels_that_one() {
        let book = mount(800);
        let catalog = SpellCatalog::from_displays([(20, mount(800)), (22, mount(800))].into());
        let riding = UnitAuras::of(1, &[(0, 22, 0)]);
        assert!(is_active_spell(20, &book, Some(&riding), &catalog));
        assert_eq!(
            click_cancel_aura(20, &book, Some(&riding), &catalog, 1234),
            22
        );
        assert_eq!(
            click_cancel_aura(20, &book, Some(&riding), &catalog, 0),
            20,
            "dismounted, the clicked spell is the cancel"
        );
    }
}
