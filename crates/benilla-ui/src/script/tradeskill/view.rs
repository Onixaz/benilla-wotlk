//! The display list the tradeskill API indexes, the client's `CGTradeSkillInfo` statics: the
//! rebuild (`RefreshList 0x5dd5a0`), the filter pass and sort (`SetInvTypeFilter 0x5dcc40`) and
//! the repeat (`0x5d9fa0`, `0x5da070`, `0x5da0f0`).

use std::cmp::Ordering;

use super::{TradeSkillRecipe, TradeSkillState};
use crate::script::Model;

/// Every filter bit set: all sub-classes shown, none folded, all slots shown.
const ALL: u32 = u32::MAX;

/// One display entry: a recipe, or the header of a sub-class (`m_spellId == -1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Entry {
    /// Position in `TradeSkillState::recipes`, `None` for a header.
    pub(super) recipe: Option<usize>,
    /// The recipe's spell id, 0 for a header.
    spell: u32,
    pub(super) key: (i32, i32),
    visible: bool,
}

/// One sub-class record (`m_subClassList`).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SubClass {
    pub(super) key: (i32, i32),
    /// `ItemSubClass.dbc`'s name, `None` for the `(-1, -1)` group and an unknown pair.
    pub(super) name: Option<String>,
    shown: bool,
    collapsed: bool,
    /// The slot bits of its recipes, what `GetTradeSkillInvSlots` lists once it is filtered to.
    inv_slots: u32,
}

/// `DoTradeSkill`'s repeat: the running cast and count, and the one waiting for its cast start.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Recast {
    spell: u32,
    times: u32,
    pending_spell: u32,
    pending_times: u32,
}

/// The open window's state, every field one of the reference's statics.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TradeSkillModel {
    pub(crate) state: Option<TradeSkillState>,
    /// Visible entries first, in display order (`m_displayList`).
    pub(super) display: Vec<Entry>,
    /// `m_filteredSkills`, what `GetNumTradeSkills` answers.
    pub(super) filtered: usize,
    /// Sorted by class, then name (`QSortSubClasses 0x5da6b0`).
    pub(super) sub_classes: Vec<SubClass>,
    /// Bit i shows sub-class i (`0xad0e00`).
    pub(super) sub_class_filter: u32,
    /// The sub-class the menu last picked (`0xad0e04`), which narrows the slot menu; never reset
    /// by a line switch.
    pub(super) sub_class_filter_index: u32,
    /// Bit i shows slot i (`0xad0e08`).
    pub(super) inv_type_filter: u32,
    /// A clear bit i folds sub-class i (`0xad0e0c`).
    pub(super) collapse_filter: u32,
    /// The slot bits of every recipe built since the line opened (`0xc235d0`).
    pub(super) inv_slots: u32,
    pub(super) only_makeable: bool,
    pub(super) only_skill_ups: bool,
    /// Case-folded (`0xc235bc`).
    pub(super) name_filter: Option<String>,
    pub(super) min_level: i32,
    pub(super) max_level: i32,
    /// The selected recipe's spell id, 0 for none (`0xc235a8`).
    pub(super) selected: u32,
    /// The line the filters were last reset for (`0xc235f0`).
    last_line: u32,
    recast: Recast,
    /// `DoTradeSkill` casts for the app to send.
    pub(crate) casts: Vec<u32>,
    pub(crate) close: bool,
}

impl Default for TradeSkillModel {
    fn default() -> Self {
        Self {
            state: None,
            display: Vec::new(),
            filtered: 0,
            sub_classes: Vec::new(),
            sub_class_filter: ALL,
            sub_class_filter_index: ALL,
            inv_type_filter: ALL,
            collapse_filter: ALL,
            inv_slots: 0,
            only_makeable: false,
            only_skill_ups: false,
            name_filter: None,
            min_level: 0,
            max_level: 0,
            selected: 0,
            last_line: 0,
            recast: Recast::default(),
            casts: Vec::new(),
            close: false,
        }
    }
}

/// Bit `i` of `mask`, shifted as x86 `shl` does (count mod 32).
fn bit(mask: u32, i: usize) -> bool {
    mask & 1u32.wrapping_shl(i as u32) != 0
}

/// `SStrSortUTF8I`: case-insensitive, then raw bytes.
fn collate(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

impl TradeSkillModel {
    /// The open line's recipe behind display entry `e`.
    pub(super) fn recipe(&self, e: &Entry) -> Option<&TradeSkillRecipe> {
        self.state.as_ref()?.recipes.get(e.recipe?)
    }

    /// The display entry at a 1-based Lua index, visible or not (`index < m_numDisplayEntries`).
    pub(super) fn entry(&self, index: i64) -> Option<&Entry> {
        usize::try_from(index.checked_sub(1)?)
            .ok()
            .and_then(|i| self.display.get(i))
    }

    /// The recipe at a 1-based Lua index; `None` on a header or past the list.
    pub(super) fn recipe_at(&self, index: i64) -> Option<&TradeSkillRecipe> {
        self.entry(index).and_then(|e| self.recipe(e))
    }

    pub(super) fn has_entries(&self) -> bool {
        self.state.as_ref().is_some_and(|s| !s.recipes.is_empty())
    }

    /// The pushed snapshot; whether `UPDATE_TRADESKILL_RECAST` is owed (a close's stop).
    pub(super) fn set_state(&mut self, state: Option<TradeSkillState>) -> bool {
        let Some(s) = state else {
            self.state = None;
            self.display.clear();
            self.filtered = 0;
            return self.stop_recast();
        };
        if self.last_line != s.line {
            self.name_filter = None;
            self.min_level = 0;
            self.max_level = 0;
            self.only_makeable = false;
            self.sub_class_filter = ALL;
            self.inv_type_filter = ALL;
            self.collapse_filter = ALL;
            self.inv_slots = 0;
            self.selected = 0;
            self.last_line = s.line;
        }
        self.state = Some(s);
        self.refresh();
        false
    }

    /// `RefreshList(1)`: rebuild the entries and sub-classes, keeping each sub-class's shown and
    /// folded flags by key. A product template in flight holds the old list (`0x5dd5a0`).
    /// Deviation: the reference holds its old count over the new spells; an old list that no
    /// longer matches the recipes is emptied instead.
    fn refresh(&mut self) {
        let Some(s) = self.state.as_ref() else {
            return;
        };
        if s.recipes.iter().any(|r| r.group.is_none()) {
            let stale = self.display.iter().any(|e| {
                e.recipe
                    .is_some_and(|i| s.recipes.get(i).map(|r| r.spell_id) != Some(e.spell))
            });
            if stale {
                self.display.clear();
                self.filtered = 0;
            }
            return;
        }
        let saved: Vec<((i32, i32), bool, bool)> = self
            .sub_classes
            .iter()
            .map(|c| (c.key, c.shown, c.collapsed))
            .collect();
        let mut sub_classes: Vec<SubClass> = Vec::new();
        let mut display = Vec::with_capacity(s.recipes.len());
        for (i, r) in s.recipes.iter().enumerate() {
            let group = r.group.as_ref().expect("checked above");
            let key = group.key();
            self.inv_slots |= r.inv_slots;
            match sub_classes.iter_mut().find(|c| c.key == key) {
                Some(c) => c.inv_slots |= r.inv_slots,
                None => sub_classes.push(SubClass {
                    key,
                    name: match group {
                        super::TradeSkillGroup::Verb => None,
                        super::TradeSkillGroup::SubClass { name, .. } => name.clone(),
                    },
                    shown: true,
                    collapsed: false,
                    inv_slots: r.inv_slots,
                }),
            }
            display.push(Entry {
                recipe: Some(i),
                spell: r.spell_id,
                key,
                visible: true,
            });
        }
        for c in &mut sub_classes {
            if let Some(&(_, shown, collapsed)) = saved.iter().find(|(k, _, _)| *k == c.key) {
                c.shown = shown;
                c.collapsed = collapsed;
            }
        }
        display.extend(sub_classes.iter().map(|c| Entry {
            recipe: None,
            spell: 0,
            key: c.key,
            visible: true,
        }));
        self.sub_classes = sub_classes;
        self.display = display;
        self.sub_class_filter = ALL;
        self.collapse_filter = ALL;
        self.apply(false);
        for (i, c) in self.sub_classes.iter().enumerate() {
            let b = 1u32.wrapping_shl(i as u32);
            if !c.shown {
                self.sub_class_filter &= !b;
            }
            if c.collapsed {
                self.collapse_filter &= !b;
            }
        }
    }

    /// `Profession_FilterSpell` (`0x5dc110`): the product's required level within the level
    /// filter, then the name filter found in any searched text.
    fn passes_text_filters(&self, r: &TradeSkillRecipe) -> bool {
        if let Some(req) = r.required_level.map(|l| l as i32) {
            if (self.min_level != 0 && req < self.min_level)
                || (self.max_level != 0 && req > self.max_level)
            {
                return false;
            }
        }
        let Some(name) = self.name_filter.as_deref() else {
            return true;
        };
        r.search_text.iter().any(|t| t.contains(name))
    }

    /// `SetInvTypeFilter` (`0x5dcc40`): with `from_bits`, the masks set each sub-class's shown
    /// and folded flags first; then every entry's visibility, `filtered`, and the sort.
    pub(super) fn apply(&mut self, from_bits: bool) {
        if from_bits {
            for (i, c) in self.sub_classes.iter_mut().enumerate() {
                c.shown = bit(self.sub_class_filter, i);
                c.collapsed = !bit(self.collapse_filter, i);
            }
        }
        let mut filtered = self.display.len();
        let mut display = std::mem::take(&mut self.display);
        for e in &mut display {
            e.visible = true;
            let Some(r) = self.recipe(e) else { continue };
            if !self.sub_classes.is_empty() {
                let j = self
                    .sub_classes
                    .iter()
                    .position(|c| c.key == e.key)
                    .unwrap_or(0);
                if !self.sub_classes[j].shown {
                    e.visible = false;
                    filtered -= 1;
                    continue;
                }
            }
            if self.inv_type_filter & r.inv_slots == 0
                || (self.only_makeable && r.num_available == 0)
                || (self.only_skill_ups && r.difficulty.tier() > 2)
            {
                e.visible = false;
                filtered -= 1;
                continue;
            }
            if (self.name_filter.is_some() || self.min_level != 0 || self.max_level != 0)
                && !self.passes_text_filters(r)
            {
                e.visible = false;
                filtered -= 1;
            }
        }
        if !self.sub_classes.is_empty() {
            for i in 0..display.len() {
                if display[i].recipe.is_some() {
                    continue;
                }
                let key = display[i].key;
                let collapsed = self
                    .sub_classes
                    .iter()
                    .find(|c| c.key == key)
                    .is_some_and(|c| c.collapsed);
                let mut any = false;
                for (j, e) in display.iter_mut().enumerate() {
                    if i != j && e.key == key && e.visible {
                        any = true;
                        if collapsed {
                            e.visible = false;
                            filtered -= 1;
                        }
                    }
                }
                if !any {
                    display[i].visible = false;
                    filtered -= 1;
                }
            }
        }
        self.sub_classes.sort_by(|a, b| {
            a.key
                .0
                .cmp(&b.key.0)
                .then_with(|| match (&a.name, &b.name) {
                    (Some(x), Some(y)) => collate(x, y),
                    (None, None) => Ordering::Equal,
                    (Some(_), None) => Ordering::Greater,
                    (None, Some(_)) => Ordering::Less,
                })
        });
        display.sort_by(|a, b| self.compare(a, b));
        self.display = display;
        self.filtered = filtered;
    }

    /// `QSortSkills` (`0x5dae60`): visible entries of shown sub-classes first, by sub-class, the
    /// header first, then difficulty, trivialHigh and item level descending, and name.
    fn compare(&self, a: &Entry, b: &Entry) -> Ordering {
        if !self.sub_classes.is_empty() {
            let place = |e: &Entry| {
                self.sub_classes
                    .iter()
                    .position(|c| c.key == e.key)
                    .map_or((0, true), |i| (i, e.visible && self.sub_classes[i].shown))
            };
            let (ia, shown_a) = place(a);
            let (ib, shown_b) = place(b);
            match (shown_a, shown_b) {
                (false, true) => return Ordering::Greater,
                (false, false) => return Ordering::Equal,
                (true, false) => return Ordering::Less,
                (true, true) => {}
            }
            if ia != ib {
                return ia.cmp(&ib);
            }
        }
        let (Some(ra), Some(rb)) = (self.recipe(a), self.recipe(b)) else {
            return match (a.recipe, b.recipe) {
                (None, None) => Ordering::Equal,
                (_, None) => Ordering::Greater,
                (None, _) => Ordering::Less,
                _ => Ordering::Equal,
            };
        };
        ra.difficulty
            .tier()
            .cmp(&rb.difficulty.tier())
            .then_with(|| rb.trivial_high.cmp(&ra.trivial_high))
            .then_with(|| rb.item_level.cmp(&ra.item_level))
            .then_with(|| collate(&ra.name, &rb.name))
    }

    /// The sub-class whose header is display entry `index` (0-based), as
    /// `GetSubClassIndexFromSkill` (`0x5da7d0`).
    pub(super) fn sub_class_of_header(&self, index: usize) -> Option<usize> {
        let e = self.display.get(index).filter(|e| e.recipe.is_none())?;
        self.sub_classes.iter().position(|c| c.key == e.key)
    }

    /// Whether display entry `index` (0-based) is a folded header (`IsCollapsedHeader 0x5da830`).
    pub(super) fn is_collapsed_header(&self, index: usize) -> bool {
        self.sub_class_of_header(index)
            .is_some_and(|i| !bit(self.collapse_filter, i))
    }

    /// The selection's 0-based display position, visible or not (`GetSelectionIndex 0x5da670`).
    pub(super) fn selection_index(&self) -> Option<usize> {
        if self.selected == 0 {
            return None;
        }
        self.display
            .iter()
            .position(|e| self.recipe(e).is_some_and(|r| r.spell_id == self.selected))
    }

    /// The slot bits the slot menu lists: the picked sub-class's, else all (`GetInvSlotFilter
    /// 0x5dace0`).
    pub(super) fn listed_inv_slots(&self) -> u32 {
        let mask = self
            .sub_classes
            .get(self.sub_class_filter_index as usize)
            .map_or(ALL, |c| c.inv_slots);
        mask & self.inv_slots
    }

    /// `DoTradeSkill` (`0x5d9fa0`): a new count for the running repeat, or a pending one.
    pub(super) fn set_recast(&mut self, spell: u32, times: u32) {
        if spell == self.recast.spell {
            self.recast.times = times;
        } else {
            self.recast.pending_spell = spell;
            self.recast.pending_times = times;
        }
    }

    pub(super) fn recast_spell(&self) -> u32 {
        self.recast.spell
    }

    /// `GetTradeskillRepeatCount` (`0x5da270`): the selection's running or pending count, at
    /// least 1.
    pub(super) fn repeat_count(&self) -> u32 {
        let r = &self.recast;
        let count = if r.spell != 0 && r.spell == self.selected {
            r.times
        } else if r.pending_spell != 0 && r.pending_spell == self.selected {
            r.pending_times
        } else {
            1
        };
        count.max(1)
    }

    /// `StopRecast` (`0x5d9ff0`); whether a repeat stopped, which fires `UPDATE_TRADESKILL_RECAST`.
    pub(super) fn stop_recast(&mut self) -> bool {
        let active = self.recast.spell != 0 || self.recast.pending_spell != 0;
        self.recast.spell = 0;
        self.recast.pending_spell = 0;
        active
    }

    pub(super) fn commit_pending_recast(&mut self, spell: u32) {
        if spell == self.recast.pending_spell {
            self.recast.spell = self.recast.pending_spell;
            self.recast.times = self.recast.pending_times;
        }
        self.recast.pending_spell = 0;
        self.recast.pending_times = 0;
    }

    /// `DoRecast`: `(cast again, fire UPDATE_TRADESKILL_RECAST)`.
    pub(super) fn do_recast(&mut self, spell: u32) -> (bool, bool) {
        if self.recast.spell == 0 {
            return (false, self.stop_recast());
        }
        if self.recast.times == 0 || self.recast.spell != spell {
            return (false, self.stop_recast());
        }
        self.recast.times -= 1;
        if self.recast.times != 0 {
            (true, true)
        } else {
            self.recast.spell = 0;
            (false, false)
        }
    }

    /// `UpdateTradeSkillRecast`: whether a repeat of `spell` stopped.
    pub(super) fn cast_failed(&mut self, spell: u32) -> bool {
        let mut fire = false;
        if spell == self.recast.spell {
            self.recast.spell = 0;
            fire = true;
        }
        if spell == self.recast.pending_spell {
            self.recast.pending_spell = 0;
            fire = true;
        }
        fire
    }
}

/// The recipe at a 1-based Lua index; `None` on a header, so `SetTradeSkillItem` raises there.
pub(crate) fn recipe_at(model: &Model, index: i64) -> Option<&TradeSkillRecipe> {
    model.trade_skill.recipe_at(index)
}
