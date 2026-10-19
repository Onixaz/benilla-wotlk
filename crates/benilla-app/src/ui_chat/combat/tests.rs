//! The combat chat lines' slot orders against the install's `GlobalStrings.lua`, and the fill.

use super::*;

/// A template's `%s`/`%d` conversions in order.
fn signature(template: &str) -> Vec<char> {
    let mut out = Vec::new();
    let mut chars = template.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            match chars.next() {
                Some('%') | None => {}
                Some(k) => out.push(k),
            }
        }
    }
    out
}

/// The signature a family's slots give for a subject that is, or is not, the player.
fn declared_signature(family: Family, me: bool) -> Vec<char> {
    family
        .slots
        .iter()
        .filter_map(|slot| match slot {
            Slot::Subject => (!me && family.keying != Keying::Single).then_some('s'),
            Slot::Spell | Slot::Named => Some('s'),
            Slot::Amount => Some('d'),
        })
        .collect()
}

/// The install's `GlobalStrings.lua` run in a bare VM.
fn global_strings() -> Option<benilla_ui::script::UiScript> {
    let data = benilla_formats::wow_data_or_skip!(None);
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let src = chain
        .read_file("Interface\\FrameXML\\GlobalStrings.lua")
        .expect("GlobalStrings.lua in the chain");
    let script = benilla_ui::script::UiScript::new().expect("VM");
    script
        .run(&String::from_utf8_lossy(&src))
        .expect("GlobalStrings runs clean");
    Some(script)
}

/// Every family's keys exist in the install and take exactly the declared slots.
#[test]
fn every_family_matches_the_shipped_template() {
    let Some(script) = global_strings() else {
        return;
    };
    for &family in ALL_FAMILIES {
        let variants: &[bool] = match family.keying {
            Keying::Duo { .. } => &[true, false],
            Keying::Single => &[false],
        };
        for &me in variants {
            let key = family.key(me);
            let template = global_string(&script, &key)
                .unwrap_or_else(|| panic!("{key} is not a GlobalString"));
            if family.slots.is_empty() {
                continue; // printed verbatim, never formatted
            }
            assert_eq!(
                declared_signature(family, me),
                signature(&template),
                "{key}: our slots {:?} do not match the shipped template",
                family.slots,
            );
        }
    }
}

#[test]
fn the_fill_is_sstrprintf_and_refuses_a_mismatch() {
    fn s(v: &str) -> Arg<'_> {
        Arg::Str(v)
    }
    assert_eq!(
        fill("%s hits %s for %d.", &[s("A"), s("B"), Arg::Num(7)]).as_deref(),
        Some("A hits B for 7.")
    );
    assert_eq!(fill("100%% sure", &[]).as_deref(), Some("100% sure"));
    assert_eq!(fill("%d", &[s("A")]), None);
    assert_eq!(fill("%s", &[Arg::Num(1)]), None);
    assert_eq!(fill("%s %s", &[s("A")]), None);
    assert_eq!(fill("%s", &[s("A"), s("B")]), None);
    assert_eq!(fill("%f", &[Arg::Num(1)]), None);
}

/// A duo family spells the subject only in its `other` key.
#[test]
fn the_subject_is_spelled_only_for_someone_else() {
    let script = benilla_ui::script::UiScript::new().unwrap();
    script
        .run(
            "TRADESKILL_LOG_FIRSTPERSON = 'me:%s'
             TRADESKILL_LOG_THIRDPERSON = '%s:%s'",
        )
        .unwrap();
    let fills = Fills {
        subject: "A".into(),
        named: "B".into(),
        ..Fills::default()
    };
    assert_eq!(
        compose_line(&script, TRADESKILL_LOG, true, &fills).as_deref(),
        Some("me:B")
    );
    assert_eq!(
        compose_line(&script, TRADESKILL_LOG, false, &fills).as_deref(),
        Some("A:B")
    );
}
