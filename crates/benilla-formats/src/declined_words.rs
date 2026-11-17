//! `DeclinedWord.dbc` and `DeclinedWordCases.dbc`, the Russian case forms the `|3` grammar rule
//! looks a word up in. 12340 loads both on the rule's first use (`0x7e08e0`).

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const DECLINED_WORD: &str = "DBFilesClient\\DeclinedWord.dbc";
const DECLINED_WORD_CASES: &str = "DBFilesClient\\DeclinedWordCases.dbc";

/// Both tables in file order, the order 12340's index build walks them.
#[derive(Debug, Default, Clone)]
pub struct DeclinedWordRows {
    /// `(id, word)` per `DeclinedWord` row.
    pub words: Vec<(u32, String)>,
    /// `(word id, case, form)` per `DeclinedWordCases` row.
    pub cases: Vec<(u32, u32, String)>,
}

pub(crate) fn declined_word_schema() -> Schema {
    let mut s = Schema::new("DeclinedWord");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Word", FieldType::String));
    s
}

pub(crate) fn declined_word_cases_schema() -> Schema {
    let mut s = Schema::new("DeclinedWordCases");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("DeclinedWordID", FieldType::UInt32));
    s.add_field(SchemaField::new("CaseIndex", FieldType::UInt32));
    s.add_field(SchemaField::new("DeclinedWord", FieldType::String));
    s
}

/// Read both tables; an empty string reads as `""`, as the record's pointer does.
pub fn load_declined_words(chain: &mut Chain) -> Result<DeclinedWordRows> {
    let bytes = chain
        .read_file(DECLINED_WORD)
        .with_context(|| format!("reading {DECLINED_WORD}"))?;
    let table = parse(&bytes, declined_word_schema(), "DeclinedWord.dbc")?;
    let words = table
        .records()
        .iter()
        .filter_map(|r| Some((u32_at(r, 0)?, str_at(&table, r, 1).unwrap_or_default())))
        .collect();

    let bytes = chain
        .read_file(DECLINED_WORD_CASES)
        .with_context(|| format!("reading {DECLINED_WORD_CASES}"))?;
    let table = parse(
        &bytes,
        declined_word_cases_schema(),
        "DeclinedWordCases.dbc",
    )?;
    let cases = table
        .records()
        .iter()
        .filter_map(|r| {
            Some((
                u32_at(r, 1)?,
                u32_at(r, 2)?,
                str_at(&table, r, 3).unwrap_or_default(),
            ))
        })
        .collect();
    Ok(DeclinedWordRows { words, cases })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_cases_follow_their_words_in_id_order() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let rows = load_declined_words(&mut chain).expect("load");
        assert!(!rows.words.is_empty() && !rows.cases.is_empty());
        // The index build is a merge walk, which needs both tables sorted by word id, and the
        // case walk needs each word's cases ascending.
        assert!(rows.words.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(rows
            .cases
            .windows(2)
            .all(|w| (w[0].0, w[0].1) <= (w[1].0, w[1].1)));
        assert!(rows.cases.iter().all(|c| c.1 >= 1));
    }
}
