//! The script calls whose effects touch the selection, the player's cast or the targeting cursor.
//! In the reference each such call is done when it returns: the macro runner fires every line in
//! one pass, `TargetUnit`, `TargetByName` and `ClearTarget` commit through the selection set before
//! returning, and `CastSpellByName` casts at the selection as it stands then. So a call goes
//! through [`make`], which applies it inside the Lua call through the host's applier
//! ([`set_apply`]) wherever the host's state is lent ([`super::world_loan`]); with none lent it
//! waits in one queue, in call order.

use super::{world_loan, ActionUse, SelectionRequest, UiScript};

/// The host's applier: one call applied with the lent state, inside the Lua call that made it;
/// `false` when the host cannot apply it now, and it waits.
pub type ApplyCall = fn(&mlua::Lua, ScriptCall) -> bool;

/// The host's applier, kept in the VM's app data.
struct HostApply(ApplyCall);

/// Name the host's applier of every script call `lua` makes.
pub fn set_apply(lua: &mlua::Lua, apply: ApplyCall) {
    lua.set_app_data(HostApply(apply));
}

/// Make `call`: applied before this returns where the host's state is lent, after the calls still
/// queued; with none lent it waits in the queue. The caller holds no model borrow.
pub(crate) fn make(lua: &mlua::Lua, call: ScriptCall) {
    super::model_mut_of(lua).script_calls.push(call);
    apply_queued(lua);
}

/// Apply the queued calls front to back through the host's applier; with nothing lent, they wait.
/// A call a listener makes while one applies is applied inside that listener, nested, as 12340
/// re-enters its cast entry from a `UNIT_SPELLCAST_SENT` listener (`0x80b556`).
pub fn apply_queued(lua: &mlua::Lua) {
    let Some(apply) = lua.app_data_ref::<HostApply>().map(|a| a.0) else {
        return;
    };
    if !world_loan::lent() {
        return;
    }
    loop {
        let call = {
            let mut model = super::model_mut_of(lua);
            if model.script_calls.is_empty() {
                break;
            }
            model.script_calls.remove(0)
        };
        if !apply(lua, call.clone()) {
            super::model_mut_of(lua).script_calls.insert(0, call);
            break;
        }
    }
}

/// A verb's step the host decides over its lent state, inside the call that reached it.
#[derive(Clone, Debug, PartialEq)]
pub enum HostAsk {
    /// A guild bank swap: the bag-to-bank, bank-to-bank and bank-to-bag gates and their send (`0x5a50c0`, `0x5a5410`, `0x6db880`, `0x5a63f0`).
    GuildBank(super::GuildBankRequest),
    /// A swap that asks its bind question: the pending equip is filed (`0x6dface`).
    BindSwap(super::ContainerMove),
    /// `AutoEquipCursorItem`'s bind question for the item at `(bag, slot)`: the pending equip is filed (`0x6dfdc8`).
    BindAutoEquip { bag: i64, slot: u32 },
    /// `EquipPendingItem`, `CancelPendingEquip` and `ConfirmBindOnUse`: the answers given so far run now (`0x6e01a0`, `0x7093f0`).
    BindAnswers,
}

/// What the host answered a [`HostAsk`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostAnswer {
    GuildBank(GuildBankVerdict),
    /// The pending equip's index, the bind question's argument.
    BindIndex(u32),
    /// The step ran.
    Done,
}

/// What a guild bank swap's gates left for the verb to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuildBankVerdict {
    /// A gate returned before anything: no banker, no item, no record.
    Nothing,
    /// The swap went out: the cursor clears and `GUILDBANKBAGSLOTS_CHANGED` fires (`0x5a53d3`, `0x5a53df`; `0x5a5688`, `0x5a5694`).
    Sent,
    /// The auto-store went out: `GUILDBANKBAGSLOTS_CHANGED` fires (`0x5a65c5`).
    Stored,
    /// A gate refused with its line, the cursor as it was.
    Refused(&'static str),
    /// An unresolved item back on its own slot: the cursor clears and nothing goes out (`0x5a55fe`).
    DropHeld,
}

/// The host's answerer of a [`HostAsk`]; `None` when it cannot answer now.
pub type AnswerHost = fn(&mlua::Lua, &HostAsk) -> Option<HostAnswer>;

/// The host's answerer, kept in the VM's app data.
struct HostAnswerer(AnswerHost);

/// Name the host's answerer of every [`HostAsk`] `lua` makes.
pub fn set_answer(lua: &mlua::Lua, answer: AnswerHost) {
    lua.set_app_data(HostAnswerer(answer));
}

/// Ask the host inside the call, where its state is lent; `None` with none lent, and the verb's request waits for the host's drain. The caller holds no model borrow.
pub(crate) fn ask_host(lua: &mlua::Lua, ask: &HostAsk) -> Option<HostAnswer> {
    let answer = lua.app_data_ref::<HostAnswerer>().map(|a| a.0)?;
    if !world_loan::lent() {
        return None;
    }
    answer(lua, ask)
}

/// Whether no earlier call waits once the queued ones applied through the lent hook: a verb that
/// runs inside its call does so only then, so it keeps its turn behind a waiting call.
pub(crate) fn none_waiting(lua: &mlua::Lua) -> bool {
    apply_queued(lua);
    super::model_mut_of(lua).script_calls.is_empty()
}

impl UiScript {
    /// Another handle on this VM (the same Lua state, counter and session), for a host that lends
    /// its state into a call while the VM itself stays where it lives.
    pub fn share(&self) -> UiScript {
        UiScript {
            lua: self.lua.clone(),
            instructions: self.instructions.clone(),
            session: self.session,
        }
    }

    /// The host over `lua` for an apply inside a Lua call: the same VM, sharing its counters.
    pub fn in_call(lua: &mlua::Lua) -> UiScript {
        let ids = lua
            .app_data_ref::<super::HostIds>()
            .expect("host ids app_data");
        UiScript {
            lua: lua.clone(),
            instructions: ids.instructions.clone(),
            session: ids.session,
        }
    }
}

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
    /// `StartAttack([unit [, exact]])` (`0x523090`): `unit` names the target outright, skipping the
    /// nearest-enemy fallback; nothing at all while already attacking.
    StartAttack { unit: Option<String>, exact: bool },
    /// `StopAttack()` (`0x51d0b0`).
    StopAttack,
    /// `Dismount()` (`0x51d170`).
    Dismount,
    /// `CancelShapeshiftForm()` (`0x53de60`).
    CancelShapeshiftForm,
    /// `AttackTarget()`: the attack validator's pick from the selection as it stands then, which
    /// may move it to the nearest enemy, then the toggle.
    AttackTarget,
    /// `SpellTargetUnit(unit)`, made while the cursor was up.
    SpellTargetUnit(String),
    /// `PlaceGlyphInSocket(index)` (`0x5b75a0`) with a targeting cast: the 0-based socket and
    /// the glyph click's verdict on it.
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
    /// `DestroyTotem(slot)` behind a queued call, the 0-based slot (`0x5197d0`).
    DestroyTotem(u8),
    /// `UseContainerItem(bag, slot)` outside repair mode.
    UseContainerItem { bag: i64, slot: u32 },
    /// A questgiver GameObject's use from `DeclineQuest`, its right-click (`0x58ceb2` -> `0x711140`).
    InteractObject(u64),
    /// `UseQuestLogSpecialItem(index)`: the bag item's own use (`0x708c20`), past the right-click's
    /// merchant, bank and equip arms.
    UseItem { bag: i64, slot: u32 },
    /// `UseItemByName(item [, unit])` (`0x51cd00`): the found item's own use, at `at`; `bag` is
    /// [`super::cursor::EQUIPMENT_BAG`] for a worn item.
    UseItemAt { bag: i64, slot: u32, at: CastAt },
    /// `EquipItemByName(item [, slot])` (`0x51cdb0`): to wire slot `dst`, else where it fits.
    EquipItem {
        bag: i64,
        slot: u32,
        dst: Option<u8>,
    },
    /// `UseInventoryItem(id)` outside repair mode.
    UseInventoryItem(u32),
    /// `CastPetAction(slot)`, a 1-based pet bar slot.
    PetAction(u32),
    /// `PetAttack` and the other one-shot orders, as their bar slot's packed word.
    PetOrder(u32),
    /// A verb whose press the host runs inside the call, off the intents it queued.
    Press(VerbPress),
    /// A loot verb, run by the host inside the call.
    Loot(LootVerb),
}

/// A loot verb the host runs inside its call, its slots 1-based display rows as Lua passes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LootVerb {
    /// `LootSlot(slot)`: the take dispatcher (`0x589520` -> `0x589140`).
    Slot(u32),
    /// `ConfirmLootSlot(slot)`: the pending bind confirm's take (`0x589590` -> `0x589300`).
    Confirm(u32),
    /// `GiveMasterLoot(slot, index)`: `index` the 1-based candidate the binding resolves (`0x589676`), then the send (`0x5893a0`).
    MasterGive { slot: u32, candidate: u32 },
    /// `CloseLoot([noUI])`: the close chain with `(1, 0, noUI)` (`0x5888b0` -> `0x523640`).
    Close { no_ui: bool },
    /// A `RollOnLoot` or `ConfirmLootRoll` vote past its confirm and its vote mask: the send, then
    /// the frame's close and the roll's finish (`0x6fbd43`-`0x6fbdd5`); `roll_type` the byte sent.
    Roll { roll_id: u32, roll_type: u8 },
}

/// The verbs that press inside their call off a queued intent: each casts or sends before it returns, so the caller's next line reads what the press armed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerbPress {
    /// `DoTradeSkill(index [, repeat])` (`0x5dac00`): the cast (`0x5dacb1` -> `0x80da40`), then the repeat set on success alone (`0x5dacbb`, `0x5dacc1` -> `0x5d9fa0`).
    TradeSkill,
    /// `DropItemOnUnit(unit)` (`0x51bdd0`): the pet's feed (`0x51bf26` -> `0x80dcf0`).
    DropItemOnUnit,
    /// `RepopMe`, `RetrieveCorpse`, `AcceptResurrect`, `DeclineResurrect`, `AcceptXPLoss` and `UseSoulstone`, each sending or using its item inside the call (`0x51aab3`, `0x51ae6d`).
    Death,
    /// An item pick of the targeting cursor (`SpellTargetItem` `0x8008a5` -> `0x5210d0`), the enchant confirms (`0x522f7f`, `0x5167c8`) and the trade slot's (`0x80c5f0`).
    ItemPicks,
}

/// Press `verb` inside this call where the host's state is lent (else it waits, in call order). The caller holds no model borrow.
pub(crate) fn press(lua: &mlua::Lua, verb: VerbPress) {
    make(lua, ScriptCall::Press(verb));
}

/// [`press`] the item picks when a pick, an enchant confirm or a trade slot's is queued.
pub(crate) fn press_item_picks(lua: &mlua::Lua) {
    let queued = {
        let model = super::model_mut_of(lua);
        !model.item_picks.is_empty()
            || !model.enchant_confirms.is_empty()
            || !model.trade_enchant_targets.is_empty()
    };
    if queued {
        press(lua, VerbPress::ItemPicks);
    }
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
    /// A token the resolver answers, resolved where the call takes effect, after the calls before
    /// it: `/target Bob` then `/cast [target=target] Heal` heals Bob. One naming nobody leaves
    /// guid `0xffffffff_fffffffe` (`0x60b00c`), which no object answers, so the cast binds no unit.
    Unit(String),
}

/// A cast verb's unit argument as `0x60abf0` reads it, `lua_tolstring` first (`0x84e0e0`): a
/// value with no string form, an absent one or `""` is the selection (`0x60b029`). `None` is a
/// token no base reads and no name lookup finds, where the verb casts nothing (`0x54037b`, `0x5ac043`,
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
    let unknown = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data")
        .token_unknown(&token);
    (!unknown).then_some(CastAt::Unit(token))
}

/// The mode the four `TargetNearest*` shims hand the TAB cycler, the only byte that differs between
/// them; it reaches only the per-candidate filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NearestMode {
    /// Mode 1, `TargetNearestEnemy`.
    Enemy,
    /// Mode 2, `TargetNearestFriend`.
    Friend,
    /// Mode 2 of 12340's table, `TargetNearestEnemyPlayer`: [`Self::Enemy`] for players alone.
    EnemyPlayer,
    /// Mode 4 of 12340's table, `TargetNearestFriendPlayer`: [`Self::Friend`] for players alone.
    FriendPlayer,
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

    /// Take the loot verbs left waiting where no host state was lent, in call order, leaving the
    /// rest queued.
    pub fn take_loot_verbs(&mut self) -> Vec<LootVerb> {
        self.take_calls_where(|c| match c {
            ScriptCall::Loot(verb) => Some(*verb),
            _ => None,
        })
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

/// A host standing in for the app in this crate's tests: its state lent, its applier and answerer named.
#[cfg(test)]
pub(crate) mod in_call {
    use std::cell::RefCell;

    use super::{set_answer, set_apply, HostAnswer, HostAsk, ScriptCall};
    use crate::script::UiScript;

    thread_local! {
        static ANSWER: RefCell<Option<HostAnswer>> = const { RefCell::new(None) };
        static ASKED: RefCell<Vec<HostAsk>> = const { RefCell::new(Vec::new()) };
    }

    /// Applies every call at once, leaving `APPLIED <call>` in the fired log at its step.
    fn apply(lua: &mlua::Lua, call: ScriptCall) -> bool {
        crate::script::tick::fire_event_into(lua, &format!("APPLIED {call:?}"), Vec::new());
        true
    }

    fn answer(_lua: &mlua::Lua, ask: &HostAsk) -> Option<HostAnswer> {
        ASKED.with_borrow_mut(|a| a.push(ask.clone()));
        ANSWER.with_borrow(|a| *a)
    }

    /// Run `src` with a host lent that answers every ask with `with`, inside the caller's loan or a bare view's; what the verbs asked, in order.
    pub(crate) fn run(s: &mut UiScript, with: Option<HostAnswer>, src: &str) -> Vec<HostAsk> {
        let lua = s.lua().clone();
        set_apply(&lua, apply);
        set_answer(&lua, answer);
        ANSWER.with_borrow_mut(|a| *a = with);
        // Inside a test's own loan its view stays the one read; else a bare view is lent.
        if crate::script::world_loan::lent() {
            s.run(src).unwrap();
        } else {
            let mut view = crate::script::live::TestView::default();
            crate::script::world_loan::lend_live(&lua, &mut view, || s.run(src).unwrap());
        }
        ASKED.with_borrow_mut(std::mem::take)
    }
}
