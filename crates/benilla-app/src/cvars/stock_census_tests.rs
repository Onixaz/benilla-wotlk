//! Every CVar the stock 3.3.5a interface names is a row (#212). The names are read off the player's
//! own chain at test time, so nothing from the client is committed.

use std::collections::BTreeMap;

use super::registered_rows;

/// Names the stock interface uses that the Windows 12340 client never registers, each with why.
const NOT_REGISTERED: &[(&str, &str)] = &[
    (
        "iTunesRemoteFeedback",
        "MacOptionsFrame.lua: the Mac client's",
    ),
    (
        "iTunesTrackDisplay",
        "MacOptionsFrame.lua: the Mac client's",
    ),
    (
        "MovieRecordingAutoCompress",
        "MacOptionsFrame.lua: the Mac client's",
    ),
    (
        "MovieRecordingCompression",
        "MacOptionsFrame.lua: the Mac client's",
    ),
    (
        "MovieRecordingCursor",
        "MacOptionsFrame.lua: the Mac client's",
    ),
    (
        "MovieRecordingFramerate",
        "MacOptionsFrame.lua: the Mac client's",
    ),
    ("MovieRecordingGUI", "MacOptionsFrame.lua: the Mac client's"),
    (
        "MovieRecordingIcon",
        "MacOptionsFrame.lua: the Mac client's",
    ),
    (
        "MovieRecordingQuality",
        "MacOptionsFrame.lua: the Mac client's",
    ),
    (
        "MovieRecordingRecover",
        "MacOptionsFrame.lua: the Mac client's",
    ),
    (
        "MovieRecordingSound",
        "MacOptionsFrame.lua: the Mac client's",
    ),
    (
        "MovieRecordingWidth",
        "MacOptionsFrame.lua: the Mac client's",
    ),
    (
        "lfgDebug",
        "LFGFrame.lua's developer switch, nil on a retail client",
    ),
];

/// Each CVar name the stock interface spells out, by lowercased name, with the files naming it:
/// the first argument of a CVar verb, a `cvar = "…"` field, or a `CVAR_… = "…"` local.
fn stock_cvar_names(data: &std::path::Path) -> BTreeMap<String, (String, Vec<String>)> {
    let pattern = regex::Regex::new(
        r#"(?:\b(?:GetCVar|GetCVarBool|SetCVar|GetCVarDefault|GetCVarInfo|GetCVarMin|GetCVarMax|BlizzardOptionsPanel_(?:Get|Set)CVar(?:Default|Min|Max)?Safe)\s*\(\s*|\bcvar\s*=\s*|\bCVAR_\w+\s*=\s*)"([^"]+)""#,
    )
    .expect("the pattern compiles");
    let chain = benilla_formats::open_chain(data).expect("open the install's MPQ chain");
    let mut names: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    for entry in chain.list().expect("list the chain") {
        let lower = entry.name.to_ascii_lowercase().replace('/', "\\");
        let stock = [
            "interface\\framexml\\",
            "interface\\gluexml\\",
            "interface\\addons\\blizzard_",
        ]
        .iter()
        .any(|root| lower.starts_with(root));
        if !stock || !(lower.ends_with(".lua") || lower.ends_with(".xml")) {
            continue;
        }
        let bytes = chain.read(&entry.name).expect("a listed file reads");
        let text = String::from_utf8_lossy(&bytes);
        for cap in pattern.captures_iter(&text) {
            let name = &cap[1];
            let row = names
                .entry(name.to_ascii_lowercase())
                .or_insert_with(|| (name.to_string(), Vec::new()));
            if !row.1.contains(&entry.name) {
                row.1.push(entry.name.clone());
            }
        }
    }
    names
}

#[test]
fn every_cvar_the_stock_interface_names_is_registered() {
    let data = benilla_formats::wow_data_or_skip!();
    let names = stock_cvar_names(&data);
    // The run is valid: 3.3.5a's interface names about 250.
    assert!(names.len() > 200, "only {} names found", names.len());
    let registered = |key: &str| registered_rows().any(|r| r.name.eq_ignore_ascii_case(key));
    let exempt = |key: &str| {
        NOT_REGISTERED
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(key))
    };
    let missing: Vec<_> = names
        .iter()
        .filter(|(key, _)| !registered(key) && !exempt(key))
        .map(|(_, (name, files))| format!("{name} ({})", files.join(", ")))
        .collect();
    assert!(
        missing.is_empty(),
        "stock CVars with no row (GetCVar answers nil): {missing:#?}"
    );
    for (name, why) in NOT_REGISTERED {
        let key = name.to_ascii_lowercase();
        assert!(
            names.contains_key(&key),
            "{name} ({why}): no stock file names it now"
        );
        assert!(
            !registered(&key),
            "{name} ({why}): registered, so drop it from this list"
        );
    }
}
