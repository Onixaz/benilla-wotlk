//! The client's talent state (`0xc20ff0`-`0xc21138`), engine-free: the host keeps it in its own
//! state, its packet handlers write it, and the talent verbs read and write it at the call through
//! the live view. Three sides: the player's class tabs (`0xc21018`) and two groups (`0xc21030`),
//! the pet's family tabs (`0xc21024`) and one group (`0xc210d8`), and the inspected player's tabs
//! (`0xc2112c`) and two groups (`0xc21138`).
//!
//! A group keeps each talent's 0-based rank and preview rank by talent id (`0x5c9310`), and per
//! tab, laid over the side's tabs (`0x5c9960`), the points spent, the preview points on top and the
//! talents it holds by tier, which the preview gate and the preview learn walk (`0x5c73d0`,
//! `0x5c6a10`).

use std::sync::Arc;

use super::talent::{TalentTabView, TalentView};

/// One side's tabs as the tab build lays them (`0x5c64d0`): the `TalentTab.dbc` rows the class or
/// the pet family takes, each with its `Talent.dbc` rows in file order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TalentPages {
    pub tabs: Vec<TalentTabView>,
    /// `talents[t]` holds tab `t`'s talents in `Talent.dbc` row order.
    pub talents: Vec<Vec<TalentView>>,
}

impl TalentPages {
    /// Tab `index`'s view and talents, `None` past the tabs.
    pub fn tab(&self, index: usize) -> Option<(&TalentTabView, &[TalentView])> {
        Some((self.tabs.get(index)?, self.talents.get(index)?.as_slice()))
    }

    /// The talent `id` and its tab's index, in any tab.
    fn find(&self, id: u32) -> Option<(usize, &TalentView)> {
        self.talents
            .iter()
            .enumerate()
            .find_map(|(t, list)| list.iter().find(|v| v.id == id).map(|v| (t, v)))
    }
}

/// One talent a group holds: its 0-based rank and preview rank, -1 for none (`0x5c9310`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TalentItem {
    pub talent: u32,
    pub rank: i32,
    pub preview: i32,
}

/// One tier of a group's tab: the talents it holds, in the order they were added (`0x5c9230`).
#[derive(Clone, Debug, Default, PartialEq)]
struct GroupTier {
    /// 0-based, as the record's tier.
    tier: u32,
    talents: Vec<u32>,
}

/// One tab of a group (`0x5c9960`): the points spent, the preview points on top and the tiers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GroupTab {
    pub tab_id: u32,
    pub spent: i32,
    pub preview_spent: i32,
    /// Sorted by tier (`0x5c5750`).
    tiers: Vec<GroupTier>,
}

impl GroupTab {
    /// Seat `talent` in its tier (`0x5c9230`), a new tier sorted in.
    fn add_item(&mut self, talent: &TalentView) {
        let tier = talent.tier.saturating_sub(1);
        match self.tiers.iter_mut().find(|t| t.tier == tier) {
            // The reference seats a talent again on a second lay-out of the same tabs (a new node
            // per call); one seat per talent here, as the walks over it expect.
            Some(t) if !t.talents.contains(&talent.id) => t.talents.push(talent.id),
            Some(_) => {}
            None => {
                self.tiers.push(GroupTier {
                    tier,
                    talents: vec![talent.id],
                });
                self.tiers.sort_by_key(|t| t.tier);
            }
        }
    }
}

/// One talent group (a spec): its talents, its tabs, its unspent points and its six glyphs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TalentGroup {
    items: Vec<TalentItem>,
    tabs: Vec<GroupTab>,
    /// `GroupInfo+0x38`.
    pub unspent: i32,
    /// `GlyphProperties.dbc` ids, 0 for none (`+0x3c`, `0x5c987b`).
    pub glyphs: [u32; 6],
}

impl TalentGroup {
    /// The group emptied (`0x5c9910`): no talents, no tabs, no points, no glyphs.
    fn reset(&mut self) {
        *self = TalentGroup::default();
    }

    /// The talent `id`'s ranks (`0x5c73b0`), `None` for one the group does not hold.
    pub fn item(&self, id: u32) -> Option<&TalentItem> {
        self.items.iter().find(|i| i.talent == id)
    }

    fn item_mut(&mut self, id: u32) -> Option<&mut TalentItem> {
        self.items.iter_mut().find(|i| i.talent == id)
    }

    /// Tab `index` of the group (`0x5c5b00`), `None` past the tabs it was laid over.
    pub fn tab(&self, index: usize) -> Option<&GroupTab> {
        self.tabs.get(index)
    }

    /// The preview points across the group's tabs (`0x5c6010`).
    pub fn preview_spent(&self) -> i32 {
        self.tabs.iter().map(|t| t.preview_spent).sum()
    }

    /// A packet's talent (`0x5c9310`): a rank below 0, past 9, naming no rank spell or no spell
    /// record (`valid`) is dropped; else rank and preview rank are the packet's.
    fn add_talent_item(&mut self, id: u32, rank: i32, valid: &dyn Fn(u32, i32) -> bool) {
        if !(0..9).contains(&rank) || !valid(id, rank) {
            return;
        }
        // The packet readers empty the group first, so a talent seen again has no tab to move.
        match self.item_mut(id) {
            Some(item) => {
                item.rank = rank;
                item.preview = rank;
            }
            None => self.items.push(TalentItem {
                talent: id,
                rank,
                preview: rank,
            }),
        }
    }

    /// The group laid over `pages` (`0x5c9960`): one tab each, its spent points the ranks it holds
    /// plus one each, its tiers the talents it holds. A tab already there keeps its preview points
    /// and tiers, as the resize keeps its elements (`0x5c9978`).
    fn update_tabs(&mut self, pages: &TalentPages) {
        self.tabs.resize_with(pages.tabs.len(), GroupTab::default);
        for (tab, (view, list)) in self
            .tabs
            .iter_mut()
            .zip(pages.tabs.iter().zip(&pages.talents))
        {
            tab.tab_id = view.id;
            tab.spent = 0;
            for talent in list {
                if let Some(item) = self.items.iter().find(|i| i.talent == talent.id) {
                    tab.spent += item.rank + 1;
                    tab.add_item(talent);
                }
            }
        }
    }

    /// Every preview rank back to its rank and every tab's preview points to 0 (`0x5c6fb0`).
    pub fn reset_all_preview(&mut self) {
        for item in &mut self.items {
            item.preview = item.rank;
        }
        for tab in &mut self.tabs {
            tab.preview_spent = 0;
        }
    }

    /// Tab `index`'s preview ranks back to its ranks (`0x5c6f20`), nothing past the tabs.
    pub fn reset_preview(&mut self, index: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        for id in tab.tiers.iter().flat_map(|t| &t.talents) {
            if let Some(item) = self.items.iter_mut().find(|i| i.talent == *id) {
                item.preview = item.rank;
            }
        }
        tab.preview_spent = 0;
    }

    /// The preview gate (`0x5c73d0`): the preview delta `points` would apply to `talent` in tab
    /// `index`, `None` when refused. `find` reads a talent record by id from the side's tabs.
    pub fn validate_preview(
        &self,
        index: usize,
        talent: &TalentView,
        points: i32,
        pet: bool,
        find: &dyn Fn(u32) -> Option<TalentView>,
    ) -> Option<i32> {
        let tab = self.tabs.get(index)?;
        if points == 0 {
            return Some(0);
        }
        // No point left to raise with (`0x5c742e`).
        if self.unspent - self.preview_spent() <= 0 && points > 0 {
            return None;
        }
        let tier = talent.tier.saturating_sub(1) as i32;
        let per_tier = if pet { 3 } else { 5 };
        // The tier's lock (`0x5c747b`).
        if tab.spent + tab.preview_spent < tier * per_tier {
            return None;
        }
        let item = self.item(talent.id);
        let rank = item.map_or(-1, |i| i.rank);
        let preview = item.map_or(-1, |i| i.preview);
        // Each prerequisite with a record held at its rank in preview (`0x5c74cb`).
        for p in &talent.prereqs {
            if p.seat.is_none() {
                continue;
            }
            if self
                .item(p.talent)
                .is_none_or(|i| i.preview < p.rank as i32)
            {
                return None;
            }
        }
        // The highest rank with a spell, the rank itself for none (`0x5c74f4`).
        let max = if talent.ranks.is_empty() {
            rank
        } else {
            talent.ranks.len() as i32 - 1
        };
        let room = max - rank;
        let wanted = preview + points - rank;
        let clamped = if wanted < 0 {
            0
        } else if wanted > room {
            room
        } else {
            wanted
        };
        let change = rank + clamped - preview;
        if change == 0 {
            return None;
        }
        if change > 0 {
            return Some(change);
        }
        // A dependent holding a rank keeps its prerequisite's (`0x5c7581`).
        for other in &self.items {
            if other.preview < 0 {
                continue;
            }
            let Some(rec) = find(other.talent) else {
                continue;
            };
            for p in &rec.prereqs {
                if p.talent == talent.id && item.is_some_and(|i| change + i.preview < p.rank as i32)
                {
                    return None;
                }
            }
        }
        // Every higher tier holding points keeps the points under it (`0x5c7631`).
        if tab.tiers.len() <= 1 {
            return Some(change);
        }
        let mut total = 0;
        for t in &tab.tiers {
            let mut here = 0;
            for id in &t.talents {
                let Some(i) = self.item(*id) else {
                    continue;
                };
                here += i.preview + 1;
                total += i.preview + 1 + if *id == talent.id { change } else { 0 };
            }
            if here > 0 && t.tier as i32 > tier && total - here < per_tier * t.tier as i32 {
                return None;
            }
        }
        Some(change)
    }

    /// The preview move (`0x5c9450`): the gate's delta applied to `talent` in tab `index`, a
    /// talent not yet held seated first; the delta, `None` when refused.
    pub fn add_preview(
        &mut self,
        index: usize,
        talent: &TalentView,
        points: i32,
        pet: bool,
        find: &dyn Fn(u32) -> Option<TalentView>,
    ) -> Option<i32> {
        let delta = self.validate_preview(index, talent, points, pet, find)?;
        if self.item(talent.id).is_none() {
            self.items.push(TalentItem {
                talent: talent.id,
                rank: -1,
                preview: -1,
            });
            self.tabs[index].add_item(talent);
        }
        if let Some(item) = self.item_mut(talent.id) {
            item.preview += delta;
        }
        self.tabs[index].preview_spent += delta;
        Some(delta)
    }

    /// The preview learn's walk over each tab (`0x5c6b49`) and tier (`0x5c6b80`): each seated
    /// talent whose preview rank is past its rank (`0x5c6ba4`), as `(talentId, previewRank)`.
    pub fn preview_learns(&self) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for tab in &self.tabs {
            for tier in &tab.tiers {
                for id in &tier.talents {
                    if let Some(i) = self.item(*id) {
                        if i.preview > i.rank {
                            out.push((i.talent, i.preview as u32));
                        }
                    }
                }
            }
        }
        out
    }
}

/// One preview learn as sent: the pet's guid for the pet's form, and its `(talentId, previewRank)`s.
pub type PreviewLearn = (Option<u64>, Vec<(u32, u32)>);

/// One player talent body: the unspent points, the groups and the active one (`0x5c9740`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TalentPacket {
    pub unspent: u32,
    pub active: u8,
    pub groups: Vec<TalentPacketGroup>,
}

/// One group of a talent body: its `(talentId, rank)` pairs and its glyphs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TalentPacketGroup {
    pub talents: Vec<(u32, u8)>,
    pub glyphs: Vec<u16>,
}

/// A side's two groups with their count, active one and the packet's unspent points.
#[derive(Clone, Debug, Default, PartialEq)]
struct Groups {
    groups: [TalentGroup; 2],
    active: u32,
    count: u32,
    unspent: i32,
}

impl Groups {
    /// The groups emptied (`0x5c9a30`, `0x5c9ad0`).
    fn reset(&mut self) {
        *self = Groups::default();
    }

    /// The body read into the groups (`0x5c9740`): a negative unspent reads 0, a group past two is
    /// read and dropped, the first six glyphs kept.
    fn read(&mut self, p: &TalentPacket, valid: &dyn Fn(u32, i32) -> bool) {
        self.unspent = (p.unspent as i32).max(0);
        self.count = u32::from(p.groups.len().min(255) as u8);
        self.active = u32::from(p.active);
        for (g, body) in p.groups.iter().enumerate() {
            let Some(group) = self.groups.get_mut(g) else {
                continue;
            };
            for &(id, rank) in &body.talents {
                group.add_talent_item(id, i32::from(rank), valid);
            }
            for (slot, &glyph) in group.glyphs.iter_mut().zip(&body.glyphs) {
                *slot = u32::from(glyph);
            }
        }
    }

    /// The groups laid over `pages` (`0x5c9ca0`), with no group read nothing: the active group's
    /// unspent is the packet's, each other's the total less its own spent, the total being the
    /// packet's unspent plus the active group's spent.
    fn update(&mut self, pages: &TalentPages) {
        if self.count == 0 {
            return;
        }
        for g in &mut self.groups {
            g.update_tabs(pages);
        }
        let spent = |g: &TalentGroup| -> i32 { g.tabs.iter().map(|t| t.spent).sum() };
        let mut total = self.unspent;
        if self.active < self.count {
            if let Some(g) = self.groups.get_mut(self.active as usize) {
                total = spent(g) + self.unspent;
                g.unspent = self.unspent;
            }
        }
        for (i, g) in self.groups.iter_mut().enumerate() {
            if i as u32 != self.active {
                g.unspent = (total - spent(g)).max(0);
            }
        }
    }

    /// Group `index` while the count holds it (`0x5c5fc0`, `0x5c6080`).
    fn get(&self, index: u32) -> Option<&TalentGroup> {
        (index < self.count).then(|| self.groups.get(index as usize))?
    }

    fn get_mut(&mut self, index: u32) -> Option<&mut TalentGroup> {
        (index < self.count).then(|| self.groups.get_mut(index as usize))?
    }
}

/// The client's talent state: the player's, the pet's and the inspected player's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TalentInfo {
    /// The player's tabs (`0xc21018`), built once per world entry (`0xc20ff0`).
    tabs: Arc<TalentPages>,
    tabs_built: bool,
    player: Groups,
    /// The pet's tabs (`0xc21024`), the family they were built for (`0xc21000`) and its group.
    pet_tabs: Arc<TalentPages>,
    pet_family: i32,
    pet_group: TalentGroup,
    /// The inspected player (`0xc21008`), its tabs (`0xc2112c`) and groups.
    inspect_target: u64,
    inspect_tabs: Arc<TalentPages>,
    inspect: Groups,
}

/// Which side a verb reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TalentSide {
    Player,
    Pet,
    Inspect,
}

impl TalentInfo {
    /// The player's talent body (`0x5c9e50`): the groups emptied and read, laid over the tabs once
    /// built; `ACTIVE_TALENT_GROUP_CHANGED`'s `(new, old)` when the active group moved, 1-based and
    /// 0 for a side with no group (`0x5c9ecf`), raised after `PLAYER_TALENT_UPDATE`.
    pub fn update_talent_data(
        &mut self,
        p: &TalentPacket,
        valid: &dyn Fn(u32, i32) -> bool,
    ) -> Option<(i64, i64)> {
        let (old_count, old_active) = (self.player.count, self.player.active);
        self.player.reset();
        self.player.read(p, valid);
        if self.tabs_built {
            let tabs = self.tabs.clone();
            self.player.update(&tabs);
        }
        (old_active != self.player.active).then(|| {
            let new = if self.player.count != 0 {
                i64::from(self.player.active) + 1
            } else {
                0
            };
            let old = if old_count != 0 {
                i64::from(old_active) + 1
            } else {
                0
            };
            (new, old)
        })
    }

    /// The pet's talent body (`0x5c9f10`): its group emptied and read (`0x5c98a0`), laid over the
    /// family's tabs while it has a family.
    pub fn update_pet_talent_data(
        &mut self,
        unspent: u32,
        talents: &[(u32, u8)],
        valid: &dyn Fn(u32, i32) -> bool,
    ) {
        self.pet_group.reset();
        self.pet_group.unspent = unspent as i32;
        for &(id, rank) in talents {
            self.pet_group.add_talent_item(id, i32::from(rank), valid);
        }
        if self.pet_family != 0 {
            let tabs = self.pet_tabs.clone();
            self.pet_group.update_tabs(&tabs);
        }
    }

    /// The world entry (`0x5c9d50`): the player's tabs built once (`build`, `None` without our
    /// player, where the tabs stay), then the groups laid over them.
    pub fn enter_world(&mut self, build: impl FnOnce() -> Option<TalentPages>) {
        if self.tabs_built {
            return;
        }
        if let Some(pages) = build() {
            self.tabs = Arc::new(pages);
        }
        self.tabs_built = true;
        let tabs = self.tabs.clone();
        self.player.update(&tabs);
    }

    /// The inspected side's world entry and leave (`0x5c57c0`): the target forgotten, its tabs kept.
    pub fn inspect_enter_world(&mut self) {
        self.inspect_target = 0;
    }

    /// The world leave (`0x5c5790`): the tabs rebuild at the next entry, the pet's at its next
    /// family; both keep their pages.
    pub fn leave_world(&mut self) {
        self.tabs_built = false;
        self.pet_family = 0;
    }

    /// The game shutdown (`0x5c9dc0`, `0x5c9f60`): every side's tabs and groups emptied.
    pub fn shutdown(&mut self) {
        let built = self.tabs_built;
        *self = TalentInfo {
            tabs_built: built,
            ..TalentInfo::default()
        };
    }

    /// The pet setter's tab update (`0x5c9df0`): a new family rebuilds the tabs (`build`, `None`
    /// for a family with no tree, where the tabs stay) and lays the group over them; no family
    /// empties both.
    pub fn update_pet_tabs(&mut self, family: i32, build: impl FnOnce() -> Option<TalentPages>) {
        if family != 0 {
            if self.pet_family != family {
                if let Some(pages) = build() {
                    self.pet_tabs = Arc::new(pages);
                }
                let tabs = self.pet_tabs.clone();
                self.pet_group.update_tabs(&tabs);
            }
        } else {
            self.pet_tabs = Arc::default();
            self.pet_group.reset();
        }
        self.pet_family = family;
    }

    /// The inspect target set (`0x5c9f70`): the tabs emptied, then, for a held player (`pages`),
    /// its class's tabs, the target and the groups laid over them.
    pub fn set_inspect_target(&mut self, guid: u64, pages: Option<TalentPages>) {
        self.inspect_tabs = Arc::default();
        if let Some(pages) = pages {
            self.inspect_tabs = Arc::new(pages);
            self.inspect_target = guid;
            let tabs = self.inspect_tabs.clone();
            self.inspect.update(&tabs);
        }
    }

    /// The inspected player's talent body (`0x5c9ff0`): the groups emptied and read, laid over its
    /// tabs while a target is held.
    pub fn update_inspect_talent_data(
        &mut self,
        p: &TalentPacket,
        valid: &dyn Fn(u32, i32) -> bool,
    ) {
        self.inspect.reset();
        self.inspect.read(p, valid);
        if self.inspect_target != 0 {
            let tabs = self.inspect_tabs.clone();
            self.inspect.update(&tabs);
        }
    }

    /// A side's tabs (`0x5c5c60`): the inspected pet has none.
    pub fn pages(&self, side: TalentSide) -> &TalentPages {
        match side {
            TalentSide::Player => &self.tabs,
            TalentSide::Pet => &self.pet_tabs,
            TalentSide::Inspect => &self.inspect_tabs,
        }
    }

    /// A side's tabs, shared.
    pub fn pages_shared(&self, side: TalentSide) -> Arc<TalentPages> {
        match side {
            TalentSide::Player => self.tabs.clone(),
            TalentSide::Pet => self.pet_tabs.clone(),
            TalentSide::Inspect => self.inspect_tabs.clone(),
        }
    }

    /// Whether the pet has a family and tabs, the pet group's gate (`0x5c5fc0`).
    pub fn pet_has_tree(&self) -> bool {
        self.pet_family != 0 && !self.pet_tabs.tabs.is_empty()
    }

    /// A side's group `index` (`0x5c6080`, `0x5c5fc0`): the pet's only one while it has a tree.
    pub fn group(&self, side: TalentSide, index: u32) -> Option<&TalentGroup> {
        match side {
            TalentSide::Player => self.player.get(index),
            TalentSide::Inspect => self.inspect.get(index),
            TalentSide::Pet => (self.pet_has_tree() && index < 1).then_some(&self.pet_group),
        }
    }

    /// The player's or the pet's group `index`, for the preview verbs (`0x5c5fc0`).
    pub fn group_mut(&mut self, pet: bool, index: u32) -> Option<&mut TalentGroup> {
        if pet {
            (self.pet_has_tree() && index < 1).then_some(&mut self.pet_group)
        } else {
            self.player.get_mut(index)
        }
    }

    /// The player's group count (`0xc20ff8`).
    pub fn num_groups(&self) -> u32 {
        self.player.count
    }

    /// The inspected player's group count (`0xc21010`).
    pub fn num_inspect_groups(&self) -> u32 {
        self.inspect.count
    }

    /// The player's active group, 0-based (`0xc20ff4`).
    pub fn active_group(&self) -> u32 {
        self.player.active
    }

    /// The inspected player's active group, 0-based (`0xc21004`).
    pub fn inspect_active_group(&self) -> u32 {
        self.inspect.active
    }

    /// The preview points across every player group (or the pet's while it has a tree)
    /// (`0x5c5be0`).
    pub fn sum_preview_spent(&self, pet: bool) -> i32 {
        if pet {
            if self.pet_has_tree() {
                self.pet_group.preview_spent()
            } else {
                0
            }
        } else {
            (0..self.player.count)
                .filter_map(|g| self.player.get(g))
                .map(TalentGroup::preview_spent)
                .sum()
        }
    }

    /// Every player group's (or the pet's) preview back to its ranks (`0x5c70d0`).
    pub fn recalculate_preview(&mut self, pet: bool) {
        if pet {
            if self.pet_has_tree() {
                self.pet_group.reset_all_preview();
            }
        } else {
            for g in 0..self.player.count {
                if let Some(group) = self.player.get_mut(g) {
                    group.reset_all_preview();
                }
            }
        }
    }

    /// The talent record `id` from a side's tabs, as the gate reads a dependent's.
    pub fn find(&self, side: TalentSide, id: u32) -> Option<TalentView> {
        self.pages(side).find(id).map(|(_, v)| v.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::super::talent::{TalentPrereqView, TalentRankView};
    use super::*;

    fn rank(spell: u32) -> TalentRankView {
        TalentRankView {
            spell,
            name: "T".into(),
            icon: None,
        }
    }

    /// One tab: A (tier 1, three ranks), B (tier 1, one rank), C (tier 2, needs A at rank 3).
    fn pages() -> TalentPages {
        TalentPages {
            tabs: vec![TalentTabView {
                id: 41,
                name: "Fire".into(),
                ..Default::default()
            }],
            talents: vec![vec![
                TalentView {
                    id: 10,
                    tier: 1,
                    column: 1,
                    ranks: vec![rank(100), rank(101), rank(102)],
                    ..Default::default()
                },
                TalentView {
                    id: 11,
                    tier: 1,
                    column: 2,
                    ranks: vec![rank(110)],
                    ..Default::default()
                },
                TalentView {
                    id: 12,
                    tier: 2,
                    column: 1,
                    ranks: vec![rank(120)],
                    prereqs: vec![TalentPrereqView {
                        talent: 10,
                        rank: 2,
                        seat: Some((1, 1)),
                        spell: 1,
                        name: Some("A".into()),
                    }],
                    ..Default::default()
                },
            ]],
        }
    }

    fn packet(unspent: u32, active: u8, groups: Vec<Vec<(u32, u8)>>) -> TalentPacket {
        TalentPacket {
            unspent,
            active,
            groups: groups
                .into_iter()
                .map(|talents| TalentPacketGroup {
                    talents,
                    glyphs: vec![7, 8],
                })
                .collect(),
        }
    }

    fn any(_: u32, _: i32) -> bool {
        true
    }

    /// A body ahead of the world entry holds the ranks with no tabs; the entry lays them, and the
    /// other group's unspent is the total less its own spent (`0x5c9e50`, `0x5c9d50`, `0x5c9ca0`).
    #[test]
    fn a_body_before_the_world_entry_is_laid_at_the_entry() {
        let mut info = TalentInfo::default();
        let moved = info.update_talent_data(&packet(3, 1, vec![vec![(10, 1)], vec![]]), &any);
        assert_eq!(moved, Some((2, 0)), "new 2, old 0: no group before");
        let g = info.group(TalentSide::Player, 0).unwrap();
        assert_eq!(g.item(10).map(|i| (i.rank, i.preview)), Some((1, 1)));
        assert!(g.tab(0).is_none(), "no tab before the entry");
        assert_eq!(g.glyphs[..2], [7, 8]);
        info.enter_world(|| Some(pages()));
        let g0 = info.group(TalentSide::Player, 0).unwrap();
        assert_eq!(g0.tab(0).map(|t| t.spent), Some(2));
        assert_eq!(
            (
                g0.unspent,
                info.group(TalentSide::Player, 1).unwrap().unspent
            ),
            (1, 3),
            "group 2 is active with the packet's 3 and nothing spent: group 1 spent 2 of the 3"
        );
        assert!(info.group(TalentSide::Player, 2).is_none());
    }

    /// The preview gate: locked tier, the clamp at the max, and the dependent and tier holds on a
    /// lowering (`0x5c73d0`).
    #[test]
    fn the_preview_gate_is_12340s() {
        let mut info = TalentInfo::default();
        info.enter_world(|| Some(pages()));
        info.update_talent_data(&packet(5, 0, vec![vec![(10, 1)]]), &any);
        let p = pages();
        let find = |id| p.find(id).map(|(_, v)| v.clone());
        let (a, c) = (p.talents[0][0].clone(), p.talents[0][2].clone());
        let g = info.group_mut(false, 0).unwrap();
        assert_eq!(g.add_preview(0, &c, 1, false, &find), None, "tier 2 locked");
        assert_eq!(
            g.add_preview(0, &a, 5, false, &find),
            Some(1),
            "A clamps at max"
        );
        assert_eq!(g.preview_spent(), 1);
        assert_eq!(g.add_preview(0, &a, -5, false, &find), Some(-1));
        assert_eq!(
            g.add_preview(0, &a, -1, false, &find),
            None,
            "never under the rank"
        );
        assert_eq!(g.preview_learns(), Vec::<(u32, u32)>::new());
        g.add_preview(0, &a, 1, false, &find);
        let b = p.talents[0][1].clone();
        g.add_preview(0, &b, 1, false, &find);
        assert_eq!(g.preview_learns(), vec![(10, 2), (11, 0)]);
        g.reset_all_preview();
        assert_eq!(g.preview_spent(), 0);
    }

    /// A group with no tab laid at the index takes no move, a zero one included, and never indexes
    /// past its tabs: 12340 checks the index against the info's tabs (`0x5c951e`) and lays every
    /// group over them, so it never reads a missing one.
    #[test]
    fn a_move_on_an_unlaid_tab_takes_nothing() {
        let mut info = TalentInfo::default();
        info.update_talent_data(&packet(5, 0, vec![vec![]]), &any);
        let p = pages();
        let find = |id| p.find(id).map(|(_, v)| v.clone());
        let a = p.talents[0][0].clone();
        let g = info.group_mut(false, 0).unwrap();
        assert_eq!(g.add_preview(0, &a, 0, false, &find), None);
        assert_eq!(g.add_preview(0, &a, 1, false, &find), None);
        assert!(g.item(10).is_none());
    }
}
