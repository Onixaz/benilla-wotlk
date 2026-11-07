//! The item stat summary: one item's stats folded into 73 cells and a DPS, which the compare
//! tooltip subtracts (`0x631590`) and `GetItemStats`/`GetItemStatDelta` (`0x608690`, `0x608760`)
//! read. Not built: a scaling item (`0x61f550`), which reads as its template.

use super::item_link::ItemLinkFields;
use super::model::Model;
use super::spell_info::SpellStatEffects;
use super::ItemTemplateView;

const STAT_COUNT: usize = 73;

/// The stat cells' first `ITEM_MOD` token; a template's stat `t` lands in `t + ITEM_MOD_BASE`.
const ITEM_MOD_BASE: usize = 11;

/// The rating stats an `ITEM_MOD` type names, by `CR_` index (`0xad2dc8`).
const RATING_STATS: [i32; 25] = [
    11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, -1, -1, -1, 37,
    44,
];

/// The `ITEM_MOD_*` tokens by stat cell less [`ITEM_MOD_BASE`].
const ITEM_MOD_TOKENS: [&str; 49] = [
    "ITEM_MOD_MANA",
    "ITEM_MOD_HEALTH",
    "",
    "ITEM_MOD_AGILITY",
    "ITEM_MOD_STRENGTH",
    "ITEM_MOD_INTELLECT",
    "ITEM_MOD_SPIRIT",
    "ITEM_MOD_STAMINA",
    "",
    "",
    "",
    "",
    "ITEM_MOD_DEFENSE_SKILL_RATING",
    "ITEM_MOD_DODGE_RATING",
    "ITEM_MOD_PARRY_RATING",
    "ITEM_MOD_BLOCK_RATING",
    "ITEM_MOD_HIT_MELEE_RATING",
    "ITEM_MOD_HIT_RANGED_RATING",
    "ITEM_MOD_HIT_SPELL_RATING",
    "ITEM_MOD_CRIT_MELEE_RATING",
    "ITEM_MOD_CRIT_RANGED_RATING",
    "ITEM_MOD_CRIT_SPELL_RATING",
    "ITEM_MOD_HIT_TAKEN_MELEE_RATING",
    "ITEM_MOD_HIT_TAKEN_RANGED_RATING",
    "ITEM_MOD_HIT_TAKEN_SPELL_RATING",
    "ITEM_MOD_CRIT_TAKEN_MELEE_RATING",
    "ITEM_MOD_CRIT_TAKEN_RANGED_RATING",
    "ITEM_MOD_CRIT_TAKEN_SPELL_RATING",
    "ITEM_MOD_HASTE_MELEE_RATING",
    "ITEM_MOD_HASTE_RANGED_RATING",
    "ITEM_MOD_HASTE_SPELL_RATING",
    "ITEM_MOD_HIT_RATING",
    "ITEM_MOD_CRIT_RATING",
    "ITEM_MOD_HIT_TAKEN_RATING",
    "ITEM_MOD_CRIT_TAKEN_RATING",
    "ITEM_MOD_RESILIENCE_RATING",
    "ITEM_MOD_HASTE_RATING",
    "ITEM_MOD_EXPERTISE_RATING",
    "ITEM_MOD_ATTACK_POWER",
    "ITEM_MOD_RANGED_ATTACK_POWER",
    "",
    "ITEM_MOD_SPELL_HEALING_DONE",
    "ITEM_MOD_SPELL_DAMAGE_DONE",
    "ITEM_MOD_MANA_REGENERATION",
    "ITEM_MOD_ARMOR_PENETRATION_RATING",
    "ITEM_MOD_SPELL_POWER",
    "ITEM_MOD_HEALTH_REGEN",
    "ITEM_MOD_SPELL_PENETRATION",
    "ITEM_MOD_BLOCK_VALUE",
];

/// The sockets' color words by stat cell less 69.
const SOCKET_COLORS: [&str; 4] = ["META", "RED", "YELLOW", "BLUE"];
/// The socket art's color words, `0xad6704`.
const SOCKET_ART: [&str; 4] = ["Meta", "Red", "Yellow", "Blue"];

/// One item's stat cells and DPS.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct StatSummary {
    stats: [i32; STAT_COUNT],
    dps: f32,
}

impl Default for StatSummary {
    fn default() -> Self {
        Self {
            stats: [0; STAT_COUNT],
            dps: 0.0,
        }
    }
}

/// The cells' localized-string key (`0x61bb20`).
fn stat_key(stat: usize) -> String {
    match stat {
        0..=6 => format!("RESISTANCE{stat}_NAME"),
        7 => "ITEM_MOD_ATTACK_POWER_SHORT".into(),
        8 => "ITEM_MOD_MELEE_ATTACK_POWER_SHORT".into(),
        9 => "ITEM_MOD_RANGED_ATTACK_POWER_SHORT".into(),
        10 => "ITEM_MOD_FERAL_ATTACK_POWER_SHORT".into(),
        11..=59 => format!("{}_SHORT", ITEM_MOD_TOKENS[stat - ITEM_MOD_BASE]),
        60 => "ITEM_MOD_BLOCK_VALUE_SHORT".into(),
        61..=67 => format!("ITEM_MOD_POWER_REGEN{}_SHORT", stat - 61),
        68 => "ITEM_MOD_HEALTH_REGEN_SHORT".into(),
        _ => format!("EMPTY_SOCKET_{}", SOCKET_COLORS[stat - 69]),
    }
}

impl StatSummary {
    /// The summary of a template with a link block's roll (`0x6265c0`).
    pub(super) fn of(model: &Model, v: &ItemTemplateView, f: &ItemLinkFields) -> Self {
        let mut s = Self::default();
        s.accumulate(model, v);
        if f.random_property != 0 {
            s.add_roll(model, f);
        }
        s.clean_names();
        s
    }

    /// `a - b`, its group cells rejoined (`0x61dbc0`).
    pub(super) fn delta(a: &Self, b: &Self) -> Self {
        let mut d = Self::default();
        for i in 0..STAT_COUNT {
            d.stats[i] = a.stats[i] - b.stats[i];
        }
        d.dps = a.dps - b.dps;
        d.clean_names();
        d
    }

    /// `(key, value)` for each stat the table form names: a non-zero cell, cells 49 and 50 never
    /// (`0x6085d0`).
    pub(super) fn entries(&self) -> impl Iterator<Item = (String, i32)> + '_ {
        (0..STAT_COUNT)
            .filter(|&i| i != 49 && i != 50 && self.stats[i] != 0)
            .map(|i| (stat_key(i), self.stats[i]))
    }

    /// The DPS when it reads non-zero (`0x6085d0`).
    pub(super) fn dps(&self) -> Option<f32> {
        (self.dps > f32::EPSILON || self.dps < -f32::EPSILON).then_some(self.dps)
    }

    /// The compare delta's line texts in order (`0x62d930`): the DPS, the stats, then the sockets
    /// with their art; `get` reads the player's `GlobalStrings.lua`.
    pub(super) fn delta_lines(&self, get: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
        const UP: &str = "|cff00ff00";
        const DOWN: &str = "|cffff2020";
        let mut out = Vec::new();
        if self.dps.abs() > f32::EPSILON {
            let (color, sign) = if self.dps <= 0.0 {
                (DOWN, '-')
            } else {
                (UP, '+')
            };
            let name = get("ITEM_MOD_DAMAGE_PER_SECOND_SHORT").unwrap_or_default();
            out.push(format!("{color}{sign}{:.1}|r {name}", self.dps.abs()));
        }
        let mut stat_line = |value: i32, name: String| {
            if value != 0 {
                let (color, sign) = if value > 0 { (UP, '+') } else { (DOWN, '-') };
                out.push(format!("{color}{sign}{}|r {name}", value.unsigned_abs()));
            }
        };
        for i in (0..69).filter(|&i| i != 49 && i != 50) {
            if self.stats[i] != 0 {
                stat_line(self.stats[i], get(&stat_key(i)).unwrap_or_default());
            }
        }
        for i in 69..STAT_COUNT {
            if self.stats[i] != 0 {
                let label = get(&stat_key(i)).unwrap_or_default();
                let art = SOCKET_ART[i - 69];
                stat_line(
                    self.stats[i],
                    format!(
                        "|TInterface\\ItemSocketingFrame\\UI-EmptySocket-{art}.blp:12|t  {label}"
                    ),
                );
            }
        }
        out
    }

    fn accumulate(&mut self, model: &Model, v: &ItemTemplateView) {
        for &(kind, value) in &v.stats {
            self.add_stat(kind as usize, value);
        }
        for s in &v.item_spells {
            self.add_spell(model, s.spell);
        }
        self.stats[0] += v.armor as i32;
        for (i, r) in v.resistances.iter().enumerate() {
            self.stats[i + 1] += r;
        }
        for &color in &v.socket_colors {
            for bit in 0..4 {
                if color & (1 << bit) != 0 {
                    self.stats[69 + bit] += 1;
                }
            }
        }
        self.stats[60] += v.block as i32;
        // The two-block average over the delay, without a guard: no damage on no delay reads NaN,
        // which no comparison passes, so a non-weapon's DPS shows nowhere.
        let delay = v.delay_ms as f32;
        let average = |i: usize| {
            v.damages
                .get(i)
                .map_or(0.0, |&(min, max, _)| (max + min) * 0.5)
        };
        let (main, off) = (average(0), average(1));
        self.dps += main / (delay * 0.001);
        self.dps += off / (0.001 * delay);
        if model.player_req.class_id == 11 {
            self.stats[10] += self.stats[8] + feral_attack_power(v, main + off);
        }
    }

    /// One `ITEM_MOD` stat: the attack powers also feed their melee and ranged cells (`0x61b990`).
    fn add_stat(&mut self, kind: usize, value: i32) {
        let Some(cell) = self.stats.get_mut(kind + ITEM_MOD_BASE) else {
            return;
        };
        *cell += value;
        match kind {
            38 => {
                self.stats[8] += value;
                self.stats[9] += value;
            }
            39 => self.stats[9] += value,
            _ => {}
        }
    }

    fn add_spell(&mut self, model: &Model, spell: u32) {
        if let Some(e) = model
            .spell_info
            .as_ref()
            .and_then(|s| s.spell_stat_effects(spell))
        {
            self.add_spell_effects(&e);
        }
    }

    /// A passive spell's apply-aura effects (`0x61b5e0`).
    fn add_spell_effects(&mut self, e: &SpellStatEffects) {
        if e.attributes & 0x40 == 0 {
            return;
        }
        for x in e.effects.iter().filter(|x| x.effect == 6) {
            let value = x.base_points + 1;
            let misc = x.misc_value;
            match x.aura {
                13 => self.stats[53] += value,
                22 => {
                    for r in (0..7).filter(|r| misc & (1 << r) != 0) {
                        self.stats[r] += value;
                    }
                }
                85 => {
                    if let Some(cell) = usize::try_from(misc + 61)
                        .ok()
                        .and_then(|i| self.stats.get_mut(i))
                    {
                        *cell += value;
                    }
                }
                99 => self.stats[8] += value,
                123 => self.stats[58] += -1 - x.base_points,
                124 => self.stats[9] += value,
                135 => self.stats[52] += value,
                150 => self.stats[26] += value,
                158 => self.stats[60] += value,
                161 => self.stats[68] += value,
                189 => {
                    for (r, &stat) in RATING_STATS.iter().enumerate() {
                        if misc & (1 << r) != 0 && stat != -1 {
                            self.stats[stat as usize + ITEM_MOD_BASE] += value;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// An enchant effect: 3 a spell, 4 a resistance, 5 a stat (`0x61f2b0`).
    fn add_enchant(&mut self, model: &Model, id: u32, amount: Option<i32>) {
        let Some(row) = model.enchant_rows.get(&id) else {
            return;
        };
        for e in &row.effects {
            let value = amount.unwrap_or((e.points_min + e.points_max) / 2);
            match e.kind {
                3 => self.add_spell(model, e.arg),
                4 => {
                    if let Some(cell) = self.stats.get_mut(e.arg as usize) {
                        *cell += value;
                    }
                }
                5 => self.add_stat(e.arg as usize, value),
                _ => {}
            }
        }
    }

    /// A roll: a positive id's enchants at their average, a negative id's by the seed (`0x61fa20`).
    fn add_roll(&mut self, model: &Model, f: &ItemLinkFields) {
        if f.random_property > 0 {
            let Some(row) = model.random_properties.get(&(f.random_property as u32)) else {
                return;
            };
            for e in &row.enchants {
                self.add_enchant(model, e.id, None);
            }
            return;
        }
        let Some(row) = model.random_suffixes.get(&f.random_property.unsigned_abs()) else {
            return;
        };
        for slot in 0..row.enchants.len() {
            let amount = row.value(slot, f.seed);
            self.add_enchant(model, row.enchants[slot], Some(amount));
        }
    }

    /// One group's cells: the stat fans out, and rejoins when every cell agrees (`0x61b9e0`).
    fn pretty(&mut self, stat: usize, group: &[usize]) {
        let value = self.stats[stat];
        let mut merge = true;
        let mut previous = 0;
        for (i, &g) in group.iter().enumerate() {
            self.stats[g] += value;
            merge = merge && (i == 0 || self.stats[g] == previous);
            previous = self.stats[g];
        }
        if merge {
            self.stats[stat] = self.stats[group[0]];
            for &g in group {
                self.stats[g] = 0;
            }
        } else {
            self.stats[stat] = 0;
        }
    }

    /// Joins the rating families when their cells agree (`0x61ba90`).
    fn clean_names(&mut self) {
        if self.stats[8] == self.stats[10] {
            self.stats[10] = 0;
        }
        self.pretty(56, &[53, 52]);
        self.pretty(42, &[27, 28, 29]);
        self.pretty(43, &[30, 31, 32]);
        self.pretty(44, &[33, 34, 35]);
        self.pretty(45, &[36, 37, 38]);
        self.pretty(47, &[39, 40, 41]);
        self.pretty(61, &[54]);
        self.pretty(7, &[8, 9]);
    }
}

/// A druid's main-hand weapon: its DPS over 54.81 at 14 a point (`0x61a8f0`).
fn feral_attack_power(v: &ItemTemplateView, damage: f32) -> i32 {
    if v.class != 2 || !matches!(v.inventory_type, 13 | 17 | 21 | 22) || v.delay_ms == 0 {
        return 0;
    }
    let dps = f64::from(damage) / (f64::from(v.delay_ms) * f64::from(0.001_f32));
    let cut = f64::from(54.81_f32);
    if dps > cut {
        ((dps - cut) * 14.0 + 0.5).floor() as i32
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::{stat_key, StatSummary};
    use crate::script::model::Model;
    use crate::script::{ItemLinkFields, ItemTemplateView};

    fn entries(s: &StatSummary) -> Vec<(String, i32)> {
        let mut e: Vec<_> = s.entries().collect();
        e.sort();
        e
    }

    #[test]
    fn stat_cells_name_their_tokens() {
        assert_eq!(stat_key(0), "RESISTANCE0_NAME");
        assert_eq!(stat_key(14), "ITEM_MOD_AGILITY_SHORT");
        assert_eq!(stat_key(42), "ITEM_MOD_HIT_RATING_SHORT");
        assert_eq!(stat_key(70), "EMPTY_SOCKET_RED");
    }

    #[test]
    fn a_template_folds_its_stats_and_a_delta_keeps_the_differences() {
        let model = Model::new();
        let a = ItemTemplateView {
            stats: vec![(3, 10), (7, 5), (31, 20)],
            armor: 100,
            socket_colors: [2, 8, 0],
            ..Default::default()
        };
        let b = ItemTemplateView {
            stats: vec![(3, 4), (7, 5)],
            armor: 150,
            ..Default::default()
        };
        let f = ItemLinkFields::default();
        let (sa, sb) = (
            StatSummary::of(&model, &a, &f),
            StatSummary::of(&model, &b, &f),
        );
        assert_eq!(
            entries(&sa),
            vec![
                ("EMPTY_SOCKET_BLUE".into(), 1),
                ("EMPTY_SOCKET_RED".into(), 1),
                ("ITEM_MOD_AGILITY_SHORT".into(), 10),
                ("ITEM_MOD_HIT_RATING_SHORT".into(), 20),
                ("ITEM_MOD_STAMINA_SHORT".into(), 5),
                ("RESISTANCE0_NAME".into(), 100),
            ]
        );
        let d = StatSummary::delta(&sa, &sb);
        assert_eq!(
            d.delta_lines(&|k| Some(k.to_string()))[..3],
            [
                "|cffff2020-50|r RESISTANCE0_NAME".to_string(),
                "|cff00ff00+6|r ITEM_MOD_AGILITY_SHORT".to_string(),
                "|cff00ff00+20|r ITEM_MOD_HIT_RATING_SHORT".to_string(),
            ]
        );
    }

    #[test]
    fn a_weapon_reads_its_dps_and_a_non_weapon_reads_none() {
        let model = Model::new();
        let f = ItemLinkFields::default();
        let sword = ItemTemplateView {
            damages: vec![(10.0, 20.0, 0)],
            delay_ms: 2000,
            ..Default::default()
        };
        assert_eq!(StatSummary::of(&model, &sword, &f).dps(), Some(7.5));
        assert_eq!(
            StatSummary::of(&model, &ItemTemplateView::default(), &f).dps(),
            None
        );
    }
}
