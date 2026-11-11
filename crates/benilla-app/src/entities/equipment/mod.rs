//! Equipment visuals: `resolve` decides what each unit holds and wears and where, and `spawn` hangs
//! the item models from the body's attachment joints (the reference's attach install). A creature
//! carries `Item.dbc` ids (`UNIT_VIRTUAL_ITEM_SLOT_ID`), a player item entries
//! (`PLAYER_VISIBLE_ITEM_*`), resolved through the templates as the reference's ItemCache does.

use std::collections::HashMap;

use benilla_formats::ItemDisplayCatalog;
use bevy::prelude::*;

use super::{DisplayModel, ModelHandle};
use benilla_assets::m2_url;

mod resolve;
pub(in crate::entities) use resolve::placement;
pub(crate) use resolve::DressKey;
pub(super) use resolve::{resolve_corpse_equipment, resolve_equipment};
mod spawn;
pub(super) use spawn::attach_held_items;
mod swap;
mod wait;
pub(super) use wait::warn_unsettled_equipment;

/// The held-item descriptor slots, in vmangos `WeaponAttackType` order: mainhand, offhand, ranged.
const HELD_SLOTS: usize = 3;

/// The held items' equipment slots, which index the `PLAYER_VISIBLE_ITEM_*` blocks.
const PLAYER_HELD_SLOTS: [u8; HELD_SLOTS] = [15, 16, 17];

/// M2 attachment ids. A stowed item's follows the reference's stow pick: K - 1 for the mainhand
/// and K for the offhand, K = 27 two-hander, 31 staff, 33 one-hander; 28 for a shield.
pub(crate) mod attach_id {
    /// Left forearm: a drawn shield.
    pub(crate) const SHIELD: u16 = 0;
    pub(crate) const SHOULDER_RIGHT: u16 = 5;
    pub(crate) const SHOULDER_LEFT: u16 = 6;
    pub(crate) const HELM: u16 = 11;
    /// The drawn mainhand, and a drawn gun, crossbow, wand or thrown weapon.
    pub(crate) const HAND_RIGHT: u16 = 1;
    /// A drawn offhand other than a shield, and a drawn bow.
    pub(crate) const HAND_LEFT: u16 = 2;
    pub(crate) const BACK_RIGHT: u16 = 26;
    pub(crate) const BACK_LEFT: u16 = 27;
    pub(crate) const SHIELD_BACK: u16 = 28;
    pub(crate) const BACK_LOWER_MAIN: u16 = 30;
    pub(crate) const BACK_LOWER_OFF: u16 = 31;
    /// The mainhand's hip is the left one.
    pub(crate) const HIP_MAIN: u16 = 32;
    pub(crate) const HIP_OFF: u16 = 33;
    /// HandArrow, the nocked arrow's one body attach (id `0x23` on the body, in the bow draw and
    /// the `$BWP` handler). The 0x18/0x19 the item-model loader takes pick a model directory, not
    /// an attach.
    pub(crate) const HAND_ARROW: u16 = 0x23;
    /// The worn quiver's attachment, the stowed mainhand two-hander's point.
    pub(crate) const QUIVER: u16 = 26;
}

/// The `ItemDisplayInfo.dbc` catalog and the item model cache; absent if the DBC fails to load.
#[derive(Resource)]
pub(crate) struct ItemDisplays {
    pub(crate) catalog: ItemDisplayCatalog,
    /// Keyed by model kind too: a helm display has a file per race and sex, a shoulder a pair.
    pub(super) models: HashMap<(u32, ItemModelKind), DisplayModel>,
    /// ChrRaces' `ClientPrefix` by race id, the helm model's race suffix (`0x4e7800`).
    pub(crate) helm_prefixes: HashMap<u8, String>,
    /// `Item.dbc` by entry, which a creature's held items and the bag icon read.
    pub(crate) item_dbc: HashMap<u32, benilla_formats::ItemDbcRow>,
}

/// The icon no item or display names (`0x70a9ef`).
pub(crate) const FALLBACK_ITEM_ICON: &str = r"Interface\Icons\INV_Misc_QuestionMark";

impl ItemDisplays {
    /// A carried item's icon (`0x70aa00`, `0x70a910`): `Item.dbc`'s display, display 0 with no
    /// row, ending on [`FALLBACK_ITEM_ICON`]. No template is read.
    pub(crate) fn carried_icon(&self, entry: u32) -> String {
        let display = self.item_dbc.get(&entry).map_or(0, |r| r.display);
        self.catalog
            .get(display)
            .and_then(|d| d.icon.clone())
            .unwrap_or_else(|| FALLBACK_ITEM_ICON.to_string())
    }
}

#[cfg(test)]
impl ItemDisplays {
    pub(crate) fn icons_for_tests(catalog: ItemDisplayCatalog) -> Self {
        ItemDisplays {
            catalog,
            models: HashMap::new(),
            helm_prefixes: HashMap::new(),
            item_dbc: HashMap::new(),
        }
    }
}

/// A player's worn display ids, 0 for none (an NPC's armor is baked into its display);
/// `bodyslots` by bodyslot - 2, shirt to tabard. `settled` holds once every worn entry has a
/// template answer, so the first composite is dressed rather than naked for a frame.
#[derive(Component, Clone, Copy, PartialEq, Eq, Default, Debug)]
#[component(storage = "SparseSet")]
pub(crate) struct Equipment {
    pub(crate) bodyslots: [u32; 8],
    /// A geoset and a runtime cape texture, no body region.
    pub(crate) cloak: u32,
    /// An attach model plus the masks that hide hair, facial hair and ears.
    pub(crate) helm: u32,
    /// The guild emblem off `PLAYER_GUILDID`; `None` for no guild, or until its query answers.
    pub(crate) emblem: Option<benilla_formats::GuildEmblem>,
    /// The tabard designer's preview on our own body (`[cc+0xc]`).
    pub(crate) tabard_preview: bool,
    /// A player's look in dial order (skin, face, hair style, hair colour, facial hair): the
    /// `PLAYER_BYTES` fields, or on our own body the barber shop's preview. A change re-dresses the
    /// body in place, as each field's update handler resets the component (`0x6df530`).
    pub(crate) look: Option<[u8; 5]>,
    pub(crate) settled: bool,
}

/// The [`Equipment`] a player's visual was dressed with; a change re-dresses it in place.
#[derive(Component)]
#[component(storage = "SparseSet")]
pub(in crate::entities) struct AppliedEquipment(pub(in crate::entities) Equipment);

/// An NPC's worn displays off its `CreatureDisplayInfoExtra` row, as its component adds them
/// (`0x730210`): helm, the eight composite slots and the cape.
pub(in crate::entities) fn npc_equipment(npc: &benilla_formats::NpcAppearance) -> Equipment {
    Equipment {
        bodyslots: std::array::from_fn(|i| npc.equipment[i + 2]),
        cloak: npc.cape,
        helm: npc.equipment[0],
        settled: true,
        ..Default::default()
    }
}

/// A visual rebuilt for a mount or display swap, not a spawn: it skips the appear-fade.
#[derive(Component)]
pub(super) struct Reattached;

/// The armor composite's equipment slots and their `Equipment::bodyslots` index.
const COMPOSITE_SLOTS: [(u8, usize); 8] = [
    (3, 0),
    (4, 1),
    (5, 2),
    (6, 3),
    (7, 4),
    (8, 5),
    (9, 6),
    (18, 7),
];

/// The item models a unit shows this frame, in [`ATTACH_SLOT_NAMES`] order.
#[derive(Component, Default, Clone, PartialEq, Eq)]
#[component(storage = "SparseSet")]
pub(super) struct HeldItems {
    slots: [Option<HeldSlot>; ATTACH_SLOTS],
}

pub(super) const ATTACH_SLOTS: usize = 8;

/// What the disarm reflex left attached: the reference builds attachments from events, and this is
/// the one such state kept across frames. The reference's disarm handlers run on any unit whose
/// disarm bits change (`0x73c330` main flag, `0x73c5d0` off-hand and ranged flags); as a hand's
/// bit rises, a drawn item leaves the hand (attach `0xf`) and a stowed one stays, and no attach
/// site moves it while the unit's weapon getter reads NULL.
#[derive(Component, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct DisarmFreeze {
    /// Per held slot (main, off, ranged): the item hidden at that hand's edge, and where the reflex
    /// left it (`None` when it was detached from the hand).
    hands: [Option<(u32, Option<u16>)>; 3],
}

impl DisarmFreeze {
    /// Hold `display` in `slot` at `attach`, at that hand's rising edge.
    pub(super) fn hold(&mut self, slot: usize, display: u32, attach: Option<u16>) {
        self.hands[slot] = Some((display, attach));
    }

    /// Drop `slot`'s hold, at its falling edge.
    pub(super) fn release(&mut self, slot: usize) {
        self.hands[slot] = None;
    }

    /// Where `display` still hangs in `slot`; `None` for an unheld hand or another item.
    pub(super) fn attach_for(&self, slot: usize, display: u32) -> Option<u16> {
        self.hands[slot]
            .filter(|&(d, _)| d == display)
            .and_then(|(_, a)| a)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.hands.iter().all(Option::is_none)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct HeldSlot {
    display: u32,
    kind: ItemModelKind,
    attach: u16,
    /// The `ItemVisuals.dbc` glow, the display's own else its first enchant's; usually 0.
    visual: i32,
}

impl HeldSlot {
    /// The same model wherever it hangs: a change of attach point alone is a move, not a rebuild.
    fn same_item(&self, other: &Self) -> bool {
        self.display == other.display && self.kind == other.kind && self.visual == other.visual
    }
}

/// Which of an item display's models a slot shows; `ensure_item_model` maps each to its file.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum ItemModelKind {
    Weapon,
    Shield,
    ShoulderLeft,
    ShoulderRight,
    Helm {
        race: u8,
        sex: u8,
    },
    /// A nocked-ammo model: a display with `model[0]` is a thrown weapon under `Weapon\`, one with
    /// only `model[1]` an arrow or bullet under `Ammo\`.
    Ammo,
    /// The worn ammo container, on the back while the ranged weapon is drawn.
    Quiver,
}

/// A body model's attachment points and event markers, each a bone and a Bevy-space offset from
/// its bind pivot; the joint entities are `RigPose::anchor_for`'s.
#[derive(Component)]
#[component(storage = "SparseSet")]
pub(crate) struct BoneAttach {
    pub(crate) points: HashMap<u16, (u16, Vec3)>,
    /// First record per FourCC: the missile launch points `$CSL`/`$CSR`/`$CST` and `$BWR`.
    pub(crate) markers: HashMap<[u8; 4], (u16, Vec3)>,
}

/// The item models spawned for a unit: the [`HeldItems`] they were built from and each slot's root.
#[derive(Component, Default)]
#[component(storage = "SparseSet")]
pub(crate) struct HeldAttached {
    applied: HeldItems,
    spawned: [Option<Entity>; ATTACH_SLOTS],
}

impl HeldAttached {
    /// The spawned root per attach slot: the models actually on the body, not the resolution.
    pub(crate) fn spawned_slots(&self) -> &[Option<Entity>; ATTACH_SLOTS] {
        &self.spawned
    }

    #[cfg(test)]
    pub(crate) fn with_spawned(spawned: [Option<Entity>; ATTACH_SLOTS]) -> Self {
        Self {
            applied: HeldItems::default(),
            spawned,
        }
    }
}

impl HeldAttached {
    /// The spawned item model on the right hand, attachment 1, as the effect walk finds it
    /// (`0x71a8a3`).
    fn right_hand(&self) -> Option<(u32, ItemModelKind)> {
        self.applied
            .slots
            .iter()
            .zip(&self.spawned)
            .find_map(|(slot, root)| {
                slot.filter(|s| s.attach == attach_id::HAND_RIGHT && root.is_some())
            })
            .map(|s| (s.display, s.kind))
    }
}

/// Index 13's point (`0x71a8d0`): the right-hand model's first attachment of 4 down to 0, as the
/// body's bone and an offset in its frame; `None` with no model there or none of those.
fn held_weapon_point(
    bones: &BoneAttach,
    held: &HeldAttached,
    items: &ItemDisplays,
) -> Option<(u16, Vec3)> {
    let item = items.models.get(&held.right_hand()?)?;
    let &(bone, seat) = bones.points.get(&attach_id::HAND_RIGHT)?;
    // The item root hangs unrotated at the hand's seat, so its bind-pose points add to it.
    let at = (0..=4u16)
        .rev()
        .find_map(|id| crate::portrait::attachment_point(&item.skeleton, &item.attachments, id))?;
    Some((bone, seat + at))
}

/// What a unit effect attachment reads beyond the body: the held models and the item cache.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct HeldWeapons<'w, 's> {
    held: Query<'w, 's, &'static HeldAttached>,
    items: Option<Res<'w, ItemDisplays>>,
}

impl HeldWeapons<'_, '_> {
    /// A unit effect attachment's bone and offset (`0x71a860`): index 13 the held weapon's point,
    /// else the body's tag; `None` when the body lacks that tag, the test before it (`0x71a7f0`).
    pub(crate) fn effect_point(
        &self,
        unit: Entity,
        bones: &BoneAttach,
        attach: benilla_formats::EffectAttach,
    ) -> Option<(u16, Vec3)> {
        let tagged = bones.points.get(&attach.tag()).copied()?;
        if attach != benilla_formats::EffectAttach::HeldWeapon {
            return Some(tagged);
        }
        let weapon = self.held.get(unit).ok().zip(self.items.as_deref());
        Some(
            weapon
                .and_then(|(held, items)| held_weapon_point(bones, held, items))
                .unwrap_or(tagged),
        )
    }
}

/// The attach slots' names, in [`HeldAttached::spawned_slots`] order.
pub(crate) const ATTACH_SLOT_NAMES: [&str; ATTACH_SLOTS] = [
    "main", "off", "ranged", "helm", "shL", "shR", "ammo", "quiver",
];

/// The glow of a slot the reference never lights: the helm, shoulder and quiver builders pass a
/// literal 0; only the hand attach and the ranged and ammo builder pass the display's visual.
const NO_GLOW: i32 = 0;

/// [`ensure_item_model`] through the resource's change-detected handle: a model already present is
/// found by a plain read, so `ItemDisplays` reads as changed only when a model is added.
pub(in crate::entities) fn ensure_item_model_mut(
    held: &mut ResMut<ItemDisplays>,
    display: u32,
    kind: ItemModelKind,
    asset_server: &AssetServer,
) {
    if held.models.contains_key(&(display, kind)) {
        return;
    }
    ensure_item_model(held, display, kind, asset_server);
}

/// Ensure `display` has a [`DisplayModel`] for `kind`; its texture is the BLP the display row
/// names, never derived from the model name.
pub(in crate::entities) fn ensure_item_model(
    held: &mut ItemDisplays,
    display: u32,
    kind: ItemModelKind,
    asset_server: &AssetServer,
) {
    if held.models.contains_key(&(display, kind)) {
        return;
    }
    let (dir_name, col) = match kind {
        ItemModelKind::Weapon => ("Weapon", 0),
        ItemModelKind::Shield => ("Shield", 0),
        ItemModelKind::ShoulderLeft => ("Shoulder", 0),
        ItemModelKind::ShoulderRight => ("Shoulder", 1),
        ItemModelKind::Helm { .. } => ("Head", 0),
        ItemModelKind::Ammo => {
            if held
                .catalog
                .get(display)
                .is_some_and(|d| d.model[0].is_some())
            {
                ("Weapon", 0)
            } else {
                ("Ammo", 1)
            }
        }
        ItemModelKind::Quiver => ("Quiver", 0),
    };
    let dir = format!("Item\\ObjectComponents\\{dir_name}");
    let dm = match held.catalog.get(display) {
        Some(d) if d.model[col].is_some() => 'model: {
            let mut model = d.model[col].clone().unwrap();
            if let ItemModelKind::Helm { race, sex } = kind {
                match helm_file(&held.helm_prefixes, &model, race, sex) {
                    Some(m) => model = m,
                    None => break 'model super::empty_display(),
                }
            }
            let disp_id = display;
            debug!(
                "item model display {disp_id} → {dir}\\{model} (tex {:?})",
                d.model_texture[col]
            );
            DisplayModel {
                handle: ModelHandle::M2(asset_server.load(m2_url(&format!("{dir}\\{model}")))),
                // The runtime object skin, bound to the model's type-2 batches.
                object_texture: d.model_texture[col].clone(),
                particle_colors: d.particle_colors,
                dir,
                ..super::empty_shell()
            }
        }
        _ => super::empty_display(),
    };
    held.models.insert((display, kind), dm);
}

/// A helm's per-race file, `<model>_<ClientPrefix><M|F>` (`0x4e7800`, decorations `[0xac46a0]`);
/// `None` for a race with no ChrRaces row, which wears none (`0x4eefa0`).
fn helm_file(prefixes: &HashMap<u8, String>, model: &str, race: u8, sex: u8) -> Option<String> {
    let prefix = prefixes.get(&race)?;
    let letter = if sex == 1 { 'F' } else { 'M' };
    let stem = model.strip_suffix(".m2").unwrap_or(model);
    Some(format!("{stem}_{prefix}{letter}.m2"))
}

#[cfg(test)]
mod item_display_tests {
    use bevy::prelude::*;

    use super::super::display::empty_display;
    use super::{ensure_item_model_mut, helm_file, ItemDisplays, ItemModelKind};

    /// The helm's race suffix is the race's ChrRaces prefix, so a Blood Elf wears its own file
    /// rather than the eighth race's; a race without a row wears none.
    #[test]
    fn a_helm_file_takes_the_races_client_prefix() {
        let prefixes = std::collections::HashMap::from([(10, "Be".to_string())]);
        assert_eq!(
            helm_file(&prefixes, "Helm_A.m2", 10, 1).as_deref(),
            Some("Helm_A_BeF.m2")
        );
        assert_eq!(helm_file(&prefixes, "Helm_A.m2", 11, 0), None);
    }

    /// What each frame's resolver saw of the resource's change flag.
    #[derive(Resource, Default)]
    struct Seen(Vec<bool>);

    fn resolve_present(
        mut held: ResMut<ItemDisplays>,
        assets: Res<AssetServer>,
        mut seen: ResMut<Seen>,
    ) {
        ensure_item_model_mut(&mut held, 900, ItemModelKind::Weapon, &assets);
        seen.0.push(held.is_changed());
    }

    /// A resolver asking for a model already present reads it, so `ItemDisplays` reads as changed
    /// only when a model is added: the settled-wearer skip in `attach_held_items` keys on it.
    #[test]
    fn a_present_item_model_leaves_item_displays_unchanged() {
        let mut displays = ItemDisplays::icons_for_tests(
            benilla_formats::ItemDisplayCatalog::from_displays(std::collections::HashMap::new()),
        );
        displays
            .models
            .insert((900, ItemModelKind::Weapon), empty_display());
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        app.insert_resource(displays);
        app.init_resource::<Seen>();
        app.add_systems(Update, resolve_present);
        for _ in 0..3 {
            app.update();
        }
        let seen = &app.world().resource::<Seen>().0;
        assert_eq!(seen.len(), 3);
        assert!(
            !seen[1] && !seen[2],
            "a present model marks nothing after the insert frame: {seen:?}"
        );
    }
}

#[cfg(test)]
mod held_weapon_tests {
    use std::collections::HashMap;

    use benilla_formats::EffectAttach;
    use bevy::ecs::system::SystemState;
    use bevy::prelude::*;

    use super::super::display::empty_display;
    use super::{
        attach_id, BoneAttach, HeldAttached, HeldItems, HeldSlot, HeldWeapons, ItemDisplays,
        ItemModelKind, ATTACH_SLOTS, NO_GLOW,
    };

    const SWORD: u32 = 7;
    const HAND: (u16, Vec3) = (5, Vec3::new(0.1, 0.0, 0.0));
    const SPELL_HAND: (u16, Vec3) = (6, Vec3::new(0.0, 0.2, 0.0));

    /// A weapon model whose one bone sits 1 yd up, with attachments at `ids`, each `id` yd along x.
    fn sword(ids: &[u16]) -> ItemDisplays {
        let mut displays = ItemDisplays::icons_for_tests(
            benilla_formats::ItemDisplayCatalog::from_displays(HashMap::new()),
        );
        let mut dm = empty_display();
        dm.skeleton = benilla_assets::ModelSkeleton {
            joints: vec![benilla_assets::ModelJoint {
                parent: -1,
                local_translation: Vec3::Y,
                billboard: None,
                parent_arm: None,
            }],
            spine_bone: None,
            head_bone: None,
        };
        dm.attachments = ids
            .iter()
            .map(|&id| benilla_assets::ModelAttachment {
                id,
                bone: 0,
                offset: Vec3::X * f32::from(id),
            })
            .collect();
        displays.models.insert((SWORD, ItemModelKind::Weapon), dm);
        displays
    }

    /// The sword drawn in the right hand, or hung at `attach`; `None` for an empty hand.
    fn held(attach: Option<u16>) -> HeldAttached {
        let mut applied = HeldItems::default();
        let mut spawned = [None; ATTACH_SLOTS];
        if let Some(attach) = attach {
            applied.slots[0] = Some(HeldSlot {
                display: SWORD,
                kind: ItemModelKind::Weapon,
                attach,
                visual: NO_GLOW,
            });
            spawned[0] = Some(Entity::PLACEHOLDER);
        }
        HeldAttached { applied, spawned }
    }

    fn point(
        displays: ItemDisplays,
        held: HeldAttached,
        attach: EffectAttach,
        body: &BoneAttach,
    ) -> Option<(u16, Vec3)> {
        let mut world = World::new();
        world.insert_resource(displays);
        let unit = world.spawn(held).id();
        let mut state = SystemState::<HeldWeapons>::new(&mut world);
        state.get(&world).effect_point(unit, body, attach)
    }

    fn body() -> BoneAttach {
        BoneAttach {
            points: HashMap::from([(attach_id::HAND_RIGHT, HAND), (0x16, SPELL_HAND)]),
            markers: HashMap::new(),
        }
    }

    /// Index 13 takes the right-hand model's highest attachment of 4 down to 0 at the hand's seat
    /// (`0x71a8d0`); a model without any, a sheathed weapon and an empty hand fall back to the
    /// right spell hand, and a body without that point to nothing (`0x71a7f0`).
    #[test]
    fn index_13_walks_the_held_weapons_attachments_down_to_the_spell_hand() {
        let drawn = Some(attach_id::HAND_RIGHT);
        assert_eq!(
            point(
                sword(&[1, 3, 6]),
                held(drawn),
                EffectAttach::HeldWeapon,
                &body()
            ),
            Some((HAND.0, HAND.1 + Vec3::new(3.0, 1.0, 0.0))),
            "attachment 3: 6 is past the walk"
        );
        assert_eq!(
            point(sword(&[0]), held(drawn), EffectAttach::HeldWeapon, &body()),
            Some((HAND.0, HAND.1 + Vec3::Y)),
            "attachment 0 is the walk's last"
        );
        for (case, displays, held) in [
            ("no weapon attachment", sword(&[5, 11]), held(drawn)),
            ("sheathed", sword(&[3]), held(Some(attach_id::HIP_MAIN))),
            ("empty hand", sword(&[3]), held(None)),
        ] {
            assert_eq!(
                point(displays, held, EffectAttach::HeldWeapon, &body()),
                Some(SPELL_HAND),
                "{case}"
            );
        }
        let mut handless = body();
        handless.points.remove(&0x16);
        assert_eq!(
            point(
                sword(&[3]),
                held(drawn),
                EffectAttach::HeldWeapon,
                &handless
            ),
            None,
            "the test reads the spell hand first"
        );
        assert_eq!(
            point(sword(&[3]), held(drawn), EffectAttach::Point(0x16), &body()),
            Some(SPELL_HAND),
            "any other index is the body's own point"
        );
    }
}
