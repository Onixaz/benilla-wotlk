//! The tradeskill bindings behind the stock `Blizzard_TradeSkillUI`. The client's tradeskill
//! statics ([`TradeSkillList`]) live in the app, which rebuilds the list where the reference does
//! (`0x5dd5a0`): one entry per known recipe of the open line with its difficulty, makeable count
//! and slot bits, one header per product sub-class, sorted and filtered in place (`0x5dcc40`). The
//! verbs read the list and the game through the lent view at the call. Every Lua index,
//! `SetTradeSkillItem`'s included, is 1-based into that list; its first `GetNumTradeSkills()`
//! entries are the visible ones.

mod api;
#[cfg(any(test, feature = "fixtures"))]
mod fixture;
mod view;

#[cfg(test)]
mod tests;

pub(super) use api::install;
pub(crate) use api::recipe_at;
#[cfg(any(test, feature = "fixtures"))]
pub(crate) use fixture::TradeSkillFixture;
#[cfg(any(test, feature = "fixtures"))]
pub use fixture::{TradeSkillReagent, TradeSkillRecipe, TradeSkillState};
pub use view::TradeSkillList;

/// A recipe's difficulty band on the line's rank: gray at or above trivialHigh, green from the
/// low/high midpoint, yellow from trivialLow, orange below (`0x5dd5a0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TradeSkillDifficulty {
    /// Orange: a near-certain skill-up.
    Optimal,
    /// Yellow: a good skill-up chance.
    Medium,
    /// Green: a fading skill-up chance.
    Easy,
    /// Gray: no skill-up.
    Trivial,
}

impl TradeSkillDifficulty {
    /// A recipe row's `type` in `GetTradeSkillInfo`, a `TradeSkillTypeColor` key (`0xa1b788`).
    pub fn as_str(self) -> &'static str {
        match self {
            TradeSkillDifficulty::Optimal => "optimal",
            TradeSkillDifficulty::Medium => "medium",
            TradeSkillDifficulty::Easy => "easy",
            TradeSkillDifficulty::Trivial => "trivial",
        }
    }

    /// The difficulty the list sorts by, ascending (`0x5dae60`).
    pub(crate) fn tier(self) -> u8 {
        match self {
            TradeSkillDifficulty::Optimal => 0,
            TradeSkillDifficulty::Medium => 1,
            TradeSkillDifficulty::Easy => 2,
            TradeSkillDifficulty::Trivial => 3,
        }
    }
}

/// The header a recipe files under, the reference's sub-class record (`0x5dd5a0`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TradeSkillGroup {
    /// No created item (an enchant): the `(-1, -1)` group, named by the line's alternate verb.
    Verb,
    /// The product's `(ItemClass, ItemSubClass)` and its `ItemSubClass.dbc` name, verbose first;
    /// `None` without a row.
    SubClass {
        class: u32,
        subclass: u32,
        name: Option<String>,
    },
}

impl TradeSkillGroup {
    /// The `(itemClass, itemSubClass)` key, `(-1, -1)` for [`TradeSkillGroup::Verb`].
    pub(crate) fn key(&self) -> (i32, i32) {
        match self {
            TradeSkillGroup::Verb => (-1, -1),
            TradeSkillGroup::SubClass {
                class, subclass, ..
            } => (*class as i32, *subclass as i32),
        }
    }
}

/// One recipe as the list rebuild files it (`0x5dd5a0`): the entry's fields, and the spell's name
/// the sort reads.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeSkillRow {
    /// What `DoTradeSkill` casts.
    pub spell_id: u32,
    /// `Spell.dbc` name, the sort's last key (`0x5dae60`).
    pub name: String,
    /// The band on the rank the rebuild read (`+0x4`).
    pub difficulty: TradeSkillDifficulty,
    /// The fewest crafts the carried reagents cover, -1 for a recipe without reagents (`+0x18`).
    pub num_available: i32,
    /// `SkillLineAbility` trivialHigh, the sort key after difficulty (`+0x1c`).
    pub trivial_high: u32,
    /// `None` while the product's record is in flight, which holds the whole list back.
    pub group: Option<TradeSkillGroup>,
    /// The product's `ItemLevel`, 0 without one (`+0x14`).
    pub item_level: u32,
    /// The inventory-slot filter bits (`+0x10`).
    pub inv_slots: u32,
}

/// The open line at the call (`0x5db810`): `SkillLine.dbc`'s name and alternate verb, and the
/// player's rank and max rank of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradeSkillLineView {
    /// `None` without a record: `GetTradeSkillLine` answers `"UNKNOWN", 0, 0`.
    pub name: Option<String>,
    pub alt_verb: Option<String>,
    pub rank: i64,
    pub max_rank: i64,
}

/// A recipe's spell record at the call.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradeSkillRecipeView {
    pub name: String,
    /// `SPELL_ATTR_IS_TRADESKILL` (`0x20`), the recipe link's gate (`0x5db9a9`).
    pub tradeskill: bool,
    /// `Effect[0]` creates an item (24, 157 or 59).
    pub creates_item: bool,
    /// `EffectItemType[0]`.
    pub product_item: u32,
    /// The product's inventory icon, else the spell's (`0x5db550`).
    pub icon: Option<String>,
    pub min_made: u32,
    pub max_made: u32,
    /// The nonzero reagent slots, `(item, count)` in slot order.
    pub reagents: Vec<(u32, u32)>,
    /// The recipe's own skill line's name, the recipe link's prefix (`0x5db9c8` -> `0x5da5f0`).
    pub line_name: Option<String>,
}

/// One reagent at the call (`0x5de400`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradeSkillReagentView {
    /// `None` until the item record arrives.
    pub name: Option<String>,
    /// `None` until the item record arrives.
    pub icon: Option<String>,
    /// How many the bags hold.
    pub have: u32,
}

impl super::UiScript {
    /// A test's window: `state` filed as the app files it, with the list's filters and selection
    /// kept for the same line (`0x5de120`); `None` closes it.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn set_trade_skill(&mut self, state: Option<TradeSkillState>) {
        let mut model = self.model_mut();
        let fixture = model
            .trade_skill
            .get_or_insert_with(|| Box::new(TradeSkillFixture::default()));
        let fire = fixture.set(state);
        drop(model);
        if fire {
            self.fire_event("UPDATE_TRADESKILL_RECAST", vec![]);
        }
    }

    /// The test window's list, which a test drives as the app drives the live one.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn trade_skill_list_mut<R>(&mut self, f: impl FnOnce(&mut TradeSkillList) -> R) -> R {
        let mut model = self.model_mut();
        let fixture = model
            .trade_skill
            .get_or_insert_with(|| Box::new(TradeSkillFixture::default()));
        f(&mut fixture.list)
    }
}
