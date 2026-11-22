//! The handlers' raise census (#1364): every packet handler that raises into the VM before a later write of its own. 12340 fires each event (`0x703e50`) and line (`0x5216f0`) at its step inside the handler, so a listener reads the state as of that step; a handler here raises at its step once it runs the write after the raise as a later step ([`super::live::UiEvents::world`], or an exclusive handler's [`crate::net::handlers::step`]). The list only falls: each #1245 part converts its own rows and strikes them.

use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;

use crate::test_support::{fn_items, rel_path, rust_files, src_dir};

/// The handlers that raise before a later write: (file, handler, the #1245 part or issue that converts it).
const RAISES_BEFORE_A_WRITE: &[(&str, &str, u16)] = &[
    ("spell/net.rs", "on_spell_go", 1414),
    ("spell/net.rs", "on_spell_learned", 1414),
    ("spell/net.rs", "on_spell_removed", 1414),
    ("spell/net.rs", "on_spell_superceded", 1414),
    ("ui_loot/net.rs", "on_error", 1280),
    ("ui_loot/net.rs", "on_release_response", 1280),
    ("ui_loot/net.rs", "on_response", 1280),
    // Raised in turn since #1354: the disband's line ahead of its clear-all (0x6cbe4b before 0x6cbe55), and the list's lines among its writes (0x6d8870).
    ("ui_party/net.rs", "on_destroyed", 1284),
    ("ui_party/net.rs", "on_list", 1284),
    ("ui_trade/net.rs", "on_trade_status_extended", 1280),
    ("ui_transfer.rs", "on_transfer_pending", 1286),
];

/// The walker's rows read by hand against 12340: every write it sees after the first raise is at the reference's place. (file, handler, why).
const READ_AT_THEIR_STEP: &[(&str, &str, &str)] = &[
    (
        "creature_anim/net.rs",
        "on_attack_stop",
        "the helper writes, then raises PLAYER_LEAVE_COMBAT last",
    ),
    (
        "death/net.rs",
        "on_death_refresh",
        "each packet is its own arm: the forced update derives the release timer before its refresh (0x6e3f13, 0x6e3f18); the walker reads the arms as one",
    ),
    (
        "spell/net.rs",
        "on_channel",
        "each arm writes the channel, then its edge; the walker reads the two arms as one",
    ),
    (
        "spell/net.rs",
        "on_cooldown_packet",
        "the store's write runs before the site's signals; the bundle is handed to the write alone",
    ),
    (
        "spell/net.rs",
        "on_spell_failed",
        "the end's state clear applies before the signals (0x7fed9f before 0x7fee2c), the cast event is the animation's, and the pending release is the finish's step (0x8055a2)",
    ),
    (
        "spell/net.rs",
        "on_spell_start",
        "the cast state's insert applies before the start edge fires, as 0x8053ea precedes the signal 0x8054b8",
    ),
    (
        "ui_battlefield.rs",
        "on_player",
        "a cached name shows the line, else the name's callback is filed (0x54b551); the walker reads the two arms as one",
    ),
    (
        "ui_battlefield.rs",
        "on_status",
        "its raises before the tutorial writes are VM writes, and its one event fires last (0x54b1a9)",
    ),
    (
        "ui_battlefield_score.rs",
        "on_pvp_log_data",
        "every state write precedes the log's VM write and the rebuild's (0x54d469 before 0x54d495)",
    ),
    (
        "ui_companion.rs",
        "on_creature_answer",
        "the callback takes its pending entry before the line, and the update after it is a step (0x5427be)",
    ),
    (
        "ui_quest_watch.rs",
        "on_quest_poi",
        "the store's VM write is the raise; QUEST_POI_UPDATE waits for the frame's flush (0x5e7628 -> 0x614760)",
    ),
];

/// The #1245 parts a row may name, and #1414, which converts #1281's closed rows.
const PARTS: &[u16] = &[
    1248, 1274, 1275, 1276, 1277, 1278, 1279, 1280, 1281, 1282, 1283, 1284, 1285, 1286, 1287, 1288,
    1289, 1366, 1414,
];

/// What raises into the VM through the handler queue or a sink over it.
const RAISES: &[&str] = &[
    "fire",
    "clear_cursor",
    "system_line",
    "write",
    "cast_edge",
    "signal",
    "line",
    "refuse",
];

/// Methods that only read the state they are called on.
const READS: &[&str] = &[
    "get",
    "contains",
    "contains_key",
    "iter",
    "len",
    "is_empty",
    "first",
    "last",
    "copied",
    "cloned",
    "as_ref",
    "as_deref",
    "map",
    "and_then",
    "single",
    "is_some",
    "is_none",
    "is_some_and",
    "unwrap_or",
    "unwrap_or_default",
    "ok",
    "filter",
    "find",
    "any",
    "all",
    "values",
    "keys",
    "is_changed",
    "is_added",
    "clone",
    "slot_of",
];

/// The line queue: a push there is a chat line, not state.
const LINE_QUEUES: &[&str] = &["ChatLog"];

/// `source` with comments and string and char literals blanked, every offset and line kept.
fn blank(source: &str) -> String {
    let b = source.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0;
    let wipe = |out: &mut Vec<u8>, from: usize, to: usize| {
        for c in &mut out[from..to] {
            if *c != b'\n' {
                *c = b' ';
            }
        }
    };
    while i < b.len() {
        let word_before = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
        if b[i..].starts_with(b"//") {
            let end = source[i..].find('\n').map_or(b.len(), |e| i + e);
            wipe(&mut out, i, end);
            i = end;
        } else if b[i..].starts_with(b"/*") {
            let end = source[i..].find("*/").map_or(b.len(), |e| i + e + 2);
            wipe(&mut out, i, end);
            i = end;
        } else if b[i] == b'r' && !word_before && matches!(b.get(i + 1), Some(b'"' | b'#')) {
            let hashes = b[i + 1..].iter().take_while(|&&c| c == b'#').count();
            if b.get(i + 1 + hashes) != Some(&b'"') {
                i += 1;
                continue;
            }
            let close = format!("\"{}", "#".repeat(hashes));
            let from = i + 2 + hashes;
            let end = source[from..]
                .find(&close)
                .map_or(b.len(), |e| from + e + close.len());
            wipe(&mut out, from, end - 1 - hashes);
            i = end;
        } else if b[i] == b'"' {
            let mut j = i + 1;
            while j < b.len() && b[j] != b'"' {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            wipe(&mut out, i + 1, j.min(b.len()));
            i = j + 1;
        } else if b[i] == b'\'' {
            // A char literal, not a lifetime: `'x'`, `'\n'`, `'\u{..}'`.
            let close = if b.get(i + 1) == Some(&b'\\') {
                source[i + 2..].find('\'').map(|e| i + 2 + e)
            } else if b.get(i + 2) == Some(&b'\'') {
                Some(i + 2)
            } else {
                source[i + 1..]
                    .chars()
                    .next()
                    .filter(|c| c.len_utf8() > 1)
                    .and_then(|c| {
                        (b.get(i + 1 + c.len_utf8()) == Some(&b'\'')).then(|| i + 1 + c.len_utf8())
                    })
            };
            match close {
                Some(end) => {
                    wipe(&mut out, i, end + 1);
                    i = end + 1;
                }
                None => i += 1,
            }
        } else {
            i += 1;
        }
    }
    String::from_utf8(out).expect("only ASCII bytes were replaced")
}

/// Whether `text` names `word` as a whole word.
fn names(text: &str, word: &str) -> bool {
    let part = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    text.match_indices(word).any(|(at, _)| {
        !part(text[..at].chars().next_back()) && !part(text[at + word.len()..].chars().next())
    })
}

/// The index of the bracket closing the one at `open`.
fn closing(text: &str, open: usize, lhs: u8, rhs: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (j, &c) in text.as_bytes()[open..].iter().enumerate() {
        if c == lhs {
            depth += 1;
        } else if c == rhs {
            depth -= 1;
            if depth == 0 {
                return Some(open + j);
            }
        }
    }
    None
}

/// The parameters as `(name, type)`, split at the top-level commas.
fn params_of(params: &str) -> Vec<(String, String)> {
    let mut parts = Vec::new();
    let (mut depth, mut from) = (0i32, 0);
    for (i, c) in params.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&params[from..i]);
                from = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&params[from..]);
    parts
        .into_iter()
        .filter_map(|p| {
            let (name, ty) = p.split_once(':')?;
            let name = name.trim().trim_start_matches("mut ").trim();
            Some((
                name.to_string(),
                ty.split_whitespace().collect::<Vec<_>>().join(" "),
            ))
        })
        .collect()
}

/// `body` with the argument of each later step blanked: a `ui.world(..)` step, `in_turn(..)`, `after_raises(..)` and a turn's `lent(..)` run after the raises queued before them fire.
fn without_later_steps(body: &str) -> String {
    let later =
        Regex::new(r"\.world\(|\bin_turn\(|\bafter_raises\(|\.lent\(|\bstep\(world,").unwrap();
    let mut out = body.as_bytes().to_vec();
    for m in later.find_iter(body) {
        let open = body[..m.end()]
            .rfind('(')
            .expect("the match ends in its bracket");
        if let Some(close) = closing(body, open, b'(', b')') {
            for c in &mut out[open + 1..close] {
                if *c != b'\n' {
                    *c = b' ';
                }
            }
        }
    }
    String::from_utf8(out).expect("only ASCII bytes were replaced")
}

/// The `SystemParam` bundles that hold the handler queue, directly or through another bundle.
fn queue_bundles(files: &BTreeMap<String, String>) -> BTreeSet<String> {
    let derive = Regex::new(
        r"#\[derive\(SystemParam\)\]\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))? )?struct (\w+)",
    )
    .unwrap();
    let mut bodies = BTreeMap::new();
    for text in files.values() {
        for c in derive.captures_iter(text) {
            let at = c.get(0).unwrap().end();
            let Some(open) = text[at..].find('{').map(|o| at + o) else {
                continue;
            };
            if let Some(close) = closing(text, open, b'{', b'}') {
                bodies.insert(c[1].to_string(), text[open + 1..close].to_string());
            }
        }
    }
    let mut holders = BTreeSet::new();
    loop {
        let before = holders.len();
        for (name, body) in &bodies {
            let holds = ["UiEvents", "InTurn"]
                .into_iter()
                .chain(holders.iter().map(String::as_str))
                .any(|t| names(body, t));
            if holds {
                holders.insert(name.clone());
            }
        }
        if holders.len() == before {
            return holders;
        }
    }
}

/// Whether the handler raises before a later write of its own: the first raise, then a write to one of its parameters; a helper handed a bundle that holds the queue counts as both.
fn raises_before_a_write(params: &str, body: &str, bundles: &BTreeSet<String>) -> bool {
    let body = without_later_steps(body);
    let mut queues = Vec::new();
    let mut turns = Vec::new();
    let mut state = Vec::new();
    for (name, ty) in params_of(params) {
        if name.starts_with("In(") || name == "_" {
            continue;
        }
        let head = ty.trim_start_matches("Option<");
        if names(&ty, "UiEvents") {
            queues.push(name);
        } else if names(&ty, "InTurn") && !ty.contains("script_calls") {
            turns.push(name);
        } else if LINE_QUEUES.iter().any(|q| ty.contains(q))
            || head.starts_with("Res<")
            || head.starts_with("NonSend<")
            || head.starts_with("Local<")
            || (head.starts_with("Query<") && !ty.contains("&mut"))
        {
            continue;
        } else {
            let bundle = bundles.contains(head.split(['<', ' ']).next().unwrap_or(head));
            state.push((name, bundle));
        }
    }
    let mut raises = Vec::new();
    let mut writes = Vec::new();
    for q in &queues {
        let q = regex::escape(q);
        let call = Regex::new(&format!(r"(?:^|[^\w.]){q}\.(\w+)\(")).unwrap();
        raises.extend(
            call.captures_iter(&body)
                .filter(|c| RAISES.contains(&&c[1]))
                .map(|c| c.get(0).unwrap().start()),
        );
        let handed = Regex::new(&format!(r"&mut \*?{q}\b[^.]|[(,]\s*{q}\s*[,)]")).unwrap();
        raises.extend(handed.find_iter(&body).map(|m| m.start()));
    }
    for t in &turns {
        let turn = Regex::new(&format!(r"\b{}\s*\.turn\(\)", regex::escape(t))).unwrap();
        raises.extend(turn.find_iter(&body).map(|m| m.start()));
    }
    let mut both = Vec::new();
    for (name, bundle) in &state {
        let n = regex::escape(name);
        if *bundle {
            let raise =
                Regex::new(&format!(r"(?:^|[^\w.]){n}\.ui\.(\w+)\(|&mut \*?{n}\.ui\b")).unwrap();
            raises.extend(
                raise
                    .captures_iter(&body)
                    .filter(|c| c.get(1).is_none_or(|m| RAISES.contains(&m.as_str())))
                    .map(|c| c.get(0).unwrap().start()),
            );
            let whole = Regex::new(&format!(r"&mut \*?{n}\b[^.\w]|[(,]\s*{n}\s*[,)]")).unwrap();
            both.extend(whole.find_iter(&body).map(|m| m.start()));
        }
        let method = Regex::new(&format!(r"(?:^|[^\w.]){n}((?:\.\w+)*)\.(\w+)\(")).unwrap();
        for c in method.captures_iter(&body) {
            if (*bundle && c[1].starts_with(".ui")) || READS.contains(&&c[2]) {
                continue;
            }
            writes.push(c.get(0).unwrap().start());
        }
        let lent = Regex::new(&format!(r"&mut \*?{n}\b((?:\.\w+)*)")).unwrap();
        for c in lent.captures_iter(&body) {
            if !(*bundle && c[1].starts_with(".ui")) {
                writes.push(c.get(0).unwrap().start());
            }
        }
        let assign =
            Regex::new(&format!(r"(?:^|[^\w.])\*?{n}((?:\.\w+)*)\s*[+\-|&^]?=[^=]")).unwrap();
        writes.extend(assign.find_iter(&body).map(|m| m.start()));
    }
    if !both.is_empty() {
        return true;
    }
    let Some(first) = raises.iter().min() else {
        return false;
    };
    writes.iter().any(|w| w > first)
}

/// Every registered handler's `(file, name)`, with its parameters and body.
fn handlers(files: &BTreeMap<String, String>) -> Vec<(String, String, String, String)> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for (rel, text) in files {
        for call in text.split("net_handler(").skip(1) {
            let Some((args, _)) = call.split_once(')') else {
                continue;
            };
            let Some((_, handler)) = args.split_once(',') else {
                continue;
            };
            let handler = handler.trim().trim_end_matches(',').trim();
            if handler.is_empty() || !handler.chars().all(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            if !seen.insert((rel.clone(), handler.to_string())) {
                continue;
            }
            let f = fn_items(text)
                .into_iter()
                .find(|f| f.name == handler && !f.body.is_empty())
                .unwrap_or_else(|| {
                    panic!("{rel}: `{handler}` is not in the file that registers it")
                });
            out.push((
                rel.clone(),
                handler.to_string(),
                f.params.to_string(),
                f.body.to_string(),
            ));
        }
    }
    out
}

/// Every non-test source file, comments and literals blanked, inline test modules cut.
fn sources() -> BTreeMap<String, String> {
    rust_files(&src_dir())
        .into_iter()
        .filter_map(|file| {
            let rel = rel_path(&file);
            if rel.ends_with("tests.rs") {
                return None;
            }
            let text = std::fs::read_to_string(&file)
                .expect("readable source")
                .replace("\r\n", "\n");
            Some((rel, super::raise_census_tests::production(&blank(&text))))
        })
        .collect()
}

/// The walker's rows are the list's, and every row names a part: a new handler that raises before a write fails, and a converted one is struck.
#[test]
fn every_handler_that_raises_before_a_write_is_listed_by_its_part() {
    let files = sources();
    let bundles = queue_bundles(&files);
    let mut holding = 0;
    let mut found = BTreeSet::new();
    for (rel, name, params, body) in handlers(&files) {
        let holds = names(&params, "UiEvents")
            || names(&params, "InTurn")
            || bundles.iter().any(|b| names(&params, b));
        if !holds {
            continue;
        }
        holding += 1;
        if raises_before_a_write(&params, &body, &bundles) {
            found.insert((rel, name));
        }
    }
    let listed: BTreeSet<(String, String)> = RAISES_BEFORE_A_WRITE
        .iter()
        .map(|&(f, h, _)| (f, h))
        .chain(READ_AT_THEIR_STEP.iter().map(|&(f, h, _)| (f, h)))
        .map(|(f, h)| (f.to_string(), h.to_string()))
        .collect();
    let rows: String = found
        .iter()
        .map(|(f, h)| format!("    (\"{f}\", \"{h}\", _),\n"))
        .collect();
    eprintln!(
        "handlers holding the queue: {holding}; raising before a write: {}",
        found.len()
    );
    let new: Vec<_> = found.difference(&listed).collect();
    assert!(
        new.is_empty(),
        "handlers raise before a later write: run the write as a step after the raise (#1364): \
         {new:?}. The tree's rows:\n{rows}"
    );
    assert_eq!(
        found, listed,
        "handlers now raise at their step: strike them. The tree's rows:\n{rows}"
    );
}

#[test]
fn the_list_is_sorted_unique_and_names_a_part() {
    assert!(
        READ_AT_THEIR_STEP
            .windows(2)
            .all(|w| (w[0].0, w[0].1) < (w[1].0, w[1].1)),
        "the read rows sorted by file then handler, each once"
    );
    for (f, h, _) in READ_AT_THEIR_STEP {
        assert!(
            !RAISES_BEFORE_A_WRITE.iter().any(|r| (r.0, r.1) == (*f, *h)),
            "{f} `{h}` is listed twice"
        );
    }
    assert!(
        RAISES_BEFORE_A_WRITE
            .windows(2)
            .all(|w| (w[0].0, w[0].1) < (w[1].0, w[1].1)),
        "sorted by file then handler, each once"
    );
    for (f, h, part) in RAISES_BEFORE_A_WRITE {
        assert!(
            PARTS.contains(part),
            "{f} `{h}`: #{part} is not a #1245 part or #1414"
        );
    }
}

/// The walker reads a handler's order: a raise then a write is listed; a write then a raise, or the write as a later step, is not.
#[test]
fn the_walker_reads_a_raise_before_a_write_and_not_a_later_step() {
    let bundles = BTreeSet::new();
    let params = "In(ev): In<SessionEvent>, mut flag: ResMut<Flag>, mut ui: ResMut<UiEvents>";
    assert!(raises_before_a_write(
        params,
        "ui.fire(\"A\", vec![]); flag.0 = true;",
        &bundles
    ));
    assert!(!raises_before_a_write(
        params,
        "flag.0 = true; ui.fire(\"A\", vec![]);",
        &bundles
    ));
    assert!(!raises_before_a_write(
        params,
        "ui.fire(\"A\", vec![]); ui.world(move |w| { flag.0 = true; });",
        &bundles
    ));
    assert!(raises_before_a_write(
        params,
        "helper(&mut ui); flag.set(1);",
        &bundles
    ));
    assert!(!raises_before_a_write(
        params,
        "ui.fire(\"A\", vec![]); let x = flag.get(1);",
        &bundles
    ));
}
