//! `ItemDisplayInfo.dbc`: a display id to a held item's or worn equipment's visual identity. 23
//! fields and 92-byte records in 5875, both checked by the reference loader (`0x547590`).
//!
//! A row's model and texture are independently resolved basenames in the same
//! `Item\ObjectComponents\<dir>\` folder; never derive one from the other. The Worn Wooden Shield
//! pairs model `Shield_Round_A_01.mdx` with texture `Buckler_Damaged_A_01Purple`.

use std::collections::HashMap;

use crate::dbc_layout;
use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, RecordSet, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::models::model_path;

const ITEM_DISPLAY_INFO: &str = "DBFilesClient\\ItemDisplayInfo.dbc";

/// One `ItemDisplayInfo` row: the visual identity of a held item or worn equipment. Basenames carry
/// no directory: the `Item\ObjectComponents\<dir>\` join belongs to the app, since `<dir>` follows
/// the inventory type, which is not a column here.
#[derive(Debug, Clone, Default)]
pub struct ItemDisplay {
    /// Left and right model basenames, normalized by [`crate::models::model_path`].
    pub model: [Option<String>; 2],
    /// Left and right texture basenames, raw and extensionless; never derived from [`Self::model`].
    pub model_texture: [Option<String>; 2],
    /// Worn-equipment geoset selectors, 0 where unauthored.
    pub geoset_groups: [u32; 3],
    /// Body-region textures, in ArmUpper, ArmLower, Hand, TorsoUpper, TorsoLower, LegUpper,
    /// LegLower, Foot order.
    pub region_textures: [Option<String>; 8],
    /// A helm's `HelmetGeosetVisData` rows, `[male, female]`, 0 for none (`0x4799a0` reads
    /// `+0x30 + sex * 4`).
    pub helmet_vis: [u32; 2],
    /// The inventory icon as an extensionless `Interface\Icons\…` path, ready to load.
    pub icon: Option<String>,
    /// The `ItemGroupSounds.dbc` pickup and place sound group (`0x458008`); 0 is silent, as the
    /// reference's bounds check returns (`0x45800b`).
    pub group_sounds: u32,
    /// The `SpellVisual.dbc` id a ranged-attribute spell borrows from the equipped ranged weapon,
    /// field by field where its own visual leaves a 0 (`VisualStages::merged_over_weapon`,
    /// `0x60d450`, read at `0x60d493`).
    pub spell_visual: u32,
    /// The flags word, the reference's `+0x24`. Only bit 0 is read, [`Self::takes_guild_emblem`];
    /// bit 1, set on four chest and robe rows, has no reader.
    pub flags: u32,
    /// The `ItemVisuals.dbc` id of the display's intrinsic weapon glow (`0x47a200`). Signed, as the
    /// reference's `jle` gate reads it (`0x4798c0`): 0 and `-1` both mean none.
    pub item_visual: i32,
}

impl ItemDisplay {
    /// The `HelmetGeosetVisData` pair this display hides hair and ears with, only when it names a
    /// left model: the reference's head-slot handler (`0x4799a0`) tests `ModelName[0]` for an empty
    /// string (`0x4799c1`) and skips the whole geoset-vis tail (`0x479b33`), never reading the
    /// right slot or the model's load result. Twelve model-less rows carry a mask that 126 NPC head
    /// displays point at, and the reference draws those NPCs' hair in full.
    pub fn worn_helm_vis(&self) -> Option<[u32; 2]> {
        self.model[0].is_some().then_some(self.helmet_vis)
    }

    /// Whether the wearer's guild emblem replaces this tabard's torso art (`flags & 1`, tested at
    /// `0x472ca4` on the tabard slot). Seven shipped rows set it, all tabards.
    pub fn takes_guild_emblem(&self) -> bool {
        self.flags & FLAG_GUILD_EMBLEM_TABARD != 0
    }
}

const FLAG_GUILD_EMBLEM_TABARD: u32 = 0x1;

/// `ItemDisplayInfo.dbc` by display id, the id `ItemDisplayInfoID` and
/// `UNIT_VIRTUAL_ITEM_SLOT_DISPLAY` name.
pub struct ItemDisplayCatalog {
    displays: HashMap<u32, ItemDisplay>,
}

impl ItemDisplayCatalog {
    /// A catalog from an explicit row map, for tests and fixtures.
    pub fn from_displays(displays: HashMap<u32, ItemDisplay>) -> Self {
        ItemDisplayCatalog { displays }
    }

    pub fn get(&self, display_id: u32) -> Option<&ItemDisplay> {
        self.displays.get(&display_id)
    }

    pub fn len(&self) -> usize {
        self.displays.len()
    }

    pub fn is_empty(&self) -> bool {
        self.displays.is_empty()
    }

    /// Every `(displayId, row)`, unordered.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &ItemDisplay)> {
        self.displays.iter().map(|(&id, d)| (id, d))
    }
}

/// Every column, the unread ones included, so the field-count check against the header is exact.
pub(crate) fn item_display_info_schema(l: &dbc_layout::ItemDisplayInfo) -> Schema {
    let mut s = Schema::new("ItemDisplayInfo");
    for (name, ty) in [
        ("ID", FieldType::UInt32),
        ("ModelNameLeft", FieldType::String),
        ("ModelNameRight", FieldType::String),
        ("ModelTextureLeft", FieldType::String),
        ("ModelTextureRight", FieldType::String),
        ("Icon", FieldType::String),
    ] {
        s.add_field(SchemaField::new(name, ty));
    }
    let named = |i: usize| -> Option<(String, FieldType)> {
        const REGIONS: [&str; 8] = [
            "ArmUpperTexture",
            "ArmLowerTexture",
            "HandTexture",
            "TorsoUpperTexture",
            "TorsoLowerTexture",
            "LegUpperTexture",
            "LegLowerTexture",
            "FootTexture",
        ];
        let int = |name: &str| Some((name.to_string(), FieldType::UInt32));
        match i {
            _ if (l.geoset_group..l.geoset_group + 3).contains(&i) => Some((
                format!("GeosetGroup{}", i - l.geoset_group),
                FieldType::UInt32,
            )),
            _ if i == l.flags => int("Flags"),
            _ if i == l.spell_visual => int("SpellVisualID"),
            _ if i == l.group_sound => int("ItemGroupSoundsID"),
            _ if i == l.helmet_geoset_vis => int("HelmVisMale"),
            _ if i == l.helmet_geoset_vis + 1 => int("HelmVisFemale"),
            _ if (l.textures..l.textures + 8).contains(&i) => {
                Some((REGIONS[i - l.textures].to_string(), FieldType::String))
            }
            _ if i == l.item_visual => int("ItemVisualID"),
            _ => None,
        }
    };
    for i in s.fields.len()..l.fields {
        let (name, ty) = named(i).unwrap_or_else(|| (format!("c{i}"), FieldType::UInt32));
        s.add_field(SchemaField::new(name, ty));
    }
    s
}

/// The catalog from a record set parsed with `l`'s schema, testable without a chain.
fn catalog_from_records(rs: RecordSet, l: &dbc_layout::ItemDisplayInfo) -> ItemDisplayCatalog {
    let mut displays = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let model = [
            str_at(&rs, r, 1).map(|s| model_path(&s)),
            str_at(&rs, r, 2).map(|s| model_path(&s)),
        ];
        let model_texture = [str_at(&rs, r, 3), str_at(&rs, r, 4)];
        let geoset_groups = std::array::from_fn(|i| u32_at(r, l.geoset_group + i).unwrap_or(0));
        let region_textures = std::array::from_fn(|i| str_at(&rs, r, l.textures + i));
        let helmet_vis = std::array::from_fn(|i| u32_at(r, l.helmet_geoset_vis + i).unwrap_or(0));
        let icon = str_at(&rs, r, 5).map(|i| format!("Interface\\Icons\\{i}"));
        let group_sounds = u32_at(r, l.group_sound).unwrap_or(0);
        let spell_visual = u32_at(r, l.spell_visual).unwrap_or(0);
        let flags = u32_at(r, l.flags).unwrap_or(0);
        let item_visual = u32_at(r, l.item_visual).unwrap_or(0) as i32;
        displays.insert(
            id,
            ItemDisplay {
                model,
                model_texture,
                geoset_groups,
                region_textures,
                helmet_vis,
                icon,
                group_sounds,
                spell_visual,
                flags,
                item_visual,
            },
        );
    }
    ItemDisplayCatalog { displays }
}

/// Load `ItemDisplayInfo.dbc` off the patch chain into an [`ItemDisplayCatalog`].
pub fn load_item_display_catalog(chain: &mut Chain) -> Result<ItemDisplayCatalog> {
    let bytes = chain
        .read_file(ITEM_DISPLAY_INFO)
        .with_context(|| format!("reading {ITEM_DISPLAY_INFO}"))?;
    let l = &crate::dbc_layout::LAYOUTS.item_display_info;
    let rs = parse(&bytes, item_display_info_schema(l), "ItemDisplayInfo")?;
    Ok(catalog_from_records(rs, l))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal synthetic WDBC: 20-byte header, fixed-width records, string block.
    fn build_wdbc(
        record_count: u32,
        field_count: u32,
        record_size: u32,
        records: &[u8],
        strings: &[u8],
    ) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"WDBC");
        b.extend_from_slice(&record_count.to_le_bytes());
        b.extend_from_slice(&field_count.to_le_bytes());
        b.extend_from_slice(&record_size.to_le_bytes());
        b.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        b.extend_from_slice(records);
        b.extend_from_slice(strings);
        b
    }

    const FIELD_COUNT: u32 = 25;
    const RECORD_SIZE: u32 = FIELD_COUNT * 4;

    fn layout() -> &'static dbc_layout::ItemDisplayInfo {
        &crate::dbc_layout::LAYOUTS.item_display_info
    }

    /// A string block whose offset 0 is `""`, where an absent column points.
    #[derive(Default)]
    struct StringBlock(Vec<u8>);
    impl StringBlock {
        fn new() -> Self {
            Self(vec![0u8])
        }
        fn push(&mut self, s: &str) -> u32 {
            let off = self.0.len() as u32;
            self.0.extend_from_slice(s.as_bytes());
            self.0.push(0);
            off
        }
    }

    /// A projectile display carries its flight model in the right slot (`Ammo\`), a thrown weapon
    /// in the left (`Weapon\`). The missile spawner keys its folder on that shape; the reference
    /// keys the same fork on the ammo's `InventoryType` (`0x19` thrown, `0x60ba30`), with the same
    /// result on every shipped row.
    #[test]
    fn real_ammo_displays_carry_flight_models_right_thrown_left() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_item_display_catalog(&mut chain).expect("load ItemDisplayInfo");

        // Rough Arrow: flight model and skin in the right slot, the left empty.
        let arrow = cat.get(5996).expect("arrow display");
        assert_eq!(arrow.model, [None, Some("arrowflight_01.m2".into())]);
        assert_eq!(arrow.model_texture[1].as_deref(), Some("Arrow_A_01Brown"));
        // Light Shot: the bullet pair.
        let shot = cat.get(5998).expect("bullet display");
        assert_eq!(shot.model[1].as_deref(), Some("bulletflight_01.m2"));

        // Balanced Throwing Dagger: the weapon model, from `Item\ObjectComponents\Weapon\`, in the
        // left slot.
        let thrown = cat.get(16752).expect("thrown display");
        assert_eq!(thrown.model[0].as_deref(), Some("thrown_1h_dagger_a_01.m2"));
        assert_eq!(
            thrown.model_texture[0].as_deref(),
            Some("Thrown_1H_Dagger_A_01Copper")
        );
    }

    /// A row: the second icon at column 6, and `ParticleColorID` closes the 25-column record.
    #[test]
    fn a_row_reads_through_its_layout() {
        let l = layout();
        let mut strings = StringBlock::new();
        let icon = strings.push("INV_Boots_01");
        let foot = strings.push("Generic_HuMa_01_Boot_FO");
        let mut cols = [0u32; 25];
        cols[0] = 222;
        cols[5] = icon;
        cols[7] = 2; // GeosetGroup0
        cols[10] = 0x10; // Flags
        cols[11] = 41; // SpellVisualID
        cols[12] = 7; // ItemGroupSoundsID
        cols[14] = 3; // HelmVisFemale
        cols[22] = foot; // FootTexture
        cols[23] = 9; // ItemVisualID
        cols[24] = 5; // ParticleColorID, unread
        let rec: Vec<u8> = cols.iter().flat_map(|c| c.to_le_bytes()).collect();
        let bytes = build_wdbc(1, FIELD_COUNT, RECORD_SIZE, &rec, &strings.0);
        let rs = parse(&bytes, item_display_info_schema(l), "test").expect("synthetic DBC parses");
        let d = catalog_from_records(rs, l).get(222).cloned().expect("row");
        assert_eq!(d.icon.as_deref(), Some("Interface\\Icons\\INV_Boots_01"));
        assert_eq!(d.geoset_groups, [2, 0, 0]);
        assert_eq!((d.flags, d.spell_visual, d.group_sounds), (0x10, 41, 7));
        assert_eq!(d.helmet_vis, [0, 3]);
        assert_eq!(
            d.region_textures[7].as_deref(),
            Some("Generic_HuMa_01_Boot_FO")
        );
        assert_eq!(d.item_visual, 9);
    }

    #[test]
    fn unknown_display_id_misses() {
        let rec = vec![0u8; RECORD_SIZE as usize]; // ID 0, everything else empty or zero
        let bytes = build_wdbc(1, FIELD_COUNT, RECORD_SIZE, &rec, &StringBlock::new().0);
        let rs = parse(&bytes, item_display_info_schema(layout()), "test")
            .expect("synthetic DBC parses");
        let catalog = catalog_from_records(rs, layout());
        assert_eq!(catalog.len(), 1);
        assert!(catalog.get(999).is_none());
    }
}
