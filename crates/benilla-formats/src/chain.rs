//! The patch chain: a priority-ordered set of MPQ archives, read through `benilla-mpq`, composed
//! from the `Data` folder by the active client's [`ArchiveLayout`]. A read resolves a name to the
//! highest-priority archive holding it, so a patch wins; base archives carry no `(listfile)`, so
//! resolution is by name hash.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use benilla_expansions::{ArchiveLayout, Expansion, EXPANSION_ENV, LOCALE};
use benilla_mpq::Archive;

/// One entry from a chain listing: an internal path and its uncompressed size.
pub struct ChainEntry {
    pub name: String,
    pub size: u64,
}

/// A priority-ordered patch chain of MPQ archives (`Send + Sync`; reads are `&self` and lock-free).
pub struct Chain {
    /// Ascending priority: later archives win.
    archives: Vec<Archive>,
    /// The client whose layouts the chain's files are read in.
    expansion: Expansion,
}

/// Whether `path` matches `pattern` as the reference's `FindFirstFileW` glob does (1.12.1 wrapper
/// `0x42ad10`): any case, `?` exactly one character other than a separator, so `patch-10.MPQ`
/// never matches `patch-?.MPQ`.
fn glob_match(pattern: &str, path: &str) -> bool {
    pattern.len() == path.len()
        && pattern
            .bytes()
            .zip(path.bytes())
            .all(|(p, c)| (p == b'?' && c != b'/') || p.eq_ignore_ascii_case(&c))
}

/// The reference's mount order over a `Data` listing, ascending priority: `layout`'s base
/// archives, its named patches, every patch-glob match by case-folded `\`-separated path (the
/// references sort descending with `strnicmp` and walk backwards), then its tail. `files` are
/// paths relative to `Data`, `/`-separated; they match case-insensitively and come back as found
/// on disk. A missing required archive, or a present unsupported one, is an error.
fn mount_order(
    layout: &ArchiveLayout,
    files: &[String],
    locale: Option<&str>,
) -> Result<Vec<String>> {
    let named = |p: &str| match locale {
        Some(l) => p.replace(LOCALE, l),
        None => p.to_string(),
    };
    let find = |want: &str| {
        let want = named(want);
        files
            .iter()
            .find(|f| f.eq_ignore_ascii_case(&want))
            .cloned()
    };
    if let Some(found) = layout.unsupported.iter().find_map(|u| find(u)) {
        bail!("{found} is mounted by a mechanism benilla does not implement");
    }
    let mut order = Vec::new();
    for base in layout.base {
        match find(base.path) {
            Some(found) => order.push(found),
            None if base.required => bail!("the required archive {} is missing", named(base.path)),
            None => {}
        }
    }
    order.extend(layout.patches.iter().filter_map(|p| find(p)));
    let globs: Vec<String> = layout.patch_globs.iter().map(|g| named(g)).collect();
    let mut patches: Vec<String> = files
        .iter()
        .filter(|f| globs.iter().any(|g| glob_match(g, f)))
        .cloned()
        .collect();
    patches.sort_by_key(|f| f.replace('/', "\\").to_ascii_lowercase());
    order.extend(patches);
    order.extend(layout.tail.iter().filter_map(|t| find(t)));
    Ok(order)
}

/// The locale folder a layout reads from: the one subfolder of `Data` holding every required
/// archive that names the locale. The reference reads its `locale` CVar; benilla has no locale
/// selection yet, so more than one candidate is an error.
fn locale_of(layout: &ArchiveLayout, files: &[String], dirs: &[String]) -> Result<String> {
    let complete = |dir: &&String| {
        layout
            .base
            .iter()
            .filter(|b| b.required && b.path.contains(LOCALE))
            .all(|b| {
                let want = b.path.replace(LOCALE, dir);
                files.iter().any(|f| f.eq_ignore_ascii_case(&want))
            })
    };
    match dirs.iter().filter(complete).collect::<Vec<_>>()[..] {
        [one] => Ok(one.clone()),
        [] => bail!("no locale folder holds the locale archives"),
        ref many => bail!("more than one locale folder: {many:?}"),
    }
}

/// The names in `dir` of the entries that are files (`want_files`) or folders, sorted, as
/// `read_dir` order is arbitrary and case-variant ties must resolve deterministically.
fn list(dir: &Path, want_files: bool) -> Result<Vec<String>> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .with_context(|| format!("listing {}", dir.display()))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            // `path().is_file()` follows symlinks (`read_dir`'s file_type doesn't).
            let path = entry.path();
            let keep = if want_files {
                path.is_file()
            } else {
                path.is_dir()
            };
            keep.then(|| entry.file_name())
        })
        .filter_map(|name| name.into_string().ok())
        .collect();
    names.sort();
    Ok(names)
}

impl Chain {
    /// Open a `Data` directory's archives in the active client's [`mount_order`], or a single
    /// `.MPQ` file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_for(path, Expansion::active())
    }

    /// [`Chain::open`] for a named client.
    ///
    /// Deviation: an archive that fails to open is an error, where the reference logs
    /// `"Failed to open archive"` and goes on, because a skipped corrupt archive surfaces only as
    /// missing files far downstream.
    pub fn open_for(path: &Path, expansion: Expansion) -> Result<Self> {
        let mut archives = Vec::new();
        if path.is_dir() {
            let layout = &expansion.profile().archives;
            let wrong_install = || {
                format!(
                    "{} is not a {expansion} Data folder (${EXPANSION_ENV} selects the client)",
                    path.display()
                )
            };
            let mut files = list(path, true)?;
            let locale = if layout.uses_locale() {
                let dirs = list(path, false)?;
                for dir in &dirs {
                    files.extend(
                        list(&path.join(dir), true)?
                            .into_iter()
                            .map(|f| format!("{dir}/{f}")),
                    );
                }
                Some(locale_of(layout, &files, &dirs).with_context(wrong_install)?)
            } else {
                None
            };
            let order =
                mount_order(layout, &files, locale.as_deref()).with_context(wrong_install)?;
            for name in order {
                let mpq = path.join(&name);
                archives.push(
                    Archive::open(&mpq).with_context(|| format!("opening {}", mpq.display()))?,
                );
            }
            if archives.is_empty() {
                bail!("no known {expansion} MPQs found in {}", path.display());
            }
        } else {
            archives.push(
                Archive::open(path).with_context(|| format!("opening MPQ {}", path.display()))?,
            );
        }
        Ok(Self {
            archives,
            expansion,
        })
    }

    /// The highest-priority archive with an entry for `name`, a delete marker included, as a
    /// tombstone shadows every lower copy: check [`Archive::is_delete_marker`] for a readable file.
    fn resolve(&self, name: &str) -> Option<&Archive> {
        self.archives.iter().rev().find(|a| a.contains(name))
    }

    /// The client whose data this is: its tables are read in that client's layouts.
    pub fn expansion(&self) -> Expansion {
        self.expansion
    }

    /// The mounted archives' paths, lowest priority first.
    pub fn archive_paths(&self) -> impl Iterator<Item = &Path> {
        self.archives.iter().map(Archive::path)
    }

    /// Whether `name` (`/` or `\`, any case) is a readable file, not a delete marker.
    pub fn contains(&self, name: &str) -> bool {
        self.resolve(name)
            .is_some_and(|a| !a.is_delete_marker(name))
    }

    /// The path of the archive `name` resolves to, for debugging and extraction.
    pub fn find_file_archive(&self, name: &str) -> Option<&Path> {
        self.resolve(name).map(|a| a.path())
    }

    /// Read a file by internal path (`/` or `\`) from its winning archive.
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let archive = self.archive_for(name)?;
        archive
            .read_file(name)
            .with_context(|| format!("reading {name} from {}", archive.path().display()))
    }

    /// The archive `name` reads from, as a cheap handle: a caller holding the chain behind a lock
    /// looks the file up under it and reads (`Archive::open_file`) after releasing it. No I/O.
    pub fn archive_for(&self, name: &str) -> Result<Archive> {
        let archive = self
            .resolve(name)
            .ok_or_else(|| anyhow!("file not in patch chain: {name}"))?;
        // A tombstone deletes the path from the composite: not found, never a stale lower copy.
        if archive.is_delete_marker(name) {
            bail!(
                "file deleted from patch chain: {name} (tombstoned by {})",
                archive.path().display()
            );
        }
        Ok(archive.clone())
    }

    /// `&mut` alias of [`Chain::read`] for call sites that thread a `&mut Chain`.
    pub fn read_file(&mut self, name: &str) -> Result<Vec<u8>> {
        self.read(name)
    }

    /// The chain's named files with sizes, for development and extraction; a file in no listfile
    /// (most of `texture.MPQ`) is readable by name but not listed. Unions every archive's
    /// `(listfile)`, as each names only its own files; sizes come from the winning archive.
    pub fn list(&self) -> Result<Vec<ChainEntry>> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for archive in &self.archives {
            let Ok(listfile) = archive.read_file("(listfile)") else {
                continue;
            };
            for raw in String::from_utf8_lossy(&listfile).split([';', '\r', '\n']) {
                let name = raw.trim();
                // Dedupe the way MPQ hashing compares names: any case, `/` and `\` alike.
                if name.is_empty() || !seen.insert(name.replace('/', "\\").to_ascii_lowercase()) {
                    continue;
                }
                if let Some(a) = self.resolve(name) {
                    // A tombstoned path is not a file in the composite.
                    if a.is_delete_marker(name) {
                        continue;
                    }
                    out.push(ChainEntry {
                        name: name.to_string(),
                        size: a.file_size(name).unwrap_or(0) as u64,
                    });
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn classic() -> &'static ArchiveLayout {
        &Expansion::Classic.profile().archives
    }

    fn wotlk() -> &'static ArchiveLayout {
        &Expansion::Wotlk.profile().archives
    }

    /// The Classic order over a flat `Data` listing.
    fn mount_order_classic(dir: &[String]) -> Vec<String> {
        mount_order(classic(), dir, None).expect("Classic requires no archive")
    }

    #[test]
    fn patch_glob_matches_exactly_one_character_case_insensitively() {
        let is_patch_glob_match = |name| glob_match("patch-?.MPQ", name);
        assert!(is_patch_glob_match("patch-2.MPQ"));
        assert!(is_patch_glob_match("patch-3.MPQ"));
        assert!(is_patch_glob_match("PATCH-A.mpq"));
        assert!(!is_patch_glob_match("patch-.MPQ"));
        assert!(!is_patch_glob_match("patch-10.MPQ"));
        assert!(!is_patch_glob_match("patch-33.MPQ"));
        assert!(!is_patch_glob_match("patch.MPQ"));
        assert!(!is_patch_glob_match("patch-2.MPQ.bak"));
        assert!(!is_patch_glob_match("mypatch-2.MPQ"));
        assert!(!glob_match("a?b", "a/b"));
    }

    #[test]
    fn mount_order_is_the_carved_law() {
        // base.MPQ is telemetry-only in the reference and never mounts.
        let dir = owned(&[
            "patch-2.MPQ",
            "backup.MPQ",
            "model.MPQ",
            "base.MPQ",
            "dbc.MPQ",
            "patch.MPQ",
            "eula.html",
            "patch-3.MPQ",
            "speech2.MPQ",
            "texture.MPQ",
        ]);
        assert_eq!(
            mount_order_classic(&dir),
            owned(&[
                "dbc.MPQ",
                "texture.MPQ",
                "model.MPQ",
                "patch.MPQ",
                "patch-2.MPQ",
                "patch-3.MPQ",
                "speech2.MPQ",
            ])
        );
    }

    #[test]
    fn patch_sort_is_ascending_and_case_folded() {
        let dir = owned(&["patch-B.MPQ", "patch-3.MPQ", "patch-a.MPQ", "patch-2.MPQ"]);
        assert_eq!(
            mount_order_classic(&dir),
            owned(&["patch-2.MPQ", "patch-3.MPQ", "patch-a.MPQ", "patch-B.MPQ"])
        );
    }

    #[test]
    fn base_archives_are_found_case_insensitively() {
        let dir = owned(&["DBC.mpq", "Model.MPQ", "PATCH.mpq"]);
        assert_eq!(
            mount_order_classic(&dir),
            owned(&["DBC.mpq", "Model.MPQ", "PATCH.mpq"])
        );
    }

    /// The `wotlk-original` install's `Data` listing, the locale folder's loose files included.
    fn wotlk_install() -> Vec<String> {
        owned(&[
            "common-2.MPQ",
            "common.MPQ",
            "expansion.MPQ",
            "lichking.MPQ",
            "patch-2.MPQ",
            "patch-3.MPQ",
            "patch.MPQ",
            "enUS/backup-enUS.MPQ",
            "enUS/base-enUS.MPQ",
            "enUS/expansion-locale-enUS.MPQ",
            "enUS/expansion-speech-enUS.MPQ",
            "enUS/lichking-locale-enUS.MPQ",
            "enUS/lichking-speech-enUS.MPQ",
            "enUS/locale-enUS.MPQ",
            "enUS/patch-enUS-2.MPQ",
            "enUS/patch-enUS-3.MPQ",
            "enUS/patch-enUS.MPQ",
            "enUS/realmlist.wtf",
            "enUS/speech-enUS.MPQ",
        ])
    }

    #[test]
    fn wotlk_mount_order_is_the_reference_s() {
        let files = wotlk_install();
        assert_eq!(
            mount_order(wotlk(), &files, Some("enUS")).unwrap(),
            owned(&[
                "enUS/lichking-speech-enUS.MPQ",
                "enUS/expansion-speech-enUS.MPQ",
                "enUS/lichking-locale-enUS.MPQ",
                "enUS/expansion-locale-enUS.MPQ",
                "enUS/speech-enUS.MPQ",
                "enUS/locale-enUS.MPQ",
                "common-2.MPQ",
                "common.MPQ",
                "lichking.MPQ",
                "expansion.MPQ",
                "enUS/patch-enUS.MPQ",
                "patch.MPQ",
                // `data\enus\…` sorts below `data\patch-…`, so the locale patches lose to these.
                "enUS/patch-enUS-2.MPQ",
                "enUS/patch-enUS-3.MPQ",
                "patch-2.MPQ",
                "patch-3.MPQ",
            ])
        );
    }

    #[test]
    fn wotlk_patch_sort_is_by_full_path_so_the_locale_name_decides() {
        // `data\rurU\…` sorts above `data\patch-…`: the reference's sort, not a locale rule.
        let files = owned(&[
            "common.MPQ",
            "patch-2.MPQ",
            "ruRU/locale-ruRU.MPQ",
            "ruRU/speech-ruRU.MPQ",
            "ruRU/patch-ruRU-2.MPQ",
        ]);
        let order = mount_order(wotlk(), &files, Some("ruRU")).unwrap();
        assert_eq!(
            order[order.len() - 2..],
            owned(&["patch-2.MPQ", "ruRU/patch-ruRU-2.MPQ"])
        );
    }

    #[test]
    fn wotlk_fails_on_a_missing_required_archive() {
        let files: Vec<String> = wotlk_install()
            .into_iter()
            .filter(|f| f != "common.MPQ")
            .collect();
        let err = mount_order(wotlk(), &files, Some("enUS")).unwrap_err();
        assert!(err.to_string().contains("common.MPQ"), "{err}");
    }

    #[test]
    fn wotlk_skips_a_missing_optional_archive() {
        let files: Vec<String> = wotlk_install()
            .into_iter()
            .filter(|f| f != "lichking.MPQ")
            .collect();
        let order = mount_order(wotlk(), &files, Some("enUS")).unwrap();
        assert!(!order.iter().any(|f| f == "lichking.MPQ"));
    }

    #[test]
    fn wotlk_fails_on_alternate_mpq_rather_than_skip_it() {
        let mut files = wotlk_install();
        files.push("Alternate.MPQ".into());
        assert!(mount_order(wotlk(), &files, Some("enUS")).is_err());
    }

    #[test]
    fn the_locale_is_the_one_folder_with_the_locale_archives() {
        let files = wotlk_install();
        let dirs = owned(&["Cache", "enUS"]);
        assert_eq!(locale_of(wotlk(), &files, &dirs).unwrap(), "enUS");

        let mut two = files.clone();
        two.extend(owned(&["deDE/locale-deDE.MPQ", "deDE/speech-deDE.MPQ"]));
        let dirs = owned(&["deDE", "enUS"]);
        assert!(locale_of(wotlk(), &two, &dirs).is_err());

        assert!(locale_of(wotlk(), &owned(&["dbc.MPQ"]), &[]).is_err());
    }

    #[test]
    fn a_classic_data_folder_is_not_a_wotlk_one() {
        let files = owned(&["dbc.MPQ", "patch.MPQ", "patch-2.MPQ"]);
        assert!(mount_order(wotlk(), &files, Some("enUS")).is_err());
    }
}
