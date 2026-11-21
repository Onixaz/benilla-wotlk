//! The stance bar's four bindings, which `BonusActionBarFrame.lua`'s `ShapeshiftBar_*` calls, and
//! `GetShapeshiftForm`, over the stance list and form byte read at the call; flags answer 1/nil and
//! the cooldown is the reference's `(start, duration, enable)`.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::bool_or_default;
use super::Model;

/// One stance-bar button, resolved by the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShapeshiftFormView {
    /// The form spell a click queues.
    pub spell_id: u32,
    /// The icon texture path; `None` shows the slot's fallback.
    pub texture: Option<String>,
    /// The spell name, `GetShapeshiftFormInfo`'s second return.
    pub name: String,
    /// The player's current form (the checked ring).
    pub active: bool,
    /// Castable now; the reference greys the icon to 0.4 when not.
    pub castable: bool,
    /// `(start_ms on the GetTime clock, duration_ms, enabled)`, as an action's cooldown.
    pub cooldown: Option<(i64, u32, bool)>,
}

/// A pushed form with its cooldown in `GetTime` seconds.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct StoredShapeshiftForm {
    pub(crate) view: ShapeshiftFormView,
    /// `(start_s, duration_s, enabled)`.
    pub(crate) cooldown: Option<(f64, f64, bool)>,
}

#[cfg(any(test, feature = "fixtures"))]
impl super::UiScript {
    /// Stand in the form list, in bar order, for a VM with no live view.
    pub fn set_shapeshift_forms(&mut self, forms: Vec<ShapeshiftFormView>) {
        let mut model = self.model_mut();
        model.shapeshift_forms = forms
            .into_iter()
            .map(|view| {
                let cooldown = view.cooldown.map(|(start_ms, duration_ms, enabled)| {
                    (
                        start_ms as f64 / 1000.0,
                        f64::from(duration_ms) / 1000.0,
                        enabled,
                    )
                });
                StoredShapeshiftForm { view, cooldown }
            })
            .collect();
    }

    /// Stand in the player's raw form byte, 0 unshifted or no player, for a VM with no live view.
    pub fn set_shapeshift_form_byte(&mut self, form: u8) {
        self.model_mut().shapeshift_form_byte = form;
    }
}

impl super::UiScript {
    /// Take the `CastShapeshiftForm` calls out of the call stream, as form spells; the app casts
    /// each, or cancels it when it is the active form.
    pub fn take_shapeshift_casts(&mut self) -> Vec<u32> {
        self.take_calls_where(|c| match c {
            super::ScriptCall::CastShapeshiftForm(id) => Some(*id),
            _ => None,
        })
    }
}

/// The stance bar at the call: the live view's, else the stand-in a test or fixture pushed, each
/// with its cooldown in `GetTime` seconds where the stand-in carries one.
pub(super) fn forms(lua: &Lua) -> Vec<StoredShapeshiftForm> {
    if let Some(live) = super::read_live(|l| l.shapeshift_forms()).flatten() {
        return live
            .into_iter()
            .map(|view| StoredShapeshiftForm {
                view,
                cooldown: None,
            })
            .collect();
    }
    stand_in_forms(lua)
}

#[cfg(any(test, feature = "fixtures"))]
fn stand_in_forms(lua: &Lua) -> Vec<StoredShapeshiftForm> {
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    model.shapeshift_forms.clone()
}

#[cfg(not(any(test, feature = "fixtures")))]
fn stand_in_forms(_: &Lua) -> Vec<StoredShapeshiftForm> {
    Vec::new()
}

/// The player's form byte at the call, else the stand-in's.
fn form_byte(model: &Model) -> u8 {
    let live = super::read_live(|l| l.shapeshift_form_byte()).flatten();
    #[cfg(any(test, feature = "fixtures"))]
    let live = live.or(Some(model.shapeshift_form_byte));
    #[cfg(not(any(test, feature = "fixtures")))]
    let _ = model;
    live.unwrap_or(0)
}

/// A 1-based button index, as the reference indexes.
fn form_at(forms: &[StoredShapeshiftForm], i: u32) -> Option<&StoredShapeshiftForm> {
    usize::try_from(i.checked_sub(1)?)
        .ok()
        .and_then(|n| forms.get(n))
}

/// The current form's 1-based bar index (`0x53d4b0`): the last button reading active, else one
/// past the bar while the form byte is set, else 0; `shifted_only` answers 0 while it is not.
pub(super) fn current_form_index(model: &Model, shifted_only: bool) -> u32 {
    let shifted = form_byte(model) != 0;
    if shifted_only && !shifted {
        return 0;
    }
    let live = super::read_live(|l| l.shapeshift_forms()).flatten();
    let active: Vec<bool> = match live {
        Some(forms) => forms.iter().map(|f| f.active).collect(),
        #[cfg(any(test, feature = "fixtures"))]
        None => model
            .shapeshift_forms
            .iter()
            .map(|f| f.view.active)
            .collect(),
        #[cfg(not(any(test, feature = "fixtures")))]
        None => Vec::new(),
    };
    match active.iter().rposition(|&a| a) {
        Some(i) => i as u32 + 1,
        None if shifted => active.len() as u32 + 1,
        None => 0,
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `GetShapeshiftForm([shiftedOnly])` (`0x53de10`): the flag read as a boolean, default false.
    g.set(
        "GetShapeshiftForm",
        lua.create_function(|lua, shifted_only: Value| {
            let shifted_only = bool_or_default(Some(&shifted_only), false);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(current_form_index(&model, shifted_only))
        })?,
    )?;

    // `GetNumShapeshiftForms()` (`0x53c0b0`): the list's count.
    g.set(
        "GetNumShapeshiftForms",
        lua.create_function(|lua, ()| Ok(forms(lua).len() as i64))?,
    )?;

    g.set(
        "GetShapeshiftFormInfo",
        lua.create_function(|lua, i: u32| {
            let forms = forms(lua);
            let Some(form) = form_at(&forms, i) else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            let texture = match &form.view.texture {
                Some(t) => Value::String(lua.create_string(t)?),
                None => Value::Nil,
            };
            let flag = |b: bool| if b { Value::Integer(1) } else { Value::Nil };
            Ok(MultiValue::from_vec(vec![
                texture,
                Value::String(lua.create_string(&form.view.name)?),
                flag(form.view.active),
                flag(form.view.castable),
            ]))
        })?,
    )?;

    // As `GetActionCooldown`: an elapsed or absent cooldown answers `(0, 0, 1)`, so a re-feed never
    // replays the sweep.
    // The live bar reads the spell's cooldown at the call (`0x53c180` -> `0x809000`).
    g.set(
        "GetShapeshiftFormCooldown",
        lua.create_function(|lua, i: u32| {
            let now = crate::script::clock::now(lua);
            let live = super::read_live(|l| l.shapeshift_forms().is_some()).unwrap_or(false);
            let forms = forms(lua);
            let cooldown = form_at(&forms, i).and_then(|f| {
                if live {
                    super::spellbook::live_spell_cooldown(lua, f.view.spell_id).map(
                        |(start_ms, duration_ms, enabled)| {
                            (
                                start_ms as f64 / 1000.0,
                                f64::from(duration_ms) / 1000.0,
                                enabled,
                            )
                        },
                    )
                } else {
                    f.cooldown
                }
            });
            Ok(match cooldown {
                Some((start, duration, enabled)) if start + duration > now || !enabled => {
                    (start, duration, i32::from(enabled))
                }
                _ => (0.0, 0.0, 1),
            })
        })?,
    )?;

    g.set(
        "CastShapeshiftForm",
        lua.create_function(|lua, i: u32| {
            let spell = form_at(&forms(lua), i).map(|f| f.view.spell_id);
            if let Some(spell_id) = spell {
                super::calls::make(lua, super::ScriptCall::CastShapeshiftForm(spell_id));
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ShapeshiftFormView;
    use crate::script::UiScript;

    fn forms() -> Vec<ShapeshiftFormView> {
        vec![
            ShapeshiftFormView {
                spell_id: 2457,
                texture: Some("Interface\\Icons\\Ability_Warrior_OffensiveStance".into()),
                name: "Battle Stance".into(),
                active: true,
                castable: true,
                cooldown: None,
            },
            ShapeshiftFormView {
                spell_id: 71,
                texture: Some("Interface\\Icons\\Ability_Warrior_DefensiveStance".into()),
                name: "Defensive Stance".into(),
                active: false,
                castable: true,
                cooldown: Some((9400, 1500, true)),
            },
        ]
    }

    #[test]
    fn form_info_reads_and_out_of_range_nil() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumShapeshiftForms()").unwrap(), 0);
        assert!(s
            .eval::<bool>("return GetShapeshiftFormInfo(1) == nil")
            .unwrap());

        s.set_shapeshift_forms(forms());
        assert_eq!(s.eval::<i64>("return GetNumShapeshiftForms()").unwrap(), 2);

        let (texture, name, active, castable) = s
            .eval::<(String, String, Option<i64>, Option<i64>)>("return GetShapeshiftFormInfo(1)")
            .unwrap();
        assert_eq!(
            (texture.as_str(), name.as_str(), active, castable),
            (
                "Interface\\Icons\\Ability_Warrior_OffensiveStance",
                "Battle Stance",
                Some(1),
                Some(1)
            )
        );
        assert!(s
            .eval::<bool>("local _, _, a, c = GetShapeshiftFormInfo(2) return a == nil and c == 1")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetShapeshiftFormInfo(3) == nil")
            .unwrap());
    }

    #[test]
    fn get_shapeshift_form_walks_the_bar_like_0x53d4b0() {
        let form = |s: &mut UiScript, arg: &str| {
            s.eval::<i64>(&format!("return GetShapeshiftForm({arg})"))
                .unwrap()
        };
        let mut s = UiScript::new().unwrap();
        assert_eq!(form(&mut s, ""), 0, "no bar, no form");

        // Battle Stance lit, form 17.
        let mut bar = forms();
        s.set_shapeshift_forms(bar.clone());
        s.set_shapeshift_form_byte(17);
        assert_eq!(form(&mut s, ""), 1);
        assert_eq!(form(&mut s, "true"), 1);

        // Defensive Stance lit, form 18.
        bar[0].active = false;
        bar[1].active = true;
        s.set_shapeshift_forms(bar.clone());
        s.set_shapeshift_form_byte(18);
        assert_eq!(form(&mut s, ""), 2);

        // Both lit: the walk runs from the end.
        bar[0].active = true;
        s.set_shapeshift_forms(bar.clone());
        assert_eq!(form(&mut s, ""), 2);

        // Shifted into a form no button matches: one past the bar.
        bar[0].active = false;
        bar[1].active = false;
        s.set_shapeshift_forms(bar.clone());
        assert_eq!(form(&mut s, ""), 3);

        // A lit button with the form byte clear (a paladin aura): only the flag hides it, and
        // the boolean read takes 1 as true and nil as false.
        bar[0].active = true;
        s.set_shapeshift_forms(bar);
        s.set_shapeshift_form_byte(0);
        assert_eq!(form(&mut s, ""), 1);
        assert_eq!(form(&mut s, "nil"), 1);
        assert_eq!(form(&mut s, "1"), 0);
        assert_eq!(form(&mut s, "true"), 0);
    }

    #[test]
    fn cooldown_triple_stamps_to_the_vm_clock_and_goes_cold() {
        let mut s = UiScript::new().unwrap();
        s.tick(10.0); // GetTime == 10
        s.set_shapeshift_forms(forms());

        assert_eq!(
            s.eval::<(f64, f64, i32)>("return GetShapeshiftFormCooldown(1)")
                .unwrap(),
            (0.0, 0.0, 1)
        );
        let (start, duration, enable) = s
            .eval::<(f64, f64, i32)>("return GetShapeshiftFormCooldown(2)")
            .unwrap();
        assert!((start - 9.4).abs() < 1e-9, "start {start}");
        assert!((duration - 1.5).abs() < 1e-9);
        assert_eq!(enable, 1);

        s.tick(2.0); // now == 12 > 9.4 + 1.5
        assert_eq!(
            s.eval::<(f64, f64, i32)>("return GetShapeshiftFormCooldown(2)")
                .unwrap(),
            (0.0, 0.0, 1)
        );
    }

    #[test]
    fn cast_queues_the_form_spell_and_ignores_out_of_range() {
        let mut s = UiScript::new().unwrap();
        s.set_shapeshift_forms(forms());

        s.run("CastShapeshiftForm(2)").unwrap();
        s.run("CastShapeshiftForm(9)").unwrap(); // out of range: no-op
        assert_eq!(s.take_shapeshift_casts(), vec![71]);
        assert!(s.take_shapeshift_casts().is_empty(), "drain empties");
    }
}
