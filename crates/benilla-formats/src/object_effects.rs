//! Object effect packages: the sounds a unit's display carries by state (a siege engine's engine
//! loop while it stands or moves, its launch and load one-shots). `CreatureDisplayInfo` column 15
//! names an `ObjectEffectPackage`; `ObjectEffectPackageElem` lists, per package, the
//! `ObjectEffectGroup` each state turns on; `ObjectEffect` puts each effect in a group as an enter
//! one-shot (trigger 1), a loop (2) or an exit one-shot (3). Built as the system init builds its
//! lookups (`0x6f66b0`).
//!
//! Not read: trigger 4 (event effects, keyed by column 4): one 12340 row sets an event type and
//! none uses trigger 4. `ObjectEffectModifier` (the speed → pitch map on 11 rows, `0x6f1790`) and
//! each effect's attachment and offset (all 145 rows name attachment 0, 14 carry an offset): the
//! sound plays at the unit's origin at its kit's pitch.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::creatures::creature_display_info_schema;
use crate::dbc::{parse, u32_at};
use crate::dbc_layout::LAYOUTS;
use crate::Chain;

/// `CreatureDisplayInfo`'s `ObjectEffectPackageID`.
const DISPLAY_PACKAGE_COLUMN: usize = 15;

/// `ObjectEffect.EffectRecType` for a `SoundEntries` kit, the only kind 12340 plays and the only
/// kind its rows use.
pub const EFFECT_SOUND: u32 = 1;

/// One `ObjectEffect` row as its group holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectEffect {
    pub id: u32,
    /// `EffectRecType`: [`EFFECT_SOUND`] for a sound kit.
    pub kind: u32,
    /// `EffectRecID`: the `SoundEntries` id for a sound.
    pub rec_id: u32,
}

impl ObjectEffect {
    /// The sound kit this effect plays, `None` for any other kind.
    pub fn sound_kit(&self) -> Option<u32> {
        (self.kind == EFFECT_SOUND).then_some(self.rec_id)
    }
}

/// An `ObjectEffectGroup`'s effects by trigger, each in table order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EffectGroup {
    pub id: u32,
    /// Trigger 1: played once when a state naming the group is entered.
    pub enter: Vec<ObjectEffect>,
    /// Trigger 2: played while a state naming the group holds.
    pub looping: Vec<ObjectEffect>,
    /// Trigger 3: played once when such a state is left.
    pub exit: Vec<ObjectEffect>,
}

/// The joined tables: display → package → state → groups → effects.
#[derive(Default)]
pub struct ObjectEffectCatalog {
    display_packages: HashMap<u32, u32>,
    /// Package id → state → its groups' ids in `ObjectEffectPackageElem` order.
    packages: HashMap<u32, HashMap<u32, Vec<u32>>>,
    groups: HashMap<u32, EffectGroup>,
}

impl ObjectEffectCatalog {
    /// Join display → package, package → state → group ids, and the groups; a state's group
    /// with no entry in `groups` is dropped.
    pub fn from_parts(
        display_packages: HashMap<u32, u32>,
        packages: HashMap<u32, HashMap<u32, Vec<u32>>>,
        groups: impl IntoIterator<Item = EffectGroup>,
    ) -> Self {
        let groups: HashMap<u32, EffectGroup> = groups.into_iter().map(|g| (g.id, g)).collect();
        let mut packages = packages;
        for states in packages.values_mut() {
            for ids in states.values_mut() {
                ids.retain(|g| groups.contains_key(g));
            }
        }
        Self {
            display_packages,
            packages,
            groups,
        }
    }

    /// The package a creature display carries; `None` for 0 or a display with no row.
    pub fn package_for_display(&self, display_id: u32) -> Option<u32> {
        self.display_packages.get(&display_id).copied()
    }

    /// Whether `package` is an `ObjectEffectPackage` row: a manager starts only on one (`0x6f7420`).
    pub fn has_package(&self, package: u32) -> bool {
        self.packages.contains_key(&package)
    }

    /// The groups `state` turns on in `package`, in table order; empty for none.
    pub fn state_groups(&self, package: u32, state: u32) -> impl Iterator<Item = &EffectGroup> {
        self.packages
            .get(&package)
            .and_then(|states| states.get(&state))
            .into_iter()
            .flatten()
            .filter_map(|g| self.groups.get(g))
    }

    /// Every group `package` names, under any state.
    pub fn package_groups(&self, package: u32) -> impl Iterator<Item = &EffectGroup> {
        self.packages
            .get(&package)
            .into_iter()
            .flat_map(|states| states.values().flatten())
            .filter_map(|g| self.groups.get(g))
    }

    pub fn len(&self) -> usize {
        self.packages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }
}

pub(crate) fn package_schema() -> Schema {
    id_name("ObjectEffectPackage")
}

pub(crate) fn group_schema() -> Schema {
    id_name("ObjectEffectGroup")
}

fn id_name(what: &str) -> Schema {
    let mut s = Schema::new(what);
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Name", FieldType::String));
    s.set_key_field("ID");
    s
}

pub(crate) fn package_elem_schema() -> Schema {
    let mut s = Schema::new("ObjectEffectPackageElem");
    for name in [
        "ID",
        "ObjectEffectPackageID",
        "ObjectEffectGroupID",
        "StateType",
    ] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s.set_key_field("ID");
    s
}

pub(crate) fn effect_schema() -> Schema {
    let mut s = Schema::new("ObjectEffect");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Name", FieldType::String));
    for name in [
        "ObjectEffectGroupID",
        "TriggerType",
        "EventType",
        "EffectRecType",
        "EffectRecID",
        "Attachment",
    ] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    for name in ["OffsetX", "OffsetY", "OffsetZ"] {
        s.add_field(SchemaField::new(name, FieldType::Float32));
    }
    s.add_field(SchemaField::new(
        "ObjectEffectModifierID",
        FieldType::UInt32,
    ));
    s.set_key_field("ID");
    s
}

fn read(chain: &mut Chain, table: &str) -> Result<Vec<u8>> {
    let path = format!("DBFilesClient\\{table}.dbc");
    chain
        .read_file(&path)
        .with_context(|| format!("reading {path}"))
}

/// Load the package tables and `CreatureDisplayInfo`'s package column off the patch chain.
pub fn load_object_effects(chain: &mut Chain) -> Result<ObjectEffectCatalog> {
    let tables = Tables {
        packages: read(chain, "ObjectEffectPackage")?,
        groups: read(chain, "ObjectEffectGroup")?,
        elems: read(chain, "ObjectEffectPackageElem")?,
        effects: read(chain, "ObjectEffect")?,
        displays: read(chain, "CreatureDisplayInfo")?,
    };
    catalog_from(&tables)
}

struct Tables {
    packages: Vec<u8>,
    groups: Vec<u8>,
    elems: Vec<u8>,
    effects: Vec<u8>,
    displays: Vec<u8>,
}

fn ids(bytes: &[u8], schema: Schema, what: &str) -> Result<HashSet<u32>> {
    let rs = parse(bytes, schema, what)?;
    Ok(rs.records().iter().filter_map(|r| u32_at(r, 0)).collect())
}

fn catalog_from(t: &Tables) -> Result<ObjectEffectCatalog> {
    let mut packages: HashMap<u32, HashMap<u32, Vec<u32>>> =
        ids(&t.packages, package_schema(), "ObjectEffectPackage")?
            .into_iter()
            .map(|id| (id, HashMap::new()))
            .collect();
    let mut groups: HashMap<u32, EffectGroup> =
        ids(&t.groups, group_schema(), "ObjectEffectGroup")?
            .into_iter()
            .map(|id| {
                let group = EffectGroup {
                    id,
                    ..Default::default()
                };
                (id, group)
            })
            .collect();

    // An element of a missing package is skipped; one naming a missing group is dropped by the
    // join, where the reference would link a null group (`0x6f66b0`).
    let rs = parse(&t.elems, package_elem_schema(), "ObjectEffectPackageElem")?;
    for r in rs.records() {
        let col = |i| u32_at(r, i).unwrap_or(0);
        let (package, group, state) = (col(1), col(2), col(3));
        let Some(states) = packages.get_mut(&package) else {
            continue;
        };
        states.entry(state).or_default().push(group);
    }

    let rs = parse(&t.effects, effect_schema(), "ObjectEffect")?;
    for r in rs.records() {
        let col = |i| u32_at(r, i).unwrap_or(0);
        let effect = ObjectEffect {
            id: col(0),
            kind: col(5),
            rec_id: col(6),
        };
        let Some(group) = groups.get_mut(&col(2)) else {
            continue;
        };
        match col(3) {
            1 => group.enter.push(effect),
            2 => group.looping.push(effect),
            3 => group.exit.push(effect),
            _ => {}
        }
    }

    let rs = parse(
        &t.displays,
        creature_display_info_schema(&LAYOUTS.creature_display_info),
        "CreatureDisplayInfo",
    )?;
    let display_packages = rs
        .records()
        .iter()
        .filter_map(|r| Some((u32_at(r, 0)?, u32_at(r, DISPLAY_PACKAGE_COLUMN)?)))
        .filter(|&(_, package)| package != 0)
        .collect();

    Ok(ObjectEffectCatalog::from_parts(
        display_packages,
        packages,
        groups.into_values(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WDBC file of four-byte columns and an empty string block.
    fn dbc(cols: usize, rows: &[Vec<u32>]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"WDBC");
        for v in [rows.len(), cols, cols * 4, 1] {
            out.extend_from_slice(&(v as u32).to_le_bytes());
        }
        for row in rows {
            assert_eq!(row.len(), cols);
            row.iter()
                .for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
        }
        out.push(0);
        out
    }

    /// An `ObjectEffect` row: id, group, trigger, sound kit.
    fn effect(id: u32, group: u32, trigger: u32, kit: u32) -> Vec<u32> {
        vec![id, 0, group, trigger, 0, EFFECT_SOUND, kit, 0, 0, 0, 0, 0]
    }

    fn display(id: u32, package: u32) -> Vec<u32> {
        let mut row = vec![0; 16];
        row[0] = id;
        row[DISPLAY_PACKAGE_COLUMN] = package;
        row
    }

    fn synthetic() -> ObjectEffectCatalog {
        catalog_from(&Tables {
            packages: dbc(2, &[vec![122, 0]]),
            groups: dbc(2, &[vec![202, 0], vec![204, 0]]),
            elems: dbc(
                4,
                &[
                    vec![1, 122, 202, 38],
                    vec![2, 122, 204, 37],
                    // A missing group and a missing package are dropped.
                    vec![3, 122, 999, 37],
                    vec![4, 77, 202, 38],
                ],
            ),
            effects: dbc(
                12,
                &[
                    effect(1, 202, 2, 13883),
                    effect(2, 204, 2, 13885),
                    effect(3, 204, 1, 14719),
                    effect(4, 204, 3, 1),
                    effect(5, 204, 4, 2),
                ],
            ),
            displays: dbc(16, &[display(25092, 122), display(1, 0)]),
        })
        .unwrap()
    }

    #[test]
    fn a_display_reaches_its_states_groups_and_effects() {
        let cat = synthetic();
        assert_eq!(cat.package_for_display(25092), Some(122));
        assert_eq!(cat.package_for_display(1), None);
        assert!(cat.has_package(122) && !cat.has_package(77));
        let moving: Vec<_> = cat.state_groups(122, 37).collect();
        assert_eq!(moving.len(), 1, "the missing group 999 is dropped");
        let g = moving[0];
        assert_eq!(
            g.looping.iter().map(|e| e.sound_kit()).collect::<Vec<_>>(),
            [Some(13885)]
        );
        assert_eq!(g.enter[0].sound_kit(), Some(14719));
        assert_eq!(g.exit[0].id, 4);
        assert!(cat.state_groups(122, 1).next().is_none());
        assert_eq!(cat.package_groups(122).count(), 2);
    }

    /// The Isle of Conquest siege engines: the Demolisher's displays and the Catapult's.
    #[test]
    fn real_siege_engine_packages() {
        let data = crate::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let cat = load_object_effects(&mut chain).expect("object effect tables");
        assert_eq!(cat.len(), 29, "every ObjectEffectPackage row");
        for display in [25092, 27655, 27658] {
            assert_eq!(cat.package_for_display(display), Some(122));
        }
        assert_eq!(cat.package_for_display(23884), Some(81));
        let kits = |package, state, pick: fn(&EffectGroup) -> &Vec<ObjectEffect>| {
            cat.state_groups(package, state)
                .flat_map(|g| pick(g).iter().filter_map(|e| e.sound_kit()))
                .collect::<Vec<_>>()
        };
        assert_eq!(kits(122, 38, |g| &g.looping), [13883], "DemolisherIdle");
        assert_eq!(kits(122, 37, |g| &g.looping), [13885], "DemolisherRunWalk");
        assert_eq!(kits(122, 37, |g| &g.enter), [14719], "the gear shift");
        assert_eq!(kits(122, 8, |g| &g.enter), [13880], "the launch");
        assert_eq!(kits(122, 1, |g| &g.looping), [12748]);
        assert_eq!(
            kits(81, 38, |g| &g.looping),
            [13812],
            "ForsakenCatapultStand"
        );
        assert_eq!(
            kits(81, 37, |g| &g.looping),
            [13810],
            "ForsakenCatapultRunWalk"
        );
    }
}
