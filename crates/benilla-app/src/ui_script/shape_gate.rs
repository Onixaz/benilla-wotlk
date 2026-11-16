//! The return-shape gate: `reference/3.3.5a-shapes.tsv`, every binding the 12340 client registers,
//! against what this client answers.
//!
//! The table says per row what to trust (`reference/README.md`): arity where `arity_conf` is
//! `exact` (one count) or `set` (a sound `|`-set, any member of which is an answer), never where it
//! is `unknown`; kinds only where `kinds_conf` is `agree` and the row lacks the `delegated-push`
//! note, whose trace sees only the binding's own pushes; `argc` never, an observed floor only. A
//! 3.3.5a predicate may push a Lua boolean, so a boolean is a kind like any other, held to the
//! row's kinds where they are fit to gate. Only query verbs are probed, a call that raises is
//! skipped, and a name registered from several tables (`GetBuildInfo` is 5 in the glue table) is
//! read at its non-glue row.

/// Globals known to answer the wrong arity: a name and the reason. The list may only shrink.
const NOT_YET_ASSERTED: &[(&str, &str)] = &[];

/// The kind gate's list, same rules: a name, the wrong kinds it answers and the reason. An entry
/// whose binding comes into agreement fails the gate.
const KINDS_NOT_YET_ASSERTED: &[(&str, &str, &str)] = &[];

/// The widget kind gate's list, same rules as [`KINDS_NOT_YET_ASSERTED`].
const WIDGET_KINDS_NOT_YET_ASSERTED: &[(&str, &str, &str)] = &[];

/// The widget arity gate's list, same rules as [`NOT_YET_ASSERTED`].
const WIDGET_NOT_YET_ASSERTED: &[&str] = &[];

/// The coverage floors, each the count a run measured when it was set: raise one when coverage
/// rises, never lower it to fit a change.
const ARITY_FLOOR: usize = 537;
const KIND_FLOOR: usize = 142;
const BOOLEAN_FLOOR: usize = 153;
const WIDGET_ARITY_FLOOR: usize = 241;
/// One widget method row is fit for a kind gate: nearly every method's pushes go through a callee
/// the trace does not follow (`delegated-push`).
const WIDGET_KIND_FLOOR: usize = 1;
const WIDGET_BOOLEAN_FLOOR: usize = 1;

/// The VM the gates probe: the PC client's, as the table is the Windows build's harvest, so
/// `IsWindowsClient` and `IsMacClient` answer as there whatever machine runs the gate.
fn vm() -> benilla_ui::script::UiScript {
    benilla_ui::script::UiScript::new_on(benilla_ui::script::ClientOs::Pc).expect("VM")
}

/// The glue registrar table. This VM is the in-game one, so a duplicated name is read at its
/// other row.
const GLUE_TABLE: &str = "t022";

struct Row {
    name: String,
    table: String,
    /// The answer counts the row allows: one for `exact`, the members of a `set`.
    arities: Vec<usize>,
    /// The `kinds` column verbatim, `|`-separated alternatives; empty unless trustworthy.
    kinds: String,
}

/// The arities an `arity` cell allows, when `arity_conf` makes it gateable.
fn arity_set(arity: &str, conf: &str) -> Option<Vec<usize>> {
    if conf != "exact" && conf != "set" {
        return None;
    }
    arity.split('|').map(|a| a.trim().parse().ok()).collect()
}

fn rows() -> Vec<Row> {
    rows_of_kind("global")
}

/// The rows of one `table_kind` with a gateable arity: `global` is the registrar surface, `baselib`
/// the Lua base array looped into `_G`.
fn rows_of_kind(kind: &str) -> Vec<Row> {
    let tsv = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/3.3.5a-shapes.tsv"
    );
    let text = std::fs::read_to_string(tsv).expect("reference/3.3.5a-shapes.tsv");
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("name\t"))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            // name table table_kind argc argc_conf arity arity_conf kinds kinds_conf notes
            if f.len() < 7 || f[2] != kind {
                return None;
            }
            // A `delegated-push` row's callee pushes on its behalf, unseen by the push trace, so
            // its kinds cover the binding's own ops only; its arity, read from `eax`, holds.
            let delegated = f.len() > 9 && f[9].contains("delegated-push");
            Some(Row {
                name: f[0].to_string(),
                table: f[1].to_string(),
                arities: arity_set(f[5], f[6])?,
                // Kinds are advisory unless `kinds_conf` is `agree`, so such a row carries none.
                kinds: if f.len() > 8 && f[8] == "agree" && !delegated {
                    f[7].to_string()
                } else {
                    String::new()
                },
            })
        })
        .collect()
}

/// Every registered global whose arity the reference proves, answered by this client.
#[test]
fn every_query_binding_answers_the_reference_s_return_arity() {
    let all = rows();
    assert!(
        all.len() > 800,
        "the vendored table looks wrong: {}",
        all.len()
    );

    let mut by_name: std::collections::HashMap<&str, Vec<&Row>> = std::collections::HashMap::new();
    for r in &all {
        by_name.entry(&r.name).or_default().push(r);
    }

    let mut s = vm();
    // A seated player: the `Unit*` bindings raise on a nil token and answer a real one.
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Shapeprobe".into()),
            level: 60,
            ..Default::default()
        }),
    );
    let mut checked = 0usize;
    let mut mismatches: Vec<String> = Vec::new();

    for (name, rs) in &by_name {
        // Query verbs only: the gate calls what it measures.
        if !["Get", "Is", "Has", "Can", "Unit", "Num"]
            .iter()
            .any(|p| name.starts_with(p))
        {
            continue;
        }
        let candidates: Vec<&&Row> = if rs.len() == 1 {
            rs.iter().collect()
        } else {
            rs.iter().filter(|r| r.table != GLUE_TABLE).collect()
        };
        // Still ambiguous after dropping glue: skip rather than guess.
        let Some(first) = candidates.first() else {
            continue;
        };
        if candidates.iter().any(|r| r.arities != first.arities) {
            continue;
        }
        let want = &first.arities;

        // Counted with the vararg table's `arg.n` inside the `pcall`.
        let call = if name.starts_with("Unit") {
            format!(r#"{name}("player")"#)
        } else {
            format!("{name}()")
        };
        let probe = format!(
            "if type({name}) ~= 'function' then return -1 end \
             local ok, n = pcall(function() return (function(...) return arg.n end)({call}) end) \
             if not ok then return -1 end return n"
        );
        let got: i64 = match s.eval(&probe) {
            Ok(n) => n,
            Err(_) => continue,
        };
        if got < 0 {
            continue;
        }
        checked += 1;
        if !want.contains(&(got as usize)) {
            if NOT_YET_ASSERTED.iter().any(|(n, _)| n == name) {
                continue;
            }
            mismatches.push(format!("{name}: answers {got}, reference states {want:?}"));
        }
    }

    // A floor, not a target: raise it when coverage rises, never lower it to fit a change.
    assert!(
        checked >= ARITY_FLOOR,
        "the gate measured only {checked} bindings — it has stopped covering anything"
    );
    // The list may only shrink: an entry that now answers the reference's arity must come off.
    let stale: Vec<&str> = NOT_YET_ASSERTED
        .iter()
        .map(|(n, ..)| *n)
        .filter(|n| {
            by_name.get(n).is_some_and(|rs| {
                s.eval::<i64>(&format!(
                    "local ok, n = pcall(function() return (function(...) return arg.n end)({n}()) end) \
                     if not ok then return -1 end return n"
                ))
                .is_ok_and(|got| got >= 0 && rs[0].arities.contains(&(got as usize)))
            })
        })
        .collect();
    assert!(
        stale.is_empty(),
        "these now answer the reference's arity and must come off NOT_YET_ASSERTED: {stale:?}"
    );
    assert!(
        mismatches.is_empty(),
        "{} of {checked} probed bindings answer the wrong number of values:\n  {}",
        mismatches.len(),
        mismatches.join("\n  ")
    );
}

/// The base-library probes, a name and its call arguments: these names follow no query-verb
/// convention. The arguments are what the 5.0 signatures accept, chosen to discriminate; `setfenv`
/// is left out because it would change the environment every later probe runs in.
const BASELIB_PROBES: &[(&str, &str)] = &[
    ("collectgarbage", ""),
    ("date", ""),
    // `debugbreak`, `debugdump`, `debuginfo`, `debugload`, `debugprint` and `debugtimestamp` are
    // the reference's six `xor eax,eax; ret` stubs, safe to call.
    ("debugbreak", ""),
    ("debugdump", ""),
    ("debughook", ""),
    ("debuginfo", ""),
    ("debugload", ""),
    ("debuglocals", ""),
    ("debugprint", ""),
    ("debugprofilestart", ""),
    ("debugprofilestop", ""),
    ("debugstack", ""),
    ("debugtimestamp", ""),
    ("gcinfo", ""),
    ("geterrorhandler", ""),
    ("getfenv", ""),
    ("issecure", ""),
    ("time", ""),
    // ── probes that take arguments ──────────────────────────────────────────────────────────
    // Two arguments: only a multi-argument call tells 5.0's `lua_settop(L,1); return 1` from
    // 5.1's `return lua_gettop(L)`.
    ("assert", "1, 2"),
    ("difftime", "2, 1"),
    ("loadstring", "\"return 1\""),
    ("next", "{}"),
    ("strconcat", "\"a\", \"b\""),
    ("strlenutf8", "\"abc\""),
    ("strreplace", "\"abc\", \"b\", \"c\""),
    ("strtrim", "\" a \""),
    ("getglobal", "\"BenillaShapeGateAbsent\""),
    ("getmetatable", "{}"),
    ("pairs", "{}"),
    ("rawequal", "1, 1"),
    ("rawget", "{}, 1"),
    ("rawset", "{}, 1, 1"),
    // These write, but namespaced or inert, so every later probe sees the same VM. The bare VM's
    // handler slot is empty and takes a function alone, so the probe hands it a no-op, which
    // nothing here dispatches to.
    ("seterrorhandler", "function() end"),
    ("setglobal", "\"BenillaShapeGateProbe\", 1"),
    ("setmetatable", "{}, nil"),
    ("tonumber", "\"1\""),
    ("tostring", "nil"),
    ("type", "nil"),
];

/// The base library's list, same rules as [`NOT_YET_ASSERTED`].
const BASELIB_NOT_YET_ASSERTED: &[(&str, &str)] = &[];

#[test]
fn the_base_library_answers_the_reference_s_return_arity_and_kinds() {
    let rows = rows_of_kind("baselib");
    assert!(
        rows.len() > 20,
        "the vendored table's baselib rows look wrong: {}",
        rows.len()
    );
    let s = vm();
    let mut checked = 0usize;
    let mut kinds_checked = 0usize;
    let mut mismatches: Vec<String> = Vec::new();

    for r in &rows {
        let Some((_, args)) = BASELIB_PROBES.iter().find(|(n, _)| *n == r.name) else {
            continue;
        };
        let name = &r.name;
        let probe = format!(
            "if type({name}) ~= 'function' then return -1 end \
             local ok, n = pcall(function() return (function(...) return arg.n end)({name}({args})) end) \
             if not ok then return -1 end return n"
        );
        let got: i64 = match s.eval(&probe) {
            Ok(n) => n,
            Err(_) => continue,
        };
        if got < 0 {
            continue;
        }
        checked += 1;
        if !r.arities.contains(&(got as usize)) {
            if BASELIB_NOT_YET_ASSERTED.iter().any(|(n, _)| n == name) {
                continue;
            }
            mismatches.push(format!(
                "{name}: answers {got}, reference states {:?}",
                r.arities
            ));
            continue;
        }
        // Kinds only where `rows_of_kind` kept them as trustworthy.
        if r.kinds.is_empty() {
            continue;
        }
        let Ok(kinds) = s.eval::<String>(&format!(
            "local t = {{ {name}({args}) }} local out = '' \
             for i = 1, {got} do out = out .. (i > 1 and ',' or '') .. type(t[i]) end return out"
        )) else {
            continue;
        };
        kinds_checked += 1;
        let got_tuple = format!("({kinds})");
        let acceptable = r.kinds.split('|').map(str::trim).any(|alt| {
            alt == got_tuple
                || alt
                    .trim_matches(|c| c == '(' || c == ')')
                    .split(',')
                    .map(str::trim)
                    .zip(kinds.split(',').map(str::trim))
                    // `string?` is string-or-nil; `any`/`value` accept anything.
                    .all(|(want, got)| {
                        want == got
                            || (want == "string?" && (got == "string" || got == "nil"))
                            || want == "any"
                            || want == "value"
                    })
        });
        if !acceptable {
            mismatches.push(format!(
                "{name}: answers kinds {got_tuple}, reference states {}",
                r.kinds
            ));
        }
    }

    // A floor, not a target.
    assert!(
        checked >= 33,
        "the base-library gate measured only {checked} bindings"
    );
    // The kinds comparison has its own floor: the trust narrowing can take it to zero while the
    // arity floor stays green.
    assert!(
        kinds_checked >= 14,
        "the base-library gate compared kinds on only {kinds_checked} bindings"
    );
    assert!(
        mismatches.is_empty(),
        "{} of {checked} probed base-library bindings diverge:\n  {}",
        mismatches.len(),
        mismatches.join("\n  ")
    );
}

/// Every in-world widget class's registrar table and the Lua that makes an object answering it;
/// a method is probed on its own table's class, since one name can sit in several tables at
/// different arities. The glue screens' `ModelFFX` (`t026`) is the glue VM's.
const WIDGET_PROBES: &[(&str, &str)] = &[
    ("t021", "CreateFrame('Frame')"),
    ("t010", "CreateFrame('Frame')"),
    ("t011", "CreateFrame('Frame')"),
    ("t008", "CreateFrame('Frame'):CreateTexture()"),
    ("t009", "CreateFrame('Frame'):CreateFontString()"),
    ("t012", "CreateFont('PGFontObject')"),
    ("t020", "CreateFrame('Frame'):CreateAnimationGroup()"),
    (
        "t013",
        "CreateFrame('Frame'):CreateAnimationGroup():CreateAnimation('Animation')",
    ),
    (
        "t014",
        "CreateFrame('Frame'):CreateAnimationGroup():CreateAnimation('Translation')",
    ),
    (
        "t015",
        "CreateFrame('Frame'):CreateAnimationGroup():CreateAnimation('Rotation')",
    ),
    (
        "t016",
        "CreateFrame('Frame'):CreateAnimationGroup():CreateAnimation('Scale')",
    ),
    (
        "t019",
        "CreateFrame('Frame'):CreateAnimationGroup():CreateAnimation('Alpha')",
    ),
    (
        "t018",
        "CreateFrame('Frame'):CreateAnimationGroup():CreateAnimation('Path')",
    ),
    (
        "t017",
        "CreateFrame('Frame'):CreateAnimationGroup():CreateAnimation('Path'):CreateControlPoint()",
    ),
    ("t049", "CreateFrame('Minimap')"),
    ("t057", "CreateFrame('QuestPOIFrame')"),
    ("t060", "CreateFrame('PlayerModel')"),
    ("t061", "CreateFrame('DressUpModel')"),
    ("t063", "CreateFrame('TabardModel')"),
    ("t090", "CreateFrame('Cooldown')"),
    ("t095", "CreateFrame('GameTooltip', 'PGTip')"),
    ("t101", "CreateFrame('Model')"),
    ("t102", "CreateFrame('MovieFrame')"),
    ("t103", "CreateFrame('ColorSelect')"),
    ("t104", "CreateFrame('StatusBar')"),
    ("t105", "CreateFrame('Slider')"),
    ("t106", "CreateFrame('ScrollFrame')"),
    ("t107", "CreateFrame('ScrollingMessageFrame')"),
    ("t108", "CreateFrame('MessageFrame')"),
    ("t109", "CreateFrame('SimpleHTML')"),
    ("t110", "CreateFrame('EditBox', 'PGEdit')"),
    ("t111", "CreateFrame('CheckButton')"),
    ("t112", "CreateFrame('Button')"),
];

/// One object per [`WIDGET_PROBES`] row that this client can make, as the global holding it.
fn make_widgets(
    s: &benilla_ui::script::UiScript,
) -> std::collections::HashMap<&'static str, String> {
    let mut made = std::collections::HashMap::new();
    for (table, expr) in WIDGET_PROBES {
        let var = format!("PGW_{table}");
        if s.run(&format!("{var} = {expr}")).is_ok()
            && s.eval::<bool>(&format!("return {var} ~= nil"))
                .unwrap_or(false)
        {
            made.insert(*table, var);
        }
    }
    // Every class but the control point, which a Path makes, is an object benilla has.
    assert!(
        made.len() >= WIDGET_PROBES.len() - 1,
        "only {} widget classes could be instantiated — the probe set is broken",
        made.len()
    );
    made
}

/// The `table_kind = method` rows, split into columns.
fn method_rows(text: &str) -> impl Iterator<Item = Vec<&str>> {
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("name\t"))
        .map(|l| l.split('\t').collect::<Vec<&str>>())
        .filter(|f| f.len() >= 9 && f[2] == "method")
}

/// Whether a row's kinds are fit for a kind gate: an `agree` trace that saw every push.
fn kinds_fit(f: &[&str]) -> bool {
    f[8] == "agree" && !(f.len() > 9 && f[9].contains("delegated-push"))
}

/// The widget arity gate: the global gate's rules, with each method called on an instance of its
/// own registrar table's class.
#[test]
fn every_widget_method_answers_the_reference_s_return_arity() {
    let tsv = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/3.3.5a-shapes.tsv"
    );
    let text = std::fs::read_to_string(tsv).expect("reference/3.3.5a-shapes.tsv");

    let s = vm();
    let made = make_widgets(&s);

    let mut checked = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    for f in method_rows(&text) {
        let (name, table) = (f[0], f[1]);
        if !["Get", "Is", "Has", "Can", "Num"]
            .iter()
            .any(|p| name.starts_with(p))
        {
            continue;
        }
        let Some(obj) = made.get(table) else { continue };
        let Some(want) = arity_set(f[5], f[6]) else {
            continue;
        };

        let probe = format!(
            "if type({obj}.{name}) ~= 'function' then return -1 end \
             local ok, n = pcall(function() return (function(...) return arg.n end)({obj}:{name}()) end) \
             if not ok then return -1 end return n"
        );
        let Ok(got) = s.eval::<i64>(&probe) else {
            continue;
        };
        if got < 0 {
            continue;
        }
        checked += 1;
        if !want.contains(&(got as usize)) {
            if WIDGET_NOT_YET_ASSERTED.contains(&name) {
                continue;
            }
            mismatches.push(format!(
                "{obj}:{name} answers {got}, reference states {want:?}"
            ));
        }
    }

    assert!(
        checked >= WIDGET_ARITY_FLOOR,
        "the widget gate measured only {checked} methods — it has stopped covering anything"
    );
    assert!(
        mismatches.is_empty(),
        "{} of {checked} probed widget methods answer the wrong number of values:\n  {}",
        mismatches.len(),
        mismatches.join("\n  ")
    );
}

/// Splits a `kinds` cell into alternatives of per-slot kinds: `(nil) | (number)` is two of one
/// slot, and `()` yields none, so the kind gates skip a verb that answers nothing.
fn kind_alternatives(cell: &str) -> Vec<Vec<String>> {
    cell.split('|')
        .map(|alt| {
            alt.trim()
                .trim_start_matches('(')
                .trim_end_matches(')')
                .split(',')
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
                .collect::<Vec<String>>()
        })
        .filter(|a| !a.is_empty())
        .collect()
}

/// Whether an observed Lua `type()` satisfies one reference slot. `string?` is string-or-nil:
/// `lua_pushstring` pushes nil for a NULL pointer, and the harvest writes plain `string` only for a
/// constant literal. `any` and `value` are slots the trace could not narrow.
fn slot_accepts(want: &str, got: &str) -> bool {
    match want {
        "any" | "value" => true,
        "string?" => got == "string" || got == "nil",
        w => w == got,
    }
}

/// The Lua helper the kind gates probe through: the `type()` of every returned value, in order,
/// comma-joined. It counts with `arg.n`: a table constructor drops trailing nils, and `...` as a
/// value is a syntax error in 5.0 (no `TK_DOTS` arm in `simpleexp`), which this VM's parser keeps.
const KIND_HELPER: &str = "function PGKinds(...) \
     local o = {} for i = 1, arg.n do o[i] = type(arg[i]) end \
     return table.concat(o, ',') end";

/// Every registered global whose return kinds the reference states trustworthily, answered by
/// this client, under the arity gate's narrowings.
#[test]
fn every_query_binding_answers_the_reference_s_return_kinds() {
    let all = rows();
    let mut by_name: std::collections::HashMap<&str, Vec<&Row>> = std::collections::HashMap::new();
    for r in &all {
        by_name.entry(&r.name).or_default().push(r);
    }

    let mut s = vm();
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Shapeprobe".into()),
            level: 60,
            ..Default::default()
        }),
    );
    s.run(KIND_HELPER).expect("kind helper");

    let mut checked = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    for (name, rs) in &by_name {
        if !["Get", "Is", "Has", "Can", "Unit", "Num"]
            .iter()
            .any(|p| name.starts_with(p))
        {
            continue;
        }
        let candidates: Vec<&&Row> = if rs.len() == 1 {
            rs.iter().collect()
        } else {
            rs.iter().filter(|r| r.table != GLUE_TABLE).collect()
        };
        let Some(first) = candidates.first() else {
            continue;
        };
        if candidates.iter().any(|r| r.kinds != first.kinds) {
            continue;
        }
        let want = kind_alternatives(&first.kinds);
        if want.is_empty() {
            continue;
        }

        let call = if name.starts_with("Unit") {
            format!(r#"{name}("player")"#)
        } else {
            format!("{name}()")
        };
        let probe = format!(
            "if type({name}) ~= 'function' then return '?' end \
             local ok, s = pcall(function() return PGKinds({call}) end) \
             if not ok then return '?' end return s"
        );
        let Ok(got) = s.eval::<String>(&probe) else {
            continue;
        };
        if got == "?" {
            continue;
        }
        let got: Vec<&str> = got.split(',').filter(|k| !k.is_empty()).collect();
        // An arity fault is the arity gate's to report.
        if !want.iter().any(|alt| alt.len() == got.len()) {
            continue;
        }
        checked += 1;
        let agrees = want.iter().any(|alt| {
            alt.len() == got.len()
                && alt
                    .iter()
                    .zip(&got)
                    .all(|(w, g)| slot_accepts(w.as_str(), g))
        });
        let observed = got.join(",");
        if !agrees {
            if let Some((_, known, _)) = KINDS_NOT_YET_ASSERTED.iter().find(|(n, ..)| n == name) {
                assert_eq!(
                    *known, observed,
                    "{name}'s not-yet-asserted row is stale: it now answers ({observed})"
                );
                continue;
            }
            mismatches.push(format!(
                "{name}: answers ({observed}), reference states {}",
                first.kinds
            ));
        } else {
            assert!(
                !KINDS_NOT_YET_ASSERTED.iter().any(|(n, ..)| n == name),
                "{name} now agrees with the reference and must come off KINDS_NOT_YET_ASSERTED"
            );
        }
    }

    // A floor, not a target.
    assert!(
        checked >= KIND_FLOOR,
        "the kind gate measured only {checked} bindings — it has stopped covering anything"
    );
    assert!(
        mismatches.is_empty(),
        "{} of {checked} probed bindings answer values of the wrong kind:\n  {}",
        mismatches.len(),
        mismatches.join("\n  ")
    );
}

/// The widget kind gate, on an instance of each method's own registrar class. Few method rows
/// are fit for it: most widget bindings push through a callee the trace does not follow.
#[test]
fn every_widget_method_answers_the_reference_s_return_kinds() {
    let tsv = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/3.3.5a-shapes.tsv"
    );
    let text = std::fs::read_to_string(tsv).expect("reference/3.3.5a-shapes.tsv");

    let s = vm();
    s.run(KIND_HELPER).expect("kind helper");
    let made = make_widgets(&s);

    let mut checked = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    for f in method_rows(&text) {
        if arity_set(f[5], f[6]).is_none() || !kinds_fit(&f) {
            continue;
        }
        let (name, table) = (f[0], f[1]);
        if !["Get", "Is", "Has", "Can", "Num"]
            .iter()
            .any(|p| name.starts_with(p))
        {
            continue;
        }
        let Some(obj) = made.get(table) else { continue };
        let want = kind_alternatives(f[7]);
        if want.is_empty() {
            continue;
        }

        let probe = format!(
            "if type({obj}.{name}) ~= 'function' then return '?' end \
             local ok, s = pcall(function() return PGKinds({obj}:{name}()) end) \
             if not ok then return '?' end return s"
        );
        let Ok(got) = s.eval::<String>(&probe) else {
            continue;
        };
        if got == "?" {
            continue;
        }
        let got: Vec<&str> = got.split(',').filter(|k| !k.is_empty()).collect();
        if !want.iter().any(|alt| alt.len() == got.len()) {
            continue;
        }
        checked += 1;
        let agrees = want.iter().any(|alt| {
            alt.len() == got.len()
                && alt
                    .iter()
                    .zip(&got)
                    .all(|(w, g)| slot_accepts(w.as_str(), g))
        });
        let observed = got.join(",");
        if !agrees {
            if let Some((_, known, _)) = WIDGET_KINDS_NOT_YET_ASSERTED
                .iter()
                .find(|(n, ..)| *n == name)
            {
                assert_eq!(
                    *known, observed,
                    "{name}'s widget not-yet-asserted row is stale: it now answers ({observed})"
                );
                continue;
            }
            mismatches.push(format!(
                "{obj}:{name} answers ({observed}), reference states {}",
                f[7]
            ));
        }
    }

    assert!(
        checked >= WIDGET_KIND_FLOOR,
        "the widget kind gate measured only {checked} methods — it has stopped covering anything"
    );
    assert!(
        mismatches.is_empty(),
        "{} of {checked} probed widget methods answer values of the wrong kind:\n  {}",
        mismatches.len(),
        mismatches.join("\n  ")
    );
}

/// Every 3.3.5a row whose catalogued returns are the numeric flag, `(nil) | (number)`, of a global
/// or a method: `(name, is_method)`. A row that can also push a boolean is not a flag row.
fn nil_number_flag_rows() -> Vec<(String, bool)> {
    let tsv = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/3.3.5a-shapes.tsv"
    );
    let text = std::fs::read_to_string(tsv).expect("reference/3.3.5a-shapes.tsv");
    let mut rows: Vec<(String, bool)> = text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("name\t"))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            let is_method = match *f.get(2)? {
                "global" => false,
                "method" => true,
                _ => return None,
            };
            let kinds = *f.get(7)?;
            (*f.get(8)? == "agree"
                && *f.get(1)? != GLUE_TABLE
                && kinds.contains("(nil)")
                && kinds.contains("(number)")
                && !kinds.contains("boolean"))
            .then(|| (f[0].to_string(), is_method))
        })
        .collect();
    rows.sort();
    rows.dedup();
    rows
}

/// A numeric flag pushes `1` or nil, never a Lua boolean: `x == 1`, `tostring(x)` and a saved
/// variable tell them apart. Every row the catalogue marks, under each call shape that returns.
#[test]
fn no_nil_number_flag_answers_a_lua_boolean() {
    /// Calls a broken argument can reach but no real one does: their result is a usage answer.
    const OBJECTS: &[(&str, &str)] = &[
        ("Frame", "PGFrame"),
        ("Button", "PGButton"),
        ("CheckButton", "PGCheck"),
        ("Slider", "PGSlider"),
        ("StatusBar", "PGBar"),
        ("EditBox", "PGBox"),
        ("ScrollFrame", "PGScroll"),
        ("GameTooltip", "PGTip"),
        ("MessageFrame", "PGMsg"),
        ("ScrollingMessageFrame", "PGSmf"),
        ("SimpleHTML", "PGHtml"),
        ("Model", "PGModel"),
        ("PlayerModel", "PGPlayerModel"),
        ("Cooldown", "PGCooldown"),
        ("ColorSelect", "PGColor"),
    ];
    let rows = nil_number_flag_rows();
    let s = vm();
    s.run(KIND_HELPER).expect("kind helper");
    for (kind, name) in OBJECTS {
        let _ = s.run(&format!(
            "{name} = CreateFrame(\"{kind}\", \"{name}\", UIParent)"
        ));
    }
    let _ = s.run("PGTex = PGFrame:CreateTexture() PGFont = PGFrame:CreateFontString()");
    let receivers: Vec<&str> = OBJECTS
        .iter()
        .map(|&(_, n)| n)
        .chain(["PGTex", "PGFont"])
        .collect();
    const ARGS: &[&str] = &[
        "",
        "\"player\"",
        "1",
        "1, 1",
        "\"player\", 1",
        "\"OnClick\"",
        "\"Frame\"",
    ];
    let mut checked = 0usize;
    let mut booleans = Vec::new();
    let mut probe = |call: String, shown: String| {
        let chunk = format!(
            "local ok, s = pcall(function() return PGKinds({call}) end) \
             if not ok then return '?' end return s"
        );
        let Ok(got) = s.eval::<String>(&chunk) else {
            return false;
        };
        if got == "?" {
            return false;
        }
        checked += 1;
        if got.split(',').any(|k| k == "boolean") {
            booleans.push(format!("{shown} answers ({got})"));
        }
        true
    };
    for (name, is_method) in &rows {
        for args in ARGS {
            if *is_method {
                for obj in &receivers {
                    let exists = s
                        .eval::<bool>(&format!("return type({obj}.{name}) == 'function'"))
                        .unwrap_or(false);
                    if exists {
                        probe(
                            format!("{obj}:{name}({args})"),
                            format!("{obj}:{name}({args})"),
                        );
                    }
                }
            } else {
                let exists = s
                    .eval::<bool>(&format!("return type({name}) == 'function'"))
                    .unwrap_or(false);
                if exists && probe(format!("{name}({args})"), format!("{name}({args})")) {
                    break;
                }
            }
        }
    }
    assert!(
        checked >= 150,
        "the flag gate measured only {checked} calls: it has stopped covering anything"
    );
    booleans.sort();
    booleans.dedup();
    assert!(
        booleans.is_empty(),
        "{} calls on `(nil) | (number)` rows answer a Lua boolean:\n  {}",
        booleans.len(),
        booleans.join("\n  ")
    );
}

/// Argument tuples the boolean gate tries in order until a call returns. A plausible wrong
/// argument can change which kinds come back, which is why the kind gate passes only a unit
/// token, but it never turns `1` into `true`.
const ARG_LADDER: &[&str] = &["", "\"player\"", "1", "1, 1", "\"player\", 1"];

/// Whether an answer holding a Lua boolean is one the row's kinds allow: some alternative of the
/// answer's length takes a boolean, or anything, in each boolean slot.
fn boolean_allowed(kinds: &str, got: &[&str]) -> bool {
    kind_alternatives(kinds).iter().any(|alt| {
        alt.len() == got.len()
            && alt
                .iter()
                .zip(got)
                .all(|(w, g)| *g != "boolean" || matches!(w.as_str(), "boolean" | "any" | "value"))
    })
}

/// 3.3.5a pushes a Lua boolean from some bindings and `1` or nil from others, which `x == 1`,
/// `tostring(x)` and a saved variable tell apart. Every query binding whose row's kinds are fit
/// to gate answers a boolean only where they allow one, under the argument ladder that reaches the
/// predicates the kind gate's bare call skips.
#[test]
fn no_query_binding_answers_a_lua_boolean_its_row_lacks() {
    let all = rows();
    let mut s = vm();
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Shapeprobe".into()),
            level: 60,
            ..Default::default()
        }),
    );
    s.run(KIND_HELPER).expect("kind helper");

    let mut by_name: std::collections::BTreeMap<&str, Vec<&Row>> = Default::default();
    for r in &all {
        if ["Get", "Is", "Has", "Can", "Unit", "Num"]
            .iter()
            .any(|p| r.name.starts_with(p))
            && r.table != GLUE_TABLE
        {
            by_name.entry(&r.name).or_default().push(r);
        }
    }

    let mut checked = 0usize;
    let mut booleans: Vec<String> = Vec::new();
    for (name, rs) in by_name {
        // One fit trace for the name, or nothing to hold it to.
        let Some(kinds) = rs
            .first()
            .map(|r| r.kinds.as_str())
            .filter(|k| !k.is_empty())
        else {
            continue;
        };
        if rs.iter().any(|r| r.kinds != kinds) {
            continue;
        }
        for args in ARG_LADDER {
            let probe = format!(
                "if type({name}) ~= 'function' then return '?' end \
                 local ok, s = pcall(function() return PGKinds({name}({args})) end) \
                 if not ok then return '?' end return s"
            );
            let Ok(got) = s.eval::<String>(&probe) else {
                continue;
            };
            if got == "?" {
                continue;
            }
            checked += 1;
            let slots: Vec<&str> = got.split(',').filter(|k| !k.is_empty()).collect();
            if slots.contains(&"boolean") && !boolean_allowed(kinds, &slots) {
                booleans.push(format!(
                    "{name}({args}) answers ({got}), reference states {kinds}"
                ));
            }
            break;
        }
    }

    assert!(
        checked >= BOOLEAN_FLOOR,
        "the boolean gate measured only {checked} bindings — it has stopped covering anything"
    );
    assert!(
        booleans.is_empty(),
        "{} bindings answer a Lua boolean their row does not push:\n  {}",
        booleans.len(),
        booleans.join("\n  ")
    );
}

/// The widget boolean gate. Its argument ladder reaches the predicates the kind gate skips for
/// raising without one, such as `HasScript("OnClick")` and `IsObjectType("Frame")`.
#[test]
fn no_widget_method_answers_a_lua_boolean_its_row_lacks() {
    let tsv = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/3.3.5a-shapes.tsv"
    );
    let text = std::fs::read_to_string(tsv).expect("reference/3.3.5a-shapes.tsv");

    let s = vm();
    s.run(KIND_HELPER).expect("kind helper");
    let made = make_widgets(&s);

    // A widget predicate takes a script or object-type name more often than a number.
    const WIDGET_ARGS: &[&str] = &["", "\"OnClick\"", "\"Frame\"", "1", "1, 1"];

    let mut checked = 0usize;
    let mut booleans: Vec<String> = Vec::new();
    for f in method_rows(&text) {
        if arity_set(f[5], f[6]).is_none() || !kinds_fit(&f) {
            continue;
        }
        let (name, table) = (f[0], f[1]);
        if !["Get", "Is", "Has", "Can", "Num"]
            .iter()
            .any(|p| name.starts_with(p))
        {
            continue;
        }
        let Some(obj) = made.get(table) else { continue };
        for args in WIDGET_ARGS {
            let probe = format!(
                "if type({obj}.{name}) ~= 'function' then return '?' end \
                 local ok, s = pcall(function() return PGKinds({obj}:{name}({args})) end) \
                 if not ok then return '?' end return s"
            );
            let Ok(got) = s.eval::<String>(&probe) else {
                continue;
            };
            if got == "?" {
                continue;
            }
            checked += 1;
            let slots: Vec<&str> = got.split(',').filter(|k| !k.is_empty()).collect();
            if slots.contains(&"boolean") && !boolean_allowed(f[7], &slots) {
                booleans.push(format!(
                    "{obj}:{name}({args}) answers ({got}), reference states {}",
                    f[7]
                ));
            }
            break;
        }
    }

    assert!(
        checked >= WIDGET_BOOLEAN_FLOOR,
        "the widget boolean gate measured only {checked} methods — it has stopped covering anything"
    );
    assert!(
        booleans.is_empty(),
        "{} widget methods answer a Lua boolean their row does not push:\n  {}",
        booleans.len(),
        booleans.join("\n  ")
    );
}
