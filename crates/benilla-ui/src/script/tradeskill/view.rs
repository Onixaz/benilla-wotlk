//! The display list the tradeskill API indexes, the client's tradeskill statics
//! (`0xc235a0`-`0xc23604`), which the app holds: the rebuild (`0x5dd5a0`), the filter pass and
//! sort (`0x5dcc40`) and the repeat (`0x5d9fa0`, `0x5da070`, `0x5da0f0`).

use std::cmp::Ordering;

use super::TradeSkillRow;

/// Every filter bit set: all sub-classes shown, none folded, all slots shown.
const ALL: u32 = u32::MAX;

/// One display entry: a recipe, or the header of a sub-class (spell id `-1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Entry {
    /// Position in [`TradeSkillList::rows`], `None` for a header.
    pub(super) recipe: Option<usize>,
    /// The recipe's spell id, 0 for a header.
    spell: u32,
    pub(super) key: (i32, i32),
    visible: bool,
}

/// One sub-class record.
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
pub struct TradeSkillList {
    /// The open line (`[0xc235a4]`), 0 with none.
    pub line: u32,
    /// The opener's spell id (`[0xc235a0]`), the list link's.
    pub opener: u32,
    /// Product records the rebuild asked for and still waits on (`[0xc235ac]`).
    pub items_pending: u32,
    /// The entries' recipes as the last rebuild filed them; the display indexes them.
    pub(super) rows: Vec<TradeSkillRow>,
    /// Visible entries first, in display order.
    pub(super) display: Vec<Entry>,
    /// The filtered count, what `GetNumTradeSkills` answers (`[0xc235b8]`).
    pub(super) filtered: usize,
    /// Sorted by class, then name (`0x5da6b0`).
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
    /// `DoTradeSkill` casts for the app to send inside the call, each with the repeat count it
    /// sets once sent.
    pub casts: Vec<(u32, u32)>,
}

impl Default for TradeSkillList {
    fn default() -> Self {
        Self {
            line: 0,
            opener: 0,
            items_pending: 0,
            rows: Vec::new(),
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
        }
    }
}

/// Bit `i` of `mask`, shifted as x86 `shl` does (count mod 32).
fn bit(mask: u32, i: usize) -> bool {
    mask & 1u32.wrapping_shl(i as u32) != 0
}

/// The reference's UTF-8 collation: case-insensitive, then raw bytes.
fn collate(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

impl TradeSkillList {
    /// The recipe behind display entry `e`.
    pub(super) fn recipe(&self, e: &Entry) -> Option<&TradeSkillRow> {
        self.rows.get(e.recipe?)
    }

    /// The display entry at a 1-based Lua index, visible or not (below the display entry count).
    pub(super) fn entry(&self, index: i64) -> Option<&Entry> {
        usize::try_from(index.checked_sub(1)?)
            .ok()
            .and_then(|i| self.display.get(i))
    }

    /// The recipe at a 1-based Lua index; `None` on a header or past the list.
    pub fn recipe_at(&self, index: i64) -> Option<&TradeSkillRow> {
        self.entry(index).and_then(|e| self.recipe(e))
    }

    /// The display entry count (`[0xc235b0]`), headers included once the filter pass ran and the
    /// recipes alone while a product record is in flight: `TRADE_SKILL_SHOW`'s gate (`0x5de2d1`).
    pub fn entry_count(&self) -> usize {
        self.display.len()
    }

    /// The filtered count `GetNumTradeSkills` answers (`[0xc235b8]`).
    pub fn filtered(&self) -> usize {
        self.filtered
    }

    /// The open's line switch (`0x5de1f7`): a line other than the last resets the name, level and
    /// makeable filters, the sub-class, slot and fold masks, the slot bits and the selection
    /// (`0x5de264`-`0x5de2bb`).
    pub fn open_line(&mut self, line: u32, opener: u32) {
        self.line = line;
        self.opener = opener;
        if self.last_line != line {
            self.name_filter = None;
            self.min_level = 0;
            self.max_level = 0;
            self.only_makeable = false;
            self.sub_class_filter = ALL;
            self.inv_type_filter = ALL;
            self.collapse_filter = ALL;
            self.inv_slots = 0;
            self.selected = 0;
        }
    }

    /// The open's rebuild done: the line is the last one (`0x5de2d7`).
    pub fn opened(&mut self) {
        self.last_line = self.line;
    }

    /// The close (`0x5da5a0`, `0x5da8b0`): no line or opener; whether a repeat stopped, which
    /// fires `UPDATE_TRADESKILL_RECAST`.
    pub fn close(&mut self) -> bool {
        self.line = 0;
        self.opener = 0;
        self.stop_recast()
    }

    /// The game shutdown (`0x5dcaf0`, writes `0x5dcb5d`-`0x5dcc2a`): no line or opener, no
    /// entries, sub-classes, name filter or last line, no repeat. The masks, level range, makeable
    /// and skill-ups flags (`0xc235cc`, written only at `0x5dd43d`), the picked sub-class
    /// (`0xad0e04`), the selection and the slot bits stay.
    pub fn shutdown(&mut self) {
        self.line = 0;
        self.opener = 0;
        self.rows.clear();
        self.display.clear();
        self.filtered = 0;
        self.sub_classes.clear();
        self.name_filter = None;
        self.last_line = 0;
        self.recast.spell = 0;
        self.recast.pending_spell = 0;
    }

    /// The list refresh (`0x5dd5a0`) over the recipes `rows` the app filed, `None` with no line or
    /// player: with `full`, the sub-classes refiled from the recipes, each one's shown and folded
    /// flags restored by key; then the slot filter pass (`0x5dcc40`) with `passes` as the text
    /// filter (`0x5dc110`). Whether `TRADE_SKILL_UPDATE` fires (`0x5de0d5`): not with no rows or a
    /// product record in flight.
    pub fn refresh(
        &mut self,
        full: bool,
        rows: Option<Vec<TradeSkillRow>>,
        passes: &dyn Fn(u32) -> bool,
    ) -> bool {
        // A whole rebuild saves the flags, then drops the sub-classes and opens both masks
        // (`0x5dd638`, `0x5dd63e`, `0x5dd644`).
        let saved: Vec<((i32, i32), bool, bool)> = if full {
            let saved = self
                .sub_classes
                .iter()
                .map(|c| (c.key, c.shown, c.collapsed))
                .collect();
            self.sub_classes.clear();
            self.sub_class_filter = ALL;
            self.collapse_filter = ALL;
            saved
        } else {
            Vec::new()
        };
        let Some(rows) = rows else {
            self.display.clear();
            return false;
        };
        let first_miss = rows.iter().position(|r| r.group.is_none());
        for (i, r) in rows.iter().enumerate() {
            let before_miss = first_miss.is_none_or(|m| i < m);
            // Past a miss a product recipe is skipped (`0x5ddb08`); one without a product still
            // adds its slot bits (`0x5ddcfc`).
            if !before_miss && !matches!(r.group, Some(super::TradeSkillGroup::Verb)) {
                continue;
            }
            self.inv_slots |= r.inv_slots;
            // Sub-classes file only on a whole rebuild with nothing in flight (`0x5ddd02`), a new
            // one shown and unfolded (`0x5dde6e`).
            if !(before_miss && full) {
                continue;
            }
            let group = r.group.as_ref().expect("filed before any miss");
            let key = group.key();
            match self.sub_classes.iter_mut().find(|c| c.key == key) {
                Some(c) => c.inv_slots |= r.inv_slots,
                None => self.sub_classes.push(SubClass {
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
        }
        if first_miss.is_some() {
            // In flight (`0x5ddeb7`): the recipes are filed and counted (`0x5dd889`); the saved
            // flags, headers, filter pass and sort never run, and the filtered count stands.
            let old = std::mem::take(&mut self.display);
            self.rows = rows;
            self.display = self
                .rows
                .iter()
                .enumerate()
                .map(|(i, r)| Entry {
                    recipe: Some(i),
                    spell: r.spell_id,
                    key: (-1, -1),
                    visible: old.get(i).is_none_or(|e| e.visible),
                })
                .collect();
            return false;
        }
        self.rows = rows;
        if !full {
            // The entries refiled in place: the sub-classes and their flags stand.
            self.display = self.entries(&self.sub_classes);
            self.apply(false, passes);
            return true;
        }
        // The saved flags back on the sub-classes that remain (`0x5ddec4`).
        for c in &mut self.sub_classes {
            if let Some(&(_, shown, collapsed)) = saved.iter().find(|(k, _, _)| *k == c.key) {
                c.shown = shown;
                c.collapsed = collapsed;
            }
        }
        self.display = self.entries(&self.sub_classes);
        self.apply(false, passes);
        for (i, c) in self.sub_classes.iter().enumerate() {
            let b = 1u32.wrapping_shl(i as u32);
            if !c.shown {
                self.sub_class_filter &= !b;
            }
            if c.collapsed {
                self.collapse_filter &= !b;
            }
        }
        true
    }

    /// Each sub-class's key, shown and folded flags, in list order.
    #[cfg(test)]
    pub(super) fn sub_class_flags(&self) -> Vec<((i32, i32), bool, bool)> {
        self.sub_classes
            .iter()
            .map(|c| (c.key, c.shown, c.collapsed))
            .collect()
    }

    /// The recipes' entries in row order, then one header per sub-class.
    fn entries(&self, sub_classes: &[SubClass]) -> Vec<Entry> {
        let mut display = Vec::with_capacity(self.rows.len() + sub_classes.len());
        for (i, r) in self.rows.iter().enumerate() {
            let key = r.group.as_ref().map_or((-1, -1), |g| g.key());
            display.push(Entry {
                recipe: Some(i),
                spell: r.spell_id,
                key,
                visible: true,
            });
        }
        display.extend(sub_classes.iter().map(|c| Entry {
            recipe: None,
            spell: 0,
            key: c.key,
            visible: true,
        }));
        display
    }

    /// The text filter's inputs (`[0xc235bc]`, `[0xc235c0]`, `[0xc235c4]`): the folded name, the
    /// minimum and the maximum level.
    pub fn text_filter(&self) -> (Option<&str>, i32, i32) {
        (self.name_filter.as_deref(), self.min_level, self.max_level)
    }

    /// The recipes' spell ids, the ones the text filter can test.
    pub fn spells(&self) -> impl Iterator<Item = u32> + '_ {
        self.rows.iter().map(|r| r.spell_id)
    }

    /// The slot filter pass (`0x5dcc40`): with `from_bits`, the masks set each sub-class's shown
    /// and folded flags first; then every entry's visibility, `filtered`, and the sort. `passes`
    /// is the text filter (`0x5dc110`), asked only with a name or level filter set.
    pub fn apply(&mut self, from_bits: bool, passes: &dyn Fn(u32) -> bool) {
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
                && !passes(r.spell_id)
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

    /// The recipe sort (`0x5dae60`): visible entries of shown sub-classes first, by sub-class, the
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

    /// The sub-class whose header is display entry `index` (0-based), as the reference resolves it
    /// (`0x5da7d0`).
    pub(super) fn sub_class_of_header(&self, index: usize) -> Option<usize> {
        let e = self.display.get(index).filter(|e| e.recipe.is_none())?;
        self.sub_classes.iter().position(|c| c.key == e.key)
    }

    /// Whether display entry `index` (0-based) is a folded header (`0x5da830`).
    pub(super) fn is_collapsed_header(&self, index: usize) -> bool {
        self.sub_class_of_header(index)
            .is_some_and(|i| !bit(self.collapse_filter, i))
    }

    /// The selection's 0-based display position, visible or not (`0x5da670`).
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
    pub fn set_recast(&mut self, spell: u32, times: u32) {
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

    /// The repeat stop (`0x5d9ff0`); whether a repeat stopped, which fires
    /// `UPDATE_TRADESKILL_RECAST`.
    pub fn stop_recast(&mut self) -> bool {
        let active = self.recast.spell != 0 || self.recast.pending_spell != 0;
        self.recast.spell = 0;
        self.recast.pending_spell = 0;
        active
    }

    /// Our cast of `spell` started (`0x5da0f0`): a pending repeat of it becomes the running one,
    /// and any other is dropped.
    pub fn commit_pending_recast(&mut self, spell: u32) {
        if spell == self.recast.pending_spell {
            self.recast.spell = self.recast.pending_spell;
            self.recast.times = self.recast.pending_times;
        }
        self.recast.pending_spell = 0;
        self.recast.pending_times = 0;
    }

    /// The repeat's next cast (`0x5da070`): `(cast again, fire UPDATE_TRADESKILL_RECAST)`.
    pub fn do_recast(&mut self, spell: u32) -> (bool, bool) {
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

    /// The repeat update on a failed cast (`0x5da020`): how many `UPDATE_TRADESKILL_RECAST` fire,
    /// one per repeat of `spell` that stopped.
    pub fn cast_failed(&mut self, spell: u32) -> usize {
        let mut fire = 0;
        if spell == self.recast.spell {
            self.recast.spell = 0;
            fire += 1;
        }
        if spell == self.recast.pending_spell {
            self.recast.pending_spell = 0;
            fire += 1;
        }
        fire
    }
}
