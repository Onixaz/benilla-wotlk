//! `SpellDescriptionVariables.dbc`: the `$name=` definitions a spell's description reads through
//! `$<name>`, by the spell's `SpellDescriptionVariablesID`
//! ([`crate::spells::SpellDisplay::description_variables_id`]). The table `0x57abc0` reads is the
//! DB at `0xad47b4`, loaded from `DBFilesClient\SpellDescriptionVariables.dbc`.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};

/// `SpellDescriptionVariables.dbc`, by row id.
#[derive(Default)]
pub struct SpellDescriptionVariablesCatalog {
    rows: HashMap<u32, String>,
}

impl SpellDescriptionVariablesCatalog {
    /// The definitions text of row `id`.
    pub fn get(&self, id: u32) -> Option<&str> {
        self.rows.get(&id).map(String::as_str)
    }

    /// Fixture constructor for tests; the live path is [`load_spell_description_variables`].
    pub fn from_rows(rows: HashMap<u32, String>) -> Self {
        Self { rows }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

pub(crate) fn schema() -> Schema {
    let mut schema = Schema::new("SpellDescriptionVariables");
    schema.add_field(SchemaField::new("ID", FieldType::UInt32));
    schema.add_field(SchemaField::new("Variables", FieldType::String));
    schema
}

/// Load `SpellDescriptionVariables.dbc` off the patch chain.
pub fn load_spell_description_variables(
    chain: &mut Chain,
) -> Result<SpellDescriptionVariablesCatalog> {
    let bytes = chain
        .read_file("DBFilesClient\\SpellDescriptionVariables.dbc")
        .context("reading SpellDescriptionVariables.dbc")?;
    let set = parse(&bytes, schema(), "SpellDescriptionVariables.dbc")?;
    let mut rows = HashMap::new();
    for r in set.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        if let Some(text) = str_at(&set, r, 1) {
            rows.insert(id, text);
        }
    }
    Ok(SpellDescriptionVariablesCatalog { rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row defines at least one variable, as `$name=` followed by a `$` form.
    #[test]
    fn every_row_defines_a_variable() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let vars = load_spell_description_variables(&mut chain).expect("load");
        assert!(!vars.is_empty());
        for (id, text) in &vars.rows {
            assert!(
                text.trim_start().starts_with('$') && text.contains('='),
                "row {id} has no definition"
            );
        }
    }
}
