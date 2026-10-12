//! Every DBC table has one schema in this crate, which each of its readers shares: a second copy
//! is how a reader goes on parsing a layout the table no longer has.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The string literal opening `rest`, past leading whitespace.
fn leading_literal(rest: &str) -> Option<&str> {
    let rest = rest.trim_start().strip_prefix('"')?;
    Some(&rest[..rest.find('"')?])
}

/// The tables a source file builds a schema for: the name a `Schema::new` or `…schema(` call
/// opens with, and the file a `load_id_name_table` call reads.
fn schema_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for (i, m) in text.match_indices("chema(") {
        if let Some(name) = leading_literal(&text[i + m.len()..]) {
            names.push(name.to_owned());
        }
    }
    for (i, m) in text.match_indices("Schema::new(") {
        if let Some(name) = leading_literal(&text[i + m.len()..]) {
            names.push(name.to_owned());
        }
    }
    for (i, _) in text.match_indices("load_id_name_table(") {
        let call = &text[i..];
        let call = &call[..call.find(')').unwrap_or(call.len())];
        if let Some(at) = call.find("DBFilesClient\\\\") {
            let file = &call[at + "DBFilesClient\\\\".len()..];
            if let Some(end) = file.find(".dbc") {
                names.push(file[..end].to_owned());
            }
        }
    }
    names
}

#[test]
fn the_scan_sees_each_way_a_schema_is_built() {
    let src = "Schema::new(\"A\"); n_u32_schema(\n    \"B\", 3); load_id_name_table(chain, \
               \"DBFilesClient\\\\C.dbc\", 1, 10, \"C\")";
    assert_eq!(schema_names(src), ["B", "A", "C"]);
}

#[test]
fn every_table_is_declared_once() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let mut sites: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for file in &files {
        // A test module's fixture builds the loader's own constants into a synthetic table.
        if file.file_name().is_some_and(|n| n == "tests.rs") {
            continue;
        }
        let text = std::fs::read_to_string(file).unwrap();
        let at = file.strip_prefix(&src).unwrap().display().to_string();
        for name in schema_names(&text) {
            sites.entry(name).or_default().push(at.clone());
        }
    }
    assert!(sites.len() > 50, "the scan found the schemas: {sites:?}");
    let doubled: Vec<_> = sites.iter().filter(|(_, at)| at.len() > 1).collect();
    assert!(
        doubled.is_empty(),
        "tables with more than one schema: {doubled:?}"
    );
}
