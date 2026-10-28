//! The barber shop's nine globals (registrar `0xacc790`). The chair opens it with
//! `SMSG_ENABLE_BARBER_SHOP`; while open, the player's own body wears the dials the verbs step
//! through the race's `CharSections` grid in the barber context, and each dial's style is the
//! `BarberShopStyle` row whose `Data` it holds (`0x52f760`). The app pushes the real look, the
//! rows and the level's base price, sends the paid change and runs the close.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{bool_or_default, number_arg};
use super::glue_customize::{Customizer, GlueCustomizeGrid};

/// The style types, the verbs' `type - 1`.
const HAIR_STYLE: usize = 0;
const HAIR_COLOR: usize = 1;
const FACIAL_HAIR: usize = 2;
const SKIN: usize = 3;

/// The dial order the look and the walker share: skin, face, hair style, hair colour, facial hair.
const DIAL_SKIN: usize = 0;
const DIAL_HAIR_STYLE: usize = 2;
const DIAL_HAIR_COLOR: usize = 3;
const DIAL_FACIAL_HAIR: usize = 4;

/// One `BarberShopStyle.dbc` row of the player's race and sex.
#[derive(Clone, Debug, PartialEq)]
pub struct BarberStyle {
    pub id: u32,
    /// 0 hair style, 2 facial hair, 3 skin.
    pub style_type: u32,
    pub name: String,
    pub description: String,
    pub cost_modifier: f32,
    /// The look value the row stands for.
    pub data: i32,
}

/// The facts the app pushes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BarberHost {
    /// The open flag (`[0xbd19b8]`): set by `SMSG_ENABLE_BARBER_SHOP`, cleared by the close.
    pub open: bool,
    pub class: u8,
    /// `gtBarberShopCostBase` at the player's level.
    pub base_cost: f32,
    /// The real look off `PLAYER_BYTES`/`PLAYER_BYTES_2`, in dial order.
    pub look: [u8; 5],
    /// The rows of the player's race and sex, in file order (`0x52fe90`).
    pub styles: Vec<BarberStyle>,
    /// The race and sex's `CharSections` grid, which the dials walk.
    pub grid: Option<GlueCustomizeGrid>,
    /// `ChrRaces` `hairCustomization` and the sex's `facialHairCustomization`.
    pub hair_customization: String,
    pub facial_hair_customization: String,
}

/// What the verbs ask the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarberIntent {
    /// `ApplyBarberShopStyle()` with a changed look: the four ids of `CMSG_ALTER_APPEARANCE` and
    /// the price the app holds against the purse.
    Apply { styles: [u32; 4], cost: u32 },
    /// `CancelBarberShop()`.
    Cancel,
}

/// The VM's barber state: the host's facts, the dials the body previews and the queued intents.
#[derive(Debug, Default)]
pub(crate) struct BarberState {
    host: BarberHost,
    dials: [u8; 5],
    intents: Vec<BarberIntent>,
}

impl BarberState {
    /// The style row of `ty` whose data is `value`, the first in file order.
    fn style(&self, ty: usize, value: u8) -> Option<&BarberStyle> {
        self.host
            .styles
            .iter()
            .find(|s| s.style_type as usize == ty && s.data == i32::from(value))
    }

    /// The dial a style type steps.
    fn dial(ty: usize) -> usize {
        match ty {
            HAIR_STYLE => DIAL_HAIR_STYLE,
            HAIR_COLOR => DIAL_HAIR_COLOR,
            FACIAL_HAIR => DIAL_FACIAL_HAIR,
            _ => DIAL_SKIN,
        }
    }

    /// The current style of a type with rows (`0x52e490`), off its dial.
    fn current(&self, ty: usize) -> Option<&BarberStyle> {
        self.style(ty, self.dials[Self::dial(ty)])
    }

    /// `0x52e790`: the dial still holds the real look.
    fn is_current(&self, ty: usize) -> bool {
        let d = Self::dial(ty);
        self.host.look[d] == self.dials[d]
    }

    /// `CanAlterSkin()` (`0x52e4f0`): the skin has a style row.
    fn can_alter_skin(&self) -> bool {
        self.current(SKIN).is_some()
    }

    /// `0x52eba0`: the hair style's price, else half the base for a colour alone, plus the facial
    /// hair's and the skin's, each on the base truncated to copper first.
    fn total_cost(&self) -> u32 {
        if !self.host.open {
            return 0;
        }
        let base = self.host.base_cost as i32 as f32;
        let price = |m: f32| (base * m) as i32;
        let modifier = |ty| self.current(ty).map_or(0.0, |s| s.cost_modifier);
        let mut cost = 0;
        if !self.is_current(HAIR_STYLE) {
            cost = price(modifier(HAIR_STYLE));
        } else if !self.is_current(HAIR_COLOR) {
            cost = price(0.5);
        }
        if !self.is_current(FACIAL_HAIR) {
            cost += price(modifier(FACIAL_HAIR));
        }
        if !self.is_current(SKIN) {
            cost += price(modifier(SKIN));
        }
        cost.max(0) as u32
    }

    /// `0x52fc80`: one step of a dial in the barber context.
    fn step(&mut self, ty: usize, backward: bool) {
        let Some(grid) = self.host.grid.as_ref() else {
            return;
        };
        let mut c = Customizer::barber(grid, self.host.class, self.dials);
        let step = if backward { -1 } else { 1 };
        match ty {
            HAIR_STYLE => c.step_hair_style(step),
            HAIR_COLOR => c.step_hair_color(step),
            FACIAL_HAIR => c.step_facial(step),
            _ => c.step_skin(step),
        };
        self.dials = c.dials();
    }

    /// `0x52eda0`: the paid change's four ids, `None` while the look is the real one or a dial has
    /// no style row. The reference sends 0 for a skin without one.
    fn change(&self) -> Option<[u32; 4]> {
        if (0..4).all(|ty| self.is_current(ty)) {
            return None;
        }
        let hair = self.current(HAIR_STYLE)?.id;
        let facial = self.current(FACIAL_HAIR)?.id;
        let skin = self.current(SKIN).map_or(0, |s| s.id);
        Some([
            hair,
            u32::from(self.dials[DIAL_HAIR_COLOR]),
            facial,
            skin,
        ])
    }
}

impl super::UiScript {
    /// Push the barber shop's host facts. The open edge seeds the dials with the real look; while
    /// open, a look field the server changes lands on its dial, as each `PLAYER_BYTES` field's
    /// update handler sets the body to it (`0x6da7f0`, `0x6da870`, `0x6df530`, `0x6df620`).
    pub fn set_barber_host(&mut self, host: BarberHost) {
        if self.model_ref().barber.host == host {
            return;
        }
        let mut model = self.model_mut();
        let b = &mut model.barber;
        if host.open && b.host.open {
            for (i, (old, new)) in b.host.look.iter().zip(host.look).enumerate() {
                if *old != new {
                    b.dials[i] = new;
                }
            }
        } else {
            b.dials = host.look;
        }
        b.host = host;
    }

    /// The look the player's body previews while the shop is open, in dial order.
    pub fn barber_preview(&self) -> Option<[u8; 5]> {
        let model = self.model_ref();
        model.barber.host.open.then_some(model.barber.dials)
    }

    /// The queued intents since the last drain.
    pub fn take_barber_intents(&mut self) -> Vec<BarberIntent> {
        if self.model_ref().barber.intents.is_empty() {
            return Vec::new();
        }
        std::mem::take(&mut self.model_mut().barber.intents)
    }
}

fn with<R>(lua: &Lua, f: impl FnOnce(&BarberState) -> R) -> R {
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    f(&model.barber)
}

/// The style index of a verb's `type`, 1-based: a `u32` truncation minus one, so 0 and a negative
/// wrap past 3 (`0x52e872`, `0x52fdee`).
fn style_index(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<usize> {
    let n = number_arg(lua, v, usage)?;
    Ok((n as u32).wrapping_sub(1) as usize)
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `0x52e850`: nothing while closed or for a type off 1..=4 or a skin with no row; else name,
    // description, price and whether it is the real look (1 or nil). Hair colour has no row and
    // costs half the base; this price is on the untruncated base.
    g.set(
        "GetBarberShopStyleInfo",
        lua.create_function(|lua, args: MultiValue| {
            if !with(lua, |b| b.host.open) {
                return Ok(MultiValue::new());
            }
            let ty = style_index(
                lua,
                args.into_iter().next().unwrap_or(Value::Nil),
                "Usage: GetBarberShopInfo(type)",
            )?;
            if ty > SKIN {
                return Ok(MultiValue::new());
            }
            let row = with(lua, |b| {
                let base = b.host.base_cost;
                let current = b.is_current(ty);
                if ty == HAIR_COLOR {
                    return Some((None, (base * 0.5) as i32, current));
                }
                b.current(ty).map(|s| {
                    (
                        Some((s.name.clone(), s.description.clone())),
                        (base * s.cost_modifier) as i32,
                        current,
                    )
                })
            });
            let Some((names, cost, current)) = row else {
                return Ok(MultiValue::new());
            };
            let (name, description) = match names {
                Some((n, d)) => (
                    Value::String(lua.create_string(n)?),
                    Value::String(lua.create_string(d)?),
                ),
                None => (Value::Nil, Value::Nil),
            };
            Ok(MultiValue::from_vec(vec![
                name,
                description,
                Value::Integer(cost.into()),
                if current {
                    Value::Number(1.0)
                } else {
                    Value::Nil
                },
            ]))
        })?,
    )?;

    // `0x52fdd0`: a type off 1..=4, or the skin with no row, is ignored; `backward` is
    // `StringToBOOL`, absent meaning forward.
    g.set(
        "SetNextBarberShopStyle",
        lua.create_function(|lua, args: MultiValue| {
            let mut args = args.into_iter();
            let ty = style_index(
                lua,
                args.next().unwrap_or(Value::Nil),
                "Usage: SetNextBarberShopStyle(type[, backward])",
            )?;
            let backward = bool_or_default(args.next().as_ref(), false);
            let mut model = crate::script::model_mut_of(lua);
            let b = &mut model.barber;
            if b.host.open && ty <= SKIN && (ty != SKIN || b.can_alter_skin()) {
                b.step(ty, backward);
            }
            Ok(())
        })?,
    )?;

    // `0x52ed60`.
    g.set(
        "GetBarberShopTotalCost",
        lua.create_function(|lua, _: MultiValue| Ok(i64::from(with(lua, |b| b.total_cost()))))?,
    )?;

    // `0x52eef0`: the app holds the price against the purse and sends.
    g.set(
        "ApplyBarberShopStyle",
        lua.create_function(|lua, _: MultiValue| {
            let mut model = crate::script::model_mut_of(lua);
            let b = &mut model.barber;
            if !b.host.open {
                return Ok(());
            }
            if let Some(styles) = b.change() {
                let cost = b.total_cost();
                b.intents.push(BarberIntent::Apply { styles, cost });
            }
            Ok(())
        })?,
    )?;

    // `0x52ed80`: the app runs the close.
    g.set(
        "CancelBarberShop",
        lua.create_function(|lua, _: MultiValue| {
            crate::script::model_mut_of(lua)
                .barber
                .intents
                .push(BarberIntent::Cancel);
            Ok(())
        })?,
    )?;

    // `0x52f900`: the dials back to the real look. Deviation: the reference resets the hair and
    // facial-hair styles and the body but leaves the skin's style on the preview, so a reset
    // skin is still sent; we reset all four.
    g.set(
        "BarberShopReset",
        lua.create_function(|lua, _: MultiValue| {
            let mut model = crate::script::model_mut_of(lua);
            let b = &mut model.barber;
            if b.host.open {
                b.dials = b.host.look;
            }
            Ok(())
        })?,
    )?;

    // `0x52e9b0`, `0x52e730`: the race's tokens, `"NORMAL"` without a row.
    g.set(
        "GetHairCustomization",
        lua.create_function(|lua, _: MultiValue| {
            let token = with(lua, |b| token(&b.host.hair_customization));
            lua.create_string(token)
        })?,
    )?;
    g.set(
        "GetFacialHairCustomization",
        lua.create_function(|lua, _: MultiValue| {
            let token = with(lua, |b| token(&b.host.facial_hair_customization));
            lua.create_string(token)
        })?,
    )?;

    // `0x52e4f0`: a boolean, not 1 or nil.
    g.set(
        "CanAlterSkin",
        lua.create_function(|lua, _: MultiValue| Ok(with(lua, |b| b.can_alter_skin())))?,
    )?;
    Ok(())
}

fn token(t: &str) -> String {
    if t.is_empty() {
        "NORMAL".into()
    } else {
        t.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    /// One hair colour per style, `n` styles and skins, all creatable; no facial-hair rows.
    fn grid(styles: usize, colors: usize, skins: usize) -> GlueCustomizeGrid {
        GlueCustomizeGrid {
            sections: [
                vec![vec![Some(1); skins]],
                vec![vec![Some(1); skins]],
                Vec::new(),
                vec![vec![Some(1); colors]; styles],
                vec![vec![Some(1); skins]],
            ],
            facial_hair_styles: 3,
        }
    }

    fn row(id: u32, ty: u32, data: i32, cost: f32) -> BarberStyle {
        BarberStyle {
            id,
            style_type: ty,
            name: format!("Style {id}"),
            description: String::new(),
            cost_modifier: cost,
            data,
        }
    }

    fn host() -> BarberHost {
        BarberHost {
            open: true,
            class: 1,
            base_cost: 1001.5,
            look: [0, 0, 1, 0, 0],
            styles: vec![
                row(10, 0, 0, 1.0),
                row(11, 0, 1, 1.0),
                row(12, 0, 2, 2.0),
                row(20, 2, 0, 0.75),
                row(21, 2, 1, 0.75),
                row(22, 2, 2, 0.75),
            ],
            grid: Some(grid(3, 4, 2)),
            hair_customization: "HORNS".into(),
            facial_hair_customization: String::new(),
        }
    }

    fn vm() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_barber_host(host());
        s
    }

    #[test]
    fn closed_the_verbs_answer_nothing_and_the_body_previews_nothing() {
        let mut s = UiScript::new().unwrap();
        s.set_barber_host(BarberHost {
            open: false,
            ..host()
        });
        assert_eq!(
            s.eval::<MultiValue>("return GetBarberShopStyleInfo(1)")
                .unwrap()
                .len(),
            0
        );
        assert_eq!(s.eval::<i64>("return GetBarberShopTotalCost()").unwrap(), 0);
        s.run("SetNextBarberShopStyle(1) ApplyBarberShopStyle()")
            .unwrap();
        assert_eq!(s.barber_preview(), None);
        assert!(s.take_barber_intents().is_empty());
    }

    #[test]
    fn the_info_reads_the_current_row_and_prices_it_on_the_level_base() {
        let s = vm();
        let (name, _desc, cost, current): (String, String, i64, Option<f64>) =
            s.eval("return GetBarberShopStyleInfo(1)").unwrap();
        assert_eq!(name, "Style 11");
        assert_eq!(cost, 1001, "1001.5 × 1.0, truncated");
        assert_eq!(current, Some(1.0));
        let (name, desc, cost, current): (Option<String>, Option<String>, i64, Option<f64>) =
            s.eval("return GetBarberShopStyleInfo(2)").unwrap();
        assert_eq!((name, desc), (None, None), "hair colour has no row");
        assert_eq!(cost, 500);
        assert_eq!(current, Some(1.0));
        assert_eq!(
            s.eval::<MultiValue>("return GetBarberShopStyleInfo(4)")
                .unwrap()
                .len(),
            0,
            "no skin rows"
        );
        assert_eq!(
            s.eval::<MultiValue>("return GetBarberShopStyleInfo(0)")
                .unwrap()
                .len(),
            0,
            "type 0 wraps past 3"
        );
        assert!(s.run("GetBarberShopStyleInfo('x')").is_err());
        assert!(!s.eval::<bool>("return CanAlterSkin()").unwrap());
    }

    #[test]
    fn a_step_previews_and_prices_the_change() {
        let s = vm();
        s.run("SetNextBarberShopStyle(1)").unwrap();
        assert_eq!(s.barber_preview(), Some([0, 0, 2, 0, 0]));
        // Style 2 costs 2.0 × the truncated base.
        assert_eq!(s.eval::<i64>("return GetBarberShopTotalCost()").unwrap(), 2002);
        s.run("SetNextBarberShopStyle(1, 1)").unwrap();
        assert_eq!(s.barber_preview(), Some([0, 0, 1, 0, 0]), "backward");
        assert_eq!(s.eval::<i64>("return GetBarberShopTotalCost()").unwrap(), 0);
        // A colour alone costs half the base; a facial change adds its own price.
        s.run("SetNextBarberShopStyle(2) SetNextBarberShopStyle(3)")
            .unwrap();
        assert_eq!(s.barber_preview(), Some([0, 0, 1, 1, 1]));
        assert_eq!(
            s.eval::<i64>("return GetBarberShopTotalCost()").unwrap(),
            500 + 750
        );
        // The skin is ignored without a row.
        s.run("SetNextBarberShopStyle(4)").unwrap();
        assert_eq!(s.barber_preview(), Some([0, 0, 1, 1, 1]));
    }

    #[test]
    fn apply_queues_the_four_ids_only_for_a_changed_look() {
        let mut s = vm();
        s.run("ApplyBarberShopStyle()").unwrap();
        assert!(s.take_barber_intents().is_empty(), "the real look sends nothing");
        s.run("SetNextBarberShopStyle(1) SetNextBarberShopStyle(2) ApplyBarberShopStyle()")
            .unwrap();
        assert_eq!(
            s.take_barber_intents(),
            vec![BarberIntent::Apply {
                styles: [12, 1, 20, 0],
                cost: 2002,
            }]
        );
        s.run("CancelBarberShop()").unwrap();
        assert_eq!(s.take_barber_intents(), vec![BarberIntent::Cancel]);
    }

    #[test]
    fn reset_restores_the_real_look_and_a_server_change_lands_on_its_dial() {
        let mut s = vm();
        s.run("SetNextBarberShopStyle(1) SetNextBarberShopStyle(2) BarberShopReset()")
            .unwrap();
        assert_eq!(s.barber_preview(), Some([0, 0, 1, 0, 0]));
        s.run("SetNextBarberShopStyle(1) SetNextBarberShopStyle(2)")
            .unwrap();
        // The server applies the hair style alone: its dial takes the new real value, the
        // unchanged colour keeps the preview.
        s.set_barber_host(BarberHost {
            look: [0, 0, 2, 0, 0],
            ..host()
        });
        assert_eq!(s.barber_preview(), Some([0, 0, 2, 1, 0]));
        assert_eq!(s.eval::<i64>("return GetBarberShopTotalCost()").unwrap(), 500);
    }

    #[test]
    fn the_tokens_default_to_normal() {
        let s = vm();
        assert_eq!(
            s.eval::<String>("return GetHairCustomization()").unwrap(),
            "HORNS"
        );
        assert_eq!(
            s.eval::<String>("return GetFacialHairCustomization()").unwrap(),
            "NORMAL"
        );
    }

    #[test]
    fn a_barber_only_row_is_offered_in_the_chair() {
        // Hair style 1 is flagged barber-only (`0x2`).
        let mut h = host();
        h.look = [0, 0, 0, 0, 0];
        if let Some(g) = h.grid.as_mut() {
            g.sections[3] = vec![vec![Some(1); 4], vec![Some(2); 4], vec![Some(1); 4]];
        }
        let mut s = UiScript::new().unwrap();
        s.set_barber_host(h);
        s.run("SetNextBarberShopStyle(1)").unwrap();
        assert_eq!(s.barber_preview(), Some([0, 0, 1, 0, 0]));
    }
}
