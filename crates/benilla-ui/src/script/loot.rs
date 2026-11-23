//! The loot-window bindings. The window is the client's (`[0xbfa8d8]` the object, `[0xbfa8d0]` the
//! coins, the 18-row table at `0xbfa690`), read at each call through the live view
//! ([`super::LiveState::loot_window`]); a coin row comes first while the window has coins. Its events
//! fire from the packet handlers that move it. `slot` is 1-based; the app maps a row to the money
//! intent or the item's wire slot, which Lua never sees.
//!
//! `LootSlot` is the take: 12340's binding `0x589520` calls the take dispatcher `0x589140`, and the
//! stock row click ends in it (`LootFrame.lua:221`). `ConfirmLootSlot` (`0x589590` → `0x589300`)
//! completes a LOOT_BIND confirm and refuses every slot but the pending one. Master loot's dropdown
//! opens from the app, on a taken row whose wire `slot_type` is `MASTER`, as the dispatcher
//! branches on that byte.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;
use super::model::Model;

/// The quality `GetLootSlotInfo` answers on an item-cache miss: the reference's -1, never nil.
/// Stock `UIParent.lua:66` builds `ITEM_QUALITY_COLORS` from -1, and `LootFrame_Update` indexes it
/// unguarded (`LootFrame.lua:82`), so a nil would raise on a cold cache.
const CACHE_MISS_QUALITY: i64 = -1;

/// The row text on that miss: the reference leaves its buffer empty with no cache record, and the
/// binding pushes that empty string.
const CACHE_MISS_NAME: &str = "";

/// The icon for a row with an item but no display icon: `INV_Misc_QuestionMark`. A slot with no
/// item gets nil instead: the reference returns NULL on its three guards, which `lua_pushstring`
/// pushes as nil.
const MISSING_ICON: &str = "Interface\\Icons\\INV_Misc_QuestionMark";

/// The quality of a slot with no item (past the table, slot 0, no window, the coin slot once its
/// coins are taken): the reference's guards return 0 (`0x588310`). A cleared item row takes -1
/// instead: its itemId is 0, which the reference turns into a NULL record, the cache-miss arm.
const NO_SLOT_QUALITY: i64 = 0;

/// One loot-window row, resolved at the call; its 1-based slot is its place in [`LootState::rows`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LootRow {
    /// Item name; `None` while in flight, answered as [`CACHE_MISS_NAME`], and for the coin row.
    pub name: Option<String>,
    /// Icon path; `None` without a display icon, answered as [`MISSING_ICON`].
    pub texture: Option<String>,
    /// Stack size looted; 1 for the coin row.
    pub quantity: u32,
    /// Quality 0..6; `None` while the template is in flight, answered as [`CACHE_MISS_QUALITY`].
    pub quality: Option<u32>,
    /// The synthesized coin pile (`LootSlotIsCoin` true, `LootSlotIsItem` false).
    pub is_coin: bool,
    /// The coin row's copper, its text composed at the call from the global strings (`0x588430`);
    /// 0 for an item row.
    pub coins: u32,
    /// The tooltip store's key, 0 for the coin row.
    pub item_id: u32,
    /// The wire `slot_type` is `LOCKED` (3): `GetLootSlotInfo`'s fifth return, which tints the row
    /// red (`LootFrame.lua:102`).
    pub locked: bool,
    /// `GetLootSlotLink`'s answer; `None` for the coin row and while the template is in flight.
    pub link: Option<String>,
    /// The drop's random-suffix roll (`randomPropertyId`, 0 unrolled), resolved by the tooltip
    /// against [`super::ItemTables::random_properties`]. A loot slot is no item object, so this
    /// is its only enchant source (`SetLootItem` copies it from the loot record's `+0x14`).
    pub random_property_id: u32,
}

/// One open loot window as a verb reads it; `None` means no loot is open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LootState {
    /// The display slots: the coin row first when [`Self::coin_slot`], then the item table. A
    /// looted slot is `None`: neither item nor coin, so `LootFrame_Update` hides its button in
    /// place (`LootFrame.lua:80`). The live view lists all 18 table rows, an empty one `None`.
    pub rows: Vec<Option<LootRow>>,
    /// Display slot 1 is the coin's (`[0xbfa8d0]` nonzero), kept once its coins are taken
    /// (`0x589184` stores -1), so the item rows keep their slots.
    pub coin_slot: bool,
    /// `IsFishingLoot` (wire `loot_type` 3): the reel-in sound and portrait (`LootFrame.lua:137`).
    pub fishing: bool,
    /// The names `GetMasterLootCandidate(i)` answers by 1-based slot where no view is lent.
    pub master_candidates: Vec<Option<String>>,
    /// The loot source resolves as a unit object (typemask 8): a corpse, not a chest, a bobber or
    /// an item. `SetLootPortrait` bakes it under the token [`LOOT_PORTRAIT_UNIT`].
    pub source_unit: bool,
    /// That unit's level, 0 for any other source: the requester a scaling item shows at in the
    /// loot tooltip (`0x62e638` passes the source, `0x61e7bf` reads its level).
    pub source_level: u32,
}

impl LootState {
    /// `GetNumLootItems` (`0x588190`): through the last table row with an item, plus the coin slot,
    /// so a trailing looted row drops off the count.
    fn count(&self) -> usize {
        let coin = usize::from(self.coin_slot);
        let items = self.rows.get(coin..).unwrap_or_default();
        coin + items.iter().rposition(Option::is_some).map_or(0, |i| i + 1)
    }
}

/// The portrait key `SetLootPortrait` binds a texture to, which the app bakes from the loot
/// source. Upper case, so no `SetPortraitTexture` token, which is lowercased, can name it.
pub const LOOT_PORTRAIT_UNIT: &str = "LOOT";

/// The open loot at the call; with no view lent, a fixture's.
pub(crate) fn window(model: &Model) -> Option<LootState> {
    if let Some(window) = super::read_live(|l| l.loot_window()).flatten() {
        return window;
    }
    #[cfg(any(test, feature = "fixtures"))]
    {
        model.loot.clone()
    }
    #[cfg(not(any(test, feature = "fixtures")))]
    {
        let _ = model;
        None
    }
}

/// The looted object (`[0xbfa8d8]`), 0 with no loot open; with no view lent, a fixture's.
pub(crate) fn source(model: &Model) -> u64 {
    if let Some(Some(guid)) = super::read_live(|l| l.loot_source()) {
        return guid;
    }
    #[cfg(any(test, feature = "fixtures"))]
    {
        model.loot_source
    }
    #[cfg(not(any(test, feature = "fixtures")))]
    {
        let _ = model;
        0
    }
}

impl super::UiScript {
    /// A test's or a capture fixture's open loot (or none), read where no view is lent.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn set_loot(&mut self, state: Option<LootState>) {
        set_field!(self, loot, state);
    }

    /// A test's looted object, 0 with no loot open, read where no view is lent.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn set_loot_source(&mut self, guid: u64) {
        set_field!(self, loot_source, guid);
    }
}

/// `LootSlot`'s and `ConfirmLootSlot`'s usage error, which both raise (`0x589539`, `0x5895a9`).
const LOOT_SLOT_USAGE: &str = "Usage: LootSlot(slot)";

/// `GiveMasterLoot`'s usage error, on either argument not a number (`0x589697`).
const GIVE_MASTER_LOOT_USAGE: &str = "Usage: GiveMasterLoot(slot, index)";

/// A loot verb's 1-based argument as its binding converts it (the truncating `fistp`, then `sub 1`):
/// below 1 the decremented index fails the unsigned bound, so it names nothing.
fn one_based(n: i32) -> Option<u32> {
    u32::try_from(n).ok().filter(|&n| n >= 1)
}

/// Run `verb` inside this call where the host's state is lent (else it waits, in call order).
fn run_loot(lua: &Lua, verb: super::LootVerb) {
    super::calls::make(lua, super::ScriptCall::Loot(verb));
}

/// Display slot `slot` (1-based) of the open loot: `Some(Some(row))` an item or the coins,
/// `Some(None)` an empty row or taken coins, `None` no such slot.
fn slot_of(window: Option<&LootState>, slot: usize) -> Option<Option<&LootRow>> {
    window
        .and_then(|l| slot.checked_sub(1).and_then(|n| l.rows.get(n)))
        .map(Option::as_ref)
}

/// Register the loot globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // SetLootPortrait(texture): the loot source's 3D portrait, the unit-frame bake, and the number
    // 1; a source that is no unit object, or no loot, clears the texture and answers nil. A
    // non-Texture raises the member prologue's errors. No stock caller: `LootFrame.lua` paints its
    // own portrait art.
    g.set(
        "SetLootPortrait",
        lua.create_function(|lua, texture: Value| {
            let rh = super::region::this_texture(lua, &texture)?;
            let mut model = crate::script::model_mut_of(lua);
            if !window(&model).is_some_and(|l| l.source_unit) {
                super::region::clear_texture(&mut model, rh);
                return Ok(Value::Nil);
            }
            let data = model.region_data.entry(rh).or_default();
            data.portrait_unit = Some(LOOT_PORTRAIT_UNIT.to_string());
            data.texture = None;
            data.fill = None;
            data.circular = true;
            Ok(Value::Integer(1))
        })?,
    )?;

    // GetNumLootItems() (`0x588190`): through the last row with an item, plus the coin slot.
    g.set(
        "GetNumLootItems",
        lua.create_function(|lua, ()| {
            let model = crate::script::model_mut_of(lua);
            Ok(window(&model).map_or(0, |l| l.count()) as i64)
        })?,
    )?;

    // GetLootSlotInfo(slot) → texture, item, quantity, quality, locked (`0x588570`,
    // `LootFrame.lua:97`), five on every leg: `locked` is 1 for a `LOCKED` slot type
    // (`[0xbfa6ac]` == 3) and nil otherwise. No slot is `nil, "", 0, 0`, a looted item row
    // `nil, "", 0, -1` (`0x588310` reads the record of item 0), taken coins `nil, "", 0, 0`
    // (`0x5883a7`, `0x588459`, `0x588329`). The miss sentinels are a floor: the window opens once
    // every record has landed (`0x5896b0`, `0x589460`).
    g.set(
        "GetLootSlotInfo",
        lua.create_function(|lua, slot: usize| {
            let window = {
                let model = crate::script::model_mut_of(lua);
                window(&model)
            };
            let state = slot_of(window.as_ref(), slot);
            let Some(Some(row)) = state else {
                let coins_taken = slot == 1 && window.as_ref().is_some_and(|l| l.coin_slot);
                let quality = if state.is_some() && !coins_taken {
                    CACHE_MISS_QUALITY
                } else {
                    NO_SLOT_QUALITY
                };
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::String(lua.create_string(CACHE_MISS_NAME)?),
                    Value::Integer(0),
                    Value::Integer(quality),
                    Value::Nil,
                ]));
            };
            let texture =
                Value::String(lua.create_string(row.texture.as_deref().unwrap_or(MISSING_ICON))?);
            // The coin row's text: each denomination through its global string, newline-joined
            // (`0x588430` -> `0x7e7d80`).
            let item = if row.is_coin {
                super::cursor::money::currency_text(lua, row.coins, "\n")
            } else {
                row.name.clone().unwrap_or_else(|| CACHE_MISS_NAME.into())
            };
            let item = Value::String(lua.create_string(item)?);
            let quality = Value::Integer(row.quality.map_or(CACHE_MISS_QUALITY, i64::from));
            Ok(MultiValue::from_vec(vec![
                texture,
                item,
                Value::Integer(i64::from(row.quantity)),
                quality,
                if row.locked {
                    Value::Integer(1)
                } else {
                    Value::Nil
                },
            ]))
        })?,
    )?;

    // GetLootSlotLink(slot) (`0x5886d0` → `0x5884a0`): nil out of range, for the coin row and while
    // the record is in flight, which it reads with no callback. The stock row click hands it to
    // `DressUpItemLink` (`LootFrame.lua:149`) and `ChatFrameEditBox:Insert` (`:152`), both of which
    // take a nil.
    g.set(
        "GetLootSlotLink",
        lua.create_function(|lua, slot: usize| {
            let link = {
                let model = crate::script::model_mut_of(lua);
                let window = window(&model);
                slot_of(window.as_ref(), slot)
                    .flatten()
                    .and_then(|r| r.link.clone())
            };
            match link {
                Some(link) => Ok(Value::String(lua.create_string(&link)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // LootSlotIsItem(slot) (`0x588750`): a table row holding an item.
    g.set(
        "LootSlotIsItem",
        lua.create_function(|lua, slot: usize| {
            let model = crate::script::model_mut_of(lua);
            let window = window(&model);
            Ok(flag(
                slot_of(window.as_ref(), slot)
                    .flatten()
                    .is_some_and(|r| !r.is_coin),
            ))
        })?,
    )?;

    // LootSlotIsCoin(slot) (`0x588810`): slot 1 while the coins are there.
    g.set(
        "LootSlotIsCoin",
        lua.create_function(|lua, slot: usize| {
            let model = crate::script::model_mut_of(lua);
            let window = window(&model);
            Ok(flag(
                slot_of(window.as_ref(), slot)
                    .flatten()
                    .is_some_and(|r| r.is_coin),
            ))
        })?,
    )?;

    // IsFishingLoot() (`0x5888e0`): the last window's loot type (`[0xbfa68c]`), which a close
    // leaves in place.
    g.set(
        "IsFishingLoot",
        lua.create_function(|lua, ()| {
            if let Some(Some(fishing)) = super::read_live(|l| l.loot_fishing()) {
                return Ok(flag(fishing));
            }
            let model = crate::script::model_mut_of(lua);
            Ok(flag(window(&model).is_some_and(|l| l.fishing)))
        })?,
    )?;

    // LootSlot(slot): the take (`0x589520`: a number check, the truncating `fistp`, `sub eax,1`,
    // then `0x589140`), run inside the call: code after it reads what it did.
    g.set(
        "LootSlot",
        lua.create_function(|lua, slot: Value| {
            let slot = super::binding_abi::number_arg(lua, slot, LOOT_SLOT_USAGE)?;
            if let Some(slot) = one_based(slot) {
                run_loot(lua, super::LootVerb::Slot(slot));
            }
            Ok(())
        })?,
    )?;

    // ConfirmLootSlot(slot): the `LOOT_BIND` popup's accept (`StaticPopup.lua:1519`). `0x589590`
    // converts as `LootSlot` does, with its usage line, and calls `0x589300` inside the call, which
    // sends only for the pending slot.
    g.set(
        "ConfirmLootSlot",
        lua.create_function(|lua, slot: Value| {
            let slot = super::binding_abi::number_arg(lua, slot, LOOT_SLOT_USAGE)?;
            if let Some(slot) = one_based(slot) {
                run_loot(lua, super::LootVerb::Confirm(slot));
            }
            Ok(())
        })?,
    )?;

    // CloseLoot([noUI]) (`0x5888b0` -> the close chain with `(1, 0, noUI)`, inside the call, up to
    // `LOOT_CLOSED`): `noUI` through the bool-or-default read with false, `LootFrame.lua:21`'s
    // "unable to open the UI".
    g.set(
        "CloseLoot",
        lua.create_function(|lua, args: MultiValue| {
            let no_ui = super::binding_abi::bool_or_default(args.front(), false);
            run_loot(lua, super::LootVerb::Close { no_ui });
            Ok(())
        })?,
    )?;

    // GetMasterLootCandidate(index) (`0x588920`): the candidate list's guid at `index - 1`
    // (`0x6fa770`) and its name read off the name cache, which leaves its callback on a miss
    // (`0x58899d`, `0x588150`); nil, never an error, out of range or on an empty slot, since
    // `GroupLootDropDown_Initialize` probes slots 1-5 in a party and 1-40 in a raid and reads nil
    // as nobody (`LootFrame.lua:168-232`).
    g.set(
        "GetMasterLootCandidate",
        lua.create_function(|lua, index: u32| {
            let Some(index) = index.checked_sub(1) else {
                return Ok(None);
            };
            if let Some(Some(name)) = super::write_live(|l| l.master_loot_candidate(index)) {
                return Ok(name);
            }
            let model = crate::script::model_mut_of(lua);
            Ok(window(&model)
                .and_then(|l| l.master_candidates.get(index as usize).cloned())
                .flatten())
        })?,
    )?;

    // GiveMasterLoot(slot, candidateIndex): from the dropdown below the quality threshold, and from
    // the `CONFIRM_LOOT_DISTRIBUTION` popup above it (`LootFrame.lua:234-244`,
    // `StaticPopup.lua:85-94`). Both are number-checked and truncated (`0x589617`, `0x589626`); a
    // candidate that resolves to no guid sends nothing (`0x589680`); the send runs inside the call.
    g.set(
        "GiveMasterLoot",
        lua.create_function(|lua, (slot, candidate): (Value, Value)| {
            let slot = super::binding_abi::number_arg(lua, slot, GIVE_MASTER_LOOT_USAGE)?;
            let candidate = super::binding_abi::number_arg(lua, candidate, GIVE_MASTER_LOOT_USAGE)?;
            if let (Some(slot), Some(candidate)) = (one_based(slot), one_based(candidate)) {
                run_loot(lua, super::LootVerb::MasterGive { slot, candidate });
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{LootRow, LootState};
    use crate::script::{LootVerb, UiScript};

    /// The region data behind a named texture: its portrait key and file.
    fn portrait_of(s: &UiScript, name: &str) -> (Option<String>, Option<String>) {
        let model = s.model_ref();
        let id = model.region_names[name];
        let rh = *model.id_to_region.get(&id).expect("a region");
        let data = &model.region_data[&rh];
        (data.portrait_unit.clone(), data.texture.clone())
    }

    /// `SetLootPortrait`: a unit source binds the loot portrait and answers 1; anything else clears
    /// the texture and answers nil.
    #[test]
    fn loot_portrait_binds_a_unit_source_and_clears_otherwise() {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"local f = CreateFrame("Frame", "LootHost", UIParent)
               f:CreateTexture("LootTex")
               LootTex:SetTexture("Interface\TargetingFrame\TargetDead")"#,
        )
        .unwrap();
        assert!(s
            .eval::<bool>("return SetLootPortrait(LootTex) == nil")
            .unwrap());
        assert_eq!(portrait_of(&s, "LootTex"), (None, None), "no loot: cleared");

        s.run(r#"LootTex:SetTexture("Interface\TargetingFrame\TargetDead")"#)
            .unwrap();
        s.set_loot(Some(LootState {
            source_unit: true,
            ..loot()
        }));
        assert_eq!(s.eval::<i64>("return SetLootPortrait(LootTex)").unwrap(), 1);
        assert_eq!(
            portrait_of(&s, "LootTex"),
            (Some(super::LOOT_PORTRAIT_UNIT.to_string()), None)
        );

        s.set_loot(Some(loot()));
        assert!(s
            .eval::<bool>("return SetLootPortrait(LootTex) == nil")
            .unwrap());
        assert_eq!(portrait_of(&s, "LootTex"), (None, None), "a chest: cleared");

        let err = s.run("SetLootPortrait(LootHost)").unwrap_err().to_string();
        assert!(
            err.contains("Wrong object type for member function"),
            "{err}"
        );
        let err = s.run("SetLootPortrait()").unwrap_err().to_string();
        assert!(err.contains("non-table object"), "{err}");
    }

    fn loot() -> LootState {
        LootState {
            coin_slot: false,
            source_unit: false,
            source_level: 0,
            master_candidates: Vec::new(),
            rows: vec![
                Some(LootRow {
                    item_id: 0,
                    name: None,
                    texture: Some("Interface\\Icons\\INV_Misc_Coin_01".into()),
                    quantity: 1,
                    quality: Some(1),
                    is_coin: true,
                    coins: 12_345,
                    link: None,
                    random_property_id: 0,
                    locked: false,
                }),
                Some(LootRow {
                    item_id: 0,
                    name: Some("Wool Cloth".into()),
                    texture: Some("Interface\\Icons\\INV_Fabric_Wool_01".into()),
                    quantity: 3,
                    quality: Some(1),
                    is_coin: false,
                    coins: 0,
                    link: Some("|cffffffff|Hitem:2589:0:0:0|h[Wool Cloth]|h|r".into()),
                    random_property_id: 0,
                    locked: false,
                }),
                // In flight: the loot arrived, its item template has not.
                Some(LootRow {
                    item_id: 0,
                    name: None,
                    texture: Some("Interface\\Icons\\INV_Misc_QuestionMark".into()),
                    quantity: 1,
                    quality: None,
                    is_coin: false,
                    coins: 0,
                    link: None,
                    random_property_id: 0,
                    locked: false,
                }),
            ],
            fishing: false,
        }
    }

    #[test]
    fn loot_snapshot_reads() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"GOLD_AMOUNT = "%d Gold" SILVER_AMOUNT = "%d Silver" COPPER_AMOUNT = "%d Copper""#)
            .unwrap();
        assert_eq!(s.eval::<i64>("return GetNumLootItems()").unwrap(), 0);
        assert!(s.eval::<bool>("return GetLootSlotInfo(1) == nil").unwrap());

        s.set_loot(Some(loot()));
        assert_eq!(s.eval::<i64>("return GetNumLootItems()").unwrap(), 3);

        assert!(s.eval::<bool>("return LootSlotIsCoin(1)").unwrap());
        assert!(s.eval::<bool>("return not LootSlotIsItem(1)").unwrap());
        let (texture, item, quantity, quality) = s
            .eval::<(String, String, i64, i64)>("return GetLootSlotInfo(1)")
            .unwrap();
        assert_eq!(texture, "Interface\\Icons\\INV_Misc_Coin_01");
        assert_eq!(
            item, "1 Gold\n23 Silver\n45 Copper",
            "`0x588430` -> `0x7e7d80`"
        );
        assert_eq!((quantity, quality), (1, 1));

        assert!(s.eval::<bool>("return LootSlotIsItem(2)").unwrap());
        assert!(s.eval::<bool>("return not LootSlotIsCoin(2)").unwrap());
        let (name, qty) = s
            .eval::<(String, i64)>("local _, i, q = GetLootSlotInfo(2)\nreturn i, q")
            .unwrap();
        assert_eq!((name.as_str(), qty), ("Wool Cloth", 3));

        // Row 3 is in flight: the cache-miss sentinels, not nils.
        let (texture, item, quantity, quality) = s
            .eval::<(String, String, i64, i64)>("return GetLootSlotInfo(3)")
            .unwrap();
        assert_eq!(
            (item.as_str(), quantity, quality),
            ("", 1, -1),
            "an in-flight row answers \"\" / -1, never nil"
        );
        assert_eq!(texture, "Interface\\Icons\\INV_Misc_QuestionMark");
        let mut iconless = loot();
        iconless.rows[2].as_mut().unwrap().texture = None;
        s.set_loot(Some(iconless));
        assert_eq!(
            s.eval::<String>("return (GetLootSlotInfo(3))").unwrap(),
            "Interface\\Icons\\INV_Misc_QuestionMark",
            "a display-info miss answers the reference's question mark, not nil"
        );
        s.set_loot(Some(loot()));

        assert_eq!(
            s.eval::<String>("return GetLootSlotLink(2)").unwrap(),
            "|cffffffff|Hitem:2589:0:0:0|h[Wool Cloth]|h|r"
        );
        assert!(s.eval::<bool>("return GetLootSlotLink(1) == nil").unwrap());
        assert!(s.eval::<bool>("return GetLootSlotLink(3) == nil").unwrap());

        assert!(s.eval::<bool>("return not IsFishingLoot()").unwrap());
        let mut fished = loot();
        fished.fishing = true;
        s.set_loot(Some(fished));
        assert!(s.eval::<bool>("return IsFishingLoot()").unwrap());
        s.set_loot(Some(loot()));

        assert!(s.eval::<bool>("return GetLootSlotInfo(9) == nil").unwrap());
        assert!(s.eval::<bool>("return GetLootSlotLink(9) == nil").unwrap());
        assert!(s.eval::<bool>("return not LootSlotIsItem(9)").unwrap());
        assert!(s.eval::<bool>("return not LootSlotIsCoin(9)").unwrap());

        // A looted slot stays counted but answers neither predicate, and the rows below keep
        // their slots.
        let mut cleared = loot();
        cleared.rows[0] = None;
        s.set_loot(Some(cleared));
        assert_eq!(s.eval::<i64>("return GetNumLootItems()").unwrap(), 3);
        assert!(s.eval::<bool>("return GetLootSlotInfo(1) == nil").unwrap());
        assert!(s.eval::<bool>("return GetLootSlotLink(1) == nil").unwrap());
        assert!(s.eval::<bool>("return not LootSlotIsItem(1)").unwrap());
        assert!(s.eval::<bool>("return not LootSlotIsCoin(1)").unwrap());
        assert!(s.eval::<bool>("return LootSlotIsItem(2)").unwrap());
    }

    /// `GetNumLootItems` counts through the last row with an item, plus the coin slot
    /// (`0x588190`); taken coins answer `nil, "", 0, 0` (`0x5883a7`, `0x588459`, `0x588329`)
    /// where a looted item row answers -1 (`0x588310` on item 0).
    #[test]
    fn the_count_stops_at_the_last_item_and_taken_coins_read_quality_0() {
        let mut s = UiScript::new().unwrap();
        let mut l = loot();
        l.coin_slot = true;
        l.rows.extend([None, None]);
        s.set_loot(Some(l.clone()));
        assert_eq!(
            s.eval::<i64>("return GetNumLootItems()").unwrap(),
            3,
            "trailing empty rows are not counted"
        );
        l.rows[2] = None;
        s.set_loot(Some(l.clone()));
        assert_eq!(s.eval::<i64>("return GetNumLootItems()").unwrap(), 2);
        assert_eq!(
            s.eval::<i64>("return select(4, GetLootSlotInfo(3))")
                .unwrap(),
            -1,
            "a looted item row inside the table"
        );
        l.rows[1] = None;
        l.rows[0] = None;
        s.set_loot(Some(l));
        assert_eq!(
            s.eval::<i64>("return GetNumLootItems()").unwrap(),
            1,
            "the coin slot still counts once its coins are taken"
        );
        let (name, count, quality) = s
            .eval::<(String, i64, i64)>("local _, n, c, q = GetLootSlotInfo(1) return n, c, q")
            .unwrap();
        assert_eq!((name.as_str(), count, quality), ("", 0, 0));
        assert!(s.eval::<bool>("return not LootSlotIsCoin(1)").unwrap());
    }

    #[test]
    fn master_loot_candidates_read_1_based_and_nil_past_the_end() {
        let mut s = UiScript::new().unwrap();

        assert!(s
            .eval::<bool>("return GetMasterLootCandidate(1) == nil")
            .unwrap());

        // No candidate list: any loot method but master.
        s.set_loot(Some(loot()));
        assert!(s
            .eval::<bool>("return GetMasterLootCandidate(1) == nil")
            .unwrap());

        let mut ml = loot();
        ml.master_candidates = vec![Some("Thrall".into()), Some("Cairne".into())];
        s.set_loot(Some(ml));
        assert_eq!(
            s.eval::<String>("return GetMasterLootCandidate(1)")
                .unwrap(),
            "Thrall"
        );
        assert_eq!(
            s.eval::<String>("return GetMasterLootCandidate(2)")
                .unwrap(),
            "Cairne"
        );
        assert!(
            s.eval::<bool>("return GetMasterLootCandidate(3) == nil")
                .unwrap(),
            "past the end is nil"
        );
        assert!(
            s.eval::<bool>("return GetMasterLootCandidate(0) == nil")
                .unwrap(),
            "0 is not a Lua index"
        );
        // The raid arm's sweep: 40 probes, none raising.
        assert_eq!(
            s.eval::<i64>(
                "local n = 0
                 for i = 1, 40 do if GetMasterLootCandidate(i) then n = n + 1 end end
                 return n",
            )
            .unwrap(),
            2
        );

        // A raid's slot carries the subgroup, so slot 6 can be filled while 2-5 are empty.
        let mut raid = loot();
        raid.master_candidates = vec![
            Some("Thrall".into()),
            None,
            None,
            None,
            None,
            Some("Cairne".into()),
        ];
        s.set_loot(Some(raid));
        assert_eq!(
            s.eval::<String>("return GetMasterLootCandidate(1)")
                .unwrap(),
            "Thrall"
        );
        for hole in 2..=5 {
            assert!(
                s.eval::<bool>(&format!("return GetMasterLootCandidate({hole}) == nil"))
                    .unwrap(),
                "slot {hole} is a hole"
            );
        }
        assert_eq!(
            s.eval::<String>("return GetMasterLootCandidate(6)")
                .unwrap(),
            "Cairne",
            "an occupant past the holes is still reachable"
        );
    }

    /// `GiveMasterLoot` resolves both arguments as numbers, raising its own usage line otherwise
    /// (`0x589697`), and names nothing below 1.
    #[test]
    fn give_master_loot_runs_the_assignment() {
        let mut s = UiScript::new().unwrap();
        let mut ml = loot();
        ml.master_candidates = vec![Some("Thrall".into())];
        s.set_loot(Some(ml));
        s.run("GiveMasterLoot(2, 1) GiveMasterLoot(2.9, '1') GiveMasterLoot(0, 1) GiveMasterLoot(2, 0)")
            .unwrap();
        let give = LootVerb::MasterGive {
            slot: 2,
            candidate: 1,
        };
        assert_eq!(s.take_loot_verbs(), vec![give, give]);
        let err = s.run("GiveMasterLoot(2)").unwrap_err().to_string();
        assert!(err.contains("Usage: GiveMasterLoot(slot, index)"), "{err}");
        assert!(s.take_loot_verbs().is_empty());
    }

    /// `LootSlot` is the take (`0x589520` → `0x589140`); `ConfirmLootSlot` the confirm
    /// (`0x589590` → `0x589300`), raising `LootSlot`'s usage line (`0x5895a9`).
    #[test]
    fn loot_slot_takes_and_confirm_loot_slot_confirms() {
        let mut s = UiScript::new().unwrap();
        s.set_loot(Some(loot()));
        s.run("LootSlot(2) ConfirmLootSlot(3)").unwrap();
        assert_eq!(
            s.take_loot_verbs(),
            vec![LootVerb::Slot(2), LootVerb::Confirm(3)],
            "in call order"
        );

        // The `fistp` truncates, and a decremented slot below 0 takes nothing.
        s.run("LootSlot(2.9) LootSlot(0) LootSlot(-1) ConfirmLootSlot(0.5)")
            .unwrap();
        assert_eq!(s.take_loot_verbs(), vec![LootVerb::Slot(2)]);
        for verb in ["LootSlot", "ConfirmLootSlot"] {
            let err = s.run(&format!("{verb}('x')")).unwrap_err().to_string();
            assert!(err.contains("Usage: LootSlot(slot)"), "{verb}: {err}");
        }
    }

    /// The fifth return is 1 for a `LOCKED` row and nil otherwise, never the item id.
    #[test]
    fn get_loot_slot_info_reports_a_locked_row() {
        let mut s = UiScript::new().unwrap();
        let mut l = loot();
        l.rows[1].as_mut().unwrap().item_id = 2589;
        s.set_loot(Some(l.clone()));
        for slot in 1..=4 {
            assert!(
                s.eval::<bool>(&format!("return select(5, GetLootSlotInfo({slot})) == nil"))
                    .unwrap(),
                "slot {slot} is not locked"
            );
        }
        assert_eq!(
            s.eval::<i64>("return select('#', GetLootSlotInfo(9))")
                .unwrap(),
            5,
            "five returns past the end too"
        );
        l.rows[1].as_mut().unwrap().locked = true;
        s.set_loot(Some(l));
        assert_eq!(
            s.eval::<i64>("return select(5, GetLootSlotInfo(2))")
                .unwrap(),
            1
        );
    }

    /// `CloseLoot(noUI)`: stock `CloseLoot(autoLoot == 0)` passes a boolean (`LootFrame.lua:21`).
    #[test]
    fn close_loot_runs_with_its_no_ui_argument() {
        let mut s = UiScript::new().unwrap();
        s.set_loot(Some(loot()));
        s.run("CloseLoot()").unwrap();
        assert_eq!(s.take_loot_verbs(), vec![LootVerb::Close { no_ui: false }]);
        for (arg, no_ui) in [
            ("1", true),
            ("true", true),
            ("0", false),
            ("false", false),
            ("nil", false),
        ] {
            s.run(&format!("CloseLoot({arg})")).unwrap();
            assert_eq!(
                s.take_loot_verbs(),
                vec![LootVerb::Close { no_ui }],
                "CloseLoot({arg})"
            );
        }
    }

    #[test]
    fn clearing_the_loot_empties_it() {
        let mut s = UiScript::new().unwrap();
        s.set_loot(Some(loot()));
        s.set_loot(None);
        assert_eq!(s.eval::<i64>("return GetNumLootItems()").unwrap(), 0);
        assert!(s.eval::<bool>("return GetLootSlotInfo(1) == nil").unwrap());
    }

    /// 12340 has no `LootButton` frame type: `CreateFrame` refuses it, and the stock rows are
    /// plain Buttons that take through `LootSlot` (`LootFrame.lua:212-221`).
    #[test]
    fn there_is_no_loot_button_frame_type() {
        let s = UiScript::new().unwrap();
        let err = s
            .run(r#"CreateFrame("LootButton", "LB1", UIParent)"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("LootButton"), "{err}");
        s.run(r#"b = CreateFrame("Button", "PlainB", UIParent)"#)
            .unwrap();
        assert_eq!(s.eval::<String>("return type(b.SetSlot)").unwrap(), "nil");
    }
}
