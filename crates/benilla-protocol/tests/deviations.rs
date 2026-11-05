//! `docs/DEVIATIONS.md` against the decoder: every incoming 12340 opcode with no parse arm is
//! listed under "Not decoded", each listed one still has none, and each "Read and dropped" row
//! has an arm.

use std::collections::BTreeSet;
use std::path::Path;

use benilla_protocol::messages::{self, ServerPacket};

fn repo_file(path: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::read_to_string(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn hex(s: &str) -> u16 {
    u16::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or_else(|_| panic!("opcode {s}"))
}

/// True when the opcode has no parse arm: an empty body comes back unmodelled, not read or short.
fn undecoded(opcode: u16) -> bool {
    matches!(
        messages::parse_server(opcode, &[]),
        Ok(ServerPacket::Other { .. })
    )
}

/// The page's rows as `(section, opcode, name)`; the section is the last `## ` heading.
fn rows(page: &str) -> Vec<(String, u16, String)> {
    let mut section = String::new();
    let mut out = Vec::new();
    for line in page.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            section = h.to_string();
        }
        let cells: Vec<&str> = line
            .split('|')
            .map(|c| c.trim().trim_matches('`'))
            .collect();
        if cells.len() > 3 && cells[1].starts_with("0x") {
            out.push((section.clone(), hex(cells[1]), cells[2].to_string()));
        }
    }
    out
}

#[test]
fn the_deviation_page_lists_exactly_the_opcodes_with_no_decoder() {
    let reference: BTreeSet<u16> = repo_file("reference/3.3.5a-opcodes.tsv")
        .lines()
        .filter(|l| l.starts_with("0x"))
        .map(|l| hex(l.split('\t').next().unwrap()))
        .collect();
    let page = rows(&repo_file("docs/DEVIATIONS.md"));

    for (section, opcode, name) in &page {
        assert!(
            reference.contains(opcode),
            "{opcode:#05x} is no 12340 opcode"
        );
        assert_eq!(
            messages::opcode_name(*opcode),
            Some(name.as_str()),
            "{opcode:#05x}'s name"
        );
        match section.as_str() {
            "Read and dropped" => assert!(!undecoded(*opcode), "{name} lost its decoder"),
            "Not decoded" => assert!(undecoded(*opcode), "{name} has a decoder: drop its row"),
            other => panic!("{name} under an unknown section {other:?}"),
        }
    }

    let listed: BTreeSet<u16> = page
        .iter()
        .filter(|(section, ..)| section == "Not decoded")
        .map(|&(_, opcode, _)| opcode)
        .collect();
    let missing: Vec<String> = reference
        .iter()
        .filter(|&&o| undecoded(o) && !listed.contains(&o))
        .map(|&o| format!("{o:#05x} {}", messages::opcode_name(o).unwrap_or("?")))
        .collect();
    assert!(
        missing.is_empty(),
        "no decoder and no row in docs/DEVIATIONS.md: {missing:?}"
    );
}
