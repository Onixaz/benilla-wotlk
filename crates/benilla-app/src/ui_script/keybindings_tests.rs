//! The stock key-binding commands (`Bindings.xml` off the player's chain) as the load defines
//! them, and the pointer helpers the stock-window tests click with.

use benilla_ui::script::UiScript;

/// A frame's centre in screen pixels, where the pointer feed hits it.
pub(crate) fn centre(s: &mut UiScript, frame: &str) -> (f32, f32) {
    s.resolve();
    let (l, r, t, b, k): (f64, f64, f64, f64, f64) = s
        .eval(&format!(
            "local f = {frame} \
             local k = (f.GetEffectiveScale and f or f:GetParent()):GetEffectiveScale() \
             return f:GetLeft(), f:GetRight(), f:GetTop(), f:GetBottom(), k"
        ))
        .unwrap();
    (((l + r) * 0.5 * k) as f32, ((t + b) * 0.5 * k) as f32)
}

/// A click of `button` on `frame` through the pointer feed.
pub(crate) fn click(s: &mut UiScript, frame: &str, button: &str) {
    let (x, y) = centre(s, frame);
    s.mouse_move(x, y);
    s.mouse_button(x, y, button, true);
    s.mouse_button(x, y, button, false);
}

/// `loadstring` on each stock command body: the reference compiles each at load, and running one
/// would need FrameXML this test does not load.
#[test]
fn every_command_body_compiles() {
    benilla_formats::wow_data_or_skip!();
    let stock = benilla_ui::bindings_xml::parse(
        &crate::ui_script::stock_bindings_file().expect("the install's Bindings.xml"),
    )
    .unwrap();
    let s = UiScript::new().unwrap();
    for b in &stock {
        s.run(&format!(
            "local f, err = loadstring({body:?}); \
             if not f then BenillaBodyError = {name:?} .. \": \" .. err end",
            body = b.body,
            name = b.name
        ))
        .unwrap_or_else(|e| panic!("{}: {e}", b.name));
    }
    let bad: Vec<String> = s.errors();
    assert!(bad.is_empty(), "script errors: {bad:?}");
    s.run("if BenillaBodyError then error(BenillaBodyError) end")
        .unwrap_or_else(|e| panic!("a command body is not valid Lua: {e}"));
}

/// Every function and object a stock command body names exists once the in-game UI has loaded:
/// the engine's behaviour sits behind the Lua globals the bodies call. Read off each body: a
/// `Name(` call is a function, an `Object:` or `Object.` receiver a frame or table.
#[test]
#[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
fn every_stock_body_calls_what_the_load_defines() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (s, failures) = super::layer_tests::production_load_with("bindcalls", false, "", |_| {});
    assert!(failures.is_empty(), "load failures: {failures:#?}");
    let stock = benilla_ui::bindings_xml::parse(
        &crate::ui_script::stock_bindings_file().expect("the install's Bindings.xml"),
    )
    .unwrap();
    const KEYWORDS: [&str; 14] = [
        "if", "then", "else", "elseif", "end", "not", "and", "or", "local", "function", "return",
        "do", "while", "for",
    ];
    let mut missing = Vec::new();
    for b in stock.iter().filter(|b| b.platform.is_none()) {
        let body = b.body.as_bytes();
        let mut i = 0;
        while i < body.len() {
            let c = body[i];
            if !(c.is_ascii_alphabetic() || c == b'_') {
                i += 1;
                continue;
            }
            let start = i;
            while i < body.len() && (body[i].is_ascii_alphanumeric() || body[i] == b'_') {
                i += 1;
            }
            // A field or method name is its receiver's, not a global.
            if start > 0 && matches!(body[start - 1], b'.' | b':') {
                continue;
            }
            let name = &b.body[start..i];
            let next = b.body[i..].trim_start().chars().next();
            let want = match next {
                Some('(') if !KEYWORDS.contains(&name) => "function",
                Some(':') | Some('.') => "table",
                _ => continue,
            };
            let kind: String = s.eval(&format!("return type({name})")).unwrap();
            if kind != want {
                missing.push(format!("{}: {name} is {kind}", b.name));
            }
        }
    }
    assert!(missing.is_empty(), "unresolved: {missing:#?}");
}
