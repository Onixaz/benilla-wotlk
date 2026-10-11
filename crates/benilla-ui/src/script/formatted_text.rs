//! 3.3.5a's `SetFormattedText` formatter (`0x818070`), which is the client's own and not
//! `string.format`. It takes `%N$` argument positions, reads each argument with `lua_tonumber` or
//! `lua_tolstring` (so a wrong or missing one prints 0 or `(null)` and never raises), and hands
//! each conversion to MSVC's `_vsnprintf` (`0x88bbd2`, through `SStrPrintf` `0x76f070`) inside the
//! binding's 4096-byte buffer.

use mlua::{Lua, MultiValue, Value};

/// The binding's stack buffer (`0x48d864`, `0x9779e5`: `0x1000`), its terminator included.
const BUFFER: usize = 0x1000;

/// The longest flags-width-precision run one conversion takes (`0x818175`).
const MAX_SPEC: usize = 0x7d;

/// `fmt, ...` formatted as `0x818070` does: `args` is the binding's stack from its second slot,
/// the format first. The text ends at the first NUL, as the C string the setter receives does.
pub(crate) fn format(lua: &Lua, args: &MultiValue) -> mlua::Result<String> {
    let fmt = to_bytes(lua, args.front()).unwrap_or_default();
    let number = |i: usize| -> f64 {
        args.get(i)
            .and_then(|v| lua.coerce_number(v.clone()).ok().flatten())
            .unwrap_or(0.0)
    };
    // Past the end the scan reads the string's terminator.
    let at = |i: usize| fmt.get(i).copied().unwrap_or(0);

    let mut out: Vec<u8> = Vec::new();
    // Bytes left in the buffer, the terminator's included.
    let mut room = BUFFER;
    let put = |out: &mut Vec<u8>, room: &mut usize, piece: &[u8]| {
        // `SStrPrintf` writes at most `size - 1` and answers what it wrote.
        let n = piece.len().min(*room - 1);
        out.extend_from_slice(&piece[..n]);
        *room -= n;
    };
    let mut p = 0;
    let mut arg = 0usize;
    while p < fmt.len() && room > 1 {
        let c = fmt[p];
        p += 1;
        if c != b'%' {
            put(&mut out, &mut room, &[c]);
            continue;
        }
        if at(p) == b'%' {
            put(&mut out, &mut room, b"%");
            p += 1;
            continue;
        }
        // `%N$` names argument N, one digit; the next unnamed one follows it.
        if at(p).is_ascii_digit() && at(p + 1) == b'$' {
            arg = usize::from(at(p) - b'0');
            p += 2;
        } else {
            arg += 1;
        }
        let start = p;
        while matches!(at(p), b'-' | b'+' | b' ' | b'#' | b'0') {
            p += 1;
        }
        while at(p).is_ascii_digit() {
            p += 1;
        }
        if at(p) == b'.' {
            p += 1;
        }
        if at(p) == b'-' {
            p += 1;
        }
        while at(p).is_ascii_digit() {
            p += 1;
        }
        if p - start > MAX_SPEC {
            return Err(mlua::Error::runtime(
                "invalid format (width or precision too long)",
            ));
        }
        let spec = Spec::parse(&fmt[start..p]);
        let conv = at(p);
        p += 1;
        match conv {
            // The low byte of the truncated number, written raw: no width, no padding.
            b'c' => put(&mut out, &mut room, &[truncate(number(arg)) as u8]),
            // `0x84f030` then swaps `.` and `,` between digits under a locale flag, which an
            // integer's text never holds.
            b'd' | b'i' => {
                let v = truncate(number(arg));
                let text = spec.integer(v < 0, u64::from(v.unsigned_abs()), 10, false, true);
                put(&mut out, &mut room, &text);
            }
            b'o' | b'u' | b'x' | b'X' => {
                let v = truncate(number(arg)) as u32;
                let (base, upper) = match conv {
                    b'o' => (8, false),
                    b'u' => (10, false),
                    b'x' => (16, false),
                    _ => (16, true),
                };
                let text = spec.integer(false, u64::from(v), base, upper, false);
                put(&mut out, &mut room, &text);
            }
            // `F` is rewritten to `f` in the spec (`0x818290`).
            b'e' | b'E' | b'f' | b'F' | b'g' | b'G' => {
                let upper = matches!(conv, b'E' | b'G');
                let text = spec.float(conv.to_ascii_lowercase(), upper, number(arg));
                put(&mut out, &mut room, &text);
            }
            b's' => {
                let s = to_bytes(lua, args.get(arg)).unwrap_or_else(|| b"(null)".to_vec());
                put(&mut out, &mut room, &spec.string(&s));
            }
            _ => return Err(mlua::Error::runtime("invalid option in `format'")),
        }
    }
    if let Some(nul) = out.iter().position(|&b| b == 0) {
        out.truncate(nul);
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// `lua_tolstring`: a string's bytes, a number's text, else absent (a NULL `char*`).
fn to_bytes(lua: &Lua, v: Option<&Value>) -> Option<Vec<u8>> {
    let v = v?;
    if !matches!(v, Value::String(_) | Value::Integer(_) | Value::Number(_)) {
        return None;
    }
    lua.coerce_string(v.clone())
        .ok()
        .flatten()
        .map(|s| s.as_bytes().to_vec())
}

/// x87 truncation to the low dword, as `_ftol` (`0x88b9c0`) and `fistp` under a chop control word
/// (`0x818245`) leave it: a value past 64 bits, or NaN, is the integer indefinite, whose low dword
/// is 0.
fn truncate(n: f64) -> i32 {
    // 2^63, exact as a double.
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    if (-LIMIT..LIMIT).contains(&n) {
        n as i64 as i32
    } else {
        0
    }
}

/// One conversion's flags, width and precision.
#[derive(Default)]
struct Spec {
    left: bool,
    plus: bool,
    space: bool,
    alt: bool,
    zero: bool,
    width: usize,
    precision: Option<usize>,
}

impl Spec {
    fn parse(s: &[u8]) -> Spec {
        let mut spec = Spec::default();
        let mut i = 0;
        while let Some(&c) = s.get(i) {
            match c {
                b'-' => spec.left = true,
                b'+' => spec.plus = true,
                b' ' => spec.space = true,
                b'#' => spec.alt = true,
                b'0' => spec.zero = true,
                _ => break,
            }
            i += 1;
        }
        let digits = |i: &mut usize| {
            let mut n = 0usize;
            while let Some(d) = s.get(*i).filter(|c| c.is_ascii_digit()) {
                // No width outgrows the buffer, so a longer one pads the same.
                n = (n * 10 + usize::from(d - b'0')).min(BUFFER);
                *i += 1;
            }
            n
        };
        spec.width = digits(&mut i);
        if s.get(i) == Some(&b'.') {
            i += 1;
            // A `-` after the point is C's negative precision: none.
            let negative = s.get(i) == Some(&b'-');
            if negative {
                i += 1;
            }
            let p = digits(&mut i);
            spec.precision = (!negative).then_some(p);
        }
        spec
    }

    /// The sign column of a signed conversion.
    fn sign(&self, negative: bool) -> &'static [u8] {
        if negative {
            b"-"
        } else if self.plus {
            b"+"
        } else if self.space {
            b" "
        } else {
            b""
        }
    }

    /// `prefix` then `body`, padded to the width: spaces on the right under `-`, zeros between
    /// the two under `0` (MSVC zero-pads every conversion, `%s` included), else spaces first.
    fn pad(&self, prefix: &[u8], body: &[u8], zero: bool) -> Vec<u8> {
        let fill = self.width.saturating_sub(prefix.len() + body.len());
        let mut out = Vec::with_capacity(prefix.len() + body.len() + fill);
        if self.left {
            out.extend_from_slice(prefix);
            out.extend_from_slice(body);
            out.resize(out.len() + fill, b' ');
        } else if zero {
            out.extend_from_slice(prefix);
            out.resize(out.len() + fill, b'0');
            out.extend_from_slice(body);
        } else {
            out.resize(fill, b' ');
            out.extend_from_slice(prefix);
            out.extend_from_slice(body);
        }
        out
    }

    fn integer(&self, negative: bool, mag: u64, base: u32, upper: bool, signed: bool) -> Vec<u8> {
        let mut digits = match base {
            8 => format!("{mag:o}"),
            16 if upper => format!("{mag:X}"),
            16 => format!("{mag:x}"),
            _ => mag.to_string(),
        }
        .into_bytes();
        match self.precision {
            // An explicit zero precision prints no digit for zero.
            Some(0) if mag == 0 => digits.clear(),
            Some(p) if p > digits.len() => {
                let mut z = vec![b'0'; p - digits.len()];
                z.extend_from_slice(&digits);
                digits = z;
            }
            _ => {}
        }
        if base == 8 && self.alt && digits.first() != Some(&b'0') {
            digits.insert(0, b'0');
        }
        let prefix: &[u8] = match base {
            _ if signed => self.sign(negative),
            // `#` puts `0x` ahead of a nonzero value's digits and of any zero padding.
            16 if self.alt && mag != 0 && upper => b"0X",
            16 if self.alt && mag != 0 => b"0x",
            _ => b"",
        };
        // A precision turns the `0` flag off for integers.
        self.pad(prefix, &digits, self.zero && self.precision.is_none())
    }

    /// `e`, `f` or `g` (lowercase `conv`), its letters capitalised under `upper`.
    fn float(&self, conv: u8, upper: bool, v: f64) -> Vec<u8> {
        let negative = v.is_sign_negative();
        let prefix = self.sign(negative);
        let a = v.abs();
        if !a.is_finite() {
            // MSVC's spellings. Deviation: the reference also runs them through the precision
            // (`%f` prints `1.#INF00`); benilla prints them bare.
            let body: &[u8] = if a.is_infinite() {
                b"1.#INF"
            } else if negative {
                b"1.#IND"
            } else {
                b"1.#QNAN"
            };
            return self.pad(prefix, body, self.zero);
        }
        let precision = self.precision.unwrap_or(6);
        let d = Digits::of(a);
        let mut body = match conv {
            b'f' => d.fixed(precision, self.alt),
            b'e' => d.exponential(precision, self.alt),
            _ => {
                let p = precision.max(1);
                let x = d.exponent_at(p);
                let text = if x >= -4 && x < p as i32 {
                    d.fixed((p as i32 - 1 - x) as usize, self.alt)
                } else {
                    d.exponential(p - 1, self.alt)
                };
                if self.alt {
                    text
                } else {
                    trim_g(text)
                }
            }
        };
        if upper {
            body.make_ascii_uppercase();
        }
        self.pad(prefix, &body, self.zero)
    }

    fn string(&self, s: &[u8]) -> Vec<u8> {
        // `%s` stops at a NUL, as C reads the string.
        let s = s.split(|&b| b == 0).next().unwrap_or_default();
        let s = match self.precision {
            Some(p) if p < s.len() => &s[..p],
            _ => s,
        };
        self.pad(b"", s, self.zero)
    }
}

/// A non-negative finite value as the MSVC CRT converts it: 17 significant digits, which each
/// conversion then rounds half-up at its own place. A tie therefore rounds up (`%.1f` of 0.25 is
/// `0.3`), where a correctly rounded `printf` gives `0.2`.
struct Digits {
    /// ASCII digits; the value is `d[0].d[1..] * 10^exp`.
    digits: Vec<u8>,
    exp: i32,
}

impl Digits {
    fn of(a: f64) -> Digits {
        let s = format!("{a:.16e}");
        let (mantissa, exp) = s.split_once('e').expect("Rust's `e` form has an exponent");
        Digits {
            digits: mantissa.bytes().filter(u8::is_ascii_digit).collect(),
            exp: exp.parse().expect("an integer exponent"),
        }
    }

    /// The first `keep` significant digits rounded half-up, and whether the carry added one in
    /// front (`9.96` kept to two is `100`). A `keep` of zero or less rounds every digit away.
    fn round(&self, keep: i32) -> (Vec<u8>, bool) {
        let Ok(k) = usize::try_from(keep) else {
            return (Vec::new(), false);
        };
        let mut out: Vec<u8> = self.digits.iter().copied().take(k).collect();
        out.resize(k, b'0');
        if self.digits.get(k).is_none_or(|&d| d < b'5') {
            return (out, false);
        }
        for d in out.iter_mut().rev() {
            if *d == b'9' {
                *d = b'0';
            } else {
                *d += 1;
                return (out, false);
            }
        }
        out.insert(0, b'1');
        (out, true)
    }

    /// The exponent `%e` prints at `keep` significant digits, after its rounding.
    fn exponent_at(&self, keep: usize) -> i32 {
        let keep = i32::try_from(keep).unwrap_or(i32::MAX);
        self.exp + i32::from(self.round(keep).1)
    }

    /// `%.Nf`; `#` keeps the point at zero precision.
    fn fixed(&self, precision: usize, alt: bool) -> Vec<u8> {
        let p = i32::try_from(precision).unwrap_or(i32::MAX);
        let (mut s, carried) = self.round(self.exp.saturating_add(1).saturating_add(p));
        // Digits ahead of the point.
        let mut point = self.exp + 1 + i32::from(carried);
        if point <= 0 {
            let lead = (1 - point) as usize;
            s.splice(0..0, std::iter::repeat_n(b'0', lead));
            point = 1;
        }
        let point = point as usize;
        s.resize(point + precision, b'0');
        if precision > 0 || alt {
            s.insert(point, b'.');
        }
        s
    }

    /// `%.Ne`, with MSVC's exponent of at least three digits.
    fn exponential(&self, precision: usize, alt: bool) -> Vec<u8> {
        let keep = i32::try_from(precision)
            .unwrap_or(i32::MAX)
            .saturating_add(1);
        let (mut s, carried) = self.round(keep);
        s.truncate(precision + 1);
        // Zero has no leading digit to find; it prints `0.000000e+000`.
        let exp = if self.digits.iter().all(|&d| d == b'0') {
            0
        } else {
            self.exp + i32::from(carried)
        };
        if precision > 0 || alt {
            s.insert(1, b'.');
        }
        let sign = if exp < 0 { '-' } else { '+' };
        s.extend_from_slice(format!("e{sign}{:03}", exp.unsigned_abs()).as_bytes());
        s
    }
}

/// `%g`'s trailing zeros removed from the fraction, and the point if nothing follows it.
fn trim_g(text: Vec<u8>) -> Vec<u8> {
    let e = text.iter().position(|&b| b == b'e').unwrap_or(text.len());
    let (head, tail) = text.split_at(e);
    if !head.contains(&b'.') {
        return text;
    }
    let mut end = head.len();
    while head[end - 1] == b'0' {
        end -= 1;
    }
    if head[end - 1] == b'.' {
        end -= 1;
    }
    let mut out = head[..end].to_vec();
    out.extend_from_slice(tail);
    out
}

#[cfg(test)]
mod tests {

    use crate::script::UiScript;

    fn wotlk() -> UiScript {
        let s = UiScript::new().unwrap();
        s.eval::<()>(
            r#"local f = CreateFrame("Frame", "Holder")
               Label = f:CreateFontString("Label", "ARTWORK")
               Btn = CreateFrame("Button", "Btn", f)
               Check = CreateFrame("CheckButton", "Check", f)"#,
        )
        .unwrap();
        s
    }

    /// `Label:SetFormattedText(<args>)`, read back.
    fn formatted(s: &UiScript, args: &str) -> Option<String> {
        s.eval(&format!(
            "Label:SetFormattedText({args}) return Label:GetText()"
        ))
        .unwrap()
    }

    /// `WorldFrame_OnUpdate`'s framerate line.
    #[test]
    fn the_framerate_line_prints_one_decimal() {
        let s = wotlk();
        assert_eq!(formatted(&s, r#""%.1f", 59.94"#).as_deref(), Some("59.9"));
        assert_eq!(formatted(&s, r#""%.1f", 60"#).as_deref(), Some("60.0"));
    }

    #[test]
    fn conversions_print_as_the_msvc_crt_does() {
        let s = wotlk();
        let rows = [
            (r#""%d", 3.9"#, "3"),
            (r#""%d", -3.9"#, "-3"),
            (
                r#""%i|%04d|%-4d|%+d|% d", 1, -7, 7, 5, 5"#,
                "1|-007|7   |+5| 5",
            ),
            (r#""%.3d|%5.3d", 7, 7"#, "007|  007"),
            (
                r#""%x %X %#x %#X %o %#o %u", 255, 255, 255, 255, 8, 8, -1"#,
                "ff FF 0xff 0XFF 10 010 4294967295",
            ),
            (
                r#""%5.2f|%-7.1f|%07.2f|%+.0f|%#.0f", 3.14159, 2.25, -1.5, 2, 2"#,
                " 3.14|2.3    |-001.50|+2|2.",
            ),
            (r#""%F", 1.5"#, "1.500000"),
            (
                r#""%e|%.2E|%e", 12345.678, 0.000123, 0"#,
                "1.234568e+004|1.23E-004|0.000000e+000",
            ),
            (
                r#""%g|%g|%g|%g|%G|%g", 0.0001, 0.00001, 100000, 1000000, 1e-10, 0"#,
                "0.0001|1e-005|100000|1e+006|1E-010|0",
            ),
            (r#""%g|%#g|%.3g", 2.5, 2.5, 99.96"#, "2.5|2.50000|100"),
            (
                r#""%s|%.2s|%5s|%-5s|%05s", "abc", "abc", "ab", "ab", "ab""#,
                "abc|ab|   ab|ab   |000ab",
            ),
            (r#""%s %s", 12, 1.5"#, "12 1.5"),
            (r#""%c%c", 72, 105"#, "Hi"),
            (r#""100%%""#, "100%"),
            (r#""%5.1f%%", 12.34"#, " 12.3%"),
        ];
        for (args, want) in rows {
            assert_eq!(formatted(&s, args).as_deref(), Some(want), "{args}");
        }
    }

    /// Ties round up: the CRT rounds 17 digits half-up at the requested place.
    #[test]
    fn a_tie_rounds_up() {
        let s = wotlk();
        assert_eq!(
            formatted(
                &s,
                r#""%.1f %.1f %.0f %.0f %.2f", 0.25, 60.25, 0.5, 2.5, 0.125"#
            )
            .as_deref(),
            Some("0.3 60.3 1 3 0.13")
        );
        assert_eq!(
            formatted(&s, r#""%.1f %.0f", 9.96, 0.04"#).as_deref(),
            Some("10.0 0")
        );
    }

    /// `lua_tonumber` and `lua_tolstring` read each argument: nothing raises.
    #[test]
    fn a_wrong_or_missing_argument_prints_zero_or_null() {
        let s = wotlk();
        assert_eq!(
            formatted(&s, r#""%d|%d", "12", "abc""#).as_deref(),
            Some("12|0")
        );
        assert_eq!(
            formatted(&s, r#""%d %.1f %s""#).as_deref(),
            Some("0 0.0 (null)")
        );
        assert_eq!(formatted(&s, r#""%s", {}"#).as_deref(), Some("(null)"));
        // A nil format is an empty text, which `FontString:GetText` answers as nil.
        assert_eq!(formatted(&s, "nil"), None);
        assert_eq!(formatted(&s, "5").as_deref(), Some("5"));
    }

    #[test]
    fn a_position_names_its_argument_and_the_next_follows_it() {
        let s = wotlk();
        assert_eq!(
            formatted(&s, r#""%2$s %1$s", "a", "b""#).as_deref(),
            Some("b a")
        );
        assert_eq!(
            formatted(&s, r#""%2$s %s", "a", "b", "c""#).as_deref(),
            Some("b c")
        );
    }

    #[test]
    fn a_nul_ends_the_text() {
        let s = wotlk();
        assert_eq!(formatted(&s, r#""a%cb", 0"#).as_deref(), Some("a"));
    }

    #[test]
    fn an_unknown_conversion_or_a_long_spec_raises() {
        let s = wotlk();
        for (args, msg) in [
            (r#""%q", 1"#, "invalid option in `format'"),
            (r#""50%""#, "invalid option in `format'"),
            (
                &format!(r#""%{}d", 1"#, "1".repeat(126)) as &str,
                "invalid format (width or precision too long)",
            ),
        ] {
            let err = s
                .eval::<()>(&format!("Label:SetFormattedText({args})"))
                .unwrap_err()
                .to_string();
            assert!(err.contains(msg), "{args}: {err}");
        }
        // 125 is the longest spec taken.
        let long = formatted(&s, &format!(r#""%{}d", 1"#, "0".repeat(124) + "8"));
        assert_eq!(long.map(|t| t.len()), Some(8));
    }

    /// The 4096-byte buffer holds 4095 bytes and the terminator.
    #[test]
    fn the_text_stops_at_the_buffer() {
        let s = wotlk();
        let len: usize = s
            .eval(
                r#"Label:SetFormattedText(string.rep("x", 5000) .. "%s", "tail")
                   return string.len(Label:GetText())"#,
            )
            .unwrap();
        assert_eq!(len, 4095);
        let len: usize = s
            .eval(
                r#"Label:SetFormattedText("%s%s", string.rep("y", 4090), string.rep("z", 100))
                   return string.len(Label:GetText())"#,
            )
            .unwrap();
        assert_eq!(len, 4095);
    }

    #[test]
    fn a_button_takes_the_text_into_its_label() {
        let s = wotlk();
        assert_eq!(
            s.eval::<Option<String>>(r#"Btn:SetFormattedText("%d/%d", 3, 5) return Btn:GetText()"#)
                .unwrap()
                .as_deref(),
            Some("3/5")
        );
        assert_eq!(
            s.eval::<Option<String>>(
                r#"Check:SetFormattedText("%s", "on") return Check:GetText()"#
            )
            .unwrap()
            .as_deref(),
            Some("on")
        );
        // An empty text creates no label (`0x96f860`).
        assert!(s
            .eval::<bool>(
                r#"local b = CreateFrame("Button", "Bare", Holder)
                   b:SetFormattedText("")
                   return b:GetFontString() == nil"#
            )
            .unwrap());
    }

    #[test]
    fn set_formatted_text_is_a_fontstring_and_button_method() {
        let s = wotlk();
        for obj in ["Label", "Btn", "Check", "Btn:GetFontString() or Label"] {
            assert!(
                s.eval::<bool>(&format!(
                    "return type(({obj}).SetFormattedText) == 'function'"
                ))
                .unwrap(),
                "{obj}"
            );
        }
        // Not EditBox's: its registrar table (`0xb2d210`) has no such row.
        assert!(s
            .eval::<bool>("return CreateFrame('EditBox', 'Box').SetFormattedText == nil")
            .unwrap());
    }
}
