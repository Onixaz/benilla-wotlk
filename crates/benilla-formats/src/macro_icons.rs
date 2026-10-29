//! The macro icon chooser's catalog, the list `GetNumMacroIcons`/`GetMacroIconInfo` serve. 12340's
//! `CGUIMacros::LoadIcons` (`0x565840`) enumerates the files under `Interface\Icons\`; it does not
//! read `SpellIcon.dbc`.

use anyhow::Result;

use crate::Chain;

/// The directory the chooser enumerates and the prefix `GetMacroIconInfo` splices back on; the
/// reference strips its 16 characters from each stored entry.
const ICON_DIR: &str = "Interface\\Icons\\";

/// The chooser's list: `INV_Misc_QuestionMark`, which `LoadIcons` seeds and its comparator
/// (`0x564a20`) pins first, then every `Interface\Icons\` file whose name begins `Spell_` or
/// `Ability_` (`IconListArchiveCallback` `0x565630`), sorted and deduplicated without case, each as
/// the full path `GetMacroIconInfo` returns.
///
/// The reference unions the archives' listfiles with two loose-file scans of `Interface\Icons\`;
/// only the archive half is built, as a stock install has no loose icons. Per entry it keeps the
/// name after the prefix without a last-dot `.blp` or `.tga` (`AddMacroIcon` `0x565550`), skipping
/// directories, then `qsort`s and drops adjacent duplicates.
pub fn load_macro_icons(chain: &mut Chain) -> Result<Vec<String>> {
    const SEED: &str = "INV_Misc_QuestionMark";
    let mut names = icon_stems(chain, &["Spell_", "Ability_"])?;
    names.insert(0, SEED.to_string());
    Ok(names
        .into_iter()
        .map(|n| format!("{ICON_DIR}{n}"))
        .collect())
}

/// The guild bank tab chooser's list (`GetNumMacroItemIcons` `0x5664e0`), which 12340's
/// `MacroIcons_Build` (`0x565840`) fills beside the macro list: every `Interface\Icons\` file
/// whose name begins `INV_`, seeded with `INV_Misc_QuestionMark`, which its comparator pins first,
/// then sorted and deduplicated without case.
pub fn load_macro_item_icons(chain: &mut Chain) -> Result<Vec<String>> {
    const SEED: &str = "INV_Misc_QuestionMark";
    let mut names = icon_stems(chain, &["INV_"])?;
    names.retain(|n| !n.eq_ignore_ascii_case(SEED));
    names.insert(0, SEED.to_string());
    Ok(names
        .into_iter()
        .map(|n| format!("{ICON_DIR}{n}"))
        .collect())
}

/// The stems of the `Interface\Icons\` files beginning with one of `prefixes`, sorted and
/// deduplicated without case.
fn icon_stems(chain: &mut Chain, prefixes: &[&str]) -> Result<Vec<String>> {
    let mut names: Vec<String> = Vec::new();
    for entry in chain.list()? {
        let path = entry.name.replace('/', "\\");
        // Case-insensitive (`SStrCmpI`), as listfiles are not consistently cased.
        if path.len() <= ICON_DIR.len()
            || !path.as_bytes()[..ICON_DIR.len()].eq_ignore_ascii_case(ICON_DIR.as_bytes())
        {
            continue;
        }
        let file = &path[ICON_DIR.len()..];
        // The reference skips directories (`flags & 0x10`): here, a name in a subdirectory.
        if file.contains('\\') {
            continue;
        }
        // The last dot starts the extension, so `Foo.tga.blp` stores as `Foo.tga`: the reference's
        // picker shows `Ability_Druid_Mangle.tga` beside `Ability_Druid_Mangle`.
        let Some((stem, ext)) = file.rsplit_once('.') else {
            continue;
        };
        if !ext.eq_ignore_ascii_case("blp") && !ext.eq_ignore_ascii_case("tga") {
            continue;
        }
        let keeps = prefixes.iter().any(|p| {
            stem.len() >= p.len() && stem.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes())
        });
        if keeps {
            names.push(stem.to_string());
        }
    }
    // Sort without case, then drop adjacent duplicates: the reference's two steps, in its order.
    names.sort_by_key(|n| n.to_ascii_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    Ok(names)
}
