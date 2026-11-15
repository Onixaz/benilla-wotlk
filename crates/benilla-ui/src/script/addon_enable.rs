//! One character's addon enable hash, the table an `ADDONSTATELIST` node holds at `+0x10`, with the
//! node's dirty byte (`+0xc`). Both the glue AddOns screen and the in-game verbs write through its
//! setter, and the writer saves a node only while it is dirty, one line per entry, so a character
//! that toggled nothing writes nothing and one that never set an addon keeps inheriting what the
//! realm's other characters chose.

use std::collections::HashMap;

/// A character's explicit enable rows: those the per-character load read from its `AddOns.txt`,
/// whether or not that addon is installed now, plus those the setter put there since. An addon with
/// no row here is not the character's choice, and the query answers it from the other characters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnableHash {
    /// In insertion order, each name in the spelling that first put it here.
    rows: Vec<(String, bool)>,
    /// Lowercased name to its row: every compare in the reference is case-insensitive.
    index: HashMap<String, usize>,
    /// The node's `+0xc`: set when a setter call changes the hash, cleared by a reload and by the
    /// writer as it saves.
    dirty: bool,
}

impl EnableHash {
    /// The loader's hash, clean. A name on two lines keeps its first spelling and takes the last
    /// value, as the reload looks each one up and overwrites its byte.
    pub fn from_rows(rows: impl IntoIterator<Item = (String, bool)>) -> Self {
        let mut hash = Self::default();
        for (name, on) in rows {
            hash.put(name, on);
        }
        hash
    }

    /// The account-wide file's rows standing in for a character with no file of its own: dirty, so
    /// the next save writes them as the character's (`0x5f9382`).
    pub fn from_account_rows(rows: impl IntoIterator<Item = (String, bool)>) -> Self {
        let mut hash = Self::from_rows(rows);
        hash.dirty = true;
        hash
    }

    /// The row for `name`, if this character has one.
    pub fn get(&self, name: &str) -> Option<bool> {
        self.index
            .get(&name.to_ascii_lowercase())
            .map(|&i| self.rows[i].1)
    }

    /// The setter on this node: a row with the same value is left alone; a changed row is stored
    /// and a missing one inserted, and either dirties the node.
    pub fn set(&mut self, name: &str, on: bool) {
        if self.get(name) == Some(on) {
            return;
        }
        self.put(name.to_string(), on);
        self.dirty = true;
    }

    /// Whether the writer would save this node.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Every row, in order.
    pub fn rows(&self) -> &[(String, bool)] {
        &self.rows
    }

    /// The writer's view of the node: every row when dirty, clearing the flag; `None` when clean,
    /// which writes nothing.
    pub fn take_dirty(&mut self) -> Option<Vec<(String, bool)>> {
        std::mem::take(&mut self.dirty).then(|| self.rows.clone())
    }

    fn put(&mut self, name: String, on: bool) {
        match self.index.get(&name.to_ascii_lowercase()) {
            Some(&i) => self.rows[i].1 = on,
            None => {
                self.index
                    .insert(name.to_ascii_lowercase(), self.rows.len());
                self.rows.push((name, on));
            }
        }
    }
}

/// The reference's addon-state list: a node per character, each holding that character's
/// [`EnableHash`]. The in-world load and the glue AddOns screen answer and set through it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnableLists {
    /// `(character, enable hash)` in character-list order.
    nodes: Vec<(String, EnableHash)>,
}

impl EnableLists {
    pub fn new(nodes: Vec<(String, EnableHash)>) -> Self {
        Self { nodes }
    }

    /// The aggregate (`0x5f7570`): explicit rows only, across every node; none is the default
    /// state's 2/0, all off 0, all on 2, mixed 1.
    pub fn aggregate(&self, addon: &str, default_state: bool) -> u8 {
        let (mut total, mut on) = (0usize, 0usize);
        for (_, hash) in &self.nodes {
            if let Some(v) = hash.get(addon) {
                total += 1;
                on += usize::from(v);
            }
        }
        tri_state(total, on, default_state)
    }

    /// The state query with the compute flag (`0x5f7d70`), over the named node or, with `None`,
    /// every node: a row counts as itself, a node without one takes the aggregate (0 off, 2 on, 1
    /// the default state). 0 none on, 2 all on, 1 some; no node counted is the default's 2/0.
    pub fn state(&self, addon: &str, default_state: bool, character: Option<&str>) -> u8 {
        let (mut total, mut on) = (0usize, 0usize);
        for (name, hash) in &self.nodes {
            if character.is_some_and(|c| !name.eq_ignore_ascii_case(c)) {
                continue;
            }
            let enabled = match hash.get(addon) {
                Some(v) => v,
                None => match self.aggregate(addon, default_state) {
                    0 => false,
                    2 => true,
                    _ => default_state,
                },
            };
            total += 1;
            on += usize::from(enabled);
            if character.is_some() {
                break;
            }
        }
        tri_state(total, on, default_state)
    }

    /// The setter (`0x5f90b0`): the named node, or every node with `None`; a name with no node is
    /// a no-op, as the setter never creates one.
    pub fn set(&mut self, addon: &str, character: Option<&str>, on: bool) {
        for (name, hash) in &mut self.nodes {
            if character.is_none_or(|c| name.eq_ignore_ascii_case(c)) {
                hash.set(addon, on);
                if character.is_some() {
                    break;
                }
            }
        }
    }

    /// The node a character name picks, any case.
    pub fn node(&self, character: &str) -> Option<&EnableHash> {
        self.nodes
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(character))
            .map(|(_, hash)| hash)
    }

    /// The writer's view (`0x5f76a0`): each dirty node's character and rows, clearing the flags.
    pub fn take_dirty(&mut self) -> Vec<(String, Vec<(String, bool)>)> {
        self.nodes
            .iter_mut()
            .filter_map(|(name, hash)| hash.take_dirty().map(|rows| (name.clone(), rows)))
            .collect()
    }
}

/// The 0/1/2 both walks end with: nothing counted is the default state's 2/0.
fn tri_state(total: usize, on: usize, default_state: bool) -> u8 {
    match (total, on) {
        (0, _) => 2 * u8::from(default_state),
        (_, 0) => 0,
        (t, o) if o == t => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::EnableHash;

    fn loaded() -> EnableHash {
        EnableHash::from_rows([("Kept".to_string(), true), ("Off".to_string(), false)])
    }

    #[test]
    fn a_loaded_hash_is_clean_and_keeps_the_first_spelling_with_the_last_value() {
        let h = EnableHash::from_rows([
            ("MyAddon".to_string(), true),
            ("myaddon".to_string(), false),
        ]);
        assert!(!h.is_dirty());
        assert_eq!(h.rows(), &[("MyAddon".to_string(), false)]);
        assert_eq!(h.get("MYADDON"), Some(false));
    }

    #[test]
    fn setting_a_row_to_its_own_value_does_not_dirty() {
        let mut h = loaded();
        h.set("kept", true);
        h.set("OFF", false);
        assert!(!h.is_dirty());
        assert_eq!(h.clone().take_dirty(), None, "a clean node writes nothing");
    }

    #[test]
    fn a_changed_row_dirties_and_the_writer_takes_every_row_once() {
        let mut h = loaded();
        h.set("Kept", false);
        assert!(h.is_dirty());
        assert_eq!(
            h.take_dirty(),
            Some(vec![
                ("Kept".to_string(), false),
                ("Off".to_string(), false)
            ])
        );
        assert!(!h.is_dirty(), "the writer clears the flag");
        assert_eq!(h.take_dirty(), None);
    }

    /// A miss inserts even when the value matches what the character inherited: the setter has
    /// no view of the aggregate, only of this node's table.
    #[test]
    fn a_missing_row_is_inserted_and_dirties() {
        let mut h = loaded();
        h.set("New", true);
        assert!(h.is_dirty());
        assert_eq!(h.get("new"), Some(true));
        assert_eq!(h.rows().len(), 3);
    }
}
