//! The grammar is 3.3.5a's: stock Lua 5.1 (`$Lua: Lua 5.1.1` at `0xa470a0`), which the vendored
//! `third_party/lua-src` tree is, byte for byte.

use super::common::script;

/// The compile error `loadstring` returns for `src`, or `"<compiled>"`.
fn compile_error(src: &str) -> String {
    script()
        .eval(&format!(
            "local f, e = loadstring({src:?}) return f and '<compiled>' or tostring(e)"
        ))
        .unwrap()
}

/// `#`, `%` and `...` as a value are the 3.3.5a FrameXML's everyday grammar: the client carries
/// `__len` (`0xa48184`), `__mod` (`0xa4819c`) and the opcode names `LEN` and `VARARG` (`0xa48318`).
#[test]
fn length_modulo_and_vararg_expressions_parse_and_run() {
    let s = script();
    assert_eq!(s.eval::<i64>("return #({ 1, 2, 3 }) + #'ab'").unwrap(), 5);
    assert_eq!(s.eval::<f64>("return 7 % 3").unwrap(), 1.0);
    assert_eq!(
        s.eval::<i64>("local function n(...) return #{ ... } end return n(1, nil, 3)")
            .unwrap(),
        3
    );
    assert_eq!(
        s.eval::<String>("local function f(a, ...) return a .. (...) end return f('x', 'y')")
            .unwrap(),
        "xy"
    );
}

/// Ace2's probe, `loadstring("return function(...) return ... end")`, answers true on 3.3.5a.
#[test]
fn the_ace2_lua51_probe_answers_true() {
    assert_eq!(
        compile_error("return function(...) return ... end"),
        "<compiled>"
    );
}

/// `...` outside a vararg function is the parser's own error (`0xa488d8`).
#[test]
fn a_vararg_outside_a_vararg_function_is_the_5_1_error() {
    let e = compile_error("return function() return ... end");
    assert!(
        e.ends_with("cannot use '...' outside a vararg function near '...'"),
        "{e}"
    );
}

/// 5.1 has no `OP_TFORPREP` (the opcode names at `0xa48318` run `TFORLOOP`, `FORPREP`), so a table
/// as a generic-for generator is called, and raises.
#[test]
fn a_table_generator_is_called_and_raises() {
    let e = script()
        .eval::<()>("for k, v in { 1, 2 } do end")
        .unwrap_err()
        .to_string();
    assert!(e.contains("attempt to call a table value"), "{e}");
}

/// `LUA_COMPAT_LSTR` is 5.1's 1: nesting is refused with the message the client carries
/// (`0xa48678`).
#[test]
fn a_nested_long_string_is_refused() {
    let e = compile_error("return [[a [[b]] c]]");
    assert!(e.contains("nesting of [[...]] is deprecated"), "{e}");
    assert_eq!(compile_error("return [==[a [[b]] c]==]"), "<compiled>");
}

/// 5.1's `constructor` has no compat-semicolon skip: one separator between fields, at most.
#[test]
fn a_double_separator_in_a_constructor_is_refused() {
    assert_eq!(compile_error("return { a = 1; b = 2; }"), "<compiled>");
    let e = compile_error("return { a = 1;; b = 2 }");
    assert!(e.ends_with("unexpected symbol near ';'"), "{e}");
}

/// Program elements are quoted `'x'`, as the client's formats are: `%s near '%s'` (`0xa48630`),
/// `attempt to %s %s '%s' (a %s value)` (`0xa474c4`), `bad argument #%d to '%s' (%s)`
/// (`0xa473b4`) and `'for' initial value must be a number` (`0xa48118`).
#[test]
fn errors_quote_program_elements_with_apostrophes() {
    let s = script();
    let raised = |lua: &str| {
        s.eval::<String>(&format!(
            "local ok, e = pcall(function() {lua} end) return tostring(e)"
        ))
        .unwrap()
    };
    let e = raised("local t = nil return t.x");
    assert!(
        e.contains("attempt to index local 't' (a nil value)"),
        "{e}"
    );
    let e = raised("return string.rep(nil, 2)");
    assert!(e.contains("bad argument #1 to 'rep'"), "{e}");
    let e = raised("for i = 'x', 2 do end");
    assert!(e.contains("'for' initial value must be a number"), "{e}");
    let e = compile_error("return 1 +");
    assert!(e.ends_with("near '<eof>'"), "{e}");
}
