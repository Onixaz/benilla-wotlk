//! The action-bar bindings. The app pushes what each of the 120 slots shows and its per-frame
//! state, and drains the queued `UseAction` presses and the slot writes the cursor made
//! (`CMSG_SET_ACTION_BUTTON`); the engine keeps each slot's packed `(kind, action)` so the cursor
//! can move a slot without the app. Actions are keyed by Lua action id, 1..120, one more than the
//! wire's slot index. FrameXML's bonus bar shows actions `(6 + offset - 1) * 12 + i` for the
//! app-pushed `GetBonusBarOffset`.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;

/// The kind byte, bits 24-31 of the packed slot word (vmangos `Player.h:133`). Must match
/// `benilla_protocol`'s `ACTION_KIND_*`; this crate has no protocol dependency.
pub(crate) const ACTION_KIND_SPELL: u8 = 0x00;
pub(crate) const ACTION_KIND_MACRO: u8 = 0x40;
pub(crate) const ACTION_KIND_ITEM: u8 = 0x80;

/// What one action slot shows; the app resolves the icon before pushing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActionSlot {
    /// The icon texture path (`Interface\Icons\…`); `None` shows the slot's fallback.
    pub texture: Option<String>,
    /// The kind byte, one of the `ACTION_KIND_*` above.
    pub kind: u8,
    /// The spell, macro or item id (bits 0-23 of the packed slot word).
    pub action: u32,
    /// `GetActionCount`'s bag count for an item slot, 0 otherwise.
    pub count: u32,
    /// `IsConsumableAction`: `0x4e5250` reads only the slot's item template, so this rides the
    /// icon's push, not [`ActionState`]'s.
    pub consumable: bool,
    /// `IsStackableAction`: an item slot whose template stacks past one (`0x5a8470`, record
    /// `+0x5c`); it rides the icon's push as [`Self::consumable`] does.
    pub stackable: bool,
}

/// One action's per-frame state, read by `IsUsableAction`, `IsActionInRange`, `GetActionCooldown`
/// and kin; kept apart from [`ActionSlot`] so its churn never fires `ACTIONBAR_SLOT_CHANGED`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ActionState {
    /// `IsUsableAction`'s first return: castable now.
    pub usable: bool,
    /// `IsUsableAction`'s second return: unusable only for lack of power (FrameXML's blue tint).
    pub not_enough_mana: bool,
    /// `IsActionInRange`: `None` (nil) for a rangeless action or no target, else 1 or 0.
    pub in_range: Option<bool>,
    /// `ActionHasRange`: the action's spell has a range to test.
    pub has_range: bool,
    /// `IsCurrentAction`: the Attack action while auto-attack is on, or the spell being cast.
    pub current: bool,
    /// `IsAutoRepeatAction`: the action's spell is the live autorepeat spell (`0xceac30`).
    pub auto_repeat: bool,
    /// `IsAttackAction`: the action is the melee auto-attack.
    pub is_attack: bool,
    /// `IsEquippedAction`: an item action currently worn.
    pub equipped: bool,
    /// `(start_ms, duration_ms, enabled)`, the start absolute on the `GetTime` clock, so a running
    /// cooldown re-pushes the same triple and a re-arm a new one (`0x6e13e0` returns the start).
    pub cooldown: Option<(i64, u32, bool)>,
}

/// [`ActionState`] with its cooldown in `GetTime` seconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct StoredActionState {
    pub(crate) state: ActionState,
    pub(crate) cooldown: Option<(f64, f64, bool)>,
}

/// One queued `UseAction` press.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionUse {
    /// The 1-based Lua action id, as `UseAction` was given it.
    pub action: u32,
    /// `UseAction`'s unit argument: `SecureActionButton_OnClick` passes `"player"` under the
    /// self-cast modifier (`SecureTemplates.lua:207-210`), else nil, the selection.
    pub at: super::CastAt,
}

impl super::UiScript {
    /// Push (or clear) one action slot, keyed by Lua action id (1..120).
    pub fn set_action(&mut self, action: u32, slot: Option<ActionSlot>) {
        let mut model = self.model_mut();
        match slot {
            Some(s) => {
                model.actions.insert(action, s);
            }
            None => {
                model.actions.remove(&action);
            }
        }
    }

    /// Push (or clear) one action's state; the cooldown start is already on the `GetTime` clock,
    /// so storing only converts milliseconds to seconds.
    pub fn set_action_state(&mut self, action: u32, state: Option<ActionState>) {
        let mut model = self.model_mut();
        match state {
            Some(s) => {
                let cooldown = s.cooldown.map(|(start_ms, duration_ms, enabled)| {
                    (
                        start_ms as f64 / 1000.0,
                        f64::from(duration_ms) / 1000.0,
                        enabled,
                    )
                });
                model
                    .action_states
                    .insert(action, StoredActionState { state: s, cooldown });
            }
            None => {
                model.action_states.remove(&action);
            }
        }
    }

    /// Push the current bonus-bar page offset (0 = the plain main bar; warrior stances 1..3).
    pub fn set_bonus_bar_offset(&mut self, offset: u8) {
        set_field!(self, bonus_bar_offset, offset);
    }

    /// Take the `UseAction` presses out of the call stream.
    pub fn take_action_uses(&mut self) -> Vec<ActionUse> {
        self.take_calls_where(|c| match c {
            super::ScriptCall::UseAction(press) => Some(press.clone()),
            _ => None,
        })
    }

    /// Drain the `(lua action id, packed)` pairs `PickupAction`/`PlaceAction` queued; the app
    /// sends each as `CMSG_SET_ACTION_BUTTON` (`packed == 0` clears) and updates its own store.
    pub fn take_action_sets(&mut self) -> Vec<(u32, u32)> {
        drain_field!(self, action_sets)
    }

    /// Drain the GlobalStrings keys of engine-side refusals; the app fires `UI_ERROR_MESSAGE` for
    /// each, where the reference calls `CGGameUI::DisplayError` inline.
    pub fn take_ui_errors(&mut self) -> Vec<&'static str> {
        drain_field!(self, ui_errors)
    }
}

/// `UseAction`'s flag test: `nil`, `false` and `0` are false, unlike Lua, where `0` is true.
pub(super) fn truthy_nonzero(v: &Value) -> bool {
    match v {
        Value::Nil => false,
        Value::Boolean(b) => *b,
        Value::Integer(i) => *i != 0,
        Value::Number(n) => *n != 0.0,
        _ => true,
    }
}

/// Register the action globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "HasAction",
        lua.create_function(|lua, action: u32| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(model.actions.contains_key(&action)))
        })?,
    )?;

    // GetActionAutocast(slot) (`0x4e6f90`) → autoCastAllowed, autoCastEnabled: 1 or nil each,
    // always two values. Only a pet spell has them: the slot resolver (`0x4e5a50`) writes its
    // book flag 0 for a spell or an item and a macro's `[rec+0x568]`, and only a set flag looks the
    // spell up in the pet's words (`0x4bd160`, low 16 bits), reading bits 31 and 30. An empty,
    // out-of-range or missing-macro slot leaves the flag unwritten, and the stale non-zero local
    // looks up spell 0, which no pet word carries. A non-number raises.
    g.set(
        "GetActionAutocast",
        lua.create_function(|lua, slot: Value| {
            let slot = super::binding_abi::number_arg(lua, slot, "Usage: GetActionAutocast(slot)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            // `(spell, pet_book)` as the resolver leaves them.
            let resolved = u32::try_from(slot)
                .ok()
                .filter(|s| (1..=120).contains(s))
                .and_then(|s| model.actions.get(&s))
                .and_then(|a| match a.kind & 0xf0 {
                    ACTION_KIND_MACRO => model.macros.get(a.action as usize).map(|_| {
                        let b = model
                            .macro_bindings
                            .get(&a.action)
                            .copied()
                            .unwrap_or_default();
                        (b.spell as u32, b.pet_book)
                    }),
                    _ => Some((a.action, false)),
                })
                .unwrap_or((0, true));
            let autocast = match resolved {
                (spell, true) => model
                    .pet_book
                    .slots
                    .iter()
                    .find(|s| s.spell_id & 0xffff == spell)
                    .and_then(|s| s.autocast),
                (_, false) => None,
            };
            let (allowed, enabled) = autocast.unwrap_or((false, false));
            Ok((flag(allowed), flag(enabled)))
        })?,
    )?;

    g.set(
        "GetActionTexture",
        lua.create_function(|lua, action: u32| {
            let tex = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.actions.get(&action).and_then(|s| s.texture.clone())
            };
            match tex {
                Some(t) => Ok(Value::String(lua.create_string(&t)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetActionText(slot): the macro's name, else nil. `0x4e7050` tests only
    // `raw & 0xf0000000 == 0x40000000`, so spell, item and empty slots all answer nil; a missing or
    // non-number slot raises `Usage: GetActionText(slot)` (`0x84bff4`); the slot is 1-based
    // (`0x4e7072` `dec eax`). An empty name answers "": it is an inline buffer at `+0x24`, so
    // `0x6f3890`'s null guard never fires.
    // The reference's macro payload (`raw & 0x3fffffff`) is a hash key into `[0xbdcc54]`; ours is
    // the 1..36 index, the lookup the app's macro icon makes too, so text and icon agree.
    // Deviation: an out-of-range slot answers nil, because the reference indexes `0xbc6980` with
    // no bounds check and reads the memory beside its 120 slots.
    g.set(
        "GetActionText",
        lua.create_function(|lua, slot: Value| {
            let slot = super::binding_abi::number_arg(lua, slot, "Usage: GetActionText(slot)")?;
            let name = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                u32::try_from(slot)
                    .ok()
                    .and_then(|s| model.actions.get(&s))
                    .filter(|s| s.kind & 0xf0 == ACTION_KIND_MACRO)
                    .and_then(|s| model.macros.get(s.action as usize))
                    .map(|m| m.name.clone())
            };
            match name {
                Some(n) => Ok(Value::String(lua.create_string(&n)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetActionInfo(slot) (`0x5a8f10`): the slot is a number or `Usage:` (`0x5a91a6`). A spell
    // answers `"spell", bookIndex, "spell", spellId`, the index 1-based in the book `0x53b4e0`
    // searches and 0 for a spell not in it (`0x5a9049`); an item `"item", itemId`; a macro
    // `"macro", index` (`0x5a9133`); an empty slot nothing.
    //
    // Deviation: a companion spell answers as a spell, where the reference answers `"companion",
    // index, "CRITTER"|"MOUNT", spellId` (`0x5a8fa8`), and an equipment-set slot answers nothing
    // for `"equipmentset", name` (`0x5a9178`): benilla keeps no companion list and no equipment
    // sets.
    g.set(
        "GetActionInfo",
        lua.create_function(|lua, slot: Value| {
            let slot = super::binding_abi::number_arg(lua, slot, "Usage: GetActionInfo(slot)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let Some(a) = u32::try_from(slot).ok().and_then(|s| model.actions.get(&s)) else {
                return Ok(MultiValue::new());
            };
            let str = |s: &str| lua.create_string(s).map(Value::String);
            Ok(MultiValue::from_vec(match a.kind & 0xf0 {
                ACTION_KIND_ITEM => vec![str("item")?, Value::Integer(i64::from(a.action))],
                ACTION_KIND_MACRO => vec![str("macro")?, Value::Integer(i64::from(a.action))],
                0x00 => {
                    let index = model
                        .spellbook
                        .slots
                        .iter()
                        .position(|s| s.spell_id == a.action)
                        .map_or(0, |i| i as i64 + 1);
                    vec![
                        str("spell")?,
                        Value::Integer(index),
                        str("spell")?,
                        Value::Integer(i64::from(a.action)),
                    ]
                }
                _ => Vec::new(),
            }))
        })?,
    )?;

    // UseAction(slot [, unit [, button]]) (`0x5ac000`): the slot is a number or `Usage:`
    // (`0x5ac06f`), and the unit goes through the cast target parse (`0x5ac043`), where a token
    // the resolver does not know uses nothing. The button string reaches only the possess-bar and
    // macro arms of `0x5abbc0`. Nothing here reads the cursor: the stock bar places a payload
    // through `OnReceiveDrag` (`ActionBarFrame.xml:25-29`).
    g.set(
        "UseAction",
        lua.create_function(|lua, (action, rest): (Value, MultiValue)| {
            let action = super::binding_abi::number_arg(
                lua,
                action,
                "Usage: UseAction(slot, [, target] [, button])",
            )?;
            let Some(at) = super::calls::cast_at_arg(lua, rest.front()) else {
                return Ok(());
            };
            let Ok(action) = u32::try_from(action) else {
                return Ok(());
            };
            let mut model = crate::script::model_mut_of(lua);
            model
                .script_calls
                .push(super::ScriptCall::UseAction(ActionUse { action, at }));
            Ok(())
        })?,
    )?;

    g.set(
        "GetActionCount",
        lua.create_function(|lua, action: u32| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model
                .actions
                .get(&action)
                .filter(|s| s.kind == ACTION_KIND_ITEM)
                .map_or(0, |s| s.count))
        })?,
    )?;

    g.set(
        "GetBonusBarOffset",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(model.bonus_bar_offset))
        })?,
    )?;

    // GetActionBarPage() (`0x5a7fd0`): the main bar's page, 1..6. A vehicle or possess bar pins it
    // to 1 (`[0xc1e5a0]`), which benilla never raises.
    g.set(
        "GetActionBarPage",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(model.action_bar_page) + 1)
        })?,
    )?;

    // ChangeActionBarPage(page) (`0x5a7f60`): the page is `lua_tonumber` truncated, so nil is 0,
    // and outside 1..6 raises (`0x5a7fb5`). A new page is stored and fires `ACTIONBAR_PAGE_CHANGED`
    // synchronously (`0x5a7fa7`); the same page does nothing. The protected-call check
    // (`0x5191c0`) answers only tainted code, which benilla does not track.
    g.set(
        "ChangeActionBarPage",
        lua.create_function(|lua, page: Value| {
            let page = super::binding_abi::coerced_number(lua, Some(page)) as i64;
            let Some(index) = page.checked_sub(1).filter(|i| (0..6).contains(i)) else {
                return Err(mlua::Error::runtime(
                    "ChangeActionBarPage() needs a page in the range 1 to 6.",
                ));
            };
            let changed = {
                let mut model = crate::script::model_mut_of(lua);
                let changed = i64::from(model.action_bar_page) != index;
                model.action_bar_page = index as u8;
                changed
            };
            if changed {
                super::tick::fire_event_into(lua, "ACTIONBAR_PAGE_CHANGED", Vec::new());
            }
            Ok(())
        })?,
    )?;

    // ── Per-action state ──
    // 1/nil booleans and `IsActionInRange`'s nil/0/1, which FrameXML tests with a plain `if`.

    fn state_flag(lua: &Lua, action: u32, pick: impl Fn(&ActionState) -> bool) -> Value {
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model app_data");
        match model.action_states.get(&action) {
            Some(s) if pick(&s.state) => Value::Integer(1),
            _ => Value::Nil,
        }
    }

    g.set(
        "IsUsableAction",
        lua.create_function(|lua, action: u32| {
            Ok((
                state_flag(lua, action, |s| s.usable),
                state_flag(lua, action, |s| s.not_enough_mana),
            ))
        })?,
    )?;

    g.set(
        "IsActionInRange",
        lua.create_function(|lua, action: u32| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(
                match model
                    .action_states
                    .get(&action)
                    .and_then(|s| s.state.in_range)
                {
                    Some(true) => Value::Integer(1),
                    Some(false) => Value::Integer(0),
                    None => Value::Nil,
                },
            )
        })?,
    )?;

    g.set(
        "ActionHasRange",
        lua.create_function(|lua, action: u32| Ok(state_flag(lua, action, |s| s.has_range)))?,
    )?;
    g.set(
        "IsCurrentAction",
        lua.create_function(|lua, action: u32| Ok(state_flag(lua, action, |s| s.current)))?,
    )?;
    g.set(
        "IsAutoRepeatAction",
        lua.create_function(|lua, action: u32| Ok(state_flag(lua, action, |s| s.auto_repeat)))?,
    )?;
    g.set(
        "IsAttackAction",
        lua.create_function(|lua, action: u32| Ok(state_flag(lua, action, |s| s.is_attack)))?,
    )?;
    // From the slot, not the state map: `0x4e5250` depends only on the item template.
    g.set(
        "IsConsumableAction",
        lua.create_function(|lua, action: u32| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            match model.actions.get(&action) {
                Some(s) if s.consumable => Ok(Value::Integer(1)),
                _ => Ok(Value::Nil),
            }
        })?,
    )?;
    // IsStackableAction(slot) (`0x5a8720`): 1 or nil; a non-number raises (`0x5a877d`).
    g.set(
        "IsStackableAction",
        lua.create_function(|lua, slot: Value| {
            let slot = super::binding_abi::number_arg(lua, slot, "Usage: IsStackableAction(slot)")?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(
                u32::try_from(slot)
                    .ok()
                    .and_then(|s| model.actions.get(&s))
                    .is_some_and(|a| a.stackable),
            ))
        })?,
    )?;
    g.set(
        "IsEquippedAction",
        lua.create_function(|lua, action: u32| Ok(state_flag(lua, action, |s| s.equipped)))?,
    )?;

    // GetActionCooldown(action) → start, duration, enable, in `GetTime` seconds. An absent or
    // elapsed enabled cooldown answers `(0, 0, 1)`, or a later `CooldownFrame_SetTimer` would
    // replay the sweep and its finish flash.
    g.set(
        "GetActionCooldown",
        lua.create_function(|lua, action: u32| {
            let now = crate::script::clock::now(lua);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(
                match model.action_states.get(&action).and_then(|s| s.cooldown) {
                    Some((start, duration, enabled)) if start + duration > now || !enabled => {
                        (start, duration, i32::from(enabled))
                    }
                    _ => (0.0, 0.0, 1),
                },
            )
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ActionSlot;
    use crate::script::CastAt;
    use crate::script::UiScript;

    /// `GetActionAutocast` answers a pet spell's two bits only for a macro bound out of the pet's
    /// book (`0x4e6fcb`), never for a spell slot of the same id, and always two values.
    #[test]
    fn action_autocast_reads_a_pet_book_macro_only() {
        use crate::script::{MacroBinding, MacroState, MacroView, PetBookState, SpellSlotView};
        let mut s = UiScript::new().unwrap();
        s.set_pet_book(PetBookState {
            token: Some("PET".into()),
            slots: vec![SpellSlotView {
                spell_id: 3009, // Claw
                autocast: Some((true, false)),
                ..Default::default()
            }],
        });
        s.set_macros(MacroState {
            account: vec![
                MacroView {
                    name: "claw".into(),
                    body: "/cast Claw".into(),
                    ..Default::default()
                },
                MacroView {
                    name: "fb".into(),
                    body: "/cast Fireball".into(),
                    ..Default::default()
                },
            ],
            character: Vec::new(),
        });
        s.set_macro_bindings(std::collections::HashMap::from([
            (
                1,
                MacroBinding {
                    spell: 3009,
                    pet_book: true,
                },
            ),
            (
                2,
                MacroBinding {
                    spell: 133,
                    pet_book: false,
                },
            ),
        ]));
        let slot = |kind, action| ActionSlot {
            kind,
            action,
            ..Default::default()
        };
        s.set_action(1, Some(slot(super::ACTION_KIND_MACRO, 1)));
        s.set_action(2, Some(slot(super::ACTION_KIND_MACRO, 2)));
        s.set_action(3, Some(slot(super::ACTION_KIND_SPELL, 3009)));
        s.set_action(4, Some(slot(super::ACTION_KIND_MACRO, 9)));

        let ask = |s: &UiScript, n: u32| {
            s.eval::<(i64, Option<i64>, Option<i64>)>(&format!(
                "local f = function(...) return arg.n, arg[1], arg[2] end \
                 return f(GetActionAutocast({n}))"
            ))
            .unwrap()
        };
        assert_eq!(ask(&s, 1), (2, Some(1), None), "the pet-book macro");
        for n in [2, 3, 4, 5, 0, 121] {
            assert_eq!(ask(&s, n), (2, None, None), "slot {n}");
        }
        s.set_pet_book(PetBookState {
            token: Some("PET".into()),
            slots: vec![SpellSlotView {
                spell_id: 3009,
                autocast: Some((true, true)),
                ..Default::default()
            }],
        });
        assert_eq!(ask(&s, 1), (2, Some(1), Some(1)), "autocasting");
        let err = s.run("GetActionAutocast()").unwrap_err().to_string();
        assert!(err.contains("Usage: GetActionAutocast(slot)"), "{err}");
    }

    fn uses(s: &mut UiScript) -> Vec<(u32, CastAt)> {
        s.take_action_uses()
            .into_iter()
            .map(|u| (u.action, u.at))
            .collect()
    }

    const ME: u64 = 0x10;
    const MOB: u64 = 0xF130_0000_0000_0001;

    fn with_units() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit_guids(&crate::script::UnitGuids {
            player: ME,
            target: MOB,
            ..Default::default()
        });
        s
    }

    /// The second argument is a unit token (`0x5ac043`): nil or `""` is the selection, a token
    /// its unit, a token naming nobody the sentinel, and one the resolver does not know uses
    /// nothing. The third, the mouse button, never aims the press.
    #[test]
    fn use_actions_second_argument_is_the_unit() {
        let mut s = with_units();
        for lua in [
            "UseAction(1)",
            "UseAction(2, nil, 'LeftButton')",
            "UseAction(3, '')",
            "UseAction(4, 'player', 'LeftButton')",
            "UseAction(5, 'target')",
            "UseAction(6, 'party1')",
            "UseAction(7, 'nosuchunit')",
            "UseAction(8, 1)",
            "UseAction(9, true)",
        ] {
            s.run(lua).unwrap();
        }
        assert_eq!(
            uses(&mut s),
            vec![
                (1, CastAt::Selection),
                (2, CastAt::Selection),
                (3, CastAt::Selection),
                (4, CastAt::Unit("player".into())),
                (5, CastAt::Unit("target".into())),
                (6, CastAt::Unit("party1".into())),
                // 7 and 8: tokens the resolver does not know. A boolean has no string form, so
                // `lua_tolstring` reads it as absent.
                (9, CastAt::Selection),
            ],
        );
        let err = s.run("UseAction()").unwrap_err().to_string();
        assert!(
            err.contains("Usage: UseAction(slot, [, target] [, button])"),
            "{err}"
        );
    }

    /// The page is the engine's (`0x5a7f60`): stored, read back by `GetActionBarPage`, and the
    /// event fires synchronously on a change only.
    #[test]
    fn change_action_bar_page_stores_the_page_and_fires_on_a_change() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetActionBarPage()").unwrap(), 1);
        s.run(
            "FIRED = 0 local f = CreateFrame('Frame') f:RegisterEvent('ACTIONBAR_PAGE_CHANGED') \
             f:SetScript('OnEvent', function() FIRED = FIRED + 1 SEEN = GetActionBarPage() end) \
             N = table.getn({ChangeActionBarPage(3)}) ChangeActionBarPage(3)",
        )
        .unwrap();
        assert_eq!(
            s.eval::<i64>("return SEEN").unwrap(),
            3,
            "stored before it fires"
        );
        assert_eq!(
            s.eval::<i64>("return FIRED").unwrap(),
            1,
            "the same page is silent"
        );
        assert_eq!(s.eval::<i64>("return N").unwrap(), 0, "zero return values");
        for bad in [
            "ChangeActionBarPage(7)",
            "ChangeActionBarPage(0)",
            "ChangeActionBarPage()",
        ] {
            let err = s.run(bad).unwrap_err().to_string();
            assert!(
                err.contains("needs a page in the range 1 to 6"),
                "{bad}: {err}"
            );
        }
        assert_eq!(s.eval::<i64>("return GetActionBarPage()").unwrap(), 3);
    }

    #[test]
    fn action_snapshot_reads_and_use_queues() {
        let mut s = UiScript::new().unwrap();
        assert!(!s.eval::<bool>("return HasAction(73)").unwrap());
        assert!(s
            .eval::<bool>("return GetActionTexture(73) == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return GetBonusBarOffset()").unwrap(), 0);

        s.set_action(
            73,
            Some(ActionSlot {
                texture: Some("Interface\\Icons\\Ability_Rogue_Ambush".into()),
                kind: 0x00,
                action: 133,
                count: 0,
                consumable: false,
                stackable: false,
            }),
        );
        s.set_bonus_bar_offset(1);
        assert!(s.eval::<bool>("return HasAction(73)").unwrap());
        assert_eq!(
            s.eval::<String>("return GetActionTexture(73)").unwrap(),
            "Interface\\Icons\\Ability_Rogue_Ambush"
        );
        assert_eq!(s.eval::<i64>("return GetBonusBarOffset()").unwrap(), 1);

        s.run("UseAction(73)").unwrap();
        s.run("UseAction(74, 'player')").unwrap();
        assert_eq!(
            uses(&mut s),
            vec![(73, CastAt::Selection), (74, CastAt::Unit("player".into()))],
            "the token rides to the app, which resolves it in call order"
        );
        assert!(s.take_action_uses().is_empty());

        s.set_action(73, None);
        assert!(!s.eval::<bool>("return HasAction(73)").unwrap());
    }

    #[test]
    fn get_action_count_reads_the_pushed_count() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetActionCount(5)").unwrap(), 0);
        s.set_action(
            5,
            Some(ActionSlot {
                texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
                kind: 0x80,
                action: 117,
                count: 4,
                consumable: false,
                stackable: false,
            }),
        );
        assert_eq!(s.eval::<i64>("return GetActionCount(5)").unwrap(), 4);
    }

    /// `IsStackableAction` (`0x5a8720`) reads the pushed flag; an empty slot is nil.
    #[test]
    fn is_stackable_action_reads_the_slot() {
        let mut s = UiScript::new().unwrap();
        s.set_action(
            3,
            Some(ActionSlot {
                kind: super::ACTION_KIND_ITEM,
                action: 117,
                stackable: true,
                ..Default::default()
            }),
        );
        assert_eq!(
            s.eval::<Option<i64>>("return IsStackableAction(3)")
                .unwrap(),
            Some(1)
        );
        assert_eq!(
            s.eval::<Option<i64>>("return IsStackableAction(4)")
                .unwrap(),
            None
        );
        let err = s.run("IsStackableAction()").unwrap_err().to_string();
        assert!(err.contains("Usage: IsStackableAction(slot)"), "{err}");
    }

    /// `GetActionInfo` (`0x5a8f10`): four values for a spell, two for an item or a macro, none for
    /// an empty slot.
    #[test]
    fn get_action_info_answers_by_kind() {
        use crate::script::{SpellBookState, SpellSlotView};
        let mut s = UiScript::new().unwrap();
        s.set_spellbook(SpellBookState {
            tabs: Vec::new(),
            slots: [6603, 2973, 75]
                .into_iter()
                .map(|spell_id| SpellSlotView {
                    spell_id,
                    ..Default::default()
                })
                .collect(),
        });
        let slot = |kind, action| ActionSlot {
            kind,
            action,
            ..Default::default()
        };
        s.set_action(1, Some(slot(super::ACTION_KIND_SPELL, 2973)));
        s.set_action(2, Some(slot(super::ACTION_KIND_SPELL, 2050)));
        s.set_action(3, Some(slot(super::ACTION_KIND_ITEM, 117)));
        s.set_action(4, Some(slot(super::ACTION_KIND_MACRO, 2)));
        let info = |s: &UiScript, n: u32| {
            s.eval::<String>(&format!(
                "local t = {{GetActionInfo({n})}} \
                 for i = 1, table.getn(t) do t[i] = tostring(t[i]) end \
                 return table.concat(t, ',')"
            ))
            .unwrap()
        };
        assert_eq!(info(&s, 1), "spell,2,spell,2973", "the 1-based book index");
        assert_eq!(
            info(&s, 2),
            "spell,0,spell,2050",
            "not in the book: index 0"
        );
        assert_eq!(info(&s, 3), "item,117");
        assert_eq!(info(&s, 4), "macro,2");
        assert_eq!(s.arity("GetActionInfo(5)").unwrap(), 0, "an empty slot");
        let err = s.run("GetActionInfo()").unwrap_err().to_string();
        assert!(err.contains("Usage: GetActionInfo(slot)"), "{err}");
    }

    /// `UseAction` reads no cursor (`0x5abbc0`): a held payload stays held and the press queues.
    #[test]
    fn use_action_never_places_the_cursor() {
        use crate::script::cursor::{CursorAction, CursorPayload};

        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 1,
            kind: 0x00,
            action: 111,
            texture: Some("Interface\\Icons\\Spell_A".into()),
        }));
        s.run("UseAction(9, nil, 'LeftButton')").unwrap();
        assert_eq!(uses(&mut s), vec![(9, CastAt::Selection)]);
        assert!(s.cursor_payload().is_some());
        assert!(s.take_action_sets().is_empty());
    }

    // ── `GetActionText` (`0x4e7050`) ──

    #[test]
    fn get_action_text_is_the_macro_name_and_nil_for_everything_else() {
        use crate::script::{MacroState, MacroView};

        let mut s = UiScript::new().unwrap();
        s.set_macros(MacroState {
            account: vec![
                MacroView {
                    name: "Pull".into(),
                    ..Default::default()
                },
                MacroView {
                    name: String::new(),
                    ..Default::default()
                },
            ],
            character: Vec::new(),
        });
        let slot = |kind: u8, action: u32| {
            Some(ActionSlot {
                texture: Some("Interface\\Icons\\Spell_A".into()),
                kind,
                action,
                count: 0,
                consumable: false,
                stackable: false,
            })
        };
        s.set_action(1, slot(0x40, 1)); // macro "Pull"
        s.set_action(2, slot(0x40, 2)); // a macro with an empty name
        s.set_action(3, slot(0x40, 30)); // no such macro
        s.set_action(4, slot(0x00, 133)); // a spell
        s.set_action(5, slot(0x80, 117)); // an item

        assert_eq!(s.eval::<String>("return GetActionText(1)").unwrap(), "Pull");
        assert_eq!(
            s.eval::<String>("return GetActionText(2)").unwrap(),
            "",
            "an empty macro NAME is the empty string, not nil — `+0x24` is an inline buffer"
        );
        for (slot, why) in [
            (3, "a macro id that resolves to nothing"),
            (4, "a SPELL — the same nil arm"),
            (5, "an ITEM — the same nil arm"),
            (6, "an empty slot"),
            (121, "past the 120-slot array"),
            (0, "slot 0 (1-based)"),
        ] {
            assert!(
                s.eval::<bool>(&format!("return GetActionText({slot}) == nil"))
                    .unwrap(),
                "{why} must be nil"
            );
        }
        // One value on every non-raising path.
        assert_eq!(s.arity("GetActionText(4)").unwrap(), 1);
    }

    /// The raise is `0x4e70be` into `0x6f4940`, which never returns.
    #[test]
    fn get_action_text_raises_on_a_bad_slot() {
        let s = UiScript::new().unwrap();
        for call in ["GetActionText()", "GetActionText({})"] {
            let err = s
                .eval::<mlua::Value>(&format!("return {call}"))
                .unwrap_err();
            assert!(
                format!("{err}").contains("Usage: GetActionText(slot)"),
                "{call} must raise, got {err}"
            );
        }
    }
}
