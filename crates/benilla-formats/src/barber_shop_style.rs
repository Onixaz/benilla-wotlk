//! `BarberShopStyle.dbc`: the barber shop's styles. 12340 reads it once, at world entry
//! (`0x52fe90`): every row of the player's race and of their sex or of sex `-1` goes into a list
//! per `Type` (0 hair style, 2 facial hair, 3 skin), in file order, and a style is found again by
//! its `Data`, the `PLAYER_BYTES` value it stands for (`0x52f760`). A row's price is its
//! `CostModifier` times the level's `gtBarberShopCostBase` value (`0x52e850`, `0x52eba0`).

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, i32_at, parse, str_at, u32_at};
use crate::Chain;

const BARBER_SHOP_STYLE: &str = "DBFilesClient\\BarberShopStyle.dbc";

/// The 16 locale slots of a localized string; a mask column follows them.
const LOCALES: usize = 16;

/// One `BarberShopStyle.dbc` row.
#[derive(Clone, Debug, PartialEq)]
pub struct BarberShopStyle {
    pub id: u32,
    /// 0 hair style, 2 facial hair, 3 skin; 1, hair colour, has no rows.
    pub style_type: u32,
    /// `DisplayName_Lang` at the enUS slot, `""` where the slot is empty.
    pub name: String,
    /// `Description_Lang` at the enUS slot, `""` where the slot is empty.
    pub description: String,
    pub cost_modifier: f32,
    pub race: i32,
    /// 0 male, 1 female, `-1` either.
    pub sex: i32,
    /// The `PLAYER_BYTES` value the style stands for: a hair or facial-hair variation, a skin.
    pub data: i32,
}

impl BarberShopStyle {
    /// Whether the world-entry filter (`0x52fe90`) keeps the row for a character.
    pub fn fits(&self, race: u8, sex: u8) -> bool {
        self.race == i32::from(race) && (self.sex == i32::from(sex) || self.sex == -1)
    }
}

/// `ID`, `Type`, the two localized strings with their masks, then four scalars: 40 columns.
pub(crate) fn schema() -> Schema {
    let mut s = Schema::new("BarberShopStyle");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Type", FieldType::UInt32));
    s.add_field(SchemaField::new_array(
        "DisplayName",
        FieldType::String,
        LOCALES,
    ));
    s.add_field(SchemaField::new("DisplayNameMask", FieldType::UInt32));
    s.add_field(SchemaField::new_array(
        "Description",
        FieldType::String,
        LOCALES,
    ));
    s.add_field(SchemaField::new("DescriptionMask", FieldType::UInt32));
    s.add_field(SchemaField::new("CostModifier", FieldType::Float32));
    s.add_field(SchemaField::new("Race", FieldType::Int32));
    s.add_field(SchemaField::new("Sex", FieldType::Int32));
    s.add_field(SchemaField::new("Data", FieldType::Int32));
    s.set_key_field("ID");
    s
}

/// Load every row, in file order: the order the per-type lists keep.
pub fn load_barber_shop_styles(chain: &mut Chain) -> Result<Vec<BarberShopStyle>> {
    let bytes = chain
        .read_file(BARBER_SHOP_STYLE)
        .with_context(|| format!("reading {BARBER_SHOP_STYLE}"))?;
    rows_from(&bytes)
}

fn rows_from(bytes: &[u8]) -> Result<Vec<BarberShopStyle>> {
    let rs = parse(bytes, schema(), "BarberShopStyle")?;
    let description = 2 + LOCALES + 1;
    let scalars = description + LOCALES + 1;
    Ok(rs
        .records()
        .iter()
        .filter_map(|r| {
            Some(BarberShopStyle {
                id: u32_at(r, 0)?,
                style_type: u32_at(r, 1)?,
                name: str_at(&rs, r, 2).unwrap_or_default(),
                description: str_at(&rs, r, description).unwrap_or_default(),
                cost_modifier: f32_at(r, scalars)?,
                race: i32_at(r, scalars + 1)?,
                sex: i32_at(r, scalars + 2)?,
                data: i32_at(r, scalars + 3)?,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WDBC of `rows` as (id, type, name, cost, race, sex, data), names in the enUS slot.
    fn synthesize(rows: &[(u32, u32, &str, f32, i32, i32, i32)]) -> Vec<u8> {
        let mut strings = vec![0u8];
        let mut body = Vec::new();
        for &(id, ty, name, cost, race, sex, data) in rows {
            let off = strings.len() as u32;
            strings.extend_from_slice(name.as_bytes());
            strings.push(0);
            body.extend_from_slice(&id.to_le_bytes());
            body.extend_from_slice(&ty.to_le_bytes());
            body.extend_from_slice(&off.to_le_bytes());
            body.extend(std::iter::repeat_n(0u8, 4 * (LOCALES - 1) + 4));
            body.extend(std::iter::repeat_n(0u8, 4 * LOCALES + 4));
            body.extend_from_slice(&cost.to_le_bytes());
            body.extend_from_slice(&race.to_le_bytes());
            body.extend_from_slice(&sex.to_le_bytes());
            body.extend_from_slice(&data.to_le_bytes());
        }
        let mut dbc = Vec::new();
        dbc.extend_from_slice(b"WDBC");
        dbc.extend_from_slice(&(rows.len() as u32).to_le_bytes());
        dbc.extend_from_slice(&40u32.to_le_bytes());
        dbc.extend_from_slice(&160u32.to_le_bytes());
        dbc.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        dbc.extend_from_slice(&body);
        dbc.extend_from_slice(&strings);
        dbc
    }

    #[test]
    fn a_row_reads_its_scalars_past_both_localized_strings() {
        let rows = rows_from(&synthesize(&[
            (1, 0, "Mohawk", 1.0, 1, 0, 3),
            (2, 3, "", 0.5, 10, -1, 7),
        ]))
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            BarberShopStyle {
                id: 1,
                style_type: 0,
                name: "Mohawk".into(),
                description: String::new(),
                cost_modifier: 1.0,
                race: 1,
                sex: 0,
                data: 3,
            }
        );
        assert_eq!(rows[1].name, "", "an empty slot reads as the empty string");
        assert_eq!((rows[1].sex, rows[1].data), (-1, 7));
    }

    #[test]
    fn the_filter_keeps_the_race_and_the_sex_or_either() {
        let row = |race, sex| BarberShopStyle {
            id: 1,
            style_type: 0,
            name: String::new(),
            description: String::new(),
            cost_modifier: 1.0,
            race,
            sex,
            data: 0,
        };
        assert!(row(1, 0).fits(1, 0));
        assert!(!row(1, 0).fits(1, 1));
        assert!(row(1, -1).fits(1, 1));
        assert!(!row(2, -1).fits(1, 0));
    }

    #[test]
    fn the_install_s_table_has_styles_for_every_playable_race() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let rows = load_barber_shop_styles(&mut chain).expect("BarberShopStyle");
        for race in [1u8, 2, 3, 4, 5, 6, 7, 8, 10, 11] {
            for sex in [0u8, 1] {
                assert!(
                    rows.iter().any(|r| r.style_type == 0 && r.fits(race, sex)),
                    "race {race} sex {sex} has a hair style"
                );
            }
        }
        assert!(rows.iter().all(|r| r.style_type != 1));
    }
}
