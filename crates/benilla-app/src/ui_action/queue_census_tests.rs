//! The tracker of the app's frame-late refusal lines (#1354): every place still holding
//! `UiErrorKeys` or `UiErrorTexts`, whose lines `super::feed::feed_actions` shows a frame after
//! the code that refused. 12340's message display (`0x5216f0`) runs inside that code, so a line
//! shows through the caller's [`super::signals::SignalSink::line`] instead: a packet handler's
//! queue, a press's edges, the VM in hand, or a system's queued signals. Each #1245 part moves
//! its own sites and strikes its counts here; the list only shrinks, and the part that empties it
//! deletes both queues and the drain.

use std::collections::BTreeMap;
use std::path::Path;

/// Per source file, the lines outside its tests that name either queue: the systems and helpers
/// that still hold one. Sorted by path.
const HOLDERS: &[(&str, usize)] = &[
    ("death/net.rs", 1),
    ("target/by_name.rs", 1),
    ("target/click.rs", 1),
    ("target/stone.rs", 1),
    ("ui_binder.rs", 2),
    ("ui_chat/input/mod.rs", 3),
    ("ui_chat/net.rs", 9),
    ("ui_guild/mod.rs", 10),
    ("ui_lfg.rs", 2),
    ("ui_loot/mod.rs", 3),
    ("ui_loot/net.rs", 6),
    ("ui_notices.rs", 5),
    ("ui_party/feed.rs", 1),
    ("ui_party/net.rs", 18),
    ("ui_socket.rs", 2),
    ("ui_stable/net.rs", 3),
    ("ui_trade/mod.rs", 2),
    ("ui_trade/net.rs", 3),
    ("ui_transfer.rs", 3),
];

/// Where the queues are defined, drained and registered, which the count leaves out.
const HOME: &[&str] = &[
    "game_plugins.rs",
    "ui_action/errors.rs",
    "ui_action/feed.rs",
    "ui_action/mod.rs",
];

fn is_test_file(path: &str) -> bool {
    path.ends_with("_tests.rs") || path.ends_with("/tests.rs") || path.contains("/tests/")
}

/// The source before its `#[cfg(test)] mod`, if any.
fn non_test(source: &str) -> &str {
    source
        .find("\n#[cfg(test)]\nmod ")
        .map_or(source, |at| &source[..at])
}

fn names_a_queue(line: &str) -> bool {
    let line = line.trim_start();
    !line.starts_with("//")
        && ["UiErrorKeys", "UiErrorTexts"].iter().any(|name| {
            line.match_indices(name).any(|(at, _)| {
                let next = line[at + name.len()..].chars().next();
                !next.is_some_and(|c| c.is_alphanumeric() || c == '_')
            })
        })
}

fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, usize>) {
    for entry in std::fs::read_dir(dir).expect("the source tree reads") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            walk(&path, root, out);
            continue;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .expect("under the root")
            .to_string_lossy()
            .replace('\\', "/");
        if is_test_file(&rel) || HOME.contains(&rel.as_str()) {
            continue;
        }
        // A checkout's line ends do not move the `#[cfg(test)]` split.
        let source = std::fs::read_to_string(&path)
            .expect("a source file reads")
            .replace("\r\n", "\n");
        let count = non_test(&source)
            .lines()
            .filter(|l| names_a_queue(l))
            .count();
        if count > 0 {
            out.insert(rel, count);
        }
    }
}

#[test]
fn the_frame_late_line_holders_are_the_listed_ones() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = BTreeMap::new();
    walk(&root, &root, &mut found);
    let listed: BTreeMap<String, usize> = HOLDERS
        .iter()
        .map(|&(path, n)| (path.to_string(), n))
        .collect();
    assert_eq!(
        found, listed,
        "a refusal line shows through the caller's SignalSink::line, never a frame-late queue; \
         a part that moved its sites strikes their count in HOLDERS"
    );
}

#[test]
fn the_holder_list_is_sorted_and_unique() {
    assert!(
        HOLDERS.windows(2).all(|w| w[0].0 < w[1].0),
        "HOLDERS stays sorted by path, each path once"
    );
}
