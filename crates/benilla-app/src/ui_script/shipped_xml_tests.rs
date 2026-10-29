//! Whole-tree guards over the shipped interface: `assets/ui` and the manifest it loads.

/// Every shipped `assets/ui/*.xml` parses (XML forbids `--` inside a comment). Parse-only: a file
/// loaded out of manifest order reports templates its predecessors define.
#[test]
fn every_shipped_ui_xml_parses() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("assets/ui").flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "xml") {
            let text = std::fs::read_to_string(&path).expect("read");
            if let Err(e) = benilla_ui::framexml::parse(&text) {
                panic!("{}: {e}", path.display());
            }
            checked += 1;
        }
    }
    assert!(
        // A floor, not a census: a moved directory must not pass by sweeping nothing.
        checked >= 5,
        "only {checked} xml files swept — sweep broke"
    );
}

/// The whole manifest loads in its real order with zero loader errors, which the app only logs.
#[test]
fn the_whole_shipped_manifest_loads_without_errors() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, with the player and display a login seats.
    super::test_ui::seat_login(&mut s);
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Loading the UI makes no sound: a dropdown's OnLoad runs `UIDropDownMenu_Initialize`, which calls
/// its init at once (`UIDropDownMenu.lua`), and a unit popup's init reaches `UnitPopup_ShowMenu`
/// and its closing `PlaySound("igMainMenuOpen")` unless `UnitPopup_HideButtons` leaves only CANCEL
/// for a unit that does not exist (`UnitPopup.lua`).
#[test]
fn loading_the_shipped_ui_queues_no_sounds() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, with the player and display a login seats.
    super::test_ui::seat_login(&mut s);
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");
    s.resolve();
    assert_eq!(
        s.take_sounds(),
        vec![],
        "loading the UI played a sound — a load-time handler is ringing; see \
         UnitPopup_HideButtons / UIDropDownMenu_Initialize"
    );
}

/// The stock pet bar with the hunter fixture (Claw autocasting), settled past its slide.
fn pet_bar_vm() -> benilla_ui::script::UiScript {
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    super::pet_bar_tests::load_pet_bar(&mut s);
    super::pet_bar_tests::declare_token_strings(&s);
    s.set_pet_actions(true, true, true, super::pet_bar_tests::hunter_slots());
    s.fire_event("PET_BAR_UPDATE", vec![]);
    for _ in 0..3 {
        s.tick(0.05);
    }
    s.resolve();
    assert!(s.errors().is_empty(), "pet bar errors: {:?}", s.errors());
    s
}

/// The autocast brackets land on each button's corners: the art fills the middle 33/64 of
/// `UI-AutoCastableOverlay`, which puts the stock pet button's on its corners (58 × 33/64 = 29.9
/// on 30). Deviation: the spell book takes that ratio (`SpellBookAdapters.xml`), because its stock
/// brackets (60 × 33/64 = 30.9 on 37) sit three units inside each edge.
#[test]
fn the_autocast_brackets_reach_each_buttons_corners() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::framexml::{Element, TopLevel};

    /// The bracket art's share of `UI-AutoCastableOverlay.blp`, texels 15..47 of 64, measured with
    /// `benilla-extract blp`.
    const ART: f32 = 33.0 / 64.0;

    fn dim(el: &Element, tag: &str, axis: &str) -> Option<f32> {
        el.children
            .iter()
            .find(|c| c.tag.eq_ignore_ascii_case(tag))
            .and_then(|n| n.children.first())
            .and_then(|d| {
                d.attrs()
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(axis))
                    .map(|(_, v)| v.clone())
            })
            .and_then(|v| v.parse().ok())
    }

    /// Walk a template, carrying the nearest enclosing button size down to the overlay.
    fn walk(el: &Element, button: Option<f32>, out: &mut Vec<(String, f32, f32)>) {
        let button = dim(el, "Size", "x")
            .filter(|_| el.tag.ends_with("Button"))
            .or(button);
        let name = el
            .attrs()
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("name"))
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        if name.ends_with("AutoCastable") {
            if let (Some(size), Some(btn)) = (dim(el, "Size", "x"), button) {
                out.push((name.clone(), size * ART, btn));
            }
        }
        for child in &el.children {
            walk(child, button, out);
        }
    }

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("assets/ui") {
        let path = entry.expect("entry").path();
        if path.extension().is_none_or(|e| e != "xml") {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("read");
        for item in benilla_ui::framexml::parse(&src).expect("parse").items {
            match item {
                TopLevel::Font(el) | TopLevel::Template(el) | TopLevel::Instance(el) => {
                    walk(&el, None, &mut found)
                }
                TopLevel::Include(_) | TopLevel::Script(_) => {}
            }
        }
    }
    // The spell book's stock overlay is resized by a script, so it is measured off a VM.
    let s = super::spellbook_tests::spellbook_ui(1024.0, 768.0);
    let (overlay, button): (f32, f32) = s
        .eval("return SpellButton1AutoCastable:GetWidth(), SpellButton1:GetWidth()")
        .unwrap();
    found.push(("SpellButton1AutoCastable".into(), overlay * ART, button));
    // The pet button's is the stock template, measured the same way.
    let s = pet_bar_vm();
    let (overlay, button): (f32, f32) = s
        .eval("return PetActionButton1AutoCastable:GetWidth(), PetActionButton1:GetWidth()")
        .unwrap();
    found.push(("PetActionButton1AutoCastable".into(), overlay * ART, button));
    assert_eq!(found.len(), 2, "expected two autocast overlays: {found:?}");
    for (name, brackets, button) in &found {
        assert!(
            (brackets / button - 0.997).abs() < 0.01,
            "{name}: a {brackets:.1}-unit bracket square on a {button}-unit button is {:.3}x — the \
             pet button's is 0.997x, which is what puts brackets IN the corners",
            brackets / button
        );
    }
}

/// Every texture path the shipped UI names resolves in the archives, through the renderer's own
/// `sprite_candidates` and its `.blp`/`.tga` fallback: a miss draws as a plain white quad. The
/// shape half runs without client data.
#[test]
fn every_shipped_texture_path_resolves_in_the_client_archives() {
    use benilla_ui::framexml::{Element, TopLevel};

    // `file=` on an element is an archive path; `<Script file=>` and `<Include file=>` are
    // top-level items, not elements.
    fn walk(el: &Element, file: &str, out: &mut Vec<(String, String, String)>) {
        // `<Model file=>` names a model, never drawn as a texture.
        if el.tag.eq_ignore_ascii_case("Model") {
            return;
        }
        for (key, value) in el.attrs() {
            let archive_path = ["file", "bgfile", "edgefile"]
                .contains(&key.to_ascii_lowercase().as_str())
                && !value.is_empty();
            if archive_path {
                out.push((file.to_string(), el.tag.clone(), value.clone()));
            }
        }
        for child in &el.children {
            walk(child, file, out);
        }
    }

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut refs = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("assets/ui").flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "xml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read");
        let file = path.file_name().unwrap_or_default().to_string_lossy();
        let doc = benilla_ui::framexml::parse(&text).expect("parses");
        for item in &doc.items {
            match item {
                TopLevel::Font(el) | TopLevel::Template(el) | TopLevel::Instance(el) => {
                    walk(el, &file, &mut refs);
                }
                TopLevel::Include(_) | TopLevel::Script(_) => {}
            }
        }
    }
    // A floor, so the sweep cannot pass by matching nothing.
    assert!(refs.len() >= 15, "only {} texture paths swept", refs.len());

    // The shape half: a doubled separator is Lua escaping written into XML.
    for (file, tag, path) in &refs {
        assert!(
            !path.contains("\\\\"),
            "{file}: <{tag} file=\"{path}\"> has DOUBLED separators — XML attributes are not Lua \
             strings, so the backslashes stay doubled, the archive lookup misses, and the widget \
             draws as a white quad"
        );
    }

    let data = benilla_formats::wow_data_or_skip!();
    let chain = benilla_formats::open_chain(&data).expect("open chain");
    let missing: Vec<String> = refs
        .iter()
        .filter(|(_, _, path)| {
            !benilla_assets::sprite_candidates(path)
                .iter()
                .any(|c| chain.contains(c))
        })
        .map(|(file, tag, path)| format!("{file}: <{tag} file=\"{path}\">"))
        .collect();
    assert!(
        missing.is_empty(),
        "texture paths that resolve to nothing (each draws as a white quad): {missing:#?}"
    );
}

/// Every archive path a shipped Lua chunk names survives its own escaping: Lua silently drops the
/// backslash of an unknown escape, so `"Interface\P…"` arrives as `InterfaceP…` and the texture
/// never appears. The mirror of the XML check above, where a doubled separator is the bug.
#[test]
fn every_archive_path_a_shipped_lua_chunk_names_survives_its_own_escaping() {
    use benilla_ui::framexml::{Element, ScriptRef, TopLevel};

    // Archive roots a path literal starts with; other strings carry `\n` or `\124`, not paths.
    const ROOTS: [&str; 7] = [
        "interface",
        "textures",
        "world",
        "sound",
        "character",
        "item",
        "spells",
    ];

    /// Every quoted literal in one Lua chunk as raw source text, escapes uninterpreted; comments
    /// and long strings are skipped.
    fn literals(chunk: &str) -> Vec<(String, bool)> {
        let src: Vec<char> = chunk.chars().collect();
        let at = |i: usize, s: &str| src[i..].starts_with(&s.chars().collect::<Vec<_>>()[..]);
        let mut out = Vec::new();
        let mut i = 0;
        while i < src.len() {
            if at(i, "--") {
                i = if at(i + 2, "[[") {
                    src[i..]
                        .windows(2)
                        .position(|w| w == [']', ']'])
                        .map_or(src.len(), |k| i + k + 2)
                } else {
                    src[i..]
                        .iter()
                        .position(|&c| c == '\n')
                        .map_or(src.len(), |k| i + k + 1)
                };
                continue;
            }
            if at(i, "[[") {
                i = src[i + 2..]
                    .windows(2)
                    .position(|w| w == [']', ']'])
                    .map_or(src.len(), |k| i + 2 + k + 2);
                continue;
            }
            let quote = src[i];
            if quote != '"' && quote != '\'' {
                i += 1;
                continue;
            }
            let (mut raw, mut j, mut closed) = (String::new(), i + 1, false);
            while j < src.len() {
                let c = src[j];
                if c == '\\' && j + 1 < src.len() {
                    raw.push(c);
                    raw.push(src[j + 1]);
                    j += 2;
                    continue;
                }
                // A newline before the closing quote: not a literal (often an apostrophe).
                if c == '\n' {
                    break;
                }
                if c == quote {
                    closed = true;
                    break;
                }
                raw.push(c);
                j += 1;
            }
            if closed {
                // Joined by `..` on either side, or a `%` template: a fragment, not a whole path.
                let before = src[..i].iter().rposition(|c| !c.is_whitespace());
                let after = src[j + 1..].iter().position(|c| !c.is_whitespace());
                let joined = before.is_some_and(|k| k >= 1 && src[k] == '.' && src[k - 1] == '.')
                    || after.is_some_and(|k| {
                        src.get(j + 1 + k) == Some(&'.') && src.get(j + 2 + k) == Some(&'.')
                    });
                // A literal ending in a separator (a directory) or a dash (a family prefix,
                // `"…\\MageFire-" .. rank`) is joined elsewhere; no texture name ends in either.
                let fragment = raw.ends_with('\\') || raw.ends_with('-');
                let whole = !joined && !fragment && !raw.contains('%');
                out.push((raw, whole));
                i = j + 1;
            } else {
                i += 1;
            }
        }
        out
    }

    // Every Lua chunk (top-level `<Script>` blocks and handler bodies), through the parser: an
    // attribute value is also double-quoted, and a text scan cannot tell the two apart.
    fn bodies(el: &Element, out: &mut Vec<String>) {
        if !el.body.trim().is_empty() {
            out.push(el.body.clone());
        }
        for child in &el.children {
            bodies(child, out);
        }
    }

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut paths: Vec<(String, String, bool)> = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("assets/ui").flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "xml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read");
        let file = path.file_name().unwrap_or_default().to_string_lossy();
        let mut chunks = Vec::new();
        for item in &benilla_ui::framexml::parse(&text).expect("parses").items {
            match item {
                TopLevel::Script(ScriptRef::Inline { body, .. }) => chunks.push(body.clone()),
                TopLevel::Font(el) | TopLevel::Template(el) | TopLevel::Instance(el) => {
                    bodies(el, &mut chunks);
                }
                TopLevel::Script(ScriptRef::File(_)) | TopLevel::Include(_) => {}
            }
        }
        for (raw, whole) in chunks.iter().flat_map(|c| literals(c)) {
            let lower = raw.to_ascii_lowercase();
            if ROOTS.iter().any(|r| lower.starts_with(r)) && raw.contains('\\') {
                paths.push((file.to_string(), raw, whole));
            }
        }
    }
    // A floor, so the sweep cannot pass by matching nothing.
    assert!(
        !paths.is_empty(),
        "only {} archive paths swept out of the shipped Lua",
        paths.len()
    );

    // The shape half: with every `\\` pair collapsed, no backslash may remain.
    for (file, raw, _) in &paths {
        assert!(
            !raw.replace("\\\\", "").contains('\\'),
            "{file}: the Lua literal \"{raw}\" has SINGLE separators — Lua drops the backslash \
             from every one of them, so the path arrives with its folders run together, the \
             archive lookup misses, and the texture silently never appears. Double them."
        );
    }

    let data = benilla_formats::wow_data_or_skip!();
    let chain = benilla_formats::open_chain(&data).expect("open chain");
    let missing: Vec<String> = paths
        .iter()
        // Only whole paths under the two texture roots resolve through `sprite_candidates`.
        .filter(|(_, raw, whole)| {
            let lower = raw.to_ascii_lowercase();
            *whole && (lower.starts_with("interface") || lower.starts_with("textures"))
        })
        .filter(|(_, raw, _)| {
            let real = raw.replace("\\\\", "\\");
            !benilla_assets::sprite_candidates(&real)
                .iter()
                .any(|c| chain.contains(c))
        })
        .map(|(file, raw, _)| format!("{file}: \"{raw}\""))
        .collect();
    assert!(
        missing.is_empty(),
        "archive paths a shipped Lua chunk names that resolve to nothing: {missing:#?}"
    );
}

/// Every shipped `text=` answers against the real `GlobalStrings.lua`: `text=` is a lookup
/// (`FrameScript_GetText`) that shows the raw attribute only on a miss. A key-shaped value must
/// resolve, or it shows its own key; a literal must not collide with a global, or the lookup swaps
/// in that string.
#[test]
fn every_shipped_text_attribute_answers_against_the_real_global_strings() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let src = chain
        .read_file("Interface\\FrameXML\\GlobalStrings.lua")
        .expect("GlobalStrings.lua in the chain");
    let s = benilla_ui::script::UiScript::new().expect("VM");
    s.run(&String::from_utf8_lossy(&src)).expect("runs clean");

    // The loader's key-shape predicate, restated because it is private.
    let key_shaped = |v: &str| {
        v.len() >= 2
            && v.chars().any(|c| c.is_ascii_uppercase())
            && v.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    };

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut swept = 0;
    for entry in std::fs::read_dir(&dir).expect("assets/ui").flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "xml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read");
        let file = path.file_name().unwrap_or_default().to_string_lossy();
        for value in text
            .match_indices("text=\"")
            .filter_map(|(i, m)| text[i + m.len()..].split('"').next())
        {
            let resolved = s.lua().globals().get::<String>(value).ok();
            if key_shaped(value) {
                assert!(
                    resolved.is_some_and(|t| !t.is_empty()),
                    "{file}: text=\"{value}\" is shaped like a GlobalStrings key but the real \
                     GlobalStrings.lua has no such string — it would render as its own key name"
                );
            } else {
                assert!(
                    resolved.is_none(),
                    "{file}: the literal text=\"{value}\" collides with a real GlobalStrings key — \
                     the loader would silently show that string's value instead of these words"
                );
            }
            swept += 1;
        }
    }
    // A floor, so the sweep cannot pass by matching nothing.
    assert!(swept >= 1, "only {swept} text= values swept");
}

/// No shipped script hands a GlobalStrings key to a text sink as its words: `SetText` cannot tell
/// a key from a word, so `SetText("DEAD")` shows the key where the word is its value, "Dead"
/// (`GlobalStrings.lua`). The `text=` half is the sweep above.
#[test]
fn no_shipped_script_sets_a_global_string_key_as_display_text() {
    /// `loader::is_global_string_key`'s predicate, restated because it is private.
    fn key_shaped(s: &str) -> bool {
        s.len() >= 2
            && s.chars().any(|c| c.is_ascii_uppercase())
            && s.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    }

    // The string literal after each text sink's open paren on one line.
    fn sink_literals(line: &str) -> Vec<&str> {
        let mut out = Vec::new();
        // `SetFormattedText`'s first argument is its format, shown as is but for its tokens.
        for sink in [":SetText(", ":SetButtonText(", ":SetFormattedText("] {
            let mut rest = line;
            while let Some(at) = rest.find(sink) {
                rest = &rest[at + sink.len()..];
                let arg = rest.trim_start();
                if let Some(body) = arg.strip_prefix('"') {
                    if let Some(end) = body.find('"') {
                        out.push(&body[..end]);
                    }
                }
            }
        }
        out
    }

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut offenders: Vec<String> = Vec::new();
    let mut swept = 0;
    for entry in std::fs::read_dir(&dir).expect("assets/ui").flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "xml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read");
        let mut in_comment = false;
        for (n, raw) in text.lines().enumerate() {
            // Strip whatever of this line sits inside an XML comment, carrying the state over.
            let mut code = String::new();
            let mut cur = raw;
            loop {
                if in_comment {
                    match cur.find("-->") {
                        Some(at) => {
                            in_comment = false;
                            cur = &cur[at + 3..];
                        }
                        None => break,
                    }
                } else {
                    match cur.find("<!--") {
                        Some(at) => {
                            code.push_str(&cur[..at]);
                            in_comment = true;
                            cur = &cur[at + 4..];
                        }
                        None => {
                            code.push_str(cur);
                            break;
                        }
                    }
                }
            }
            if code.trim_start().starts_with("--") {
                continue; // a Lua comment line
            }
            for lit in sink_literals(&code) {
                if key_shaped(lit) {
                    offenders.push(format!(
                        "{}:{}: SetText(\"{lit}\") — a GlobalStrings KEY, not the word",
                        path.file_name().unwrap().to_string_lossy(),
                        n + 1
                    ));
                }
            }
        }
        swept += 1;
    }
    assert!(offenders.is_empty(), "{}", offenders.join("\n"));
    // A floor, so the sweep cannot pass by finding nothing.
    assert!(swept >= 5, "only {swept} xml files swept — sweep broke");
}

/// Every `$parentTextureFrame` outranks its unit frame's status bars by frame level: level is the
/// only draw-key term above the layer, which would otherwise lift an ARTWORK bar fill over
/// BACKGROUND art. The stock frames spend a level on it (`TargetFrame.lua`) or nest the art
/// in anonymous frames (`PlayerFrame.xml`).
#[test]
fn every_texture_frame_outranks_its_status_bars() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::order::unpack;

    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, with the player and display a login seats.
    super::test_ui::seat_login(&mut s);
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");
    // Every unit frame shown: a hidden frame emits no quads to read a level from.
    for unit in [
        "player",
        "target",
        "targettarget",
        "party1",
        "party2",
        "party3",
        "party4",
    ] {
        s.set_unit(
            unit,
            Some(benilla_ui::script::UnitState {
                exists: true,
                name: Some("Someone".into()),
                health: 60,
                max_health: 100,
                level: 60,
                power_type: 0,
                power: 60,
                max_power: 100,
                // Every unit carries a side; the PvP frame's `PLAYER_ENTERING_WORLD` reads ours.
                faction_group: Some("Alliance".into()),
                ..benilla_ui::script::UnitState::default()
            }),
        );
    }
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    s.fire_event("PARTY_MEMBERS_CHANGED", vec![]);
    // Target-of-target ships off (`SHOW_TARGET_OF_TARGET` default "0", `InterfaceOptionsFrame.lua`);
    // `TargetFrame_CreateTargetofTarget` builds it as `TargetFrameToT`.
    s.run(r#"SHOW_TARGET_OF_TARGET = "1""#).unwrap();
    s.run("TargetofTarget_Update(TargetFrameToT)").unwrap();
    s.resolve();

    // owner frame name → its (strata, level), read off the packed draw key the renderer sorts by.
    let mut level_of: std::collections::BTreeMap<String, (u8, u16)> =
        std::collections::BTreeMap::new();
    for q in s.extract() {
        if let Some(name) = s.quad_owner_name(q.target) {
            let p = unpack(q.z);
            level_of.insert(name, (p.strata, p.level));
        }
    }

    let mut checked = 0;
    for (texture_frame, &(tf_strata, tf_level)) in &level_of {
        let Some(base) = texture_frame.strip_suffix("TextureFrame") else {
            continue;
        };
        for suffix in ["HealthBar", "ManaBar"] {
            let bar = format!("{base}{suffix}");
            let Some(&(bar_strata, bar_level)) = level_of.get(&bar) else {
                continue;
            };
            assert_eq!(
                bar_strata, tf_strata,
                "{bar} and {texture_frame} must share a strata for the level to decide"
            );
            assert!(
                tf_level > bar_level,
                "{texture_frame} (level {tf_level}) must outrank {bar} (level {bar_level}): \
                 tied, the draw layer lifts the bar's ARTWORK fill over the frame's BACKGROUND art"
            );
            checked += 1;
        }
    }
    // The target template and its target-of-target name their TextureFrames (`TargetFrame.xml`);
    // the player, pet and party frames nest their art in anonymous frames instead.
    assert!(
        checked >= 4,
        "only {checked} texture-frame/bar pairs checked — the name sweep found nothing"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The shipped UI takes `VARIABLES_LOADED`, which the saved-variables load fires at every launch
/// before any window has shown, without a script error.
#[test]
fn the_shipped_ui_takes_variables_loaded_without_a_script_error() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, with the player and display a login seats.
    super::test_ui::seat_login(&mut s);
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "loader errors: {failures:?}");
    let _ = s.errors(); // drain the load's own errors
    s.fire_event("VARIABLES_LOADED", vec![]);
    assert!(
        s.errors().is_empty(),
        "VARIABLES_LOADED script errors: {:?}",
        s.errors()
    );
}

/// Every `<Font name=…>` the manifest declares is a `Font` global answering the FontInstance
/// getters, in the production order: `publish_global` never overwrites, so a same-named frame
/// loaded first would keep a font unpublished.
#[test]
fn every_shipped_font_object_is_published_as_a_lua_global() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, with the player and display a login seats.
    super::test_ui::seat_login(&mut s);
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "loader errors: {failures:?}");

    // The names come from the manifest's own entries, chain files included.
    let mut names: Vec<String> = Vec::new();
    for entry in &super::manifest::manifest_files() {
        if !entry.to_ascii_lowercase().ends_with(".xml") {
            continue;
        }
        let Some(bytes) = super::test_ui::read(&entry.replace('\\', "/")) else {
            continue;
        };
        let text = benilla_ui::source::decode(&bytes);
        for chunk in text.split("<Font ").skip(1) {
            if let Some(rest) = chunk.split_once("name=\"") {
                if let Some((name, _)) = rest.1.split_once('"') {
                    names.push(name.to_string());
                }
            }
        }
    }
    names.sort();
    names.dedup();
    assert!(
        names.len() >= 50,
        "only {} <Font name=> declarations found — the sweep broke",
        names.len()
    );

    let mut unpublished: Vec<&str> = Vec::new();
    for name in &names {
        match s.eval::<String>(&format!("return {name}:GetObjectType()")) {
            Ok(t) if t == "Font" => {}
            _ => unpublished.push(name),
        }
    }
    assert!(
        unpublished.is_empty(),
        "declared <Font name=> that is not a Font global: {unpublished:?}"
    );

    // The four font objects addons use most carry a face and a size, not just a name.
    for name in [
        "GameFontNormal",
        "GameTooltipText",
        "GameFontHighlightSmall",
        "GameTooltipHeaderText",
    ] {
        let (face, height) = s
            .eval::<(String, f32)>(&format!("return {name}:GetFont()"))
            .unwrap_or_else(|e| panic!("{name}:GetFont() — {e}"));
        assert!(
            face.to_ascii_uppercase().ends_with(".TTF"),
            "{name}: {face}"
        );
        assert!(height > 0.0, "{name}: height {height}");
    }
}

/// `TargetBuffButtonTemplate` (`TargetFrame.xml`, 21×21) and `MainMenuBarMicroButton`
/// (`MainMenuBarMicroButtons.xml`, 29×58) confer their size, regions and scripts, read back as a
/// consumer gets them: an unresolved `inherits=` fails silently.
#[test]
fn the_inheritable_reference_templates_confer_their_shape() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, with the player and display a login seats.
    super::test_ui::seat_login(&mut s);
    assert!(super::load_default_ui(&s).is_empty());

    s.run(
        r#"
        BuffProbe = CreateFrame("Frame", "BuffProbe", UIParent, "TargetBuffFrameTemplate")
        MicroProbe = CreateFrame("Button", "MicroProbe", UIParent, "MainMenuBarMicroButton")
        "#,
    )
    .unwrap();
    s.resolve();

    assert_eq!(
        s.eval::<(f64, f64)>("return BuffProbe:GetWidth(), BuffProbe:GetHeight()")
            .unwrap(),
        (21.0, 21.0),
        "the buff button carries the reference's 21x21"
    );
    assert!(
        s.eval::<bool>("return BuffProbeIcon ~= nil").unwrap(),
        "$parentIcon is the region a consumer getglobals to set the texture"
    );
    assert!(
        s.eval::<bool>(r#"return BuffProbe:GetScript("OnEnter") ~= nil"#)
            .unwrap(),
        "the tooltip script comes with the template"
    );

    assert_eq!(
        s.eval::<(f64, f64)>("return MicroProbe:GetWidth(), MicroProbe:GetHeight()")
            .unwrap(),
        (28.0, 58.0)
    );
    assert!(s
        .eval::<bool>(r#"return MicroProbe:GetScript("OnEnter") ~= nil"#)
        .unwrap());
    assert!(s.errors().is_empty(), "no script errors: {:?}", s.errors());
}

/// The stock inspect-cursor pair, `CursorUpdate` and `CursorOnUpdate` (`UIParent.lua`),
/// runs clean on the arms a frame without an item takes.
#[test]
fn the_inspect_cursor_pair_takes_both_arms() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, with the player and display a login seats.
    super::test_ui::seat_login(&mut s);
    assert!(super::load_default_ui(&s).is_empty());

    // Both exist as functions, the shape a caller checks before hooking.
    assert_eq!(
        s.eval::<(String, String)>("return type(CursorUpdate), type(CursorOnUpdate)")
            .unwrap(),
        ("function".into(), "function".into())
    );

    // A frame without `self.hasItem` takes the `ResetCursor` arm.
    s.run(
        r#"
        CursorProbe = CreateFrame("Frame", "CursorProbe", UIParent)
        CursorUpdate(CursorProbe)
        "#,
    )
    .unwrap();
    assert!(
        s.errors().is_empty(),
        "the ResetCursor arm runs clean: {:?}",
        s.errors()
    );

    // `CursorOnUpdate` does nothing unless the frame owns the tooltip.
    s.run("CursorOnUpdate(CursorProbe)").unwrap();
    assert!(
        s.errors().is_empty(),
        "the unowned-tooltip gate short-circuits: {:?}",
        s.errors()
    );
}

/// Nothing of the interface stays visible through a cinematic, swept over what is visible rather
/// than a list. The one survivor is `CinematicFrame`, which the reference declares with no parent
/// and `SetFullScreenFrame` shows as it hides `UIParent` (`UIParent.lua`).
#[test]
fn a_cinematic_leaves_nothing_of_the_interface_on_screen() {
    benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, with the player and display a login seats.
    super::test_ui::seat_login(&mut s);
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");
    s.resolve();

    // Our files' frames plus the stock HUD, which is most of what the cascade hides.
    let mut names = shipped_frame_names();
    names.extend(
        [
            "MainMenuBar",
            "ChatFrame1",
            "PlayerFrame",
            "MinimapCluster",
            "BuffFrame",
            "MainMenuBarBackpackButton",
            "CharacterMicroButton",
            "UIErrorsFrame",
            // The frame being shown, the one expected survivor.
            "CinematicFrame",
        ]
        .into_iter()
        .map(String::from),
    );
    let visible = |s: &benilla_ui::script::UiScript, n: &str| -> bool {
        s.eval::<i64>(&format!(
            "local f = getglobal(\"{n}\") \
             if not f or not f.IsVisible then return 0 end \
             return f:IsVisible() and 1 or 0"
        ))
        .unwrap_or(0)
            == 1
    };

    // The sweep is worth something only if there was something to hide.
    let before = names.iter().filter(|n| visible(&s, n)).count();
    assert!(
        // A floor over the named HUD, not over our files' census.
        before >= 6,
        "only {before} frames visible before the cinematic — the sweep found no interface to \
         hide, so it would pass no matter what the cascade did"
    );

    s.set_in_cinematic(true);
    s.fire_event("CINEMATIC_START", vec![]);
    s.resolve();

    let mut after: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|n| visible(&s, n))
        .collect();
    after.sort_unstable();
    assert_eq!(
        after,
        ["CinematicFrame"],
        "something is drawing over the fly-by (see this test's header for why exactly these \
         two are allowed to survive)"
    );

    // …and the player gets it all back.
    s.set_in_cinematic(false);
    s.fire_event("CINEMATIC_STOP", vec![]);
    s.resolve();
    let restored = names.iter().filter(|n| visible(&s, n)).count();
    assert_eq!(
        restored, before,
        "the interface comes back exactly as it was"
    );
}

/// Every named, non-virtual frame the shipped tree declares.
pub(super) fn shipped_frame_names() -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("assets/ui").flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|e| e == "xml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read");
        for chunk in text.split('<').skip(1) {
            let head = &chunk[..chunk.find('>').unwrap_or(chunk.len())];
            if head.contains("virtual=\"true\"") {
                continue;
            }
            let Some(i) = head.find("name=\"") else {
                continue;
            };
            let rest = &head[i + 6..];
            let Some(j) = rest.find('"') else { continue };
            let n = &rest[..j];
            // `$parent`-templated names are not globals; nothing can look them up by name.
            if !n.is_empty() && !n.contains('$') {
                names.push(n.to_string());
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// The macro icon picker opens off the shipped manifest, not a harness list: the on-demand macro
/// window's `MacroPopupScrollFrame` inherits a template only the manifest's
/// `ClassTrainerFrameTemplates.xml` row brings.
#[test]
fn the_shipped_manifest_opens_the_macro_icon_picker() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = benilla_ui::script::UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI loads on world entry, with the player and display a login seats.
    super::test_ui::seat_login(&mut s);
    // One saved macro, so "Change Name/Icon" has something selected.
    s.set_macros(benilla_ui::script::MacroState {
        account: vec![benilla_ui::script::MacroView {
            name: "die".into(),
            texture: Some(r"Interface\Icons\Ability_Ambush".into()),
            body: ".die".into(),
        }],
        character: Vec::new(),
    });
    s.set_macro_icons(vec![
        r"Interface\Icons\Ability_Ambush".into(),
        r"Interface\Icons\Ability_Backstab".into(),
    ]);
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");
    s.resolve();
    let _ = s.errors();

    // A LoadOnDemand addon, which `ShowMacroFrame` loads through the stock `MacroFrame_LoadUI`.
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_MacroUI");
    s.run("ShowMacroFrame()").unwrap();
    s.run("MacroButton1:Click()").unwrap();
    s.run("MacroEditButton:Click()").unwrap();
    assert!(
        s.errors().is_empty(),
        "opening the icon picker off the shipped manifest must not raise: {:?}",
        s.errors()
    );
    assert!(
        s.eval::<bool>("return MacroPopupFrame:IsShown() and true or false")
            .unwrap(),
        "the picker is up"
    );
}

/// benilla's own interface is one layer, and it does not grow: `layer.toc` names exactly these
/// files, all ours (none off the chain), and `assets/ui` holds only that manifest and its files.
/// The core loads every stock window the player's `FrameXML.toc` lists; what changes the stock UI
/// goes in one of these files. The glue screens load only the player's stock `GlueXML.toc`.
#[test]
fn the_layer_does_not_grow() {
    const LAYER: &[&str] = &[
        "ScrollTemplates.xml",
        "ContainerFrameAdapters.xml",
        "SpellBookAdapters.xml",
        "ScriptLogFrame.xml",
        "FrameXMLFixes.xml",
    ];
    assert_eq!(
        super::addons::Addon::layer().toc.files,
        LAYER,
        "layer.toc changed. The layer does not grow (docs/METHOD.md): a file that retired comes \
         off this list; a new one needs a reason this list can name."
    );
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let mut found: Vec<String> = std::fs::read_dir(&dir)
        .expect("assets/ui")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    found.sort();
    let mut shipped: Vec<String> = LAYER
        .iter()
        .chain(&["layer.toc"])
        .map(|s| s.to_string())
        .collect();
    shipped.sort();
    assert_eq!(
        found, shipped,
        "assets/ui holds the layer's manifest and its files, nothing else"
    );
}

/// The layer speaks the dialect of the FrameXML it loads over: a 3.3.5a handler body is compiled
/// under its parameter list (`self`, `delta`, `value`, ...), and the client sets `this` and `event`
/// around a handler but no `argN` global (no such name in the 12340 binary), so a body reads its
/// arguments, never `arg1`. The strata table has no `BLIZZARD`; `SetDrawLayer` takes a layer name
/// alone; and a button's state textures take a texture, never a colour.
#[test]
fn the_layer_speaks_the_3_3_5a_dialect() {
    fn handlers(el: &benilla_ui::framexml::Element, out: &mut Vec<(String, String)>) {
        if el.tag == "Scripts" {
            for h in &el.children {
                out.push((h.tag.clone(), h.body.clone()));
            }
        }
        for c in &el.children {
            handlers(c, out);
        }
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
    let files = super::addons::Addon::layer().toc.files;
    assert!(files.len() >= 5, "the layer's toc lists {files:?}");
    let word = |text: &str, w: &str| {
        text.match_indices(w).any(|(i, _)| {
            let before = text[..i].chars().next_back();
            let after = text[i + w.len()..].chars().next();
            !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
                && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
        })
    };
    for file in files {
        let text = std::fs::read_to_string(dir.join(&file)).expect("read");
        let doc = benilla_ui::framexml::parse(&text).expect("parses");
        let mut found = Vec::new();
        for item in &doc.items {
            use benilla_ui::framexml::TopLevel;
            if let TopLevel::Template(el) | TopLevel::Instance(el) = item {
                handlers(el, &mut found);
            }
        }
        for (tag, body) in found {
            for n in 1..=9 {
                let global = format!("arg{n}");
                assert!(
                    !word(&body, &global),
                    "{file}: <{tag}> reads `{global}`, which 3.3.5a never sets: {body}"
                );
            }
        }
        assert!(!text.contains("BLIZZARD"), "{file}: BLIZZARD strata");
        for line in text.lines().filter(|l| l.contains("SetDrawLayer(")) {
            assert_eq!(
                line.matches(',').count(),
                0,
                "{file}: SetDrawLayer takes the layer alone: {line}"
            );
        }
        for verb in [
            "SetNormalTexture(",
            "SetPushedTexture(",
            "SetHighlightTexture(",
            "SetDisabledTexture(",
            "SetCheckedTexture(",
            "SetDisabledCheckedTexture(",
        ] {
            for line in text.lines().filter(|l| l.contains(verb)) {
                let args = line.split(verb).nth(1).unwrap_or_default();
                assert!(
                    !args.trim_start().starts_with(|c: char| c.is_ascii_digit()),
                    "{file}: {verb} takes a texture, never a colour: {line}"
                );
            }
        }
    }
}
