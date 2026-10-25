//! The calendar's holidays as 12340 lays them out: `Holidays.dbc` (55 columns in 0xdc-byte
//! records, `HolidaysRec::Read` `0x8b00f0`), and the name and description tables it points into,
//! `HolidayNames.dbc` and `HolidayDescriptions.dbc` (18 columns in 0x48 each, an id and a
//! localized string). The readers are `CGCalendar`'s (`GetHolidayName` `0x5b9430`,
//! `GetHolidayDescription` `0x5b9490`, `InitHolidays` `0x5c04f0`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{i32_at, parse, str_at};

const HOLIDAYS: &str = "DBFilesClient\\Holidays.dbc";
const HOLIDAY_NAMES: &str = "DBFilesClient\\HolidayNames.dbc";
const HOLIDAY_DESCRIPTIONS: &str = "DBFilesClient\\HolidayDescriptions.dbc";

/// A localized column's enUS slot and the 16 columns after it.
const LOC_COLUMNS: usize = 17;

/// One `Holidays.dbc` row, its name and description joined.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Holiday {
    pub id: i32,
    /// Each date slot's length in hours; 0 is a day.
    pub durations: [i32; 10],
    /// Packed `WowTime` words; a field left all ones repeats (a weekday alone is weekly).
    pub dates: [i32; 26],
    /// A bit per client Region that sees the holiday; 0 is every Region.
    pub region: i32,
    pub looping: i32,
    /// Each date slot's sides: 1 Alliance, 2 Horde.
    pub calendar_flags: [i32; 10],
    /// `HolidayNames.dbc`'s enUS string; `None` when the row is missing.
    pub name: Option<String>,
    pub description: Option<String>,
    pub texture: String,
    pub priority: i32,
    /// 0 weekly, 1 Darkmoon, 2 battleground, -1 none: the calendar's filter.
    pub filter_type: i32,
    /// `0x1`: the dates are in the server's zone.
    pub flags: i32,
}

fn ints(s: &mut Schema, prefix: &str, n: usize) {
    for i in 0..n {
        s.add_field(SchemaField::new(format!("{prefix}{i}"), FieldType::Int32));
    }
}

pub(crate) fn holidays_schema() -> Schema {
    let mut s = Schema::new("Holidays");
    s.add_field(SchemaField::new("ID", FieldType::Int32));
    ints(&mut s, "Duration", 10);
    ints(&mut s, "Date", 26);
    for name in ["Region", "Looping"] {
        s.add_field(SchemaField::new(name, FieldType::Int32));
    }
    ints(&mut s, "CalendarFlags", 10);
    for name in ["HolidayNameID", "HolidayDescriptionID"] {
        s.add_field(SchemaField::new(name, FieldType::Int32));
    }
    s.add_field(SchemaField::new("TextureFilename", FieldType::String));
    for name in ["Priority", "CalendarFilterType", "Flags"] {
        s.add_field(SchemaField::new(name, FieldType::Int32));
    }
    s
}

/// An id and one localized string, both tables' layout.
fn text_schema(mut s: Schema) -> Schema {
    s.add_field(SchemaField::new("ID", FieldType::Int32));
    s.add_field(SchemaField::new("Text", FieldType::String));
    for i in 1..LOC_COLUMNS {
        s.add_field(SchemaField::new(format!("Text{i}"), FieldType::UInt32));
    }
    s
}

pub(crate) fn names_schema() -> Schema {
    text_schema(Schema::new("HolidayNames"))
}

pub(crate) fn descriptions_schema() -> Schema {
    text_schema(Schema::new("HolidayDescriptions"))
}

fn read_texts(chain: &mut Chain, path: &str, schema: Schema) -> Result<HashMap<i32, String>> {
    let what = path.trim_start_matches("DBFilesClient\\").to_string();
    let bytes = chain
        .read_file(path)
        .with_context(|| format!("reading {what}"))?;
    let rs = parse(&bytes, schema, &what)?;
    Ok(rs
        .records()
        .iter()
        .map(|r| {
            (
                i32_at(r, 0).unwrap_or(0),
                str_at(&rs, r, 1).unwrap_or_default(),
            )
        })
        .collect())
}

/// `N` int columns from `start`.
fn run<const N: usize>(r: &benilla_dbc::Record, start: usize) -> [i32; N] {
    std::array::from_fn(|k| i32_at(r, start + k).unwrap_or(0))
}

/// Read the three holiday tables off the patch chain, `Holidays.dbc` in file order, the order
/// `InitHolidays` walks.
pub fn load_holidays(chain: &mut Chain) -> Result<Vec<Holiday>> {
    let names = read_texts(chain, HOLIDAY_NAMES, names_schema())?;
    let descriptions = read_texts(chain, HOLIDAY_DESCRIPTIONS, descriptions_schema())?;
    let bytes = chain.read_file(HOLIDAYS).context("reading Holidays.dbc")?;
    let rs = parse(&bytes, holidays_schema(), "Holidays.dbc")?;
    let int = |r, i| i32_at(r, i).unwrap_or(0);
    Ok(rs
        .records()
        .iter()
        .map(|r| Holiday {
            id: int(r, 0),
            durations: run(r, 1),
            dates: run(r, 11),
            region: int(r, 37),
            looping: int(r, 38),
            calendar_flags: run(r, 39),
            name: names.get(&int(r, 49)).cloned(),
            description: descriptions.get(&int(r, 50)).cloned(),
            texture: str_at(&rs, r, 51).unwrap_or_default(),
            priority: int(r, 52),
            filter_type: int(r, 53),
            flags: int(r, 54),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The widths 12340's loaders check: 55 columns in 220 bytes, 18 in 72.
    #[test]
    fn the_schemas_hold_the_reference_widths() {
        let h = holidays_schema();
        assert_eq!((h.expanded_len(), h.record_width()), (55, 220));
        let t = names_schema();
        assert_eq!((t.expanded_len(), t.record_width()), (18, 72));
    }

    /// Every named holiday has a description and a slot some side sees, its filter is one the
    /// calendar knows, and each Darkmoon Faire has its texture: a one-column slip breaks these.
    #[test]
    fn the_install_holidays_are_named_and_sided() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let rows = load_holidays(&mut chain).expect("holidays");
        let named: Vec<&Holiday> = rows.iter().filter(|h| h.name.is_some()).collect();
        assert!(named.len() > 20, "{} named holidays", named.len());
        for h in &named {
            assert!(
                h.description.is_some(),
                "holiday {} has no description",
                h.id
            );
            if h.filter_type == 1 {
                assert!(h.texture.starts_with("Calendar_"), "holiday {}", h.id);
            }
            assert!(
                h.calendar_flags.iter().any(|f| f & 3 != 0),
                "holiday {} shows to no side",
                h.id
            );
            assert!((-1..=2).contains(&h.filter_type), "holiday {}", h.id);
        }
    }
}
