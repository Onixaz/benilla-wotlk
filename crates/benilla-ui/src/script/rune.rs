//! The rune table, 12340's rune state (`0xc24304`-`0xc24388`), and its verbs `GetRuneType`
//! (`0x6131e0`), `GetRuneCooldown` (`0x613020`) and `GetRuneCount` (`0x613140`). Times are
//! milliseconds on the `GetTime` clock, as the reference keeps millisecond OS clock stamps; a
//! last-used time of 0 is a ready rune.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::number_arg;

/// The slots the rune verbs index.
pub const MAX_RUNES: usize = 8;

/// The rune kinds, each with its own regen rate: blood, unholy, frost, death.
pub const RUNE_KINDS: usize = 4;

/// What a rune change tells the UI, in the order the reference fires it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuneEvent {
    /// `RUNE_POWER_UPDATE(rune, usable)`, the rune 1-based.
    Power { rune: u8, usable: bool },
    /// `RUNE_TYPE_UPDATE(rune)`, the rune 1-based.
    Type { rune: u8 },
    /// `ACTIONBAR_UPDATE_COOLDOWN`, once after `SMSG_ADD_RUNE_POWER` (`0x5ee598`).
    ActionBarCooldown,
    /// The spellbook's usable update (`0x53cf10`): the usable state reads the runes again.
    Usable,
}

/// The player's runes.
#[derive(Clone, Debug, PartialEq)]
pub struct RuneState {
    /// Each slot's kind (0 blood, 1 unholy, 2 frost, 3 death).
    pub kinds: [u8; MAX_RUNES],
    /// The kinds the last resync set, the base runes.
    pub base_kinds: [u8; MAX_RUNES],
    /// How many slots are live; 0 makes every slot valid.
    pub count: u32,
    /// The ready-rune mask (`0xc24388`), all set until a cast reports otherwise.
    pub ready: u32,
    /// When each rune's recharge began, 0 for a ready rune (`0xc24364`).
    pub last_used_ms: [u32; MAX_RUNES],
    /// When each rune comes ready, 0 for none (`0xc24344`).
    pub recharge_ms: [u32; MAX_RUNES],
    /// `PLAYER_RUNE_REGEN_1..4`: runes of each kind regained per second.
    pub regen: [f32; RUNE_KINDS],
}

/// The table every game starts with, any class (`0x5edef0`): six runes, two each of blood,
/// unholy and frost, all ready.
impl Default for RuneState {
    fn default() -> Self {
        let kinds = [0, 0, 1, 1, 2, 2, 0, 0];
        Self {
            kinds,
            base_kinds: kinds,
            count: 6,
            ready: u32::MAX,
            last_used_ms: [0; MAX_RUNES],
            recharge_ms: [0; MAX_RUNES],
            regen: [0.0; RUNE_KINDS],
        }
    }
}

/// A wire progress byte as the share of the recharge passed, capped at 1 (`0xa45564`, 1/255).
fn progress_of(byte: u8) -> f32 {
    (f32::from(byte) * (1.0 / 255.0)).min(1.0)
}

/// `now` less `share` of the reference's fixed ten-second window (`0x9ea4c8`), truncated; the
/// product is wide, as the x87 forms it.
fn started_ago(now_ms: u32, share: f32) -> u32 {
    now_ms.wrapping_sub((f64::from(share) * 10_000.0) as i64 as u32)
}

impl RuneState {
    /// A slot below the live count, or any of the eight while none is set.
    pub fn valid(&self, index: usize) -> bool {
        index < MAX_RUNES && (self.count == 0 || index < self.count as usize)
    }

    /// The regen rate of slot `index`'s kind.
    fn rate(&self, index: usize) -> f32 {
        self.regen
            .get(usize::from(self.kinds[index]))
            .copied()
            .unwrap_or(0.0)
    }

    /// The rune recharge time (`0x5ee140`): due `(1 - progress) / rate` seconds from now, or
    /// never below 0.01 runes a second. The x87 keeps the quotient wide and truncates, so a fresh
    /// rune at 0.1 a second is due in 9999 ms.
    fn compute_recharge(&mut self, index: usize, progress: f32, now_ms: u32) {
        let rate = self.rate(index);
        self.recharge_ms[index] = if rate >= 0.01 {
            let wait = (1.0 / f64::from(rate)) * (1.0 - f64::from(progress)) * -1000.0;
            now_ms.wrapping_sub(wait as i64 as u32)
        } else {
            0
        };
    }

    /// `GetRuneCooldown`'s duration: one rune's recharge in seconds, 0 at or below 0.01 a second.
    pub fn duration(&self, index: usize) -> f32 {
        let rate = self.rate(index);
        if rate > 0.01 {
            1.0 / rate
        } else {
            0.0
        }
    }

    /// `SMSG_CONVERT_RUNE` (`0x5ee020`): the slot takes `kind`; the reference does not check the
    /// wire's slot, so one past the table is dropped here.
    pub fn convert(&mut self, rune: u8, kind: u8) -> Vec<RuneEvent> {
        let Some(slot) = self.kinds.get_mut(usize::from(rune)) else {
            return Vec::new();
        };
        *slot = kind;
        vec![RuneEvent::Type { rune: rune + 1 }, RuneEvent::Usable]
    }

    /// `SMSG_RESYNC_RUNES` (`0x5ee440`): the count, then each listed rune's kind in both tables and
    /// its recharge restarted from the progress byte. A full rune keeps a last-used time ten
    /// seconds back, and the ready mask is left alone.
    pub fn resync(&mut self, count: u32, runes: &[(u8, u8)], now_ms: u32) -> Vec<RuneEvent> {
        self.count = count;
        let mut events = Vec::new();
        for (i, &(kind, progress)) in runes.iter().enumerate().take(MAX_RUNES) {
            self.kinds[i] = kind;
            self.base_kinds[i] = kind;
            let progress = progress_of(progress);
            self.last_used_ms[i] = started_ago(now_ms, progress);
            self.compute_recharge(i, progress, now_ms);
            events.push(RuneEvent::Power {
                rune: i as u8 + 1,
                usable: false,
            });
        }
        events
    }

    /// `SMSG_ADD_RUNE_POWER` (`0x5ee520`): each masked live rune not already ready gets a finished
    /// recharge; the ready mask is left to the regen tick.
    pub fn add_power(&mut self, mask: u32, now_ms: u32) -> Vec<RuneEvent> {
        let mut events = Vec::new();
        for i in 0..(self.count as usize).min(MAX_RUNES) {
            let bit = 1 << i;
            if mask & bit != 0 && self.ready & bit == 0 {
                self.last_used_ms[i] = now_ms.wrapping_sub(10_000);
                self.compute_recharge(i, 1.0, now_ms);
                events.push(RuneEvent::Power {
                    rune: i as u8 + 1,
                    usable: false,
                });
            }
        }
        events.push(RuneEvent::ActionBarCooldown);
        events
    }

    /// The predicted-runes set (`0x5ee240`): `after` becomes the ready mask. A rune ready in
    /// `before` and spent in `after` starts its recharge from its progress byte, unless already
    /// started; one coming ready, or ready with a start left over, clears.
    pub fn set_predicted(
        &mut self,
        before: u32,
        after: u32,
        progress: Option<&[u8]>,
        now_ms: u32,
    ) -> Vec<RuneEvent> {
        self.ready = after;
        let mut events = Vec::new();
        for i in 0..MAX_RUNES {
            let bit = 1 << i;
            let rune = i as u8 + 1;
            if before & bit != 0 && after & bit == 0 {
                if self.last_used_ms[i] == 0 {
                    let share = progress
                        .and_then(|p| p.get(i))
                        .map_or(0.0, |&b| progress_of(b));
                    self.last_used_ms[i] = started_ago(now_ms, share);
                    self.compute_recharge(i, share, now_ms);
                    events.push(RuneEvent::Power {
                        rune,
                        usable: false,
                    });
                }
            } else if after & bit != 0 && (before & bit == 0 || self.last_used_ms[i] != 0) {
                self.last_used_ms[i] = 0;
                self.recharge_ms[i] = 0;
                events.push(RuneEvent::Power { rune, usable: true });
            }
        }
        events.push(RuneEvent::Usable);
        events
    }

    /// The death knight's regen tick (`0x728aa6`-`0x728af8`): every rune not ready whose
    /// recharge time has come joins the ready mask, through [`Self::set_predicted`].
    pub fn tick(&mut self, now_ms: u32) -> Vec<RuneEvent> {
        let mut ready = self.ready;
        for i in 0..MAX_RUNES {
            let bit = 1 << i;
            if self.ready & bit == 0 && now_ms.wrapping_sub(self.recharge_ms[i]) as i32 >= 0 {
                ready |= bit;
            }
        }
        if ready == self.ready {
            return Vec::new();
        }
        self.set_predicted(self.ready, ready, None, now_ms)
    }

    /// The predicted-runes count by type (`0x5ee3d0`): live ready runes of `kind`.
    pub fn ready_of_kind(&self, kind: u8) -> u32 {
        (0..(self.count as usize).min(MAX_RUNES))
            .filter(|&i| self.kinds[i] == kind && self.ready & (1 << i) != 0)
            .count() as u32
    }
}

impl super::UiScript {
    /// Replace the rune table; the app fires the rune events itself.
    pub fn set_runes(&mut self, runes: RuneState) {
        set_field!(self, runes, runes);
    }
}

/// The player object exists, the gate each rune verb opens with.
fn has_player(model: &super::model::Model) -> bool {
    model.unit("player").is_some_and(|u| u.exists)
}

/// `slot - 1` as the reference's unsigned compare reads it: below 1 wraps past every slot.
fn slot_index(slot: i32) -> usize {
    (slot as u32).wrapping_sub(1) as usize
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    // GetRuneType(slot): nothing without the player object, then the 1-based kind of a valid slot.
    g.set(
        "GetRuneType",
        lua.create_function(|lua, slot: Value| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            if !has_player(&model) {
                return Ok(MultiValue::new());
            }
            let slot = number_arg(lua, slot, "Usage: Script_GetRuneType(slot)")?;
            let index = slot_index(slot);
            if !model.runes.valid(index) {
                return Ok(MultiValue::new());
            }
            Ok(MultiValue::from_vec(vec![Value::Integer(
                i64::from(model.runes.kinds[index]) + 1,
            )]))
        })?,
    )?;
    // GetRuneCooldown(slot): start in seconds, duration and ready. A slot past 1..8 raises, no
    // player answers nothing, and the live count is not checked.
    g.set(
        "GetRuneCooldown",
        lua.create_function(|lua, slot: Value| {
            let index = slot_index(number_arg(lua, slot, "Usage: GetRuneCooldown(slot)")?);
            if index >= MAX_RUNES {
                return Err(mlua::Error::RuntimeError("Invalid slot".into()));
            }
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            if !has_player(&model) {
                return Ok(MultiValue::new());
            }
            let last = model.runes.last_used_ms[index];
            Ok(MultiValue::from_vec(vec![
                Value::Number(f64::from(last) * 0.001),
                Value::Number(f64::from(model.runes.duration(index))),
                Value::Boolean(last == 0),
            ]))
        })?,
    )?;
    // GetRuneCount(slot): nothing without the player, then 1 for a ready slot and 0 for a spent
    // one; a slot past 1..8 answers nothing.
    g.set(
        "GetRuneCount",
        lua.create_function(|lua, slot: Value| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            if !has_player(&model) {
                return Ok(MultiValue::new());
            }
            let index = slot_index(number_arg(lua, slot, "Usage: GetRuneCount(slot)")?);
            if index >= MAX_RUNES {
                return Ok(MultiValue::new());
            }
            Ok(MultiValue::from_vec(vec![Value::Integer(i64::from(
                model.runes.ready & (1 << index) != 0,
            ))]))
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{UiScript, UnitState};

    fn seated() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_unit(
            "player",
            Some(UnitState {
                exists: true,
                ..Default::default()
            }),
        );
        s
    }

    /// A death knight's table: ten seconds a rune for every kind.
    fn dk() -> RuneState {
        RuneState {
            regen: [0.1; RUNE_KINDS],
            ..Default::default()
        }
    }

    const SPENT_1: RuneEvent = RuneEvent::Power {
        rune: 1,
        usable: false,
    };
    const READY_1: RuneEvent = RuneEvent::Power {
        rune: 1,
        usable: true,
    };

    /// Every character starts on the reference's six runes, blood, unholy and frost in pairs.
    #[test]
    fn a_new_game_reads_two_runes_of_each_kind() {
        let s = seated();
        let kinds: Vec<i64> = (1..=6)
            .map(|i| s.eval(&format!("return GetRuneType({i})")).unwrap())
            .collect();
        assert_eq!(kinds, [1, 1, 2, 2, 3, 3]);
    }

    /// A slot past the live six or below 1 answers nothing, as does a call with no player.
    #[test]
    fn an_invalid_slot_or_no_player_answers_nothing() {
        let s = seated();
        for slot in [0, 7, 9, -1] {
            assert_eq!(
                s.arity(&format!("GetRuneType({slot})")).unwrap(),
                0,
                "{slot}"
            );
        }
        assert!(s.run("GetRuneType('x')").is_err());
        let bare = UiScript::new().unwrap();
        assert_eq!(bare.arity("GetRuneType('x')").unwrap(), 0);
    }

    /// A spent rune starts its recharge now and comes ready after its kind's period, when the
    /// regen tick clears it (`0x728aa6`). The wire's six-rune masks also clear slots 7 and 8,
    /// which hold no recharge, so the first tick readies them again, as the reference's does.
    #[test]
    fn a_spent_rune_recharges_over_its_kind_s_period() {
        let mut r = dk();
        let spent = r.set_predicted(0x3f, 0x3e, Some(&[0; 6]), 50_000);
        assert_eq!(spent, [SPENT_1, RuneEvent::Usable]);
        assert_eq!(r.ready, 0x3e);
        assert_eq!(r.last_used_ms[0], 50_000);
        assert_eq!(r.recharge_ms[0], 59_999);
        assert_eq!(r.ready_of_kind(0), 1);
        let spare = |rune| RuneEvent::Power { rune, usable: true };
        assert_eq!(r.tick(50_016), [spare(7), spare(8), RuneEvent::Usable]);
        assert!(r.tick(59_998).is_empty());
        assert_eq!(r.tick(59_999), [READY_1, RuneEvent::Usable]);
        assert_eq!((r.ready & 1, r.last_used_ms[0]), (1, 0));
    }

    /// A rune spent part-way through its window starts back by the share already passed.
    #[test]
    fn a_progress_byte_moves_the_start_back() {
        let mut r = dk();
        r.set_predicted(0x3f, 0x3b, Some(&[0, 0, 51, 0, 0, 0]), 50_000);
        assert_eq!(r.last_used_ms[2], 48_000);
        assert_eq!(r.recharge_ms[2], 57_999);
    }

    /// A rune already recharging is not restarted by a cast that reports it spent again.
    #[test]
    fn a_recharging_rune_keeps_its_start() {
        let mut r = dk();
        r.set_predicted(0x3f, 0x3e, None, 50_000);
        assert!(!r.set_predicted(0x3f, 0x3e, None, 52_000).contains(&SPENT_1));
        assert_eq!(r.last_used_ms[0], 50_000);
    }

    /// `SMSG_CONVERT_RUNE` changes the kind and fires `RUNE_TYPE_UPDATE`; a resync sets both
    /// tables and restarts every rune, a full one ten seconds back.
    #[test]
    fn convert_and_resync_change_kinds() {
        let mut r = dk();
        assert_eq!(
            r.convert(0, 3),
            [RuneEvent::Type { rune: 1 }, RuneEvent::Usable]
        );
        assert_eq!((r.kinds[0], r.base_kinds[0]), (3, 0));
        assert!(r.convert(8, 3).is_empty());
        let wire = [(0, 255), (0, 0), (1, 255), (1, 255), (2, 255), (2, 255)];
        assert_eq!(r.resync(6, &wire, 90_000).len(), 6);
        assert_eq!((r.kinds[0], r.base_kinds[0]), (0, 0));
        assert_eq!((r.last_used_ms[0], r.recharge_ms[0]), (80_000, 90_000));
        assert_eq!((r.last_used_ms[1], r.recharge_ms[1]), (90_000, 99_999));
    }

    /// `SMSG_ADD_RUNE_POWER` finishes only spent runes and ends on `ACTIONBAR_UPDATE_COOLDOWN`.
    #[test]
    fn add_rune_power_finishes_spent_runes() {
        let mut r = dk();
        r.set_predicted(0xff, 0xfc, None, 10_000);
        assert_eq!(
            r.add_power(0x05, 12_000),
            [SPENT_1, RuneEvent::ActionBarCooldown]
        );
        assert_eq!(r.recharge_ms[0], 12_000);
        assert_eq!(r.tick(12_000), [READY_1, RuneEvent::Usable]);
    }

    /// Below 0.01 runes a second there is no recharge time and no duration.
    #[test]
    fn a_slow_rate_never_recharges() {
        let mut r = RuneState::default();
        r.set_predicted(0x3f, 0x3e, None, 50_000);
        assert_eq!((r.recharge_ms[0], r.duration(0)), (0, 0.0));
    }

    /// `GetRuneCooldown` answers the start in seconds, the period and the ready flag;
    /// `GetRuneCount` the ready bit.
    #[test]
    fn the_cooldown_and_count_verbs_read_the_table() {
        let mut s = seated();
        let mut r = dk();
        r.set_predicted(0x3f, 0x3d, None, 50_000);
        s.set_runes(r);
        let (start, duration, ready): (f64, f64, bool) =
            s.eval("return GetRuneCooldown(2)").unwrap();
        assert_eq!((start, duration, ready), (50.0, 10.0, false));
        let (start, ready): (f64, bool) = s
            .eval("local a, _, c = GetRuneCooldown(1) return a, c")
            .unwrap();
        assert_eq!((start, ready), (0.0, true));
        assert_eq!(s.eval::<i64>("return GetRuneCount(1)").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return GetRuneCount(2)").unwrap(), 0);
        assert_eq!(s.arity("GetRuneCount(9)").unwrap(), 0);
        assert!(s.run("GetRuneCooldown(9)").is_err());
        assert!(s.run("GetRuneCooldown()").is_err());
        assert!(s.run("GetRuneCount()").is_err());
        let bare = UiScript::new().unwrap();
        assert_eq!(bare.arity("GetRuneCooldown(1)").unwrap(), 0);
        assert_eq!(bare.arity("GetRuneCount()").unwrap(), 0);
    }
}
