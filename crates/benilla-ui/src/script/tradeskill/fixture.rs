//! A test's tradeskill window: a book of recipes resolved up front, filed into a
//! [`TradeSkillList`] as the app files the live one, and answering the verbs' reads from it where
//! no view lends the live list.

use super::{
    TradeSkillDifficulty, TradeSkillGroup, TradeSkillLineView, TradeSkillList,
    TradeSkillReagentView, TradeSkillRecipeView, TradeSkillRow,
};
use crate::script::live::{GlobalStrings, LiveState};

/// One reagent of a test recipe.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeSkillReagent {
    pub item: u32,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub need: u32,
    pub have: u32,
}

/// One test recipe, its spell record and reads resolved up front.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeSkillRecipe {
    pub spell_id: u32,
    pub name: String,
    pub difficulty: TradeSkillDifficulty,
    pub num_available: i32,
    pub trivial_high: u32,
    pub group: Option<TradeSkillGroup>,
    pub item_level: u32,
    pub inv_slots: u32,
    pub creates_item: bool,
    pub product_item: u32,
    /// The product's `RequiredLevel`, which the level filter reads.
    pub required_level: Option<u32>,
    pub icon: Option<String>,
    pub min_made: u32,
    pub max_made: u32,
    /// When the cooldown ends on the `GetTime()` clock; `None` when ready.
    pub cooldown_end: Option<f64>,
    /// The cooldown is parked on hold (enable 0) rather than running.
    pub cooldown_on_hold: bool,
    pub description: Option<String>,
    /// The case-folded texts the name filter searches.
    pub search_text: Vec<String>,
    pub reagents: Vec<TradeSkillReagent>,
    pub tools: Vec<(String, bool)>,
}

/// One test window.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeSkillState {
    pub line: u32,
    pub line_name: String,
    pub alt_verb: String,
    pub rank: u32,
    pub max_rank: u32,
    pub list_link: Option<String>,
    pub recipes: Vec<TradeSkillRecipe>,
}

/// The test window and its list.
#[derive(Default)]
pub(crate) struct TradeSkillFixture {
    pub(crate) list: TradeSkillList,
    state: Option<TradeSkillState>,
}

impl TradeSkillFixture {
    /// File `state` as the open does (`0x5de120`): the line switch, the rebuild, the last line;
    /// `None` closes. Whether `UPDATE_TRADESKILL_RECAST` is owed.
    pub(super) fn set(&mut self, state: Option<TradeSkillState>) -> bool {
        let Some(s) = state else {
            self.state = None;
            self.list.refresh(true, None, &|_| true);
            return self.list.close();
        };
        self.list.open_line(s.line, 0);
        let rows = s
            .recipes
            .iter()
            .map(|r| TradeSkillRow {
                spell_id: r.spell_id,
                name: r.name.clone(),
                difficulty: r.difficulty,
                num_available: r.num_available,
                trivial_high: r.trivial_high,
                group: r.group.clone(),
                item_level: r.item_level,
                inv_slots: r.inv_slots,
            })
            .collect();
        self.state = Some(s);
        let (name, min, max) = {
            let (n, a, b) = self.list.text_filter();
            (n.map(str::to_string), a, b)
        };
        let passes: Vec<u32> = self
            .recipes()
            .filter(|r| r.passes(min, max, name.as_deref()))
            .map(|r| r.spell_id)
            .collect();
        self.list
            .refresh(true, Some(rows), &|s| passes.contains(&s));
        self.list.opened();
        false
    }

    fn recipes(&self) -> impl Iterator<Item = &TradeSkillRecipe> {
        self.state.iter().flat_map(|s| s.recipes.iter())
    }

    fn recipe(&self, spell_id: u32) -> Option<&TradeSkillRecipe> {
        self.recipes().find(|r| r.spell_id == spell_id)
    }
}

impl TradeSkillRecipe {
    /// The text filter over the resolved texts.
    fn passes(&self, min: i32, max: i32, name: Option<&str>) -> bool {
        if let Some(req) = self.required_level.map(|l| l as i32) {
            if (min != 0 && req < min) || (max != 0 && req > max) {
                return false;
            }
        }
        let Some(name) = name else {
            return true;
        };
        self.search_text.iter().any(|t| t.contains(name))
    }
}

impl LiveState for TradeSkillFixture {
    fn unit_flags(&self, _guid: u64) -> Option<u32> {
        None
    }

    fn unit_health(&self, _guid: u64) -> Option<u32> {
        None
    }

    fn active_player(&self) -> Option<u64> {
        None
    }

    fn has_control(&self) -> bool {
        true
    }

    fn far_sight(&self) -> bool {
        false
    }

    fn trade_skill(&self) -> Option<&TradeSkillList> {
        Some(&self.list)
    }

    fn trade_skill_mut(&mut self) -> Option<&mut TradeSkillList> {
        Some(&mut self.list)
    }

    fn trade_skill_line(&self) -> TradeSkillLineView {
        match self.state.as_ref().filter(|_| self.list.line != 0) {
            Some(s) => TradeSkillLineView {
                name: Some(s.line_name.clone()),
                alt_verb: Some(s.alt_verb.clone()),
                rank: i64::from(s.rank),
                max_rank: i64::from(s.max_rank),
            },
            None => TradeSkillLineView::default(),
        }
    }

    fn trade_skill_recipe(&self, spell_id: u32) -> Option<TradeSkillRecipeView> {
        let r = self.recipe(spell_id)?;
        Some(TradeSkillRecipeView {
            name: r.name.clone(),
            tradeskill: true,
            creates_item: r.creates_item,
            product_item: r.product_item,
            icon: r.icon.clone(),
            min_made: r.min_made,
            max_made: r.max_made,
            reagents: r.reagents.iter().map(|re| (re.item, re.need)).collect(),
            line_name: self.state.as_ref().map(|s| s.line_name.clone()),
        })
    }

    fn trade_skill_reagent(&mut self, spell_id: u32, item: u32) -> TradeSkillReagentView {
        self.recipe(spell_id)
            .and_then(|r| r.reagents.iter().find(|re| re.item == item))
            .map(|re| TradeSkillReagentView {
                name: re.name.clone(),
                icon: re.icon.clone(),
                have: re.have,
            })
            .unwrap_or_default()
    }

    fn trade_skill_tools(&mut self, spell_id: u32) -> Vec<(String, bool)> {
        self.recipe(spell_id)
            .map(|r| r.tools.clone())
            .unwrap_or_default()
    }

    fn trade_skill_description(&self, spell_id: u32, _global: GlobalStrings<'_>) -> Option<String> {
        self.recipe(spell_id)?.description.clone()
    }

    fn trade_skill_cooldown(
        &self,
        spell_id: u32,
        _anchor: std::time::Instant,
        _ui_now: f64,
    ) -> Option<(i64, u32, bool)> {
        let r = self.recipe(spell_id)?;
        let end = r.cooldown_end?;
        Some(((end * 1000.0) as i64, 0, !r.cooldown_on_hold))
    }

    fn trade_skill_list_link(&self) -> Option<String> {
        self.state.as_ref()?.list_link.clone()
    }

    fn trade_skill_filter_spell(
        &self,
        spell_id: u32,
        min: i32,
        max: i32,
        name: Option<&str>,
        _global: GlobalStrings<'_>,
    ) -> bool {
        self.recipe(spell_id)
            .is_some_and(|r| r.passes(min, max, name))
    }
}
