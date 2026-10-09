//! Where each client keeps the DBC columns benilla reads. A layout gives the file's column count,
//! which the reader checks against the header, and the index of each column read; an array gives
//! its first column. Columns at the same index in every client are left to the reader.

/// The DBC layouts of one reference client.
#[derive(Debug)]
pub struct DbcLayouts {
    pub chr_races: ChrRaces,
    pub char_start_outfit: CharStartOutfit,
    pub char_sections: CharSections,
    pub helmet_geoset_vis_data: HelmetGeosetVisData,
    pub character_facial_hair_styles: CharacterFacialHairStyles,
    pub creature_model_data: CreatureModelData,
    pub creature_display_info: CreatureDisplayInfo,
    pub creature_display_info_extra: CreatureDisplayInfoExtra,
    pub item_display_info: ItemDisplayInfo,
    pub map: Map,
    pub loading_screens: LoadingScreens,
    pub game_tips: GameTips,
}

/// `LoadingScreens.dbc`; `ID`, `Name` and `FileName` are columns 0, 1 and 2 in both clients.
#[derive(Debug)]
pub struct LoadingScreens {
    pub fields: usize,
}

/// `GameTips.dbc`: `ID`, then `Text_lang` (its enUS column 1 in both clients), then its mask.
#[derive(Debug)]
pub struct GameTips {
    pub fields: usize,
}

/// `Map.dbc`; the id and `Directory` are columns 0 and 1, `InstanceType` 2, in both clients.
#[derive(Debug)]
pub struct Map {
    pub fields: usize,
    /// `MapName_lang`, its enUS column.
    pub map_name: usize,
    pub loading_screen: usize,
    /// The battleground columns 1.12.1 keeps here; 3.3.5a moved them to `BattlemasterList.dbc`
    /// and `PvpDifficulty.dbc`.
    pub battleground: Option<MapBattleground>,
}

/// 1.12.1's battleground columns of `Map.dbc`; the descriptions are their enUS columns.
#[derive(Debug)]
pub struct MapBattleground {
    pub min_level: usize,
    pub max_level: usize,
    pub max_players: usize,
    /// Columns 16, 17 and 18, an `i32` and two `f32`, unnamed.
    pub unnamed: usize,
    pub descriptions: [usize; 2],
    pub bracket_span: usize,
    pub group_queue: usize,
    pub minimap_icon_scale: usize,
}

/// `ChrRaces.dbc`; the body displays are columns 4 and 5 in both clients.
#[derive(Debug)]
pub struct ChrRaces {
    pub fields: usize,
    /// `clientFileString` (`"Human"`, `"Scourge"`).
    pub client_file: usize,
    /// `facialHairCustomization[2]`, male then female.
    pub facial_hair_customization: usize,
    pub hair_customization: usize,
}

/// `CharStartOutfit.dbc`, a packed record: `u32 ID`, race, class, sex and outfit as one byte each
/// (four fields in four bytes), then three `slots`-long `i32` arrays of item ids, display ids and
/// inventory types. Offsets here are in bytes.
#[derive(Debug)]
pub struct CharStartOutfit {
    pub fields: usize,
    pub record_size: usize,
    pub slots: usize,
    pub display_ids: usize,
    pub inventory_types: usize,
}

/// `CharSections.dbc`; race, sex and section type are columns 1, 2 and 3 in both clients.
#[derive(Debug)]
pub struct CharSections {
    pub fields: usize,
    pub variation: usize,
    pub color: usize,
    /// `TextureName[3]`.
    pub textures: usize,
    pub flags: usize,
    /// A row is offered at character creation, to any class but the Death Knight, when
    /// `flags & offered_mask == offered_flags`.
    pub offered_mask: u32,
    pub offered_flags: u32,
}

impl CharSections {
    /// Whether a row with these `Flags` is offered at character creation (not to a Death Knight
    /// alone).
    pub fn offered(&self, flags: u32) -> bool {
        flags & self.offered_mask == self.offered_flags
    }
}

/// `HelmetGeosetVisData.dbc`: an ID, then race masks, of which 1.12.1's five (hair, the three
/// facial groups, ears) lead in both clients.
#[derive(Debug)]
pub struct HelmetGeosetVisData {
    pub fields: usize,
}

/// `CharacterFacialHairStyles.dbc`; race, sex and variation are columns 0, 1 and 2 in both clients.
#[derive(Debug)]
pub struct CharacterFacialHairStyles {
    pub fields: usize,
    /// The geoset array, whose first three are the 1xx, 3xx and 2xx groups.
    pub geosets: usize,
}

/// `CreatureModelData.dbc`: 3.3.5a appends columns after 1.12.1's sixteen and moves none.
#[derive(Debug)]
pub struct CreatureModelData {
    pub fields: usize,
}

/// `CreatureDisplayInfo.dbc`; the columns before the size class sit alike in both clients.
#[derive(Debug)]
pub struct CreatureDisplayInfo {
    pub fields: usize,
    pub size_class: usize,
    pub blood_level: usize,
    pub npc_sound: usize,
}

/// `CreatureDisplayInfoExtra.dbc`; the appearance bytes are columns 1 to 7 in both clients.
#[derive(Debug)]
pub struct CreatureDisplayInfoExtra {
    pub fields: usize,
    /// `NPCItemDisplay[equipment_slots]`; 3.3.5a's eleventh is the cape.
    pub equipment: usize,
    pub equipment_slots: usize,
    pub bake_name: usize,
}

/// `ItemDisplayInfo.dbc`; the models, their textures and the first icon are columns 1 to 5 in both
/// clients.
#[derive(Debug)]
pub struct ItemDisplayInfo {
    pub fields: usize,
    /// `GeosetGroup[3]`.
    pub geoset_group: usize,
    pub flags: usize,
    pub spell_visual: usize,
    pub group_sound: usize,
    /// `HelmetGeosetVis[2]`.
    pub helmet_geoset_vis: usize,
    /// `Texture[8]`, the body-region textures.
    pub textures: usize,
    pub item_visual: usize,
}

/// Build 5875.
pub(crate) const CLASSIC: DbcLayouts = DbcLayouts {
    chr_races: ChrRaces {
        fields: 29,
        client_file: 15,
        facial_hair_customization: 26,
        hair_customization: 28,
    },
    char_start_outfit: CharStartOutfit {
        fields: 41,
        record_size: 152,
        slots: 12,
        display_ids: 56,
        inventory_types: 104,
    },
    // `0x1` marks an extra, NPC-only row (vmangos `SECTION_FLAG_UNAVAILABLE`).
    char_sections: CharSections {
        fields: 10,
        variation: 4,
        color: 5,
        textures: 6,
        flags: 9,
        offered_mask: 0x1,
        offered_flags: 0,
    },
    helmet_geoset_vis_data: HelmetGeosetVisData { fields: 6 },
    // Columns 3 to 5 are unread.
    character_facial_hair_styles: CharacterFacialHairStyles {
        fields: 9,
        geosets: 6,
    },
    creature_model_data: CreatureModelData { fields: 16 },
    creature_display_info: CreatureDisplayInfo {
        fields: 12,
        size_class: 9,
        blood_level: 10,
        npc_sound: 11,
    },
    creature_display_info_extra: CreatureDisplayInfoExtra {
        fields: 19,
        equipment: 8,
        equipment_slots: 10,
        bake_name: 18,
    },
    item_display_info: ItemDisplayInfo {
        fields: 23,
        geoset_group: 6,
        flags: 9,
        spell_visual: 10,
        group_sound: 11,
        helmet_geoset_vis: 12,
        textures: 14,
        item_visual: 22,
    },
    // Nine-column localized strings.
    map: Map {
        fields: 42,
        map_name: 4,
        loading_screen: 38,
        battleground: Some(MapBattleground {
            min_level: 13,
            max_level: 14,
            max_players: 15,
            unnamed: 16,
            descriptions: [20, 29],
            bracket_span: 39,
            group_queue: 40,
            minimap_icon_scale: 41,
        }),
    },
    loading_screens: LoadingScreens { fields: 3 },
    // Eight locales and the mask.
    game_tips: GameTips { fields: 10 },
};

/// Build 12340, each checked against the shipped files row for row with 1.12.1's.
pub(crate) const WOTLK: DbcLayouts = DbcLayouts {
    // 3.3.5a drops 1.12.1's mount scale and three more before the file string, and localizes the
    // name three times (male, female, neutral) over sixteen locales.
    chr_races: ChrRaces {
        fields: 69,
        client_file: 11,
        facial_hair_customization: 65,
        hair_customization: 67,
    },
    char_start_outfit: CharStartOutfit {
        fields: 77,
        record_size: 296,
        slots: 24,
        display_ids: 104,
        inventory_types: 200,
    },
    // The textures and flags come before the variation and color in 3.3.5a, and the flags turn
    // around: `0x1` marks a creatable row, `0x4` a Death Knight's, `0x8` an NPC's. Human male skin
    // colors 0..9 are `0x11`, 10 and 11 `0x8`, 12..14 `0x5`.
    char_sections: CharSections {
        fields: 10,
        variation: 8,
        color: 9,
        textures: 4,
        flags: 7,
        offered_mask: 0x5,
        offered_flags: 0x1,
    },
    // Two more masks follow 1.12.1's five.
    helmet_geoset_vis_data: HelmetGeosetVisData { fields: 8 },
    character_facial_hair_styles: CharacterFacialHairStyles {
        fields: 8,
        geosets: 3,
    },
    creature_model_data: CreatureModelData { fields: 28 },
    // `PortraitTextureName` is column 9 in 3.3.5a.
    creature_display_info: CreatureDisplayInfo {
        fields: 16,
        size_class: 10,
        blood_level: 11,
        npc_sound: 12,
    },
    // A cape slot, then `Flags`, before the bake name.
    creature_display_info_extra: CreatureDisplayInfoExtra {
        fields: 21,
        equipment: 8,
        equipment_slots: 11,
        bake_name: 20,
    },
    // A second icon at column 6 moves the rest by one; `ParticleColorID` closes the record.
    item_display_info: ItemDisplayInfo {
        fields: 25,
        geoset_group: 7,
        flags: 10,
        spell_visual: 11,
        group_sound: 12,
        helmet_geoset_vis: 13,
        textures: 15,
        item_visual: 23,
    },
    // `Flags` and `PVP` at 3 and 4 put the seventeen-column `MapName_lang` at 5; `AreaTableID`
    // at 22 and the two descriptions take it to `LoadingScreenID` at 57.
    map: Map {
        fields: 66,
        map_name: 5,
        loading_screen: 57,
        battleground: None,
    },
    // `HasWideScreen` closes the record: 1 on the continents, which ship a `…Wide.blp` beside the
    // art.
    loading_screens: LoadingScreens { fields: 4 },
    // Sixteen locales and the mask.
    game_tips: GameTips { fields: 18 },
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_client_offers_its_own_section_rows() {
        let classic = &CLASSIC.char_sections;
        assert!(classic.offered(0));
        assert!(!classic.offered(0x1), "1.12.1's extra row");
        let wotlk = &WOTLK.char_sections;
        assert!(wotlk.offered(0x1) && wotlk.offered(0x11));
        assert!(!wotlk.offered(0), "3.3.5a offers only flagged rows");
        assert!(!wotlk.offered(0x5), "a Death Knight's row");
        assert!(!wotlk.offered(0x8), "an NPC's row");
    }

    /// Every column read sits inside its table, arrays included.
    #[test]
    fn every_column_is_inside_its_table() {
        for l in [&CLASSIC, &WOTLK] {
            let r = &l.chr_races;
            assert!(
                r.client_file
                    .max(r.facial_hair_customization + 1)
                    .max(r.hair_customization)
                    < r.fields
            );
            let o = &l.char_start_outfit;
            assert_eq!(o.inventory_types + 4 * o.slots, o.record_size);
            // `ID`, four packed bytes, then the three arrays.
            assert_eq!(o.fields, 1 + 4 + 3 * o.slots);
            assert_eq!(o.display_ids + 4 * o.slots, o.inventory_types);
            let s = &l.char_sections;
            assert!(s.variation.max(s.color).max(s.textures + 2).max(s.flags) < s.fields);
            let f = &l.character_facial_hair_styles;
            assert!(f.geosets + 2 < f.fields);
            let d = &l.creature_display_info;
            assert!(d.size_class.max(d.blood_level).max(d.npc_sound) < d.fields);
            let e = &l.creature_display_info_extra;
            assert!(e.equipment + e.equipment_slots <= e.bake_name && e.bake_name < e.fields);
            let i = &l.item_display_info;
            assert!(i.textures + 8 <= i.item_visual && i.item_visual < i.fields);
            assert!(l.creature_model_data.fields >= 16 && l.helmet_geoset_vis_data.fields >= 6);
            let m = &l.map;
            assert!(m.map_name > 2 && m.loading_screen < m.fields);
            assert!(l.loading_screens.fields >= 3 && l.game_tips.fields > 2);
            if let Some(b) = &m.battleground {
                assert!(b.minimap_icon_scale < m.fields && b.unnamed + 2 < b.descriptions[0]);
            }
        }
    }
}
