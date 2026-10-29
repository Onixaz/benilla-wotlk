//! `SpellDisplay`: one spell's `Spell.dbc` row and the client predicates over it.

use super::*;

/// A spell's `SPELL_EFFECT_OPEN_LOCK` effect: the `LockType` it opens and the slot it is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenLock {
    /// `EffectMiscValue`: the `LockType.dbc` id, such as pick lock 1, herbalism 2, mining 3.
    pub lock_type: u32,
    /// Which of `Effect[0..3]` is the OPEN_LOCK one.
    pub effect: usize,
}

/// The two companion lists, in the order the reference's token table names them
/// (`"CRITTER"`, `"MOUNT"`, `0xacccc4`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompanionKind {
    Critter,
    Mount,
}

/// `SPELL_EFFECT_SUMMON`.
const SPELL_EFFECT_SUMMON: u32 = 28;
/// `SPELL_AURA_MOUNTED`.
pub const SPELL_AURA_MOUNTED: u32 = 78;

/// `SpellShapeshiftForm` flag `0x1`: a stance, which `IsShapeShifted` (`0x721ca0`) does not count.
const FORM_FLAG_STANCE: u32 = 0x1;
/// `SpellShapeshiftForm` flag `0x400`: a true shapeshift that refuses every spell its mask does
/// not name (`0x72bac0`); Tharon'ja's skeleton and the Zombie form carry it.
const FORM_FLAG_NO_CASTS: u32 = 0x400;

/// `Spell_IsAreaAuraEffect` (`0x76bed0`): a passive with one of these effects is still listed.
const AREA_AURA_EFFECTS: [u32; 7] = [27, 35, 65, 119, 128, 129, 143];
/// `AttributesEx5` bits 27 and 28, `CanShowAura`'s self-cast pair (`0x53d580`).
const ATTR_EX5_HIDE_IF_SELF_CAST: u32 = 0x0800_0000;
const ATTR_EX5_HIDE_IF_NOT_SELF_CAST: u32 = 0x1000_0000;
const ATTR_EX6_ONLY_VISIBLE_TO_CASTER: u32 = 0x0010_0000;
/// `AttributesEx4 & 0x8000`: the spell is known but filed in no tab (`0x542856`).
const ATTR_EX4_NOT_IN_SPELLBOOK: u32 = 0x8000;

/// One spell's `Spell.dbc` row, as the display, tooltip and cast gates read it.
pub struct SpellDisplay {
    /// `ID` (column 0): the row's own id, which `GetEffectPoints` hands the level lookup
    /// `0x6e3130` (`0x6e384b`).
    pub id: u32,
    pub name: String,
    /// `SpellNameSubtext` enUS (column 153): "Rank N", even "Rank 1", or a word such as "Racial".
    pub rank: Option<String>,
    /// The icon's MPQ path, without extension; `None` for icon 0 or an id not in `SpellIcon.dbc`.
    pub icon: Option<String>,
    /// `SpellVisual.dbc` id (column 131, the first of two); 0 is a silent cast.
    pub visual: u32,
    /// The second `SpellVisual.dbc` id (column 132), taken below violence level 2 when nonzero
    /// (`GetSpellVisual` `0x7fa290`).
    pub visual_low_violence: u32,
    /// Projectile speed (column 47) in world units per second; 0 is an instant impact.
    pub speed: f32,
    /// `SpellMissile.dbc` id (column 163), a trajectory missile's flight; 0 for none.
    pub missile: u32,
    /// `Attributes` (column 4).
    pub attributes: u32,
    /// `SchoolMask` (column 225): one bit per school, Fireball `0x4`; immunity tests it against a
    /// school-immunity mask.
    pub school_mask: u32,
    /// `Mechanic` (column 3): the whole spell's `SpellMechanic.dbc` id.
    pub mechanic: u32,
    /// `AttributesEx` (column 5).
    pub attributes_ex: u32,
    /// `AttributesEx2` (column 6).
    pub attributes_ex2: u32,
    /// `AttributesEx3` (column 7). Bits `0x400` and `0x1000000` limit the equipped-item search to
    /// the main or the off hand (`0x5f0c50`).
    pub attributes_ex3: u32,
    /// `AttributesEx4` (column 8, `+0x20`).
    pub attributes_ex4: u32,
    /// `AttributesExE` (column 9, `+0x24`): `0x200` counts one more tick in `$o`, `0x2000` scales a
    /// periodic spell's `$d` and `$t` by cast speed.
    pub attributes_ex5: u32,
    /// `AttributesExF` (column 10, `+0x28`): `0x40` floats a cast's name in the combat text
    /// (`0x7519e0`), `0x400` keeps its aura out of the combat log (`0x752710`).
    pub attributes_ex6: u32,
    /// `AttributesEx7` (column 11, `+0x2c`); `0x20` files a totem in the totem bar's lists.
    pub attributes_ex7: u32,
    /// `TotemCategory[2]` (columns 222-223): the tool rows the spell needs, a totem's element.
    pub totem_categories: [u32; 2],
    /// `SpellFamilyName` (column 208): `GetSpellModifiers` (`0x7fd970`) applies nothing unless it
    /// is nonzero and equals the local player's class family.
    pub spell_family: u32,
    /// `SpellFamilyFlags` (columns 209-211, low dword first): the modifier rows that apply, one
    /// summed cell per set bit over all 96 (`0x7fd970`); bits 96 and up are always clear.
    pub spell_family_flags: u128,
    /// `PreventionType` (column 214): 1 silence, 2 pacify, 0 neither; the crowd control that
    /// refuses the spell locally (`0x6094f0`, from `TryCast` `0x6e4b60`). A stun refuses all.
    pub prevention_type: u32,
    /// `modalNextSpell` (column 48): cast by the client itself (`0x6e74aa`), at no target, when
    /// `SMSG_CAST_RESULT` for this in-flight spell arrives, success or failure (`0x6e7330`); if
    /// it is the running auto-repeat, that is only re-armed (`0x6e745d`). Every hunter shot
    /// names 75, Auto Shot: that is how shooting starts.
    pub modal_next_spell: u32,
    /// `Attributes & 0x40`, `SPELL_ATTR_PASSIVE`: the spellbook grays it.
    pub passive: bool,
    /// `castUI`, a 1.12.1 column 12340 dropped: nonzero keeps the spell out of the spellbook; 0 on
    /// every 3.3.5a spell.
    pub cast_ui: u32,
    /// `Effect[3]` (columns 71-73); the trainer's icon scans all three (`0x4d8fed`).
    pub effects: [u32; 3],
    /// The `SPELL_EFFECT_OPEN_LOCK` effect the GameObject interact-cast matches against a lock.
    pub open_lock: Option<OpenLock>,
    /// `baseLevel` (column 38, not `spellLevel`): the level effect values are quoted at, read by
    /// the effect-value walk (`0x6e3854`) and the cast-time scaling (`0x6e3340`).
    pub base_level: u32,
    /// `maxLevel` (column 37): caps an opener's skill at `maxLevel × 5`, unless 0 (`0x5ea6e3`).
    pub max_level: u32,
    /// `spellLevel` (column 39), read only by the Beast Training rank comparator.
    pub spell_level: u32,
    /// `Dispel` (column 2): the `SpellDispelType.dbc` id, as `UnitDebuff`'s third return.
    pub dispel: u32,
    /// `Category` (column 1): the shared-cooldown category; 0 is none.
    pub category: u32,
    /// The category's `SpellCategory.dbc` flags carry `0x2`, matching every cooldown query
    /// (`GetCooldownInfo` `0x6e13e0`); true only for wand Shoot's 351, whose swing sweeps the bar.
    pub category_wildcard: bool,
    /// `RecoveryTime` (column 29): the spell's own cooldown in ms.
    pub recovery_ms: u32,
    /// `InterruptFlags` (column 31): what breaks the cast; `0xf` on timed casts, `0x1` movement.
    pub interrupt_flags: u32,
    /// `AuraInterruptFlags` (column 32): what breaks the applied aura, such as food's sit-still
    /// bits. The cast-start moving gate reads its `0x18`, moving or turning (`0x609de3`).
    pub aura_interrupt_flags: u32,
    /// `ChannelInterruptFlags` (column 33): what breaks the channel, in aura-interrupt bits.
    pub channel_interrupt_flags: u32,
    /// `CategoryRecoveryTime` (column 30): the category's shared cooldown in ms.
    pub category_recovery_ms: u32,
    /// `StartRecoveryCategory` (column 205): the global cooldown's category, usually 133.
    pub start_recovery_category: u32,
    /// `StartRecoveryTime` (column 206): the global cooldown in ms, usually 1500; 0 for none.
    pub start_recovery_ms: u32,
    /// `powerType` (column 41): 0 mana, 1 rage, 3 energy, 5 runes, 6 runic power; health is -2,
    /// stored as `0xFFFFFFFE` (Life Tap, Health Funnel).
    pub power_type: u32,
    /// `manaCost` (column 42): the flat cost in `power_type`'s unit.
    pub mana_cost: u32,
    /// `RuneCostID` (column 226): the `SpellRuneCost.dbc` row a rune spell (power type 5) spends.
    pub rune_cost_id: u32,
    /// `ManaCostPercentage` (column 204): percent of base mana added to the flat cost.
    pub mana_cost_pct: u32,
    /// `manaCostPerlevel` (column 43): the per-level cost term of `GetPowerCost` (`0x6e31b0`).
    pub mana_cost_per_level: u32,
    /// `manaPerSecond` (column 44): the tooltip's "plus N per sec".
    pub mana_per_second: u32,
    /// `manaPerSecondPerLevel` (column 45): its per-level term (`0x7ff100`).
    pub mana_per_second_per_level: u32,
    /// `rangeIndex` (column 46): the `SpellRange.dbc` row.
    pub range_index: u32,
    /// `Targets` (column 16): the `TARGET_FLAG_*` seed of the cast arm's targeting word.
    pub targets: u32,
    /// `TargetCreatureType` (column 17): the creature types the spell may target, bit `type - 1`
    /// each; 0 is any. Hibernate takes Beast and Dragonkin, Banish Demon and Elemental.
    pub target_creature_type: u32,
    /// `EffectImplicitTargetA[0]` (column 86): the cast arm's switch adjusts the targeting word
    /// by it, and the usable walk's target-aura-state leg forks on it (6 enemy, 21 friend).
    pub implicit_target_a1: u32,
    /// `EffectImplicitTargetA[3]` (columns 86-88), then `EffectImplicitTargetB[3]` (89-91).
    pub effect_implicit_target_a: [u32; 3],
    pub effect_implicit_target_b: [u32; 3],
    /// `Stances` (column 12, the low dword of a 64-bit mask): forms the spell may be cast in, `1 << (form - 1)` each; 0 is none.
    pub stances: u32,
    /// Column 13, the mask's high dword (`SpellRec+0x34`), forms 33 and up; the bar-page chooser
    /// `0x53b970` reads it after the low one. No 12340 form id exceeds 32.
    pub stances_high: u32,
    /// `StancesNot` (column 14, likewise): forms the spell may not be cast in.
    pub stances_not: u32,
    /// `CasterAuraState` (column 20): state n needs bit `1 << (n - 1)` of the caster's
    /// `UNIT_FIELD_AURASTATE`; 0 is none.
    pub caster_aura_state: u32,
    /// `TargetAuraState` (column 21): the current target's required aura state (`0x6e3f58`).
    pub target_aura_state: u32,
    /// `Totem[2]` (columns 50-51): tools that must be carried, not consumed, such as a hammer.
    pub totems: [u32; 2],
    /// `Reagent[8]` and `ReagentCount[8]` (columns 52-59, 60-67): consumed items and counts.
    pub reagents: [(u32, u32); 8],
    /// `EquippedItemClass` (column 68, signed): the item class a worn item must be; -1 is none.
    pub equipped_item_class: i32,
    /// `EquippedItemSubClassMask` (column 69): the allowed subclasses, `1 << subclass` each.
    pub equipped_item_subclass_mask: u32,
    /// `EquippedItemInventoryTypeMask` (column 70): `1 << InventoryType` each; the item-target
    /// gate's last leg (`0x495d60`), whose mismatch is the local "Invalid target" (`0x0a`).
    pub equipped_item_inventory_type_mask: u32,
    /// `RequiresSpellFocus` (column 18): the `SpellFocusObject.dbc` object needed nearby.
    pub requires_spell_focus: u32,
    /// The `SpellShapeshiftForm.dbc` form: the first `SPELL_AURA_MOD_SHAPESHIFT` effect's
    /// `EffectMiscValue`, which the stance bar keys on (`0x4b2810`, `0x4b475c`).
    pub shapeshift_form: Option<u32>,
    /// `StanceBarOrder` (column 215, signed): the stance bar's sort key, negative last.
    pub stance_bar_order: i32,
    /// `ActiveIconID`'s texture, shown on the stance button while the form is active.
    pub active_icon: Option<String>,
    /// `ActiveIconID` (column 134) as stored: nonzero makes a second press cancel the spell's
    /// live aura. The client tests the id, not the texture (`0x4e55f0`, `0x4b36f0`).
    pub active_icon_id: u32,
    /// `Description` enUS (column 170): the tooltip body, its `$` tokens unsubstituted.
    pub description: Option<String>,
    /// `AuraDescription` enUS (column 187): the buff or debuff icon's tooltip.
    pub aura_description: Option<String>,
    /// `SpellDescriptionVariablesID` (column 232): the `SpellDescriptionVariables.dbc` row whose
    /// definitions the texts' `$<name>` read; 0 for none.
    pub description_variables_id: u32,
    /// `DurationIndex` (column 40): the `SpellDuration.dbc` row; 0 for no duration.
    pub duration_index: u32,
    /// `CastingTimeIndex` (column 28): the `SpellCastTimes.dbc` row; row 1 is instant.
    pub casting_time_index: u32,
    /// `ProcFlags` (column 34, `SpellRec+0x88`): bit 0 makes the description's `$t` a fixed five
    /// seconds.
    pub proc_flags: u32,
    /// `ProcChance` (column 35): percent; vmangos reads 101 as always, with no roll.
    pub proc_chance: u32,
    /// `ProcCharges` (column 36): the description expander's `$n` value before op 4.
    pub proc_charges: u32,
    /// `StackAmount` (column 49): the description expander's `$u` (`507ef1`).
    pub stack_amount: u32,
    /// `MaxTargetLevel` (column 207): the description expander's `$v` (`507f4f`).
    pub max_target_level: u32,
    /// `MaxAffectedTargets` (column 212): the description expander's `$i` (`507fc3`).
    pub max_affected_targets: u32,
    /// `RequiredAuraVision` (column 221, `SpellRec+0x274`): an aura vision bit, 1-based, that
    /// [`Self::aura_vision_mask`] folds in; 0 for none.
    pub required_aura_vision: u32,
    /// `EffectBasePoints[3]` (columns 80-82, signed): each roll's floor; -1 on weapon damage.
    pub effect_base_points: [i32; 3],
    /// `EffectDieSides[3]` (columns 74-76, signed): with n dice, the value runs base + n to
    /// base + sides × n, n being [`Self::effect_base_dice`] plus its per-level term.
    pub effect_die_sides: [i32; 3],
    /// The dice count, 1 on every 12340 row: that client dropped `EffectBaseDice` and rolls
    /// `BasePoints + 1` to `BasePoints + DieSides` (`0x7ff7e6`, `0x7ff82a`).
    pub effect_base_dice: [i32; 3],
    /// `EffectAmplitude[3]` (columns 98-100): a periodic effect's tick in ms; 0 is not periodic.
    pub effect_amplitude: [u32; 3],
    /// `EffectApplyAuraName[3]` (columns 95-97): each effect's aura type; 0 is not an aura.
    pub effect_apply_aura: [u32; 3],
    /// `EffectMechanic[3]` (columns 83-85): each effect's `SpellMechanic.dbc` id. Immunity
    /// matches it or [`Self::mechanic`]; the `0x8d` "Can't do that while %s" line prefers it.
    pub effect_mechanic: [u32; 3],
    /// `EffectRadiusIndex[3]` (columns 92-94): each effect's `SpellRadius.dbc` row; 0 is none.
    pub effect_radius_index: [u32; 3],
    /// `EffectChainTarget[3]` (columns 104-106): extra chain targets beyond the first.
    pub effect_chain_targets: [u32; 3],
    /// `EffectMultipleValue[3]` (columns 101-103, float): the chain or multi-target falloff.
    pub effect_multiple_value: [f32; 3],
    /// `DmgMultiplier[3]` (columns 216-218, float): `$f`/`$F` print it scaled (`507ff7`, `508025`).
    pub damage_multiplier: [f32; 3],
    /// `EffectBonusMultiplier[3]` (columns 229-231, float): the expression operands `bc1`..`bc3`.
    pub effect_bonus_multiplier: [f32; 3],
    /// `EffectPointsPerComboPoint[3]` (columns 119-121, float): `$b` prints it chopped (`507ed4`).
    pub effect_points_per_combo_point: [f32; 3],
    /// `EffectTriggerSpell[3]` (columns 116-118): each effect's triggered spell, 0 for none.
    pub effect_trigger_spell: [u32; 3],
    /// `EffectItemType[3]` (columns 107-109): the item a `SPELL_EFFECT_CREATE_ITEM` effect makes.
    pub effect_item_type: [u32; 3],
    /// `EffectMiscValue[3]` (columns 110-112, signed).
    pub effect_misc_value: [i32; 3],
    /// `EffectMiscValueB[3]` (columns 113-115, signed): a summon effect's `SummonProperties` row.
    pub effect_misc_value_b: [i32; 3],
    /// The integer per-level dice term, 0 on every 12340 row, which dropped `EffectDicePerLevel`.
    pub effect_dice_per_level: [i32; 3],
    /// `EffectRealPointsPerLevel[3]` (columns 77-79): the float per-level term (`0x6e3889`); 5.0
    /// on Pick Lock, Mining and Herb Gathering, so the skill they provide tracks the player's.
    pub effect_real_points_per_level: [f32; 3],
}

/// Written out so `equipped_item_class` defaults to -1, no requirement: 0 is a real class.
impl Default for SpellDisplay {
    fn default() -> Self {
        SpellDisplay {
            id: 0,
            name: String::new(),
            rank: None,
            icon: None,
            visual: 0,
            visual_low_violence: 0,
            speed: 0.0,
            missile: 0,
            attributes: 0,
            school_mask: 0,
            mechanic: 0,
            attributes_ex: 0,
            attributes_ex2: 0,
            modal_next_spell: 0,
            attributes_ex3: 0,
            attributes_ex4: 0,
            spell_family: 0,
            spell_family_flags: 0,
            prevention_type: 0,
            passive: false,
            cast_ui: 0,
            effects: [0, 0, 0],
            open_lock: None,
            base_level: 0,
            max_level: 0,
            spell_level: 0,
            dispel: 0,
            category: 0,
            category_wildcard: false,
            recovery_ms: 0,
            interrupt_flags: 0,
            aura_interrupt_flags: 0,
            channel_interrupt_flags: 0,
            category_recovery_ms: 0,
            start_recovery_category: 0,
            start_recovery_ms: 0,
            power_type: 0,
            mana_cost: 0,
            rune_cost_id: 0,
            mana_cost_pct: 0,
            mana_cost_per_level: 0,
            mana_per_second: 0,
            mana_per_second_per_level: 0,
            range_index: 0,
            targets: 0,
            target_creature_type: 0,
            implicit_target_a1: 0,
            stances: 0,
            stances_high: 0,
            stances_not: 0,
            caster_aura_state: 0,
            target_aura_state: 0,
            totems: [0; 2],
            reagents: [(0, 0); 8],
            equipped_item_class: -1,
            equipped_item_subclass_mask: 0,
            equipped_item_inventory_type_mask: 0,
            requires_spell_focus: 0,
            shapeshift_form: None,
            stance_bar_order: 0,
            active_icon: None,
            active_icon_id: 0,
            description: None,
            aura_description: None,
            description_variables_id: 0,
            duration_index: 0,
            casting_time_index: 0,
            proc_flags: 0,
            proc_chance: 0,
            proc_charges: 0,
            stack_amount: 0,
            max_target_level: 0,
            max_affected_targets: 0,
            required_aura_vision: 0,
            effect_base_points: [0; 3],
            effect_die_sides: [0; 3],
            effect_base_dice: [0; 3],
            effect_dice_per_level: [0; 3],
            effect_real_points_per_level: [0.0; 3],
            effect_amplitude: [0; 3],
            effect_apply_aura: [0; 3],
            effect_implicit_target_a: [0; 3],
            effect_implicit_target_b: [0; 3],
            effect_mechanic: [0; 3],
            effect_radius_index: [0; 3],
            effect_chain_targets: [0; 3],
            effect_multiple_value: [0.0; 3],
            damage_multiplier: [0.0; 3],
            effect_bonus_multiplier: [0.0; 3],
            effect_points_per_combo_point: [0.0; 3],
            effect_trigger_spell: [0; 3],
            effect_item_type: [0; 3],
            effect_misc_value: [0; 3],
            effect_misc_value_b: [0; 3],
            attributes_ex5: 0,
            attributes_ex6: 0,
            attributes_ex7: 0,
            totem_categories: [0; 2],
        }
    }
}

impl SpellDisplay {
    /// The level the effect values, cast time, cost and duration scale by for a caster whose
    /// skill in this spell's line is `skill_value` (`0x6e3130`): the skill capped at
    /// `maxLevel × 5` unless `maxLevel` is 0 (`0x5ea6dc`-`0x5ea6ea`), over 5 (`0x6e3195`).
    pub fn skill_level(&self, skill_value: u32) -> u32 {
        let capped = if self.max_level > 0 {
            skill_value.min(self.max_level.saturating_mul(5))
        } else {
            skill_value
        };
        capped / 5
    }

    /// The `LockType` this spell opens, if any.
    pub fn open_lock_type(&self) -> Option<u32> {
        self.open_lock.map(|o| o.lock_type)
    }

    /// The lock skill this spell provides (`0x5f850f`), from the player's skill in its line:
    /// the effect value (`0x6e3800`) with its caller's rounding (`0x6e3760`). Its level term is
    /// that skill with its bonuses (`0x5ea690`), not the caster's level; a skill of 0 leaves only
    /// the flat terms.
    pub fn open_lock_skill(&self, skill_value: u32) -> Option<i32> {
        let e = self.open_lock?.effect;
        // Less baseLevel floored at 0 (read at `0x6e3854`, subtracted at `0x6e385b`-`0x6e385f`).
        let delta = self
            .skill_level(skill_value)
            .saturating_sub(self.base_level) as f32;
        let v = self.effect_base_points[e] as f32
            + self.effect_base_dice[e] as f32
            + self.effect_dice_per_level[e] as f32 * delta
            + self.effect_real_points_per_level[e] * delta;
        // `0x6e3760`'s rounding verbatim: double, bias away from zero by a half, round-to-nearest
        // (x87 `fistp`), then arithmetic-halve.
        let doubled = if v >= 0.0 {
            v * 2.0 - 0.5
        } else {
            v * 2.0 + 0.5
        };
        Some((doubled.round_ties_even() as i32) >> 1)
    }

    /// `SpellRec::IsHelpfulOrHarmful` (`0x7fe1b0`): the ally flag `0x100` is helpful, then the
    /// enemy flag `0x80` harmful; then any enemy A or B implicit target is harmful, before any
    /// friendly one is helpful (jump tables `0x7fe280`/`0x7fe2e4`, `0x7fe34c`/`0x7fe398`). A
    /// caster target (1) is helpful unless its effect applies aura 4.
    fn helpful_or_harmful(&self) -> SpellIntent {
        const ENEMY_TARGETS: [u32; 9] = [2, 6, 15, 16, 24, 28, 53, 54, 93];
        const FRIEND_TARGETS: [u32; 19] = [
            3, 4, 5, 20, 21, 27, 29, 30, 31, 33, 34, 35, 45, 56, 57, 58, 59, 61, 62,
        ];
        if self.targets & 0x100 != 0 {
            return SpellIntent::Helpful;
        }
        if self.targets & 0x80 != 0 {
            return SpellIntent::Harmful;
        }
        let (a, b) = (self.effect_implicit_target_a, self.effect_implicit_target_b);
        if (0..3).any(|i| ENEMY_TARGETS.contains(&a[i]) || ENEMY_TARGETS.contains(&b[i])) {
            return SpellIntent::Harmful;
        }
        let friendly = |target: u32, i: usize| {
            (target == 1 && self.effect_apply_aura[i] != 4) || FRIEND_TARGETS.contains(&target)
        };
        if (0..3).any(|i| friendly(a[i], i) || friendly(b[i], i)) {
            return SpellIntent::Helpful;
        }
        SpellIntent::Neither
    }

    /// [`Self::helpful_or_harmful`] answering harmful: the gate on the victim's flinch at a spell
    /// impact.
    pub fn is_harmful(&self) -> bool {
        self.helpful_or_harmful() == SpellIntent::Harmful
    }

    /// [`Self::helpful_or_harmful`] answering helpful: with no unit target, `GetMinMaxRange`'s
    /// wrapper (`0x801692`) reads the friendly `SpellRange.dbc` pair for such a spell.
    pub fn is_helpful(&self) -> bool {
        self.helpful_or_harmful() == SpellIntent::Helpful
    }

    /// The ranged-stance gate, `AttributesEx2 & 0x20` or `Attributes & 0x2`, as the cast-start
    /// visuals (`0x7fa2e0`) and the cast send (`CastSpell 0x80ccd0`) test it.
    pub fn ranged_attack(&self) -> bool {
        self.attributes_ex2 & ATTR_EX2_AUTO_REPEAT != 0 || self.attributes & ATTR_RANGED != 0
    }

    /// `Attributes & 0x200`: the cast aims itself at the equipped main hand, so a weapon imbue
    /// binds on the press with no item cursor (`CastSpell 0x80ccd0`, `BeginCast 0x80c790`, for a
    /// player caster). Only the shaman weapon imbues carry it.
    pub fn targets_main_hand_item(&self) -> bool {
        self.attributes & ATTR_TARGET_MAIN_HAND_ITEM != 0
    }

    /// `AttributesEx2 & 0x20` alone: the cast send sets the client's auto-repeat state from it
    /// (`0x6e593b`), which drives the shooter's Load/Hold idle. Throw is ranged, not auto-repeat.
    pub fn auto_repeat(&self) -> bool {
        self.attributes_ex2 & ATTR_EX2_AUTO_REPEAT != 0
    }

    /// `AttributesEx4 & 0x20` (`SendPetAction 0x5d4210`): a pet-bar spell press takes the generic
    /// cast entry and returns, sending no pet action and starting no GCD of its own.
    pub fn allows_client_targeting(&self) -> bool {
        self.attributes_ex4 & ATTR_EX4_ALLOW_CLIENT_TARGETING != 0
    }

    /// `AttributesEx3 & 0x400000`: a running auto-repeat with this bit ends when any new cast
    /// begins (`0x60959e`). Only wand Shoot 5019 has it; Auto Shot survives, so hunters weave.
    pub fn casting_cancels_autorepeat(&self) -> bool {
        self.attributes_ex3 & 0x0040_0000 != 0
    }

    /// `Attributes & 0x2` alone: a ranged-slot spell with no `SpellVisual` borrows the ranged
    /// weapon's `ItemDisplayInfo` visual (`0x60d46a`), which is how Throw and the shots animate.
    pub fn ranged_slot(&self) -> bool {
        self.attributes & ATTR_RANGED != 0
    }

    /// `AttributesEx3 & 0x8000`: damage floats melee white, not spell gold (`0x6128b0`); set on
    /// the basic ranged shots (Auto Shot, Shoot Bow, Throw, wand Shoot).
    pub fn melee_white_damage(&self) -> bool {
        self.attributes_ex3 & ATTR_EX3_NORMAL_RANGED_ATTACK != 0
    }

    /// `AttributesEx3 & 0x4`: `UnitCastingInfo` (`0x611df0`) answers an empty text, a blank cast
    /// bar label; `UnitChannelInfo` never reads it.
    pub fn no_casting_bar_text(&self) -> bool {
        self.attributes_ex3 & ATTR_EX3_NO_CASTING_BAR_TEXT != 0
    }

    /// `AttributesEx & 0x80000`: the caster is never a bind candidate, not for the cursor's unit
    /// test (`0x803a50`) nor the click that binds it (`0x80bc80`).
    pub fn excludes_caster(&self) -> bool {
        self.attributes_ex & ATTR_EX_EXCLUDE_CASTER != 0
    }

    /// `AttributesEx2 & 0x1`: a dead unit stays a bind candidate (`0x80bc80`, `0x809610`).
    pub fn allows_dead_target(&self) -> bool {
        self.attributes_ex2 & ATTR_EX2_ALLOW_DEAD_TARGET != 0
    }

    /// The `TargetCreatureType` gate of the bind click (`0x80bc80`) and its mirrors (`0x803a50`,
    /// `0x809610`): no mask admits any unit, else a unit of type 0 is refused and type `t` needs
    /// bit `t - 1`. The shift wraps at 32 as the hardware's does.
    pub fn admits_creature_type(&self, creature_type: u32) -> bool {
        self.target_creature_type == 0
            || (creature_type != 0
                && self.target_creature_type & 1u32.wrapping_shl(creature_type - 1) != 0)
    }

    /// `AttributesEx & 0x2000_0000`: `UnitChannelInfo` (`0x612090`) gives the `"player"` token
    /// the spell's own name, else `CHANNELING`; Fishing sets it.
    pub fn channel_bar_own_name(&self) -> bool {
        self.attributes_ex & ATTR_EX_CHANNEL_BAR_OWN_NAME != 0
    }

    /// `Attributes & 0x2` without `AttributesEx2 & 0x20000`: the caster's `SMSG_SPELL_GO` adds
    /// `UNIT_FIELD_RANGEDATTACKTIME` to its category cooldown locally (`0x6e2b60`).
    pub fn ranged_speed_cooldown(&self) -> bool {
        self.attributes & ATTR_RANGED != 0
            && self.attributes_ex2 & ATTR_EX2_DO_NOT_RESET_COMBAT_TIMERS == 0
    }

    /// The spellbook's add gate, the exits `AddKnownSpell` (`0x542030`) takes before it files a
    /// tab: `Attributes` `0x80` or `0x20`, a companion or mount (`0x5426a5`), and
    /// `AttributesEx4 & 0x8000` (`0x542856`). Passive alone does not hide a spell.
    pub fn in_spellbook(&self, summons: &crate::SummonPropertiesCatalog) -> bool {
        self.in_name_cache()
            && !self.is_companion(summons)
            && self.attributes_ex4 & ATTR_EX4_NOT_IN_SPELLBOOK == 0
    }

    /// Past `0x542030`'s `0x80` and `0x20` exits a spell joins the name cache (`0x542699`), which
    /// `CastSpellByName` searches: companions, mounts and `AttributesEx4 & 0x8000` included.
    pub fn in_name_cache(&self) -> bool {
        self.attributes & (ATTR_DO_NOT_DISPLAY | SPELL_ATTR_IS_TRADESKILL) == 0
    }

    /// The pet book's add gate (`0x4b2f90`) tests only `Attributes & 0x80`, as the sign of its
    /// low byte (`0x4b2fad`); it has none of [`Self::in_spellbook`]'s other two legs.
    pub fn in_pet_book(&self) -> bool {
        self.attributes & ATTR_DO_NOT_DISPLAY == 0
    }

    /// `Effect[0] == SPELL_EFFECT_ATTACK` (78): the client's trigger for showing the main-hand
    /// weapon's icon instead, and the usable walk's attack leg (`0x80b5d0`).
    pub fn is_melee_auto_attack(&self) -> bool {
        self.effects[0] == SPELL_EFFECT_ATTACK
    }

    /// The tooltip skips its cast-time and cooldown line for a passive or `Effect[0]` 47 or 78
    /// (`0x52eb15`-`0x52eb45`); [`Self::passive`] must keep reading the attribute alone.
    pub fn tooltip_omits_cast_line(&self) -> bool {
        self.passive
            || matches!(
                self.effects[0],
                SPELL_EFFECT_TRADE_SKILL | SPELL_EFFECT_ATTACK
            )
    }

    /// No tooltip range cell: tested before `GetMinMaxRange` (`0x6e3480`), an on-next-swing spell
    /// (`0x52e9a5`) or `AttributesEx3` bit 30 (`0x52e9b2`); after it, a max of 0 (`0x52e9ed`).
    pub fn tooltip_omits_range_line(&self) -> bool {
        self.on_next_swing() || self.attributes_ex3 & 0x4000_0000 != 0
    }

    /// `GetCooldownInfo` (`0x6e13e0`) reports no cooldown for `Effect[0]` 78 (attack) or 47
    /// (trade skill): those buttons never show one and are never refused by one.
    pub fn cooldown_query_excluded(&self) -> bool {
        matches!(
            self.effects[0],
            SPELL_EFFECT_TRADE_SKILL | SPELL_EFFECT_ATTACK
        )
    }

    /// The ranged icon substitution needs `Attributes & 0x2` and `AttributesEx2 & 0x20` both
    /// (`0x4b3f99`, `0x4e5a2e`): Auto Shot and wand Shoot show the weapon's icon, Throw its own.
    pub fn ranged_icon_substitution(&self) -> bool {
        self.attributes & ATTR_RANGED != 0 && self.attributes_ex2 & ATTR_EX2_AUTO_REPEAT != 0
    }

    /// `Attributes & 0x2000000` (`0x801220`): the cooldown is held until `SMSG_COOLDOWN_EVENT`.
    pub fn cooldown_on_event(&self) -> bool {
        self.attributes & ATTR_COOLDOWN_ON_EVENT != 0
    }

    /// A finisher (`AttributesEx` bit 20 or 22): unusable with no combo points (`0x80b5d0`).
    pub fn needs_combo_points(&self) -> bool {
        self.attributes_ex & ATTR_EX_FINISHING_MOVE != 0
    }

    /// `Attributes & 0x404`: queued on the server's melee slot for the next swing, not cast.
    pub fn on_next_swing(&self) -> bool {
        self.attributes & ATTR_ON_NEXT_SWING != 0
    }

    /// The tooltip's "Attack speed" arm: `Attributes & 0x2` alone (`0x52ec1c`), so Throw too.
    pub fn tooltip_on_next_ranged(&self) -> bool {
        self.attributes & ATTR_RANGED != 0
    }

    /// The tooltip cast cell's "Channeled" arm: `AttributesEx & 0x44` (`0x52ec27`).
    pub fn tooltip_channeled(&self) -> bool {
        self.attributes_ex & ATTR_EX_CHANNELED != 0
    }

    /// Predicate `0x7fcc70`: an on-next-swing, `INITIATES_COMBAT` or `INITIATE_COMBAT_POST_CAST`
    /// spell. The cast runs the attack validator's target pick for it (`CastSpell 0x80ccd0`), so
    /// a press with no hostile selection acquires one, as the Attack button does.
    pub fn initiates_combat(&self) -> bool {
        self.attributes & ATTR_ON_NEXT_SWING != 0
            || self.attributes_ex & ATTR_EX_INITIATES_COMBAT != 0
            || self.attributes_ex2 & ATTR_EX2_INITIATE_COMBAT_POST_CAST != 0
    }

    /// Casting this starts melee auto-attack at the send unless one runs (`CastSpell 0x80ccd0`):
    /// predicate `0x7fcc70` with `AttributesEx2` bit 20 clear, so an on-next-swing or
    /// `INITIATES_COMBAT` spell. Every cast path shares it; there is no macro opt-out.
    pub fn initiates_auto_attack(&self) -> bool {
        self.initiates_combat() && !self.initiates_auto_attack_at_go()
    }

    /// This spell's own `SMSG_SPELL_GO` starts melee auto-attack at its first hit target:
    /// `AttributesEx2` bit 20, the complement of [`Self::initiates_auto_attack`] (`0x80e1b0`).
    ///
    /// Only this leg is built: the handler's other, `0x7fcc70` with `InterruptFlags` bit 3, never
    /// fires, as its rows already attack from the send. Built here, it would send a second
    /// `CMSG_ATTACKSWING`: our engaged state is the server's echo.
    pub fn initiates_auto_attack_at_go(&self) -> bool {
        self.attributes_ex2 & ATTR_EX2_INITIATE_COMBAT_POST_CAST != 0
    }

    /// The spell's own legs of `CanShowAura` (`0x53d580`), every aura list's filter: not
    /// `Attributes & 0x80` nor `AttributesEx & 0x10000000` (warrior stances), not a passive
    /// without an area-aura effect (`0x76bed0`), and not `AttributesEx5` bit 27 on a self-cast
    /// aura nor bit 28 on another's. The caster and aura-vision legs read the viewer
    /// ([`Self::only_visible_to_caster`], `required_aura_vision`).
    pub fn shows_aura(&self, self_cast: bool) -> bool {
        if self.attributes & ATTR_DO_NOT_DISPLAY != 0
            || self.attributes_ex & ATTR_EX_NO_AURA_ICON != 0
        {
            return false;
        }
        if self.passive && !self.effects.iter().any(|&e| AREA_AURA_EFFECTS.contains(&e)) {
            return false;
        }
        let hide = if self_cast {
            ATTR_EX5_HIDE_IF_SELF_CAST
        } else {
            ATTR_EX5_HIDE_IF_NOT_SELF_CAST
        };
        self.attributes_ex5 & hide == 0
    }

    /// `AttributesEx6 & 0x100000`: `CanShowAura` lists the aura only when the active player or
    /// its pet cast it (`0x53d580`).
    pub fn only_visible_to_caster(&self) -> bool {
        self.attributes_ex6 & ATTR_EX6_ONLY_VISIBLE_TO_CASTER != 0
    }

    /// The aura vision this aura's state kit needs on the active player (`0x7fe3e0`): bit
    /// `RequiredAuraVision - 1`, plus `0x20` for a `MOD_STEALTH_DETECT` effect and `0x40` for a
    /// `MOD_INVISIBILITY_DETECT` of type 0 or 10. 0 shows the kit to everyone.
    pub fn aura_vision_mask(&self) -> u8 {
        let mut mask = match self.required_aura_vision {
            0 => 0,
            n => 1u32.wrapping_shl(n - 1) as u8,
        };
        for (&aura, &misc) in self.effect_apply_aura.iter().zip(&self.effect_misc_value) {
            match aura {
                SPELL_AURA_MOD_STEALTH_DETECT => mask |= AURA_VISION_STEALTH,
                SPELL_AURA_MOD_INVISIBILITY_DETECT if misc == 0 || misc == 10 => {
                    mask |= AURA_VISION_INVISIBILITY
                }
                _ => {}
            }
        }
        mask
    }

    /// A tracking spell (`EffectApplyAuraName` 44, 45 or 151, `SpellIsTrackingAura 0x7fdf60`):
    /// a known one is listed in the minimap's tracking menu.
    pub fn tracking_aura(&self) -> bool {
        self.effect_apply_aura
            .iter()
            .any(|a| TRACKING_AURA_TYPES.contains(a))
    }

    /// `AttributesEx2` bit 19: `Stances` lists forms the spell may also be cast in, not forms it
    /// needs. The form gate `0x72bac0` and the tooltip's form line both test it; no spell that
    /// truly needs a form (stances, druid forms, stealth openers) carries it.
    pub fn form_mask_is_permissive(&self) -> bool {
        self.attributes_ex2 & ATTR_EX2_ALLOW_WHILE_NOT_SHAPESHIFTED != 0
    }

    /// The form gate as a boolean, the usable walk's form leg.
    pub fn usable_in_form(&self, form: u8, form_flags: u32) -> bool {
        self.form_refusal(form, form_flags).is_none()
    }

    /// `CheckShapeshiftRules` (`0x72bac0`) for the caster's form and that form's
    /// `SpellShapeshiftForm` flags (0 for no row): `StancesNot`, then `Stances`; then, in a true
    /// shapeshift (`IsShapeShifted 0x721ca0`: a form without flag `0x1`), `Attributes` bit 16 or
    /// the form's flag `0x400` refuses, while unshifted or in a stance `AttributesEx2` bit 19
    /// waives the rest; last, any `Stances` bit refuses. Not built: aura 275, which lifts every
    /// refusal for the spells it names.
    pub fn form_refusal(&self, form: u8, form_flags: u32) -> Option<FormRefusal> {
        let stance_bit = if form == 0 { 0 } else { 1u32 << (form - 1) };
        if self.stances_not & stance_bit != 0 {
            return Some(FormRefusal::NotShapeshift);
        }
        if self.stances & stance_bit != 0 {
            return None;
        }
        if form != 0 && form_flags & FORM_FLAG_STANCE == 0 {
            if self.attributes & ATTR_NOT_SHAPESHIFT != 0 || form_flags & FORM_FLAG_NO_CASTS != 0 {
                return Some(FormRefusal::NotShapeshift);
            }
        } else if self.form_mask_is_permissive() {
            return None;
        }
        (self.stances != 0 || self.stances_high != 0).then_some(FormRefusal::OnlyShapeshift)
    }
    /// The rank number the subtext carries, as `0x53b540` parses `SpellRec+0x224`: the first
    /// run of digits, wherever it starts ("Rank 2" is 2), and 0 when there is none ("Racial",
    /// "Apprentice", or no subtext).
    pub fn rank_number(&self) -> u32 {
        let mut n: u32 = 0;
        let mut seen = false;
        for c in self.rank.as_deref().unwrap_or("").bytes() {
            if c.is_ascii_digit() {
                n = n.wrapping_mul(10).wrapping_add(u32::from(c - b'0'));
                seen = true;
            } else if seen {
                break;
            }
        }
        n
    }

    /// `0x53b5a0`: the rank number, or `spellLevel` (`+0x9c`) for a spell whose subtext carries
    /// none; the bar's rank-replacement comparator (`0x5aafd0`) orders ranks by it.
    pub fn rank_or_level(&self) -> u32 {
        match self.rank_number() {
            0 => self.spell_level,
            n => n,
        }
    }

    /// A companion or mount (`0x8009b0`): effect 0 summons into slot 5, or aura 0 is `MOUNTED`.
    pub fn is_companion(&self, summons: &crate::SummonPropertiesCatalog) -> bool {
        self.companion_kind(summons).is_some()
    }

    /// Which list a companion spell joins (`0x53b410`): a summon into slot 5 is a critter, an
    /// aura-0 `MOUNTED` spell a mount; `None` is not a companion (`0x8009b0`).
    pub fn companion_kind(
        &self,
        summons: &crate::SummonPropertiesCatalog,
    ) -> Option<CompanionKind> {
        const SUMMON_SLOT_COMPANION: u32 = 5;
        if self.effects[0] == SPELL_EFFECT_SUMMON
            && u32::try_from(self.effect_misc_value_b[0])
                .ok()
                .and_then(|row| summons.slot(row))
                == Some(SUMMON_SLOT_COMPANION)
        {
            Some(CompanionKind::Critter)
        } else if self.effect_apply_aura[0] == SPELL_AURA_MOUNTED {
            Some(CompanionKind::Mount)
        } else {
            None
        }
    }

    /// The `SummonProperties.dbc` row of the first `SUMMON` effect, as
    /// `CGUnit_C::GetSummonProperties` (`0x71d1f0`) finds a unit's through its creating spell.
    pub fn summon_properties_row(&self) -> Option<u32> {
        let effect = self
            .effects
            .iter()
            .position(|&e| e == SPELL_EFFECT_SUMMON)?;
        u32::try_from(self.effect_misc_value_b[effect]).ok()
    }

    /// The creature a companion spell summons or rides, `EffectMiscValue[0]`, which
    /// `GetCompanionInfo` (`0x53e2c0`) returns first and names through the creature cache.
    pub fn companion_creature(&self) -> u32 {
        u32::try_from(self.effect_misc_value[0]).unwrap_or(0)
    }

    /// The name and rank as the client composes them, `"%s (%s)"` (`0x8468b0`), or the bare name
    /// when the rank is empty; the trainer (`0x4d96e0`) and the learn line (`0x4b2982`) use it.
    pub fn ranked_name(&self) -> String {
        match self.rank.as_deref() {
            Some(rank) if !rank.is_empty() => format!("{} ({})", self.name, rank),
            _ => self.name.clone(),
        }
    }

    /// Whether removing this spell prints "You have unlearned %s." (`0x14a`) with the bare name:
    /// `RemoveSpell` (`0x5e9fe0`) is silent for `IS_TRADESKILL` (`0x5ea170`), a nonzero `castUI`
    /// (`0x5ea17a`) or `DO_NOT_DISPLAY` (`0x5ea29b`). The learn line ignores `castUI`. The
    /// caller's suppress flag is clear for `SMSG_REMOVED_SPELL` (`0x5e43e3`), set on a rank-up
    /// (`0x5e6392`).
    pub fn announces_unlearn(&self) -> bool {
        self.attributes & SPELL_ATTR_IS_TRADESKILL == 0
            && self.cast_ui == 0
            && self.attributes & ATTR_DO_NOT_DISPLAY == 0
    }

    /// The chat line a learn prints, off `Attributes` alone (`0x4b2909`): none for `0x80`, the
    /// recipe line for `0x20`, else ability for `0x10` or spell; passive is not read. The block
    /// runs only on a live learn: the `SMSG_INITIAL_SPELLS` replay leaves its flag clear
    /// (`0x5deaa4`); `SMSG_LEARNED_SPELL` (`0x5e61c0`) and a rank-up (`0x4b2f61`) set it.
    pub fn learn_announcement(&self) -> Option<LearnAnnouncement> {
        if self.attributes & ATTR_DO_NOT_DISPLAY != 0 {
            return None;
        }
        if self.attributes & SPELL_ATTR_IS_TRADESKILL != 0 {
            return Some(LearnAnnouncement::Recipe);
        }
        Some(if self.attributes & ATTR_ABILITY != 0 {
            LearnAnnouncement::Ability
        } else {
            LearnAnnouncement::Spell
        })
    }
}

/// `IsHelpfulOrHarmful`'s three answers (`0x7fe1b0`: 0, 1, 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpellIntent {
    Neither,
    Helpful,
    Harmful,
}

/// Which `ERR_LEARN_*` line a learn prints; all three are system chat lines, not UI errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LearnAnnouncement {
    /// `0x37` `ERR_LEARN_SPELL_S`, "You have learned a new spell: %s."
    Spell,
    /// `0x38` `ERR_LEARN_ABILITY_S`, "You have learned a new ability: %s.", for `0x10`.
    Ability,
    /// `0x39` `ERR_LEARN_RECIPE_S`, "You have learned how to create a new item: %s.", for `0x20`;
    /// the name is bare, never ranked.
    Recipe,
}

/// A [`SpellDisplay::form_refusal`] verdict: the cast-fail reason `0x72bac0` writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormRefusal {
    /// `SPELL_FAILED_NOT_SHAPESHIFT` (12340's 68): a `StancesNot` hit, or a no-shapeshift spell
    /// in a form.
    NotShapeshift,
    /// `SPELL_FAILED_ONLY_SHAPESHIFT` (12340's 94, "Must be in %s"): not in a required form.
    OnlyShapeshift,
}

impl FormRefusal {
    /// The `SPELL_FAILED_*` reason in the shared numbering the cast-failure display keys on.
    pub fn reason(self) -> u8 {
        match self {
            FormRefusal::NotShapeshift => 0x3d,
            FormRefusal::OnlyShapeshift => 0x56,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpful_or_harmful_reads_the_harmful_pass_first() {
        let mut d = SpellDisplay::default();
        assert_eq!(d.helpful_or_harmful(), SpellIntent::Neither);
        d.targets = 0x100 | 0x80;
        assert_eq!(
            d.helpful_or_harmful(),
            SpellIntent::Helpful,
            "the ally flag first"
        );
        d.targets = 0;
        d.effect_implicit_target_a = [21, 93, 0];
        assert_eq!(
            d.helpful_or_harmful(),
            SpellIntent::Harmful,
            "an enemy target in any slot beats a friend in an earlier one"
        );
        d.effect_implicit_target_a = [21, 0, 0];
        assert!(d.is_helpful(), "TARGET_UNIT_TARGET_ALLY");
        d.effect_implicit_target_a = [1, 0, 0];
        assert!(d.is_helpful(), "the caster, with no aura 4");
        d.effect_apply_aura = [4, 0, 0];
        assert_eq!(
            d.helpful_or_harmful(),
            SpellIntent::Neither,
            "the caster under aura 4"
        );
        d.effect_implicit_target_b = [45, 0, 0];
        assert!(d.is_helpful(), "aura 4 on A falls through to B");
    }

    #[test]
    fn is_harmful_follows_the_client_classifier() {
        let mut d = SpellDisplay::default();
        assert!(!d.is_harmful(), "an empty row targets nobody");
        d.targets = 0x80;
        assert!(d.is_harmful(), "the enemy target flag alone is harmful");
        d.targets = 0x80 | 0x100;
        assert!(!d.is_harmful(), "the ally flag wins over the enemy flag");
        d.targets = 0;
        d.effect_implicit_target_a = [6, 0, 0];
        assert!(d.is_harmful(), "TARGET_UNIT_TARGET_ENEMY in slot 0");
        d.effect_implicit_target_a = [21, 0, 0];
        assert!(!d.is_harmful(), "a single-friend target is not harmful");
        d.effect_implicit_target_b = [0, 16, 0];
        assert!(d.is_harmful(), "an enemy area in a B slot counts too");
        d.effect_implicit_target_b = [0, 0, 0];
        d.effect_implicit_target_a = [22, 0, 54];
        assert!(d.is_harmful(), "the enemy cone in the third effect");
        d.targets = 0x100;
        assert!(!d.is_harmful(), "the ally flag short-circuits the walk");
    }

    #[test]
    fn learn_announcement_reads_the_three_attribute_bits_in_the_clients_order() {
        let mut d = SpellDisplay::default();
        assert_eq!(
            d.learn_announcement(),
            Some(LearnAnnouncement::Spell),
            "a plain row is a spell — Fireball 133 carries Attributes 0x10000"
        );
        d.attributes = 0x10;
        assert_eq!(
            d.learn_announcement(),
            Some(LearnAnnouncement::Ability),
            "SPELL_ATTR_ABILITY picks the ability wording"
        );
        d.attributes = 0x20;
        assert_eq!(
            d.learn_announcement(),
            Some(LearnAnnouncement::Recipe),
            "SPELL_ATTR_IS_TRADESKILL diverts to the recipe line"
        );
        d.attributes = 0x20 | 0x10;
        assert_eq!(
            d.learn_announcement(),
            Some(LearnAnnouncement::Recipe),
            "the tradeskill branch is taken before the ability bit is ever read"
        );
        d.attributes = 0x80;
        assert_eq!(
            d.learn_announcement(),
            None,
            "SPELL_ATTR_DO_NOT_DISPLAY is announced silently — every language and proficiency"
        );
        d.attributes = 0x80 | 0x20;
        assert_eq!(
            d.learn_announcement(),
            None,
            "…and the sign test at 0x4b290f runs before the tradeskill test at 0x4b2917"
        );
        d.attributes = 0x40;
        assert_eq!(
            d.learn_announcement(),
            Some(LearnAnnouncement::Spell),
            "PASSIVE is not read by the block at all — only DO_NOT_DISPLAY silences a spell"
        );
    }

    #[test]
    fn announces_unlearn_is_not_the_mirror_of_the_learn_gates() {
        let mut d = SpellDisplay::default();
        assert!(d.announces_unlearn(), "a plain row says it");
        d.attributes = 0x10;
        assert!(
            d.announces_unlearn(),
            "ABILITY is a learn-wording bit and nothing to this path"
        );
        d.attributes = 0x40;
        assert!(d.announces_unlearn(), "PASSIVE is not read here either");
        d.attributes = 0x20;
        assert!(
            !d.announces_unlearn(),
            "IS_TRADESKILL: 0x5ea170 zeroes the flag"
        );
        d.attributes = 0x80;
        assert!(!d.announces_unlearn(), "DO_NOT_DISPLAY: 0x5ea29b");
        d.attributes = 0;
        d.cast_ui = 1;
        assert!(
            !d.announces_unlearn(),
            "castUI > 0 never reaches 0x5ea292 — its single entry is 0x5ea17a jle"
        );
        assert_eq!(
            d.learn_announcement(),
            Some(LearnAnnouncement::Spell),
            "…while the SAME row still announces its learn: castUI is read after the message"
        );
    }

    /// The reference tests the subtext's first byte (`0x4b2963`), so an empty rank is no rank.
    #[test]
    fn ranked_name_appends_the_subtext_only_when_there_is_one() {
        let mut d = SpellDisplay {
            name: "Fireball".to_string(),
            ..SpellDisplay::default()
        };
        assert_eq!(d.ranked_name(), "Fireball", "no subtext, no parentheses");
        d.rank = Some(String::new());
        assert_eq!(
            d.ranked_name(),
            "Fireball",
            "an empty subtext is no subtext"
        );
        d.rank = Some("Rank 2".to_string());
        assert_eq!(d.ranked_name(), "Fireball (Rank 2)");
    }
}
