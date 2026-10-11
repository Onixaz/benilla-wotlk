//! The client's `bit` library, which the stock Lua 5.1 does not have.

use mlua::Lua;

/// Install `bit`: its eight functions (`reference/3.3.5a-shapes.tsv`), on 32-bit two's complement.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let bit = lua.create_table()?;
    // The client truncates to a 32-bit int and answers signed. `as i64 as u32` is C's wrapping
    // `(unsigned)(int)x`, not a saturating cast, so `bnot(0)` is -1 as there.
    fn u32_of(v: f64) -> u32 {
        v as i64 as u32
    }
    fn out(v: u32) -> i64 {
        v as i32 as i64
    }
    bit.set(
        "bnot",
        lua.create_function(|_, a: f64| Ok(out(!u32_of(a))))?,
    )?;
    bit.set(
        "band",
        lua.create_function(|_, (a, b): (f64, f64)| Ok(out(u32_of(a) & u32_of(b))))?,
    )?;
    bit.set(
        "bor",
        lua.create_function(|_, (a, b): (f64, f64)| Ok(out(u32_of(a) | u32_of(b))))?,
    )?;
    bit.set(
        "bxor",
        lua.create_function(|_, (a, b): (f64, f64)| Ok(out(u32_of(a) ^ u32_of(b))))?,
    )?;
    // Shift counts mask to 5 bits, as the client's x86 shifts do: `lshift(1, 32)` is 1.
    bit.set(
        "lshift",
        lua.create_function(|_, (a, n): (f64, f64)| Ok(out(u32_of(a) << (u32_of(n) & 31))))?,
    )?;
    bit.set(
        "rshift",
        lua.create_function(|_, (a, n): (f64, f64)| Ok(out(u32_of(a) >> (u32_of(n) & 31))))?,
    )?;
    bit.set(
        "arshift",
        lua.create_function(|_, (a, n): (f64, f64)| {
            Ok(out(((u32_of(a) as i32) >> (u32_of(n) & 31)) as u32))
        })?,
    )?;
    // `bit.mod`: an integer remainder, unlike `math.mod`'s float one.
    bit.set(
        "mod",
        lua.create_function(|_, (a, b): (f64, f64)| {
            let b = u32_of(b) as i32;
            if b == 0 {
                return Err(mlua::Error::runtime("bit.mod: division by zero"));
            }
            Ok(((u32_of(a) as i32) % b) as i64)
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
        // Two's complement, signed out: `bnot(0)` is -1, not 4294967295.
        assert_eq!(s.eval::<i64>("return bit.bnot(0)").unwrap(), -1);
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
    }
}
