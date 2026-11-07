//! The dressing room: the `DressUpModel` verbs and the intent queue behind them and behind
//! `SetUnit`/`RefreshUnit` on a dressing-room pane. The reference edits a clone of the unit's
//! model; here the app composes the look from the intents in order, which matters: on a closed
//! window `DressUpItemLink` resets, then tries on (`DressUpFrame.lua:2-10`).

use mlua::{Lua, Table, Value};

use super::item_link::{reference_atoi, ItemLinkFields};
use super::item_search::{find_item, ItemQuery};
use super::object::frame_handle_of;
use crate::widget::FrameKind;

/// Registry key of the `DressUpModel` method table, its own three; the rest come by the chain.
pub(super) const REG_DRESSUPMODEL_METHODS: &str = "__benilla_dressupmodel_methods";

/// One queued dressing-room intent, applied in order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DressUpIntent {
    /// `SetUnit`, `RefreshUnit` and `Dress` (the Reset button): the reference's rebuild from the
    /// unit, dropping every substitution.
    Dress,
    /// `Undress()`: bodyslots `0..0xb` cleared, worn and tried-on alike; the hands keep what they
    /// hold.
    Undress,
    /// `TryOn(item)`: the item in its `InventoryType`'s slot, with no class, level or proficiency
    /// check, and the enchant ids a link names, the first with a visual glowing an item with no
    /// visual of its own (`0x598830`).
    TryOn(u32, [i32; 4]),
    /// The window was hidden, so the booth empties. The reference keeps its state while hidden,
    /// but the next `DressUpItemLink` re-issues `SetUnit("player")`, so that state is never seen.
    Close,
}

impl super::UiScript {
    /// Drain the dressing room's intents, oldest first; the app applies them in order.
    pub fn take_dressup_intents(&mut self) -> Vec<DressUpIntent> {
        drain_field!(self, dressup_intents)
    }
}

fn queue(lua: &Lua, intent: DressUpIntent) {
    crate::script::model_mut_of(lua)
        .dressup_intents
        .push(intent);
}

/// Queue the rebuild when `PlayerModel`'s `SetUnit`/`RefreshUnit` lands on a `DressUpModel`.
pub(super) fn redress_if_dressup(lua: &Lua, this: &Table) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let is_dressup = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data")
        .arena
        .frame(h)
        .is_some_and(|f| f.kind == FrameKind::DressUpModel);
    if is_dressup {
        queue(lua, DressUpIntent::Dress);
    }
    Ok(())
}

/// `TryOn`'s string forms: `None` is the refusal.
fn try_on_text(lua: &Lua, text: &str) -> Option<DressUpIntent> {
    if text.contains("item:") {
        let (id, f) = ItemLinkFields::parse(text)?;
        return Some(DressUpIntent::TryOn(id, enchant_ids(&f)));
    }
    if let Some(at) = text.find("enchant:") {
        let spell = u32::try_from(reference_atoi(&text[at + 8..])).ok()?;
        let source = crate::script::model_mut_of(lua).spell_info.clone()?;
        let e = source.spell_stat_effects(spell)?;
        let product = (e.attributes & 0x20 != 0 && e.item_type != 0).then_some(e.item_type)?;
        return Some(DressUpIntent::TryOn(product, [0; 4]));
    }
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    let held = find_item(&model, &ItemQuery::Name(text.to_string()), 0)?;
    let fields = held.link_fields().unwrap_or_default();
    Some(DressUpIntent::TryOn(held.item_id, enchant_ids(&fields)))
}

fn enchant_ids(f: &ItemLinkFields) -> [i32; 4] {
    f.enchant_slots().map(|(_, id)| id)
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    m.set(
        "Undress",
        lua.create_function(|lua, this: Table| {
            frame_handle_of(lua, &this)?;
            queue(lua, DressUpIntent::Undress);
            Ok(())
        })?,
    )?;

    // The Reset button (`DressUpFrame.xml:182`).
    m.set(
        "Dress",
        lua.create_function(|lua, this: Table| {
            frame_handle_of(lua, &this)?;
            queue(lua, DressUpIntent::Dress);
            Ok(())
        })?,
    )?;

    // TryOn(itemID | "link" | "name") (`0x598830`): a number is the item id, an `item:` link its
    // item with the enchants it names, an `enchant:` link the product of its trade-skill spell and
    // a name an item the player holds; anything else answers ERR_NOT_EQUIPPABLE.
    m.set(
        "TryOn",
        lua.create_function(|lua, (this, item): (Table, Value)| {
            frame_handle_of(lua, &this)?;
            // `lua_isnumber` first, so a digit string is an id.
            if let Some(n) = lua.coerce_number(item.clone())? {
                if let Ok(id) = u32::try_from(n.trunc() as i64) {
                    if id != 0 {
                        queue(lua, DressUpIntent::TryOn(id, [0; 4]));
                    }
                }
                return Ok(());
            }
            let intent = match &item {
                Value::String(s) => try_on_text(lua, &s.to_string_lossy()),
                _ => None,
            };
            match intent {
                Some(intent) => queue(lua, intent),
                None => crate::script::model_mut_of(lua)
                    .ui_errors
                    .push("ERR_NOT_EQUIPPABLE"),
            }
            Ok(())
        })?,
    )?;

    lua.set_named_registry_value(REG_DRESSUPMODEL_METHODS, m)
}

#[cfg(test)]
mod tests {
    use super::DressUpIntent;
    use crate::script::UiScript;

    fn room() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        s.run(r#"dm = CreateFrame("DressUpModel", "DM", UIParent)"#)
            .unwrap();
        s
    }

    #[test]
    fn a_dress_up_model_is_a_player_model_plus_three_and_the_chain_runs_one_way() {
        let s = room();
        s.run(r#"pm = CreateFrame("PlayerModel", "PMOnly", UIParent)"#)
            .unwrap();
        for verb in [
            "TryOn",
            "Dress",
            "Undress",
            "SetUnit",
            "RefreshUnit",
            "SetRotation",
            "SetCamera",
        ] {
            assert_eq!(
                s.eval::<String>(&format!("return type(DM.{verb})"))
                    .unwrap(),
                "function",
                "DressUpModel answers {verb}"
            );
        }
        for verb in ["TryOn", "Dress", "Undress"] {
            assert_eq!(
                s.eval::<String>(&format!("return type(PMOnly.{verb})"))
                    .unwrap(),
                "nil",
                "a PlayerModel must NOT answer {verb}"
            );
        }
        assert_eq!(
            s.eval::<String>("return DM:GetObjectType()").unwrap(),
            "DressUpModel"
        );
        for t in ["DressUpModel", "PlayerModel", "Model", "Frame", "Region"] {
            assert!(
                s.eval::<bool>(&format!("return DM:IsObjectType(\"{t}\") == 1"))
                    .unwrap(),
                "IsObjectType({t})"
            );
        }
    }

    /// Stock `DressUpItemLink` passes the link itself: a digit string is an id, a link its item's,
    /// and nothing it can read answers ERR_NOT_EQUIPPABLE (`0x598830`).
    #[test]
    fn the_verbs_queue_intents_in_order_and_try_on_reads_its_argument_like_the_client() {
        let mut s = room();
        s.run(
            r#"
            DM:SetUnit("player")
            DM:TryOn("117")
            DM:TryOn(1234.7)
            DM:Undress()
            DM:RefreshUnit()
            DM:Dress()
            DM:TryOn("|cffffffff|Hitem:117|h[Tough Jerky]|h|r")
            DM:TryOn(0)
            DM:TryOn(nil)
            DM:TryOn(-5)
            "#,
        )
        .unwrap();
        assert_eq!(
            s.take_dressup_intents(),
            vec![
                DressUpIntent::Dress,
                DressUpIntent::TryOn(117, [0; 4]),
                DressUpIntent::TryOn(1234, [0; 4]),
                DressUpIntent::Undress,
                DressUpIntent::Dress,
                DressUpIntent::Dress,
                DressUpIntent::TryOn(117, [0; 4]),
            ]
        );
        assert_eq!(
            s.take_ui_errors(),
            vec!["ERR_NOT_EQUIPPABLE"],
            "the nil argument"
        );
        // The same two PlayerModel verbs on a paper-doll pane queue nothing.
        s.run(r#"pm = CreateFrame("PlayerModel", "PMOnly", UIParent) pm:SetUnit("player") pm:RefreshUnit()"#)
            .unwrap();
        assert!(s.take_dressup_intents().is_empty());
        // Its yaw is its own facing, read by name like every other pane's.
        s.run("DM:SetRotation(0.61)").unwrap();
        assert!((s.model_pane_facing("DM") - 0.61).abs() < 1e-6);
    }

    /// `IsDressableItem` (`0x517530`): a cached template with an InventoryType, or a recipe whose
    /// product is one, by id, link or cached name; nil otherwise.
    #[test]
    fn is_dressable_item_reads_the_template_or_the_recipe_product() {
        let mut s = room();
        let template = |inventory_type, recipe_creates| crate::script::ItemTemplateView {
            inventory_type,
            recipe_creates,
            ..Default::default()
        };
        s.set_item_template(
            7000,
            crate::script::ItemTemplateView {
                name: "Old Loop".into(),
                ..template(11, 0)
            },
        );
        s.set_item_template(7001, template(0, 7000));
        s.set_item_template(7002, template(0, 9999));
        s.set_item_template(7003, template(0, 0));
        assert!(s
            .eval::<bool>(
                r#"return IsDressableItem(7000) == 1 and IsDressableItem(7001) == 1
                    and IsDressableItem(7002) == nil and IsDressableItem(7003) == nil
                    and IsDressableItem(9999) == nil
                    and IsDressableItem("|cffa335ee|Hitem:7000:0:0:0|h[Old Loop]|h|r") == 1
                    and IsDressableItem("Old Loop") == nil and IsDressableItem("Nobody") == nil"#
            )
            .unwrap());
        assert!(
            s.take_item_stat_asks().contains(&9999),
            "an uncached product asks, so the next click finds it"
        );
    }
}
