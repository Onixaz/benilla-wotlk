//! The soulbind confirmations other than loot's: `EQUIP_BIND`, `AUTOEQUIP_BIND` and `USE_BIND`.
//! They are client-local: the client reads the cached template's bonding and defers its own send,
//! and an accept re-issues that action with a suppress flag, as 1.12 has no confirm packet. The
//! app owns the pending records; this module holds only the intents.

use mlua::Lua;

/// One answer to a pending-equip question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingEquipAnswer {
    /// The event's `arg1`, not a slot: a 0-based index into the reference's pending-equip array
    /// (stride `0x20`), kept untranslated because Lua only hands it back.
    pub index: u32,
    /// Accept re-issues the action; cancel sends nothing and only releases the item locks.
    pub accept: bool,
}

impl super::UiScript {
    /// The equip answers in call order. The app ignores an index it holds no record for, as the
    /// reference's bounds-checks it and returns.
    pub fn take_pending_equip_answers(&mut self) -> Vec<PendingEquipAnswer> {
        drain_field!(self, pending_equip_answers)
    }

    /// `ConfirmBindOnUse()` calls since the last drain.
    pub fn take_bind_on_use_confirms(&mut self) -> u32 {
        drain_field!(self, bind_on_use_confirms)
    }

    /// The client's item-usable predicate, which the app's equip deferral asks: an item the player
    /// cannot equip is never asked about. Entry `0` and an unanswered template read usable.
    pub fn item_usable(&self, item_id: u32) -> bool {
        super::item_stats::item_usable_by_id(&self.model_ref(), item_id)
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // The `OnAccept` of `EQUIP_BIND` and `AUTOEQUIP_BIND` (`StaticPopup.lua:612-647`).
    g.set(
        "EquipPendingItem",
        lua.create_function(|lua, index: u32| {
            let mut model = crate::script::model_mut_of(lua);
            model.pending_equip_answers.push(PendingEquipAnswer {
                index,
                accept: true,
            });
            Ok(())
        })?,
    )?;

    // Their `OnCancel` and also their `OnHide`: a second bind question hides the standing dialog
    // (`exclusive = 1`), and that cancel releases the superseded item's locks.
    g.set(
        "CancelPendingEquip",
        lua.create_function(|lua, index: u32| {
            let mut model = crate::script::model_mut_of(lua);
            model.pending_equip_answers.push(PendingEquipAnswer {
                index,
                accept: false,
            });
            Ok(())
        })?,
    )?;

    // `USE_BIND`'s `OnAccept`, with no argument and no cancel: the use arm's pending state is two
    // globals, and declining drops it.
    g.set(
        "ConfirmBindOnUse",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.bind_on_use_confirms += 1;
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// Order matters: a supersede is a cancel of one record arriving between two others.
    #[test]
    fn the_equip_answers_queue_in_call_order_with_their_verdicts() {
        let mut s = UiScript::new().unwrap();
        s.run("EquipPendingItem(0) CancelPendingEquip(1) EquipPendingItem(2)")
            .unwrap();
        let answers = s.take_pending_equip_answers();
        assert_eq!(
            answers
                .iter()
                .map(|a| (a.index, a.accept))
                .collect::<Vec<_>>(),
            vec![(0, true), (1, false), (2, true)]
        );
        assert!(s.take_pending_equip_answers().is_empty(), "drained");
    }

    #[test]
    fn confirm_bind_on_use_counts() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.take_bind_on_use_confirms(), 0);
        s.run("ConfirmBindOnUse() ConfirmBindOnUse()").unwrap();
        assert_eq!(s.take_bind_on_use_confirms(), 2);
        assert_eq!(s.take_bind_on_use_confirms(), 0, "drained");
    }
}
