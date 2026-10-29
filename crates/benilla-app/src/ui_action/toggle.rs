//! Press-again-to-cancel. The reference cancels in its dispatchers, above the cast attempt, and the
//! two toggles differ: the active-action one runs in both `UseAction` and the `CastSpell`
//! dispatcher, the form-match one in `CastSpell` only. Both cancel through the aura cancel, the
//! buff right-click's `CMSG_CANCEL_AURA`; its own guard (`AttributesEx & 0x2000` set, `& 0x4`
//! clear) never fires for a shipped toggle and is not built.

use benilla_formats::{ShapeshiftForm, SpellDisplay};

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
}
