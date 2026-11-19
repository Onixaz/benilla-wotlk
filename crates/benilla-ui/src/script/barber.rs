//! The barber shop's nine globals (registrar `0xacc790`). The chair opens it with
//! `SMSG_ENABLE_BARBER_SHOP`; while open, the player's own body wears the dials the verbs step
//! through the race's `CharSections` grid in the barber context, and each dial's style is the
//! `BarberShopStyle` row whose `Data` it holds (`0x52f760`). Every verb reads the shop, our look and
//! our level at its call ([`BarberRead`]) and writes the dials, the send and the close inside it.

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

/// The barber shop at the call, with our player object held.
#[derive(Clone, Copy, Debug)]
pub struct BarberRead<'a> {
    /// The open flag (`[0xbd19b8]`): set by `SMSG_ENABLE_BARBER_SHOP`, cleared by the close.
    pub open: bool,
    pub class: u8,
    /// `gtBarberShopCostBase` at our level.
    pub base_cost: f32,
    /// The real look off `PLAYER_BYTES`/`PLAYER_BYTES_2`, in dial order.
    pub look: [u8; 5],
    /// The look our body previews, in dial order.
    pub dials: [u8; 5],
    /// The skin style's look value (`[0xbd19b4]`): set by the open and a skin step, kept by a reset (`0x52f7d0`).
    pub skin_style: u8,
    /// The rows of our race and sex, in file order (`0x52fe90`).
    pub styles: &'a [BarberStyle],
    /// Our race and sex's `CharSections` grid, which the dials walk.
    pub grid: Option<&'a GlueCustomizeGrid>,
    /// Our coinage, which the apply holds the price against (`0x52ee47`).
    pub money: u32,
}

impl BarberRead<'_> {
    /// The style row of `ty` whose data is `value`, the first in file order.
    fn style(&self, ty: usize, value: u8) -> Option<&BarberStyle> {
        self.styles
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

    /// The value a type's style stands on: the skin's own pointer, every other type's dial.
    fn value(&self, ty: usize) -> u8 {
        if ty == SKIN {
            self.skin_style
        } else {
            self.dials[Self::dial(ty)]
        }
    }

    /// The current style of a type with rows (`0x52e490`).
    fn current(&self, ty: usize) -> Option<&BarberStyle> {
        self.style(ty, self.value(ty))
    }

    /// `0x52e790`: the type still stands on the real look.
    fn is_current(&self, ty: usize) -> bool {
        self.look[Self::dial(ty)] == self.value(ty)
    }

    /// `CanAlterSkin()` (`0x52e4f0`): the skin has a style row.
    fn can_alter_skin(&self) -> bool {
        self.current(SKIN).is_some()
    }

    /// `0x52eba0`: the hair style's price, else half the base for a colour alone, plus the facial
    /// hair's and the skin's, each on the base truncated to copper first.
    fn total_cost(&self) -> u32 {
        if !self.open {
            return 0;
        }
        let base = self.base_cost as i32 as f32;
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

    /// `0x52fc80`: the dials after one step in the barber context.
    fn step(&self, ty: usize, backward: bool) -> Option<[u8; 5]> {
        let mut c = Customizer::barber(self.grid?, self.class, self.dials);
        let step = if backward { -1 } else { 1 };
        match ty {
            HAIR_STYLE => c.step_hair_style(step),
            HAIR_COLOR => c.step_hair_color(step),
            FACIAL_HAIR => c.step_facial(step),
            _ => c.step_skin(step),
        };
        Some(c.dials())
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
        Some([hair, u32::from(self.dials[DIAL_HAIR_COLOR]), facial, skin])
    }
}

/// `f` over the shop at the call; `None` with no player object or no view lent.
fn read<R>(f: impl FnOnce(&BarberRead<'_>) -> R) -> Option<R> {
    super::read_live(|l| l.barber().map(|b| f(&b))).flatten()
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
            if read(|b| b.open) != Some(true) {
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
            let row = read(|b| {
                let base = b.base_cost;
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
            })
            .flatten();
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

    // `0x52fdd0`: a type off 1..=4, or the skin with no row, is ignored; `backward` is read as a
    // boolean, absent meaning forward. The step lands on our body inside the call (`0x52fc80`).
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
            let next = read(|b| {
                (b.open && ty <= SKIN && (ty != SKIN || b.can_alter_skin()))
                    .then(|| b.step(ty, backward))
                    .flatten()
            })
            .flatten();
            if let Some(dials) = next {
                super::write_live(|l| {
                    l.set_barber_dials(dials);
                    // Only the skin's step moves its style pointer (`0x52fd94`).
                    if ty == SKIN {
                        l.set_barber_skin_style(dials[DIAL_SKIN]);
                    }
                });
            }
            Ok(())
        })?,
    )?;

    // `0x52ed60`.
    g.set(
        "GetBarberShopTotalCost",
        lua.create_function(|_, _: MultiValue| {
            Ok(i64::from(read(|b| b.total_cost()).unwrap_or(0)))
        })?,
    )?;

    // `0x52eda0`: a changed look priced over our coinage shows ERR_NOT_ENOUGH_MONEY with no packet
    // (`0x52ee47`); else `CMSG_ALTER_APPEARANCE` goes out inside the call.
    g.set(
        "ApplyBarberShopStyle",
        lua.create_function(|lua, _: MultiValue| {
            let Some(Some((styles, affordable))) = read(|b| {
                b.open
                    .then(|| b.change().map(|s| (s, b.total_cost() <= b.money)))
                    .flatten()
            }) else {
                return Ok(());
            };
            if affordable {
                super::write_live(|l| l.send_alter_appearance(styles));
            } else {
                super::ui_errors::show_now(lua, "ERR_NOT_ENOUGH_MONEY");
            }
            Ok(())
        })?,
    )?;

    // `0x52ed80` -> `0x52e9f0`: an open shop closes and fires `BARBER_SHOP_CLOSE` inside the call.
    g.set(
        "CancelBarberShop",
        lua.create_function(|lua, _: MultiValue| {
            if super::write_live(|l| l.close_barber_shop()) == Some(true) {
                super::tick::fire_event_into(lua, "BARBER_SHOP_CLOSE", Vec::new());
            }
            Ok(())
        })?,
    )?;

    // `0x52f900` -> `0x52f7d0`: the hair and facial-hair styles and the body back to the real look (`0x52e630`); the skin's style pointer stays.
    g.set(
        "BarberShopReset",
        lua.create_function(|_, _: MultiValue| {
            if let Some(Some(look)) = read(|b| b.open.then_some(b.look)) {
                super::write_live(|l| l.set_barber_dials(look));
            }
            Ok(())
        })?,
    )?;

    // `0x52e9b0`, `0x52e9d0`: our race row's token at the call as it is, `"NORMAL"` with no player object or row (`0x52e6d0`, `0x52e730`).
    g.set(
        "GetHairCustomization",
        lua.create_function(|lua, _: MultiValue| {
            let t = super::read_live(|l| l.hair_customization().map(str::to_owned)).flatten();
            lua.create_string(t.unwrap_or_else(|| "NORMAL".into()))
        })?,
    )?;
    g.set(
        "GetFacialHairCustomization",
        lua.create_function(|lua, _: MultiValue| {
            let t =
                super::read_live(|l| l.facial_hair_customization().map(str::to_owned)).flatten();
            lua.create_string(t.unwrap_or_else(|| "NORMAL".into()))
        })?,
    )?;

    // `0x52e4f0`: a boolean, not 1 or nil.
    g.set(
        "CanAlterSkin",
        lua.create_function(|_, _: MultiValue| Ok(read(|b| b.can_alter_skin()).unwrap_or(false)))?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::live::{TestBarber, TestView};
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

    /// An open shop over our look `[0, 0, 1, 0, 0]` with a full purse.
    fn view() -> TestView {
        TestView {
            barber: Some(TestBarber {
                open: true,
                class: 1,
                base_cost: 1001.5,
                look: [0, 0, 1, 0, 0],
                dials: [0, 0, 1, 0, 0],
                skin_style: 0,
                styles: vec![
                    row(10, 0, 0, 1.0),
                    row(11, 0, 1, 1.0),
                    row(12, 0, 2, 2.0),
                    row(20, 2, 0, 0.75),
                    row(21, 2, 1, 0.75),
                    row(22, 2, 2, 0.75),
                ],
                grid: Some(grid(3, 4, 2)),
                money: 1_000_000,
            }),
            hair_tokens: Some(("HORNS".into(), String::new())),
            ..Default::default()
        }
    }

    fn dials(v: &TestView) -> [u8; 5] {
        v.barber.as_ref().unwrap().dials
    }

    /// A chunk that logs `event`'s firings into `log` around `body`.
    fn logged(event: &str, body: &str) -> String {
        format!(
            "local f = CreateFrame('Frame') local log = '' f:RegisterEvent('{event}') \
             f:SetScript('OnEvent', function(_, e, m) log = log .. (m or e) .. ' ' end) {body} return log"
        )
    }

    #[test]
    fn closed_the_verbs_answer_nothing_and_change_nothing() {
        let mut s = UiScript::new().unwrap();
        let mut v = view();
        v.barber.as_mut().unwrap().open = false;
        s.with_live(&mut v, |s| {
            assert_eq!(
                s.eval::<MultiValue>("return GetBarberShopStyleInfo(1)")
                    .unwrap()
                    .len(),
                0
            );
            assert_eq!(s.eval::<i64>("return GetBarberShopTotalCost()").unwrap(), 0);
            s.run("SetNextBarberShopStyle(1) ApplyBarberShopStyle() BarberShopReset()")
                .unwrap();
            let log = s
                .eval::<String>(&logged("BARBER_SHOP_CLOSE", "CancelBarberShop()"))
                .unwrap();
            assert_eq!(log, "", "a closed shop's cancel fires nothing");
        });
        assert_eq!(dials(&v), [0, 0, 1, 0, 0]);
        assert!(v.barber_sent.is_empty());
    }

    #[test]
    fn the_info_reads_the_current_row_and_prices_it_on_the_level_base() {
        let mut s = UiScript::new().unwrap();
        let mut v = view();
        s.with_live(&mut v, |s| {
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
        });
    }

    /// Each step lands on the body inside its call, so the next verb in the same chunk prices it.
    #[test]
    fn a_step_writes_the_body_in_its_call_and_the_next_verb_prices_it() {
        let mut s = UiScript::new().unwrap();
        let mut v = view();
        s.with_live(&mut v, |s| {
            assert_eq!(
                s.eval::<i64>("SetNextBarberShopStyle(1) return GetBarberShopTotalCost()")
                    .unwrap(),
                2002,
                "style 2 costs 2.0 × the truncated base"
            );
        });
        assert_eq!(dials(&v), [0, 0, 2, 0, 0]);
        s.with_live(&mut v, |s| s.run("SetNextBarberShopStyle(1, 1)").unwrap());
        assert_eq!(dials(&v), [0, 0, 1, 0, 0], "backward");
        s.with_live(&mut v, |s| {
            assert_eq!(
                s.eval::<i64>(
                    "SetNextBarberShopStyle(2) SetNextBarberShopStyle(3) return GetBarberShopTotalCost()"
                )
                .unwrap(),
                500 + 750,
                "a colour alone costs half the base; a facial change adds its own price"
            );
            s.run("SetNextBarberShopStyle(4)").unwrap();
        });
        assert_eq!(
            dials(&v),
            [0, 0, 1, 1, 1],
            "the skin is ignored without a row"
        );
    }

    #[test]
    fn apply_sends_the_four_ids_in_its_call_only_for_a_changed_affordable_look() {
        let mut s = UiScript::new().unwrap();
        let mut v = view();
        s.with_live(&mut v, |s| s.run("ApplyBarberShopStyle()").unwrap());
        assert!(v.barber_sent.is_empty(), "the real look sends nothing");
        s.with_live(&mut v, |s| {
            s.run("SetNextBarberShopStyle(1) SetNextBarberShopStyle(2) ApplyBarberShopStyle()")
                .unwrap()
        });
        assert_eq!(v.barber_sent, vec![[12, 1, 20, 0]]);
        // 2002 over a purse of 2001: the line shows in the call, with no packet (`0x52ee47`).
        v.barber.as_mut().unwrap().money = 2001;
        s.run("ERR_NOT_ENOUGH_MONEY = 'Not enough money'").unwrap();
        let log = s.with_live(&mut v, |s| {
            s.eval::<String>(&logged(
                "UI_ERROR_MESSAGE",
                "ApplyBarberShopStyle() log = log .. 'after'",
            ))
            .unwrap()
        });
        assert_eq!(log, "Not enough money after");
        assert_eq!(v.barber_sent.len(), 1);
    }

    #[test]
    fn cancel_closes_and_fires_inside_its_call_once() {
        let mut s = UiScript::new().unwrap();
        let mut v = view();
        let log = s.with_live(&mut v, |s| {
            s.eval::<String>(&logged(
                "BARBER_SHOP_CLOSE",
                "CancelBarberShop() log = log .. GetBarberShopTotalCost() .. ' ' CancelBarberShop()",
            ))
            .unwrap()
        });
        assert_eq!(
            log, "BARBER_SHOP_CLOSE 0 ",
            "closed in the call; a second cancel is silent"
        );
        assert!(!v.barber.as_ref().unwrap().open);
    }

    #[test]
    fn reset_restores_the_real_look_and_a_server_change_is_read_at_the_call() {
        let mut s = UiScript::new().unwrap();
        let mut v = view();
        s.with_live(&mut v, |s| {
            s.run("SetNextBarberShopStyle(1) SetNextBarberShopStyle(2) BarberShopReset()")
                .unwrap()
        });
        assert_eq!(dials(&v), [0, 0, 1, 0, 0]);
        s.with_live(&mut v, |s| {
            s.run("SetNextBarberShopStyle(1) SetNextBarberShopStyle(2)")
                .unwrap()
        });
        // The server applies the hair style: its watch sets the look and the dial alike.
        let b = v.barber.as_mut().unwrap();
        b.look = [0, 0, 2, 0, 0];
        b.dials[2] = 2;
        let cost = s.with_live(&mut v, |s| {
            s.eval::<i64>("return GetBarberShopTotalCost()").unwrap()
        });
        assert_eq!(cost, 500, "the colour alone is left to pay");
    }

    #[test]
    fn reset_keeps_the_skin_style_on_its_step() {
        // `0x52f7d0` walks the hair and facial-hair pointers back and resets the body (`0x52e630`), never `[0xbd19b4]`.
        let mut v = view();
        let b = v.barber.as_mut().unwrap();
        b.styles.push(row(30, 3, 0, 1.0));
        b.styles.push(row(31, 3, 1, 1.0));
        let mut s = UiScript::new().unwrap();
        let (current, cost) = s.with_live(&mut v, |s| {
            s.run("SetNextBarberShopStyle(4) BarberShopReset()")
                .unwrap();
            (
                s.eval::<Option<f64>>("return (select(4, GetBarberShopStyleInfo(4)))")
                    .unwrap(),
                s.eval::<i64>("return GetBarberShopTotalCost()").unwrap(),
            )
        });
        let b = v.barber.as_ref().unwrap();
        assert_eq!((b.dials, b.skin_style), ([0, 0, 1, 0, 0], 1));
        assert_eq!(current, None, "the skin still stands on its step");
        assert_eq!(cost, 1001, "and is still priced");
        s.with_live(&mut v, |s| s.run("ApplyBarberShopStyle()").unwrap());
        assert_eq!(v.barber_sent, vec![[11, 0, 20, 31]], "and sent");
    }

    #[test]
    fn the_tokens_are_the_rows_as_they_are_and_normal_with_none() {
        let mut s = UiScript::new().unwrap();
        let mut v = view();
        s.with_live(&mut v, |s| {
            assert_eq!(
                s.eval::<String>("return GetHairCustomization()").unwrap(),
                "HORNS"
            );
            assert_eq!(
                s.eval::<String>("return GetFacialHairCustomization()")
                    .unwrap(),
                "",
                "an empty row token is passed as it is (`0x52e730`)"
            );
        });
        assert_eq!(
            s.eval::<String>("return GetHairCustomization()").unwrap(),
            "NORMAL"
        );
        let mut none = TestView::default();
        s.with_live(&mut none, |s| {
            assert_eq!(
                s.eval::<String>("return GetFacialHairCustomization()")
                    .unwrap(),
                "NORMAL"
            );
        });
    }

    #[test]
    fn a_barber_only_row_is_offered_in_the_chair() {
        // Hair style 1 is flagged barber-only (`0x2`).
        let mut v = view();
        let b = v.barber.as_mut().unwrap();
        b.look = [0; 5];
        b.dials = [0; 5];
        if let Some(g) = b.grid.as_mut() {
            g.sections[3] = vec![vec![Some(1); 4], vec![Some(2); 4], vec![Some(1); 4]];
        }
        let mut s = UiScript::new().unwrap();
        s.with_live(&mut v, |s| s.run("SetNextBarberShopStyle(1)").unwrap());
        assert_eq!(dials(&v), [0, 0, 1, 0, 0]);
    }
}
