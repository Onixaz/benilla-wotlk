//! The glue screens' Lua surface, held to the reference's registrar tables in
//! `reference/3.3.5a-shapes.tsv`: every function the glue VM ([`UiScript::new_glue`]) puts in `_G`
//! is one the glue state has, and every glue-state function is there or on [`NOT_YET`]. Then the
//! glue state's frame types and event table.

use std::collections::{BTreeMap, BTreeSet};

use crate::script::{UiScript, GLUE_EVENTS};
use crate::widget::KindState;

/// A vendored `reference/` table's data rows, split on tabs, the header row dropped.
fn reference_rows(file: &str, header: &str) -> Vec<Vec<String>> {
    let path = format!("{}/../../reference/{file}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| l.split('\t').map(str::to_string).collect::<Vec<_>>())
        .filter(|f| f[0] != header)
        .collect()
}

/// The tables FrameScript's Lua init (`FrameScript_Initialize`, `0x819bb0`) registers into `_G`:
/// the Lua base library and the base table `0xa443a8`.
const BASE_TABLES: [&str; 2] = ["0xa47c80", "0xa443a8"];

/// The registrar tables the glue manager's `Resume` (`0x4da5f0`) shares with the in-game UI:
/// system (`0x60a120`), frame script (`0x81b870`), sound (`0x985ea0`), account messages
/// (`0x550080`) and video options (`0x54ec00`).
const SHARED_TABLES: [&str; 5] = ["0xad2184", "0xaf5848", "0xb2d928", "0xacd418", "0xacd320"];

/// The glue-only registrar tables and their row counts: the glue core (`0x4dd580`), character
/// select (`0x4e2570`), character create (`0x4dfd00`) and the realm list (`0x4de4d0`).
const GLUE_TABLES: [(&str, usize); 4] = [
    ("0xac3e00", 113),
    ("0xac4378", 13),
    ("0xac4228", 32),
    ("0xac4190", 14),
];

/// The global function aliases of the table, math and string libraries the compat chunk
/// FrameScript's Lua init runs defines (`0xaf57f0`).
const COMPAT: [&str; 42] = [
    "foreach", "foreachi", "getn", "tinsert", "tremove", "sort", "wipe", "abs", "acos", "asin",
    "atan", "atan2", "ceil", "cos", "deg", "exp", "floor", "frexp", "ldexp", "log", "log10", "max",
    "min", "mod", "rad", "random", "sin", "sqrt", "tan", "strbyte", "strchar", "strfind", "format",
    "gmatch", "gsub", "strlen", "strlower", "strmatch", "strrep", "strrev", "strsub", "strupper",
];

/// Base-library globals Lua 5.1's `luaopen_base` sets outside its registered table.
const LUA_BASE_EXTRA: [&str; 4] = ["ipairs", "pairs", "newproxy", "print"];

/// Globals the glue VM exposes past the reference's glue state, each with its reason: benilla's
/// stdlib layer is shared with the in-game VM, which carries the same names
/// (`reference_surface::BEYOND_REFERENCE`).
const BEYOND: &[&str] = &[];

/// Every function name the glue state has, and the glue tables' names alone.
fn glue_state_names() -> (BTreeSet<String>, BTreeSet<String>) {
    let shapes = reference_rows("3.3.5a-shapes.tsv", "name");
    let mut allowed: BTreeSet<String> = BTreeSet::new();
    let mut glue: BTreeSet<String> = BTreeSet::new();
    for f in &shapes {
        let table = f[3].as_str();
        if BASE_TABLES.contains(&table) || SHARED_TABLES.contains(&table) {
            allowed.insert(f[0].clone());
        }
        if GLUE_TABLES.iter().any(|(t, _)| *t == table) {
            allowed.insert(f[0].clone());
            glue.insert(f[0].clone());
        }
    }
    for n in COMPAT.iter().chain(&LUA_BASE_EXTRA) {
        allowed.insert(n.to_string());
    }
    (allowed, glue)
}

/// Each glue registrar table has the row count its registrar loops.
#[test]
fn the_glue_tables_hold_their_rows() {
    let shapes = reference_rows("3.3.5a-shapes.tsv", "name");
    for (table, rows) in GLUE_TABLES {
        let n = shapes.iter().filter(|f| f[3] == table).count();
        assert_eq!(n, rows, "table {table}");
    }
}

/// Glue-table functions the glue VM does not answer yet, by registrar table. The list only
/// shrinks: a verb is absent until built, never stubbed.
const NOT_YET: &[(&str, &[&str])] = &[
    (
        "the character create screen (table 0xac4228) and its paid-service variants",
        &[
            "CreateCharacter",
            "CustomizeExistingCharacter",
            "CycleCharCustomization",
            "GetAvailableClasses",
            "GetAvailableRaces",
            "GetCharacterCreateFacing",
            "GetClassesForRace",
            "GetCreateBackgroundModel",
            "GetFacialHairCustomization",
            "GetFactionForRace",
            "GetHairCustomization",
            "GetNameForRace",
            "GetRandomName",
            "GetSelectedClass",
            "GetSelectedRace",
            "GetSelectedSex",
            "IsRaceClassRestricted",
            "IsRaceClassValid",
            "PaidChange_GetCurrentClassIndex",
            "PaidChange_GetCurrentRaceIndex",
            "PaidChange_GetName",
            "PaidChange_GetPreviousRaceIndex",
            "RandomizeCharCustomization",
            "ResetCharCustomize",
            "SetCharCustomizeBackground",
            "SetCharacterCreateFacing",
            "SetSelectedClass",
            "SetSelectedRace",
            "SetSelectedSex",
            "UpdateCustomizationBackground",
            "UpdateCustomizationScene",
        ],
    ),
    (
        "character select's delete, rename and declension (table 0xac4378)",
        &["DeleteCharacter", "RenameCharacter", "DeclineCharacter"],
    ),
    (
        "the AddOns screen's per-character enable state (table 0xac3e00)",
        &[
            "DisableAddOn",
            "DisableAllAddOns",
            "EnableAddOn",
            "EnableAllAddOns",
            "GetAddOnEnableState",
            "LaunchAddOnURL",
            "ResetAddOns",
            "SaveAddOns",
        ],
    ),
    (
        "the glue's credits text and its options defaults (table 0xac3e00)",
        &[
            "GetCreditsText",
            "RestoreVideoEffectsDefaults",
            "RestoreVideoResolutionDefaults",
            "RestoreVideoStereoDefaults",
        ],
    ),
];

/// The glue VM's globals by Lua type name.
fn glue_globals() -> BTreeMap<String, String> {
    UiScript::new_glue()
        .expect("construct the glue VM")
        .eval::<Vec<Vec<String>>>(
            "local out = {} \
             for k, v in pairs(getfenv(0)) do \
               if type(k) == 'string' then table.insert(out, {k, type(v)}) end \
             end \
             return out",
        )
        .expect("dump _G")
        .into_iter()
        .map(|kv| (kv[0].clone(), kv[1].clone()))
        .collect()
}

/// The glue VM's functions are glue-state ones, and every glue-table function is built or on
/// [`NOT_YET`].
#[test]
fn the_glue_vm_answers_the_glue_surface() {
    let (allowed, glue) = glue_state_names();
    let not_yet: BTreeSet<&str> = NOT_YET
        .iter()
        .flat_map(|(_, n)| n.iter().copied())
        .collect();
    let ours: BTreeSet<String> = glue_globals()
        .into_iter()
        .filter(|(_, t)| t == "function")
        .map(|(k, _)| k)
        .collect();

    let beyond: Vec<&String> = ours
        .iter()
        .filter(|n| !allowed.contains(*n) && !BEYOND.contains(&n.as_str()))
        .collect();
    assert!(
        beyond.is_empty(),
        "the glue VM exposes {} function(s) the glue state does not have: {beyond:?}. The \
         in-game surface stays out of it (`UiScript::new` installs that).",
        beyond.len()
    );
    let missing: Vec<&String> = glue
        .iter()
        .filter(|n| !ours.contains(*n) && !not_yet.contains(n.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "glue-table functions the glue VM lacks and NOT_YET does not name: {missing:?}"
    );
    let stale: Vec<&&str> = not_yet
        .iter()
        .filter(|n| ours.contains(**n) || !glue.contains(**n))
        .collect();
    assert!(
        stale.is_empty(),
        "NOT_YET names that are built, or that are not glue-table functions; take them off: \
         {stale:?}"
    );
}

/// The glue event table in code is `reference/3.3.5a-glue-events.tsv`, by id.
#[test]
fn the_glue_event_table_matches_the_reference() {
    let rows = reference_rows("3.3.5a-glue-events.tsv", "eventId");
    assert_eq!(rows.len(), GLUE_EVENTS.len());
    for (id, row) in rows.iter().enumerate() {
        assert_eq!(row[0], id.to_string(), "ids run 0..41 in order");
        assert_eq!(row[1], GLUE_EVENTS[id], "event {id}");
    }
}

/// `RegisterEvent` matches a name against the glue table case-insensitively and drops one the
/// table lacks without raising; `UnregisterEvent` matches the same way. An in-game event fired at
/// the glue VM reaches nothing, an all-events frame included, since the reference fires by id.
#[test]
fn glue_events_register_by_the_glue_table() {
    let mut s = UiScript::new_glue().unwrap();
    s.run(
        r#"
        heard = {}
        f = CreateFrame("Frame")
        f:SetScript("OnEvent", function() table.insert(heard, "f:" .. event) end)
        f:RegisterEvent("set_glue_screen")
        f:RegisterEvent("PLAYER_LOGIN")
        all = CreateFrame("Frame")
        all:SetScript("OnEvent", function() table.insert(heard, "all:" .. event) end)
        all:RegisterAllEvents()
        "#,
    )
    .expect("an event off the glue table raises nothing");
    s.fire_event("SET_GLUE_SCREEN", vec![]);
    s.queue_event("PLAYER_LOGIN", vec![]);
    s.tick(0.016);
    assert_eq!(
        s.eval::<Vec<String>>("return heard").unwrap(),
        ["f:SET_GLUE_SCREEN", "all:SET_GLUE_SCREEN"],
        "the glue event reaches both frames under the table's spelling; the in-game one neither"
    );
    s.run(r#"heard = {} f:UnregisterEvent("Set_Glue_Screen")"#)
        .unwrap();
    s.fire_event("SET_GLUE_SCREEN", vec![]);
    assert_eq!(
        s.eval::<Vec<String>>("return heard").unwrap(),
        ["all:SET_GLUE_SCREEN"]
    );
}

/// The in-game VM keeps its by-name registry: an in-game event registers and fires there.
#[test]
fn the_in_game_vm_keeps_its_events() {
    let mut s = UiScript::new().unwrap();
    s.run(
        r#"heard = 0 f = CreateFrame("Frame") f:RegisterEvent("PLAYER_LOGIN")
           f:SetScript("OnEvent", function() heard = heard + 1 end)"#,
    )
    .unwrap();
    s.fire_event("PLAYER_LOGIN", vec![]);
    assert_eq!(s.eval::<i64>("return heard").unwrap(), 1);
}

/// The frame types: the 13 registered at startup (`FrameXML_RegisterDefault`, `0x813df0`) and
/// `ModelFFX` (the glue manager's init, `0x4db9f0`) in both states;
/// `CGWorldFrame::RegisterFrames`' types (`0x521040`) only in-game.
#[test]
fn the_glue_vm_makes_the_startup_frame_types_only() {
    const STARTUP: [&str; 14] = [
        "Button",
        "CheckButton",
        "EditBox",
        "Frame",
        "MessageFrame",
        "Model",
        "ScrollFrame",
        "ScrollingMessageFrame",
        "Slider",
        "SimpleHTML",
        "StatusBar",
        "ColorSelect",
        "MovieFrame",
        "ModelFFX",
    ];
    const IN_GAME: [&str; 7] = [
        "GameTooltip",
        "Cooldown",
        "Minimap",
        "PlayerModel",
        "DressUpModel",
        "TabardModel",
        "QuestPOIFrame",
    ];
    let glue = UiScript::new_glue().unwrap();
    let world = UiScript::new().unwrap();
    for t in STARTUP {
        for (vm, s) in [("glue", &glue), ("in-game", &world)] {
            s.run(&format!(r#"CreateFrame("{t}")"#))
                .unwrap_or_else(|e| panic!("{vm} CreateFrame(\"{t}\"): {e}"));
        }
    }
    for t in IN_GAME {
        glue.run(&format!(r#"CreateFrame("{t}")"#)).expect_err(t);
        world.run(&format!(r#"CreateFrame("{t}")"#)).unwrap();
    }
}

/// The glue's screen verbs answer the screen root with no UI scale (`GetScreenWidth`
/// `0x4dcc20`, `GetScreenHeight` `0x4dcc70`), even when a frame is named `UIParent`; the cursor
/// is the root's (`0x4dcb60`).
#[test]
fn the_glue_screen_verbs_answer_the_root_unscaled() {
    let root = (768.0_f32 * 16.0 / 9.0, 768.0_f32);
    let mut s = UiScript::new_glue().unwrap();
    s.set_screen_size(root.0, root.1);
    s.run(r#"local p = CreateFrame("Frame", "UIParent") p:SetScale(0.5)"#)
        .unwrap();
    s.mouse_move(100.0, 200.0);
    assert_eq!(
        s.eval::<(f32, f32, f32, f32)>(
            "local x, y = GetCursorPosition() return GetScreenWidth(), GetScreenHeight(), x, y",
        )
        .unwrap(),
        (root.0, root.1, 100.0, 200.0)
    );
}

/// `ModelFFX` is a `Model` to Lua (`0x470030`, `0x46ffd0`) with `Model`'s methods, flagged FFX for
/// the renderer; its XML is `Model`'s (`0x76cac0`), `file=` and `scale=` included.
#[test]
fn model_ffx_is_a_model_flagged_ffx() {
    let s = UiScript::new_glue().unwrap();
    s.run(
        r#"
        a = CreateFrame("ModelFFX", "FfxPane")
        b = CreateFrame("Model", "PlainPane")
        a:SetModel("Test/Scene.mdx")
        "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<(String, bool, bool, String)>(
            r#"return a:GetObjectType(), a:IsObjectType("Model") == 1,
                      a:IsObjectType("ModelFFX") == 1, a:GetModel()"#
        )
        .unwrap(),
        (
            "Model".to_string(),
            true,
            false,
            "Test/Scene.mdx".to_string()
        )
    );
    let ffx = |name: &str| {
        let model = s.model_ref();
        let h = model.arena.lookup(name).unwrap();
        match &model.arena.frame(h).unwrap().kind_state {
            KindState::Model(m) => m.ffx,
            other => panic!("{name}: {other:?}"),
        }
    };
    assert!(ffx("FfxPane"));
    assert!(!ffx("PlainPane"));

    let doc = crate::framexml::parse(
        r#"<Ui>
            <ModelFFX name="XmlFfx" file="Test/Xml.mdx" scale="2"/>
            <PlayerModel name="XmlPlayer"/>
        </Ui>"#,
    )
    .unwrap();
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w == "Unknown frame type: PlayerModel"),
        "the glue state has no PlayerModel factory: {:?}",
        report.warnings
    );
    assert!(ffx("XmlFfx"));
    assert_eq!(
        s.eval::<(String, f64)>("return XmlFfx:GetModel(), XmlFfx:GetModelScale()")
            .unwrap(),
        ("Test/Xml.mdx".to_string(), 2.0)
    );
}
