//! The script calls whose effects touch the selection, the player's cast or the targeting cursor,
//! held in one queue in the order the script made them. In the reference each such call is done
//! when it returns: the macro runner fires every line in one pass, `TargetUnit`, `TargetByName` and
//! `ClearTarget` commit through `SetSelection` before returning, and `CastSpellByName` casts at the
//! selection as it stands then. So `/target Bob` then `/cast Flash Heal` heals Bob, and the app
//! applies this queue front to back at one point in the frame.

use super::{ActionUse, SelectionRequest, UiScript};

/// One queued call, in the reference's terms.
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptCall {
    /// `TargetUnit`, `AssistUnit`, `AssistByName` and `TargetLastEnemy`.
    Select(SelectionRequest),
    /// `TargetByName(name, exactMatch)`, the stock `/target`.
    TargetByName { name: String, exact: bool },
    /// `TargetNearestEnemy`, `TargetNearestFriend`, `TargetNearestPartyMember` and
    /// `TargetNearestRaidMember`: one TAB cycle, `(reverse, mode)`.
    TargetNearest { mode: NearestMode, reverse: bool },
    /// `TargetLastTarget()`: re-select the last-target pair, or deselect when it is empty.
    TargetLastTarget,
    /// `ClearTarget()`, a no-op when nothing is selected by then.
    ClearTarget,
    /// `AttackTarget()`: the attack validator's pick from the selection as it stands then, which
    /// may move it to the nearest enemy, then the toggle.
    AttackTarget,
    /// `SpellTargetUnit(unit)`, made while the cursor was up.
    SpellTargetUnit(String),
    /// `PlaceGlyphInSocket(index)` (`0x5b75a0`) with a targeting cast: the 0-based socket and
    /// `Spell_C_HandleGlyphClick`'s verdict on it.
    PlaceGlyph {
        socket: u32,
        verdict: super::GlyphPlacement,
    },
    /// `SpellStopTargeting()`, made while the cursor was up.
    SpellStopTargeting,
    /// `SpellStopCasting()`, made with something to stop.
    SpellStopCasting,
    /// `CastSpell(index, "spell" [, onSelf])` (`0x541250`), `CastSpellByName(name [, unit])`
    /// (`0x540310`) and `CastSpellByID(id [, unit])` (`0x53e060`), aimed at `at`. `via_book`: the
    /// book's dispatcher `0x53bc40`, whose cancel forks a re-press runs into; a `!name` and
    /// `CastSpellByID` cast through `0x80da40` past them (`0x5403cc`, `0x53e177`).
    CastSpell {
        spell_id: u32,
        at: CastAt,
        via_book: bool,
    },
    /// `CastSpell(id, "pet", onSelf)`, the reference's `CMSG_PET_ACTION` fork, with the same guid
    /// swap.
    CastPetSpell { spell_id: u32, on_self: bool },
    /// `CastShapeshiftForm(index)`, as the form's spell.
    CastShapeshiftForm(u32),
    /// `UseAction(slot [, unit [, button]])` (`0x5ac000` → `0x5abbc0`); a macro slot runs its
    /// lines inside it.
    UseAction(ActionUse),
    /// `UseContainerItem(bag, slot)` outside repair mode.
    UseContainerItem { bag: i64, slot: u32 },
    /// `UseQuestLogSpecialItem(index)`: the bag item's own use (`CGItem_C::UseItem` `0x708c20`), past
    /// the right-click's merchant, bank and equip arms.
    UseItem { bag: i64, slot: u32 },
    /// `UseInventoryItem(id)` outside repair mode.
    UseInventoryItem(u32),
    /// `CastPetAction(slot)`, a 1-based pet bar slot.
    PetAction(u32),
    /// `PetAttack` and the other one-shot orders, as their bar slot's packed word.
    PetOrder(u32),
}

/// The unit a cast verb aims at, the guid its target argument leaves for the cast
/// (`0x80da40`'s guid pair).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum CastAt {
    /// No target: the selection (`[0xbd07b0]`), what an absent or empty unit argument reads
    /// (`0x60b029`) and what `CastSpell` passes without `onSelf` (`0x541279`).
    #[default]
    Selection,
    /// The active player: `CastSpell`'s `onSelf` (`0x541296`).
    Player,
    /// A token the resolver knows, resolved where the call takes effect, after the calls before
    /// it: `/target Bob` then `/cast [target=target] Heal` heals Bob. One naming nobody leaves
    /// guid `0xffffffff_fffffffe` (`0x60b00c`), which no object answers, so the cast binds no unit.
    Unit(String),
}

/// A cast verb's unit argument as `0x60abf0` reads it, `lua_tolstring` first (`0x84e0e0`): a
/// value with no string form, an absent one or `""` is the selection (`0x60b029`). `None` is a
/// token the resolver does not know, where the verb casts nothing (`0x54037b`, `0x5ac043`,
/// `0x53e0cb`).
pub(crate) fn cast_at_arg(lua: &mlua::Lua, v: Option<&mlua::Value>) -> Option<CastAt> {
    let token = match v {
        Some(v @ (mlua::Value::String(_) | mlua::Value::Integer(_) | mlua::Value::Number(_))) => {
            lua.coerce_string(v.clone())
                .ok()
                .flatten()
                .map(|s| s.to_string_lossy())
                .unwrap_or_default()
        }
        _ => String::new(),
    };
    if token.is_empty() {
        return Some(CastAt::Selection);
    }
    match super::parse_unit_token(&token) {
        super::UnitTokenParse::Unknown => None,
        _ => Some(CastAt::Unit(token)),
    }
}

/// The mode the four `TargetNearest*` shims hand the TAB cycler, the only byte that differs between
/// them; it reaches only the per-candidate filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NearestMode {
    /// Mode 1, `TargetNearestEnemy`.
    Enemy,
    /// Mode 2, `TargetNearestFriend`.
    Friend,
    /// Mode 3, `TargetNearestPartyMember`: a party member other than us.
    PartyMember,
    /// Mode 4, `TargetNearestRaidMember`: a party or raid member other than us.
    RaidMember,
}

impl UiScript {
    /// Take every queued call, in call order.
    pub fn take_script_calls(&mut self) -> Vec<ScriptCall> {
        drain_field!(self, script_calls)
    }

    /// Put calls back at the front of the queue, ahead of any queued since.
    pub fn requeue_script_calls(&mut self, calls: Vec<ScriptCall>) {
        let mut model = self.model_mut();
        let later = std::mem::replace(&mut model.script_calls, calls);
        model.script_calls.extend(later);
    }

    /// Take the queued calls `pick` answers for, in call order, leaving the rest queued.
    pub(crate) fn take_calls_where<T>(
        &mut self,
        mut pick: impl FnMut(&ScriptCall) -> Option<T>,
    ) -> Vec<T> {
        let mut model = self.model_mut();
        let mut taken = Vec::new();
        model.script_calls.retain(|call| match pick(call) {
            Some(t) => {
                taken.push(t);
                false
            }
            None => true,
        });
        taken
    }
}
