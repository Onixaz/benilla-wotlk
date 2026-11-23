//! 3.3.5a's achievement engine: the three DBC tables ([`AchievementDb`]), the client's lists
//! ([`AchievementBook`]: the player's completed list and criteria progress, an inspected player's,
//! the tracked list), the packet handlers' steps ([`handle_achievement_packet`]), and the 37
//! bindings registered from `0xacfc70` bar the two quest-completion ones.
//!
//! The app holds the tables and the book; each binding reads them through the lent view at the
//! call, and each handler writes the book and fires its events at their steps through an
//! [`AchievementHost`]. Lists keep 12340's orders: completed newest first (`0x5b28d0`), criteria
//! newest first (`0x5b2520`). Achievement strings carry no `$` tokens, so the reference's
//! quest-text expansion of titles, descriptions and rewards is the identity here.

use std::collections::HashMap;

use super::{LiveState, ScriptValue};

mod text;
mod tooltip;
mod verbs;

#[cfg(test)]
mod tests;

pub use text::achievement_hyperlink;
pub(crate) use tooltip::set_achievement_link;
pub(super) use verbs::install;

/// `Achievement.dbc` flags the bindings test.
pub(crate) mod flag {
    /// A statistic, listed under the Statistics root.
    pub const STATISTIC: u32 = 0x1;
    /// A hidden row: every binding answers nothing for it.
    pub const HIDDEN: u32 = 0x2;
    pub const SUM: u32 = 0x8;
    pub const MAX: u32 = 0x10;
    pub const COUNT: u32 = 0x20;
    pub const PER_DAY: u32 = 0x40;
    pub const PROGRESS_BAR: u32 = 0x80;
    /// A tie in a `MAX` statistic names no criterion.
    pub const TIE_NAMES_NONE: u32 = 0x400;
    /// Listed only once earned (Feats of Strength-like rows outside category 81).
    pub const EARNED_ONLY: u32 = 0x800;
}

/// The Feats of Strength category, listed only as earned.
pub(crate) const FEATS_OF_STRENGTH: i32 = 81;

/// The Statistics root category.
const STATISTICS_ROOT: i32 = 1;

/// Tracked achievements are capped at ten (`0x5b23e0`).
const MAX_TRACKED: usize = 10;

/// One `Achievement.dbc` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AchievementRow {
    pub id: u32,
    pub faction: i32,
    pub supercedes: u32,
    pub title: String,
    pub description: String,
    pub category: i32,
    pub points: u32,
    pub ui_order: u32,
    pub flags: u32,
    pub icon: String,
    pub reward: String,
    pub minimum_criteria: u32,
    pub shares_criteria: u32,
}

/// One `Achievement_Category.dbc` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AchievementCategoryRow {
    pub id: i32,
    pub parent: i32,
    pub name: String,
}

/// One `Achievement_Criteria.dbc` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AchievementCriteriaRow {
    pub id: u32,
    pub achievement: u32,
    pub kind: u32,
    pub asset: u32,
    pub quantity: u32,
    pub description: String,
    pub flags: u32,
    pub timer_time: u32,
}

/// The three tables in file order, with the indices 12340 builds at game start (`0x5b6df0`).
#[derive(Debug, Default)]
pub struct AchievementDb {
    rows: Vec<AchievementRow>,
    by_id: HashMap<u32, usize>,
    categories: Vec<AchievementCategoryRow>,
    category_by_id: HashMap<i32, usize>,
    criteria: Vec<AchievementCriteriaRow>,
    criteria_by_id: HashMap<u32, usize>,
    /// Each achievement's criteria, file order; a criterion of an unknown achievement or of a
    /// type past 123 joins none.
    criteria_of: HashMap<u32, Vec<usize>>,
    /// The achievements naming each id as `supercedes`, for the superseded-by-completed test.
    successors: HashMap<u32, Vec<u32>>,
}

impl AchievementDb {
    pub fn new(
        rows: Vec<AchievementRow>,
        categories: Vec<AchievementCategoryRow>,
        criteria: Vec<AchievementCriteriaRow>,
    ) -> Self {
        let by_id: HashMap<u32, usize> = rows.iter().enumerate().map(|(i, r)| (r.id, i)).collect();
        let category_by_id = categories
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id, i))
            .collect();
        let criteria_by_id = criteria
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id, i))
            .collect();
        let mut criteria_of: HashMap<u32, Vec<usize>> = HashMap::new();
        for (i, c) in criteria.iter().enumerate() {
            if by_id.contains_key(&c.achievement) && c.kind <= 123 {
                criteria_of.entry(c.achievement).or_default().push(i);
            }
        }
        let mut successors: HashMap<u32, Vec<u32>> = HashMap::new();
        for r in &rows {
            successors.entry(r.supercedes).or_default().push(r.id);
        }
        Self {
            rows,
            by_id,
            categories,
            category_by_id,
            criteria,
            criteria_by_id,
            criteria_of,
            successors,
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub(crate) fn rows(&self) -> &[AchievementRow] {
        &self.rows
    }

    pub fn get(&self, id: u32) -> Option<&AchievementRow> {
        self.by_id.get(&id).map(|&i| &self.rows[i])
    }

    pub(crate) fn categories(&self) -> &[AchievementCategoryRow] {
        &self.categories
    }

    pub(crate) fn category(&self, id: i32) -> Option<&AchievementCategoryRow> {
        self.category_by_id.get(&id).map(|&i| &self.categories[i])
    }

    pub(crate) fn criterion(&self, id: u32) -> Option<&AchievementCriteriaRow> {
        self.criteria_by_id.get(&id).map(|&i| &self.criteria[i])
    }

    /// The achievement whose criteria `id` reads: its `shares_criteria` row, else itself; `None`
    /// when either is missing.
    pub(crate) fn source(&self, id: u32) -> Option<&AchievementRow> {
        let a = self.get(id)?;
        if a.shares_criteria != 0 {
            self.get(a.shares_criteria)
        } else {
            Some(a)
        }
    }

    /// The criteria of `source`'s own list, file order.
    pub(crate) fn criteria_list(
        &self,
        source: u32,
    ) -> impl Iterator<Item = &AchievementCriteriaRow> {
        self.criteria_of
            .get(&source)
            .into_iter()
            .flatten()
            .map(|&i| &self.criteria[i])
    }

    pub(crate) fn successors(&self, id: u32) -> &[u32] {
        self.successors.get(&id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Whether `category` sits under the Statistics root, itself included (`0x5b0de0`).
    fn under_statistics(&self, category: i32) -> bool {
        let mut at = self.category(category);
        while let Some(c) = at {
            if c.id == STATISTICS_ROOT {
                return true;
            }
            at = self.category(c.parent);
        }
        false
    }

    /// The real-achievement test (`0x5b0d50`): a row in a category outside the Statistics tree.
    pub(crate) fn is_real(&self, id: u32) -> bool {
        self.get(id).is_some_and(|a| {
            self.category(a.category).is_some() && !self.under_statistics(a.category)
        })
    }

    /// The criteria-statistic test (`0x5b0de0`): a criterion of a statistic.
    fn is_criteria_stat(&self, id: u32) -> bool {
        self.criterion(id)
            .and_then(|c| self.get(c.achievement))
            .is_some_and(|a| self.under_statistics(a.category))
    }
}

/// A packed `WowTime` word: minute 6 bits, hour 5, weekday 3, month day 6 (0-based), month 4
/// (0-based), year 5 (since 2000); an all-ones field is unset (`0x76c970`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WowDate(pub u32);

impl WowDate {
    fn field(self, shift: u32, bits: u32) -> i32 {
        let mask = (1 << bits) - 1;
        let v = (self.0 >> shift) & mask;
        if v == mask {
            -1
        } else {
            v as i32
        }
    }

    pub fn minute(self) -> i32 {
        self.field(0, 6)
    }
    pub fn hour(self) -> i32 {
        self.field(6, 5)
    }
    fn weekday(self) -> i32 {
        self.field(11, 3)
    }
    /// 0-based.
    pub fn month_day(self) -> i32 {
        self.field(14, 6)
    }
    /// 0-based.
    pub fn month(self) -> i32 {
        self.field(20, 4)
    }
    /// Years since 2000.
    pub fn year(self) -> i32 {
        self.field(24, 5)
    }

    /// Pack a calendar date: 0-based month and day, years since 2000; the clock fields stay 0.
    pub fn from_date(month: u32, day: u32, year: u32) -> Self {
        Self(((year & 0x1f) << 24) | ((month & 0xf) << 20) | ((day & 0x3f) << 14))
    }

    /// The fields from year down, each skipped when either side's is unset.
    fn fields(self) -> [i32; 6] {
        [
            self.year(),
            self.month(),
            self.month_day(),
            self.weekday(),
            self.hour(),
            self.minute(),
        ]
    }

    /// `WowTime::operator>` (`0x76c7b0`): the first field both sides set that differs decides.
    fn after(self, other: Self) -> bool {
        for (a, b) in self.fields().into_iter().zip(other.fields()) {
            if a >= 0 && b >= 0 && a != b {
                return a > b;
            }
        }
        false
    }

    /// `WowTime::operator==` (`0x76c890`): every field both sides set agrees.
    fn same(self, other: Self) -> bool {
        self.fields()
            .into_iter()
            .zip(other.fields())
            .all(|(a, b)| a < 0 || b < 0 || a == b)
    }

    fn at_or_after(self, other: Self) -> bool {
        self.same(other) || self.after(other)
    }
}

/// One criterion's progress as the wire carries it (the timers are elapsed seconds).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CriteriaWire {
    pub id: u32,
    pub counter: u64,
    pub player: u64,
    pub flags: u32,
    pub date: u32,
    pub timer1: u32,
    pub timer2: u32,
}

/// One held criterion: the timers are start times, `now - elapsed` (`0x5b3160`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CriteriaState {
    pub(crate) id: u32,
    pub(crate) counter: u64,
    pub(crate) flags: u32,
    pub(crate) date: WowDate,
    pub(crate) player: u64,
    pub(crate) timer1: u32,
    pub(crate) timer2: u32,
}

/// One completed achievement and its date.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Dated {
    pub(crate) id: u32,
    pub(crate) date: WowDate,
}

/// One achievement packet, as the app hands it on; `now` is the receive time, unix seconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AchievementUpdate {
    /// `SMSG_ALL_ACHIEVEMENT_DATA` (`0x5b32f0`).
    List {
        achievements: Vec<(u32, u32)>,
        criteria: Vec<CriteriaWire>,
        now: u32,
    },
    /// `SMSG_ACHIEVEMENT_EARNED` (`0x5b3020`); `own` when `player` is the active player.
    Earned {
        player: u64,
        id: u32,
        date: u32,
        silent: bool,
        own: bool,
    },
    /// `SMSG_CRITERIA_UPDATE` (`0x5b3160`).
    Criteria { criterion: CriteriaWire, now: u32 },
    /// `SMSG_CRITERIA_DELETED` (`0x5b36f0`).
    CriteriaDeleted(u32),
    /// `SMSG_ACHIEVEMENT_DELETED` (`0x5b3610`).
    AchievementDeleted(u32),
    /// `SMSG_RESPOND_INSPECT_ACHIEVEMENTS` (`0x5b34a0`).
    Inspect {
        guid: u64,
        achievements: Vec<(u32, u32)>,
        criteria: Vec<CriteriaWire>,
        now: u32,
    },
}

/// What one packet handler raises: `TRACKED_ACHIEVEMENT_UPDATE`'s one or four numbers, or a bare
/// event; [`AchievementHost::raise`] fires it at its step.
pub type AchievementEvent = (&'static str, Vec<ScriptValue>);

/// The client's achievement lists (`0xacfd98`-`0xacfde0`): the player's, an inspected player's and
/// the tracked list. The game start and the game shutdown empty them (`0x5b6df0`, `0x5b4f00`); a UI
/// reload keeps them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AchievementBook {
    /// `CanShowAchievementUI` (`[0xc1f454]`): a list or a criteria update has arrived.
    pub(crate) received: bool,
    pub(crate) completed: Vec<Dated>,
    pub(crate) criteria: Vec<CriteriaState>,
    /// The last few statistic criteria updated, newest first (`0xacfda4`).
    pub(crate) recent: Vec<CriteriaState>,
    /// `SetAchievementComparisonUnit`'s guid (`[0xc1f458]`), 0 for none.
    pub(crate) inspect_guid: u64,
    pub(crate) inspect_completed: Vec<Dated>,
    pub(crate) inspect_criteria: Vec<CriteriaState>,
    pub(crate) inspect_recent: Vec<CriteriaState>,
    /// The tracked list, in add order (`0xacfde0`).
    pub(crate) tracked: Vec<u32>,
}

/// One packet handler's host: the book it writes between raises, and the raise, which runs the
/// listeners at the step with the book released (`0x81b530` runs them inside the handler).
pub trait AchievementHost {
    /// The book as of this step.
    fn book(&mut self) -> &mut AchievementBook;
    /// Fire `event` now; a listener reads and writes the book the handler goes on with.
    fn raise(&mut self, event: AchievementEvent);
}

/// Run one achievement packet's handler over `host`: each write, then each raise at its step.
pub fn handle_achievement_packet(
    db: &AchievementDb,
    update: AchievementUpdate,
    host: &mut dyn AchievementHost,
) {
    match update {
        // `0x5b32f0`: the show flag, each completed row (`0x5b3355`), each criterion (`0x5b340b`)
        // with a timed one's tracker update at once (`0x5b3454`), then `CRITERIA_UPDATE`
        // (`0x5b3478`) and `RECEIVED_ACHIEVEMENT_LIST` (`0x5b3484`). Nothing is cleared first.
        AchievementUpdate::List {
            achievements,
            criteria,
            now,
        } => {
            host.book().received = true;
            for (id, date) in achievements {
                host.book().update_achievement(db, id, WowDate(date));
            }
            for c in criteria {
                host.book().update_criteria(db, c, now);
                if let Some(rec) = db.criterion(c.id).filter(|r| r.timer_time != 0) {
                    host.raise(timed_update(rec, c.timer1));
                }
            }
            host.raise(("CRITERIA_UPDATE", Vec::new()));
            host.raise(("RECEIVED_ACHIEVEMENT_LIST", Vec::new()));
        }
        // `0x5b3020`: our own earn joins the list (`0x5b308d`); a known, loud earn of ours leaves
        // the tracked list (`0x5b3136`) and fires `ACHIEVEMENT_EARNED` (`0x5b3148`). The earn
        // effect between them (`0x5b3115`) is the creature animation's handler.
        AchievementUpdate::Earned {
            id,
            date,
            silent,
            own,
            ..
        } => {
            if own {
                host.book().update_achievement(db, id, WowDate(date));
            }
            if db.get(id).is_none() || silent || !own {
                return;
            }
            if host.book().remove_tracked(id) {
                host.raise(tracked_update(id));
            }
            host.raise(("ACHIEVEMENT_EARNED", vec![ScriptValue::Int(id.into())]));
        }
        // `0x5b3160`: the show flag, the criterion (`0x5b3218`), `CRITERIA_UPDATE` (`0x5b3224`),
        // then a timed criterion's tracker update (`0x5b326d`) or one per tracked achievement
        // reading it (`0x5b32d8`), each next link saved before the raise.
        AchievementUpdate::Criteria { criterion, now } => {
            let book = host.book();
            book.received = true;
            book.update_criteria(db, criterion, now);
            host.raise(("CRITERIA_UPDATE", Vec::new()));
            let Some(rec) = db.criterion(criterion.id) else {
                return;
            };
            if rec.timer_time != 0 {
                host.raise(timed_update(rec, criterion.timer1));
                return;
            }
            // `0x5b3298`: each node's next link is read before its compare and raise, and the walk
            // resumes from that saved link (`0x5b32e0`), so a node a listener appends after the
            // last one is never visited. A listener that frees the saved node leaves 12340 reading
            // freed memory; the walk stops there.
            let mut next = host.book().tracked.first().copied();
            while let Some(t) = next {
                let tracked = &host.book().tracked;
                let saved = tracked
                    .iter()
                    .position(|&x| x == t)
                    .and_then(|i| tracked.get(i + 1).copied());
                let shares = db
                    .get(t)
                    .is_some_and(|a| a.shares_criteria == rec.achievement);
                if t == rec.achievement || shares {
                    host.raise(tracked_update(t));
                }
                next = saved.filter(|s| host.book().tracked.contains(s));
            }
        }
        // `0x5b36f0`: a held criterion unlinks, then `CRITERIA_UPDATE` (`0x5b376b`).
        AchievementUpdate::CriteriaDeleted(id) => {
            let book = host.book();
            if let Some(i) = book.criteria.iter().position(|c| c.id == id) {
                book.criteria.remove(i);
                host.raise(("CRITERIA_UPDATE", Vec::new()));
            }
        }
        // `0x5b3610`: a held completed row unlinks, then `ACHIEVEMENT_EARNED` (`0x5b3692`).
        AchievementUpdate::AchievementDeleted(id) => {
            let book = host.book();
            if let Some(i) = book.completed.iter().position(|d| d.id == id) {
                book.completed.remove(i);
                host.raise(("ACHIEVEMENT_EARNED", vec![ScriptValue::Int(id.into())]));
            }
        }
        // `0x5b34a0`: the rows (`0x5b350d`) and criteria (`0x5b35cb`) of the compared guid alone,
        // then `INSPECT_ACHIEVEMENT_READY` whoever they name (`0x5b35ef`).
        AchievementUpdate::Inspect {
            guid,
            achievements,
            criteria,
            now,
        } => {
            let book = host.book();
            if guid == book.inspect_guid {
                for (id, date) in achievements {
                    update_achievement(db, &mut book.inspect_completed, id, WowDate(date), true);
                }
                for c in criteria {
                    book.update_inspect_criteria(db, c, now);
                }
            }
            host.raise(("INSPECT_ACHIEVEMENT_READY", Vec::new()));
        }
    }
}

impl AchievementBook {
    /// Whether the player's list holds `id`.
    pub(crate) fn completed(&self, id: u32) -> Option<&Dated> {
        self.completed.iter().find(|d| d.id == id)
    }

    pub(crate) fn inspected(&self, id: u32) -> Option<&Dated> {
        self.inspect_completed.iter().find(|d| d.id == id)
    }

    pub(crate) fn progress(&self, id: u32) -> Option<&CriteriaState> {
        self.criteria.iter().find(|c| c.id == id)
    }

    /// The superseded-by-completed test (`0x5b37c0`): an achievement following `id` is complete.
    pub(crate) fn superseded(&self, db: &AchievementDb, id: u32) -> bool {
        db.successors(id)
            .iter()
            .any(|&next| self.completed(next).is_some())
    }

    /// The number of completed achievements.
    pub fn completed_len(&self) -> usize {
        self.completed.len()
    }

    /// The tracked list, in add order.
    pub fn tracked(&self) -> &[u32] {
        &self.tracked
    }

    /// The compared player's guid (`[0xc1f458]`), 0 for none.
    pub fn inspect_guid(&self) -> u64 {
        self.inspect_guid
    }

    /// The player's achievement update (`0x5b28d0`).
    fn update_achievement(&mut self, db: &AchievementDb, id: u32, date: WowDate) {
        update_achievement(db, &mut self.completed, id, date, false);
    }

    /// `AddTrackedAchievement` (`0x5b23e0`): `true` when it announces `id`, held or added; past
    /// ten nothing is added.
    pub fn add_tracked(&mut self, id: u32) -> bool {
        add_tracked(&mut self.tracked, id)
    }

    /// The tracked removal (`0x5b24d0`): `true` when `id` was held and is gone.
    pub fn remove_tracked(&mut self, id: u32) -> bool {
        match self.tracked.iter().position(|&t| t == id) {
            Some(at) => {
                self.tracked.remove(at);
                true
            }
            None => false,
        }
    }

    /// The criteria update (`0x5b2520`): a new row goes before the first one not newer; flag bit 0
    /// drops the row; a statistic's update leads the recent list.
    fn update_criteria(&mut self, db: &AchievementDb, c: CriteriaWire, now: u32) {
        let state = held_state(c, now);
        match self.criteria.iter().position(|h| h.id == c.id) {
            Some(i) => self.criteria[i] = state,
            None => {
                let at = self
                    .criteria
                    .iter()
                    .position(|h| !h.date.after(state.date))
                    .unwrap_or(self.criteria.len());
                self.criteria.insert(at, state);
            }
        }
        if c.flags & 1 != 0 {
            self.criteria.retain(|h| h.id != c.id);
            return;
        }
        if !db.is_criteria_stat(c.id) {
            return;
        }
        let achievement = db.criterion(c.id).map(|r| r.achievement);
        push_recent(
            &mut self.recent,
            state,
            |held| {
                held.id == state.id || db.criterion(held.id).map(|r| r.achievement) == achievement
            },
            |held| db.criterion(held.id).is_some(),
        );
    }

    /// The inspect criteria update (`0x5b2cb0`): as the player's, with no delete bit and the recent
    /// list deduplicated by criterion alone.
    fn update_inspect_criteria(&mut self, db: &AchievementDb, c: CriteriaWire, now: u32) {
        let state = held_state(c, now);
        match self.inspect_criteria.iter().position(|h| h.id == c.id) {
            Some(i) => self.inspect_criteria[i] = state,
            None => {
                let at = self
                    .inspect_criteria
                    .iter()
                    .position(|h| !h.date.after(state.date))
                    .unwrap_or(self.inspect_criteria.len());
                self.inspect_criteria.insert(at, state);
            }
        }
        if !db.is_criteria_stat(c.id) {
            return;
        }
        push_recent(
            &mut self.inspect_recent,
            state,
            |held| held.id == state.id,
            |_| true,
        );
    }

    /// The inspect release (`0x5b2fb0`): the inspected lists empty; the guid stays.
    pub(crate) fn release_inspect(&mut self) {
        self.inspect_criteria.clear();
        self.inspect_recent.clear();
        self.inspect_completed.clear();
    }
}

fn held_state(c: CriteriaWire, now: u32) -> CriteriaState {
    CriteriaState {
        id: c.id,
        counter: c.counter,
        flags: c.flags,
        date: WowDate(c.date),
        player: c.player,
        timer1: now.wrapping_sub(c.timer1),
        timer2: now.wrapping_sub(c.timer2),
    }
}

/// The recent list's walk (`0x5b2520`): the new row leads; the walk past it drops the first row
/// `duplicate` names and stops, or after five steps drops the row it reached. A row `advances`
/// refuses is not stepped past, as the reference's walk re-reads it.
fn push_recent(
    recent: &mut Vec<CriteriaState>,
    state: CriteriaState,
    duplicate: impl Fn(&CriteriaState) -> bool,
    advances: impl Fn(&CriteriaState) -> bool,
) {
    recent.insert(0, state);
    let mut at = 1;
    let mut count = 0;
    while at < recent.len() {
        let mut gone = false;
        if advances(&recent[at]) {
            if duplicate(&recent[at]) {
                recent.remove(at);
                gone = true;
            } else {
                at += 1;
            }
        }
        count += 1;
        if gone {
            return;
        }
        if count >= 5 {
            if at < recent.len() {
                recent.remove(at);
            }
            return;
        }
    }
}

/// The achievement update (`0x5b28d0`, inspect `0x5b2ab0`): an id already held only takes the new
/// date; a new one goes after every newer row and, among same-date rows, after those whose
/// `ui_order` is not below its own. The player's list refuses an id the table lacks; the
/// inspect list keeps it.
fn update_achievement(
    db: &AchievementDb,
    list: &mut Vec<Dated>,
    id: u32,
    date: WowDate,
    inspect: bool,
) {
    let rec = db.get(id);
    if rec.is_none() && !inspect {
        return;
    }
    if let Some(held) = list.iter_mut().find(|d| d.id == id) {
        held.date = date;
        return;
    }
    let mut at = 0;
    let mut found = false;
    while at < list.len() && list[at].date.at_or_after(date) && !found {
        if list[at].date.same(date) {
            while at < list.len() && list[at].date.at_or_after(date) && !found {
                let other = db.get(list[at].id);
                match (rec, other) {
                    (Some(r), Some(o)) if r.ui_order > o.ui_order => found = true,
                    _ => at += 1,
                }
            }
        } else {
            at += 1;
        }
    }
    list.insert(at, Dated { id, date });
}

fn tracked_update(id: u32) -> (&'static str, Vec<ScriptValue>) {
    (
        "TRACKED_ACHIEVEMENT_UPDATE",
        vec![ScriptValue::Int(id.into())],
    )
}

/// A timed criterion's tracker update: achievement, criterion, elapsed seconds and the limit
/// (`0x5b326d`).
fn timed_update(rec: &AchievementCriteriaRow, elapsed: u32) -> (&'static str, Vec<ScriptValue>) {
    (
        "TRACKED_ACHIEVEMENT_UPDATE",
        vec![
            ScriptValue::Int(rec.achievement.into()),
            ScriptValue::Int(rec.id.into()),
            ScriptValue::Int(elapsed.into()),
            ScriptValue::Int(rec.timer_time.into()),
        ],
    )
}

/// `AddTrackedAchievement` (`0x5b23e0`): a held id re-announces itself; past ten nothing is
/// added.
pub(crate) fn add_tracked(tracked: &mut Vec<u32>, id: u32) -> bool {
    for (i, &t) in tracked.iter().enumerate() {
        if t == id {
            return true;
        }
        if i + 1 == MAX_TRACKED {
            return false;
        }
    }
    tracked.push(id);
    true
}

/// The VM's achievement state: what each tooltip shows and the name answers it waits on. The tables
/// and the lists are the client's, read through the lent view at the call.
#[derive(Debug, Default)]
pub(crate) struct AchievementModel {
    /// Each tooltip's shown link, the fields the achievement fill keeps (`0x627220`), cleared with
    /// its lines.
    pub(crate) tooltips: HashMap<crate::widget::FrameHandle, tooltip::AchievementLink>,
    /// The tooltips whose name read left its callback on a guid's record (`0x627220` ->
    /// `0x61de50`), one per uncached read.
    pub(crate) name_waits: Vec<(crate::widget::FrameHandle, u64)>,
}

impl AchievementModel {
    /// The active player's PvP faction index read at the call (`0x4d3790`, `0x5b3d0a`), `None`
    /// with no player object held.
    fn player_faction(l: &dyn LiveState) -> Option<i32> {
        l.player_pvp_faction(l.active_player()?)
    }

    /// The compared player's (`guid`, `0xc1f458`) read at the call (`0x5b3bf0`), when the
    /// client holds that player.
    fn inspect_faction(l: &dyn LiveState, guid: u64) -> Option<i32> {
        l.player_pvp_faction(guid)
    }
}

/// A row the faction filter drops: tied to the other side while `faction` is known.
fn wrong_side(a: &AchievementRow, faction: Option<i32>) -> bool {
    faction.is_some_and(|f| a.faction != -1 && a.faction != f)
}

/// The `trackedAchievements` CVar's encoding (the int-list CVar wrapper, `0x766720`/`0x766f70`): a
/// `v\x01` tag, then three base-93 digits from `#` per id.
pub fn encode_tracked(ids: &[u32]) -> String {
    let mut out = String::from("v\u{1}");
    for &id in ids {
        if id > 93 * 93 * 93 - 1 || out.len() >= 0xfd {
            continue;
        }
        for d in [id / (93 * 93), id % (93 * 93) / 93, id % 93] {
            out.push(char::from(35 + d as u8));
        }
    }
    out
}

/// Decode the CVar as the int-list read does (`0x766f70`), over the value [`upgrade_tracked`]
/// leaves: base-93 triples from the third byte, stopping at a short triple, a NUL or an id of 0.
pub fn decode_tracked(value: &str) -> Vec<u32> {
    let upgraded = upgrade_tracked(value);
    let bytes = upgraded.as_deref().unwrap_or(value).as_bytes();
    let mut out = Vec::new();
    // Each byte sign-extended (`movsx` at `0x766fcd`, `0x766fd6`, `0x766fe4`).
    let digit = |b: u8| i32::from(b as i8).wrapping_sub(35);
    for &[a, b, c] in bytes.get(2..).unwrap_or(&[]).as_chunks::<3>().0 {
        if a == 0 || b == 0 || c == 0 {
            break;
        }
        let id = digit(a)
            .wrapping_mul(93)
            .wrapping_add(digit(b))
            .wrapping_mul(93)
            .wrapping_add(digit(c));
        if id == 0 {
            break;
        }
        out.push(id as u32);
    }
    out
}

/// The version-0 upgrade (`0x766f00`): a value with no `v` tag, or a bare `v`, is read whole as
/// base-220 triples and written again under the `v\x01` tag (`0x766720`, `0x5300d0`); `None` for
/// a tagged value, which the read takes as it is.
pub fn upgrade_tracked(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let version = match bytes {
        [b'v', v, ..] => *v,
        _ => 0,
    };
    if version != 0 {
        return None;
    }
    const WEIGHTS: [u32; 3] = [220 * 220, 220, 1];
    let mut ids = Vec::new();
    let (mut digit, mut id) = (0, 0u32);
    for &b in bytes {
        id = id.wrapping_add(WEIGHTS[digit].wrapping_mul(u32::from(b).wrapping_sub(35)));
        digit += 1;
        if digit > 2 {
            ids.push(id);
            (digit, id) = (0, 0);
        }
    }
    Some(encode_tracked(&ids))
}

impl super::UiScript {
    /// The tracked-list load (`0x5b3840`), a step of the tracked-state load before
    /// `VARIABLES_LOADED` (`0x5128d0`): each id `trackedAchievements` holds is added to the client's
    /// list, which a UI reload keeps, and each add that holds it announces itself (`0x5b241b`,
    /// `0x5b24b6`). Nothing with no view lent.
    pub fn load_tracked_achievements(&mut self) {
        let value = self
            .model_ref()
            .cvars
            .get("trackedachievements")
            .map(|s| s.value.clone())
            .unwrap_or_default();
        // An untagged value is written back upgraded before the read (`0x766f00`).
        if let Some(upgraded) = upgrade_tracked(&value) {
            let mut model = self.model_mut();
            super::cvars::set_from_engine(&mut model, "trackedAchievements", upgraded);
        }
        let ids = decode_tracked(&value);
        for id in ids {
            let added = super::write_live(|l| l.achievement_book_mut().map(|b| b.add_tracked(id)))
                .flatten();
            if added == Some(true) {
                self.fire_event(
                    "TRACKED_ACHIEVEMENT_UPDATE",
                    vec![ScriptValue::Int(id.into())],
                );
            }
        }
    }

    /// The tracked-list save (`0x5b1fa0`) into `trackedAchievements`, a step of the UI shutdown
    /// after `PLAYER_LOGOUT` (`0x528f00`). Nothing with no view lent.
    pub fn save_tracked_achievements(&mut self) {
        let Some(value) =
            super::read_live(|l| l.achievement_book().map(|b| encode_tracked(b.tracked())))
                .flatten()
        else {
            return;
        };
        let mut model = self.model_mut();
        super::cvars::set_from_engine(&mut model, "trackedAchievements", value);
    }

    /// A player name's answer: each achievement tooltip whose name read waited on `guid` runs its
    /// callback (`0x61de50`), which on a name rebuilds the link the tooltip still shows; a miss
    /// spends the callback.
    pub fn achievement_name_landed(&mut self, guid: u64, named: bool) {
        let waiting: Vec<crate::widget::FrameHandle> = {
            let mut model = self.model_mut();
            let waits = &mut model.achievement.name_waits;
            if waits.is_empty() {
                return;
            }
            let landed = waits.iter().filter(|w| w.1 == guid).map(|w| w.0).collect();
            waits.retain(|w| w.1 != guid);
            if !named {
                return;
            }
            landed
        };
        for h in waiting {
            if let Err(e) = tooltip::rebuild(&self.lua, h) {
                self.push_error(e);
            }
        }
    }
}
