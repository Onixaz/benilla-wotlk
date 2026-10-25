//! The breath and fatigue bars: stock `MirrorTimer.lua`, read back from Lua and from the draw
//! list, since a bar can be shown, correctly valued and still paint nothing.

use benilla_ui::script::{
    ExtractedQuad, MirrorTimer, QuadContent, ScriptValue, UiScript, MIRROR_TIMER_TOKENS,
};

/// The whole production interface, the three mirror bars among it.
fn harness() -> UiScript {
    super::test_ui::stock_ui(1024.0, 768.0)
}

/// The record a timer's name keeps in the VM's table.
fn record(name: &str) -> usize {
    MIRROR_TIMER_TOKENS
        .iter()
        .position(|t| *t == name)
        .expect("a timer token")
}

/// `MIRROR_TIMER_START` as `ui_mirror::feed_mirror_timers` fires it (the timer name, ms left, ms
/// total, the signed rate, the paused flag, the caption), then the record it writes.
fn start(
    s: &mut UiScript,
    name: &str,
    remaining_ms: i64,
    duration_ms: i64,
    scale: i64,
    label: &str,
) {
    s.fire_event(
        "MIRROR_TIMER_START",
        vec![
            ScriptValue::Str(name.into()),
            ScriptValue::Int(remaining_ms),
            ScriptValue::Int(duration_ms),
            ScriptValue::Int(scale),
            ScriptValue::Int(0),
            ScriptValue::Str(label.into()),
        ],
    );
    s.start_mirror_timer(
        record(name),
        MirrorTimer {
            value: remaining_ms as i32,
            max_value: duration_ms as i32,
            scale: scale as i32,
            ..Default::default()
        },
    );
}

/// `MIRROR_TIMER_STOP` as the drain fires it, then the record empties.
fn stop(s: &mut UiScript, name: &str) {
    s.fire_event("MIRROR_TIMER_STOP", vec![ScriptValue::Str(name.into())]);
    s.stop_mirror_timer(record(name));
}

fn shown(s: &UiScript, frame: &str) -> bool {
    s.eval::<bool>(&format!("return {frame}:IsShown()"))
        .unwrap()
}

fn bar_value(s: &UiScript, frame: &str) -> f64 {
    s.eval::<f64>(&format!("return {frame}StatusBar:GetValue()"))
        .unwrap()
}

fn bar_max(s: &UiScript, frame: &str) -> f64 {
    s.eval::<f64>(&format!(
        "local lo, hi = {frame}StatusBar:GetMinMaxValues(); return hi"
    ))
    .unwrap()
}

fn bar_color(s: &UiScript, frame: &str) -> (f64, f64, f64) {
    s.eval::<(f64, f64, f64)>(&format!(
        "local r, g, b = {frame}StatusBar:GetStatusBarColor(); return r, g, b"
    ))
    .unwrap()
}

/// A blank caption reads nil: `FontString:GetText` (`0x79d690`) returns nil for an empty string.
fn caption(s: &UiScript, frame: &str) -> Option<String> {
    s.eval::<Option<String>>(&format!("return {frame}Text:GetText()"))
        .unwrap()
}

/// One app tick in order (`tick_script`, then `paint_script`): OnUpdate, resolve, draw list.
fn frame(s: &mut UiScript, dt: f32) -> Vec<ExtractedQuad> {
    s.tick(dt);
    s.resolve();
    s.extract()
}

/// The `leaf` texture drawn over `MirrorTimer1`, the first bar: other frames of the interface draw
/// the same files elsewhere.
fn tex_quad<'a>(s: &UiScript, quads: &'a [ExtractedQuad], leaf: &str) -> Option<&'a ExtractedQuad> {
    let (l, b, r, t): (f32, f32, f32, f32) = s
        .eval("return MirrorTimer1:GetLeft(), MirrorTimer1:GetBottom(), MirrorTimer1:GetRight(), MirrorTimer1:GetTop()")
        .unwrap();
    quads.iter().find(|q| match &q.content {
        QuadContent::Texture { path: Some(p), .. } => {
            p.ends_with(leaf)
                && q.rect.is_some_and(|q| {
                    let (x, y) = ((q.left + q.right) / 2.0, (q.bottom + q.top) / 2.0);
                    x >= l && x <= r && y >= b && y <= t
                })
        }
        _ => false,
    })
}

#[test]
fn the_three_timers_load_hidden() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    for i in 1..=3 {
        assert!(!shown(&s, &format!("MirrorTimer{i}")), "MirrorTimer{i}");
    }
    let quads = frame(&mut s, 0.016);
    assert!(
        tex_quad(&s, &quads, "UI-CastingBar-Border").is_none(),
        "no border chrome before a timer starts"
    );
}

#[test]
fn breath_takes_the_first_bar_in_the_reference_blue() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    start(&mut s, "BREATH", 45_000, 60_000, -1, "Breath");

    assert!(shown(&s, "MirrorTimer1"));
    assert!(!shown(&s, "MirrorTimer2"), "only one timer is running");
    assert_eq!(caption(&s, "MirrorTimer1").as_deref(), Some("Breath"));
    // Milliseconds on the wire, seconds in the bar (`MirrorTimer.lua:47`).
    assert_eq!(bar_value(&s, "MirrorTimer1"), 45.0);
    assert_eq!(bar_max(&s, "MirrorTimer1"), 60.0);
    // MirrorTimerColors["BREATH"]
    let (r, g, b) = bar_color(&s, "MirrorTimer1");
    assert!(
        (r - 0.0).abs() < 1e-6 && (g - 0.5).abs() < 1e-6 && (b - 1.0).abs() < 1e-6,
        "reference blue, got ({r}, {g}, {b})"
    );
}

/// The client's key for fatigue is `EXHAUSTION`, not the server's `FATIGUE`.
#[test]
fn fatigue_is_the_reference_yellow_under_its_own_caption() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    start(&mut s, "EXHAUSTION", 60_000, 60_000, -1, "Fatigue");

    assert!(shown(&s, "MirrorTimer1"));
    assert_eq!(caption(&s, "MirrorTimer1").as_deref(), Some("Fatigue"));
    let (r, g, b) = bar_color(&s, "MirrorTimer1");
    assert!(
        (r - 1.0).abs() < 1e-6 && (g - 0.9).abs() < 1e-6 && (b - 0.0).abs() < 1e-6,
        "reference yellow, got ({r}, {g}, {b})"
    );
}

/// The frame integrates `scale * elapsed` per OnUpdate (`MirrorTimer.lua:106`).
#[test]
fn the_bar_drains_at_the_servers_signed_rate() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    start(&mut s, "BREATH", 45_000, 60_000, -1, "Breath");

    for _ in 0..100 {
        frame(&mut s, 0.05); // 5 s of client time, in reference-sized ticks
    }
    let after = bar_value(&s, "MirrorTimer1");
    assert!(
        (after - 40.0).abs() < 0.05,
        "5 s at scale -1 should land near 40, got {after}"
    );

    // Surfacing re-sends the timer as a START with the +10 refill rate; there is no update opcode.
    start(&mut s, "BREATH", 40_000, 60_000, 10, "Breath");
    for _ in 0..20 {
        frame(&mut s, 0.05); // 1 s
    }
    let refilled = bar_value(&s, "MirrorTimer1");
    assert!(
        (refilled - 50.0).abs() < 0.5,
        "1 s at scale +10 should climb ~10 bar-seconds to ~50, got {refilled}"
    );
}

#[test]
fn a_paused_timer_holds_its_value() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event(
        "MIRROR_TIMER_START",
        vec![
            ScriptValue::Str("BREATH".into()),
            ScriptValue::Int(45_000),
            ScriptValue::Int(60_000),
            ScriptValue::Int(-1),
            ScriptValue::Int(1), // paused
            ScriptValue::Str("Breath".into()),
        ],
    );
    for _ in 0..40 {
        frame(&mut s, 0.05);
    }
    assert_eq!(
        bar_value(&s, "MirrorTimer1"),
        45.0,
        "frozen: no integration at all"
    );

    // The stock PAUSE branch reads `arg1` as the timer name, then compares it with 0
    // (`MirrorTimer.lua:88`), which raises, so the bar stays frozen. vmangos never sends PAUSE: it
    // resends a START (`Player.cpp:933`).
    s.fire_event(
        "MIRROR_TIMER_PAUSE",
        vec![ScriptValue::Str("BREATH".into()), ScriptValue::Int(0)],
    );
    for _ in 0..20 {
        frame(&mut s, 0.05);
    }
    assert_eq!(
        bar_value(&s, "MirrorTimer1"),
        45.0,
        "the reference's own bugged branch cannot unfreeze a bar"
    );
    let errs = super::test_ui::errors_but_unported(&s);
    assert!(
        errs.iter().any(|e| e.contains("compare")),
        "…and says so, loudly, rather than failing quietly: {errs:?}"
    );
}

#[test]
fn two_timers_stack_and_restate_in_place() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    start(&mut s, "EXHAUSTION", 60_000, 60_000, -1, "Fatigue");
    start(&mut s, "BREATH", 45_000, 60_000, -1, "Breath");

    assert!(shown(&s, "MirrorTimer1") && shown(&s, "MirrorTimer2"));
    assert!(!shown(&s, "MirrorTimer3"));
    assert_eq!(caption(&s, "MirrorTimer1").as_deref(), Some("Fatigue"));
    assert_eq!(caption(&s, "MirrorTimer2").as_deref(), Some("Breath"));

    start(&mut s, "BREATH", 30_000, 60_000, -1, "Breath");
    assert!(!shown(&s, "MirrorTimer3"), "re-state must not take a frame");
    assert_eq!(bar_value(&s, "MirrorTimer2"), 30.0);
    assert_eq!(bar_value(&s, "MirrorTimer1"), 60.0, "fatigue untouched");
}

#[test]
fn stop_hides_that_timer_and_frees_its_frame() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    start(&mut s, "EXHAUSTION", 60_000, 60_000, -1, "Fatigue");
    start(&mut s, "BREATH", 45_000, 60_000, -1, "Breath");

    stop(&mut s, "EXHAUSTION");
    assert!(!shown(&s, "MirrorTimer1"), "fatigue's frame released");
    assert!(shown(&s, "MirrorTimer2"), "breath untouched");

    start(&mut s, "FEIGNDEATH", 10_000, 10_000, -1, "");
    assert!(shown(&s, "MirrorTimer1"));
    assert_eq!(
        caption(&s, "MirrorTimer1"),
        None,
        "GlobalStrings.lua defines no FEIGNDEATH_LABEL, and a blank caption reads nil"
    );
}

/// A world entry polls every record (`MirrorTimer.lua:79-88`): a running timer shows at once,
/// valued off its record, and an empty one hides its bar.
#[test]
fn entering_the_world_shows_the_running_timers_and_hides_the_rest() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    // A bar shown before an entry with no record behind it, as a relog leaves it.
    s.fire_event(
        "MIRROR_TIMER_START",
        vec![
            ScriptValue::Str("EXHAUSTION".into()),
            ScriptValue::Int(60_000),
            ScriptValue::Int(60_000),
            ScriptValue::Int(-1),
            ScriptValue::Int(0),
            ScriptValue::Str("Fatigue".into()),
        ],
    );
    // Breath is running in the table.
    s.start_mirror_timer(
        record("BREATH"),
        MirrorTimer {
            value: 45_000,
            max_value: 60_000,
            scale: -1,
            ..Default::default()
        },
    );

    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    assert!(
        super::test_ui::errors_but_unported(&s).is_empty(),
        "{:?}",
        super::test_ui::errors_but_unported(&s)
    );
    let shown_breath = (1..=3).find(|i| {
        let f = format!("MirrorTimer{i}");
        shown(&s, &f) && s.eval::<String>(&format!("return {f}.timer")).unwrap() == "BREATH"
    });
    let f = format!(
        "MirrorTimer{}",
        shown_breath.expect("breath shows at the entry")
    );
    assert_eq!(bar_value(&s, &f), 45.0, "valued off its record");
    assert_eq!(
        caption(&s, &f).as_deref(),
        Some("Breath"),
        "the label of the record"
    );
    assert_eq!(
        (1..=3)
            .filter(|i| shown(&s, &format!("MirrorTimer{i}")))
            .count(),
        1,
        "the empty records hide their bars"
    );
}

#[test]
fn a_stop_for_an_idle_timer_touches_nothing() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    start(&mut s, "BREATH", 45_000, 60_000, -1, "Breath");
    stop(&mut s, "EXHAUSTION");
    assert!(shown(&s, "MirrorTimer1"), "breath's bar survives");
    assert_eq!(bar_value(&s, "MirrorTimer1"), 45.0);
}

#[test]
fn a_running_timer_paints_its_chrome_and_fill() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    start(&mut s, "BREATH", 45_000, 60_000, -1, "Breath");
    let quads = frame(&mut s, 0.016);

    let border = tex_quad(&s, &quads, "UI-CastingBar-Border").expect("the bar's border chrome");
    let border_w = border.rect.expect("the border resolves a rect").width();
    assert!(border_w > 100.0, "the 256-wide border, got {border_w}");

    let fill = tex_quad(&s, &quads, "UI-StatusBar").expect("the status-bar fill");
    let fill_w = fill.rect.expect("the fill resolves a rect").width();
    // 45 of 60 seconds over the stock 195 px bar.
    let expected = 195.0 * 45.0 / 60.0;
    assert!(
        (fill_w - expected).abs() < 2.0,
        "fill should be ~{expected} px at 45/60, got {fill_w}"
    );

    assert!(
        quads.iter().any(|q| matches!(
            &q.content,
            QuadContent::Text { text: Some(t), .. } if t == "Breath"
        )),
        "the caption is drawn"
    );
}

#[test]
fn a_stopped_timer_paints_nothing() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    start(&mut s, "BREATH", 45_000, 60_000, -1, "Breath");
    frame(&mut s, 0.016);
    stop(&mut s, "BREATH");

    let quads = frame(&mut s, 0.016);
    assert!(tex_quad(&s, &quads, "UI-CastingBar-Border").is_none());
    assert!(tex_quad(&s, &quads, "UI-StatusBar").is_none());
}

/// The border and caption are `OVERLAY` regions of the timer frame, and the fill a child
/// StatusBar whose OnLoad drops its frame level by one, so both draw over the fill.
#[test]
fn the_border_and_caption_draw_over_the_fill() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    start(&mut s, "BREATH", 45_000, 60_000, -1, "Breath");
    let quads = frame(&mut s, 0.016);

    let fill = tex_quad(&s, &quads, "UI-StatusBar").expect("the status-bar fill");
    let border = tex_quad(&s, &quads, "UI-CastingBar-Border").expect("the border chrome");
    let caption = quads
        .iter()
        .find(|q| matches!(&q.content, QuadContent::Text { text: Some(t), .. } if t == "Breath"))
        .expect("the caption");

    assert!(
        border.z > fill.z,
        "the border must paint OVER the fill (border z={:#x}, fill z={:#x})",
        border.z,
        fill.z
    );
    assert!(
        caption.z > fill.z,
        "the caption must paint OVER the fill (caption z={:#x}, fill z={:#x})",
        caption.z,
        fill.z
    );
}
