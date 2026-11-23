//! The event argument-shape gate: every fire site with a literal name against
//! `reference/3.3.5a-events.tsv`, since an event name is a plain string at both ends.
//!
//! The table's `conf` is `advisory` (a producer is attributed and its shapes are real, but another
//! computed-id path may add one) or `none` (no producer was proved, which does not mean there is
//! none). So only an `advisory` row gates, and only one whose every alternative is a resolved
//! format: a `?` alternative is a literal-id site whose format pointer was not proven. Only a
//! `ScriptValue::` element's kind is asserted; any other argument counts toward the arity alone.
use std::collections::BTreeMap;

/// One row of `reference/3.3.5a-events.tsv`.
struct Row {
    /// The alternatives, each a per-slot kind list. `%s` -> `s`, `%d`/`%u`/`%f` -> `n`.
    shapes: Vec<Vec<char>>,
    producers: String,
    /// Every alternative is a resolved format, so a fire site the shapes do not fit disagrees with
    /// a proven producer.
    resolved: bool,
}

/// Computed-id producers the table does not attribute, each a shape the reference's own code
/// pushes: (event, its extra alternative, the site). Each only adds an alternative to a gated row.
const COMPUTED_SHAPES: &[(&str, &str, &str)] = &[(
    "UPDATE_FACTION",
    "s",
    "the watched faction's mirror watch (0x6e56e0 registers 0x6cddd0 for event 0x16a), which pushes \"player\" (0x6cdde1)",
)];

fn table() -> BTreeMap<String, Row> {
    let tsv = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/3.3.5a-events.tsv"
    );
    let text = std::fs::read_to_string(tsv).expect("reference/3.3.5a-events.tsv");
    let mut out = BTreeMap::new();
    for line in text.lines() {
        if line.starts_with('#') || line.starts_with("name\t") {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        // name ids producers arg_formats conf note
        if f.len() < 5 || f[4] != "advisory" {
            continue;
        }
        let resolved = !f[3].split('|').any(|alt| alt.contains('?'));
        let shapes = f[3]
            .split('|')
            .filter(|alt| !alt.contains('?'))
            .map(|alt| {
                if alt == "()" {
                    return Vec::new();
                }
                alt.split('%')
                    .skip(1)
                    .map(|d| match d.chars().next() {
                        Some('s') => 's',
                        _ => 'n',
                    })
                    .collect()
            })
            .collect();
        let mut shapes: Vec<Vec<char>> = shapes;
        for (name, extra, _) in COMPUTED_SHAPES {
            if *name == f[0] {
                shapes.push(extra.chars().collect());
            }
        }
        out.insert(
            f[0].to_string(),
            Row {
                shapes,
                producers: f[2].to_string(),
                resolved,
            },
        );
    }
    out
}

/// Splits `body` at its top-level commas: `vec![…]`'s elements, nested calls left whole.
fn elements(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut depth, mut cur, mut in_str, mut esc) = (0i32, String::new(), false, false);
    for c in body.chars() {
        if in_str {
            cur.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                cur.push(c);
            }
            '(' | '[' | '{' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out.into_iter()
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
        .collect()
}

/// The kind an argument expression pushes: `s` string, `n` number, `b` boolean, `x` nil,
/// `?` not determinable from the source.
fn kind_of(expr: &str) -> char {
    let e = expr.trim_start_matches('&').trim();
    for (variant, k) in [
        ("ScriptValue::Str", 's'),
        ("ScriptValue::Int", 'n'),
        ("ScriptValue::Number", 'n'),
        ("ScriptValue::Bool", 'b'),
        ("ScriptValue::Nil", 'x'),
    ] {
        if e.starts_with(variant) {
            return k;
        }
    }
    '?'
}

/// Every `(event, argument kinds, where)` benilla fires with a literal name.
fn fire_sites() -> Vec<(String, Vec<char>, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .join("crates");
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.extension().is_none_or(|x| x != "rs") {
                continue;
            }
            // Test fires are not shipped: skip test files and every `#[cfg(test)]` tail.
            if p.to_string_lossy().contains("test") {
                continue;
            }
            let whole = std::fs::read_to_string(&p).unwrap_or_default();
            let text = whole
                .split_once("#[cfg(test)]")
                .map_or(whole.as_str(), |(head, _)| head);
            // Split so this file cannot match its own scanner.
            for call in [
                concat!("fire_event", "("),
                concat!("fire_event_into", "("),
                concat!(".raise", "("),
                // An event a packet handler fires after its changes apply.
                concat!(".fire", "("),
                // A unique signal, which fires with no arguments at the frame's flush.
                concat!(".unique_signal", "("),
                // A verb's unique signal, queued through the model.
                concat!("live::unique_signal", "("),
            ] {
                let mut from = 0;
                while let Some(i) = text[from..].find(call) {
                    let at = from + i + call.len();
                    from = at;
                    let rest = text[at..].trim_start();
                    let rest = rest.strip_prefix("lua,").map_or(rest, str::trim_start);
                    let rest = ["&mut model,", "model,"]
                        .iter()
                        .find_map(|p| rest.strip_prefix(p))
                        .map_or(rest, str::trim_start);
                    let Some(body) = rest.strip_prefix('"') else {
                        continue;
                    };
                    let Some(end) = body.find('"') else { continue };
                    let (name, after) = (&body[..end], &body[end + 1..]);
                    if call.contains("unique_signal") && after.trim_start().starts_with(')') {
                        out.push((name.to_string(), Vec::new(), p.display().to_string()));
                        continue;
                    }
                    let Some(args) = after.trim_start().strip_prefix(',') else {
                        continue;
                    };
                    let args = args.trim_start();
                    let kinds = if let Some(v) = args.strip_prefix("vec![") {
                        let mut depth = 1i32;
                        let mut close = v.len();
                        for (k, c) in v.char_indices() {
                            match c {
                                '[' => depth += 1,
                                ']' => {
                                    depth -= 1;
                                    if depth == 0 {
                                        close = k;
                                        break;
                                    }
                                }
                                _ => {}
                            }
                        }
                        elements(&v[..close]).iter().map(|e| kind_of(e)).collect()
                    } else if args.starts_with("Vec::new()") {
                        Vec::new()
                    } else {
                        // An argument list built elsewhere: arity unknowable here.
                        continue;
                    };
                    out.push((name.to_string(), kinds, p.display().to_string()));
                }
            }
        }
    }
    out
}

#[test]
fn every_event_we_fire_carries_the_reference_s_arguments() {
    let table = table();
    let gateable = table.values().filter(|r| r.resolved).count();
    assert!(
        gateable > 300,
        "the events table gave only {gateable} gateable rows — it stopped being read"
    );
    let sites = fire_sites();
    assert!(
        sites.len() > 200,
        "the walker found only {} literal fire sites — it stopped matching, which would make this \
         gate silently vacuous",
        sites.len()
    );

    let mut checked = 0usize;
    let mut wrong: Vec<String> = Vec::new();
    for (name, kinds, at) in &sites {
        let Some(row) = table.get(name).filter(|r| r.resolved) else {
            continue;
        };
        checked += 1;
        let fits = row.shapes.iter().any(|want| {
            want.len() == kinds.len()
                && want
                    .iter()
                    .zip(kinds)
                    .all(|(w, g)| *g == '?' || *g == *w || (*w == 'n' && *g == 'n'))
        });
        if !fits {
            let shapes: Vec<String> = row
                .shapes
                .iter()
                .map(|s| format!("({})", s.iter().collect::<String>()))
                .collect();
            wrong.push(format!(
                "{name} fired as ({}) from {at} — the reference's {} producer(s) push {}",
                kinds.iter().collect::<String>(),
                row.producers,
                shapes.join(" | ")
            ));
        }
    }
    assert!(
        checked >= 195,
        "only {checked} fires were measurable against the table — coverage collapsed"
    );
    wrong.sort();
    wrong.dedup();
    assert!(
        wrong.is_empty(),
        "{} fire site(s) push the wrong argument shape. An event name is a plain string at both \
         ends, so nothing else in the build can see this:\n  {}",
        wrong.len(),
        wrong.join("\n  ")
    );
}

/// The reference registers one watch per named unit-window field, so each event the table
/// attributes to that bridge comes with the unit token, and each needs a producer here.
#[test]
fn every_unit_field_bridge_event_has_a_producer() {
    /// Bridge events nothing here produces, each with its reason; an entry that gains a producer,
    /// or that is no bridge event, fails the gate.
    const UNPRODUCED: &[(&str, &str)] = &[(
        "UNIT_TARGET",
        "`UNIT_FIELD_TARGET`, the unit's target. The unit snapshot the feed diffs (`UnitState`) does not \
         carry a unit's target, so nothing sees it change; the target-of-target frame refreshes \
         on its own update",
    )];

    let table = table();
    let bridge: Vec<&String> = table
        .iter()
        .filter(|(_, r)| r.producers.contains("unit-field-bridge"))
        .map(|(n, _)| n)
        .collect();
    assert!(
        bridge.len() >= 29,
        "only {} bridge events in the table — it stopped being read",
        bridge.len()
    );
    let strays: Vec<&str> = UNPRODUCED
        .iter()
        .map(|(e, _)| *e)
        .filter(|e| !bridge.iter().any(|b| b == e))
        .collect();
    assert!(
        strays.is_empty(),
        "not a bridge event of the 3.3.5a table: {strays:?}"
    );
    let fired: std::collections::HashSet<String> = fire_sites()
        .into_iter()
        .map(|(n, _, _)| n)
        .chain(
            // The twelve power events are built with `format!("UNIT_{}", power_token(…))`, so
            // there is no literal to scan; `reference_ui`'s `CONSTRUCTED` list declares the same
            // family.
            [
                "MANA",
                "RAGE",
                "FOCUS",
                "ENERGY",
                "HAPPINESS",
                "RUNIC_POWER",
                "MAXMANA",
                "MAXRAGE",
                "MAXFOCUS",
                "MAXENERGY",
                "MAXHAPPINESS",
                "MAXRUNIC_POWER",
            ]
            .iter()
            .map(|p| format!("UNIT_{p}")),
        )
        .collect();

    let expected: std::collections::HashSet<&str> = UNPRODUCED.iter().map(|(e, _)| *e).collect();
    let missing: Vec<&str> = bridge
        .iter()
        .map(|n| n.as_str())
        .filter(|n| !fired.contains(*n) && !expected.contains(n))
        .collect();
    assert!(
        missing.is_empty(),
        "the reference fires these off a named unit descriptor field and benilla fires them \
         nowhere. No stock file need listen for one — the corpus does, and an addon that \
         registers an event nothing produces hears silence with no error anywhere:\n  {missing:?}"
    );
    let now_fired: Vec<&str> = UNPRODUCED
        .iter()
        .map(|(e, _)| *e)
        .filter(|e| fired.contains(*e))
        .collect();
    assert!(
        now_fired.is_empty(),
        "these now HAVE a producer — take them out of UNPRODUCED so the list keeps meaning what \
         it says: {now_fired:?}"
    );
}
