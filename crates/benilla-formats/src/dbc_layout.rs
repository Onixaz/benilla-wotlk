//! Where the client keeps the DBC columns benilla reads whose index a loader does not spell out
//! itself. A layout gives the file's column count, which the reader checks against the header, and
//! the index of each column read; an array gives its first column.

/// The DBC layouts of build 12340.
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
    pub server_messages: LocalizedTable,
    pub emotes_text_data: LocalizedTable,
    pub chat_profanity: ChatProfanity,
    pub chat_channels: ChatChannels,
    pub area_table: AreaTable,
}

/// `AreaTable.dbc`; the id, map, parent, explore bit, flags and exploration level are columns 0
/// to 4 and 10, the dry and underwater sound providers, ambience, zone music and intro columns 5
/// to 9, and `AreaName_lang`'s enUS column is 11.
#[derive(Debug)]
pub struct AreaTable {
    pub fields: usize,
    pub faction_group_mask: usize,
}

/// A table of `ID`, one localized string (its enUS column 1) and its mask or flags:
/// `ServerMessages.dbc`, `EmotesTextData.dbc`.
#[derive(Debug)]
pub struct LocalizedTable {
    pub fields: usize,
}

/// `ChatProfanity.dbc`; `ID` and the pattern are columns 0 and 1.
#[derive(Debug)]
pub struct ChatProfanity {
    pub fields: usize,
}

/// `ChatChannels.dbc`; `ID`, `Flags`, `FactionGroup` and `Name_lang`'s enUS column are columns 0
/// to 3.
#[derive(Debug)]
pub struct ChatChannels {
    pub fields: usize,
    /// `Shortcut_lang`, its enUS column.
    pub shortcut: usize,
}

/// `LoadingScreens.dbc`; `ID`, `Name` and `FileName` are columns 0, 1 and 2.
#[derive(Debug)]
pub struct LoadingScreens {
    pub fields: usize,
}

/// `GameTips.dbc`: `ID`, then `Text_lang` (its enUS column 1), then its mask.
#[derive(Debug)]
pub struct GameTips {
    pub fields: usize,
}

/// `Map.dbc`; the id and `Directory` are columns 0 and 1, `InstanceType` 2. The battleground
/// levels and sizes live in `BattlemasterList.dbc` and `PvpDifficulty.dbc`.
#[derive(Debug)]
pub struct Map {
    pub fields: usize,
    /// `MapName_lang`, its enUS column.
    pub map_name: usize,
    /// `MapDescription0_lang` and `MapDescription1_lang`, the Horde and Alliance texts, their
    /// enUS columns.
    pub description: [usize; 2],
    pub loading_screen: usize,
}

/// `ChrRaces.dbc`; the body displays are columns 4 and 5.
#[derive(Debug)]
pub struct ChrRaces {
    pub fields: usize,
    /// `clientFileString` (`"Human"`, `"Scourge"`).
    pub client_file: usize,
    /// The enUS slot of `Name_Lang`, the race name (`"Night Elf"`).
    pub name: usize,
    /// `facialHairCustomization[2]`, male then female.
    pub facial_hair_customization: usize,
    pub hair_customization: usize,
    /// The enUS slots of `Name_female_lang` and `Name_male_lang`.
    pub name_female: usize,
    pub name_male: usize,
    /// The expansion level the race needs.
    pub required_expansion: usize,
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

/// `CharSections.dbc`; race, sex and section type are columns 1, 2 and 3.
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

/// `HelmetGeosetVisData.dbc`: an ID, then race masks, of which five (hair, the three facial
/// groups, ears) lead.
#[derive(Debug)]
pub struct HelmetGeosetVisData {
    pub fields: usize,
}

/// `CharacterFacialHairStyles.dbc`; race, sex and variation are columns 0, 1 and 2.
#[derive(Debug)]
pub struct CharacterFacialHairStyles {
    pub fields: usize,
    /// The geoset array, whose first three are the 1xx, 3xx and 2xx groups.
    pub geosets: usize,
}

/// `CreatureModelData.dbc`: the first sixteen columns are the ones benilla reads.
#[derive(Debug)]
pub struct CreatureModelData {
    pub fields: usize,
}

/// `CreatureDisplayInfo.dbc`; the columns before the size class are read by index.
#[derive(Debug)]
pub struct CreatureDisplayInfo {
    pub fields: usize,
    pub size_class: usize,
    pub blood_level: usize,
    pub npc_sound: usize,
}

/// `CreatureDisplayInfoExtra.dbc`; the appearance bytes are columns 1 to 7.
#[derive(Debug)]
pub struct CreatureDisplayInfoExtra {
    pub fields: usize,
    /// `NPCItemDisplay[equipment_slots]`; the eleventh is the cape.
    pub equipment: usize,
    pub equipment_slots: usize,
    pub bake_name: usize,
}

/// `ItemDisplayInfo.dbc`; the models, their textures and the first icon are columns 1 to 5.
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

/// Build 12340, each checked against the shipped files.
pub const LAYOUTS: DbcLayouts = DbcLayouts {
    // The name is localized three times (male, female, neutral) over sixteen locales.
    chr_races: ChrRaces {
        fields: 69,
        client_file: 11,
        name: 14,
        facial_hair_customization: 65,
        hair_customization: 67,
        name_female: 31,
        name_male: 48,
        required_expansion: 68,
    },
    char_start_outfit: CharStartOutfit {
        fields: 77,
        record_size: 296,
        slots: 24,
        display_ids: 104,
        inventory_types: 200,
    },
    // The textures and flags come before the variation and color: `0x1` marks a creatable row,
    // `0x4` a Death Knight's, `0x8` an NPC's. Human male skin colors 0..9 are `0x11`, 10 and 11
    // `0x8`, 12..14 `0x5`.
    char_sections: CharSections {
        fields: 10,
        variation: 8,
        color: 9,
        textures: 4,
        flags: 7,
        offered_mask: 0x5,
        offered_flags: 0x1,
    },
    // Two more masks follow the five.
    helmet_geoset_vis_data: HelmetGeosetVisData { fields: 8 },
    character_facial_hair_styles: CharacterFacialHairStyles {
        fields: 8,
        geosets: 3,
    },
    creature_model_data: CreatureModelData { fields: 28 },
    // `PortraitTextureName` is column 9.
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
        description: [23, 40],
        loading_screen: 57,
    },
    // `HasWideScreen` closes the record: 1 on the continents, which ship a `…Wide.blp` beside the
    // art.
    loading_screens: LoadingScreens { fields: 4 },
    // Sixteen locales and the mask.
    game_tips: GameTips { fields: 18 },
    // Sixteen locales and a mask: 9 rows x 18 and 1327 x 18 in the shipped files.
    server_messages: LocalizedTable { fields: 18 },
    emotes_text_data: LocalizedTable { fields: 18 },
    // A third column, 1 in the shipped rows, which benilla does not read.
    chat_profanity: ChatProfanity { fields: 3 },
    // Name and Shortcut each over sixteen locales and a mask: 6 rows x 37.
    chat_channels: ChatChannels {
        fields: 37,
        shortcut: 20,
    },
    // The name over sixteen locales puts the faction mask at 28: 2307 rows x 36.
    area_table: AreaTable {
        fields: 36,
        faction_group_mask: 28,
    },
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_offers_only_flagged_rows() {
        let s = &LAYOUTS.char_sections;
        assert!(s.offered(0x1) && s.offered(0x11));
        assert!(!s.offered(0), "only flagged rows are offered");
        assert!(!s.offered(0x5), "a Death Knight's row");
        assert!(!s.offered(0x8), "an NPC's row");
    }

    /// Every column read sits inside its table, arrays included.
    #[test]
    fn every_column_is_inside_its_table() {
        let l = &LAYOUTS;
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
    }
}
