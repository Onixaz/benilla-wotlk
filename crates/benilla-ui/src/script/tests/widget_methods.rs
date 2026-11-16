//! The widget methods still unbuilt (#1270): every method of a widget's class chain that 12340
//! registers and benilla's object lacks. The list is exact and only shrinks: building one fails this
//! gate until its row leaves the list, and a type that lacks a method the list does not name fails
//! it too.

use std::collections::{BTreeMap, BTreeSet};

/// Each class's registrar table in `reference/3.3.5a-shapes.tsv`.
const CLASS_TABLES: &[(&str, &str)] = &[
    ("ScriptObject", "t021"),
    ("Region", "t010"),
    ("Texture", "t008"),
    ("FontString", "t009"),
    ("Frame", "t011"),
    ("Font", "t012"),
    ("Animation", "t013"),
    ("Translation", "t014"),
    ("Rotation", "t015"),
    ("Scale", "t016"),
    ("ControlPoint", "t017"),
    ("Path", "t018"),
    ("Alpha", "t019"),
    ("AnimationGroup", "t020"),
    ("Minimap", "t049"),
    ("QuestPOIFrame", "t057"),
    ("PlayerModel", "t060"),
    ("DressUpModel", "t061"),
    ("TabardModel", "t063"),
    ("Cooldown", "t090"),
    ("GameTooltip", "t095"),
    ("Model", "t101"),
    ("MovieFrame", "t102"),
    ("ColorSelect", "t103"),
    ("StatusBar", "t104"),
    ("Slider", "t105"),
    ("ScrollFrame", "t106"),
    ("ScrollingMessageFrame", "t107"),
    ("MessageFrame", "t108"),
    ("SimpleHTML", "t109"),
    ("EditBox", "t110"),
    ("CheckButton", "t111"),
    ("Button", "t112"),
];

/// Every in-world widget type: its name, the Lua that makes one into `WM`, and its class chain.
const TYPES: &[(&str, &str, &[&str])] = &[
    // The font object's registrar fills its own table alone (`0x4a36f0`): no `GetParent`.
    ("Font", "WM = CreateFont('WMFont')", &["Font"]),
    (
        "Texture",
        "WM = CreateFrame('Frame'):CreateTexture()",
        &["ScriptObject", "Region", "Texture"],
    ),
    (
        "FontString",
        "WM = CreateFrame('Frame'):CreateFontString()",
        &["ScriptObject", "Region", "FontString"],
    ),
    (
        "AnimationGroup",
        "WM = CreateFrame('Frame'):CreateAnimationGroup()",
        &["ScriptObject", "AnimationGroup"],
    ),
    (
        "Animation",
        "WM = CreateFrame('Frame'):CreateAnimationGroup():CreateAnimation('Animation')",
        &["ScriptObject", "Animation"],
    ),
    (
        "Translation",
        "WM = CreateFrame('Frame'):CreateAnimationGroup():CreateAnimation('Translation')",
        &["ScriptObject", "Animation", "Translation"],
    ),
    (
        "Rotation",
        "WM = CreateFrame('Frame'):CreateAnimationGroup():CreateAnimation('Rotation')",
        &["ScriptObject", "Animation", "Rotation"],
    ),
    (
        "Scale",
        "WM = CreateFrame('Frame'):CreateAnimationGroup():CreateAnimation('Scale')",
        &["ScriptObject", "Animation", "Scale"],
    ),
    (
        "Alpha",
        "WM = CreateFrame('Frame'):CreateAnimationGroup():CreateAnimation('Alpha')",
        &["ScriptObject", "Animation", "Alpha"],
    ),
    (
        "Path",
        "WM = CreateFrame('Frame'):CreateAnimationGroup():CreateAnimation('Path')",
        &["ScriptObject", "Animation", "Path"],
    ),
    (
        "ControlPoint",
        "WM = CreateFrame('Frame'):CreateAnimationGroup():CreateAnimation('Path'):CreateControlPoint()",
        &["ScriptObject", "ControlPoint"],
    ),
    ("Frame", "", &[]),
    ("Button", "", &["Button"]),
    ("CheckButton", "", &["Button", "CheckButton"]),
    ("EditBox", "", &["EditBox"]),
    ("Slider", "", &["Slider"]),
    ("StatusBar", "", &["StatusBar"]),
    ("ScrollFrame", "", &["ScrollFrame"]),
    ("SimpleHTML", "", &["SimpleHTML"]),
    ("ColorSelect", "", &["ColorSelect"]),
    ("MessageFrame", "", &["MessageFrame"]),
    ("ScrollingMessageFrame", "", &["ScrollingMessageFrame"]),
    ("GameTooltip", "", &["GameTooltip"]),
    ("Minimap", "", &["Minimap"]),
    ("Cooldown", "", &["Cooldown"]),
    ("QuestPOIFrame", "", &["QuestPOIFrame"]),
    ("MovieFrame", "", &["MovieFrame"]),
    ("Model", "", &["Model"]),
    ("PlayerModel", "", &["Model", "PlayerModel"]),
    ("DressUpModel", "", &["Model", "PlayerModel", "DressUpModel"]),
    ("TabardModel", "", &["Model", "PlayerModel", "TabardModel"]),
];

/// `(type, method)` pairs of a type's class chain that benilla's object does not answer, byte
/// sorted by type then method.
const UNBUILT: &[(&str, &str)] = &[
    // #1026.
    ("GameTooltip", "SetFrameStack"),
    // #1233.
    ("ScrollingMessageFrame", "GetMessageInfo"),
    ("ScrollingMessageFrame", "RemoveMessagesByAccessID"),
];

/// Each class's method names from the shapes table.
fn class_methods() -> BTreeMap<&'static str, BTreeSet<String>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/3.3.5a-shapes.tsv"
    );
    let text = std::fs::read_to_string(path).expect("reference/3.3.5a-shapes.tsv");
    let mut out: BTreeMap<&'static str, BTreeSet<String>> = BTreeMap::new();
    for l in text.lines().filter(|l| !l.starts_with('#')) {
        let f: Vec<&str> = l.split('\t').collect();
        if f.len() < 3 || f[2] != "method" {
            continue;
        }
        if let Some((class, _)) = CLASS_TABLES.iter().find(|(_, t)| *t == f[1]) {
            out.entry(class).or_default().insert(f[0].to_string());
        }
    }
    out
}

/// Every `(type, method)` the type's class chain registers and benilla's object lacks.
fn missing() -> BTreeSet<(String, String)> {
    let classes = class_methods();
    let s = crate::script::UiScript::new().unwrap();
    let mut out = BTreeSet::new();
    for (ty, make, chain) in TYPES {
        // A frame type answers ScriptObject, Region and Frame before its own classes.
        let (make, chain): (String, Vec<&str>) = if make.is_empty() {
            (
                format!("WM = CreateFrame('{ty}')"),
                ["ScriptObject", "Region", "Frame"]
                    .into_iter()
                    .chain(chain.iter().copied())
                    .collect(),
            )
        } else {
            (make.to_string(), chain.to_vec())
        };
        s.run("WM = nil").unwrap();
        let made = s.run(&make).is_ok() && s.eval::<bool>("return WM ~= nil").unwrap_or(false);
        for class in chain {
            for m in classes.get(class).into_iter().flatten() {
                let has = made
                    && s.eval::<bool>(&format!("return type(WM.{m}) == 'function'"))
                        .unwrap_or(false);
                if !has {
                    out.insert((ty.to_string(), m.clone()));
                }
            }
        }
    }
    out
}

/// Every table the roster names is in the shapes table, with methods.
#[test]
fn every_class_table_has_methods() {
    let classes = class_methods();
    let empty: Vec<&str> = CLASS_TABLES
        .iter()
        .map(|(c, _)| *c)
        .filter(|c| classes.get(c).is_none_or(BTreeSet::is_empty))
        .collect();
    assert!(empty.is_empty(), "no method rows for {empty:?}");
}

/// The list is exactly what is missing: a built method leaves it, and a missing one joins nothing.
#[test]
fn the_unbuilt_list_is_exact() {
    let missing = missing();
    let listed: BTreeSet<(String, String)> = UNBUILT
        .iter()
        .map(|(t, m)| (t.to_string(), m.to_string()))
        .collect();
    let built: Vec<String> = listed
        .difference(&missing)
        .map(|(t, m)| format!("{t}:{m}"))
        .collect();
    let unlisted: Vec<String> = missing
        .difference(&listed)
        .map(|(t, m)| format!("{t}:{m}"))
        .collect();
    assert!(
        built.is_empty(),
        "built, but still listed in UNBUILT; remove each: {}",
        built.join(" ")
    );
    assert!(
        unlisted.is_empty(),
        "12340 registers these on the type's class chain and benilla's object lacks them; build \
         them (the list only shrinks):\n  {}",
        unlisted.join("\n  ")
    );
    let unordered: Vec<[(&str, &str); 2]> = UNBUILT
        .windows(2)
        .filter(|w| w[0] >= w[1])
        .map(|w| [w[0], w[1]])
        .collect();
    assert!(
        unordered.is_empty(),
        "out of byte order or repeated: {unordered:?}"
    );
}

/// The methods built for #1270, each as `(class, method, arguments)`: called on an object of that
/// class (a Region method on a Frame, a Texture and a FontString), its answer must fit the class
/// table's row. The arguments are a plausible call; `WM` is the receiver.
const SHAPED: &[(&str, &str, &str)] = &[
    ("Texture", "GetHorizTile", ""),
    ("Texture", "GetNonBlocking", ""),
    ("Texture", "GetVertTile", ""),
    ("Texture", "IsDesaturated", ""),
    ("Texture", "SetHorizTile", "1"),
    ("Texture", "SetNonBlocking", "1"),
    ("Texture", "SetVertTile", "1"),
    ("FontString", "CanWordWrap", ""),
    // 12340's swapped pair: the getter's name sets, the setter's name answers.
    ("FontString", "GetIndentedWordWrap", "1"),
    ("FontString", "GetStringHeight", ""),
    ("FontString", "SetIndentedWordWrap", ""),
    ("FontString", "SetWordWrap", "nil"),
    ("Region", "GetRect", ""),
    ("Region", "IsDragging", ""),
    ("Region", "IsMouseOver", "1, -1, -1, 1"),
    ("Frame", "GetDepth", ""),
    ("Frame", "GetDontSavePosition", ""),
    ("Frame", "GetEffectiveAlpha", ""),
    ("Frame", "GetEffectiveDepth", ""),
    ("Frame", "SetDepth", "2"),
    ("Frame", "SetDontSavePosition", "1"),
    ("Font", "GetIndentedWordWrap", ""),
    ("Font", "SetIndentedWordWrap", "1"),
    ("Animation", "SetParent", "WM:GetParent(), 2"),
    ("ControlPoint", "GetOffset", ""),
    ("ControlPoint", "GetOrder", ""),
    ("ControlPoint", "SetOffset", "3, 4"),
    ("ControlPoint", "SetOrder", "2"),
    ("ControlPoint", "SetParent", "WM:GetParent()"),
    ("Path", "CreateControlPoint", "'WMPoint', nil, 1"),
    ("Path", "GetControlPoints", ""),
    ("Path", "GetCurve", ""),
    ("Path", "GetMaxOrder", ""),
    ("Path", "SetCurve", "'smooth'"),
    (
        "Minimap",
        "SetBlipTexture",
        "'Interface\\\\Minimap\\\\ObjectIcons'",
    ),
    (
        "Minimap",
        "SetClassBlipTexture",
        "'Interface\\\\Minimap\\\\PartyRaidBlips'",
    ),
    ("Minimap", "SetCorpsePOIArrowTexture", "'X'"),
    ("Minimap", "SetIconTexture", "'X'"),
    ("Minimap", "SetPOIArrowTexture", "'X'"),
    ("Minimap", "SetPlayerTexture", "'X'"),
    ("Minimap", "SetStaticPOIArrowTexture", "'X'"),
    ("ColorSelect", "GetColorHSV", ""),
    ("ColorSelect", "SetColorHSV", "120, 0.5, 0.25"),
    ("StatusBar", "GetRotatesTexture", ""),
    ("StatusBar", "SetRotatesTexture", "1"),
    ("ScrollingMessageFrame", "GetCurrentLine", ""),
    ("ScrollingMessageFrame", "GetCurrentScroll", ""),
    ("ScrollingMessageFrame", "GetIndentedWordWrap", ""),
    ("ScrollingMessageFrame", "GetInsertMode", ""),
    ("ScrollingMessageFrame", "GetNumLinesDisplayed", ""),
    ("ScrollingMessageFrame", "SetIndentedWordWrap", "1"),
    ("ScrollingMessageFrame", "SetInsertMode", "'top'"),
    ("MessageFrame", "GetIndentedWordWrap", ""),
    ("MessageFrame", "SetIndentedWordWrap", "1"),
    ("SimpleHTML", "GetHyperlinksEnabled", ""),
    ("SimpleHTML", "GetIndentedWordWrap", "'H2'"),
    ("SimpleHTML", "SetHyperlinksEnabled", "nil"),
    ("SimpleHTML", "SetIndentedWordWrap", "'H2', 1"),
    ("EditBox", "GetIndentedWordWrap", ""),
    ("EditBox", "IsInIMECompositionMode", ""),
    ("EditBox", "SetIndentedWordWrap", "1"),
];

/// One `3.3.5a-shapes.tsv` row's return shape: the arities it allows (none when unknown) and its
/// kind alternatives (none when the trace is not `agree`).
fn row_shape(class: &str, method: &str) -> (Vec<usize>, Vec<Vec<String>>) {
    let table = CLASS_TABLES
        .iter()
        .find(|(c, _)| *c == class)
        .map(|(_, t)| *t)
        .expect("a mapped class");
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/3.3.5a-shapes.tsv"
    );
    let text = std::fs::read_to_string(path).expect("reference/3.3.5a-shapes.tsv");
    let f: Vec<&str> = text
        .lines()
        .map(|l| l.split('\t').collect::<Vec<_>>())
        .find(|f| f.len() > 8 && f[0] == method && f[1] == table && f[2] == "method")
        .unwrap_or_else(|| panic!("no row for {class}:{method}"));
    let arities = f[5].split('|').filter_map(|a| a.parse().ok()).collect();
    let kinds = if f[8] == "agree" {
        f[7].split('|')
            .map(|alt| {
                alt.trim()
                    .trim_start_matches('(')
                    .trim_end_matches(')')
                    .split(',')
                    .map(|k| k.trim().to_string())
                    .filter(|k| !k.is_empty())
                    .collect()
            })
            .collect()
    } else {
        Vec::new()
    };
    (arities, kinds)
}

/// Every method #1270 built answers its 12340 row's arity and, where the row's trace agrees, one
/// of its kind tuples; no play session reaches the addon-only ones.
#[test]
fn every_built_method_answers_its_reference_shape() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(
        "function WMKinds(...) local o = {} for i = 1, arg.n do o[i] = type(arg[i]) end \
         return table.concat(o, ',') end",
    )
    .unwrap();
    let mut wrong = Vec::new();
    for (class, method, args) in SHAPED {
        let receivers: &[&str] = if *class == "Region" {
            &["Frame", "Texture", "FontString"]
        } else {
            std::slice::from_ref(class)
        };
        let (arities, kinds) = row_shape(class, method);
        for ty in receivers {
            let make = TYPES
                .iter()
                .find(|(t, ..)| t == ty)
                .map(|(_, make, _)| {
                    if make.is_empty() {
                        format!("WM = CreateFrame('{ty}')")
                    } else {
                        make.to_string()
                    }
                })
                .unwrap();
            s.run(&make)
                .unwrap_or_else(|e| panic!("making a {ty}: {e}"));
            let got = s
                .eval::<String>(&format!("return WMKinds(WM:{method}({args}))"))
                .unwrap_or_else(|e| panic!("{ty}:{method}({args}): {e}"));
            let got: Vec<&str> = got.split(',').filter(|k| !k.is_empty()).collect();
            let arity_ok = arities.is_empty() || arities.contains(&got.len());
            let kinds_ok = kinds.is_empty()
                || kinds.iter().any(|alt| {
                    alt.len() == got.len()
                        && alt.iter().zip(&got).all(|(w, g)| match w.as_str() {
                            "any" | "value" => true,
                            "string?" => *g == "string" || *g == "nil",
                            w => w == *g,
                        })
                });
            if !arity_ok || !kinds_ok {
                wrong.push(format!(
                    "{ty}:{method}({args}) answers ({}), the row allows arity {arities:?} kinds {kinds:?}",
                    got.join(",")
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The `+0x160` flags round-trip, `horizTile` seeds its bit from the XML, and a tiled axis draws
/// at the rect's extent over the file's texel extent (`0x4847a0`).
#[test]
fn texture_tile_flags_round_trip_and_rescale_the_drawn_coordinates() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_texture_size_probe(Box::new(|p| (p == "Interface\\Tile").then_some((32, 16))));
    s.run(
        "F = CreateFrame('Frame', nil, UIParent) F:SetWidth(128) F:SetHeight(64) F:SetPoint('CENTER')
         T = F:CreateTexture() T:SetAllPoints(F) T:SetTexture('Interface\\\\Tile')",
    )
    .unwrap();
    assert!(s
        .eval::<bool>("return T:GetHorizTile() == nil and T:GetNonBlocking() == nil")
        .unwrap());
    s.run("T:SetHorizTile() T:SetNonBlocking('on')").unwrap();
    assert_eq!(s.eval::<i64>("return T:GetHorizTile()").unwrap(), 1);
    assert_eq!(s.eval::<i64>("return T:GetNonBlocking()").unwrap(), 1);
    assert!(s.eval::<bool>("return T:GetVertTile() == nil").unwrap());
    s.resolve();
    let tex = s
        .extract()
        .into_iter()
        .find_map(|q| match q.content {
            crate::script::QuadContent::Texture {
                path: Some(p),
                tex_coords,
                ..
            } if p == "Interface\\Tile" => Some(tex_coords),
            _ => None,
        })
        .expect("the tiled quad");
    // 128 units over 32 texels: four repeats across; the untiled axis keeps its 0..1.
    assert_eq!(
        tex,
        Some(crate::script::TexCoords::Rect([0.0, 4.0, 0.0, 1.0]))
    );
    s.run("T:SetHorizTile(nil)").unwrap();
    assert!(s.eval::<bool>("return T:GetHorizTile() == nil").unwrap());
    s.run("T:SetDesaturated(1)").unwrap();
    assert_eq!(s.eval::<i64>("return T:IsDesaturated()").unwrap(), 1);
}

/// 12340's FontString pair is registered swapped: `GetIndentedWordWrap` sets and
/// `SetIndentedWordWrap` reads, falling back to the font object's bit, which the font's own pair
/// carries. `SetWordWrap` clears `CanWordWrap`.
#[test]
fn the_word_wrap_flags_follow_the_registered_names() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(
        "WWFont = CreateFont('WWFont')
         F = CreateFrame('Frame') S = F:CreateFontString() S:SetFontObject(WWFont)",
    )
    .unwrap();
    assert_eq!(s.eval::<i64>("return S:CanWordWrap()").unwrap(), 1);
    s.run("S:SetWordWrap(false)").unwrap();
    assert!(s.eval::<bool>("return S:CanWordWrap() == nil").unwrap());
    assert!(s
        .eval::<bool>("return S:SetIndentedWordWrap() == nil")
        .unwrap());
    s.run("WWFont:SetIndentedWordWrap(true)").unwrap();
    assert_eq!(
        s.eval::<i64>("return WWFont:GetIndentedWordWrap()")
            .unwrap(),
        1
    );
    assert_eq!(
        s.eval::<i64>("return S:SetIndentedWordWrap()").unwrap(),
        1,
        "the string reads its font object's bit"
    );
    s.run("S:GetIndentedWordWrap(false)").unwrap();
    assert!(
        s.eval::<bool>("return S:SetIndentedWordWrap() == nil")
            .unwrap(),
        "its own set wins over the font's"
    );
    assert!(
        s.eval::<bool>("return WWFont.GetParent == nil").unwrap(),
        "a font object has no GetParent"
    );
}

/// Depth adds up the parent chain from the constructor's parent depth of 1; the effective alpha
/// is the byte product (`0x49f900`); the save flag round-trips.
#[test]
fn frame_depth_alpha_and_save_flag() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(
        "P = CreateFrame('Frame') C = CreateFrame('Frame', 'DepthChild', P)
         P:SetDepth(2) C:SetDepth('0.5')
         P:SetAlpha(0.5) C:SetAlpha(0.5)",
    )
    .unwrap();
    assert_eq!(s.eval::<f64>("return C:GetDepth()").unwrap(), 0.5);
    assert_eq!(s.eval::<f64>("return C:GetEffectiveDepth()").unwrap(), 3.5);
    assert_eq!(s.eval::<f64>("return P:GetEffectiveDepth()").unwrap(), 3.0);
    // 127 × 127 / 255 = 63, over 255.
    assert_eq!(
        s.eval::<f64>("return C:GetEffectiveAlpha()").unwrap(),
        63.0 / 255.0
    );
    let err = s.run("C:SetDepth('deep')").unwrap_err().to_string();
    assert!(
        err.contains("Usage: DepthChild:SetDepth(additiveDepth)"),
        "{err}"
    );
    assert!(s
        .eval::<bool>("return C:GetDontSavePosition() == nil")
        .unwrap());
    s.run("C:SetDontSavePosition()").unwrap();
    assert_eq!(s.eval::<i64>("return C:GetDontSavePosition()").unwrap(), 1);
}

/// HSV is the widget's own state: stored raw, read raw, and the set fires `OnColorSelect` with the
/// colour's RGB, as `SetColorRGB` does.
#[test]
fn color_select_hsv_round_trips_and_fires() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(
        "C = CreateFrame('ColorSelect')
         C:SetScript('OnColorSelect', function(self, r, g, b) FIRED = {r, g, b} end)
         C:SetColorHSV(120, 1, 1)",
    )
    .unwrap();
    assert_eq!(
        s.eval::<(f64, f64, f64)>("return C:GetColorHSV()").unwrap(),
        (120.0, 1.0, 1.0)
    );
    assert_eq!(
        s.eval::<(f64, f64, f64)>("return FIRED[1], FIRED[2], FIRED[3]")
            .unwrap(),
        (0.0, 1.0, 0.0)
    );
}

/// The bar's quarter turn maps its texture's corners, and turning it off restores the whole
/// texture.
#[test]
fn status_bar_rotates_its_texture() {
    let s = crate::script::UiScript::new().unwrap();
    s.run("B = CreateFrame('StatusBar') B:SetStatusBarTexture('Interface\\\\Bar')")
        .unwrap();
    assert!(s
        .eval::<bool>("return B:GetRotatesTexture() == nil")
        .unwrap());
    s.run("B:SetRotatesTexture(true)").unwrap();
    assert_eq!(s.eval::<i64>("return B:GetRotatesTexture()").unwrap(), 1);
    assert_eq!(
        s.eval::<Vec<f64>>("return {B:GetStatusBarTexture():GetTexCoord()}")
            .unwrap(),
        vec![0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0]
    );
    s.run("B:SetRotatesTexture()").unwrap();
    assert!(
        s.eval::<bool>("return B:GetRotatesTexture() == nil")
            .unwrap(),
        "an absent flag is off here: the default is 0"
    );
}

/// The ring head steps on each line and stays on a back-fill; the scroll counts back from it; the
/// insert mode is TOP or BOTTOM alone.
#[test]
fn scrolling_message_frame_ring_and_insert_mode() {
    let s = crate::script::UiScript::new().unwrap();
    s.run("M = CreateFrame('ScrollingMessageFrame', 'RingFrame') M:SetMaxLines(3)")
        .unwrap();
    assert_eq!(s.eval::<f64>("return M:GetCurrentLine()").unwrap(), -1.0);
    s.run("M:AddMessage('a') M:AddMessage('b')").unwrap();
    assert_eq!(s.eval::<f64>("return M:GetCurrentLine()").unwrap(), 1.0);
    s.run("M:AddMessage('old', 1, 1, 1, 0, true)").unwrap();
    assert_eq!(s.eval::<f64>("return M:GetCurrentLine()").unwrap(), 1.0);
    s.run("M:AddMessage('c') M:AddMessage('d')").unwrap();
    assert_eq!(s.eval::<f64>("return M:GetCurrentLine()").unwrap(), 0.0);
    s.run("M:ScrollUp()").unwrap();
    assert_eq!(s.eval::<f64>("return M:GetCurrentScroll()").unwrap(), 1.0);
    assert_eq!(
        s.eval::<String>("return M:GetInsertMode()").unwrap(),
        "BOTTOM"
    );
    s.run("M:SetInsertMode('top')").unwrap();
    assert_eq!(s.eval::<String>("return M:GetInsertMode()").unwrap(), "TOP");
    let err = s.run("M:SetInsertMode('MIDDLE')").unwrap_err().to_string();
    assert!(
        err.contains(r#"Usage: RingFrame:SetInsertMode("TOP" or "BOTTOM")"#),
        "{err}"
    );
    s.run("M:Clear()").unwrap();
    assert_eq!(s.eval::<f64>("return M:GetCurrentLine()").unwrap(), -1.0);
}

/// A SimpleHTML's word-wrap pair takes an optional element name first, one bit per element font;
/// its links are on from the constructor.
#[test]
fn simple_html_element_fonts_and_links() {
    let s = crate::script::UiScript::new().unwrap();
    s.run("H = CreateFrame('SimpleHTML') H:SetIndentedWordWrap('h2', true)")
        .unwrap();
    assert_eq!(
        s.eval::<i64>("return H:GetIndentedWordWrap('H2')").unwrap(),
        1
    );
    assert!(s
        .eval::<bool>("return H:GetIndentedWordWrap() == nil")
        .unwrap());
    assert_eq!(s.eval::<i64>("return H:GetHyperlinksEnabled()").unwrap(), 1);
    s.run("H:SetHyperlinksEnabled(0)").unwrap();
    assert!(s
        .eval::<bool>("return H:GetHyperlinksEnabled() == nil")
        .unwrap());
}

/// The minimap's art setters take a file name, with the reference's own usage and load texts.
#[test]
fn minimap_art_setters_check_their_file() {
    let mut s = crate::script::UiScript::new().unwrap();
    s.set_texture_size_probe(Box::new(|p| (p == "Interface\\Ok").then_some((8, 8))));
    s.run("MM = CreateFrame('Minimap', 'ArtMap') MM:SetPOIArrowTexture('Interface\\\\Ok')")
        .unwrap();
    let err = s.run("MM:SetPOIArrowTexture()").unwrap_err().to_string();
    assert!(
        err.contains(r#"Usage: ArtMap:SetPOITexture("file")"#),
        "{err}"
    );
    let err = s.run("MM:SetBlipTexture('')").unwrap_err().to_string();
    assert!(
        err.contains(r#"Usage: ArtMap:SetBlipTexture("file")"#),
        "{err}"
    );
    let err = s
        .run("MM:SetCorpsePOIArrowTexture('Interface\\\\Missing')")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("ArtMap:SetCorspePOITexture(): Couldn't load the file Interface\\Missing"),
        "{err}"
    );
}

/// `SetParent` moves an animation to another group; a Path's points keep their order and offset
/// and move between Paths the same way.
#[test]
fn animations_and_control_points_change_parent() {
    let s = crate::script::UiScript::new().unwrap();
    s.run(
        "F = CreateFrame('Frame') G1 = F:CreateAnimationGroup('MoveFrom')
         G2 = F:CreateAnimationGroup('MoveTo')
         A = G1:CreateAnimation('Alpha') A:SetDuration(2)
         A:SetParent('MoveTo')
         P = G2:CreateAnimation('Path') Q = G2:CreateAnimation('Path', 'OtherPath')
         C1 = P:CreateControlPoint() C2 = P:CreateControlPoint(nil, nil, 3)
         C1:SetOffset(5, -2)",
    )
    .unwrap();
    assert!(s.eval::<bool>("return A:GetParent() == G2").unwrap());
    assert_eq!(s.eval::<f64>("return G2:GetDuration()").unwrap(), 2.0);
    assert_eq!(s.eval::<f64>("return G1:GetDuration()").unwrap(), 0.0);
    let err = s.run("A:SetParent(nil)").unwrap_err().to_string();
    assert!(
        err.contains("SetParent(): Cannot set a 'nil' parent for animations"),
        "{err}"
    );
    let err = s.run("A:SetParent(F)").unwrap_err().to_string();
    assert!(
        err.contains("Wrong parent object type, expected AnimationGroup"),
        "{err}"
    );
    // An empty Path's first point takes order 0 (no step past the highest), shown as 1.
    assert_eq!(s.eval::<i64>("return C1:GetOrder()").unwrap(), 1);
    assert_eq!(s.eval::<i64>("return C2:GetOrder()").unwrap(), 3);
    assert_eq!(s.eval::<f64>("return P:GetMaxOrder()").unwrap(), 3.0);
    assert_eq!(
        s.eval::<(f64, f64)>("return C1:GetOffset()").unwrap(),
        (5.0, -2.0)
    );
    assert!(s
        .eval::<bool>("local a, b = P:GetControlPoints() return a == C1 and b == C2")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return C1:GetObjectType()").unwrap(),
        "ControlPoint"
    );
    s.run("C2:SetParent('OtherPath')").unwrap();
    assert!(s
        .eval::<bool>("return C2:GetParent() == Q and select('#', P:GetControlPoints()) == 1")
        .unwrap());
    assert_eq!(s.eval::<String>("return P:GetCurve()").unwrap(), "NONE");
    s.run("P:SetCurve('smooth')").unwrap();
    assert_eq!(s.eval::<String>("return P:GetCurve()").unwrap(), "SMOOTH");
    let err = s.run("P:SetCurve('wavy')").unwrap_err().to_string();
    assert!(
        err.contains("SetCurve(): Unknown curve type specified"),
        "{err}"
    );
}
