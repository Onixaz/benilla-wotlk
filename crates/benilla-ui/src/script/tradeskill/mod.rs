//! The tradeskill bindings behind the stock `Blizzard_TradeSkillUI`, the client's
//! `CGTradeSkillInfo`: the app pushes a snapshot ([`UiScript::set_trade_skill`]) with recipes
//! resolved from `Spell.dbc`, `SkillLineAbility.dbc`, item templates and bag counts, and the engine
//! builds the display list (`RefreshList 0x5dd5a0`): one header per product sub-class, sorted and
//! filtered in place (`0x5dcc40`). Every Lua index, `SetTradeSkillItem`'s included, is 1-based into
//! that list ([`recipe_at`]); its first `GetNumTradeSkills()` entries are the visible ones.

mod api;
mod view;

#[cfg(test)]
mod tests;

pub(super) use api::install;
pub(crate) use view::{recipe_at, TradeSkillModel};

/// A recipe's difficulty band, computed app-side: gray at or above trivialHigh, green from the
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

/// One reagent of a recipe, as `GetTradeSkillReagentInfo` reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeSkillReagent {
    /// The item entry `SetTradeSkillItem(i, j)` resolves through.
    pub item: u32,
    /// `None` until the item template arrives.
    pub name: Option<String>,
    /// `None` until the item template arrives.
    pub icon: Option<String>,
    /// How many this recipe consumes.
    pub need: u32,
    /// How many the player's bags hold.
    pub have: u32,
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

/// One known recipe of the open line, resolved app-side.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeSkillRecipe {
    /// What `DoTradeSkill` casts.
    pub spell_id: u32,
    pub name: String,
    pub difficulty: TradeSkillDifficulty,
    /// The fewest crafts the carried reagents cover; -1 for a recipe without reagents, which
    /// the list and `DoTradeSkill` read as unlimited.
    pub num_available: i32,
    /// `SkillLineAbility` trivialHigh, the sort key after difficulty (`+0x1c`).
    pub trivial_high: u32,
    /// `None` while the product's template is in flight, which holds the whole list back.
    pub group: Option<TradeSkillGroup>,
    /// The product's `ItemLevel`, 0 without one (`+0x14`).
    pub item_level: u32,
    /// The inventory-slot filter bits (`+0x10`).
    pub inv_slots: u32,
    /// `Effect[0]` creates an item (24, 157 or 59): no alternate verb, an item link and tooltip.
    pub creates_item: bool,
    /// `EffectItemType[0]`.
    pub product_item: u32,
    /// The product's `RequiredLevel` while its template is cached, which the level filter reads.
    pub required_level: Option<u32>,
    pub icon: Option<String>,
    pub min_made: u32,
    pub max_made: u32,
    /// When the cooldown ends on the `GetTime()` clock; `None` when ready.
    pub cooldown_end: Option<f64>,
    /// The spell's parsed description.
    pub description: Option<String>,
    /// The case-folded texts the name filter searches (`Profession_FilterSpell 0x5dc110`).
    pub search_text: Vec<String>,
    /// In display order.
    pub reagents: Vec<TradeSkillReagent>,
    /// The Requirements line's `(name, have)` pairs.
    pub tools: Vec<(String, bool)>,
}

/// One open tradeskill window, pushed whole by the app.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeSkillState {
    /// The `SkillLine.dbc` id, the persistence key (`0xc235f0`).
    pub line: u32,
    pub line_name: String,
    /// `SkillLine.dbc` alternateVerb, possibly empty: the `(-1, -1)` header and the Create
    /// button's text for a recipe that makes no item.
    pub alt_verb: String,
    pub rank: u32,
    pub max_rank: u32,
    /// `GetTradeSkillListLink`'s link, `None` for a line that cannot link.
    pub list_link: Option<String>,
    /// Unordered: the engine sorts them (`0x5dae60`).
    pub recipes: Vec<TradeSkillRecipe>,
}

impl super::UiScript {
    /// Pushes the open window's snapshot, or `None` on close; the app re-pushes on any change,
    /// each push a list rebuild (`RefreshList`).
    ///
    /// A push for another line resets the name, level and makeable filters, the sub-class,
    /// slot and fold masks and the selection; the same line keeps them across a close
    /// (`SetTradeSkill 0x5de120`).
    pub fn set_trade_skill(&mut self, state: Option<TradeSkillState>) {
        let fire = self.model_mut().trade_skill.set_state(state);
        if fire {
            self.fire_event("UPDATE_TRADESKILL_RECAST", vec![]);
        }
    }

    /// Whether the open list has entries, the reference's gate on `TRADE_SKILL_SHOW` (`0x5de2e5`).
    pub fn trade_skill_has_entries(&self) -> bool {
        self.model_ref().trade_skill.has_entries()
    }

    /// Drains the spell ids `DoTradeSkill` cast; the repeat is the engine's (`0x5d9fa0`).
    pub fn take_trade_skill_casts(&mut self) -> Vec<u32> {
        drain_field!(self, trade_skill.casts)
    }

    /// Whether `CloseTradeSkill` ran since the last drain; the close sends no packet.
    pub fn take_trade_skill_close(&mut self) -> bool {
        drain_field!(self, trade_skill.close)
    }

    /// Our own cast of `spell_id` started: a pending repeat of it becomes the running one, and
    /// any other is dropped (`CommitPendingRecast 0x5da0f0`).
    pub fn trade_skill_cast_started(&mut self, spell_id: u32) {
        self.model_mut().trade_skill.commit_pending_recast(spell_id);
    }

    /// Our own cast of `spell_id` went off; whether the repeat casts it again (`DoRecast
    /// 0x5da070`).
    pub fn trade_skill_cast_done(&mut self, spell_id: u32) -> bool {
        let (again, fire) = self.model_mut().trade_skill.do_recast(spell_id);
        if fire {
            self.fire_event("UPDATE_TRADESKILL_RECAST", vec![]);
        }
        again
    }

    /// Our own cast of `spell_id` failed: its repeat stops (`UpdateTradeSkillRecast 0x5da020`).
    pub fn trade_skill_cast_failed(&mut self, spell_id: u32) {
        if self.model_mut().trade_skill.cast_failed(spell_id) {
            self.fire_event("UPDATE_TRADESKILL_RECAST", vec![]);
        }
    }
}
