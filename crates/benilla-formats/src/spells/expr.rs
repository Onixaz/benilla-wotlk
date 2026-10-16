//! The `${…}` expression engine of the spell-text parser: `MathState`'s recursive-descent
//! compiler (`CompileSum` `0x5778b0` down to `CompilePrimary` `0x577530`) into a postfix program,
//! and its stack machine (`Evaluate` `0x578d10`), every value a 32-bit float.

/// The tokenizer's one-character tokens (`0xaceb2c`); anything else runs to the next of them.
const DELIMITERS: &[u8] = b"()[]^*/%+-#,> ";

/// The functions (`0xace898`), matched without case, and their argument counts.
const FUNCTIONS: [(&str, usize, Func); 12] = [
    ("abs", 1, Func::Abs),
    ("ceil", 1, Func::Ceil),
    ("floor", 1, Func::Floor),
    ("min", 2, Func::Min),
    ("max", 2, Func::Max),
    ("gt", 2, Func::Gt),
    ("lt", 2, Func::Lt),
    ("gte", 2, Func::Gte),
    ("lte", 2, Func::Lte),
    ("eq", 2, Func::Eq),
    ("cond", 3, Func::Cond),
    ("clamp", 3, Func::Clamp),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Func {
    Abs,
    Ceil,
    Floor,
    Min,
    Max,
    Gt,
    Lt,
    Gte,
    Lte,
    Eq,
    Cond,
    Clamp,
}

/// A value the active player supplies (`GetVariableValue` `0x5782d0`). Hands and schools are
/// numbered as the reference numbers them: hand 0 main, 1 off, 2 ranged; school 1 holy to 6 arcane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlayerValue {
    /// `STR`..`SPI`: `UNIT_FIELD_STAT0 + i`, floored at 0 (`GetEffectiveStat` `0x5774b0`).
    Stat(u8),
    /// `str`..`spi`: the stat less its positive and negative buffs.
    BaseStat(u8),
    /// `ap`: `UNIT_FIELD_ATTACK_POWER`, floored at 0.
    AttackPower,
    /// `AP`: attack power plus both mods, times one plus the multiplier, floored at 0.
    AttackPowerTotal,
    /// `rap`, `RAP`: the ranged pair.
    RangedAttackPower,
    RangedAttackPowerTotal,
    /// `mwb`/`MWB`, `owb`/`OWB`, `rwb`/`RWB`: the equipped weapon's first damage range from its
    /// item template, 0 unless that range is physical.
    WeaponMin(u8),
    WeaponMax(u8),
    /// `mw`/`MW`, `ow`/`OW`, `rw`/`RW`: the unit's damage fields, the minimum floored and the
    /// maximum ceiled, each at least 1.
    DamageMin(u8),
    DamageMax(u8),
    /// `ar`: the base armor; `AR`: the effective armor (`GetResistanceAndBuffs`, school 0).
    BaseArmor,
    Armor,
    /// `mws`, `ows`, `rws`: the attack time in seconds.
    AttackSpeed(u8),
    /// `pl`: `UNIT_FIELD_LEVEL`.
    Level,
    /// `hnd`: 2 with a two-handed weapon in the main hand, else 1.
    Hands,
    /// `sp`: the lowest of the six schools' damage bonus.
    SpellPower,
    /// `sph`..`spa`: one school's damage bonus, positive plus negative.
    SchoolSpellPower(u8),
    /// `bh`: `PLAYER_FIELD_MOD_HEALING_DONE_POS`.
    HealingPower,
    /// `ph`..`pa`: one school's `PLAYER_FIELD_MOD_DAMAGE_DONE_PCT`.
    SchoolDamagePct(u8),
    /// `pbh`: `PLAYER_FIELD_MOD_HEALING_PCT`; `pbhd`: `PLAYER_FIELD_MOD_HEALING_DONE_PCT`.
    HealingPct,
    HealingDonePct,
}

impl PlayerValue {
    /// Every value, the order a snapshot of them is kept in.
    pub const ALL: [PlayerValue; 49] = {
        use PlayerValue::*;
        [
            Stat(0),
            Stat(1),
            Stat(2),
            Stat(3),
            Stat(4),
            BaseStat(0),
            BaseStat(1),
            BaseStat(2),
            BaseStat(3),
            BaseStat(4),
            AttackPower,
            AttackPowerTotal,
            RangedAttackPower,
            RangedAttackPowerTotal,
            WeaponMin(0),
            WeaponMax(0),
            WeaponMin(1),
            WeaponMax(1),
            WeaponMin(2),
            WeaponMax(2),
            DamageMin(0),
            DamageMax(0),
            DamageMin(1),
            DamageMax(1),
            DamageMin(2),
            DamageMax(2),
            BaseArmor,
            Armor,
            AttackSpeed(0),
            AttackSpeed(1),
            AttackSpeed(2),
            Level,
            Hands,
            SpellPower,
            SchoolSpellPower(1),
            SchoolSpellPower(2),
            SchoolSpellPower(3),
            SchoolSpellPower(4),
            SchoolSpellPower(5),
            SchoolSpellPower(6),
            HealingPower,
            SchoolDamagePct(1),
            SchoolDamagePct(2),
            SchoolDamagePct(3),
            SchoolDamagePct(4),
            SchoolDamagePct(5),
            SchoolDamagePct(6),
            HealingPct,
            HealingDonePct,
        ]
    };

    /// Its place in [`Self::ALL`].
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&v| v == self).expect("listed")
    }
}

/// A named operand (`0xace8f8`, 140 names matched with case), each the variable op it compiles to;
/// a lower- and an upper-case name read the same value unless the reference splits them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathVar {
    /// `m1`..`m3`, `M1`..`M3`: the slot's minimum or maximum points times the expander's scale.
    MinPoints(u8),
    MaxPoints(u8),
    /// `a1`..`A3`: the slot's radius through op 6.
    Radius(u8),
    /// `d`, `D`: the duration in seconds.
    Duration,
    /// `c`, `C`; `p`, `P`: the power cost and its per-second cost.
    PowerCost,
    PowerCostPerSecond,
    /// `x1`..`X3`: chain targets through op 17.
    ChainTargets(u8),
    /// `t1`..`T3`: the aura period in ms through op 19, 5000 under proc flag 1.
    AuraPeriod(u8),
    /// `h`, `H`: proc chance through op 18; `n`, `N`: proc charges through op 4.
    ProcChance,
    ProcCharges,
    /// `b1`..`B3`: points per combo point.
    PointsPerCombo(u8),
    /// `u`, `U`: the stack amount; `v`, `V`: the maximum target level; `i`, `I`: max targets.
    StackAmount,
    MaxTargetLevel,
    MaxTargets,
    /// `e1`..`E3`: `EffectMultipleValue` through op 27.
    MultipleValue(u8),
    /// `f1`..`F3`: `DamageMultiplier`.
    DamageMultiplier(u8),
    /// `q1`..`Q3`: `EffectMiscValue`.
    MiscValue(u8),
    /// `bc1`..`BC3`: `EffectBonusMultiplier` through op 24.
    BonusCoefficient(u8),
    Player(PlayerValue),
}

/// The 140 names in table order.
fn lookup_var(name: &str) -> Option<MathVar> {
    use MathVar::*;
    use PlayerValue as P;
    const STATS: [&str; 5] = ["STR", "AGI", "STA", "INT", "SPI"];
    const BASE: [&str; 5] = ["str", "agi", "sta", "int", "spi"];
    if let Some(i) = STATS.iter().position(|&s| s == name) {
        return Some(Player(P::Stat(i as u8)));
    }
    if let Some(i) = BASE.iter().position(|&s| s == name) {
        return Some(Player(P::BaseStat(i as u8)));
    }
    // A letter and a slot digit 1-3, either case.
    let b = name.as_bytes();
    if b.len() == 2 && (b'1'..=b'3').contains(&b[1]) {
        let slot = b[1] - b'1';
        let var = match b[0] {
            b'm' => MinPoints(slot),
            b'M' => MaxPoints(slot),
            b'a' | b'A' => Radius(slot),
            b'x' | b'X' => ChainTargets(slot),
            b't' | b'T' => AuraPeriod(slot),
            b'b' | b'B' => PointsPerCombo(slot),
            b'e' | b'E' => MultipleValue(slot),
            b'f' | b'F' => DamageMultiplier(slot),
            b'q' | b'Q' => MiscValue(slot),
            _ => return None,
        };
        return Some(var);
    }
    if b.len() == 3 && (b"bc" == &b[..2] || b"BC" == &b[..2]) && (b'1'..=b'3').contains(&b[2]) {
        return Some(BonusCoefficient(b[2] - b'1'));
    }
    let school = |s: &str| {
        ["h", "fi", "n", "fr", "s", "a"]
            .iter()
            .position(|&x| x == s)
    };
    let var = match name {
        "d" | "D" => Duration,
        "c" | "C" => PowerCost,
        "p" | "P" => PowerCostPerSecond,
        "h" | "H" => ProcChance,
        "n" | "N" => ProcCharges,
        "u" | "U" => StackAmount,
        "v" | "V" => MaxTargetLevel,
        "i" | "I" => MaxTargets,
        "ap" => Player(P::AttackPower),
        "AP" => Player(P::AttackPowerTotal),
        "rap" => Player(P::RangedAttackPower),
        "RAP" => Player(P::RangedAttackPowerTotal),
        "mwb" => Player(P::WeaponMin(0)),
        "MWB" => Player(P::WeaponMax(0)),
        "owb" => Player(P::WeaponMin(1)),
        "OWB" => Player(P::WeaponMax(1)),
        "rwb" => Player(P::WeaponMin(2)),
        "RWB" => Player(P::WeaponMax(2)),
        "mw" => Player(P::DamageMin(0)),
        "MW" => Player(P::DamageMax(0)),
        "ow" => Player(P::DamageMin(1)),
        "OW" => Player(P::DamageMax(1)),
        "rw" => Player(P::DamageMin(2)),
        "RW" => Player(P::DamageMax(2)),
        "ar" => Player(P::BaseArmor),
        "AR" => Player(P::Armor),
        "mws" | "MWS" => Player(P::AttackSpeed(0)),
        "ows" | "OWS" => Player(P::AttackSpeed(1)),
        "rws" | "RWS" => Player(P::AttackSpeed(2)),
        "pl" | "PL" => Player(P::Level),
        "hnd" | "HND" => Player(P::Hands),
        "sp" | "SP" => Player(P::SpellPower),
        "bh" | "BH" => Player(P::HealingPower),
        "pbh" | "pBH" => Player(P::HealingPct),
        "pbhd" | "pBHD" => Player(P::HealingDonePct),
        _ => {
            // `sph`..`spa` and `ph`..`pa`, each in lower or upper case alone.
            let lower = name.to_ascii_lowercase();
            let same_case = name == lower || name == name.to_ascii_uppercase();
            if !same_case {
                return None;
            }
            if let Some(s) = lower.strip_prefix("sp").and_then(school) {
                return Some(Player(P::SchoolSpellPower(s as u8 + 1)));
            }
            if let Some(s) = lower.strip_prefix('p').and_then(school) {
                return Some(Player(P::SchoolDamagePct(s as u8 + 1)));
            }
            return None;
        }
    };
    Some(var)
}

/// One postfix instruction.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Const(f32),
    /// A `$<name>`, its value read at compile time (`GetSpellVariableValue` `0x5774d0`).
    Var(f32),
    /// A named operand, of another spell when `$1234m1` names one (the op `MATHOP_SPELL` before
    /// it).
    Value(Option<u32>, MathVar),
    Pow,
    Neg,
    Mul,
    Div,
    Mod,
    Add,
    Sub,
    Func(Func),
}

/// A compiled expression.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Program {
    ops: Vec<Op>,
}

/// What a compile or a run reads: the description's variables, and each named operand.
pub trait ExprEnv {
    /// A `$<name>`'s value, 0 for none (`0x5774d0`).
    fn variable(&self, name: &str) -> f32;
    /// A named operand of the expanded spell, or of spell `spell`; `None` fails the run, as a
    /// missing spell record does (`0x578d4a`).
    fn value(&self, spell: Option<u32>, var: MathVar) -> Option<f32>;
}

struct Compiler<'a> {
    text: &'a [u8],
    env: &'a dyn ExprEnv,
    ops: Vec<Op>,
    failed: bool,
}

impl Compiler<'_> {
    fn at(&self, pos: usize) -> u8 {
        self.text.get(pos).copied().unwrap_or(0)
    }

    /// `NextToken` `0x576bb0`: spaces skipped, then one delimiter or a run up to the next one.
    fn next_token(&self, pos: &mut usize) -> Option<&[u8]> {
        while self.at(*pos) == b' ' {
            *pos += 1;
        }
        let start = *pos;
        let first = self.at(start);
        if first == 0 {
            return None;
        }
        if DELIMITERS.contains(&first) {
            *pos += 1;
        } else {
            while self.at(*pos) != 0 && !DELIMITERS.contains(&self.at(*pos)) {
                *pos += 1;
            }
        }
        Some(&self.text[start..*pos])
    }

    /// `Expect` `0x576c80`: the next token, case-insensitively.
    fn expect(&mut self, want: &[u8], pos: &mut usize) {
        match self.next_token(pos) {
            Some(t) if t.eq_ignore_ascii_case(want) => {}
            _ => self.failed = true,
        }
    }

    /// `CompileSum` `0x5778b0`. The operator must sit right after its left operand: a space before
    /// it ends the expression (`m_text[pos]` is read unskipped).
    fn sum(&mut self, pos: usize) -> usize {
        let mut pos = self.product(pos);
        loop {
            let op = match self.at(pos) {
                b'+' => Op::Add,
                b'-' => Op::Sub,
                _ => return pos,
            };
            pos = self.product(pos + 1);
            self.ops.push(op);
        }
    }

    /// `CompileProduct` `0x5777f0`.
    fn product(&mut self, pos: usize) -> usize {
        let mut pos = self.power(pos);
        loop {
            let op = match self.at(pos) {
                b'*' => Op::Mul,
                b'/' => Op::Div,
                b'%' => Op::Mod,
                _ => return pos,
            };
            pos = self.power(pos + 1);
            self.ops.push(op);
        }
    }

    /// `CompilePower` `0x5777a0`, left-associative.
    fn power(&mut self, pos: usize) -> usize {
        let mut pos = self.primary(pos);
        while self.at(pos) == b'^' {
            pos = self.primary(pos + 1);
            self.ops.push(Op::Pow);
        }
        pos
    }

    /// `CompilePrimary` `0x577530`: a bracket, a negation, a `$` operand or function, or a number.
    /// The end of the text compiles to nothing.
    fn primary(&mut self, pos: usize) -> usize {
        let mut pos = pos;
        let Some(token) = self.next_token(&mut pos) else {
            return pos;
        };
        let token = token.to_vec();
        if token == b"(" {
            pos = self.sum(pos);
            self.expect(b")", &mut pos);
            return pos;
        }
        if token == b"-" {
            let end = self.primary(pos);
            self.ops.push(Op::Neg);
            return end;
        }
        if token[0] == b'$' {
            let name = &token[1..];
            if let Some(var_name) = name.strip_prefix(b"<") {
                let value = self.env.variable(&String::from_utf8_lossy(var_name));
                self.ops.push(Op::Var(value));
                self.expect(b">", &mut pos);
                return pos;
            }
            let func = FUNCTIONS
                .iter()
                .find(|(n, ..)| name.eq_ignore_ascii_case(n.as_bytes()));
            if let Some(&(_, args, func)) = func {
                self.expect(b"(", &mut pos);
                pos = self.sum(pos);
                for _ in 1..args {
                    match self.next_token(&mut pos) {
                        Some(b",") => {}
                        _ => self.failed = true,
                    }
                    pos = self.sum(pos);
                }
                self.ops.push(Op::Func(func));
                self.expect(b")", &mut pos);
                return pos;
            }
            let digits = name.iter().take_while(|b| b.is_ascii_digit()).count();
            let spell = (digits > 0).then(|| atoi(&name[..digits]));
            if let Some(var) = std::str::from_utf8(&name[digits..])
                .ok()
                .and_then(lookup_var)
            {
                self.ops.push(Op::Value(spell, var));
                return pos;
            }
            self.failed = true;
            return pos;
        }
        if !token[0].is_ascii_digit() && token[0] != b'.' {
            self.failed = true;
            return pos;
        }
        self.ops.push(Op::Const(atof(&token) as f32));
        pos
    }
}

/// Compile `text`, the expression between `${` and `}`; `None` where the reference's status ends
/// at 1 (`0x579a38`). Trailing text past the parsed expression is ignored.
pub fn compile(text: &str, env: &dyn ExprEnv) -> Option<Program> {
    let mut c = Compiler {
        text: text.as_bytes(),
        env,
        ops: Vec::new(),
        failed: false,
    };
    c.sum(0);
    (!c.failed).then_some(Program { ops: c.ops })
}

/// `Evaluate` `0x578d10` over `env`; `None` where a run fails. An empty program reads 0; a stack
/// that runs dry, which only a truncated expression such as `1+` builds, fails.
pub fn evaluate(program: &Program, env: &dyn ExprEnv) -> Option<f32> {
    let mut stack: Vec<f32> = Vec::with_capacity(8);
    for op in &program.ops {
        let value = match *op {
            Op::Const(v) | Op::Var(v) => v,
            Op::Value(spell, var) => env.value(spell, var)?,
            Op::Neg => -stack.pop()?,
            Op::Func(f) => {
                let n = FUNCTIONS.iter().find(|e| e.2 == f).map_or(0, |e| e.1);
                let at = stack.len().checked_sub(n)?;
                let args: Vec<f32> = stack.drain(at..).collect();
                function(f, &args)
            }
            binary => {
                let right = stack.pop()?;
                let left = stack.pop()?;
                operator(binary, left, right)
            }
        };
        stack.push(value);
    }
    Some(stack.last().copied().unwrap_or(0.0))
}

/// `ExecuteOperator` `0x577ad0`.
fn operator(op: Op, left: f32, right: f32) -> f32 {
    match op {
        Op::Pow => f64::from(left).powf(f64::from(right)) as f32,
        Op::Mul => right * left,
        Op::Div => left / right,
        // Floored modulo. Deviation: the reference subtracts the remainder from `left` instead
        // (`7%3` reads 6), a bug no shipped text reaches.
        Op::Mod => {
            let quotient = f64::from(left) / f64::from(right);
            (f64::from(left) - quotient.floor() * f64::from(right)) as f32
        }
        Op::Add => right + left,
        Op::Sub => left - right,
        _ => 0.0,
    }
}

/// `ExecuteFunction` `0x577900`, arguments in source order.
fn function(f: Func, a: &[f32]) -> f32 {
    let truth = |b: bool| if b { 1.0 } else { 0.0 };
    match f {
        Func::Abs => a[0].abs(),
        Func::Ceil => a[0].ceil(),
        Func::Floor => f64::from(a[0]).floor() as f32,
        Func::Min => {
            if a[0] < a[1] {
                a[0]
            } else {
                a[1]
            }
        }
        Func::Max => {
            if a[0] > a[1] {
                a[0]
            } else {
                a[1]
            }
        }
        Func::Gt => truth(a[1] < a[0]),
        Func::Lt => truth(a[1] > a[0]),
        Func::Gte => truth(a[1] <= a[0]),
        Func::Lte => truth(a[1] >= a[0]),
        Func::Eq => truth((a[0] - a[1]).abs() < 0.0001),
        Func::Cond => {
            if a[0] == 0.0 {
                a[2]
            } else {
                a[1]
            }
        }
        // The third argument under the larger of the first two, else the second if it is larger
        // than the first, else the first.
        Func::Clamp => {
            let upper = if a[1] <= a[0] { a[0] } else { a[1] };
            if a[2] <= upper {
                a[2]
            } else if a[1] > a[0] {
                a[1]
            } else {
                a[0]
            }
        }
    }
}

/// The CRT's `atoi` over leading digits.
fn atoi(digits: &[u8]) -> u32 {
    digits.iter().fold(0u32, |n, &b| {
        n.wrapping_mul(10).wrapping_add(u32::from(b - b'0'))
    })
}

/// The CRT's `atof` over a token, which holds no sign: the longest leading `digits[.digits]`
/// with an optional `e` exponent, 0 for none.
fn atof(token: &[u8]) -> f64 {
    let digits = |from: usize| {
        from + token[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let mut end = digits(0);
    if token.get(end) == Some(&b'.') {
        end = digits(end + 1);
    }
    let mantissa = end;
    if matches!(token.get(end), Some(b'e' | b'E'))
        && token.get(end + 1).is_some_and(u8::is_ascii_digit)
    {
        end = digits(end + 1);
    }
    let text = std::str::from_utf8(&token[..end]).unwrap_or("");
    text.parse()
        .or_else(|_| {
            std::str::from_utf8(&token[..mantissa])
                .unwrap_or("")
                .trim_end_matches('.')
                .parse()
        })
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `m1` is 10, `M1` 20, spell 77's `m1` 3; `$<k>` is 4; everything else fails the run.
    struct Env;

    impl ExprEnv for Env {
        fn variable(&self, name: &str) -> f32 {
            if name == "k" {
                4.0
            } else {
                0.0
            }
        }
        fn value(&self, spell: Option<u32>, var: MathVar) -> Option<f32> {
            match (spell, var) {
                (None, MathVar::MinPoints(0)) => Some(10.0),
                (None, MathVar::MaxPoints(0)) => Some(20.0),
                (Some(77), MathVar::MinPoints(0)) => Some(3.0),
                (None, MathVar::Player(PlayerValue::AttackPowerTotal)) => Some(150.0),
                _ => None,
            }
        }
    }

    fn eval(text: &str) -> Option<f32> {
        evaluate(&compile(text, &Env)?, &Env)
    }

    #[test]
    fn precedence_runs_sum_over_product_over_power() {
        assert_eq!(eval("1+2*3"), Some(7.0));
        assert_eq!(eval("(1+2)*3"), Some(9.0));
        assert_eq!(eval("2^3^2"), Some(64.0), "left-associative");
        assert_eq!(eval("2*3^2"), Some(18.0));
        assert_eq!(eval("10-4-3"), Some(3.0));
        assert_eq!(eval("-2*3"), Some(-6.0));
        assert_eq!(eval("7%3"), Some(1.0));
        assert_eq!(eval("1/4"), Some(0.25));
        assert_eq!(eval(".5+1.25+2e1+3."), Some(24.75));
    }

    #[test]
    fn operands_and_variables_resolve() {
        assert_eq!(eval("$m1*3"), Some(30.0));
        assert_eq!(eval("$M1-$m1"), Some(10.0));
        assert_eq!(eval("$77m1+1"), Some(4.0));
        assert_eq!(eval("$m1*$<k>"), Some(40.0));
        assert_eq!(eval("$<nope>+2"), Some(2.0), "an unknown variable reads 0");
        assert_eq!(eval("$AP*0.1"), Some(15.0));
        assert_eq!(eval("$m2"), None, "an unresolved operand fails the run");
    }

    #[test]
    fn functions_match_without_case() {
        assert_eq!(eval("$max(3,9)"), Some(9.0));
        assert_eq!(eval("$MIN(3,9)"), Some(3.0));
        assert_eq!(eval("$gt(3,2)"), Some(1.0));
        assert_eq!(eval("$GT(2,3)"), Some(0.0));
        assert_eq!(eval("$gte(2,2)+$lte(1,2)+$lt(1,2)"), Some(3.0));
        assert_eq!(eval("$eq(1,1.00001)"), Some(1.0));
        assert_eq!(eval("$cond(0,5,6)+$cond(1,5,6)"), Some(11.0));
        assert_eq!(eval("$floor(2.7)+$ceil(2.2)+$abs(-1)"), Some(6.0));
        assert_eq!(eval("$clamp(5,0,3)"), Some(3.0));
        assert_eq!(eval("$clamp(-1,0,3)"), Some(0.0));
        assert_eq!(eval("$clamp(-1,-2,3)"), Some(-1.0));
    }

    #[test]
    fn malformed_text_fails_or_stops_where_the_reference_does() {
        assert_eq!(
            eval("2 + 3"),
            Some(2.0),
            "a space before an operator ends it"
        );
        assert_eq!(
            eval("2+ 3"),
            Some(5.0),
            "spaces before an operand are skipped"
        );
        assert_eq!(eval("(1+2"), None);
        assert_eq!(eval("$max(1)"), None);
        assert_eq!(eval("$s1"), None, "s is not an operand");
        assert_eq!(eval("abc"), None);
        assert_eq!(eval(""), Some(0.0));
        assert_eq!(eval("1+"), None);
    }

    #[test]
    fn every_name_in_the_table_resolves() {
        let names = "STR AGI STA INT SPI str agi sta int spi m1 m2 m3 M1 M2 M3 a1 a2 a3 A1 A2 A3 \
            d D c C p P x1 x2 x3 X1 X2 X3 t1 t2 t3 T1 T2 T3 h H n N b1 b2 b3 B1 B2 B3 u U v V \
            e1 e2 e3 E1 E2 E3 i I f1 f2 f3 F1 F2 F3 q1 q2 q3 Q1 Q2 Q3 ap AP rap RAP mwb MWB owb \
            OWB rwb RWB mw MW ow OW rw RW ar AR mws MWS ows OWS rws RWS pl PL hnd HND sp SP sph \
            spfi spn spfr sps spa SPH SPFI SPN SPFR SPS SPA bh BH ph pfi pn pfr ps pa PH PFI PN \
            PFR PS PA pbh pBH pbhd pBHD bc1 bc2 bc3 BC1 BC2 BC3";
        let names: Vec<&str> = names.split_whitespace().collect();
        assert_eq!(names.len(), 140);
        for n in names {
            assert!(lookup_var(n).is_some(), "{n}");
        }
        for n in ["s1", "m4", "Sph", "Ap", "m0", "pBh", "bc4"] {
            assert!(lookup_var(n).is_none(), "{n}");
        }
        assert_eq!(
            lookup_var("SPFR"),
            Some(MathVar::Player(PlayerValue::SchoolSpellPower(4)))
        );
        assert_eq!(
            lookup_var("pa"),
            Some(MathVar::Player(PlayerValue::SchoolDamagePct(6)))
        );
    }

    #[test]
    fn player_values_are_indexed_once() {
        for (i, v) in PlayerValue::ALL.iter().enumerate() {
            assert_eq!(v.index(), i);
        }
    }
}
