//! The player's cooldowns, the client's `SpellHistory` list (node ops `0x6e12c0`, `0x6e13e0`,
//! `0x6e1630`, `0x6e1790`; handlers `0x806dd0`, `0x6e95d0`, `0x6e9670`, `0x6e9730`).
//!
//! - A record holds three timers: the spell's recovery, its category's recovery and the GCD
//!   (`startRecoveryCategory`, `startRecoveryTime`); `on_hold` parks the first two until
//!   `SMSG_COOLDOWN_EVENT` (`SPELL_ATTR_COOLDOWN_ON_EVENT`: Stealth, Feign Death).
//! - The GCD starts at cast send (`0x6e58fb`); the spell's recovery is computed from `Spell.dbc`
//!   when our own `SMSG_SPELL_GO` arrives (`0x6e8498`, `0x6e8566`). vmangos sends no cooldown
//!   packet for a plain cast; `SMSG_SPELL_COOLDOWN` is its override path.
//! - A failed cast clears the GCD armed at send (`0x6e1d83`, `0x6e1630`) and, unless the reason
//!   is 0x3c, removes a cooldown-on-event spell's parked record (`0x6e73cc`).
//!
//! Every mutation the reference flushes bumps [`Cooldowns::generation`], the edge of the flush
//! `0x4b31b0` makes (`ACTIONBAR_UPDATE_COOLDOWN`, `SPELL_UPDATE_COOLDOWN`); a natural expiry does
//! not, since the stock `Cooldown.lua` frame hides itself at the end.

use std::time::{Duration, Instant};

use bevy::prelude::*;

use super::{SpellModifiers, OP_GCD};
use benilla_formats::SpellDisplay;
use benilla_protocol::messages::ItemUseSpell;

/// `SMSG_SPELL_COOLDOWN` flag bit 0: each entry also starts its spell's GCD (`0x806f8c`).
pub(crate) const WIRE_INCLUDE_GCD: u8 = 0x1;
/// `SMSG_SPELL_COOLDOWN` flag bit 1: no entry parks for `SMSG_COOLDOWN_EVENT` (`0x806ee8`).
pub(crate) const WIRE_IGNORE_ON_EVENT: u8 = 0x2;

/// One timer; a zero duration is untracked.
#[derive(Clone, Copy, Debug)]
struct Timer {
    start: Instant,
    duration: Duration,
}

impl Timer {
    fn none(now: Instant) -> Self {
        Self {
            start: now,
            duration: Duration::ZERO,
        }
    }

    fn remaining(&self, now: Instant) -> Duration {
        (self.start + self.duration).saturating_duration_since(now)
    }
}

/// One `SpellHistory` node (`0x6e12c0`).
#[derive(Clone, Debug)]
struct Record {
    spell_id: u32,
    /// The cast item's entry, 0 for a spell: records match on the spell and item pair.
    item_id: u32,
    recovery: Timer,
    category: u32,
    /// The category's `SpellCategory` row has flags bit 0x2, so it matches every query
    /// (`0x6e1563`); wand Shoot's 351 is the only one.
    category_wildcard: bool,
    category_recovery: Timer,
    /// The recovery timers hold their durations but have not started.
    on_hold: bool,
    gcd_category: u32,
    gcd: Timer,
    /// Every timer has run out: the record reads cold and stays until [`Cooldowns::collect`].
    spent: bool,
    /// When the record was inserted, the stamp `RecentlyCast` reads.
    time: Instant,
}

/// One action's cooldown read (`GetActionCooldown`): the winning timer's start, duration and
/// remainder, and `enabled == false` for an on-hold record, which `CooldownFrame_SetTimer`
/// hides. The start is what `GetCooldownInfo 0x6e13e0` returns, so two arms never alias.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CooldownInfo {
    /// For an on-hold record, when it was inserted.
    pub start: Instant,
    pub remaining_ms: u32,
    pub duration_ms: u32,
    pub enabled: bool,
}

impl CooldownInfo {
    /// `(start_ms on the GetTime clock, duration_ms, enabled)`, or `None` when cold. `anchor` and
    /// `ui_now` must be the frame's one clock pair ([`crate::ui_script::UiClock`]), so one arm
    /// derives the same start every frame; a fresh `Instant::now()` here would jitter it.
    /// The start is capped at this frame's `GetTime()` ms: 12340 stamps and reads one live clock
    /// (`0x6081f0`), and a later start wraps the widget's unsigned `now − start` (`0x5ec91b`).
    pub(crate) fn ui_triple(&self, anchor: Instant, ui_now: f64) -> Option<(i64, u32, bool)> {
        (self.remaining_ms > 0).then(|| {
            // A timer armed after the anchor sample reads as `GetTime()` until the next frame's.
            let start = ui_now - anchor.saturating_duration_since(self.start).as_secs_f64();
            #[allow(clippy::cast_possible_truncation)] // session-clock ms fit i64
            (
                // Floored: the widget truncates `GetTime()` (`0x86ae20`).
                ((start * 1000.0).round() as i64).min((ui_now * 1000.0).floor() as i64),
                self.duration_ms,
                self.enabled,
            )
        })
    }
}

/// One cooldown list, the client's `SpellHistory`: the player's is this resource (`0xcecaec`), the
/// pet's is [`crate::ui_pet::PetBar::cooldowns`] (`0xcecb04`), which the pet bar, the pet book and
/// the pet's `SMSG_SPELL_GO` leg read and write. Each list's edge feeds its own events: the
/// player's `feed_action_state`, the pet's `ui_pet::bar::fire_pet_cooldown_events`.
#[derive(Resource, Default)]
pub(crate) struct Cooldowns {
    records: Vec<Record>,
    /// Bumped on every mutation the reference flushes (`0x4b31b0`), the flush's edge. Only counts
    /// up, across [`Self::clear_silent`] too, so a watcher reads a reset as no mutation.
    pub(crate) generation: u64,
    /// Bumped when [`Self::prune`] marks records spent or [`Self::collect`] drops them. Kept
    /// apart from [`Self::generation`]: gated feeds must see an expiry, but the reference never
    /// fires the event for one.
    expiry_epoch: u64,
    /// When the next spell go collects both lists (`0xd3f608`), `None` before the first.
    next_collect: Option<Instant>,
}

/// What the rune leg of [`Cooldowns::with_rune_leg`] reads: the rune table and costs, the
/// spell catalog for each record's rune cost, and the cost modifiers.
pub(crate) struct RuneLeg<'a> {
    pub(crate) runes: super::usable::RuneCheck<'a>,
    pub(crate) catalog: &'a benilla_formats::SpellCatalog,
    pub(crate) mods: &'a SpellModifiers,
}

/// The spacing of the history collection a spell go runs (`0x80fe5a`, 120 000 ms).
const COLLECT_EVERY: Duration = Duration::from_secs(120);

impl Cooldowns {
    fn bump(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// How many records the list holds, elapsed ones included.
    #[cfg(test)]
    pub(crate) fn record_count(&self) -> usize {
        self.records.len()
    }

    /// How many records [`Self::prune`] has not marked spent.
    #[cfg(test)]
    pub(crate) fn live_count(&self) -> usize {
        self.records.iter().filter(|r| !r.spent).count()
    }

    /// The counter a gated feed watches: moves on any mutation or pruned expiry.
    pub(crate) fn feed_epoch(&self) -> u64 {
        self.generation.wrapping_add(self.expiry_epoch)
    }

    /// An elapsed record awaits its prune. The feeds run before `feed_action_state`'s prune, so
    /// on the frame a timer crosses zero the read changes a frame before [`Self::feed_epoch`].
    pub(crate) fn sweep_pending(&self, now: Instant) -> bool {
        self.records.iter().any(|r| {
            !r.spent
                && !r.on_hold
                && r.recovery.remaining(now).is_zero()
                && r.category_recovery.remaining(now).is_zero()
                && r.gcd.remaining(now).is_zero()
        })
    }

    /// `AddCooldown 0x6e12c0`: always appends, never matching by id, so one spell holds separate
    /// nodes for its cast-send GCD and its SPELL_GO recovery. Replacing by id would let the GO
    /// insert wipe the running GCD. Spent records stay until [`Self::collect`]; whether the insert
    /// stored anything is returned.
    fn add(
        &mut self,
        now: Instant,
        spell_id: u32,
        item_id: u32,
        recovery: Timer,
        category: u32,
        category_wildcard: bool,
        category_recovery: Timer,
        on_hold: bool,
        gcd_category: u32,
        gcd: Timer,
    ) -> bool {
        // The client's early-out when nothing is tracked (`0x6e12c3`).
        if recovery.duration.is_zero()
            && category_recovery.duration.is_zero()
            && !on_hold
            && gcd.duration.is_zero()
        {
            return false;
        }
        self.prune(now);
        self.records.push(Record {
            spell_id,
            item_id,
            recovery,
            category,
            category_wildcard,
            category_recovery,
            on_hold,
            gcd_category,
            gcd,
            spent: false,
            time: now,
        });
        self.bump();
        true
    }

    /// `SpellHistory::RecentlyCast` (`0x807d40`): a record of the spell and item inserted under
    /// 1500 ms ago.
    pub(crate) fn recently_cast(&self, spell_id: u32, item_id: u32, now: Instant) -> bool {
        self.records.iter().any(|r| {
            r.spell_id == spell_id
                && r.item_id == item_id
                && now.saturating_duration_since(r.time) < Duration::from_millis(1500)
        })
    }

    /// Mark the fully elapsed records that are not on hold spent; invisible to reads, it is the
    /// expiry edge the gated feeds watch. Bumps [`Self::expiry_epoch`], not [`Self::generation`],
    /// so it flushes no event. [`Self::add`] runs it at every insert, and the player's feed each
    /// frame, which the gated feeds need to see an expiry with no insert.
    pub(crate) fn prune(&mut self, now: Instant) {
        let mut ran_out = false;
        for r in self.records.iter_mut().filter(|r| !r.spent && !r.on_hold) {
            if r.recovery.remaining(now).is_zero()
                && r.category_recovery.remaining(now).is_zero()
                && r.gcd.remaining(now).is_zero()
            {
                r.spent = true;
                ran_out = true;
            }
        }
        if ran_out {
            self.expiry_epoch = self.expiry_epoch.wrapping_add(1);
        }
    }

    /// `SpellHistory::GarbageCollect` (`0x802b00`): drop every record not on hold whose own and
    /// category recoveries have run out. The GCD is not read, so a record holding only a running
    /// GCD goes too.
    pub(crate) fn collect(&mut self, now: Instant) {
        let before = self.records.len();
        self.records.retain(|r| {
            r.on_hold
                || !r.recovery.remaining(now).is_zero()
                || !r.category_recovery.remaining(now).is_zero()
        });
        if self.records.len() != before {
            self.expiry_epoch = self.expiry_epoch.wrapping_add(1);
        }
    }

    /// Whether a spell go collects the history now: the first one does, then one every 120 s
    /// (`0x80fe50`-`0x80fe60`). The player's list keeps the clock for both lists.
    pub(crate) fn collect_due(&mut self, now: Instant) -> bool {
        if self.next_collect.is_some_and(|next| now < next) {
            return false;
        }
        self.next_collect = Some(now + COLLECT_EVERY);
        true
    }

    /// A spell's own cooldown from `Spell.dbc` (`StartCooldown 0x6e2c60`), without a GCD.
    ///
    /// `ranged_attack_time_ms` is `UNIT_FIELD_RANGEDATTACKTIME` when
    /// [`SpellDisplay::ranged_speed_cooldown`], else 0, added to the category timer (`0x6e2b60`):
    /// the Throw and wand Shoot sweep. Category 0 (Auto Shot) never surfaces it. `mods` applies
    /// op 11 to both timers, as the self and pet legs do; `None` inserts the DBC values.
    pub(crate) fn start_spell(
        &mut self,
        spell_id: u32,
        spell: &SpellDisplay,
        ranged_attack_time_ms: u32,
        now: Instant,
        mods: Option<&SpellModifiers>,
    ) {
        let (recovery_ms, category_ms) = mods.map_or_else(
            || {
                (
                    spell.recovery_ms,
                    spell
                        .category_recovery_ms
                        .saturating_add(ranged_attack_time_ms),
                )
            },
            |mods| mods.spell_cooldowns(spell, ranged_attack_time_ms),
        );
        let stored = self.add(
            now,
            spell_id,
            0,
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(recovery_ms)),
            },
            spell.category,
            spell.category_wildcard,
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(category_ms)),
            },
            spell.cooldown_on_event(),
            0,
            Timer::none(now),
        );
        // The go's flush follows the insert whatever it stored (`0x80fa30`, `0x80fe3b`).
        if !stored {
            self.bump();
        }
    }

    /// An item use's cooldown (`StartCooldown 0x6e2c60`): the item's values, a negative one
    /// falling back to the spell's `Spell.dbc` value.
    pub(crate) fn start_item(
        &mut self,
        item_entry: u32,
        use_spell: &ItemUseSpell,
        spell: Option<&SpellDisplay>,
        now: Instant,
    ) {
        let recovery_ms = if use_spell.cooldown_ms >= 0 {
            use_spell.cooldown_ms as u32
        } else {
            spell.map_or(0, |s| s.recovery_ms)
        };
        let category_ms = if use_spell.category_cooldown_ms >= 0 {
            use_spell.category_cooldown_ms as u32
        } else {
            spell.map_or(0, |s| s.category_recovery_ms)
        };
        let stored = self.add(
            now,
            use_spell.spell_id,
            item_entry,
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(recovery_ms)),
            },
            use_spell.category,
            // Resolved through the spell only when the categories agree; no item carries 351.
            spell.is_some_and(|s| s.category == use_spell.category && s.category_wildcard),
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(category_ms)),
            },
            spell.is_some_and(|s| s.cooldown_on_event()),
            0,
            Timer::none(now),
        );
        // The go's flush follows the insert whatever it stored (`0x80fc32`).
        if !stored {
            self.bump();
        }
    }

    /// Arm the GCD at cast send (`StartGlobalCooldown 0x6e2de0`, from `0x6e58fb`) for any cast or
    /// item use, and at a pet-bar spell press (`0x4bd367`, into the pet's list), unless
    /// `startRecoveryCategory` and `startRecoveryTime` are both 0
    /// (`0x6e2e0f`-`0x6e2e21`), then with op 21 applied (`0x6e2e2c`) and nothing armed for a zero
    /// result (`0x6e2e3d`). A negative one arms nothing either, where the reference inserts a
    /// record no reader shows. `on_hold` is Attributes bit 25.
    pub(crate) fn start_gcd(
        &mut self,
        spell_id: u32,
        spell: &SpellDisplay,
        now: Instant,
        mods: &SpellModifiers,
    ) {
        if spell.start_recovery_category == 0 && spell.start_recovery_ms == 0 {
            return;
        }
        // `StartGlobalCooldown 0x6e2de0`: op 21 before the zero-duration exit (`0x6e2e2c`).
        let duration_ms = mods.apply(spell, OP_GCD, spell.start_recovery_ms as i32);
        if duration_ms <= 0 {
            return;
        }
        if benilla_assets::trace::enabled() {
            benilla_assets::trace::line(
                "cd",
                &format!(
                    "arm-gcd spell={spell_id} gcdcat={} dur={}ms (cast-send)",
                    spell.start_recovery_category, duration_ms
                ),
            );
        }
        self.add(
            now,
            spell_id,
            0,
            Timer::none(now),
            0,
            false,
            Timer::none(now),
            spell.cooldown_on_event(),
            spell.start_recovery_category,
            Timer {
                start: now,
                duration: Duration::from_millis(duration_ms as u64),
            },
        );
    }

    /// Clear the GCD of a spell's records on a failed cast, then collect the history
    /// (`SpellHistory::RemoveStartCooldown` `0x804720`, `0x804768`).
    pub(crate) fn clear_gcd(&mut self, spell_id: u32, now: Instant) {
        let mut touched = false;
        for r in &mut self.records {
            if r.spell_id == spell_id && !r.gcd.duration.is_zero() {
                // `0x6e1630` zeroes both the category and the time.
                r.gcd_category = 0;
                r.gcd = Timer::none(now);
                touched = true;
            }
        }
        if touched {
            if benilla_assets::trace::enabled() {
                benilla_assets::trace::line(
                    "cd",
                    &format!("clear-gcd spell={spell_id} (cast-fail)"),
                );
            }
            self.collect(now);
            self.bump();
        }
    }

    /// `SMSG_COOLDOWN_EVENT` (`0x6e1790`, force 0): an on-hold record's timers start now.
    pub(crate) fn cooldown_event(&mut self, spell_id: u32, now: Instant) {
        let mut touched = false;
        for r in &mut self.records {
            if r.spell_id == spell_id && r.on_hold {
                r.recovery.start = now;
                r.category_recovery.start = now;
                r.on_hold = false;
                touched = true;
            }
        }
        if touched {
            self.bump();
        }
    }

    /// `SMSG_CLEAR_COOLDOWN` and the cast-fail revert (`0x6e1790`, force 1).
    pub(crate) fn clear_spell(&mut self, spell_id: u32) {
        let before = self.records.len();
        self.records.retain(|r| r.spell_id != spell_id);
        if self.records.len() != before {
            self.bump();
        }
    }

    /// `SMSG_COOLDOWN_CHEAT` (`0x6e9700`).
    pub(crate) fn wipe(&mut self) {
        if !self.records.is_empty() {
            self.records.clear();
            self.bump();
        }
    }

    /// Empty the list with no flush: `SMSG_PET_SPELLS` clears the pet's list (`0x6e9b60` →
    /// `0x6e1880`) before it seeds it again, or for a teardown leaves it empty, and the packet
    /// calls no `0x4b31b0`. The counters keep their place, so a watcher never reads the reset as
    /// a mutation.
    pub(crate) fn clear_silent(&mut self) {
        self.records.clear();
    }

    /// Session end ([`crate::net::session::disconnected`]): `SMSG_INITIAL_SPELLS` re-sends every
    /// running cooldown at world entry and [`Self::seed_initial`] appends, so a surviving record
    /// would outlive the fresh one.
    pub(crate) fn clear_session(&mut self) {
        self.wipe();
    }

    /// One `SMSG_SPELL_COOLDOWN` entry (`0x806dd0`): a nonzero duration is the recovery with no
    /// category timer; zero means the spell's own `Spell.dbc` pair. An on-event spell parks unless
    /// the packet's [`WIRE_IGNORE_ON_EVENT`] is set; a running one carries its op-21 GCD only
    /// under [`WIRE_INCLUDE_GCD`].
    pub(crate) fn apply_wire_cooldown(
        &mut self,
        spell_id: u32,
        cooldown_ms: u32,
        flags: u8,
        spell: Option<&SpellDisplay>,
        mods: &SpellModifiers,
        now: Instant,
    ) {
        let on_hold =
            flags & WIRE_IGNORE_ON_EVENT == 0 && spell.is_some_and(|s| s.cooldown_on_event());
        let (recovery_ms, category, category_ms) = if cooldown_ms != 0 {
            (cooldown_ms, spell.map_or(0, |s| s.category), 0)
        } else {
            match spell {
                Some(s) => (s.recovery_ms, s.category, s.category_recovery_ms),
                None => (0, 0, 0),
            }
        };
        let (gcd_category, gcd_ms) = match spell {
            Some(s) if flags & WIRE_INCLUDE_GCD != 0 && !on_hold => (
                s.start_recovery_category,
                mods.apply(s, OP_GCD, s.start_recovery_ms as i32).max(0) as u32,
            ),
            _ => (0, 0),
        };
        self.add(
            now,
            spell_id,
            0,
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(recovery_ms)),
            },
            category,
            spell.is_some_and(|s| s.category == category && s.category_wildcard),
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(category_ms)),
            },
            on_hold,
            gcd_category,
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(gcd_ms)),
            },
        );
    }

    /// `SMSG_ITEM_COOLDOWN` (`0x6e95d0`): the client's hardcoded 30 s on the item's on-use spell.
    pub(crate) fn apply_wire_item_cooldown(
        &mut self,
        item_entry: u32,
        spell_id: u32,
        now: Instant,
    ) {
        self.add(
            now,
            spell_id,
            item_entry,
            Timer {
                start: now,
                duration: Duration::from_millis(30_000),
            },
            0,
            false,
            Timer::none(now),
            false,
            0,
            Timer::none(now),
        );
    }

    /// One `SMSG_INITIAL_SPELLS` cooldown: the wire carries the remainder, so the record starts
    /// now. A permanent cooldown arrives as 1 ms with the category time's top bit set, which the
    /// client masks off (`0x6df16d`).
    pub(crate) fn seed_initial(
        &mut self,
        cd: &benilla_protocol::messages::SpellCooldown,
        now: Instant,
    ) {
        self.add(
            now,
            cd.spell_id,
            u32::from(cd.item_id),
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(cd.spell_cd_ms)),
            },
            u32::from(cd.category),
            // No catalog here; the wildcard category 351 never arrives in this list.
            false,
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(cd.category_cd_ms & 0x7FFF_FFFF)),
            },
            false,
            0,
            Timer::none(now),
        );
    }

    /// One `SMSG_PET_SPELLS` cooldown: remainders, starting now. The category duration's
    /// [`PET_COOLDOWN_PERMANENT`] flag is stripped, or it would read as a 24-day sweep.
    ///
    /// The handler seeds through `StartCooldown 0x6e2c60` (`0x4bdabf`), which calls no flush, so
    /// the insert leaves [`Self::generation`] where it was.
    pub(crate) fn seed_pet(
        &mut self,
        cd: &benilla_protocol::messages::PetSpellCooldown,
        spell: Option<&SpellDisplay>,
        now: Instant,
    ) {
        use benilla_protocol::messages::PET_COOLDOWN_PERMANENT;
        let category_ms = cd.category_cd_ms & !PET_COOLDOWN_PERMANENT;
        let generation = self.generation;
        self.add(
            now,
            cd.spell_id,
            0,
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(cd.spell_cd_ms)),
            },
            u32::from(cd.category),
            // Off the catalog when present, else not a wildcard.
            spell.is_some_and(|s| s.category_wildcard),
            Timer {
                start: now,
                duration: Duration::from_millis(u64::from(category_ms)),
            },
            false,
            0,
            Timer::none(now),
        );
        self.generation = generation;
    }

    /// `SpellHistory::GetSpellCooldown`'s rune leg (`0x807c37`-`0x807cfe`) on the timers' triple
    /// at `now_ms`: a rune spell that lacks runes, with a record of itself or of a spell sharing
    /// a rune kind in the history, reads the ten seconds before its runes' ready time
    /// (`0x807cf3`) once that is the latest end. The enable flag is the timers'.
    pub(crate) fn with_rune_leg(
        &self,
        spell_id: u32,
        spell: &SpellDisplay,
        triple: Option<(i64, u32, bool)>,
        leg: &RuneLeg,
        now_ms: i64,
    ) -> Option<(i64, u32, bool)> {
        let Some(cost) = leg.runes.costs.get(spell.rune_cost_id) else {
            return triple;
        };
        let shares = |r: &Record| {
            r.spell_id == spell_id
                || leg
                    .catalog
                    .get(r.spell_id)
                    .filter(|o| o.rune_cost_id != 0)
                    .and_then(|o| leg.runes.costs.get(o.rune_cost_id))
                    .is_some_and(|o| {
                        (cost.blood != 0 && o.blood != 0)
                            || (cost.unholy != 0 && o.unholy != 0)
                            || (cost.frost != 0 && o.frost != 0)
                    })
        };
        if !self.records.iter().any(shares) {
            return triple;
        }
        let Some(ready) = leg.runes.rune_ready_ms(spell, leg.mods).map(i64::from) else {
            return triple;
        };
        let latest = triple.map_or(now_ms, |(start, duration, _)| {
            (start + i64::from(duration)).max(now_ms)
        });
        if ready <= now_ms || ready < latest {
            return triple;
        }
        Some((ready - 10_000, 10_000, triple.is_none_or(|t| t.2)))
    }

    /// The client's `IsSpellOnCooldown 0x6e1690`, which despite its name is true only for an
    /// on-hold record; it feeds the usable check's grey-while-parked leg (`0x6e3fb1`).
    ///
    /// Keyed on spell and item (`0x6e173f`): the action bar's item gate `0x6e2fc0` passes the
    /// item entry (`0x6e3037`), every other caller 0. `category` is the item's for an item
    /// (`0x6e16f6` takes it from the item's spell slot), else the spell's.
    pub(crate) fn has_on_hold_record(&self, spell_id: u32, item_entry: u32, category: u32) -> bool {
        self.records.iter().any(|r| {
            r.on_hold
                && ((r.spell_id == spell_id && r.item_id == item_entry)
                    || (category != 0
                        && r.category == category
                        && !r.category_recovery.duration.is_zero()))
        })
    }

    /// The cast validator's first check (`0x6094f0` at `0x609565`, via `0x6e2ea0`): refused while
    /// [`Self::info`] reads any remainder, so a press sharing the running GCD's category is
    /// refused whatever its own `startRecoveryTime`. The caller picks the reason (`0x60952b`):
    /// 0x28 for an item, 0x3c for a spell. Refusing locally matters: the server's NOT_READY
    /// failure would clear the running GCD.
    pub(crate) fn not_ready(
        &self,
        spell_id: u32,
        item_entry: u32,
        spell: Option<&SpellDisplay>,
        now: Instant,
    ) -> bool {
        self.info(spell_id, item_entry, spell, now).remaining_ms > 0
    }

    /// `GetCooldownInfo 0x6e13e0`: the longest remainder over every record's three legs.
    ///
    /// - `Effect[0]` of ATTACK or TRADE_SKILL always reads cold (`0x6e1439`).
    /// - Spell leg: id and item match; a parked record reads its full duration, disabled.
    /// - Category leg: equal category, or any query for a wildcard row; parked likewise.
    /// - GCD leg (`0x6e15cc`): the node's GCD category equals the query's
    ///   `startRecoveryCategory` and the node's time is nonzero; it ignores on-hold.
    pub(crate) fn info(
        &self,
        spell_id: u32,
        item_entry: u32,
        spell: Option<&SpellDisplay>,
        now: Instant,
    ) -> CooldownInfo {
        let mut best = CooldownInfo {
            start: now,
            remaining_ms: 0,
            duration_ms: 0,
            enabled: true,
        };
        if spell.is_some_and(|s| s.cooldown_query_excluded()) {
            return best;
        }
        let category = spell.map_or(0, |s| s.category);
        let start_recovery_category = spell.map_or(0, |s| s.start_recovery_category);
        let mut consider = |timer: &Timer, remaining: Duration, enabled: bool| {
            let remaining_ms = remaining.as_millis().min(u128::from(u32::MAX)) as u32;
            if remaining_ms > best.remaining_ms {
                best = CooldownInfo {
                    start: timer.start,
                    remaining_ms,
                    duration_ms: timer.duration.as_millis().min(u128::from(u32::MAX)) as u32,
                    enabled,
                };
            }
        };
        for r in &self.records {
            if r.spell_id == spell_id && r.item_id == item_entry {
                if r.on_hold {
                    // Parked: full duration, disabled (enable 0 hides the sweep).
                    consider(&r.recovery, r.recovery.duration, false);
                } else {
                    consider(&r.recovery, r.recovery.remaining(now), true);
                }
            }
            if (r.category != 0 && r.category == category) || r.category_wildcard {
                if r.on_hold {
                    consider(&r.category_recovery, r.category_recovery.duration, false);
                } else {
                    consider(
                        &r.category_recovery,
                        r.category_recovery.remaining(now),
                        true,
                    );
                }
            }
            if r.gcd_category == start_recovery_category && !r.gcd.duration.is_zero() {
                consider(&r.gcd, r.gcd.remaining(now), true);
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spell(
        category: u32,
        recovery_ms: u32,
        category_recovery_ms: u32,
        gcd: (u32, u32),
        attributes: u32,
    ) -> SpellDisplay {
        SpellDisplay {
            category,
            recovery_ms,
            category_recovery_ms,
            start_recovery_category: gcd.0,
            start_recovery_ms: gcd.1,
            attributes,
            ..Default::default()
        }
    }

    /// Fireball-shaped: no own cooldown, the ordinary 133/1500 GCD.
    fn fireball() -> SpellDisplay {
        spell(0, 0, 0, (133, 1500), 0x10000)
    }

    #[test]
    fn gcd_uses_op_21_before_arming() {
        let mut d = fireball();
        d.spell_family = 3;
        d.spell_family_flags = 1 << 5;
        let mut mods = SpellModifiers::default();
        mods.set_class_family(3);
        mods.set(false, 5, OP_GCD, -50);
        let now = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_gcd(133, &d, now, &mods);
        assert_eq!(cds.info(133, 0, Some(&d), now).duration_ms, 750);

        d.start_recovery_ms = 0;
        mods.set(true, 5, OP_GCD, 1000);
        let mut cds = Cooldowns::default();
        cds.start_gcd(133, &d, now, &mods);
        assert_eq!(cds.info(133, 0, Some(&d), now).duration_ms, 500);
    }

    #[test]
    fn spell_cooldown_applies_op_11_to_both_timers_after_the_ranged_pad() {
        let mut d = spell(44, 8_000, 5_000, (0, 0), 0);
        d.spell_family = 3;
        d.spell_family_flags = 1 << 5;
        let mut mods = SpellModifiers::default();
        mods.set_class_family(3);
        mods.set(true, 5, super::super::OP_COOLDOWN, -1_500);
        assert_eq!(mods.spell_cooldowns(&d, 0), (6_500, 3_500));
        assert_eq!(mods.spell_cooldowns(&d, 2_000), (6_500, 5_500));

        let now = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_spell(2136, &d, 2_000, now, Some(&mods));
        assert_eq!(cds.info(2136, 0, Some(&d), now).duration_ms, 6_500);
        let sibling = spell(44, 0, 0, (0, 0), 0);
        assert_eq!(cds.info(999, 0, Some(&sibling), now).duration_ms, 5_500);
        let mut cds = Cooldowns::default();
        cds.start_spell(2136, &d, 2_000, now, None);
        assert_eq!(cds.info(2136, 0, Some(&d), now).duration_ms, 8_000);
        assert_eq!(cds.info(999, 0, Some(&sibling), now).duration_ms, 7_000);

        // Endurance 2/2: Sprint's five-minute recovery loses 30%.
        d.recovery_ms = 300_000;
        d.category_recovery_ms = 0;
        mods.set(true, 5, super::super::OP_COOLDOWN, 0);
        mods.set(false, 5, super::super::OP_COOLDOWN, -30);
        assert_eq!(mods.spell_cooldowns(&d, 0), (210_000, 0));
    }

    /// Charge-shaped: category 44, 15 s category cooldown, NO GCD pair.
    fn charge() -> SpellDisplay {
        spell(44, 0, 15_000, (0, 0), 0)
    }

    #[test]
    fn the_gcd_spreads_to_every_spell_sharing_the_start_recovery_category() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_gcd(133, &fireball(), t0, &super::SpellModifiers::default());

        // Any spell with the same startRecoveryCategory reads the GCD; Charge has none.
        let mid = t0 + Duration::from_millis(500);
        let fb = cds.info(133, 0, Some(&fireball()), mid);
        assert_eq!(
            (fb.remaining_ms, fb.duration_ms, fb.enabled),
            (1000, 1500, true)
        );
        let frostbolt = fireball(); // same shape, different id
        let other = cds.info(116, 0, Some(&frostbolt), mid);
        assert_eq!((other.remaining_ms, other.duration_ms), (1000, 1500));
        let ch = cds.info(100, 0, Some(&charge()), mid);
        assert_eq!(ch.remaining_ms, 0, "no startRecoveryCategory — no GCD read");

        // The press gate reads the same getter; `0x6e1690` stays false with nothing parked.
        assert!(cds.not_ready(133, 0, Some(&fireball()), mid));
        assert!(!cds.has_on_hold_record(133, 0, fireball().category));
    }

    /// The SPELL_GO insert (`StartCooldown`, no GCD) is its own node beside the cast-send GCD.
    #[test]
    fn a_go_self_insert_never_wipes_the_running_gcd() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        // Frost-Nova-shaped: own 25 s recovery, category 35, the ordinary 133/1500 GCD pair.
        let frost_nova = spell(35, 25_000, 0, (133, 1500), 0);
        cds.start_gcd(122, &frost_nova, t0, &super::SpellModifiers::default()); // the cast-send arm
        cds.start_spell(122, &frost_nova, 0, t0 + Duration::from_millis(100), None); // the GO insert

        let mid = t0 + Duration::from_millis(200);
        // Every GCD sibling still reads the running GCD.
        let fb = cds.info(133, 0, Some(&fireball()), mid);
        assert_eq!(
            (fb.remaining_ms, fb.duration_ms),
            (1300, 1500),
            "the GO self-insert must not eat the GCD node"
        );
        assert!(
            cds.not_ready(133, 0, Some(&fireball()), mid),
            "the local lock holds too"
        );
        // Frost Nova's own button reads the longer, its own cooldown.
        let own = cds.info(122, 0, Some(&frost_nova), mid);
        assert_eq!((own.remaining_ms, own.duration_ms), (24_900, 25_000));
    }

    #[test]
    fn a_failed_cast_clears_the_gcd_and_the_optimistic_recovery() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let fd = spell(0, 30_000, 0, (133, 1500), 0); // Feign-Death-shaped minus on-event
        cds.start_gcd(5384, &fd, t0, &super::SpellModifiers::default());
        cds.start_spell(5384, &fd, 0, t0, None);
        let mid = t0 + Duration::from_millis(100);
        assert!(cds.not_ready(5384, 0, Some(&fd), mid));

        // The fail path (0x6e1a00): GCD cleared (0x6e1630), the record removed (0x6e3050).
        cds.clear_gcd(5384, mid);
        cds.clear_spell(5384);
        assert!(!cds.not_ready(5384, 0, Some(&fd), mid));
        assert_eq!(cds.info(5384, 0, Some(&fd), mid).remaining_ms, 0);
    }

    #[test]
    fn category_cooldowns_reach_category_siblings_but_not_others() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_spell(100, &charge(), 0, t0, None); // Charge: category 44, 15 s

        let mid = t0 + Duration::from_secs(5);
        // Another spell in category 44 reads the shared remainder.
        let sibling = spell(44, 0, 15_000, (0, 0), 0);
        let s = cds.info(999, 0, Some(&sibling), mid);
        assert_eq!((s.remaining_ms, s.duration_ms), (10_000, 15_000));
        assert!(
            cds.not_ready(999, 0, Some(&sibling), mid),
            "category lock is a not-ready"
        );
        // An unrelated spell reads nothing.
        assert_eq!(cds.info(133, 0, Some(&fireball()), mid).remaining_ms, 0);
    }

    #[test]
    fn an_on_event_cooldown_parks_until_the_event_starts_it() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        // Feign Death: 30 s recovery, SPELL_ATTR_COOLDOWN_ON_EVENT (bit 25).
        let fd = spell(0, 30_000, 0, (0, 0), 0x0200_0000);
        cds.start_spell(5384, &fd, 0, t0, None);

        // Parked: full duration, disabled, and still not ready.
        let parked = cds.info(5384, 0, Some(&fd), t0 + Duration::from_secs(60));
        assert_eq!(
            (parked.remaining_ms, parked.enabled),
            (30_000, false),
            "an on-hold record never elapses on its own"
        );
        assert!(cds.not_ready(5384, 0, Some(&fd), t0 + Duration::from_secs(60)));
        assert!(
            cds.has_on_hold_record(5384, 0, fd.category),
            "the corrected 0x6e1690: an on-hold record — the usable walk's grey-while-parked"
        );

        // SMSG_COOLDOWN_EVENT starts the clocks now.
        let event_at = t0 + Duration::from_secs(60);
        cds.cooldown_event(5384, event_at);
        let running = cds.info(5384, 0, Some(&fd), event_at + Duration::from_secs(10));
        assert_eq!(
            (running.remaining_ms, running.duration_ms, running.enabled),
            (20_000, 30_000, true)
        );
    }

    #[test]
    fn wire_cooldowns_take_the_server_duration_or_fall_back_to_the_dbc() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        // A school lockout: nonzero wire ms is the recovery verbatim.
        let mods = SpellModifiers::default();
        cds.apply_wire_cooldown(133, 8_000, 0, Some(&fireball()), &mods, t0);
        let locked = cds.info(133, 0, Some(&fireball()), t0 + Duration::from_secs(3));
        assert_eq!((locked.remaining_ms, locked.duration_ms), (5_000, 8_000));

        // Zero wire ms: the spell's own Spell.dbc recovery/category pair.
        let mut cds = Cooldowns::default();
        cds.apply_wire_cooldown(100, 0, 0, Some(&charge()), &mods, t0);
        let ch = cds.info(100, 0, Some(&charge()), t0 + Duration::from_secs(5));
        assert_eq!((ch.remaining_ms, ch.duration_ms), (10_000, 15_000));
    }

    #[test]
    fn the_wire_flags_gate_the_gcd_and_the_on_event_hold() {
        let t0 = Instant::now();
        let mid = t0 + Duration::from_millis(500);
        let mods = SpellModifiers::default();
        let gcd = |cds: &Cooldowns| cds.info(133, 0, Some(&fireball()), mid).duration_ms;

        // No flags: the recovery alone, no GCD.
        let mut cds = Cooldowns::default();
        cds.apply_wire_cooldown(116, 8_000, 0, Some(&fireball()), &mods, t0);
        assert_eq!(gcd(&cds), 0);

        // 0x1: the spell's GCD runs beside it, through op 21.
        let mut d = fireball();
        d.spell_family = 3;
        d.spell_family_flags = 1 << 5;
        let mut halved = SpellModifiers::default();
        halved.set_class_family(3);
        halved.set(false, 5, OP_GCD, -50);
        let mut cds = Cooldowns::default();
        cds.apply_wire_cooldown(116, 8_000, WIRE_INCLUDE_GCD, Some(&d), &halved, t0);
        assert_eq!(gcd(&cds), 750);

        // An on-event spell parks with no GCD; 0x2 runs it now, and 0x1 then carries the GCD.
        let fd = spell(0, 30_000, 0, (133, 1500), 1 << 25);
        let flags = WIRE_INCLUDE_GCD;
        let mut cds = Cooldowns::default();
        cds.apply_wire_cooldown(5384, 0, flags, Some(&fd), &mods, t0);
        let parked = cds.info(5384, 0, Some(&fd), mid);
        assert_eq!((parked.enabled, gcd(&cds)), (false, 0));
        let mut cds = Cooldowns::default();
        cds.apply_wire_cooldown(5384, 0, flags | WIRE_IGNORE_ON_EVENT, Some(&fd), &mods, t0);
        let running = cds.info(5384, 0, Some(&fd), mid);
        assert_eq!(
            (running.enabled, running.remaining_ms, gcd(&cds)),
            (true, 29_500, 1500)
        );
    }

    #[test]
    fn a_session_end_empties_the_list_so_the_next_logins_wire_is_the_whole_truth() {
        use benilla_protocol::messages::SpellCooldown;
        let wire = |ms| SpellCooldown {
            spell_id: 12975, // Last Stand: 10 minutes, no category
            item_id: 0,
            category: 0,
            spell_cd_ms: ms,
            category_cd_ms: 0,
        };
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();

        // Login one: 600 s still to run.
        cds.seed_initial(&wire(600_000), t0);
        assert_eq!(cds.info(12975, 0, None, t0).duration_ms, 600_000);

        // Relog six seconds later: the server now says 594 s.
        let t1 = t0 + Duration::from_secs(6);
        cds.clear_session();
        assert!(cds.records.is_empty(), "the list dies with the session");
        cds.seed_initial(&wire(594_000), t1);

        let read = cds.info(12975, 0, None, t1);
        assert_eq!(
            (cds.records.len(), read.duration_ms, read.remaining_ms),
            (1, 594_000, 594_000),
            "one record, the second login's — a surviving first-login record would answer 600 s \
             and go on answering it for the whole cooldown"
        );
    }

    /// `0x6e1690` matches the spell and item pair; only the item gate `0x6e2fc0` passes an entry.
    #[test]
    fn an_on_hold_record_is_findable_only_under_the_key_that_armed_it() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        // An on-event on-use spell, category 0 so the category leg cannot answer.
        let use_spell = ItemUseSpell {
            spell_id: 5384,
            cooldown_ms: 30_000,
            category: 0,
            category_cooldown_ms: 0,
        };
        let on_event = spell(0, 30_000, 0, (0, 0), 0x0200_0000);
        cds.start_item(1487, &use_spell, Some(&on_event), t0);

        assert!(
            cds.has_on_hold_record(5384, 1487, 0),
            "the ITEM gate's own query — `0x6e2fc0` passes the entry as itemId"
        );
        assert!(
            !cds.has_on_hold_record(5384, 0, 0),
            "the spell-slot form must not reach an item's record: itemId is part of the match"
        );
        assert!(
            !cds.has_on_hold_record(5384, 999, 0),
            "and not another item's entry either"
        );
    }

    #[test]
    fn item_use_cooldowns_key_on_the_item_and_respect_the_wire_triple() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        // A potion: category 4 for 60 s; the use cooldown -1 falls back to the spell's (none).
        let use_spell = ItemUseSpell {
            spell_id: 439,
            cooldown_ms: -1,
            category: 4,
            category_cooldown_ms: 60_000,
        };
        let potion_spell = spell(4, 0, 60_000, (133, 1500), 0);
        cds.start_item(118, &use_spell, Some(&potion_spell), t0);

        let mid = t0 + Duration::from_secs(15);
        // Spell 439 as cast from item 118 reads the category remainder.
        let info = cds.info(439, 118, Some(&potion_spell), mid);
        assert_eq!((info.remaining_ms, info.duration_ms), (45_000, 60_000));
        // So does any other category-4 potion.
        let other = cds.info(440, 929, Some(&potion_spell), mid);
        assert_eq!(other.remaining_ms, 45_000);
    }

    #[test]
    fn prune_marks_only_fully_elapsed_records_spent() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_gcd(133, &fireball(), t0, &super::SpellModifiers::default());
        cds.start_spell(100, &charge(), 0, t0, None);
        assert_eq!(cds.live_count(), 2);

        // Past the 1.5 s GCD, inside Charge's 15 s.
        cds.prune(t0 + Duration::from_secs(5));
        assert_eq!(cds.live_count(), 1);
        assert!(!cds.records[1].spent, "Charge runs");

        cds.prune(t0 + Duration::from_secs(20));
        assert_eq!(
            (cds.live_count(), cds.record_count()),
            (0, 2),
            "kept until a collection"
        );
    }

    /// `GarbageCollect` (`0x802b00`) keeps a running recovery and a parked record, and drops the
    /// rest, a record holding only a running GCD among them.
    #[test]
    fn a_collection_drops_all_but_running_recoveries_and_parked_records() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let mods = super::SpellModifiers::default();
        let parked = || spell(9, 1_000, 0, (0, 0), 1 << 25);
        cds.start_spell(100, &charge(), 0, t0, None);
        cds.start_spell(300, &parked(), 0, t0, None);
        cds.start_spell(400, &spell(0, 2_000, 0, (0, 0), 0), 0, t0, None);
        let later = t0 + Duration::from_secs(5);
        cds.start_gcd(133, &fireball(), later, &mods);
        let epoch = cds.feed_epoch();
        cds.collect(later);
        let kept: Vec<u32> = cds.records.iter().map(|r| r.spell_id).collect();
        assert_eq!(
            kept,
            [100, 300],
            "the spent recovery and the GCD-only record go"
        );
        assert_ne!(cds.feed_epoch(), epoch, "the gated feeds see the drop");
    }

    /// The first spell go collects, then one every 120 s (`0x80fe50`).
    #[test]
    fn a_collection_falls_due_every_two_minutes() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        assert!(cds.collect_due(t0));
        assert!(!cds.collect_due(t0 + Duration::from_secs(119)));
        assert!(cds.collect_due(t0 + Duration::from_secs(120)));
        assert!(!cds.collect_due(t0 + Duration::from_secs(121)));
    }

    /// A go of a spell with no recovery stores nothing and still flushes (`0x80fa30`).
    #[test]
    fn a_go_with_nothing_to_store_still_flushes() {
        let mut cds = Cooldowns::default();
        let generation = cds.generation;
        cds.start_spell(133, &fireball(), 0, Instant::now(), None);
        assert_eq!((cds.record_count(), cds.generation), (0, generation + 1));
    }

    /// The rune leg (`0x807c37`): a rune spell short of runes, with a history record sharing a
    /// rune kind, reads the ten seconds before its runes' ready time; with runes ready, or no such
    /// record, the timers' answer stands.
    #[test]
    fn a_rune_spell_short_of_runes_reads_its_runes_ready_time() {
        use benilla_formats::{SpellCatalog, SpellRuneCost, SpellRuneCostCatalog};
        use benilla_ui::script::{RuneState, RUNE_KINDS};

        let costs = SpellRuneCostCatalog::from_rows([
            SpellRuneCost {
                id: 1,
                frost: 1,
                ..Default::default()
            },
            SpellRuneCost {
                id: 2,
                unholy: 1,
                ..Default::default()
            },
        ]);
        let rune_spell = |cost| SpellDisplay {
            power_type: super::super::usable::POWER_RUNE,
            rune_cost_id: cost,
            start_recovery_category: 133,
            start_recovery_ms: 1_500,
            ..Default::default()
        };
        const ICY_TOUCH: u32 = 45477;
        const HOWLING_BLAST: u32 = 49184;
        const PLAGUE_STRIKE: u32 = 45462;
        let catalog = SpellCatalog::from_displays(std::collections::HashMap::from([
            (ICY_TOUCH, rune_spell(1)),
            (HOWLING_BLAST, rune_spell(1)),
            (PLAGUE_STRIKE, rune_spell(2)),
        ]));
        let mods = SpellModifiers::default();
        // Both frost runes spent, at 50.0 s and 52.0 s, ten seconds a rune.
        let mut state = RuneState {
            regen: [0.1; RUNE_KINDS],
            ..Default::default()
        };
        state.set_predicted(0x3f, 0x2f, None, 50_000);
        state.set_predicted(0x2f, 0x0f, None, 52_000);
        let leg = RuneLeg {
            runes: super::super::usable::RuneCheck {
                state: &state,
                costs: &costs,
            },
            catalog: &catalog,
            mods: &mods,
        };
        let read = |cds: &Cooldowns, spell: u32| {
            cds.with_rune_leg(spell, catalog.get(spell).unwrap(), None, &leg, 53_000)
        };

        let mut cds = Cooldowns::default();
        assert_eq!(read(&cds, ICY_TOUCH), None, "no history record");
        cds.start_gcd(
            PLAGUE_STRIKE,
            catalog.get(PLAGUE_STRIKE).unwrap(),
            Instant::now(),
            &mods,
        );
        assert_eq!(
            read(&cds, ICY_TOUCH),
            None,
            "an unholy spell shares no frost"
        );
        cds.start_gcd(
            HOWLING_BLAST,
            catalog.get(HOWLING_BLAST).unwrap(),
            Instant::now(),
            &mods,
        );
        // The first frost rune is due at 59.999 s.
        assert_eq!(read(&cds, ICY_TOUCH), Some((49_999, 10_000, true)));
        assert_eq!(
            read(&cds, PLAGUE_STRIKE),
            None,
            "its unholy runes are ready"
        );
        // A later timer end wins over the runes.
        let long = Some((52_000, 20_000, true));
        assert_eq!(
            cds.with_rune_leg(
                ICY_TOUCH,
                catalog.get(ICY_TOUCH).unwrap(),
                long,
                &leg,
                53_000
            ),
            long
        );
    }

    /// Spent records stay until a collection: a GCD every 2 s for a minute leaves every record,
    /// the live ones being Charge while it runs and the newest GCD.
    #[test]
    fn spent_records_stay_until_a_collection() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let mods = super::SpellModifiers::default();
        // Charge's 15 s category timer, then a 1.5 s GCD every 2 s for a minute.
        cds.start_spell(100, &charge(), 0, t0, None);
        for i in 0..30u64 {
            let now = t0 + Duration::from_secs(2 * i);
            cds.start_gcd(133, &fireball(), now, &mods);
            let charge_live = usize::from(2 * i < 15);
            assert_eq!(
                cds.live_count(),
                1 + charge_live,
                "insert {i}: Charge if it still runs, and the new GCD"
            );
        }
        assert_eq!(cds.record_count(), 31);
        cds.collect(t0 + Duration::from_secs(58));
        assert_eq!(cds.record_count(), 0, "nothing but a running GCD was left");
    }

    /// An insert marks the records that have run out and only those: a running timer and a parked
    /// record stay live, no read changes, and the insert is the one mutation.
    #[test]
    fn an_insert_keeps_a_running_and_a_parked_record_and_changes_no_read() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let mods = super::SpellModifiers::default();
        let brief = || spell(7, 2_000, 0, (0, 0), 0);
        // Parked: `Attributes` bit 25, its timers wait for `SMSG_COOLDOWN_EVENT`.
        let parked = || spell(9, 1_000, 0, (0, 0), 1 << 25);
        cds.start_gcd(133, &fireball(), t0, &mods);
        cds.start_spell(100, &brief(), 0, t0, None);
        cds.start_spell(200, &charge(), 0, t0, None);
        cds.start_spell(300, &parked(), 0, t0, None);
        assert_eq!(cds.record_count(), 4);

        let now = t0 + Duration::from_secs(5);
        let asks = [
            (133, fireball()),
            (100, brief()),
            (200, charge()),
            (201, spell(44, 0, 0, (0, 0), 0)),
            (300, parked()),
            (301, spell(9, 0, 0, (0, 0), 0)),
            (999, spell(0, 0, 0, (0, 0), 0)),
        ];
        let reads = |cds: &Cooldowns| {
            asks.iter()
                .map(|(id, d)| cds.info(*id, 0, Some(d), now))
                .collect::<Vec<_>>()
        };
        let before = reads(&cds);
        assert_eq!(before[2].remaining_ms, 10_000, "Charge runs");
        assert!(!before[4].enabled, "and the parked spell reads disabled");
        let generation = cds.generation;

        // A spell of no category the others share.
        cds.start_spell(500, &spell(99, 5_000, 0, (0, 0), 0), 0, now, None);

        assert_eq!(
            cds.live_count(),
            3,
            "Charge, the parked record and the new one"
        );
        assert!(!cds.sweep_pending(now), "nothing that ran out is left");
        assert_eq!(
            reads(&cds),
            before,
            "the records that stay read as they did"
        );
        assert_eq!(
            cds.generation,
            generation + 1,
            "one mutation, the insert's own"
        );
    }

    /// An elapsed record answers no read, so a prune cannot change one: what it keeps, a running
    /// timer and a parked record whose own timers have run down, answers as before.
    #[test]
    fn a_prune_changes_no_read() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let mods = super::SpellModifiers::default();
        let brief = || spell(7, 2_000, 0, (0, 0), 0);
        // Parked: `Attributes` bit 25, its timers wait for `SMSG_COOLDOWN_EVENT`.
        let parked = || spell(9, 1_000, 0, (0, 0), 1 << 25);
        cds.start_gcd(133, &fireball(), t0, &mods);
        cds.start_spell(100, &brief(), 0, t0, None);
        cds.start_spell(200, &charge(), 0, t0, None);
        cds.start_spell(300, &parked(), 0, t0, None);
        assert_eq!(cds.records.len(), 4);

        let now = t0 + Duration::from_secs(5);
        let asks = [
            (133, fireball()),
            (100, brief()),
            (200, charge()),
            // Another spell of Charge's category, and one of the parked spell's.
            (201, spell(44, 0, 0, (0, 0), 0)),
            (300, parked()),
            (301, spell(9, 0, 0, (0, 0), 0)),
            (999, spell(0, 0, 0, (0, 0), 0)),
        ];
        let reads = |cds: &Cooldowns| {
            asks.iter()
                .map(|(id, d)| cds.info(*id, 0, Some(d), now))
                .collect::<Vec<_>>()
        };
        let before = reads(&cds);
        assert!(cds.sweep_pending(now), "the GCD and Brief have run out");
        assert_eq!(before[2].remaining_ms, 10_000, "Charge runs");
        assert!(!before[4].enabled, "and the parked spell reads disabled");

        cds.prune(now);

        assert_eq!(cds.live_count(), 2, "the GCD and Brief are spent");
        assert!(!cds.sweep_pending(now));
        assert_eq!(reads(&cds), before);
    }

    /// A prune that removes nothing must not move the epoch, or the per-frame prune would hold
    /// every gated feed open.
    #[test]
    fn the_feed_epoch_moves_on_expiry_but_an_empty_prune_is_silent() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let e0 = cds.feed_epoch();
        cds.start_gcd(133, &fireball(), t0, &super::SpellModifiers::default());
        let armed = cds.feed_epoch();
        assert_ne!(e0, armed, "a mutation moves the epoch (generation)");

        cds.prune(t0 + Duration::from_millis(100));
        assert_eq!(
            cds.feed_epoch(),
            armed,
            "nothing elapsed — the prune is silent"
        );

        cds.prune(t0 + Duration::from_secs(5));
        let expired = cds.feed_epoch();
        assert_ne!(
            armed, expired,
            "the expiry IS a feed edge (the triple flips to None)"
        );
        assert_eq!(
            cds.generation,
            {
                let mut probe = Cooldowns::default();
                probe.start_gcd(133, &fireball(), t0, &super::SpellModifiers::default());
                probe.generation
            },
            "…but the event-edge generation never saw it (the reference is silent on expiry)"
        );

        cds.prune(t0 + Duration::from_secs(6));
        assert_eq!(cds.feed_epoch(), expired, "an empty prune stays silent");
    }

    /// A fail-clear and re-arm between two feeds must still read as a new start.
    #[test]
    fn the_ui_triple_is_stable_per_arm_and_distinct_across_arms() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_gcd(772, &fireball(), t0, &super::SpellModifiers::default());

        // Two reads of one arm, frames apart, with both clocks in lockstep.
        let read1 = cds
            .info(772, 0, Some(&fireball()), t0 + Duration::from_millis(16))
            .ui_triple(t0 + Duration::from_millis(16), 10.016);
        let read2 = cds
            .info(772, 0, Some(&fireball()), t0 + Duration::from_millis(160))
            .ui_triple(t0 + Duration::from_millis(160), 10.160);
        assert_eq!(read1, Some((10_000, 1500, true)));
        assert_eq!(read1, read2, "one arm reads one start, every frame");

        // The fail clears the GCD and a re-press re-arms 200 ms later, unseen by the feed.
        cds.clear_gcd(772, t0 + Duration::from_millis(200));
        cds.start_gcd(
            772,
            &fireball(),
            t0 + Duration::from_millis(200),
            &super::SpellModifiers::default(),
        );
        let rearmed = cds
            .info(772, 0, Some(&fireball()), t0 + Duration::from_millis(216))
            .ui_triple(t0 + Duration::from_millis(216), 10.216);
        assert_eq!(
            rearmed,
            Some((10_200, 1500, true)),
            "a re-arm never aliases"
        );
    }

    /// A start past `GetTime()` is a finished sweep (`0x5ec91b`), so a mid-frame arm reads as the
    /// frame's `GetTime()` in whole ms, then settles on its own start.
    #[test]
    fn a_mid_frame_arm_never_reads_ahead_of_get_time() {
        let anchor0 = Instant::now();
        let mut cds = Cooldowns::default();
        // Armed 4 ms after this frame's anchor sample.
        cds.start_gcd(
            133,
            &fireball(),
            anchor0 + Duration::from_millis(4),
            &super::SpellModifiers::default(),
        );

        // Frame 1 converts through the pre-arm pair, frames 2 and 3 through the next ones.
        let read = |at: u64, ui_now: f64| {
            cds.info(
                133,
                0,
                Some(&fireball()),
                anchor0 + Duration::from_millis(at + 4),
            )
            .ui_triple(anchor0 + Duration::from_millis(at), ui_now)
        };
        // 10.0006 s: a rounded start would land on 10_001, past the widget's truncated 10_000.
        assert_eq!(read(0, 10.0006), Some((10_000, 1500, true)));
        let settled = read(16, 10.0166);
        assert_eq!(settled, Some((10_005, 1500, true)));
        assert_eq!(read(32, 10.0326), settled, "settled, every frame after");
    }

    /// The GCD leg (`0x6e15cc`) ignores the pressed spell's own time: a `{133, 0}` scroll press
    /// is refused, a `{0, 0}` press is not.
    #[test]
    fn a_running_gcd_locks_presses_by_category_equality_alone() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        assert!(
            !cds.not_ready(133, 0, Some(&fireball()), t0),
            "no GCD running — nothing locks"
        );

        cds.start_gcd(772, &fireball(), t0, &super::SpellModifiers::default());
        let mid = t0 + Duration::from_millis(200);
        assert!(
            cds.not_ready(133, 0, Some(&fireball()), mid),
            "the spam press 200 ms later is locked — refused, never sent, the GCD lives"
        );
        // Scroll of Armor's shape.
        let scroll = spell(0, 0, 0, (133, 0), 0x10000);
        assert!(
            cds.not_ready(8091, 0, Some(&scroll), mid),
            "a zero-GCD press in the shared category is locked on the reference"
        );
        assert!(!cds.not_ready(100, 0, Some(&charge()), mid));
        assert!(!cds.not_ready(133, 0, Some(&fireball()), t0 + Duration::from_millis(1_501)));
    }

    /// The getter's exclusion (`0x6e1439`, `0x6e1442`).
    #[test]
    fn attack_and_tradeskill_reads_are_always_cold() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        cds.start_gcd(772, &fireball(), t0, &super::SpellModifiers::default());
        let mid = t0 + Duration::from_millis(200);
        let attack = SpellDisplay {
            effects: [78, 0, 0], // SPELL_EFFECT_ATTACK
            start_recovery_category: 133,
            ..Default::default()
        };
        assert_eq!(cds.info(6603, 0, Some(&attack), mid).remaining_ms, 0);
        assert!(!cds.not_ready(6603, 0, Some(&attack), mid));
    }

    /// The wildcard leg (`0x6e1563`): wand Shoot's category 351 sweeps every button.
    #[test]
    fn a_wildcard_category_record_reaches_every_query() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        // Shoot: no DBC recovery; the ranged pad is the category duration.
        let shoot = SpellDisplay {
            category: 351,
            category_wildcard: true,
            attributes: 0x50012,
            ..Default::default()
        };
        cds.start_spell(5019, &shoot, 1500, t0, None);
        let mid = t0 + Duration::from_millis(500);
        // Unrelated Fireball reads the wand swing and is not ready for its duration.
        let fb = cds.info(133, 0, Some(&fireball()), mid);
        assert_eq!((fb.remaining_ms, fb.duration_ms), (1000, 1500));
        assert!(cds.not_ready(133, 0, Some(&fireball()), mid));
        assert!(!cds.not_ready(133, 0, Some(&fireball()), t0 + Duration::from_millis(1_501)));
    }

    #[test]
    fn cheat_wipe_and_clear_bump_the_generation_only_on_change() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let g0 = cds.generation;
        cds.clear_spell(133);
        cds.wipe();
        assert_eq!(cds.generation, g0);

        cds.start_gcd(133, &fireball(), t0, &super::SpellModifiers::default());
        assert_ne!(cds.generation, g0);
        let g1 = cds.generation;
        cds.wipe();
        assert_ne!(cds.generation, g1);
        assert_eq!(cds.info(133, 0, Some(&fireball()), t0).remaining_ms, 0);
    }

    /// The ranged pad (`0x6e2b60`): Throw (category 76, no DBC recovery) sweeps the weapon speed.
    #[test]
    fn throw_sweeps_the_ranged_attack_time_via_its_category() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let throw = spell(76, 0, 0, (0, 0), 0x410012);
        cds.start_spell(2764, &throw, 2200, t0, None);
        let mid = t0 + Duration::from_millis(1000);
        let info = cds.info(2764, 0, Some(&throw), mid);
        assert_eq!(info.duration_ms, 2200, "the sweep is the weapon speed");
        assert_eq!(info.remaining_ms, 1200);
        assert!(info.enabled, "running, not parked");
        assert!(cds.not_ready(2764, 0, Some(&throw), mid));
        assert!(
            !cds.not_ready(2764, 0, Some(&throw), t0 + Duration::from_millis(2300)),
            "free again after the weapon speed elapses"
        );
    }

    /// Auto Shot's category 0 has no `SpellCategory` row, so its padded timer never surfaces.
    #[test]
    fn auto_shot_category_zero_never_surfaces_its_pad() {
        let t0 = Instant::now();
        let mut cds = Cooldowns::default();
        let auto_shot = spell(0, 0, 0, (0, 0), 0x50012);
        cds.start_spell(75, &auto_shot, 3200, t0, None);
        let mid = t0 + Duration::from_millis(100);
        assert_eq!(
            cds.info(75, 0, Some(&auto_shot), mid).remaining_ms,
            0,
            "no sweep for a category-0 record"
        );
        assert!(
            !cds.not_ready(75, 0, Some(&auto_shot), mid),
            "no local refusal either"
        );
    }
}
