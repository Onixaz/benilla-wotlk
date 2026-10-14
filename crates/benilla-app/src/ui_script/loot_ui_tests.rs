//! The stock 3.3.5a loot window (`LootFrame.lua`) and its `LOOT_BIND` popup (`UIParent.lua`,
//! `StaticPopup.lua`) on the production load, over a coin pile, an ordinary item and a locked one.

use benilla_ui::script::{LootRow, LootState, ScriptValue};

/// Errors from the loot code itself; the production load carries unrelated 3.3.5a gaps.
fn loot_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| {
            ["LootFrame", "LootButton", "LOOT_BIND"]
                .iter()
                .any(|k| e.contains(k))
        })
        .collect()
}

fn row(name: &str, quality: u32, locked: bool) -> Option<LootRow> {
    Some(LootRow {
        name: Some(name.into()),
        texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
        quantity: 1,
        quality: Some(quality),
        is_coin: false,
        item_id: 117,
        link: None,
        random_property_id: 0,
        locked,
    })
}

/// Rows 1-3: coin, an epic, a locked white.
fn coin_epic_and_locked() -> LootState {
    LootState {
        rows: vec![
            Some(LootRow {
                name: Some("4 Copper".into()),
                texture: Some("Interface\\Icons\\INV_Misc_Coin_06".into()),
                quantity: 0,
                quality: Some(0),
                is_coin: true,
                ..Default::default()
            }),
            row("Felstriker", 4, false),
            row("Tough Jerky", 1, true),
        ],
        ..Default::default()
    }
}

/// A row click reaches `LootSlot` (`LootFrame.lua:221`), the `LOOT_BIND` popup's accept reaches
/// `ConfirmLootSlot` (`StaticPopup.lua:1519`), and only the locked row is tinted red.
#[test]
fn the_stock_loot_window_takes_through_loot_slot_and_confirms_through_confirm_loot_slot() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("loot", false, "", |_| {});
    let _ = s.take_errors();
    s.set_loot(Some(coin_epic_and_locked()));
    s.fire_event("LOOT_OPENED", Vec::<ScriptValue>::new());
    let errors = loot_errors(s.take_errors());
    assert!(errors.is_empty(), "loot errors: {errors:#?}");

    s.run(
        r#"
        assert(LootFrame:IsShown(), "the window opens")
        for i = 1, 3 do assert(_G["LootButton" .. i]:IsShown(), "row " .. i .. " shows") end
        local function red(i)
            local r, g, b = _G["LootButton" .. i .. "IconTexture"]:GetVertexColor()
            return g < 0.5 and b < 0.5
        end
        assert(not red(1), "the coin is not locked")
        assert(not red(2), "an ordinary item is not locked")
        assert(red(3), "the locked row is red")
        "#,
    )
    .unwrap();

    // The stock row click: `LootButton_OnClick` → `LootSlot(self.slot)`.
    s.run("LootButton2:Click('LeftButton') LootButton1:Click('LeftButton')")
        .unwrap();
    assert_eq!(s.take_loot_picks(), vec![2, 1], "each click is a take");
    assert!(s.take_loot_confirms().is_empty());

    // The app's deferral fires `LOOT_BIND_CONFIRM` with the row; Okay hands it back.
    s.fire_event("LOOT_BIND_CONFIRM", vec![ScriptValue::Int(2)]);
    s.run(
        r#"
        local dialog = StaticPopup_Visible("LOOT_BIND")
        assert(dialog, "the bind popup shows")
        _G[dialog .. "Button1"]:Click()
        "#,
    )
    .unwrap();
    assert_eq!(s.take_loot_confirms(), vec![2], "Okay confirms the row");
    assert!(s.take_loot_picks().is_empty(), "and takes nothing else");

    let errors = loot_errors(s.take_errors());
    assert!(errors.is_empty(), "loot errors: {errors:#?}");
}
