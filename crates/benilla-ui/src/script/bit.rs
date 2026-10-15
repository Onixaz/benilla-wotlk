//! The client's `bit` library, which the stock Lua 5.1 does not have (`0x850cc0`-`0x850f90`).

use mlua::{Lua, Variadic};

/// Install `bit`: its eight functions (`reference/3.3.5a-shapes.tsv`), on 32-bit words.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let bit = lua.create_table()?;
    // `__ftol2` (`0x88b9f6`): truncate to an integer and keep the low 32 bits.
    fn word(v: f64) -> u32 {
        v as i64 as u32
    }
    // Every result but `arshift` and `mod` is pushed unsigned: a negative int gains 2^32.
    fn unsigned(v: u32) -> f64 {
        f64::from(v)
    }
    // `band`, `bor` and `bxor` fold every argument from the first (`0x850d00`).
    fn fold(lua: &Lua, op: fn(u32, u32) -> u32) -> mlua::Result<mlua::Function> {
        lua.create_function(move |_, (first, rest): (f64, Variadic<f64>)| {
            Ok(unsigned(
                rest.iter().fold(word(first), |acc, &v| op(acc, word(v))),
            ))
        })
    }
    bit.set(
        "bnot",
        lua.create_function(|_, a: f64| Ok(unsigned(!word(a))))?,
    )?;
    bit.set("band", fold(lua, |a, b| a & b)?)?;
    bit.set("bor", fold(lua, |a, b| a | b)?)?;
    bit.set("bxor", fold(lua, |a, b| a ^ b)?)?;
    // Shift counts mask to 5 bits, as the client's x86 shifts do: `lshift(1, 32)` is 1.
    bit.set(
        "lshift",
        lua.create_function(|_, (a, n): (f64, f64)| Ok(unsigned(word(a) << (word(n) & 31))))?,
    )?;
    bit.set(
        "rshift",
        lua.create_function(|_, (a, n): (f64, f64)| Ok(unsigned(word(a) >> (word(n) & 31))))?,
    )?;
    bit.set(
        "arshift",
        lua.create_function(|_, (a, n): (f64, f64)| {
            Ok(f64::from((word(a) as i32) >> (word(n) & 31)))
        })?,
    )?;
    // `bit.mod`: the C remainder of the truncated ints; a divisor truncating to 0 answers `1 / b`.
    bit.set(
        "mod",
        lua.create_function(|_, (a, b): (f64, f64)| {
            let d = word(b) as i32;
            if d == 0 {
                return Ok(1.0 / b);
            }
            Ok(f64::from((word(a) as i32).wrapping_rem(d)))
        })?,
    )?;
    lua.globals().set("bit", bit)
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    #[test]
    fn the_bit_library_is_present_with_the_references_eight_functions() {
        let s = UiScript::new().unwrap();
        for name in [
            "bnot", "band", "bor", "bxor", "lshift", "rshift", "arshift", "mod",
        ] {
            assert!(
                s.eval::<bool>(&format!("return type(bit.{name}) == 'function'"))
                    .unwrap(),
                "bit.{name} is one of the array's eight entries"
            );
        }
        assert_eq!(
            s.eval::<i64>("return bit.band(0x1234, 0xFF)").unwrap(),
            0x34
        );
        assert_eq!(s.eval::<i64>("return bit.bor(0xF0, 0x0F)").unwrap(), 0xFF);
        assert_eq!(s.eval::<i64>("return bit.bxor(0xFF, 0x0F)").unwrap(), 0xF0);
        assert_eq!(s.eval::<i64>("return bit.lshift(1, 4)").unwrap(), 16);
        // `rshift` is logical, `arshift` propagates the sign.
        assert_eq!(s.eval::<i64>("return bit.rshift(-1, 28)").unwrap(), 15);
        assert_eq!(s.eval::<i64>("return bit.arshift(-1, 28)").unwrap(), -1);
        // The shift count masks to 5 bits, as the x86 instruction does.
        assert_eq!(s.eval::<i64>("return bit.lshift(1, 32)").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return bit.mod(17, 5)").unwrap(), 2);
        assert_eq!(s.eval::<i64>("return bit.mod(-17, 5)").unwrap(), -2);
    }

    /// The folds take every argument (`0x850d80`), as stock `Constants.lua` relies on.
    #[test]
    fn the_folds_take_every_argument() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<i64>("return bit.bor(0x1, 0x10, 0x100, 0x400, 0x4000)")
                .unwrap(),
            0x4511
        );
        assert_eq!(
            s.eval::<i64>("return bit.band(0xFF, 0x3C, 0x0F)").unwrap(),
            0x0C
        );
        assert_eq!(s.eval::<i64>("return bit.bxor(1, 2, 4, 1)").unwrap(), 6);
        assert_eq!(s.eval::<i64>("return bit.bor(7)").unwrap(), 7);
        assert!(s.run("bit.bor()").is_err());
    }

    /// The results are unsigned but for `arshift` and `mod`: a set top bit adds 2^32.
    #[test]
    fn results_are_unsigned_words() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return bit.bnot(0)").unwrap(), 0xFFFF_FFFF);
        assert_eq!(
            s.eval::<i64>("return bit.band(-2147483648, 0x80000000)")
                .unwrap(),
            0x8000_0000
        );
        assert_eq!(
            s.eval::<i64>("return bit.lshift(1, 31)").unwrap(),
            0x8000_0000
        );
        assert_eq!(
            s.eval::<f64>("return bit.mod(5, 0)").unwrap(),
            f64::INFINITY
        );
    }
}
