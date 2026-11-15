//! The script verbs' per-widget-type name tables: `HasScript`, `GetScript`, `SetScript` and
//! `HookScript` (`0x49eab0`, `0x49eb70`, `0x49ec80`, `0x49edb0`) look a name up in the receiving
//! type's own chain, case-insensitively.

use super::common::script;

/// Per type: a name its table carries and one it lacks.
const CASES: &[(&str, &str, &str)] = &[
    ("Frame", "OnShow", "OnClick"),
    ("Button", "OnClick", "OnValueChanged"),
    ("CheckButton", "PostClick", "OnTextChanged"),
    ("EditBox", "OnEnterPressed", "OnClick"),
    ("ScrollFrame", "OnVerticalScroll", "OnValueChanged"),
    ("Slider", "OnValueChanged", "OnClick"),
    ("StatusBar", "OnMinMaxChanged", "OnClick"),
    ("Model", "OnAnimFinished", "OnClick"),
    ("PlayerModel", "OnUpdateModel", "OnClick"),
    ("DressUpModel", "OnUpdateModel", "OnTooltipCleared"),
    ("TabardModel", "OnAnimFinished", "OnClick"),
    ("ColorSelect", "OnColorSelect", "OnValueChanged"),
    ("SimpleHTML", "OnHyperlinkClick", "OnClick"),
    ("ScrollingMessageFrame", "OnHyperlinkEnter", "OnClick"),
    ("MessageFrame", "OnEvent", "OnHyperlinkClick"),
    ("MovieFrame", "OnUpdate", "OnClick"),
    ("GameTooltip", "OnTooltipSetItem", "OnClick"),
    ("Minimap", "OnMouseUp", "OnClick"),
    ("Cooldown", "OnHide", "OnUpdateModel"),
    ("QuestPOIFrame", "OnEnter", "OnClick"),
];

#[test]
fn each_type_answers_its_own_names_through_all_four_verbs() {
    let s = script();
    for (i, &(ty, has, lacks)) in CASES.iter().enumerate() {
        let name = format!("ScriptNames{i}");
        s.run(&format!(r#"W = CreateFrame("{ty}", "{name}")"#))
            .unwrap_or_else(|e| panic!("{ty}: {e}"));
        assert!(
            s.eval::<bool>(&format!(r#"return W:HasScript("{has}") == 1"#))
                .unwrap(),
            "{ty} has {has}"
        );
        assert!(
            s.eval::<bool>(&format!(r#"return W:HasScript("{lacks}") == nil"#))
                .unwrap(),
            "{ty} lacks {lacks}"
        );
        // The name it has: set, read back, hook, clear.
        s.run(&format!(
            r#"Got = nil
               W:SetScript("{has}", function() Got = "set" end)
               W:HookScript("{has}", function() Got = Got .. "+hook" end)
               W:GetScript("{has}")(W)"#
        ))
        .unwrap_or_else(|e| panic!("{ty} {has}: {e}"));
        assert_eq!(
            s.eval::<String>("return Got").unwrap(),
            "set+hook",
            "{ty} {has}"
        );
        s.run(&format!(r#"W:SetScript("{has}", nil)"#)).unwrap();
        assert!(
            s.eval::<bool>(&format!(r#"return W:GetScript("{has}") == nil"#))
                .unwrap(),
            "{ty} {has} cleared"
        );
        // The name it lacks: each of the three raises the missing-script text.
        let missing = format!("{name} doesn't have a \"{lacks}\" script");
        for call in [
            format!(r#"W:GetScript("{lacks}")"#),
            format!(r#"W:SetScript("{lacks}", function() end)"#),
            format!(r#"W:SetScript("{lacks}", nil)"#),
            format!(r#"W:HookScript("{lacks}", function() end)"#),
        ] {
            let err = s.run(&call).expect_err(&call).to_string();
            assert!(err.contains(&missing), "{ty} {call}: {err}");
        }
    }
}

#[test]
fn names_match_case_insensitively_and_store_under_the_canonical_spelling() {
    let s = script();
    s.run(
        r#"B = CreateFrame("Button")
           B:SetScript("onclick", function() return "x" end)"#,
    )
    .unwrap();
    assert!(s
        .eval::<bool>(r#"return B:HasScript("ONCLICK") == 1"#)
        .unwrap());
    assert!(s
        .eval::<bool>(r#"return B:GetScript("OnClick") ~= nil"#)
        .unwrap());
    assert!(s
        .eval::<bool>(r#"return B:GetScript("oNcLiCk") == B:GetScript("OnClick")"#)
        .unwrap());
    // A number is a name too (`lua_isstring`), one no type carries.
    assert!(s.eval::<bool>(r#"return B:HasScript(1) == nil"#).unwrap());
}

#[test]
fn bad_arguments_raise_the_usage_texts() {
    let s = script();
    s.run(r#"U = CreateFrame("Frame", "UsageProbe"); A = CreateFrame("Frame")"#)
        .unwrap();
    for (call, usage) in [
        (
            r#"U:HasScript(nil)"#,
            r#"Usage: UsageProbe:HasScript("type")"#,
        ),
        (
            r#"U:HasScript({})"#,
            r#"Usage: UsageProbe:HasScript("type")"#,
        ),
        (r#"U:GetScript()"#, r#"Usage: UsageProbe:GetScript("type")"#),
        (
            r#"U:SetScript("OnShow")"#,
            r#"Usage: UsageProbe:SetScript("type", function)"#,
        ),
        (
            r#"U:SetScript("OnShow", 5)"#,
            r#"Usage: UsageProbe:SetScript("type", function)"#,
        ),
        (
            r#"U:SetScript(nil, function() end)"#,
            r#"Usage: UsageProbe:SetScript("type", function)"#,
        ),
        (
            r#"U:HookScript("OnShow", nil)"#,
            r#"Usage: UsageProbe:HookScript("type", function)"#,
        ),
        (
            r#"A:GetScript(true)"#,
            r#"Usage: <unnamed>:GetScript("type")"#,
        ),
        (
            r#"A:GetScript("OnClick")"#,
            r#"<unnamed> doesn't have a "OnClick" script"#,
        ),
    ] {
        let err = s.run(call).expect_err(call).to_string();
        assert!(err.contains(usage), "{call}: {err}");
    }
}
