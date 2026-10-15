//! 3.3.5a `SMSG_ITEM_QUERY_SINGLE_RESPONSE`: answers captured from AzerothCore
//! (`HandleItemQuerySingleOpcode`) must decode whole into the shared `ItemInfo`, and never through
//! the 1.12.1 reader.

mod common;

use benilla_protocol::messages::{self, opcode, ItemDamage, ItemInfo, ItemSpellEntry};
use benilla_protocol::ServerPacket;
use common::hx;

/// Entry 147, "Rugged Trapper's Pants", a Dwarf Hunter's starting legs: 443 bytes, no stats.
const PANTS: &str = concat!(
    "930000000400000001000000ffffffff", // entry, class, subclass, SoundOverrideSubclass
    "527567676564205472617070657227732050616e747300", // name
    "000000",                           // name2..4
    "f7260000000000000000000000000000", // DisplayInfoID, Quality, Flags, Flags2
    "050000000100000007000000",         // BuyPrice, SellPrice, InventoryType
    "ffffffffffffffff0100000001000000", // AllowableClass, AllowableRace, ItemLevel, RequiredLevel
    "0000000000000000000000000000000000000000", // RequiredSkill, Rank, Spell, honor, city rank
    "0000000000000000",                 // RequiredReputationFaction, RequiredReputationRank
    "000000000100000000000000",         // MaxCount, Stackable, ContainerSlots
    "00000000",                         // StatsCount
    "0000000000000000",                 // ScalingStatDistribution, ScalingStatValue
    "000000000000000000000000000000000000000000000000", // 2 Damage blocks
    "02000000000000000000000000000000000000000000000000000000", // Armor, 6 resistances
    "000000000000000000000000",         // Delay, AmmoType, RangedModRange
    "000000000000000000000000ffffffff00000000ffffffff", // spell block 0
    "000000000000000000000000ffffffff00000000ffffffff", // spell block 1
    "000000000000000000000000ffffffff00000000ffffffff", // spell block 2
    "000000000000000000000000ffffffff00000000ffffffff", // spell block 3
    "000000000000000000000000ffffffff00000000ffffffff", // spell block 4
    "00000000",                         // Bonding
    "00",                               // Description
    "00000000000000000000000000000000", // PageText, LanguageID, PageMaterial, StartQuest
    "00000000070000000000000000000000", // LockID, Material, Sheath, RandomProperty
    "00000000000000000000000019000000", // RandomSuffix, Block, ItemSet, MaxDurability
    "000000000000000000000000",         // Area, Map, BagFamily
    "00000000000000000000000000000000000000000000000000000000", // TotemCategory, 3 sockets
    // socketBonus, GemProperties, RequiredDisenchantSkill, ArmorDamageModifier, Duration,
    // ItemLimitCategory, HolidayId.
    "0000000000000000ffffffff00000000000000000000000000000000",
);

/// Entry 37177, "Wand of the San'layn": two stats, a Shadow damage block and an ON_EQUIP spell.
const WAND: &str = concat!(
    "399100000200000013000000ffffffff", // entry, class, subclass, SoundOverrideSubclass
    "57616e64206f66207468652053616e276c61796e00", // name
    "000000",                           // name2..4
    "50a80000030000000000000000000000", // DisplayInfoID, Quality, Flags, Flags2
    "b3910800bdb601001a000000",         // BuyPrice, SellPrice, InventoryType
    "ffffffffffffffffc800000050000000", // AllowableClass, AllowableRace, ItemLevel, RequiredLevel
    "0000000000000000000000000000000000000000", // RequiredSkill, Rank, Spell, honor, city rank
    "0000000000000000",                 // RequiredReputationFaction, RequiredReputationRank
    "000000000100000000000000",         // MaxCount, Stackable, ContainerSlots
    "02000000",                         // StatsCount
    "1f0000000e0000002400000022000000", // ItemStat pairs
    "0000000000000000",                 // ScalingStatDistribution, ScalingStatValue
    "0080964300000c4405000000000000000000000000000000", // 2 Damage blocks
    "00000000000000000000000000000000000000000000000000000000", // Armor, 6 resistances
    "08070000000000000000c842",         // Delay, AmmoType, RangedModRange
    "a83700000100000000000000000000000000000000000000", // spell block 0
    "000000000000000000000000ffffffff00000000ffffffff", // spell block 1
    "000000000000000000000000ffffffff00000000ffffffff", // spell block 2
    "000000000000000000000000ffffffff00000000ffffffff", // spell block 3
    "000000000000000000000000ffffffff00000000ffffffff", // spell block 4
    "01000000",                         // Bonding
    "00",                               // Description
    "00000000000000000000000000000000", // PageText, LanguageID, PageMaterial, StartQuest
    "00000000020000000000000000000000", // LockID, Material, Sheath, RandomProperty
    "00000000000000000000000041000000", // RandomSuffix, Block, ItemSet, MaxDurability
    "000000000000000000000000",         // Area, Map, BagFamily
    "00000000000000000000000000000000000000000000000000000000", // TotemCategory, 3 sockets
    // socketBonus, GemProperties, RequiredDisenchantSkill 325, ArmorDamageModifier, Duration,
    // ItemLimitCategory, HolidayId.
    "00000000000000004501000000000000000000000000000000000000",
);

fn parse_wotlk(body: &[u8]) -> (u32, Option<Box<ItemInfo>>) {
    let (packet, tail) =
        messages::parse_server_with_tail(opcode::SMSG_ITEM_QUERY_SINGLE_RESPONSE, body).unwrap();
    assert_eq!(tail, 0, "bytes left unread");
    match packet {
        ServerPacket::ItemQueryResponse { entry, info, .. } => (entry, info),
        other => panic!("expected ItemQueryResponse, got {}", other.name()),
    }
}

#[test]
fn starting_pants_decode_whole() {
    let body = hx(PANTS);
    assert_eq!(body.len(), 443);
    let (entry, info) = parse_wotlk(&body);
    assert_eq!(entry, 147);
    assert_eq!(
        info,
        Some(Box::new(ItemInfo {
            class: 4,
            subclass: 1,
            name: "Rugged Trapper's Pants".into(),
            display_info_id: 9975,
            quality: 0,
            flags: 0,
            buy_price: 5,
            sell_price: 1,
            inventory_type: 7,
            allowable_class: -1,
            allowable_race: -1,
            item_level: 1,
            required_level: 1,
            required_skill: 0,
            required_skill_rank: 0,
            required_spell: 0,
            required_honor_rank: 0,
            required_city_rank: 0,
            required_rep_faction: 0,
            required_rep_rank: 0,
            max_count: 0,
            stackable: 1,
            container_slots: 0,
            stats: vec![],
            damages: vec![],
            dmg_min: 0.0,
            dmg_max: 0.0,
            dmg_type: 0,
            armor: 2,
            resistances: [0; 6],
            delay_ms: 0,
            ammo_type: 0,
            ranged_mod_range: 0.0,
            spells: vec![],
            spell_charges_0: 0,
            use_spell: None,
            bonding: 0,
            description: String::new(),
            page_text: 0,
            language_id: 0,
            page_material: 0,
            start_quest: 0,
            lock_id: 0,
            material: 7,
            sheath: 0,
            random_property: 0,
            block: 0,
            item_set: 0,
            max_durability: 25,
            area: 0,
            map: 0,
            bag_family: 0,
        }))
    );
}

#[test]
fn wand_reads_counted_stats_two_damage_blocks_and_spells() {
    let (entry, info) = parse_wotlk(&hx(WAND));
    assert_eq!(entry, 37_177);
    let info = info.unwrap();
    assert_eq!(info.name, "Wand of the San'layn");
    assert_eq!((info.class, info.subclass, info.quality), (2, 19, 3));
    assert_eq!((info.buy_price, info.sell_price), (561_587, 112_317));
    assert_eq!((info.inventory_type, info.item_level), (26, 200));
    assert_eq!(info.required_level, 80);
    assert_eq!(info.stats, vec![(31, 14), (36, 34)]);
    assert_eq!(
        info.damages,
        vec![ItemDamage {
            min: 301.0,
            max: 560.0,
            school: 5
        }]
    );
    assert_eq!((info.delay_ms, info.ranged_mod_range), (1800, 100.0));
    assert_eq!(
        info.spells,
        vec![ItemSpellEntry {
            index: 0,
            spell_id: 14_248,
            trigger: 1,
            charges: 0,
            cooldown_ms: 0,
            category: 0,
            category_cooldown_ms: 0,
        }]
    );
    assert_eq!(info.use_spell, None, "an ON_EQUIP block is no use spell");
    assert_eq!((info.bonding, info.material), (1, 2));
    assert_eq!(info.max_durability, 65);
}

#[test]
fn miss_is_the_flagged_entry() {
    let (entry, info) = parse_wotlk(&(147u32 | 0x8000_0000).to_le_bytes());
    assert_eq!((entry, info), (147, None));
}

#[test]
fn stats_count_above_ten_is_refused() {
    let mut body = hx(PANTS);
    // StatsCount sits after the 26-byte name run and 21 words of fixed fields.
    let at = 16 + 23 + 3 + 21 * 4;
    assert_eq!(&body[at..at + 4], &[0, 0, 0, 0]);
    body[at] = 11;
    assert!(
        messages::parse_server_with_tail(opcode::SMSG_ITEM_QUERY_SINGLE_RESPONSE, &body).is_err()
    );
}
