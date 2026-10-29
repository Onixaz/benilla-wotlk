//! The macro wiring: persistence through the account data, the runner's route through the
//! stock chat frame, and the seed and dirty contract the plugin's systems rely on.

use benilla_ui::script::{MacroState, MacroView, UiScript};

fn macro_view(name: &str, body: &str) -> MacroView {
    MacroView {
        name: name.into(),
        texture: Some("Interface\\Icons\\Ability_Ambush".into()),
        body: body.into(),
    }
}

/// The shutdown's save writes both `macros-cache.txt` through the account data in the reference's
/// grammar, the character set's ids on their base, and the files read back as the table.
#[test]
fn a_saved_macro_table_round_trips_through_the_account_data() {
    use crate::account_data::AccountData;
    let tmp = std::env::temp_dir().join(format!("benilla-macros-{}", std::process::id()));
    std::fs::remove_dir_all(&tmp).ok();
    let (account_dir, character_dir) = (tmp.join("ACC"), tmp.join("ACC/Realm/Probeone"));
    let mut data = AccountData::default();
    data.initialize(true, Some(account_dir.clone()));
    data.initialize(false, Some(character_dir.clone()));
    let times = benilla_protocol::messages::AccountDataTimes {
        server_time: 1,
        flag: 1,
        mask: 0xff,
        times: [Some(0); 8],
    };
    data.on_times(&times, &|_: &crate::account_data::SlotDef| false);

    let state = MacroState {
        account: vec![macro_view("Ambush", "/cast Ambush\n/say pew")],
        character: vec![macro_view("Charge", "/cast Charge")],
    };
    let mut world = bevy::prelude::World::new();
    world.insert_resource(crate::cvars::Cvars::default());
    world.insert_resource(data);
    world.insert_resource(super::MacroFiles {
        dirty: true,
        ..Default::default()
    });
    let mut script = UiScript::new().unwrap();
    script.set_macros(state.clone());
    world.insert_non_send_resource(script);
    super::save_to_account_data(&mut world);

    let account = std::fs::read_to_string(account_dir.join("macros-cache.txt")).unwrap();
    assert_eq!(
        account,
        "MACRO 1 \"Ambush\" Ability_Ambush\r\n/cast Ambush\r\n/say pew\r\nEND\r\n"
    );
    let character = std::fs::read_to_string(character_dir.join("macros-cache.txt")).unwrap();
    assert!(
        character.starts_with("MACRO 16777217 \"Charge\""),
        "{character}"
    );
    let back = MacroState {
        account: super::store::parse(&account, 0),
        character: super::store::parse(&character, super::store::CHARACTER_BASE),
    };
    assert_eq!(back, state);
    std::fs::remove_dir_all(&tmp).ok();
}

/// The real UI, so the whole route is under test: `run_macro` fires `EXECUTE_CHAT_LINE`, and the
/// stock `ChatFrame1` sends the line through its edit box.
fn ui() -> UiScript {
    crate::ui_script::test_ui::stock_ui(1024.0, 768.0)
}

/// One `EXECUTE_CHAT_LINE` per non-empty line, in order, which `ChatFrame_OnEvent` sends through
/// the edit box (`ChatFrame.lua`): a macro line lands where a typed one does.
#[test]
fn running_a_macro_runs_its_lines_through_the_references_edit_box() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui();
    s.set_macros(MacroState {
        account: vec![macro_view("Greet", "/wave\n\n  /say pew  \n/roll 1 100")],
        character: Vec::new(),
    });

    assert!(super::run_macro(&mut s, 1));
    let sends = s.take_chat_sends();
    assert_eq!(
        sends
            .iter()
            .map(|c| (c.text.as_str(), c.chat_type.as_str()))
            .collect::<Vec<_>>(),
        vec![("pew", "SAY")],
        "the line was trimmed before the parse, and sent as the type its slash named"
    );
    assert_eq!(
        s.take_emote_requests()
            .iter()
            .map(|e| e.token.as_str())
            .collect::<Vec<_>>(),
        vec!["WAVE"],
        "the emote line reached DoEmote through the reference's emote table"
    );
    assert_eq!(s.take_roll_requests(), vec![(1, 100)]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // An empty macro and an empty slot both run nothing and queue nothing.
    s.set_macros(MacroState {
        account: vec![macro_view("Blank", "   \n\n")],
        character: Vec::new(),
    });
    assert!(!super::run_macro(&mut s, 1));
    assert!(!super::run_macro(&mut s, 7));
    assert!(s.take_chat_sends().is_empty());
}

/// Index 37 is the first per-character macro (36 account slots before it).
#[test]
fn a_character_macro_runs_by_its_own_index() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui();
    s.set_macros(MacroState {
        account: Vec::new(),
        character: vec![macro_view("Charge", "/say Charge")],
    });
    assert!(super::run_macro(&mut s, 37));
    assert_eq!(
        s.take_chat_sends()
            .iter()
            .map(|c| c.text.clone())
            .collect::<Vec<_>>(),
        vec!["Charge".to_string()]
    );
}

/// The app's own load is not an edit, or every login would rewrite the file; a script mutation is.
#[test]
fn the_dirty_edge_distinguishes_a_load_from_an_edit() {
    let mut s = UiScript::new().unwrap();
    s.set_macros(MacroState {
        account: vec![macro_view("Ambush", "/cast Ambush")],
        character: Vec::new(),
    });
    assert!(
        !s.take_macros_dirty(),
        "loading from disk is not an edit — a save here would be a write-back loop"
    );

    s.run(r#"EditMacro(1, nil, nil, "/cast Backstab")"#)
        .unwrap();
    assert!(s.take_macros_dirty());
    assert_eq!(s.macros().account[0].body, "/cast Backstab");
}

#[test]
fn the_generation_moves_on_every_write_and_is_not_drained() {
    let mut s = UiScript::new().unwrap();
    let at_start = s.macros_generation();
    assert_eq!(s.macros_generation(), at_start, "reading never drains it");

    s.set_macros(MacroState {
        account: vec![macro_view("Ambush", "/cast Ambush")],
        character: Vec::new(),
    });
    let after_seed = s.macros_generation();
    assert_ne!(after_seed, at_start, "a seed changes the bar's icons too");

    s.run(r#"EditMacro(1, "Renamed", 1)"#).unwrap();
    assert_ne!(s.macros_generation(), after_seed);
}

/// In 1.12 the event is the whole mechanism; `ChatFrame1`'s registration is only the default UI's
/// use of it.
#[test]
fn a_registered_frame_sees_every_macro_line_as_an_event() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui();
    s.run(
        r#"MacroSpy = CreateFrame("Frame")
           MacroSpy.seen = {}
           MacroSpy:RegisterEvent("EXECUTE_CHAT_LINE")
           MacroSpy:SetScript("OnEvent", function(self, event, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9)
               table.insert(MacroSpy.seen, arg1)
           end)"#,
    )
    .unwrap();
    s.set_macros(MacroState {
        account: vec![macro_view("Pull", "/wave\n/say Incoming!")],
        character: Vec::new(),
    });

    assert!(super::run_macro(&mut s, 1));
    assert_eq!(
        s.eval::<(String, String, i64)>(
            "return MacroSpy.seen[1], MacroSpy.seen[2], table.getn(MacroSpy.seen)"
        )
        .unwrap(),
        ("/wave".into(), "/say Incoming!".into(), 2),
        "each line arrives as arg1 of its own event, in body order"
    );
    // The same lines still reach the chat frame.
    assert_eq!(
        s.take_chat_sends()
            .iter()
            .map(|c| c.text.clone())
            .collect::<Vec<_>>(),
        vec!["Incoming!".to_string()]
    );
    assert_eq!(s.take_emote_requests().len(), 1);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The reference's tokenizer splits on either `\r` or `\n`.
#[test]
fn either_line_ending_splits_a_body() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui();
    s.set_macros(MacroState {
        account: vec![macro_view("Mixed", "/say Charge\r/say a\r\n/say b")],
        character: Vec::new(),
    });
    assert!(super::run_macro(&mut s, 1));
    assert_eq!(
        s.take_chat_sends()
            .iter()
            .map(|c| c.text.clone())
            .collect::<Vec<_>>(),
        vec!["Charge".to_string(), "a".to_string(), "b".to_string()],
        "\r alone splits; \r\n does not leave an empty token"
    );
}

/// Every chooser icon resolves through the renderer's own [`benilla_assets::sprite_candidates`]:
/// one that does not draws as a white square. `Ability_Druid_Mangle.tga` resolves only through the
/// second `.blp` candidate, since `Ability_Druid_Mangle.tga.blp` is what ships.
#[test]
fn every_macro_chooser_icon_resolves_in_the_client_archives() {
    /// A stock 12340 install: `INV_Misc_QuestionMark`, then 1103 distinct `Spell_`/`Ability_` names
    /// under `Interface\Icons\` (the macro icon build, `0x565840`).
    const CHOOSER_ICONS_12340: usize = 1104;

    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let icons = benilla_formats::load_macro_icons(&mut chain).expect("load chooser catalog");
    assert_eq!(
        icons.len(),
        CHOOSER_ICONS_12340,
        "chooser catalog size moved — the enumeration or its filter changed"
    );

    let missing: Vec<String> = icons
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            !benilla_assets::sprite_candidates(p)
                .iter()
                .any(|c| chain.contains(c))
        })
        .map(|(i, p)| format!("#{} {p}", i + 1))
        .collect();
    assert!(
        missing.is_empty(),
        "chooser icons that resolve to nothing (each draws as a white square): {missing:#?}"
    );

    // The question mark first (the comparator pins it, `0x564a20`), then alphabetical: the
    // reference `qsort`s without case before deduping.
    assert!(icons[0].ends_with("INV_Misc_QuestionMark"), "{}", icons[0]);
    let mut sorted = icons[1..].to_vec();
    sorted.sort_by_key(|p| p.to_ascii_lowercase());
    assert_eq!(
        &icons[1..],
        &sorted[..],
        "the chooser list must be case-insensitively sorted"
    );
}
